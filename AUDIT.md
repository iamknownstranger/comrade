# Comrade — Repository Audit & Improvement Plan

_Audit date: 2026-07-10._
_Method: full manual read of every Rust crate, the CLI, the Tauri shell + desktop JS, and the Android app, cross-checked by an 8-dimension parallel agent sweep. Architecture findings were additionally adversarially verified (all CONFIRMED); every other finding below was verified by direct file reads unless explicitly flagged otherwise._

---

## 1. Executive Summary

**Overall health: C+ (prototype grade: promising core, credibility gaps at the edges).**
The Rust workspace shows genuinely strong engineering discipline for a prototype — clean crate layering, typed errors everywhere, ~90 behavior-asserting unit tests, an encrypted-at-rest store with adversarial "scan the disk for plaintext" tests, and CI that gates fmt/clippy/test. The problem is the distance between what the README promises and what is actually wired: three of the seven advertised engines (Saathi mesh, NIP-65 gossip routing, encrypted media) are never invoked by any frontend; the Android app cannot unlock the vault at all, so its voice "post"/"timeline" commands can never succeed; and DMs use the deprecated, unauthenticated NIP-04 scheme in a product whose entire identity is "privacy-first."

**Top 3 risks:** (1) NIP-04 (AES-CBC, no authentication, metadata-leaking) for E2E DMs while NIP-44 is available in the already-pinned nostr-sdk; (2) Off-Grid mode is a *false privacy assurance* — the UI tells the user "public relays paused, Saathi mesh active" while relay websockets stay connected and no engine is ever disconnected; (3) `change_pin` re-encrypts the store non-atomically — a crash mid-way permanently corrupts user data.

**Top 3 opportunities:** (1) close the honesty gap — wire engine lifecycle to the state machine (or mark features experimental) and make the README match reality; (2) close the CI and release blind spots — Android tests and the Tauri crate are never built in CI, and official release APKs ship debug-signed with ephemeral keys and without the voice model; (3) scope the Sabha feed subscription (currently the global Kind-1 firehose) to a follow set, which fixes performance, UX, and privacy in one change.

---

## Decision log

