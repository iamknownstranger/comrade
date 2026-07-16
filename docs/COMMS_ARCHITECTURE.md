# Comrade — Communications Architecture & Delivery Roadmap

_Status: adopted working plan for the comms track, authored 2026-07-16 against main @
`8e47a67` (workspace tests green). This document proposes; the owner decisions in §4 are
open and not claimed as signed off. For anything comms-related, read this first —
`AUDIT.md` remains the repo-wide audit ledger and decision log. Owner priority
(2026-07-16, verbatim): "We'd like to get the communication features like voice calling,
messages, having a statelayer and video calling as priorities as soon as possible."_

---

## 1. Executive Summary

### Where comms actually stands (honest)

**Messaging is essentially done and better than the README claimed.** NIP-44 + NIP-17/NIP-59
gift-wrapped DMs work both directions on both platforms, with replies (NIP-10), monotonic
delivered/read receipts, message-request gating, durable offline backfill (a widening
`vault_last_seen_at` watermark), media attachments incl. voice notes, and UPI intent
detection. The remaining messaging gaps are **UX/scale, not protocol**: there is no
pagination anywhere (`conversations()` and `messages_with()` decrypt the *entire* DM store on
every call — `runtime.rs:988` calls `all_messages()`, `runtime.rs:1026` calls
`messages_with()` which full-scans `vault_cache` at `repository.rs:264-272`), and Android
notification taps don't deep-link into a conversation.

**Voice and video calling is real and, on Android, mature** — contrary to the README's stale
"follow-up" framing. Android has voice+video with Camera2 capture, a tap-to-swap self-view
tile, CallStyle notifications, full-screen intent, lock-screen ringing, Bluetooth/wired
routing, SAS display, connection-quality indicators, call history, and — critically — a
**battle-hardened, unit-tested call-session state machine** (`CallManager.kt`, ~1876 lines;
pure decision functions `decideAnswer`/`decideOfferForExistingSession`/`decideGlare`/
`decideConnectionStateAction` at `CallManager.kt:1629-1700`), plus caller-side STUN→TURN
fallback via ICE-restart re-offer.

**Desktop calling is the weak link, and it has a confirmed cross-platform P0.** The signaling
wire protocol (`comrade_core::call`) is pure, framework-free, and 30-tests solid — that is why
signaling is reliable. But the desktop frontend that drives it (`desktop/ui/main.js`) is a
weaker, already-diverged copy of Android's call logic, and reading it end-to-end confirms two
call-breaking defects (see ADR-3): it **Busies any offer that arrives while a call exists**
(`main.js:996`), which rejects an Android caller's same-`call_id` TURN re-offer and thereby
**breaks cross-platform TURN fallback**, and it uses **STUN-only on the caller side with no
ICE-restart** while the callee uses STUN+TURN — an asymmetry that makes desktop unreachable on
CGNAT/symmetric-NAT.

**The "state layer" the owner named is three distinct deficits, not one missing store:**
(1) Rust has no observable projection — the frontends pull `conversations()`/`messages_with()`
on demand and decrypt everything each time; (2) Android drives all UI off two **global
`StateFlow<Int>` tick counters** (`ChatEventRouter`, `RelayConnectionService.kt:204-232`) so
every screen does `LaunchedEffect(tick){ full refetch }` (`ChatsScreen.kt:178,183,497,929`);
(3) engine lifecycle is not owned by anyone, so Off-Grid mode is still a false privacy
assurance (AUDIT A1). There is **no metrics framework anywhere** — a gap against the org
standard that apps be observable.

### What we build next, in the owner's language

1. **Voice (call reliability first).** Make desktop calling as reliable as Android and fix the
   cross-platform interop bug so a call connects regardless of which side placed it and which
   NAT it's behind. This is Wave 1, P0.
2. **State layer.** Replace tick-driven full-refetch with **runtime-owned observable
   projections**: fine-grained per-conversation events (adding the one missing
   `OutgoingDirectMessage` event) plus a cursor-paginated read API, consumed by a thin Android
   repository/ViewModel. redb stays the single source of truth (ADR-1). Wave 2.
3. **Messages (completion).** Pagination, notification deep-linking, and test coverage for the
   currently-untested event plumbing. Wave 3.
4. **Video (polish).** Mid-call renegotiation semantics, bandwidth/resolution constraints, and
   desktop camera-toggle parity — landed *after* the shared decision layer exists so we don't
   deepen the desktop/Android divergence. Wave 4.

The sequencing rule is deliberate: **reliability before richness, and testability before
convergence.** We fix the shipped call bug first, put new logic where CI can actually test it
(the Rust workspace lane), and only converge the two call state machines once desktop is safe
to lead.

---

## 2. Architecture Decisions

Each ADR is context → decision → alternatives → consequences. Claims are cited to
`file:line` where they are load-bearing.

### ADR-1 — State layer shape: on-demand observable projections over redb, not a materialized store

**Context.** The frontends assemble views ad hoc. `conversations()` (`runtime.rs:972-1017`)
decrypts *every* stored message via `all_messages()` to compute newest-per-peer;
`messages_with()` (`runtime.rs:1022-1045`) decrypts every message and filters by peer
(`repository.rs:264-272`, which calls `values()` → unseal-all at `lib.rs:171-183`). Android
then re-runs these on **every** `chatTick`/`requestTick` bump — the ticks are global
`StateFlow<Int>` (`RelayConnectionService.kt:219,232`) collected in `MainActivity.kt:267-268`,
driving `LaunchedEffect(chatTick){ full refetch }` across screens
(`ChatsScreen.kt:178,183,497,929`). Optimistic outgoing append works only by coincidence
because **no outgoing-DM `BridgeEvent` exists** (the `BridgeEvent` enum at `runtime.rs:521-551`
has `IncomingDirectMessage` but no `OutgoingDirectMessage`).