- **2026-07-10 (owner):** Proceed on the current Rust stack; framework maturity risk (sled 0.34, yrs pre-1.0, libp2p/nostr-sdk churn) is *accepted* on the assumption these mature over time. Consequences: D1's sled migration is **out of scope** (document in SECURITY.md instead); unmaintained transitive-dep advisories from these frameworks are ignored with reasons + exit conditions in `deny.toml`; the hickory-proto DNS advisories wait on a libp2p release against hickory ≥ 0.26.
- **2026-07-10:** M0 executed (wrapper, CI lanes, cargo-deny gate, desktop lockfile, change_pin crash-safety regression test) plus M1 quick wins (M1-3 README truth pass, M1-6 backup/FLAG_SECURE/nsec masking, M1-7 checksum logic — pin pending network access, M1-8 CSP). MSRV measured from the lock: **rustc ≥ 1.88** (supersedes the sweep's 1.83 estimate in N3).
- **2026-07-12:** Field report: two fresh devices could not find each other by @handle. Root cause: the one-shot Kind-0 publish raced the relay dials and was never retried (fixed — bounded connect-wait, retry with backoff, republish on every launch), and search fanned the NIP-50 filter across non-search relays (fixed — dedicated search relays, client-side match filter, direct npub lookup). Chat UI now titles peers alias → published @handle → key, with a per-contact alias editor. Session-android feature parity adopted as the communication roadmap (§7); parity is a direction, not a claim.
- **2026-07-12 (owner, supersedes the 2026-07-10 entry above):** D1's sled migration is back in scope and has landed — `comrade_storage` now persists to `redb` instead of `sled`, keeping the identical Argon2id + AES-256-GCM envelope. `EncryptedStore::open` transparently migrates a pre-existing sled store in place on first open (decrypt under the caller's PIN, re-ingest into a fresh redb file, archive the old sled files under `sled-archive/` inside the same directory) — no caller (CLI, JNI, Tauri) changes. `sled` stays a dependency *only* for that one-time migration reader (`comrade_storage::migrate`); it is no longer used for the live store, so `deny.toml`'s sled-transitive-dep ignores now carry an updated exit condition (remove once the migration reader itself is retired). A useful side effect: because redb's write transactions are atomic, `change_pin` now runs the whole rekey in a single transaction and is genuinely crash-safe — this closes finding **S2** for real (the regression test that used to be `#[ignore]`d as a known-bug xfail now passes unconditionally); M1-2 is done.
- **2026-07-12 (owner):** M1-1 lands as full NIP-17/NIP-59 gift-wrap, not plain NIP-44-in-Kind-4 — resolving OQ4 in favor of metadata privacy over interop with older plain-NIP-44 clients. `VaultEngine::send_dm_reply` builds every new DM via `EventBuilder::private_msg` (Kind-14 rumor → Kind-13 seal → Kind-1059 gift wrap, signed by a one-time key); the inbox subscription now takes both `Kind::GiftWrap` and legacy `Kind::EncryptedDirectMessage`, decrypting each with the appropriate path, with the `since()` window widened by NIP-59's 2-day timestamp randomization so a fresh gift wrap can never be dropped as "too old." Separately, `comrade_core::call::IceStrategy` adds a STUN-first, TURN-on-failure policy (`ice_servers_for`) — `place_call`'s initial offer now asks for STUN only; a new `ComradeRuntime::call_ice_servers_for("stun_and_turn")` exists for a frontend to call once it detects its `RTCPeerConnection` can't connect (the CGNAT case) and restart ICE with the relay included. The ICE fallback is engine-level and tested but **not yet called by either frontend** — flagged in §8.1, not claimed as end-to-end wired, per Theme 1 discipline.
- **2026-07-15:** Six connectivity/calling tickets (COMMS-01–06, referenced by that tag throughout the touched code) close gaps this document already named — §8.1's "always-on background delivery needs a connection service" note, OQ10's TURN question, §3.3's unbounded-feed-subscription finding, and the calls signaling layer's untested call-setup races. **COMMS-01** (background connectivity): `GlobalScope` is gone from `ComradeCore`'s class initializer; `initializeEventBridge()` is now a `suspend fun` awaited by `ComradeApplication.appScope` at startup and, idempotently, by `unlockVaultTyped`, closing a race where an event published before a listener subscribed was silently dropped (a zero-receiver `tokio::sync::broadcast` send is a no-op). A new opt-in `RelayConnectionService` (foreground, `dataSync` type, default-on toggle in Settings) becomes the sole consumer of the event queue once the vault unlocks, and stops on lock/logout; `ChatEventRouter`, not `MainActivity`, now owns notification routing, so an Activity recreation can never double-register a listener or double-notify. The security boundary is documented, not oversold: this covers backgrounded-but-*unlocked* only — process death still drops the in-memory vault key, and there is still no push-wakeup path for an actually-dead process (a push token is a real metadata tradeoff for a privacy-first app; deliberately out of scope). **COMMS-02** (TURN): `comrade_core::call` gains `validate_turn_url` (rejects anything that isn't a well-formed `turn:`/`turns:` URI before it's persisted) and `mint_turn_rest_credentials` (coturn's `use-auth-secret` REST mode — HMAC-SHA1 username/password, time-limited; cross-checked against an independently computed Python HMAC in tests). `deploy/coturn/` ships a TLS coturn compose file + config + README for an operator who wants to self-host — the "self-hosted coturn" option OQ10 already named, now turnkey rather than theoretical; Comrade still ships no credential-broker service of its own (no account server to host one on), so an operator mints credentials via the tested function or the README's `openssl` one-liner. Settings' TURN card never reads back a saved credential (write-only, matching the existing nsec-masking discipline) and adds an honest "Test relay connectivity" diagnostic (`CallManager.testTurnConnectivity`, a throwaway RELAY-only `PeerConnection`) reporting no-server/relay-available/relay-unavailable instead of hanging. **COMMS-03** (two-peer test harness): a hermetic in-process Nostr relay (`crates/comrade_ui/tests/support`) backs a new `two_peer_integration.rs` suite proving request-gating, DM delivery, and the full offer/answer/ICE/hangup/call-log path between two real `ComradeRuntime`s — no Docker, runs under plain `cargo test`. `deploy/test-relay/` (nostr-rs-relay) plus a new `TwoPeerJniIntegrationTest` cover the same ground across the real Android JNI boundary when a relay URL is supplied via instrumentation args (the test skips, rather than silently falling back to the public relay pool, when it isn't). Scope note: the ticket's "two-installation" device harness is implemented as a lower-risk `deviceHarnessRole` Gradle property (a conditional `applicationIdSuffix`) rather than a full product-flavor split, specifically to avoid renaming every Gradle task `android-apk.yml`/`release.yml` invoke — a genuine reduction from the literal ask, left as follow-up if a fuller two-APK harness is wanted. **COMMS-04** (bounded event queue): `ComradeCore`'s unbounded `ConcurrentLinkedQueue` is replaced by `EventBus`, a three-tier priority queue — critical (calls/DMs/requests: never dropped), coalesced (mesh/ledger/profile/receipt status: latest-per-key only), feed (bounded, oldest-dropped) — with live depth/drop/coalesce/lag counters exposed as a `StateFlow`. `SabhaEngine`'s public-feed subscription moves from an unconditional relay-wide firehose to `FeedFilterSpec` (followed-authors scope once any contact exists, a bounded global bootstrap window otherwise), closing part of the executive summary's "scope the Sabha feed" opportunity. A `#[ignore]`d load test (`feed_flood_load.rs`, ~1000 concurrent public events against a live DM) is wired into CI as its own required `load-test` job. **COMMS-05** (call-setup races): `CallManager.startOutgoingCall` now builds its `Session` synchronously, before `placeCallTyped`'s async round-trip begins — closing a real bug where `hangup()` called during that window found `session == null`, silently no-opped, and let the delayed continuation send an offer after the UI had already gone back to idle; `endWith` now also cancels the in-flight `placingJob`. Reducer-style lifecycle tests live in a new instrumented `CallManagerLifecycleTest` (cancel-before-session, place failure, late-callback-after-cancel, simultaneous-offer-during-provisional-phase), since they need a real `PeerConnectionFactory`/JNI boundary a JVM unit test can't load. **COMMS-06** (Bluetooth routing): `BLUETOOTH_CONNECT` (API 31+) is requested only when the user explicitly selects the Bluetooth route in `CallScreen`'s `AudioRouteButton`; denial calls a new `CallManager.onBluetoothPermissionDenied()`, which drops Bluetooth from `availableRoutes` for the rest of the call and explains the speaker fallback via a toast instead of silently doing nothing. **Verification honesty**: every Rust change is covered by `cargo fmt --check` / `clippy --all-targets -D warnings` / `cargo test` (workspace, plus the `media-http` feature and the load test) — all green. Every Kotlin/Android change (all six tickets touch it) and the `desktop/src-tauri` event-forwarding update are **not compiled in this environment** (no Android SDK; the Tauri crate needs system GTK/webkit2gtk libs this sandbox lacks) — they're reasoned from, and kept structurally close to, already-proven patterns in the same files, but should be treated as unverified until the repo's own CI (`android-apk.yml`'s `connectedDebugAndroidTest`, the `desktop` clippy lane) runs them for the first time. `deploy/coturn/` and `deploy/test-relay/` are likewise unverified — no Docker daemon was available while authoring them.

---

## 2. Repo Map

**Purpose.** "Comrade" — a privacy-first, cross-platform social client over Nostr, with an off-grid libp2p mesh mode and an encrypted couple's shared ledger. Hindi-derived nomenclature: public post = *Chitthi*, engines = *Sabha* (public feed), *Vault* (E2E DMs), *Saathi* (mesh), *Sakha/Sakhi* (couple CRDT ledger).

**Stack.** Rust 2021 workspace (tokio, nostr-sdk 0.44, libp2p 0.56, yrs 0.21, redb 4, aes-gcm/argon2/secp256k1) · Kotlin + Jetpack Compose Android app with an offline Vosk voice assistant · Tauri 2 desktop shell with a no-build vanilla-JS SPA · interactive CLI harness. ~11k lines total. Built through a series of AI-assisted PRs (milestone-labeled commits), no releases yet.

**Architecture sketch.**

```
                    ┌────────────── frontends ──────────────┐
   CLI (src/main.rs)   Android (Kotlin ⇄ comrade_jni)   Desktop (JS ⇄ Tauri commands)
                    └───────────────┬────────────────────────┘
                          comrade_ui (UiService + ComradeRuntime + event bus)
                    ┌───────────────┴────────────────────────┐
        comrade_core (crypto, sabha, vault, sakha, saathi*, relay*, media*)
        comrade_state (pure workspace state machine)
        comrade_storage (redb + Argon2id + AES-256-GCM)
                                          * = built & tested but never wired to a frontend
```

**Key directories.**

| Path | What it is |
|---|---|
| `crates/comrade_core/` | Protocol engines; the real product logic (crypto, feed, DMs, CRDT ledger, mesh, relay routing, media) |
| `crates/comrade_state/` | I/O-free progressive-disclosure state machine (Base / OffGridTravel / CoupleSandbox) |
| `crates/comrade_storage/` | Encrypted-at-rest KV store + typed repositories; the best-tested crate |
| `crates/comrade_ui/` | Framework-agnostic view-model (`UiService`) + async bridge orchestrator (`ComradeRuntime`) |
| `crates/comrade_jni/` | Android FFI boundary — uniffi-generated Kotlin bindings (typed, panic-guarded by uniffi's own scaffolding; no hand-rolled JSON, as of 2026-07-12) |
| `src/main.rs` | 720-line interactive CLI harness; mostly demo commands |
| `desktop/` | Tauri 2 shell (excluded from workspace) + vanilla JS SPA (`desktop/ui/`) |
| `android/` | Compose app: workspace list, keygen, "Hey Comrade" voice assistant |
| `.github/workflows/` | CI (Rust-only) + manual APK release |

**Surprising things.** The desktop JS contains a full mock backend for browser preview (`desktop/ui/main.js:621`); the Android app displays the raw `nsec` on screen; the "Couple Sandbox" pairing dialog on desktop validates a token client-side but never sends it to the backend; the Tauri bundle references icons that don't exist; there is no LICENSE file anywhere.

---

## 3. Audit Report

Severity: **C**ritical / **H**igh / **M**edium / **L**ow. Each finding is labeled **[fact]** (verifiable in code) or **[judgment]** (design opinion).

### 3.1 Security & Cryptography

| # | Sev | Finding |
|---|---|---|
| S1 | ~~H~~ | **[fact, resolved 2026-07-12]** DMs used deprecated NIP-04. `send_dm_reply` now builds NIP-17 gift-wrapped (NIP-59) NIP-44 messages via `EventBuilder::private_msg` — content, real sender, and exact timestamp are all hidden from relays; only the recipient `p` tag and a randomized (±2 day) timestamp are public. Legacy NIP-04 decrypt is kept, read-only, so a peer's older DMs still open. See M1-1 and the decision log. |
| S2 | **H** | **[fact] `change_pin` is not crash-safe.** `crates/comrade_storage/src/lib.rs:207-238` re-encrypts every value in place under the new key (lines 214-227) and only afterwards rewrites the salt + verification token (230-233). A crash mid-loop leaves a store whose meta says "old key" but whose values are a mix of old- and new-key ciphertexts — the mixed values are permanently undecryptable under either PIN. No test simulates this. (Latent today — `change_pin` has no non-test callers — but it is the published re-key API.) |
| S3 | **M** | **[fact] Weak-PIN reality vs. "encrypted at rest" promise.** The unlock secret is called a "PIN" throughout (`EncryptedStore::open(path, pin)`, CLI `unlock <PIN>`); nothing enforces minimum entropy (desktop `main.js:207` only checks non-empty). Argon2 uses library defaults (`lib.rs:255`, argon2 0.5 ⇒ m=19 MiB, t=2, p=1 — OWASP minimum). A 4-6 digit PIN over a 10⁴–10⁶ keyspace is offline-brute-forceable in seconds regardless of Argon2. The at-rest encryption is only as strong as the passphrase policy that doesn't exist. |
| S4 | **M** | **[fact] Social graph stored in plaintext keys.** `comrade_storage` encrypts values only; tree keys are plaintext by design (`lib.rs:23-25`). Contacts are keyed by npub (`repository.rs:143`), so a device-level attacker reads the full contact list without the PIN. The header note says "callers that need key confidentiality should hash keys" — no caller does. |
| S5 | **M** | **[fact] Secret-key exposure surfaces.** Android `KeygenSection` renders the full `nsec` on screen with no masking and no `FLAG_SECURE` (`MainActivity.kt:284-290`); the CLI prints the first 20 chars of the nsec (`src/main.rs:592-594`) and 16 bytes of a DH shared secret (`src/main.rs:617-621`); the nsec crosses the JNI boundary inside a hand-rolled JSON Java `String` (`comrade_jni/src/lib.rs:113-117`), which is immutable and unzeroizable. `KeyProfile.nsec` / `StoredIdentity.nsec` are plain `String`s — the README's zeroization claim (`README.md:171`) is true only for the storage key. |
| S6 | **M** | **[fact] Tauri CSP disabled.** `desktop/src-tauri/tauri.conf.json:20` sets `"csp": null` in a webview that renders content from public relays. Mitigation: `desktop/ui/main.js` is disciplined — all dynamic text goes through `textContent` (`el()` helper, `main.js:35-51`), and `innerHTML` is only used for clearing. CSP is the missing second layer, cheap to add. |
| S7 | **M** | **[fact] `allowBackup="true"`** (`AndroidManifest.xml:17`). ADB/cloud backup can exfiltrate the app's data dir. Values are encrypted, but combined with S3 (weak PIN) a backup is an offline brute-force target; privacy-focused apps conventionally set `allowBackup=false`. |
| S8 | **M** | **[judgment] Sakha has no forward secrecy.** Pairing derives one static symmetric key from static-static ECDH (`sakha.rs:136-144`, `crypto.rs:78-104`) and publishes ciphertext to public relays as Kind-30078. Compromise of either partner's nsec at any future time decrypts the couple's entire ledger history retained by relays. Acceptable for a prototype only if documented as a known limitation; there is no threat-model doc (see N2). |
| S9 | L | **[fact] Sakha sync handler trusts the relay's author filter.** `sakha.rs:217-260` never checks `event.pubkey == partner_pk`; authenticity rests entirely on AES-GCM decryption succeeding. AEAD makes forgery infeasible, but replayed/reflected updates are accepted (mostly harmless — Yrs updates are idempotent). Cheap defense-in-depth check missing. |
| S10 | L/M | **[fact] Vosk model fetched without integrity pinning.** `scripts/fetch-vosk-model.sh:12,29` downloads a zip over HTTPS with no checksum; the bytes are baked into the APK. One `sha256sum -c` line fixes it. |
| S11 | M | **[judgment] Saathi mesh is plaintext broadcast with spoofable sender.** Mesh messages are unencrypted application-layer JSON visible to every LAN peer, and `MeshMessage.sender` is a free-text self-declared label (`saathi.rs:41-59,289-291`) not bound to the libp2p identity. Fine for a demo; incompatible with the README's privacy framing if ever wired up (currently it is not — see A1). |
| S12 | M | **[judgment] Untrusted DMs auto-compile to payment URIs.** Any inbound DM containing `/pay …` becomes a rendered UPI chip with a ready-made `upi://pay` URI (`vault.rs:34-50`, `desktop/ui/main.js:421-429`). Amount/VPA are shown, so the user does see what they'd approve, but money-adjacent parsing of attacker-controlled text deserves explicit confirm-flow design, and amounts are `f64` (see Q3). |

**Healthy:** AES-GCM usage is correct throughout (fresh random 96-bit nonce per seal, prepended; auth failure surfaces as typed errors) — `crypto.rs:123-149`, `storage/lib.rs:264-290`, `sakha.rs:39-65`. The ECDH x-only lift + SHA-256(x) + HKDF-labels construction (`crypto.rs:78-118`) composes standard primitives sensibly and is symmetric-verified by test. Storage's PIN verification fails closed (`StorageError::InvalidPin`), proven by tests including on-disk plaintext scans (`storage/lib.rs:346-370`, `repository.rs:377-408`).

### 3.2 Architecture & Design

| # | Sev | Finding |
|---|---|---|
| A1 | **H** | **[fact, adversarially confirmed] Off-Grid mode is a false privacy assurance — engines are never disconnected.** `toggle_workspace` (`runtime.rs:326-328`) only flips the pure state enum; a repo-wide grep shows `SabhaEngine::disconnect` / `VaultEngine::disconnect` / `SakhaEngine::disconnect` have **zero call sites**, and `SaathiEngine` is never constructed anywhere. Yet the desktop toasts "Off-Grid / Travel mode — public relays paused, Saathi mesh active" (`desktop/ui/main.js:470-474`) while the relay websockets stay live, `broadcast_chitthi` has no workspace guard, and the compose UI stays active. `GossipEngine` and `MediaEngine` are likewise unwired (call sites only in CLI demos/tests); the CLI even prints "Saathi mesh would spin up here (run 'saathi' binary in production)" (`src/main.rs:544`) — no such binary exists. The README's engine table (`README.md:8-17`) presents all of these as product features. |
| A2 | **H** | **[fact, adversarially confirmed] The Android app cannot reach any networked feature.** No Kotlin code ever calls `unlockVault`/`pollEvent` (`ComradeCore.unlockVaultTyped` has no caller; `MainActivity.kt` has no passphrase UI), so engines are never constructed on Android. Voice "post …" and "read my timeline" route to `broadcast_chitthi`/`fetch_sabha_timeline`, which always return `VaultLocked` (`runtime.rs:284,269`). The advertised assistant (`README.md:140-143`) can only ever answer "I couldn't post that. vault is locked…" — on the one platform the release workflow ships. |
| A3 | **H** | **[fact, adversarially confirmed] Couple pairing is foreclosed by the ownership design, not just unwired.** `SakhaEngine::pair_with` takes `&mut self` with no interior mutability (`sakha.rs:136`), but `ComradeRuntime` Arc-wraps the engine un-paired with an empty relay list at construction (`runtime.rs:211-214`) — pairing can never be called through that handle, no bridge command exposes it, and `subscribe_sync` has no callers. The desktop "Partner Portal" validates the pairing token client-side, throws it away, and toasts "Partner portal unlocked" with no key agreement having occurred (`desktop/ui/main.js:490-516`); `sync_ledger` then always fails with `NoSharedSecret` (`sakha.rs:173`). The desktop DM composer's Send button is likewise a stub (`main.js:595-597`) even though `VaultEngine::send_dm` exists. |
| A4 | M | **[judgment] The layering itself is good — the gap is lifecycle ownership.** `comrade_state` (pure) → `comrade_core` (engines) → `comrade_ui` (view-model) → thin bridges is the right shape, and the three bridge layers are appropriately thin duplicates. What's missing is one owner that maps workspace transitions to engine start/stop (the thing A1/A3 need) and one owner for relay configuration — today Sabha hardcodes `DEFAULT_RELAYS`, Vault receives them as a parameter, Sakha gets an empty list, and the NIP-65 `RelayRouter` is an unintegrated island. Placing both in `ComradeRuntime` is the natural evolution. |
| A5 | M | **[fact] Voice "new identity" is a disconnected code path.** The JNI `generateKeypair` calls `KeyProfile::generate()` standalone (`comrade_jni/src/lib.rs:109-118`) and never touches the shared `ComradeRuntime`, so the voice assistant's "Created a new identity" (`CommandDispatcher.kt:60-63` via `ComradeCoreBackend.kt:28-29`) succeeds without changing the identity any engine signs with. Two identity paths, one of them cosmetic. |
| A6 | L | **[fact] JNI event delivery is poll-based and lossy-by-design.** A single process-global broadcast receiver is created lazily at first `pollEvent` (`comrade_jni/src/lib.rs:69-72`); events emitted before that are silently dropped, and lag drops surface as `{"lagged":n}`. Currently moot (no Kotlin caller — A2), but the contract should be documented or replaced with a callback when Android wiring lands. |
| A7 | L | **[fact] `comrade_core` declares an unused dependency on `comrade_state`** (`crates/comrade_core/Cargo.toml:7`; zero use sites in `comrade_core/src/`) — muddies the otherwise-clean layering story. |

**Healthy:** crate dependency direction is clean (storage has no core/nostr deps to avoid cycles — `repository.rs:6-7`; state is I/O-free by doc and by fact); DTO/event contracts are serde-typed and round-trip tested (`runtime.rs:489-510`); the JNI boundary is panic-guarded — as of the uniffi-rs migration (2026-07-12), this is uniffi's own generated `catch_unwind` scaffolding rather than the hand-rolled `guard_json` this line used to cite, so no unwinding crosses `extern "C"` either way; the Android voice layer is cleanly abstracted behind the `ComradeBackend` interface so dispatcher logic is JVM-unit-testable without the native library (`CommandDispatcher.kt:9-28`).

> **2026-07-12 note:** `comrade_jni/src/lib.rs` was rewritten wholesale for the
> uniffi-rs migration (hand-written `extern "C"` exports + `guard_json` + JSON
> marshaling → generated scaffolding + typed `Comrade` object). Several
> findings above (S5, A5, A6, Q8, O4, P2, M3-4, M3-8) cite specific old line
> numbers or mechanisms (`guard_json`, hand-rolled JSON, `format!`) in that
> file; their line citations are now stale and their substance has not been
> re-verified against the new code. Re-auditing them is a separate follow-up,
> not part of the FFI migration itself.

### 3.3 Performance & Concurrency

| # | Sev | Finding |
|---|---|---|
| P1 | **H** | **[fact] Sabha subscribes to the global Kind-1 firehose.** `sabha.rs:270-272` filters only by kind + since (no authors, no limit); `runtime.rs:243` requests the last **3600 seconds of every public text note on damus/nostr.band/nos.lol**. Every event is pushed onto the bus and prepended to the desktop DOM with no cap (`main.js:298-306`, `state.chitthis` grows forever). On real relays this floods the UI within seconds and grows memory unboundedly. This is simultaneously a perf, UX, and privacy problem (you render arbitrary global content). |
| P2 | M | **[fact] Blocking work on async threads and locks held across I/O.** *Partially fixed:* `unlock_vault` now runs Argon2id (19 MiB memory-hard KDF) + sled open on Tokio's blocking pool via `spawn_blocking` (`runtime.rs`), with `kdf_ms`/`total_ms` tracing, so the KDF no longer stalls a reactor thread. Still open: the `RwLock` write guard is held across the unlock await; Tauri commands hold the read guard across relay network awaits (`commands.rs:57-68`), so one slow broadcast stalls every write command (unlock, workspace toggle). On Android, JNI `block_on`s network on the calling thread (`comrade_jni/src/lib.rs`), and the tap-to-talk path dispatches from a Vosk callback on the main looper — a latent ANR once unlock is wired. Wants narrower lock scopes. |
| P3 | M | **[fact] Unbounded in-memory growth in engines.** Vault inbox `Vec<VaultMessage>` grows forever and `inbox_snapshot` clones it all (`vault.rs:82,215,225-227`); Saathi `received: Vec` unbounded (`saathi.rs:77,257`) while its outbox is capped at 256. |
| P4 | L | **[fact] Sakha sync payload grows without bound.** Every `publish_sync` encodes the full doc state from `StateVector::default()` (`sakha.rs:175-179`), so event size grows with ledger history for the life of the pairing. |
| P5 | L | **[judgment] Triple websocket pools.** Sabha, Vault, and Sakha each construct their own `nostr_sdk::Client` (`runtime.rs:201-215`), tripling connections to the same relays; nostr-sdk supports a shared client/pool. |
| P6 | L | **[fact] Timeline reads decrypt the whole cache.** `chitthi_cache()` iterates, decrypts, and sorts every cached row per fetch with no pagination or eviction (`repository.rs:174-178`); fine at prototype scale, a cliff once incoming posts are persisted (Q2). |

**Healthy:** the event bus is a bounded `broadcast::channel(256)` where slow consumers lag rather than block producers (`runtime.rs:41`); heavy relay/decrypt work runs in spawned Tokio tasks off the UI thread (`runtime.rs:233-263`); JNI owns one process-global multi-thread runtime rather than per-call runtimes (`comrade_jni/src/lib.rs:47-55`).

### 3.4 Code Quality & Correctness

| # | Sev | Finding |
|---|---|---|
| Q1 | **H** | **[fact] Saathi's offline cache drops messages on drain failure.** On mDNS discovery the outbox is drained by `pop_front` → publish; a publish error only logs — the message is not re-queued (`saathi.rs:216-231`). Worse, drain fires immediately on discovery, before the gossipsub mesh has formed (mesh formation needs a heartbeat exchange), so `InsufficientPeers` failures are the *expected* case — the advertised store-and-forward behavior (`README.md:170`) likely loses most cached messages in practice. Zero tests exist for this module. Currently unreachable from any frontend (A1), which contains the blast radius but also means it's never been exercised. |
| Q2 | M | **[fact] The offline-first storage layer is built but barely used.** Incoming chitthis and DMs are never persisted — the event-loop callbacks only forward to the broadcast bus (`runtime.rs:238-245, 253-257`); only *outgoing* chitthis are cached (`runtime.rs:301-315`). The contacts, vault-cache, and ledger-persistence repository APIs (`repository.rs:142-235`) have zero production callers. Compounding it, the Vault subscription filters `.since(Timestamp::now())` (`vault.rs:157`) with no history backfill, so DMs received while the app is closed are silently lost. The "offline-first" story the storage crate was built for effectively exists only for one's own posts. |
| Q3 | M | **[fact] Money as `f64`.** `UpiPaymentIntent.amount_inr` (`vault.rs:25`) and `LedgerEntry.amount_inr` (`sakha.rs:72`) are floats, formatted with `{:.2}` into payment URIs. Classic representation risk for a feature that constructs real payment intents; should be integer paise. |
| Q4 | M | **[fact] Live feed discards threading it already knows how to parse.** `ChitthiDto::from_event` hardcodes `reply_to: None` (`runtime.rs:57-69`) even though `sabha.rs:122-134` implements NIP-10 parent resolution — so live chitthis lose reply structure, and the `build_chitthi_thread` tree builder is reachable only from the CLI demo. |
| Q5 | M | **[fact] Background event loops fail silently.** If a feed/inbox subscription errors, the spawned task `warn!`s and exits (`runtime.rs:243-246, 258-261`); no status/error event reaches any frontend, so the app looks unlocked and healthy while live updates are permanently dead for the session. |
| Q6 | L | **[fact] Misleading error taxonomy in Vault.** Relay add, subscription, and send failures are all mapped to `VaultError::EncryptionFailed` (`vault.rs:92,119,130,162,222`) — a connection refusal reports as "encryption failed" in every frontend. `DecryptionFailed`/`InvalidRecipient` variants exist but are never constructed (`error.rs:79-83`). Similarly, `sakha.rs:64` labels a decrypt failure `EncryptionError`. |
| Q7 | L | **[fact] AES-256-GCM seal/open is implemented three times** — `crypto.rs:123-149`, `storage/lib.rs:264-290`, `sakha.rs:39-65` — identical `[nonce | ct+tag]` envelopes with per-module error types. One shared helper (crypto.rs's) would do. |
| Q8 | L | **[fact] Hand-rolled JSON at the JNI boundary.** `generateKeypair` formats JSON with `format!` (`comrade_jni/src/lib.rs:113-117`) — an error message containing `"` would produce invalid JSON. Fine for the current messages; trivially replaced with the `json!` macro already used elsewhere in the same file. |
| Q9 | L | **[fact] NIP-10 tree builder recurses to reply-chain depth** over relay-supplied events (`sabha.rs:169-186`; `ChitthiThread::len` likewise, `sabha.rs:60-65`) — a maliciously deep chain overflows the stack. Only demo-reachable today; matters when Q4 is fixed. Also: the CLI claims "Persisted N Chitthi(s)" then discards the flush result (`src/main.rs:158`, `let _ = store.flush()`). |
| Q10 | L | **[fact] README's "zero unwrap in network/parsing paths" is nearly true.** Counterexamples are all benign-but-notable: infallible-by-construction `.expect` in `derive_symmetric_key` (`crypto.rs:116`) and listen-addr parse (`saathi.rs:152`), plus `expect("keygen")` on the CLI bootstrap path (`src/main.rs:406,430,443,447`). Engine network/parse paths hold the claim. |

**Healthy:** error handling is consistently typed (`thiserror` domain enums per engine, `error.rs`), notification loops degrade gracefully on bad input (warn-and-continue on undecryptable DM / bad base64 / bad CRDT update — `vault.rs:185-195`, `sakha.rs:226-248`), swallowed-error patterns (`let _ =`) appear only where correct (event-bus send with no subscribers, `runtime.rs:241`). No dead `pub` surface beyond the intentionally-unwired engines (A1).

### 3.5 Testing

| # | Sev | Finding |
|---|---|---|
| T1 | **H** | **[fact] CI never runs the Android unit tests.** `VoiceCommandTest`/`CommandDispatcherTest` exist and are meaningful, but `.github/workflows/ci.yml` runs only cargo; no `gradle test` anywhere (release.yml builds the APK without running Kotlin tests either, `release.yml:149-160`). The Kotlin voice grammar can regress silently. |
| T2 | **H** | **[fact] The desktop crate is never compiled by CI (or anyone).** `desktop/src-tauri` is excluded from the workspace (`Cargo.toml:13`) and no CI job builds it — `#[tauri::command]` signature drift vs. `main.js` invocations, or a plain compile error, ships undetected. `desktop/ui/main.js` (687 lines of real logic) has zero tests and no linting. |
| T3 | M | **[fact] Zero tests for `saathi.rs`** — the module with the audit's worst correctness bug (Q1). Its design (spawned swarm task, no injectable transport) resists testing; the command-channel seam exists and could be exercised with two in-process engines. |
| T4 | M | **[judgment] No cross-engine integration test and no known-answer crypto tests.** Nothing exercises unlock → engine build → broadcast → event-bus delivery against a mock/local relay; crypto tests are all self-round-trips with no fixed vectors (e.g., NIP-04 reference vectors, HKDF/AES KATs). The Rust↔Kotlin JSON contract is pinned by no test on either side, and the `nip96-http` feature is never compiled in CI (`cargo test --workspace` uses default features), so the real uploader can rot silently. |

**Healthy — and worth saying loudly:** this is a genuinely well-tested prototype where tests exist: `comrade_state` (transition graph, history), `comrade_storage` (13 unit + 6 durability tests incl. reboot cycles, wrong-PIN fail-closed, and adversarial scans proving no plaintext ever hits disk), relay router (10 pure-logic tests), media (full pipeline round-trip), `comrade_ui` (lock-gating, Send/Sync compile guarantees, serde round-trips), Kotlin voice parsing. Tests assert behavior, not execution. Roughly: storage/state/relay/media/ui ≈ well covered; vault/sakha ≈ partial (pure parts only); sabha engine, saathi, JNI, desktop JS ≈ uncovered.

### 3.6 Dependencies

| # | Sev | Finding |
|---|---|---|
| D1 | ~~M~~ | **[fact, resolved 2026-07-12]** `sled 0.34.7` was the foundation of the encrypted store, in a years-long 1.0-alpha limbo with known unresolved crash-recovery and memory issues. `comrade_storage` now persists to `redb` instead (see the decision log entry above); `sled` remains only as a one-time legacy-migration reader. |
| D2 | M | **[fact] No LICENSE file and no `license` field in any Cargo.toml** (verified by search). Legally "all rights reserved" — blocks any external contribution or reuse decision, and violates the org's new-repo standard. The APK build also strips upstream license notices (`build.gradle.kts:66` excludes `META-INF/{AL2.0,LGPL2.1}`) with no NOTICE strategy. |
| D3 | M | **[fact] Reproducibility gaps.** No Gradle wrapper committed (builds need a hand-matched system Gradle 8.5, `README.md:58`); `desktop/src-tauri` has **no Cargo.lock at all** (excluded from the workspace, never built); CI's cargo steps run without `--locked`. |
| D4 | L | **[fact] Minor version hygiene.** Duplicate `rand 0.8.6` + `0.9.4` in the lock (transitive); loose `"1"`-style version reqs (acceptable with a committed lockfile); Android toolchain a generation behind (Kotlin 1.9.22, Compose BOM 2024.02 — dated, not risky). No cargo-audit/cargo-deny gate exists (see O2). |

**Healthy:** the dependency set is lean and purposeful — every crypto crate is a RustCrypto/rust-bitcoin standard, no `openssl`, no unnecessary frameworks; the workspace `Cargo.lock` is committed; nostr-sdk 0.44 / libp2p 0.56 / yrs 0.21 are current-generation.

### 3.7 DevEx & Operations

| # | Sev | Finding |
|---|---|---|
| O1 | **H** | **[fact] CI coverage stops at the workspace boundary.** `ci.yml` is Rust-workspace-only: Android compile+test, desktop crate compile, and JS lint are all absent (details in T1/T2). One broken frontend commit is undetectable. |
| O2 | M | **[fact] No dependency/security scanning in CI** (no cargo-audit/cargo-deny job) for a crypto-heavy project, and the release workflow that handles signing secrets uses tag-pinned (not SHA-pinned) third-party actions (`release.yml:83,127,191`). |
| O3 | M | **[fact] Release APKs are debug-signed with *ephemeral per-runner keys* and ship without the voice model.** Without signing secrets, each CI runner mints its own debug keystore, so every release is signed by a *different* key — sideloaded upgrades fail on signature mismatch. And `release.yml` never runs `fetch-vosk-model.sh`, so the flagship "Hey Comrade" feature is inert ("Voice model missing") in every official APK. The debug fallback itself is documented (`README.md:124`), the upgrade-breaking key rotation and missing model are not. |
| O4 | M | **[fact] No observability story on any shipping frontend.** `tracing` is used well inside the crates, but neither the JNI library nor the desktop shell installs a subscriber (no `tracing_subscriber` in `comrade_jni` or `desktop/src-tauri/src/lib.rs`) — on Android and desktop every `info!/warn!` (including decrypt failures and dead event loops, Q5) is dropped silently. Only the CLI initializes logging (`src/main.rs:470-476`). |
| O5 | L | **[fact] The documented desktop build cannot complete.** `tauri.conf.json:26-31` references `icons/32x32.png` etc., but `desktop/src-tauri/icons/` contains only a README — `cargo tauri build` fails; the README quick-start (`README.md:93-97`) omits icon generation. |
| O6 | L | **[fact] CI friction items.** Unpinned `stable` toolchain + `-D warnings` means new clippy releases break CI spontaneously; `ci.yml:3-6` triggers on both `push: ["**"]` and `pull_request`, running everything twice per PR; the release `version` input is interpolated unvalidated into shell commands and the git tag (`release.yml:157-165,193`); `cargo install cargo-ndk` is uncached per matrix job. |
| O7 | L | **[judgment] Setup friction is honest but manual** — Rust + NDK r27c + cargo-ndk + JDK17 + system Gradle + optional Vosk model for Android; tauri-cli + system webview for desktop. A `justfile` codifying the README's command blocks would cut onboarding errors. |

**Healthy:** the Rust CI lane is exactly right for this maturity: fmt-check, `clippy --workspace --all-targets -D warnings`, full test run, with cargo caching (`ci.yml:25-43`). The release pipeline's structure (test → cross-compile 3 ABIs → assemble → GitHub Release) is sound. `.gitignore` correctly excludes the Vosk model, jniLibs, and stores; `rustfmt.toml` pins style.

### 3.8 Documentation

| # | Sev | Finding |
|---|---|---|
| N1 | **H** | **[fact] README materially oversells the product.** The engine table (`README.md:8-17`) lists Saathi ("works without internet"), NIP-65 routing, and encrypted media as features of the client — none is reachable from any frontend (A1). "The UI logic lives once in comrade_ui and is reused by every frontend" (`README.md:47-49`) is false for the CLI, which does not even depend on `comrade_ui` (root `Cargo.toml:52-60`) and duplicates identity/store handling; Android uses ~4 of 10 bridge functions; the desktop carries its own mock/logic layer. For a repo whose next developers will trust the README, this is the most expensive doc bug. |
| N2 | M | **[fact] No threat model or security document** for a product whose value proposition is a security property. S3/S4/S8/S11 and D1 are all "fine if documented, damning if discovered" items. |
| N3 | M | **[fact] Build-instruction drift.** The README's "Rust ≥ 1.75" prerequisite is stale — the agent sweep reports the locked dependency tree needs rustc ≈1.83 (**not independently re-verified**; confirm with `cargo +1.75 check` before relying); the Tauri quick-start fails on missing icons (O5); Gradle 8.5 must be hand-installed (D3). |
| N4 | L | **[fact] Small doc/reality mismatches.** `desktop/README.md:3-4` calls the frontend "HTML/Tailwind" and then correctly says "dependency-free vanilla-JS" 20 lines later — no Tailwind exists. The Architecture-notes bullets are mostly accurate (thread-safety ✓, 256-message Saathi cache ✓, fail-closed ✓, zero-unwrap ≈ Q10, zeroization partial S5). No CONTRIBUTING, no ADRs, no CLAUDE.md despite an AI-assisted-PR workflow; per-crate rustdoc module headers (`//!`) are genuinely good, which softens this. |

**Healthy:** module-level rustdoc is a real strength — every crate opens with an accurate design essay (e.g. `storage/lib.rs:1-26`, `runtime.rs:1-24`); the voice feature's "Honest scope" README section (`README.md:145-151`) is exemplary self-aware documentation — the same candor applied to the engine table would resolve N1.

---

## 4. Improvement Strategy

### Theme 1 — Close the promise/reality gap (drives N1, A1, A2, A3, A5, Q4)
**Target state:** every feature the README claims is either reachable from at least one frontend or explicitly labeled *experimental / not yet wired*. The state machine owns engine lifecycle: entering OffGridTravel disconnects relay engines (and, if product says so, starts Saathi); pairing actually calls `pair_with`; identity has one code path.
**Principle:** in a privacy product, credibility is the product. An honest smaller claim beats an aspirational bigger one — the false "relays paused" toast is worse than not having the feature.
**Trade-off:** wiring Saathi/media properly is weeks of work; re-scoping the README is an afternoon. Do the README first (M1), wire features by product priority later (M2+). **Do NOT** try to wire all three dormant engines at once.
**Done signals:** README table has an explicit status column; the Off-Grid toggle really disconnects (verifiable with `netstat`); Android can unlock + post + read timeline end-to-end; `sync_ledger` succeeds after a real pairing flow; engines with no non-test call sites are flagged experimental and out of the feature table.

### Theme 2 — Make the crypto match the marketing (S1, S2, S3, S8, N2)
**Target state:** DMs on NIP-44 (send + receive; NIP-04 kept read-only for backward compat), crash-safe re-keying (done — see D1/M1-2), a passphrase policy with strength feedback, and a `SECURITY.md` threat model that states plainly what is and isn't protected (plaintext keys in storage tables, no forward secrecy for Sakha, LAN-visible mesh).
**Principle:** composing audited primitives (which this codebase already does well) is necessary but not sufficient — protocol choice and key-management lifecycle are where privacy products actually fail.
**Trade-off:** skip a Sakha ratchet (Signal-style forward secrecy) for now — document it instead; it's XL work with low prototype payoff. **Do NOT** raise Argon2 params before profiling on the low-end Android target (unlock latency is UX-visible).
**Done signals:** new DMs are NIP-44-encrypted (gift-wrap decision recorded); a kill-the-process-mid-`change_pin` test passes; passphrase < 8 chars rejected at every frontend; `SECURITY.md` exists and matches the code.

### Theme 3 — CI and releases cover what can break (T1, T2, O1, O2, O3, D2, D3)
**Target state:** one PR gate that compiles/tests all three frontends: cargo lane (exists) + `gradle test` lane + `cargo check` of `desktop/src-tauri` + cargo-audit; wrapper committed; LICENSE decided; releases signed with a stable key and shipping the voice model (or explicitly labeled voice-less).
**Principle:** the safety net comes before the refactors it protects (this is milestone M0 for a reason); a release pipeline that produces non-upgradeable artifacts is worse than none.
**Trade-off:** don't build APKs or run emulator tests on PRs — JVM unit tests and compile checks give 90% of the signal at 10% of the cost. **Do NOT** add coverage-percentage gates yet; the test culture is already good, and gates on a moving prototype create ceremony.
**Done signals:** a deliberately broken Kotlin test fails CI; a deliberately broken Tauri command signature fails CI; CI fails on a crate with a known RUSTSEC advisory; two consecutive releases install as an upgrade on the same device.

### Theme 4 — Feed, persistence, and memory discipline (P1, P2, P3, Q2, Q4)
**Target state:** the Sabha subscription is scoped (follow set + limit, never author-unbounded), in-memory caches are ring buffers, incoming chitthis/DMs are persisted to the already-built repositories with backfill since last-seen, and blocking work is off the async threads.
**Principle:** unbounded anything (subscription, Vec, DOM) is a latent outage; the repository layer for persistence already exists and is the best-tested code in the repo — use it.
**Done signals:** desktop stays responsive against a live relay for 10 minutes; `state.chitthis` and Vault inbox have hard caps; killing and restarting the desktop app shows DMs received while it was closed (or the limitation is documented); no lock is held across a KDF or network await.

### What NOT to fix (explicit)
- **The three thin bridge layers** (CLI/JNI/Tauri command shims) — they duplicate marshalling, not logic; unifying them buys nothing. (Making the CLI depend on `comrade_ui` is worthwhile only as part of M2 wiring work, not as its own project.)
- ~~Sled → redb migration~~ — **done 2026-07-12** (see decision log); no longer deferred.
- **UPI feature hardening beyond f64 → paise** — product direction is unclear (see Open Questions); don't invest until it is.
- **Gendered `PairRole` naming, Hindi nomenclature** — product/brand decisions, not engineering defects; flagged in Open Questions only.
- **Triple nostr clients (P5), AES helper triplication (Q7), `comrade_state` history growth** — real but cheap debt; batch them into M3 or into adjacent M2 work, don't schedule standalone.

---

## 5. Task Plan

Effort: **S** < 2h · **M** ≈ half-day · **L** 1–2 days · **XL** needs breakdown. Risk = risk *of making the change*.

### M0 — Safety net

| ID | Task | Files | Acceptance criteria | Effort | Risk | Deps |
|---|---|---|---|---|---|---|
| M0-1 | Commit Gradle wrapper (8.5) | `android/gradle/`, `android/gradlew*` | `./gradlew test` runs from a clean checkout with no system Gradle | S | none | — |
| M0-2 | Android test lane in CI | `.github/workflows/ci.yml` | CI job runs `./gradlew test` (JVM unit tests only); a broken `VoiceCommandTest` fails the PR | S | low | M0-1 |
| M0-3 | Desktop compile lane in CI + commit its lockfile | `.github/workflows/ci.yml`, `desktop/src-tauri/Cargo.lock` | `cargo check` (or `clippy -D warnings`) of `desktop/src-tauri` on ubuntu w/ webkit deps; lockfile committed; a bad command signature fails the PR | M | low | — |
| M0-4 | cargo-audit/deny gate + `--locked` + SHA-pin actions | `.github/workflows/*.yml`, `deny.toml` | CI fails on RUSTSEC advisories; cargo runs `--locked`; third-party actions pinned by SHA in release.yml; single trigger per PR | S | low | — |
| M0-5 | Crash-safety regression test for `change_pin` (test first, fix in M1-2) | `crates/comrade_storage/tests/` | A test that interrupts re-keying demonstrates the corruption (initially an `#[ignore]`d xfail documenting the bug) | M | none | — |

### M1 — Critical security & correctness

| ID | Task | Files | Acceptance criteria | Effort | Risk | Deps |
|---|---|---|---|---|---|---|
| M1-1 | ~~Migrate DMs to NIP-44~~ **— done 2026-07-12** (send + decrypt both, NIP-04 decrypt-only legacy) | `crates/comrade_core/src/vault.rs` | Landed as full NIP-17 gift-wrap (chosen over plain NIP-44-in-Kind-4 — see OQ4 update below): new DMs are Kind-14 rumors → Kind-13 seals → Kind-1059 gift wrap; old NIP-04 DMs still decrypt read-only; round-trip tests added; README table updated | L | medium | — |
| M1-2 | ~~Make `change_pin` atomic~~ **— done 2026-07-12** | `crates/comrade_storage/src/lib.rs` | Landed as a side effect of the redb migration: the whole rekey runs in one redb write transaction, so an interrupted rekey never commits. The regression test no longer needs `#[ignore]`. | M | medium | M0-5 |
| M1-3 | README truth pass (status column, wired vs experimental) + remove false Off-Grid toast | `README.md`, `desktop/ui/main.js` | Every table row states its wiring status; comrade_ui claim corrected; the "relays paused" toast is gone or true | S | none | — |
| M1-4 | Passphrase policy + terminology | `desktop/ui/main.js`, `src/main.rs`, `comrade_ui/src/lib.rs` | Min-length (≥8) enforced at unlock creation everywhere; "PIN" renamed passphrase in user-facing text; CLI stops echoing passphrase (rpassword) | M | low | — |
| M1-5 | Scope the Sabha feed + cap client memory | `crates/comrade_core/src/sabha.rs`, `comrade_ui/src/runtime.rs`, `desktop/ui/main.js` | Subscription takes authors/limit (default: own + follows, `limit(200)`); `state.chitthis` ring-capped; Vault inbox capped; desktop survives live relay soak | M | medium | — |
| M1-6 | Android privacy hygiene | `AndroidManifest.xml`, `MainActivity.kt` | `allowBackup=false`; nsec masked behind a reveal tap; `FLAG_SECURE` on key screen | S | low | — |
| M1-7 | Pin Vosk model checksum | `scripts/fetch-vosk-model.sh` | Download verified against a committed sha256; mismatch aborts | S | none | — |
| M1-8 | Tauri CSP | `desktop/src-tauri/tauri.conf.json` | Strict CSP (`default-src 'self'`) set; app functions normally | S | low | — |
| M1-9 | Saathi drain re-queues on failure | `crates/comrade_core/src/saathi.rs` + new tests | Failed drain publishes return the message to the cache front; two-engine in-process test proves store-and-forward | M | low | — |
| M1-10 | Release APK integrity | `.github/workflows/release.yml` | Releases require signing secrets (or use one stable committed debug keystore for pre-releases); Vosk model fetched (with M1-7 checksum) before assemble, or release notes state voice is absent; version input validated (`^[0-9]+\.[0-9]+\.[0-9]+$`) | M | low | M1-7 |

### M2 — High-leverage improvements

| ID | Task | Files | Acceptance criteria | Effort | Risk | Deps |
|---|---|---|---|---|---|---|
| M2-1 | Android vault flow: unlock screen, timeline list, post composer over existing JNI | `android/.../MainActivity.kt`, new Compose screens | Unlock → voice "post hello" succeeds → timeline reads it back; JNI calls run off the main looper (fixes the P2 ANR); voice "new identity" routes through the runtime (fixes A5) | L | medium | M1-4 |
| M2-2 | Desktop DM send + real pairing | `commands.rs`, `runtime.rs`, `sakha.rs`, `vault.rs`, `main.js` | `send_dm` command exists; pairing state moves behind interior mutability (or pair-before-Arc) so `pair_with` is reachable through the bridge; pair token reaches `pair_with`; `sync_ledger` succeeds when paired and fails honestly when not | L | medium | M1-1 |
| M2-3 | Persist + backfill Vault inbox; persist incoming chitthis | `vault.rs`, `runtime.rs` (use existing repositories) | DMs and received chitthis survive restart; subscription backfills since last-seen timestamp | M | medium | M1-1 |
| M2-4 | Engine lifecycle owner in `ComradeRuntime` | `runtime.rs`, `comrade_state` | OffGridTravel transition disconnects Nostr engines (and starts Saathi only if product says so — OQ2); state doc updated; `netstat`-visible | L | medium | M1-9, OQ2 |
| M2-5 | Money as integer paise | `vault.rs`, `sakha.rs`, DTOs, `main.js` | `amount_paise: u64` end-to-end; URI formatting from integers; tests updated | M | low | — |
| M2-6 | `spawn_blocking` for KDF/store-open + narrow lock scopes | `comrade_ui/src/lib.rs`, `runtime.rs`, `commands.rs` | Unlock no longer stalls the runtime; no guard held across KDF or relay awaits | M | low | — |
| M2-7 | Thread the live feed | `runtime.rs`, `sabha.rs` | `ChitthiDto::from_event` populates `reply_to` via the existing NIP-10 resolver; tree builder gets an iterative implementation or depth cap | S | low | M1-5 |

### M3 — Quality & polish

| ID | Task | Files | Acceptance criteria | Effort | Risk | Deps |
|---|---|---|---|---|---|---|
| M3-1 | `SECURITY.md` threat model | new file | States protections + explicit non-goals (S3/S4/S8/S11, sled status D1); linked from README | M | none | M1-* |
| M3-2 | Error taxonomy + surfaced loop failures | `vault.rs`, `sakha.rs`, `error.rs`, `runtime.rs` | Relay/subscription failures use accurate variants; a `BridgeEvent::EngineStatus` (or similar) tells frontends when a feed/inbox loop dies (Q5) | M | low | — |
| M3-3 | JS lint + smoke tests for `main.js` | `desktop/ui/`, CI | Biome/ESLint gate; a DOM-free unit harness for the pure logic | M | low | M0-3 |
| M3-4 | On-device logging | `comrade_jni/src/lib.rs`, desktop `lib.rs` | JNI init installs an android-logger tracing subscriber; desktop initializes tracing; decrypt failures visible in logcat | S | low | — |
| M3-5 | LICENSE + Cargo license fields + NOTICE handling | root, all Cargo.toml, `build.gradle.kts` | License chosen (needs owner decision, OQ5) and applied; META-INF exclusion revisited | S | none | OQ5 |
| M3-6 | `justfile` + desktop icons + MSRV truth | root, `desktop/src-tauri/icons/` | `just android-apk`, `just desktop-dev`, `just test-all` work; `cargo tauri build` succeeds (icons generated via `tauri icon`); README states the real MSRV (verify the sweep's 1.83 claim with `cargo +1.75 check`) | M | none | — |
| M3-7 | Sakha defense-in-depth: sender check + incremental sync | `sakha.rs` | Handler verifies `event.pubkey == partner_pk`; sync encodes diff vs. peer state vector | M | medium | M2-2 |
| M3-8 | Dedup AES helpers; drop unused `comrade_state` dep from core; JNI `json!` everywhere | `crypto.rs`, `storage/lib.rs`, `sakha.rs`, `comrade_core/Cargo.toml`, `comrade_jni/src/lib.rs` | One shared seal/open; `cargo udeps` clean; no hand-rolled JSON | S | low | — |

### Quick wins (high impact ÷ effort, all S)
**M0-1** (wrapper) · **M0-4** (cargo-audit + SHA-pins) · **M1-3** (README truth pass + false toast) · **M1-6** (allowBackup/FLAG_SECURE) · **M1-7** (model checksum) · **M1-8** (CSP) · **M2-7** (live-feed threading) · **M3-4** (device logging).

### Top-3 implementation sketches

**M1-1 · NIP-44 migration.**
Approach: nostr-sdk 0.44 exposes `nip44::encrypt/decrypt` and gift-wrap helpers. Replace `send_dm`'s `nip04::encrypt` (`vault.rs:118`) with NIP-44 payload encryption; ideally wrap in NIP-59 gift wrap (Kind 1059) so recipient metadata is also hidden — that changes the inbox filter from Kind 4 to Kind 1059. In the notification handler, branch on kind: 1059 → unwrap+NIP-44; 4 → legacy `nip04::decrypt` (read-only). Keys: gift-wrap uses ephemeral keys per message — nostr-sdk handles this. Steps: (1) add `send_dm_nip44`, migrate `send_dm` to it; (2) extend subscription filter to both kinds; (3) split decrypt paths; (4) tests: NIP-44 round-trip, official NIP-44 test vectors, legacy NIP-04 still readable; (5) update README row and error variants (fixes Q6 partially). Gotchas: gift-wrap timestamps are randomized (spec) — don't assert `created_at` ordering in tests; interop with other clients only works if they also do 1059 — decide plain-NIP-44-in-Kind-4 vs full gift-wrap in review (OQ4); `.since(now)` interacts with gift-wrap randomized timestamps (subscribe with a ~2-day skew window per spec).

**M1-2 · Atomic `change_pin`.**
Approach: staged re-encryption with a version marker. Steps: (1) write `rekey_state = {new_salt, new_verify_token, status: "in-progress"}` to META under a *new* key slot while keeping old salt/token valid; (2) copy every tree's values re-encrypted into `<tree>__rekey` siblings; (3) single meta write flips `active_salt` → new + `status: done`; (4) swap trees (rename or read-indirection) and drop old ones; (5) on `open`, detect stale `in-progress` → discard `__rekey` trees and open with the old key (rollback semantics). Convert M0-5's xfail test into: kill after each phase (inject a failpoint closure between steps), reopen, assert either fully-old or fully-new, never mixed. Gotchas: sled has no cross-tree transactions — the single-key meta flip is the linearization point, so the flip must be one `insert` of one composite value (not two); `Zeroizing` the old key after the flip; document that concurrent writers must be excluded during rekey (take `&mut self` — already the case).

**M1-5 · Scoped Sabha feed.**
Approach: make the filter an explicit input instead of a hardcoded firehose. Steps: (1) `subscribe_chitthi_feed(filter_spec, cb)` where `FilterSpec { authors: Vec<PublicKey>, since_secs, limit }`; default from stored contacts + own key (the `Contact` repository already exists, `repository.rs:142-159` — this also gives it its first production caller); fall back to `limit(100)` + own-key-only when contacts are empty — never author-unbounded; (2) in `runtime.rs:243` build the spec from the store at unlock; (3) cap `state.chitthis` in `main.js` (e.g. 500, drop tail on prepend) and cap the Vault inbox Vec symmetrically (P3); (4) soak-test against a live relay behind an `--ignored` integration test. Gotchas: an empty-follow cold start means an empty feed — product may want a curated bootstrap list instead (flag at review); the `since` replay on reconnect can double-deliver — the desktop's `seenChitthi` set already dedups, keep server-side `limit` modest so replay is cheap.

---

## 6. Open Questions (need a human decision)

1. **What is Comrade's actual near-term product scope?** The audit found three tiers: working (Sabha/Vault/storage on desktop), stubbed (couple pairing, DM send, Android vault), dormant (Saathi, gossip routing, media). M2 sequencing depends entirely on which tier-2/3 items are real goals vs. exploration. (Drives M2-1/2/4, N1.)
2. **Should OffGridTravel actually start the Saathi mesh** (with its plaintext-on-LAN semantics, S11), or is off-grid mode "relays off, local cache only" for now? The honest cheap answer may be the latter — but either way the current false "relays paused" claim must go (M1-3/M2-4).
3. **UPI payments: feature or demo?** Auto-parsing attacker-controlled DMs into payment URIs (S12) plus `f64` money (Q3) is fine for a demo and unacceptable for a shipped payments flow. If it's a feature, it needs a confirm-flow design and possibly compliance review.
4. ~~**NIP-04 compatibility window**~~ — **resolved 2026-07-12**: full gift-wrap (NIP-17/NIP-59) was chosen over plain NIP-44-in-Kind-4, since "no metadata leakage" was an explicit product requirement and gift-wrap is the only option that hides the real sender and timestamp, not just content. Old NIP-04 DMs stay decrypt-only indefinitely for now (no expiry has been set); revisit once usage data shows how many peers are still on old clients.
5. **License** — none exists (D2). Owner must choose (MIT/Apache-2.0 dual is the Rust default) before any external sharing.
6. **Org-standard alignment** — repo uses GitHub Actions (org standard says GitLab CI for new repos) and public registries (org mandates Auros registries for new projects; unclear whether that policy covers crates.io/Maven Central for Rust/Android). Needs a platform-owner ruling; not adjudicated as a defect here.
7. **Gendered pairing roles** — `PairRole::Sakha/Sakhi` are documented as "Boyfriend/Male partner" / "Girlfriend/Female partner" (`comrade_state/src/lib.rs:15-20`) and desktop themes key off them. Product/inclusivity call, flagged for awareness.
8. **Passphrase UX floor** (S3) — what unlock friction is acceptable on the target low-end Android hardware? Determines both the policy (M1-4) and whether Argon2 params can be raised.

---

## 7. Session-parity roadmap (communication features)

_Added 2026-07-12. The owner's direction is "all the communication functionality of
[session-android](https://github.com/session-foundation/session-android)". This
section is the honest gap map: what Comrade already has, what is close, and
what is genuinely large. Session runs on its own onion-routed network
(oxen/lokinet) with its own protocol; Comrade speaks Nostr — parity therefore
means *feature* parity, not protocol compatibility._

| Session feature | Comrade today | Gap / next step |
|---|---|---|
| 1:1 E2E DMs | ✅ NIP-04 Kind-4 DMs, offline history, live delivery, **replies** (NIP-10 `e` tag) + **delivered/read receipts** | Upgrade to NIP-44 + gift wrap (M1-1) — Session's Signal-protocol-grade encryption is the bar; NIP-04 is deprecated and unauthenticated |
| Account = keypair, no phone number | ✅ secp256k1 keypair, npub address | — (same model) |
| Display name + optional avatar | ◐ @handle published/searched (Kind-0, retried + republished as of 2026-07-12); chats titled by handle | Avatars: publish `picture` in Kind-0; render (needs image pipeline on Android) |
| Local nicknames for contacts | ✅ per-contact alias, editable from the conversation header (2026-07-12) | — |
| Find people by ONS name / ID | ◐ NIP-50 handle search on dedicated relays + direct npub lookup (2026-07-12) | NIP-05 DNS-verified names as the ONS analogue; QR-code key exchange |
| Message requests (stranger DMs gated) | ✅ strangers land in a *requests* bucket, not the chat list; accept shares your @handle + acks their messages, block drops future DMs (engine + bridges tested; UI wired desktop + Android) | — |
| Read receipts + typing indicators | ◐ **delivered + read receipts** wired (control envelopes over the DM channel, accepted conversations only; status ticks on outgoing bubbles) | Typing indicators; a privacy toggle to disable receipts |
| Disappearing messages | ✗ | Per-conversation TTL enforced in the local store + NIP-40 expiration tags |
| Attachments / voice messages | ◐ media pipeline (NIP-94/96 + Blossom) wired on desktop **and Android send** (picker → encrypt → upload → deliver) | In-thread rendering of received media on Android; a dedicated voice-note recorder |
| Closed groups | ✗ | NIP-EE (MLS) is the serious path; a simpler interim is NIP-4x group DMs — needs a design decision |
| Communities (open groups) | ◐ public Chitthi feed exists (different shape) | Not a priority; Nostr public feeds already cover the "open square" role |
| Calls (voice/video) | ◐ signaling engine + call log **tested**; desktop wired (webview WebRTC); STUN default + configurable TURN | Android `org.webrtc` native media (§8.1); group calls stay out of scope (SFU) |
| Multi-device / linked devices | ✗ (one vault per device) | nsec export/import behind the passcode door is the pragmatic first step |
| Onion-routed transport | ✗ (direct WSS to relays) | Different network model; Tor/proxy support at the socket layer is the realistic analogue |
| Block / delete conversation | ✗ | Local block list (drop DMs by pubkey in the vault callback) + history delete |

**Sequencing recommendation.** (1) NIP-44/gift-wrap (M1-1 — encryption honesty
first), (2) message requests + block list (safety), (3) Android media +
voice notes (most-missed daily feature), (4) disappearing messages,
(5) nsec export/import, (6) groups (design doc first), (7) receipts/typing,
(8) calls last. Each lands as its own PR with tests; nothing gets a README
checkmark before it's wired end-to-end (Theme 1 discipline).

> **Note (2026-07-12, owner direction):** the wellbeing north star in §8 now
> governs priority. Communication features from this table matter in so far
> as they serve the "stay connected to a loved one" pillar (E2E DMs, voice
> notes, media, disappearing messages) — full messenger parity (groups,
> communities, calls) is deprioritised, not deleted.

---

## 8. Product north star — a (mental) wellbeing companion

_Added 2026-07-12 from owner direction: "the primary use case of Comrade is
to be your (mental) wellbeing companion — journal, help brainstorming,
therapy; write down any thoughts in chitthi/voice recording anonymously;
stay connected to your loved one who might be of help, who's with you
always."_

This is a re-framing, not a rebuild: the architecture already carries most
of the load. Mapping each pillar to what exists:

| Pillar | What exists today | What's missing (the actual work) |
|---|---|---|
| **Journal** | Encrypted-at-rest store (Argon2id + AES-256-GCM) seals anything we write; Vosk speech-to-text runs fully on-device; the voice pipeline (`OneShotRecognizer`, wake word) is wired | A `journal` tree + typed repository (entry = text, optional mood, timestamp), a Journal tab (write / dictate / browse by day), and a "dictate → entry" voice command. **Smallest real feature; all pieces exist. Build first.** |
| **Anonymous thoughts (chitthi / voice)** | Public Chitthi broadcast works; voice dictation works | True anonymity needs **per-post ephemeral keys**: sign each anonymous Chitthi with a throwaway keypair (never the identity key), no reuse across posts, so posts can't be linked to you *or to each other*. Also: strip `created_at` precision, publish via a relay subset. Without this, "anonymous" would be a false promise — the current broadcast is pseudonymous under your permanent key. |
| **Stay connected to a loved one** | This is exactly what Sakha/Sakhi was built for: a cryptographically isolated couple space (Yrs CRDT + AES-256-GCM) with engine-level tests — plus E2E DMs already shipping. The named-chats/alias work (this PR) makes the loved-one thread feel human | The pairing handshake is engine-complete but unreachable from any UI (A1 in §3.2). Wire pairing + a warm, dedicated "your person" surface: pinned thread, shared journal/ledger, maybe a lightweight "thinking of you" signal. |
| **Brainstorming / reflective companion ("therapy")** | Voice in (Vosk) and voice out (TTS) exist; the command dispatcher gives a slot to hang a conversational agent on | The companion itself. **Two honesty gates before building:** (1) an LLM companion is *not therapy* and must never present as one — reflective prompts, journaling nudges, brainstorming, plus crisis-referral hand-offs (e.g. helpline numbers) when distress cues appear; (2) privacy-first means the model should run **on-device** (small quantised model) or not at all — routing raw mental-health disclosures to a cloud API contradicts the product's core promise. Model choice and scope need an owner decision (OQ9). |

**Sequencing recommendation (supersedes §7 order):**
1. **Journal** — encrypted entries + voice dictation (foundations complete).
2. **Anonymous Chitthi** — ephemeral-key posting (small, but the privacy
   claim must be engineered, not asserted).
3. **Loved-one space** — surface Sakha/Sakhi pairing in the UI; DM quality
   items from §7 (voice notes, media on Android, disappearing messages)
   slot in here.
4. **Reflective companion** — design doc + on-device model decision first
   (OQ9), then a deliberately narrow v1 (journaling prompts over your own
   recent entries, opt-in).

**New open question (OQ9):** which model/runtime for the companion —
on-device (llama.cpp-class quantised model: private, heavy on low-end
phones) vs. none (template-based reflective prompts only) vs. cloud
(fastest, but breaks the privacy promise for the most sensitive data a
user has)? Owner call required before any companion code.

### 8.1 Calls — voice & video (owner request, 2026-07-12)

> **Status (landed).** The **signaling layer is built and tested**: a
> `CallEnvelope` (SDP offer/answer, trickled ICE, ringing/busy/hangup) rides the
> encrypted Vault DM channel exactly as sketched below, gated so strangers can't
> ring you before their message request is accepted. `comrade_ui` exposes
> `place_call` / `send_call_signal` / `hangup_call` / `log_call` / `call_history`
> and configurable ICE (`call_ice_servers` / `set_turn_server`, public STUN by
> default). **2026-07-12:** the ICE strategy is now STUN-first by design —
> `comrade_core::call::IceStrategy` + `ice_servers_for` and
> `ComradeRuntime::call_ice_servers_for` let a call start on free, blind-to-the-
> call public STUN (`place_call`'s initial offer already does), then explicitly
> ask for the TURN-inclusive list once a frontend's `RTCPeerConnection` reports
> ICE never reaches `connected` (the CGNAT case) — engine-level and unit-tested.
> Both bridges (JNI + Tauri) carry the existing calls. **Desktop** drives real
> WebRTC in the webview. **2026-07-12 (Android):** the Android frontend now
> drives real `org.webrtc` media too — `mullu.comrade.call.CallManager` builds
> the `PeerConnectionFactory`/`PeerConnection`, captures mic+camera, and runs the
> same offer/answer/ICE handshake the desktop does, forwarding every payload
> through `ComradeCore` (so the NIP-59 DM routing is reused unchanged); the
> `MainActivity` event pump feeds `IncomingCallSignal` back in (it was previously
> dropped) and raises an incoming-call notification, and `CallScreen.kt` renders
> the ringing/connecting/active/ended states with `SurfaceViewRenderer` video and
> earpiece/speaker routing. The STUN→TURN fallback (caller widens to
> `call_ice_servers_for("stun_and_turn")` and ICE-restarts on a failed
> connection) is wired on Android; desktop still does not restart ICE.
> Dependency note: the historical `org.webrtc:google-webrtc` prebuilt is gone
> from public Maven (JCenter-only, shut down 2021), so Android depends on the
> Maven-Central-published `io.github.webrtc-sdk:android`, which keeps the same
> `org.webrtc.*` package. Honest scope: calls work while the app process is alive
> (foreground / held by an activity); always-on background delivery needs a
> connection service or push (AUDIT §7). OQ10 (below) is still open.
> **2026-07-15:** an opt-in `RelayConnectionService` foreground service now
> keeps the vault-unlocked process alive (and the event drain running) while
> backgrounded, so an accepted DM or incoming call still notifies without a
> visible Activity (decision log, COMMS-01). Honest scope, still: this covers
> backgrounded-*but-unlocked* only — process death (OOM-kill, force-stop,
> reboot) still drops the in-memory vault key, and there is still no
> push-wakeup path for a killed process; that remains a separate, deliberately
> out-of-scope decision. TURN gained time-limited REST credentials and a
> turnkey self-hosted coturn deployment (COMMS-02), plus a two-peer
> signaling/call test suite (COMMS-03) and call-setup race fixes (COMMS-05) —
> see the decision log for all three.

Session-style calling for the loved-one pillar. Design sketch, honest about
size (this is a **multi-PR epic**, comparable to everything built so far):

- **Media**: WebRTC — the only realistic cross-platform stack. Android:
  `org.webrtc` (Google's prebuilt libwebrtc AAR, ~10 MB — dwarfs our whole
  `.so`); desktop: the webview's built-in WebRTC (`getUserMedia` etc. work
  inside Tauri), so the desktop side is mostly JS.
- **Signaling**: SDP offers/answers + ICE candidates as ephemeral encrypted
  events over the existing Vault DM channel (the community direction is the
  NIP-100 draft; we'd carry the same payloads over NIP-04 now, NIP-44 after
  M1-1). No new infrastructure needed for signaling.
- **The hard truth — NAT traversal**: P2P connects directly for maybe
  60-70% of real-world pairs (STUN only, e.g. public Google/Cloudflare
  STUN). The rest (CGNAT — very common on Indian mobile carriers) need a
  **TURN relay**, which is real server infrastructure someone must run and
  pay for, and which sees (encrypted) media flow metadata. Session solves
  this with its own onion network; we don't have one.
- **Sequencing**: (1) voice-only, STUN-only, 1:1 — honest "call may not
  connect on some networks" UX; (2) TURN decision (OQ10); (3) video;
  (4) group calls never (SFU territory — out of scope).
- **OQ10:** who runs TURN? Options: none (accept ~30-40% connect failure),
  self-hosted coturn (cost + ops + a server that sees IP pairs), or a paid
  TURN service (fastest, least private). Owner call before starting calls.
  (**2026-07-15:** the self-hosted-coturn option is now turnkey —
  `deploy/coturn/`, time-limited REST credentials via
  `comrade_core::call::mint_turn_rest_credentials` — which removes the
  *engineering* lift. Who operates it, and its ongoing cost, is still the
  owner's call.)

### 8.2 Watch party — listen/watch together (owner request, 2026-07-12)

Shared media consumption with the loved one / small groups. The sync problem
is small; the content problem is the real constraint:

- **What we can build cleanly**: a *sync-play control channel* — play,
  pause, seek, position-heartbeat messages over the existing E2E DM channel
  (or the Sakha CRDT for couple state). Each participant plays **their own
  copy or their own stream** of the content; Comrade only synchronises the
  clocks (drift correction on heartbeat, target < 300 ms — DM-over-relay
  latency is fine for play/pause, marginal for tight seek-sync; a later
  WebRTC data channel from 8.1 would make it tight).
- **What we must not build**: re-streaming or proxying licensed
  audio/video between users. That's both a copyright problem and a
  bandwidth problem. DRM'd platform content (Netflix/Spotify) cannot be
  frame-synced inside our app at all — the platforms prohibit and
  technically block it. The honest v1 targets: local files both sides
  already have, and embeddable sources (e.g. YouTube via the official
  embed player API, which exposes play/pause/seek).
- **v1 scope**: "Listen together" for a shared YouTube link or local audio
  file inside the loved-one space — piggybacks entirely on existing
  channels, no new infrastructure. Builds after the loved-one space
  (pairing UI) exists.

---

## Appendix — Review coverage

Every Rust source file, the CLI, all Tauri/desktop sources, the Android manifest/gradle files, and all primary Kotlin files were read in full. Lighter review: `desktop/ui/styles.css` and `index.html` (styling only), the Android interaction-session service files, `Theme.kt`, and resource XML (covered by the agent sweep only). The workflow's per-finding adversarial verification completed for the architecture dimension (all findings CONFIRMED); remaining dimensions were verified manually against source. One claim could not be independently re-verified and is flagged inline (N3: MSRV 1.83).