Crucially, the *data needed for fine-grained updates already flows* — it is only discarded at
the frontend. `MessageStatus` and `PeerProfileUpdated` carry an explicit `peer`
(`runtime.rs:537-544`); `DirectMessageDto` carries `sender` (`runtime.rs:170`);
`MediaMessageDto` carries `sender` (`runtime.rs:324`). Every DM-side event already identifies
its conversation.

**Decision.** **redb remains the single source of truth. We do NOT introduce a materialized,
mutable in-memory state store in Rust.** The "observable projection" is two complementary
mechanisms:

1. **Fine-grained per-conversation events.** Frontends consume the existing per-`peer` events
   to patch a single conversation-list row / append a single message, instead of refetching
   everything. We add the one missing variant, `OutgoingDirectMessage(MessageDto)`, emitted by
   `send_dm`/`send_dm_reply` after persistence, so the conversation-list preview and any other
   open view update on send (today the sender screen has the DTO in hand but the list screen
   goes stale until the next *incoming* event bumps the global tick).
2. **A cursor-paginated read API.** `messages_with_page(peer, before: Option<MessageCursor>,
   limit) -> MessagePageDto`, where `MessageCursor { created_at: u64, id: String }`
   (`created_at` alone is not unique; the event id breaks ties deterministically). Newest
   window first; `next` cursor pages older. `ConversationDto` gains an `unread` count derived
   from a per-conversation read watermark (extend `ConversationMeta` —
   `repository.rs:149-159` — with `last_read_at`; `mark_conversation_read` already exists to
   reset it).

The conversation list is still *computed on demand* (as today) but now recomputed only on cold
unlock, because events let the frontend keep it patched in place. Per-event cost drops from
O(all messages) to O(1) naturally, without holding any mutable cache.

**Why not a materialized store?** Because it buys little and costs two of the seams we must
respect. (a) *Consistency*: a mutable cache duplicated from redb invites dual-write divergence
bugs — every write path must remember to update both. (b) *Lock boundary*: `lock_vault` drops
engines/store/identity and state must not outlive the key; a materialized store is one more
thing that must be torn down exactly, and a leak there is a privacy defect, not just a bug.
On-demand-over-redb inherits the store's lifecycle for free. The genuine cost of on-demand —
the O(N) cold-open decrypt scan for the conversation list — is paid **once per unlock**, and if
profiling shows it hurts, we add a plaintext composite-key index (WP14) so even the cold scan
is O(peers). That optimization does **not** require a materialized store; it stays redb-native.

**uniffi / symmetry.** New DTOs derive `uniffi::Record`, the new event is a `uniffi::Enum`
variant (`comrade_ui/src/lib.rs:31,74,98,105` shows the proc-macro surface — no `.udl`, no
JSON). `messages_with_page` is added to the runtime and registered **symmetrically** in both
bridges (`desktop/src-tauri/src/commands.rs` and `comrade_jni`) in the same WP, so desktop
cannot fall behind again the way it did on the call commands (ADR-3).

**Two-bus / lock-rule compliance.** Conversation/message projections are fed **only** from the
critical bus (`events`, cap 256, `runtime.rs:62`); the lossy feed bus (`feed_events`, cap 64,
`runtime.rs:70`) carries `IncomingChitthi` only and never touches DM/call state, so a public
feed flood cannot starve conversation updates (COMMS-04 preserved). The read API is a sync
`&self` query with no network `.await`, so it is safe under a briefly-held read guard; nothing
here changes the `RuntimeHandles` snapshot discipline (`runtime.rs:2022-2058`) that network
methods use.

**Consequences.** (+) No new consistency or teardown hazard; everything testable in the strong
Rust lane; frontends stop refetching. (−) Android must adopt a small repository/ViewModel to
consume per-conversation events (WP7) — real work, but incremental and mostly JVM-testable
reducer logic. (−) Cold-open scan cost remains until WP14; acceptable and measurable.

### ADR-2 — Call-session state convergence: shared *pure decision* layer, desktop-first, Android last-or-never

**Context.** Call-session state (ringing/connecting/active/ended, glare, re-offer disposition)
lives **twice**: Android's `CallManager` (hardened, with pure tested deciders at
`CallManager.kt:1629-1700`) and desktop `main.js` (a weaker copy). They have already diverged —
the desktop P0 in ADR-3 is a *direct consequence* of that divergence, because desktop's
`decideOfferForExistingSession`-equivalent logic is simply wrong. The seam is explicit: **do
not big-bang rewrite Android's `CallManager`** (two ANR fixes, glare, idempotency are encoded
in it), and prefer logic where CI can test it (Rust workspace = strong lane; desktop JS =
untested today; Android = JVM-only unit lane).

**Decision.** Converge the **decision table**, not the whole session machine, and do it as a
*pure, framework-free* module — the same discipline that makes `comrade_core::call` reliable.
Sequenced:

- **Now (Wave 1):** Extract desktop's offer/answer/glare/connection-state dispositions into a
  pure JS module (`desktop/ui/call_decisions.mjs`) with a `node --test` lane. The functions
  mirror Android's `decide*` signatures **and their test vectors**. This is the immediate P0
  fix *and* the testability story *and* the seed of convergence — the vectors become the
  cross-implementation conformance suite.
- **Later (Wave 4):** Lift the decision table into a pure Rust module
  (`comrade_core::call_session` or a `comrade_ui` submodule — wire-free, like `call.rs`),
  exercised by the *same* conformance vectors. **Desktop adopts it first** (swap the JS module
  body for thin FFI calls; the node vectors keep guarding behavior). **Android adopts later or
  never** — its Kotlin deciders already pass the shared vectors, so we treat the vectors as the
  contract and only migrate Kotlin if the maintenance cost of two implementations exceeds the
  risk of touching `CallManager`.

**Alternatives.** (a) *Move all session state to Rust now, both platforms.* Rejected: highest
rewrite risk against the one battle-hardened component; violates the "incremental, de-risked,
desktop-first" seam. (b) *Keep both frontend-owned with only a shared conformance suite, no
shared code.* Viable and low-risk, but leaves two hand-maintained state machines forever; we
take the conformance suite *now* (cheap) and keep the door open to shared Rust code once
desktop has proven it.

**Consequences.** (+) Divergence is capped by a conformance suite immediately; the P0 fix and
the convergence path are the same work, not competing. (+) Android is never destabilized on a
schedule it didn't ask for. (−) Two implementations coexist through Wave 3; acceptable because
the shared vectors make drift a test failure, not a field bug.

### ADR-3 — Desktop call reliability parity

**Context (all verified by reading `main.js` + the bridge end-to-end).**

- **P0 interop bug.** `onCallSignal` routes *any* `offer` to `handleIncomingOffer`
  (`main.js:1091-1094`) *before* the stray-signal `callId` guard at `main.js:1096`;
  `handleIncomingOffer` then sees `state.call` is set and **sends `busy` + logs a missed call +
  returns** (`main.js:996-1014`). So a re-offer for the **same** `call_id` — exactly what an
  Android caller sends when it falls back to TURN via ICE-restart — is answered `busy`. This
  **breaks Android-caller → desktop-callee TURN fallback**, and also precludes *any* mid-call
  renegotiation. Android already does this correctly:
  `decideOfferForExistingSession` (`CallManager.kt:1663-1669`) returns `RENEGOTIATE` when the
  incoming `call_id` equals the existing one and a peer connection exists, `DUPLICATE_NOOP`
  when it matches with no pc yet, and `BUSY` only for a *different* `call_id`.
- **No wire change needed.** `CallSignal::Offer { sdp }` carries no `ice_restart` flag
  (`call.rs:123`); a re-offer is identified purely by matching `call_id`. The dedup layer will
  not eat it — `CallSignalDedup` keys on the *wrapper event id* (`runtime.rs:2835`, cap 512 at
  `runtime.rs:2709`), and a re-offer is a fresh event; the 90s staleness gate
  (`CALL_SIGNAL_MAX_AGE_SECS`, `runtime.rs:2833`) passes for a freshly-sent re-offer.
- **ICE asymmetry + no fallback.** The caller uses `session.ice_servers` (`main.js:982`) from
  `place_call`, which is **STUN-only** by design (`runtime.rs:1341`,
  `call_ice_servers_for(StunOnly)`); the callee uses the `call_ice_servers` command
  (`main.js:1052`), which returns **STUN+TURN** (`runtime.rs:1230-1232`,
  `ice_servers_for(StunAndTurn, …)`). And `onconnectionstatechange` treats `"failed"` as
  terminal → `finishCall` with no ICE restart (`main.js:940-946`). Net: a desktop *caller*
  behind CGNAT can never relay, and never retries.
- **Missing commands.** The Tauri `invoke_handler` (`lib.rs:89-144`) registers only
  `call_ice_servers` and `set_turn_server`; it does **not** register `call_ice_servers_for`,
  `call_sas`, or `turn_server_status`, all of which already exist in the runtime
  (`runtime.rs:1250,1268,1310`). So the frontend cannot select a strategy, derive a SAS, or
  read TURN status even though the core supports all three.

**Decision.** Four concrete changes, in dependency order:

1. **Callee renegotiation (P0).** Split offer handling: an offer whose `call_id` matches the
   live session is a renegotiation → `setRemoteDescription` + `createAnswer` + send `answer`
   (reuse the existing pc); an offer with a *different* `call_id` while busy → `busy`; a fresh
   offer with no session → ring. This is `decideOfferForExistingSession` transcribed to JS.
2. **Caller STUN-first + ICE-restart/TURN fallback.** Register `call_ice_servers_for`; on the
   caller's first `connectionState === "failed"` (and not yet tried), fetch
   `call_ice_servers_for("stun_and_turn")`, `setConfiguration(...)`, and re-offer with
   `createOffer({ iceRestart: true })` — mirroring Android's `tryTurnFallbackOrFail`. Fix the
   asymmetry by making both sides converge on the same relayed configuration during fallback.
3. **SAS UI.** Register `call_sas`; render the 4-emoji SAS on the connected call screen, as
   Android does.
4. **Testability.** Extract the offer disposition + connection-state action + glare tiebreak
   into `desktop/ui/call_decisions.mjs` as pure functions and add a `node --test` CI lane
   (Node ≥ 20 ships the built-in runner — **no `package.json` and no npm deps**; CI pins via
   `actions/setup-node`; locally `node --test desktop/ui/*.test.mjs`). This is the ADR-3
   testability choice: **extract pure JS + node lane now**, converge to Rust later (ADR-2). We
   choose JS-now over Rust-now because the P0 must ship immediately and a JS extraction is a
   smaller, safer diff than a new FFI surface; the node vectors are reused verbatim when Rust
   convergence lands.

**Consequences.** (+) Desktop reaches call reliability parity and cross-platform fallback works.
(+) Desktop finally has a real, CI-gated test lane for call logic. (−) Real ICE/TURN traversal
still can't run in CI (no media in headless CI) — compensated by the Rust two-peer *signaling*
test (WP5) and manual scripts against `deploy/coturn` (§6). (−) Webview `RTCPeerConnection`
renegotiation/`setConfiguration` support is a real unknown → called out as a risk on WP3.

### ADR-4 — Engine lifecycle ownership (closes AUDIT A1 for the comms scope)

**Context.** `toggle_workspace` only flips the pure `comrade_state` enum; the engine
`disconnect` methods have **zero call sites** (A1), so Off-Grid/Travel keeps relay websockets
live while telling the user relays are paused — a false privacy assurance. `comrade_state` is
correctly pure and must **stay mode-only** (don't grow chat/call state into it). There is
already precedent for runtime-owned lifecycle: the Saathi mesh engine is started/stopped on the
fly by `ComradeRuntime` on workspace transitions (`runtime.rs:563-567`,
`sync_saathi_lifecycle`).

**Decision.** Make **`ComradeRuntime` the single engine-lifecycle owner**, mapping workspace
transitions to engine start/stop. For the comms track the *minimum* shippable slice is: on
transition into `OffGridTravel`, disconnect the Nostr engines (Sabha/Vault/Sakha) and abort
their subscription tasks (`feed_task`/`vault_task`/`sakha_sync_task`, already tracked at
`runtime.rs:583-593`); on transition back, reconnect/re-subscribe. The change must be
**netstat-visible** (an integration test asserting no relay socket remains after the toggle)
and must respect the lock rule (disconnect runs off a `RuntimeHandles` snapshot, not under a
held guard across the await).

**Alternatives.** Leave lifecycle to each frontend (rejected — that is how divergence and A1
happened); a separate `LifecycleManager` type (rejected for now — `ComradeRuntime` already owns
the engine handles and the Saathi precedent; a new owner adds indirection without benefit).

**Consequences.** (+) Off-Grid becomes an honest privacy state, testably. (+) Establishes the
single lifecycle owner AUDIT A4/M2-4 want (dep OQ2 — relay-config ownership — is noted, not
solved here). (−) Must not regress reconnect latency on switch-back; covered by test.

### ADR-5 — Observability: local-first metrics, on-device only

**Context.** Zero metrics framework exists (no prometheus/OTel/metrics dep; tracing logs only).
The org standard requires apps be instrumented so performance is observable. The product is
privacy-first: **no third-party telemetry by default**, and any off-device export is an owner
decision, not an engineering default.

**Decision.** A **local-first metrics registry** owned by `ComradeRuntime`
(`comrade_ui`): a small `Metrics` struct of atomic counters + bounded latency histograms,
updated at instrumentation points, and read out via a single `metrics_snapshot() ->
MetricsSnapshotDto` command (uniffi `Record`, registered symmetrically in both bridges). It is
surfaced in a **Settings/debug screen**, not exported anywhere. Minimum instrument set:

- **Calls:** attempt / connected / TURN-fallback-triggered / failed counters, split by
  media kind; time-to-connect histogram.
- **Signaling & DM latency:** send→relay-ack and inbound decrypt→emit histograms.
- **Backfill:** watermark rewind span, messages recovered per launch, dedup drop counts.

Registry is process-local, cleared on `lock_vault` (same lifecycle as everything else — no
metric outlives the key), and updated with **lock-free atomics on hot paths** so it never
interacts with the two-bus backpressure or the lock rule.

**Alternatives.** Bundle a metrics/OTel crate (rejected as default — pulls an export model into
a no-telemetry product; a heavier dep than warranted). Log-only (rejected — not queryable, and
"observable" per the org rule means a snapshot surface, not grep).

**Consequences.** (+) Meets the org standard without compromising the privacy stance; gives us
data to decide WP14 (pagination index) and TURN economics (OQ10) empirically. (−) A future
off-device aggregate export is deliberately deferred to an owner decision (§4).

### ADR-6 — Messaging completion scope

**Context.** Messaging is protocol-complete; the gaps are pagination, deep-linking, and test
coverage of the event plumbing (EventBus at `ComradeCore.kt:588` and `ChatEventRouter` have
**zero tests**).

**Decision.**
- **Pagination** as specified in ADR-1: `messages_with_page` with a `{created_at, id}` cursor.
  Initial implementation may retain the full-scan-then-slice body (correctness first; **output**
  is bounded even before **work** is) — the cursor API is the durable contract, and WP14 swaps
  the body for an indexed range read without changing callers. This is the fail-fast, contract-
  first path.
- **Notification deep-linking:** `Notifier.openAppIntent` gains a conversation-peer extra and
  `MainActivity` gains `onNewIntent` to route a tap straight into that conversation (depends on
  the Android nav introduced with the state-layer repository, WP7).
- **Test coverage:** JVM unit tests for `EventBus` priority/coalescing/drop behavior and
  `ChatEventRouter` routing; Rust tests for the new events, cursor paging, and unread counts.

**Consequences.** (+) Closes the visible messaging gaps and retires the "no tests" liability on
the event plumbing. (−) The initial paging body is O(N) until WP14; honest and measured.

---

## 3. Work Packages

Waves are strictly ordered; WPs within a wave note their intra-wave deps. Every WP is
independently shippable and small enough to review; every WP gets a lead review before merge,
and the high-risk ones (WP1, WP3, WP6, WP7, WP13) additionally get an independent review pass.
Tiers: **Opus** = architectural/subtle concurrency or state-machine reasoning; **Sonnet** =
mechanical/boilerplate/tests/docs against a clear spec. Sizes: S ≤ ~½ day, M ~1–2 days,
L ~3–5 days of focused work.

### Wave 1 — Call reliability (P0)

**WP1 — Desktop callee renegotiation (fix the cross-platform TURN P0)**
- *Goal:* A same-`call_id` re-offer is a renegotiation, not a `busy`.
- *Scope:* `desktop/ui/main.js` (`onCallSignal`, `handleIncomingOffer`); new
  `desktop/ui/call_decisions.mjs` (pure `decideOfferDisposition(hasCall, sameCallId, hasPc)`
  → `RENEGOTIATE | DUPLICATE_NOOP | BUSY | NEW_INCOMING`).
- *Acceptance:* offer with live matching `call_id` → answer via existing pc; different
  `call_id` while busy → `busy`; no session → ring. No behavior change to the happy path.
- *Tests (required):* `node --test` vectors mirroring `CallManager.kt:1663-1669`, incl. the
  regression vector "re-offer during active call must NOT be busy".
- *Size:* M · *Tier:* Sonnet (spec is Android's proven function) + independent review ·
  *Deps:* none · *Risk:* webview support for re-`setRemoteDescription`/`createAnswer` on a
  live pc.

**WP2 — Register missing Tauri call commands**
- *Goal:* Expose `call_ice_servers_for`, `call_sas`, `turn_server_status` to the webview.
- *Scope:* `desktop/src-tauri/src/commands.rs` + `lib.rs:89-144` invoke_handler.
- *Acceptance:* all three invokable from JS; JNI/Tauri command surfaces are symmetric for these.
- *Tests:* `cargo clippy` (desktop lane) green; a bridge-symmetry checklist assertion in review.
- *Size:* S · *Tier:* Sonnet · *Deps:* none · *Risk:* low.

**WP3 — Desktop caller STUN-first + ICE-restart/TURN fallback + fix ICE symmetry**
- *Goal:* Desktop caller retries over TURN on ICE failure, symmetric with the callee.
- *Scope:* `desktop/ui/main.js` (`setupPeer` `onconnectionstatechange`, `startCall`); reuse
  `call_ice_servers_for` (WP2); extend `call_decisions.mjs` with
  `decideConnectionStateAction`.
- *Acceptance:* first `failed` → `setConfiguration(stun_and_turn)` +
  `createOffer({iceRestart:true})` re-offer once; second failure → terminal. Caller and callee
  use the same relayed config during fallback.
- *Tests:* `node --test` for `decideConnectionStateAction` (initial-failed → RESTART_WITH_TURN;
  already-tried → FAIL; disconnected → WAIT), mirroring Android.
- *Size:* M · *Tier:* Opus (ICE state machine) + independent review · *Deps:* WP1 (callee must
  accept the re-offer), WP2 · *Risk:* real ICE untestable in CI; webview `setConfiguration`
  support.

**WP4 — Desktop SAS UI**
- *Goal:* Show the 4-emoji SAS on connected calls, matching Android.
- *Scope:* `desktop/ui/main.js` (`onCallConnected`), `index.html`/`styles.css`; uses `call_sas`.
- *Acceptance:* SAS renders when both SDPs have fingerprints; shows an honest "can't verify"
  when `call_sas` returns `None` (per `runtime.rs:1268` contract).
- *Tests:* node vector for the "no fingerprint → no SAS" branch of a tiny formatting helper.
- *Size:* S · *Tier:* Sonnet · *Deps:* WP2 · *Risk:* low.

**WP5 — Two-peer Rust integration test: TURN re-offer / renegotiation signaling sequence**
- *Goal:* Prove the *transport* carries a same-`call_id` re-offer to the peer runtime (not
  deduped/dropped), plus glare ordering.
- *Scope:* `crates/comrade_ui/tests/two_peer_integration.rs` + `tests/support` (in-process
  relay).
- *Acceptance:* runtime A sends offer→(answer)→second offer same `call_id` fresh event id;
  runtime B emits a second `IncomingCallSignal(offer)` with that `call_id` within
  `CALL_SIGNAL_MAX_AGE_SECS`; a duplicate wrapper is deduped; a glare pair resolves by npub.
- *Tests:* this IS the test (extends the existing offer/answer/ICE/hangup suite from COMMS-03).
- *Size:* M · *Tier:* Sonnet (harness exists) · *Deps:* none · *Risk:* none material.

### Wave 2 — State layer phase 1 + observability

**WP6 — Rust observable projections: fine-grained events + pagination + outgoing event**
- *Goal:* Give frontends per-conversation deltas and a paginated read, per ADR-1.
- *Scope:* `crates/comrade_ui/src/runtime.rs` (`BridgeEvent` + new `OutgoingDirectMessage`;
  `messages_with_page` + `MessageCursor`/`MessagePageDto`; `unread` on `ConversationDto`;
  emit outgoing event in `send_dm_reply`); `crates/comrade_storage` (`last_read_at` on
  `ConversationMeta`, read-watermark helper); both bridges registered symmetrically.
- *Acceptance:* sending a DM emits `OutgoingDirectMessage`; conversation-list preview reflects
  the send with no incoming event; `messages_with_page` returns bounded pages with a stable
  `{created_at,id}` cursor; `unread` computed from the watermark. All DTOs uniffi-expressible.
- *Tests (required):* cursor paging (boundaries, tie-break on equal `created_at`),
  outgoing-event emission, unread math, and an assertion that the projection reads only the
  critical bus.
- *Size:* L · *Tier:* Opus + independent review · *Deps:* none · *Risk:* lock-boundary (nothing
  new must outlive the key); keep queries `.await`-free.

**WP7 — Android chat repository + ViewModels consuming per-conversation events**
- *Goal:* Kill tick-driven full refetch; consume typed per-`peer` events; page history.
- *Scope:* new `ChatRepository` + per-screen ViewModels; retire the global-tick
  `LaunchedEffect` refetch (`ChatsScreen.kt:178,183,497,929`); centralize the repeated
  `withContext(IO)/runCatching` boilerplate.
- *Acceptance:* a new message updates exactly one list row / appends one message; opening a
  chat pages the newest window, not the whole history; no screen calls `ComradeCore` directly.
- *Tests (required):* JVM tests for the repository reducer (event → state delta) and unread
  updates; keep them JNI-free behind the existing `ComradeBackend` seam.
- *Size:* L · *Tier:* Opus + independent review · *Deps:* WP6 · *Risk:* Android authored
  without local compile (CI-only) → keep diffs structurally close to proven patterns; land
  incrementally per screen.

**WP8 — Desktop adopts pagination + per-conversation updates**
- *Goal:* Same fine-grained consumption on desktop.
- *Scope:* `desktop/ui/main.js` DM rendering: patch one conversation row on event; page via
  `messages_with_page`; stop unbounded regrowth.
- *Acceptance:* parity with WP7 behavior; conversation list patched in place.
- *Tests:* node vectors for any extracted pure list-merge/patch helper.
- *Size:* M · *Tier:* Sonnet · *Deps:* WP6 · *Risk:* JS untestable beyond pure helpers.

**WP9 — Local-first metrics registry + snapshot API + instrumentation**
- *Goal:* Meet the org observability standard; instrument comms.
- *Scope:* `comrade_ui` `Metrics` (atomics + bounded histograms), `metrics_snapshot()` +
  `MetricsSnapshotDto`, instrument points in call setup/fallback, DM send/receive, backfill;
  registered symmetrically in both bridges; cleared on `lock_vault`.
- *Acceptance:* counters/histograms move under the two-peer test; snapshot round-trips uniffi;
  no metric survives lock; hot paths use atomics only.
- *Tests (required):* metric increments under `two_peer_integration.rs`; snapshot serde
  round-trip (mirrors the existing DTO round-trip test at `runtime.rs:489-510`).
- *Size:* M · *Tier:* Opus · *Deps:* none (WP5 helps assert call counters) · *Risk:* must not
  touch bus backpressure or hold locks.

**WP10 — Metrics debug/settings screen (both platforms)**
- *Goal:* Make the snapshot visible.
- *Scope:* Android Settings section + desktop settings panel calling `metrics_snapshot()`.
- *Acceptance:* renders counters/latencies; read-only; no export control present.
- *Tests:* JVM test for the Android formatter; node vector for the desktop formatter.
- *Size:* S–M · *Tier:* Sonnet · *Deps:* WP9 · *Risk:* low.

### Wave 3 — Messaging completion + engine lifecycle

**WP11 — Notification deep-linking into a conversation**
- *Goal:* Tapping a DM notification opens that conversation.
- *Scope:* `Notifier.openAppIntent` (peer extra), `MainActivity.onNewIntent` (route to chat).
- *Acceptance:* cold and warm taps land in the right conversation; no double-registration
  (respect the `ChatEventRouter`-owns-routing invariant from COMMS-01).
- *Tests:* JVM test for the intent→destination mapping (pure); emulator smoke in
  `android-apk.yml`.
- *Size:* S · *Tier:* Sonnet · *Deps:* WP7 (nav target) · *Risk:* low.

**WP12 — EventBus + ChatEventRouter test coverage**
- *Goal:* Retire the "zero tests" liability on the event plumbing.
- *Scope:* JVM tests for `EventBus` (`ComradeCore.kt:588`) priority/coalesce/drop tiers and
  `ChatEventRouter` (`RelayConnectionService.kt:204`) routing/de-dup.
- *Acceptance:* critical events never dropped; coalesced tier keeps latest-per-key; feed tier
  bounded/oldest-dropped — all asserted.
- *Tests:* this IS the WP.
- *Size:* S · *Tier:* Sonnet · *Deps:* none · *Risk:* none.

**WP13 — Engine lifecycle ownership: Nostr disconnect on Off-Grid (closes A1, comms scope)**
- *Goal:* Off-Grid actually disconnects relays, testably.
- *Scope:* `ComradeRuntime` transition handling (extend the `sync_saathi_lifecycle` pattern at
  `runtime.rs:563-567`); call the existing engine `disconnect`s; abort/re-spawn the tracked
  subscription tasks (`runtime.rs:583-593`); reconnect on switch-back.
- *Acceptance:* after toggling to `OffGridTravel`, no relay socket remains
  (netstat-/harness-visible); switch-back re-subscribes; runs off a `RuntimeHandles` snapshot,
  never under a held guard across an await.
- *Tests (required):* integration test asserting socket teardown + re-subscribe; regression
  test that `broadcast_chitthi` is refused/queued while off-grid.
- *Size:* M · *Tier:* Opus + independent review · *Deps:* none (dep OQ2 relay-config ownership
  noted) · *Risk:* reconnect latency regression; covered by test.

**WP14 — (Perf, gated) Plaintext composite-key message index for O(page) pagination**
- *Goal:* Make cold-open conversation scan and paging O(peers)/O(page), not O(all messages).
- *Scope:* `comrade_storage`: new index tree keyed `"{peer}\u{1f}{created_at:020}\u{1f}{id}"`
  (the zero-padded-timestamp pattern already used for journal ids, `repository.rs:137-138`);
  add a `range()` primitive to `EncryptedStore` (none exists today — only
  get/put/values/keys); backfill on first open; swap `messages_with_page`'s body to a range
  read (API unchanged).
- *Acceptance:* identical results to the scan implementation (differential test); paging
  touches only the page's rows.
- *Tests (required):* differential test (index vs scan) + backfill/migration test.
- *Size:* M · *Tier:* Opus · *Deps:* WP6; **owner sign-off** (plaintext key exposes peer+time
  at rest — consistent with the existing S4 posture where contacts are already keyed by npub,
  but an explicit decision, §4) · *Risk:* migration correctness; do **not** put on the Wave-2
  critical path.

### Wave 4 — Call-session convergence + video polish

**WP15 — Shared pure Rust call-session decision module; desktop adopts first**
- *Goal:* One decision table, guarded by shared conformance vectors (ADR-2).
- *Scope:* new wire-free `comrade_core::call_session` (mirror `decide*`), the WP1/WP3 node
  vectors reused as the conformance suite; desktop swaps `call_decisions.mjs` bodies for thin
  FFI calls; Android left as-is (its Kotlin deciders must pass the same vectors).
- *Acceptance:* desktop behavior unchanged (node vectors still green through the FFI swap);
  Rust module passes the shared vectors; Android deciders verified against the same vectors.
- *Tests (required):* the conformance suite runs in the Rust lane and the node lane over the
  same vector file.
- *Size:* L · *Tier:* Opus · *Deps:* WP1, WP3, WP5 · *Risk:* keep Android from drifting — treat
  the vector file as the contract.

**WP16 — Mid-call renegotiation semantics (video add/remove during a call)**
- *Goal:* Correct, cross-platform renegotiation for media changes, not just TURN restart.
- *Scope:* desktop `main.js` + shared decision module; Android verification.
- *Acceptance:* adding/removing video mid-call renegotiates via same-`call_id` offer (never
  `busy`); SAS re-derivation handled.
- *Tests:* node/Rust vectors for the renegotiation disposition; two-peer signaling test for
  the media-change offer.
- *Size:* M · *Tier:* Opus · *Deps:* WP1, WP3, WP15 · *Risk:* cross-platform interop.

**WP17 — Desktop video bandwidth/resolution constraints + camera-toggle parity**
- *Goal:* Bring desktop video toward Android's capability.
- *Scope:* `desktop/ui/main.js` getUserMedia constraints + sender parameters; add a mid-call
  camera on/off toggle if missing (Android has it in `CallScreen.kt`).
- *Acceptance:* bounded resolution/bitrate; camera toggle works mid-call.
- *Tests:* node vector for any pure constraint-builder helper.
- *Size:* M · *Tier:* Sonnet · *Deps:* none · *Risk:* JS untestable beyond pure helpers.

**WP18 — Audit hygiene (one small package)**
- *Goal:* Strike AUDIT findings resolved by shipped work, with evidence.
- *Scope:* `AUDIT.md` — mark A3/Q2/P3/M2-3/O5 resolved, each with a one-line code citation;
  no other edits.
- *Acceptance:* each struck finding cites the resolving code/commit.
- *Tests:* n/a (doc); reviewer verifies each citation.
- *Size:* S · *Tier:* Sonnet · *Deps:* none · *Risk:* none.

---

## 4. Owner Decisions Needed

1. **OQ10 — Who operates and pays for TURN, and populate CI secrets.** TURN fallback (WP3) and
   its interop test story are inert until a relay exists and `TURN_URL`/`TURN_USERNAME`/
   `TURN_PASSWORD` are populated (the `CallRelayDefaults`/BuildConfig seeding path is wired but
   the secrets are empty). `deploy/coturn` is a turnkey self-host template; the decision is
   operate-it-ourselves vs. document-your-own. **Blocks:** production TURN, and any CI/manual
   traversal test. Needed before Wave 1 can be validated end-to-end (the code ships without it;
   the *proof* doesn't).
2. **Telemetry export policy (ADR-5).** Metrics are on-device only by default. Do we ever want
   an *opt-in, aggregate* export (e.g. anonymized connect-success rates to justify TURN spend)?
   If yes, it's a separate design with an explicit consent surface; if no, WP9 is the terminal
   state. Default assumption until decided: **no export.**
3. **Plaintext composite-key index sign-off (WP14).** The index would place `peer` + message
   `created_at` in a plaintext redb key (consistent with the existing S4 posture where contacts
   are already keyed by npub, but a *new* instance of it). Approve, or require key hashing
   (which forfeits the range-scan benefit and makes WP14 pointless). Default: **WP14 deferred**
   until profiling + this sign-off.
4. **Android decision-table convergence (ADR-2).** After WP15, do we migrate Android's Kotlin
   deciders onto the shared Rust module, or keep them as-is guarded only by the shared vectors?
   Recommendation: **keep as-is** unless maintenance pain grows; touching `CallManager` carries
   ANR-regression risk. Owner may overrule.

---

## 5. Anti-Goals / Explicitly Out of Scope

- **Group / multi-party calls and any SFU.** 1:1 only. No mixing/forwarding server;
  incompatible with the serverless, gift-wrapped-DM signaling model.
- **Push wakeup for a dead process.** A push token is a real metadata tradeoff for a
  privacy-first app; deferred exactly as COMMS-01 documented. Background-but-unlocked delivery
  (`RelayConnectionService`) is the boundary.
- **Telecom `ConnectionService` / system-dialer integration.** Deferred; current CallStyle +
  full-screen-intent UX is sufficient.
- **CallSignal / CallEnvelope protocol changes.** The wire is solid and pure; renegotiation
  rides the existing same-`call_id` offer (no `ice_restart` field, no versioning bump).
- **Typing indicators, disappearing messages, group chats.** Out per AUDIT §7.
- **Big-bang rewrite of Android `CallManager`.** Prohibited; convergence is
  decision-table-only and desktop-first (ADR-2).
- **Third-party telemetry / analytics SDKs.** Prohibited by default (ADR-5).
- **Materialized mutable Rust state store.** Explicitly rejected in ADR-1.

---

## 6. Test & Verification Strategy

**CI lanes (from `.github/workflows/ci.yml` + `android-apk.yml`):**

| Lane | Covers | Strength |
|---|---|---|
| `rust` (fmt + clippy + `cargo test --workspace` + `media-http`) | All new Rust: projections, cursor paging, unread math, events, metrics, engine lifecycle, shared decision module | **Strong** — put logic here |
| `crates/comrade_ui/tests/two_peer_integration.rs` (+ in-process relay `support/`) | Signaling **sequences** end-to-end between two real runtimes: request-gating, DM delivery, offer/answer/ICE/hangup, **+ new: TURN re-offer/renegotiation (WP5), glare, metric increments** | Strong for signaling transport |
| `desktop` (Tauri clippy) | Bridge compiles/lints; command registration | Weak (no JS, no bundle) |
| **`node --test` (NEW)** | Pure desktop call decisions + formatters (`call_decisions.mjs`, list-merge/constraint helpers) | New — built-in runner, no `package.json`/npm needed |
| `android` (`gradlew test`, JVM) | `EventBus`/`ChatEventRouter` (WP12), repository reducers (WP7), intent mapping (WP11), existing `CallManager` pure deciders | Medium (no device) |
| `android-apk.yml` (emulator, `connectedDebugAndroidTest`) | `CallManagerLifecycleTest`, `TwoPeerJniIntegrationTest` across the real JNI boundary | Medium — real device, no real peer NAT |
| `load-test` | Feed-flood backpressure (COMMS-04) | Guards the two-bus split |

**Where the untestable gaps are, and the compensating controls:**

- **Real ICE/TURN traversal and media flow** cannot run in headless CI (no cameras/mics, no
  NAT). *Compensation:* WP5 proves the **signaling** re-offer sequence in Rust (the part that
  broke cross-platform fallback is a signaling-disposition bug, not a media bug); the pure
  decision vectors (node + Rust) prove the state transitions; and a **manual cross-platform
  test matrix** (below) exercises real media against `deploy/coturn` + `deploy/test-relay`.
- **Desktop JS DOM/WebRTC behavior** beyond pure functions is untestable in CI. *Compensation:*
  extract every decision into `*.mjs` pure functions under `node --test`; keep DOM glue thin;
  manual smoke.
- **Android authored without local compile** (CI-only). *Compensation:* keep diffs structurally
  close to proven patterns; land per-screen; lean on the JVM + emulator lanes.

**Required manual cross-platform matrix (documented script, run before a calling release):**
Android↔Android, Desktop↔Desktop, and both **Android-caller→Desktop-callee** and
**Desktop-caller→Android-callee**, each over (a) STUN-succeeds and (b) TURN-required (forced by
firewalling UDP), asserting connect, SAS match, and clean hangup. The TURN cases are the ones
the P0 broke; they are the acceptance gate for Wave 1 and depend on OQ10 (§4).

**Org-rule compliance:** every WP lists required tests; bug-fix WPs (WP1, WP3) carry explicit
regression vectors ("re-offer during active call must not be busy"; "already-tried fallback
must not loop"). No test is skipped.

---

## Appendix A — Critical files for implementers

- `desktop/ui/main.js` — the P0 interop bug (`:996-1014`, `:1088-1096`), STUN-only caller + no
  ICE restart (`:940-946`, `:982`, `:1052`); target of WP1/WP3/WP4/WP8/WP17.
- `crates/comrade_ui/src/runtime.rs` — `BridgeEvent` (`:521-551`),
  `conversations()`/`messages_with()` (`:972-1045`), `RuntimeHandles` (`:2022-2058`),
  ICE/SAS/TURN methods (`:1230-1343`); target of WP6/WP9/WP13 and the ADR-1 projection design.
- `crates/comrade_storage/src/repository.rs` — `messages_with` full-scan (`:264-272`),
  `StoredMessage`/`ConversationMeta`/`CallRecord` shapes (`:116-178`), journal zero-padded-key
  precedent (`:137-138`); target of WP6/WP14.
- `android/app/src/main/java/mullu/comrade/call/CallManager.kt` — the proven pure deciders
  (`:1629-1700`) that desktop must mirror (ADR-2/3) and the shared decision module (WP15) must
  conform to.
- `android/app/src/main/java/mullu/comrade/RelayConnectionService.kt` — `ChatEventRouter`
  global ticks (`:204-232`) that WP7/WP12 replace/test.
- `desktop/src-tauri/src/lib.rs:89-144` + `desktop/src-tauri/src/commands.rs` — the
  command-registration symmetry work (WP2).
