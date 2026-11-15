# Comrade

A privacy-first companion for your (mental) wellbeing — journal your
thoughts (typed or spoken, on-device), share what you choose anonymously,
and stay end-to-end-encrypted-close to the people who hold you up. Built
entirely in Rust, with a shared view-model layer driving an Android
(Kotlin/Compose), desktop (Tauri), or CLI frontend. 

> **Vision status.** The wellbeing pillars (journal, anonymous thoughts,
> loved-one space, reflective companion) are the product's north star — the
> gap map and build order live in [`AUDIT.md` §8](AUDIT.md). What is shipped
> and working today is the table below; no pillar gets a checkmark before
> it's wired end-to-end.

## What it does

| Engine | Protocol | Feature | Status |
|--------|----------|---------|--------|
| **Sabha** | Nostr Kind-1 + NIP-10 | Public microblogging — the **Chitthi Feed**, with nested `ChitthiThread` reply trees | ✅ Wired (desktop + Android: broadcast + live feed; reply threading in live feed pending) |
| **Vault** | NIP-44 + NIP-17/NIP-59 (gift-wrapped); legacy NIP-04 read-only | End-to-end encrypted direct messages; replies (NIP-10 `e` tag, with a tappable quote that jumps to the original), delivered/read receipts, **emoji reactions**, `/pay` UPI intent detection | ✅ Send + receive wired (desktop + Android), offline chat history persisted. New DMs are NIP-44-encrypted and gift-wrapped (no sender/content metadata leaks to relays); a peer's older NIP-04 DMs still decrypt. Reactions ride the same sealed channel as a *control envelope*, **not** a public NIP-25 Kind-7 event — a kind-7 reaction tags both the event and its author in the clear, so reacting to a gift-wrapped DM would tell every relay these two keys are talking |
| **Message requests** | Conversation gate + Kind-4 profile share | A stranger's DM lands in a *requests* bucket, not your chat list; **your @handle is shared only when you accept**; accept / block; blocked keys are dropped | ✅ Engine + bridges tested; UI wired (desktop + Android) |
| **Comrades** | Presence beacons over the NIP-44/NIP-17 DM channel | Mark a contact as your **comrade** and get told when they come online — a green dot in the chat list and a notification. Presence is disclosed to the peers you choose and to no one else (never to a relay: the beacon is gift-wrapped), is **mutual by construction** (you see them once they choose you back — the UI says so), and expires on its own so a phone that dies doesn't leave a permanent green dot. **Online means the app is open** — a backgrounded phone still receives messages, but stops claiming anyone is at it. A comrade who **writes something and never sends it** — or who reaches for the breathing screen — raises one *"your comrade might need you"*: once, after the fact, the same one bit either way, so nobody can tell which happened | ✅ Wired (Android: Comrades screen + ★ toggle in any conversation + presence dots + notification channel; desktop: ★ toggle, dots, toast; engine, storage and both bridges tested) |
| **Profiles** | Nostr Kind-0 + NIP-50 | @username display handles: published with retry **and republished on every launch**, searched on dedicated NIP-50 relays; peers' handles cached locally so chats are titled by name; per-contact aliases | ✅ Wired (Android onboarding + settings + chat UI; desktop backend commands) |
| **Saathi** | libp2p mDNS + Gossipsub, carrying sealed `dak` frames | **Same-WiFi delivery with no internet.** A DM no relay will take is sealed and flooded to the local network: every peer sees an opaque frame (rotating recipient tag, no sender identity, padded length) and only the addressee can open it. An opened frame runs through the *same* ingress as a relay DM, so gating, receipts and dedup behave identically; a delivered receipt falls back to the mesh too, which is what clears the sender's outbox offline. The mesh runs whenever the vault is unlocked, so a phone *with* internet still receives from nearby phones without any. **Routing follows what is actually reachable** — with no relay connected the local network leads automatically, and vice versa; `OffGridTravel` (two glyphs in the app bar, opposite the navigation menu) is the tie-break for when both work. Precedence is an order, not an exclusion: whichever route leads, a message the preferred one cannot carry still takes the other | ✅ Wired (engine + runtime, both send paths and the receive path, cross-transport dedup; tested with three real engines over real mDNS). ⚠️ Needs a **shared IP network** — WiFi, a hotspot, ad-hoc; airplane mode with WiFi *off* is not one. **Bluetooth is route 2b** (`dak::ble` + `android/…/ble/BleMeshService.kt`): the same sealed frame, fragmented to the BLE MTU and flooded with TTL/dedup/duty-cycled scanning, so two phones with no router at all can still reach each other. Framing and flooding are Rust and tested; the radio itself is Kotlin and **untested — no CI has a Bluetooth adapter**. The first two-phone test found four separate reasons it could not work — a missing Android multicast lock chief among them — all fixed and written up in [`docs/OFFLINE_DELIVERY.md`](docs/OFFLINE_DELIVERY.md); re-test on hardware still outstanding, and guest-WiFi client isolation defeats it regardless |
| **Sakha/Sakhi** | Yrs CRDT + AES-256-GCM | Cryptographically isolated shared ledger for couples | ⚠️ Engine built and **pairing ships on desktop** (Partner modal → real ECDH+HKDF handshake, encrypted-at-rest pairing record, live ledger sync); no Android surface yet, and the Flutter screen is unbridged |
| **Relay gossip** | NIP-65 | Dynamic relay discovery + outbox-model routing | 🧪 Experimental — routing library + CLI demo only |
| **Media** | NIP-94 / NIP-96 | Encrypted file staging + pluggable decentralized upload | ⚠️ Send + download-and-decrypt over Blossom (`media-http` feature) — desktop **and Android** (attach button); rich in-thread rendering of received media is Android follow-up; not on CLI. **The upload host is a list, not a single point of failure** — the one hard-coded host (`cdn.hackers.town`) stopped completing TCP connections, which broke every attachment on every platform with `error sending request for url`; uploads now try each of `comrade_core::media::DEFAULT_BLOSSOM_SERVERS` in turn, with connect/transfer timeouts so a dead host costs seconds rather than hanging, and a failure names every host it tried. The ciphertext is also uploaded as `application/octet-stream` rather than the file's real type, so the host is not told whether you just sent a photo or a PDF. A reference no relay will accept is **queued in the Dak outbox and retried** instead of orphaning the uploaded blob, and a conversation whose newest item is an attachment says so in the chat list ("📎 …") instead of showing a stale line. The unified Flutter app adds a Telegram-shaped composer (emoji · text · paper clip · swappable mic/camera), a real document picker and camera capture, and plays received audio — and hands documents and video to other apps — **from memory**, so no decrypted attachment is ever written to disk |
| **Calls** | WebRTC + signaling over the Vault DM channel (NIP-100 direction) | 1:1 **voice & video** — SDP/ICE ride encrypted DMs; STUN-only first attempt, automatic fallback to a configurable TURN relay if STUN can't connect (e.g. strict CGNAT); call log | ⚠️ 1:1 voice & video wired on BOTH desktop (webview WebRTC) and Android native (`org.webrtc` via `io.github.webrtc-sdk`): ringing/lock-screen call UI, foreground service, Bluetooth/wired audio routing, mid-call camera toggle + tap-to-swap self-view, call history + missed-call notifications, a four-bar **network-strength indicator** (Telegram-style; it replaced the earlier encryption-emoji SAS row — the SDP already rides the key-authenticated NIP-44 DM channel, so the read-aloud check was near-redundant; `derive_sas` is retained, unsurfaced), **picture-in-picture** (leaving the app during a video call floats it into an OS window; the in-call **chat button** shrinks it into a corner tile *inside* the app, so the call and the conversation are on screen together), **"Video paused"** instead of a frozen frame plus camera capture that stops when nothing is displaying it, FaceTime-style self-hiding controls, and in-notification call controls (end always; mute + audio route once answered). STUN-only first attempt with caller-driven TURN fallback (ICE restart) — wired on Android; desktop does not yet ICE-restart on failure — see [`AUDIT.md` §8.1](AUDIT.md). Calls need the app process alive (no push wakeup, by design) |
| **Storage** | redb + Argon2id + AES-256-GCM | Encrypted-at-rest persistence (identity, ChitthiCache, VaultCache, LedgerState) unlocked by a passphrase | ✅ Wired (identity, own posts, full DM history — incoming + outgoing — with offline backfill watermark, media refs, call log) |
| **Voice** | Vosk (offline) + Android TTS | "Hey Comrade" wake word, tap-to-talk, and assist-app role — all on-device, no cloud; the speech model ships in the APK or is offered as a one-time sha256-verified in-app download on first use | ⚠️ Recognition/dispatch work; `post`/`read timeline` need a vault-unlock screen the Android UI doesn't have yet |
| **Tara** | On-device reply engine (no network, no cloud) | Reflective companion — a private space to think out loud: feeling-mirroring, reflective prompts, brainstorming scaffolds, journaling nudges from mood markers. Reachable by tab **or by voice** ("Hey Comrade, tara …"), where a crisis hand-off reads a helpline number out digit-by-digit since there's no card to show. Replies **stream in** word-by-word (crisis hand-offs never do — those appear complete). **Not therapy, and it says so**: distress cues switch the reply into a hand-off to real crisis helplines. Thread sealed in the encrypted store; *clear conversation* built in | ⚠️ v1 deterministic template engine wired end-to-end (Android **Tara** tab — last in the bottom nav — with opt-in explainer, streaming replies + crisis card; desktop backend commands, web UI pending). The engine seam is ready for an **on-device** LLM once [`AUDIT.md` OQ9](AUDIT.md) is decided — a cloud model is ruled out by design. See [`docs/TARA.md`](docs/TARA.md) |
| **Model downloads** | One pinned-checksum pipeline for every on-device model | Shared fetch/verify/install for the **speech** model ("Hey Comrade") and Tara's future companion model: sha256-pinned, zip-slip-guarded, atomic install. Runs in a **foreground service** so the transfer survives backgrounding, with live progress in the **notification bar** and a tappable "ready" notification that deep-links back to where the model is used | ✅ Wired for the speech model (Android). ⚠️ The companion-model entry ships deliberately **unpinned** until OQ9 picks a model, so no unverifiable download is ever offered |
| **Dak** (store & forward) | Sender outbox + sealed courier envelopes | A DM no relay will accept is **queued, not lost**: persisted in the encrypted store, retried on a cadence and at every launch, cleared by the peer's receipt, and marked *failed* — visibly — after 8 attempts or 24 h. Alongside it, an engine for handing sealed mail to a peer who may physically meet the recipient: day-rotating recipient tags (`HMAC(recipient key, ctx‖UTC day)`), trust-tier quotas, spray-and-wait copy budgets | ✅ Outbox wired (queue → retry → receipt/fail, survives an app kill), and its sealed frames now **have a transport**: the Saathi row's local-network delivery. 🧪 Person-to-person couriering (carry a stranger's mail to someone you may meet) is engine + tests only |
| **Anonymous Chitthi** | Per-post ephemeral keys / device-seed personas | A public post signed by a key that is **not your identity**: a throwaway key per post (two anonymous posts cannot be linked to each other) or a stable per-scope persona so replies reach the same pseudonym. `created_at` coarsened to the hour | ✅ Engine + `ComradeRuntime`/Android bridge command; UI surface pending. Honest about limits: the key is unlinkable, the network path (IP, timing) is not |
| **Location channels** | Geohash-scoped Nostr rooms (kinds 20000/20001) | Public rooms scoped to a place instead of a follow graph, spoken under a per-cell persona. Presence heartbeats **only** at region/province/city precision — never street level — and an empty fine-precision channel reports "?" rather than a misleading 0 | 🧪 Engine + tests only (publish/subscribe/presence + geohash arithmetic); no UI yet |
| **Screenshots** | Android `FLAG_SECURE`, off by default | Screenshots and screen recording **work everywhere in the app**, and blocking them is a setting you turn on rather than a default you cannot turn off. Both frontends read one stored preference; a screen that genuinely shows a secret (today: the passphrase field *while revealed*) takes a scoped, reference-counted hold regardless | ✅ Wired (Compose settings switch; Flutter settings switch + `SecureScreen`). Honest about its limits: it stops the OS capturing the screen, not a camera pointed at it, and it says so in the UI |
| **App updates** | GitHub Releases + a version rule shared by both frontends | Comrade is installed by sideload, so nothing otherwise tells anyone that a release — including a security fix — shipped. A once-a-day check of the project's own `releases/latest` raises a low-priority notification ("Comrade 0.0.9 is available"), and a Settings card shows the running version, the release notes, and the download. **The APK is fetched in-app by a foreground service**: leaving the screen (or the app) does not stop it, the shade carries live progress, and it finishes with a tappable *ready to install* — no browser, no hunt through Downloads. Before anything is handed to the installer the file is checked against the running app — package name, version code, and signing certificate — and refused with a reason if any of the three disagree; Android then applies its own same-signature rule, which is unconditional enforcement rather than a dialog. **On Android 12 and newer the install usually has no confirmation dialog at all**: the update applies, Comrade's process is replaced, and a notification offers to reopen it — whether the dialog is skipped is the platform's call, and the app handles both without assuming which. Installing needs the one-time "install unknown apps" grant, which the card explains and links to; the release page stays as a fallback for a release with no single APK and for anyone who would rather not grant it. "Skip this version" silences that version, not the next one. The daily check is a real disclosure (your IP address, to GitHub) and the card says so in as many words; it is a switch, and turning it off leaves a manual "Check now" | ✅ Wired (Compose + Flutter; the rule, the fetch and the install are one shared native implementation, unit-tested on the host JVM). Not on desktop: no desktop artifact is published to update *to* |
| **Notifications** | Per-class channels + device-local mute | Messages, requests, calls, comrades-online and updates each get their own channel, so one can be silenced without the rest. Tapping a message notification **opens that conversation** (not "wherever you left off"), a request opens the requests list, an update notice opens its Settings card; several unread chats collapse under one summary. Any conversation can be **muted** from its ⋮ menu — which silences its messages, attachments *and* its comrade-online notice (and the *"might need you"* nudge that shares its line in the shade), but never a ringing call | ✅ Wired (Compose + Flutter; mute is device-local by construction — there is no server to sync it through — and the Settings card says so) |
| **Panic wipe** | Whole-store shred + re-lock | One call destroys every stored value — identity keys, DM history, journal, Tara thread, queued mail — then re-locks to a pre-onboarding state. Enumerates the database's *actual* tables, so a store added later cannot be forgotten | ✅ Wired (`ComradeRuntime::panic_wipe`, Android bridge command); a UI entry point is the follow-up. Wipes rather than hides, and needs the app unlocked — deliberately **not** a duress feature |
| **Attention** | Device-local usage rollups + a pure focus-session engine | **Winning back an attention span eroded by doomscrolling and short video.** Four parts, none of which phones home: (1) the **calm-feed contract** — the public feed is now *contractually* chronological with no ranking, autoplay or engagement counters, plus a **gentle stop** offering somewhere better to be after ten unbroken minutes (it quotes no duration and no count, because a number invites a score); (2) **quiet hours** — a nightly window silencing messages, requests, presence and update notices, but **never a ringing call**; (3) the **mirror** — opt-in `PACKAGE_USAGE_STATS` reduced *in memory* to three integers a day (screen minutes, pickups, minutes in apps **the user themself** marked — Comrade ships no blacklist of other people's apps), sealed in the same encrypted store as the journal and shown only against the user's *own* 7-day median, never a target and never another user; (4) the **practice** — focus sessions with a named intention and a progressive 25→45→90 ladder unlocked by completions, a chunked distraction-free reader for text the user brings (nothing here fetches a URL), and a *"Take a deep breath"* screen whose haptics swell on the in-breath and fade on the out (Pixel-Watch-shaped, so the pace can be followed with the eyes closed) and which tells your comrades, once, that you might need them. Tara gains a heavy-scroll-day journaling nudge that reads the rollup **numbers only** — and mood still outranks it | ✅ Wired (Android: Focus tab + mirror card on Journal + gentle stop + quiet hours + `"start a focus session"` voice command; engine/storage/view-model/both bridges tested, incl. ciphertext-at-rest and panic-wipe coverage for every new tree). Honest about its limits, in the UI itself: this is **attention practice, not treatment** — no scores, no levels, no streak to break, and an abandoned session is recorded plainly because stopping early is information, not failure. A session nobody was present for is recorded as *lapsed* rather than allowed to claim a completion. Desktop: all 17 commands registered, web UI pending like the journal's. See [`docs/ATTENTION.md`](docs/ATTENTION.md) |
| **Metrics** | Device-local counters | Delivery-behaviour tallies (queued/resent/delivered/dropped, courier deposits and handovers, dedup drops) with **no** peer, message id, content, or timestamp — so they can never become a record of who talked to whom. No exporter, no endpoint; cleared by the panic wipe | ✅ Wired at the outbox/courier call sites + `metrics_snapshot()`; diagnostics screen pending |

> **Status honesty.** 🧪 rows are working, unit-tested library code that no
> frontend invokes yet — they describe the architecture's direction, not
> shipped behavior. The full gap analysis lives in [`AUDIT.md`](AUDIT.md).

> **Nomenclature.** A public post is a **Chitthi** (Hindi for *letter*) throughout
> the application layer — `ChitthiNode`/`ChitthiThread`, `broadcast_chitthi`,
> `subscribe_chitthi_feed`, the `chitthi_cache`. **Dak** (डाक — *post, mail*) is
> the store-and-forward layer, and a courier carrying someone's mail is its
> *dakiya*. Nostr protocol constants (`Kind::TextNote`, NIP-44) are kept intact
> at the wire level.

> **Prior art.** The store-and-forward stack, the privacy mechanics (padding,
> panic wipe, identity-free metrics), the anonymity primitives, and the location
> channels are adapted from [bitchat](https://github.com/permissionlesstech/bitchat).
> [`docs/BITCHAT_ADOPTION.md`](docs/BITCHAT_ADOPTION.md) is the full ledger:
> every mechanism examined, what landed, what was adapted because Comrade's
> stack differs, and what was deliberately declined.

## Identity & usernames

Comrade is serverless, so it deliberately does **not** have Telegram-style
globally unique usernames — no central registry exists that could enforce
them, and a first-come claim on public relays would be squattable and
unreliable. The model instead:

- **Identity = the keypair.** Every account is a secp256k1 keypair created
  on-device; the public half (`npub…`) is the address peers actually message.
  It is better than a UUID for device-to-device interaction: globally unique
  *and* unforgeable — using it requires the private key.
- **Username = a display alias.** At first launch the app asks for a
  `@handle`, stores it with the identity, and publishes it as Kind-0 metadata
  so people can find you by name (NIP-50 relay search, best-effort).
  Publication retries with backoff and is **re-published on every launch** —
  a claim made offline (or during the onboarding connect race) heals itself
  the next time a relay is reachable.
- **Discovery is honest about its limits.** Search queries go to dedicated
  NIP-50 relays (`relay.nostr.band`, `search.nos.today`) and results are
  filtered against the query client-side; new profiles can take a little
  while to be indexed by the search relays. Pasting an `npub` key looks that
  identity up directly. The reliable path is always swapping keys.
- **Contacts pin the key, not the name (trust-on-first-use).** Once you add
  `@abc_user`, the contact is stored under their npub. If `@abc_user`
  disconnects and a stranger later claims the same handle, that stranger is a
  *different npub*: they show up as a separate, unverified entry, and they can
  never read or receive the encrypted messages bound to the original key.
- **Chats are titled by name, in trust order.** A conversation shows (1) the
  alias *you* set for that contact, else (2) the `@handle` *they* published
  (fetched and cached locally, refreshed in the background), else (3) the
  shortened key. Handles are claims and keys are identity, so the key is
  always one tap away in the conversation's ⋮ menu — *Copy key*, and
  *Encryption info* shows it in full — rather than crowding the header, which
  carries the name and (for a comrade) their presence. Aliases are set from
  the same ⋮ menu; an empty alias falls back to the handle.
- The opt-in path to *verified* unique names (NIP-05 DNS mapping) is future
  work.

## Comrades — knowing when your person is around

Mark a contact as your **comrade** (the ★ in any conversation, or the Comrades
screen in the navigation drawer) and Comrade tells you when they come online.

Presence is the one feature here that tells someone else something about you,
so the rules are deliberately narrow and stated in the UI itself:

- **Only the people you chose are told, and no relay ever is.** A presence
  beacon is a tiny envelope inside a gift-wrapped E2E DM — the same channel
  read receipts and call signals already ride. Nostr's public status
  convention (NIP-38) was rejected on purpose: a replaceable public event
  would publish a minute-by-minute log of when you are holding your phone, to
  everyone, forever. See [`docs/PRESENCE.md`](docs/PRESENCE.md).
- **It is mutual by construction.** You announce to whom you marked; you see
  whoever marked you. Choosing someone cannot subscribe you to their presence
  — with no server, nothing can make their device report to you — so until
  they choose you back their row honestly reads *"waiting for them to choose
  you back"* rather than showing an unexplained grey dot. Choose someone who
  already chose you and their presence appears immediately.
- **A green dot expires.** A beacon says "online for the next few minutes",
  measured from when it was *sent*, so a phone that dies (or is force-killed,
  or loses signal) fades out on its own, and a beacon replayed out of the
  relay backfill can never resurrect a stale dot. Locking your vault sends a
  goodbye immediately.
- **Nothing else leaks.** A beacon carries "online / offline" and how long to
  believe it. No activity, no location, no last-seen timeline for
  non-comrades, and **no typing indicators** — the only thing a comrade ever
  learns beyond "around / not around" is the nudge described next, which is one
  signal after the fact rather than a live feed.

### When someone nearly wrote to you

Someone opens your chat, types a few words, and then clears the box or walks
away without sending. Nothing about that used to reach you — the message was
never sent, so there was nothing to deliver — and it is often the moment that
mattered most. If they marked you as a comrade, you now get one notification:
**"Bhaskar is online" / "Your comrade might need you"**, tapping through to
their conversation.

It is deliberately not a typing indicator, and the difference is the whole
design:

- **Once, after the fact** — not continuously while someone types. The signal
  fires only when the draft is *gone*, at most once every half hour per person,
  and never for a draft that lasted less than a few seconds (a stray keystroke
  is not a hesitation).
- **The same one bit, whatever prompted it.** Reaching for the *"Take a deep
  breath"* screen sends the identical envelope, sharing the identical cooldown,
  so a comrade learns you might need them and **cannot tell which happened**.
  That is what let a second reason be added without disclosing anything new —
  and it is why there is no `reason` field to add one to.
- **Nothing about what was written.** The envelope carries a marker and a
  deadline. Not the text, not its length, not how long they typed, not whether
  they cleared it or walked away — and a test asserts the wire shape rather than
  trusting the comment.
- **Clearing the box to rewrite the same sentence discloses nothing.** Nothing
  is sent until the draft has stayed gone for a short while, so the ordinary
  edit — clear, retype, send — never leaves the device. Sending anything at all
  cancels it, because the message says what the nudge would have.
- **Same consent, same channel as presence.** Only a comrade you chose is ever
  told, only from an accepted conversation, gift-wrapped so no relay learns
  anything, and it expires from its own send time — so a nudge replayed out of
  the relay backfill, or held by a phone that slept for an hour, raises nothing.
  Muting a conversation silences it, like every other notice about a person.

See [`docs/PRESENCE.md`](docs/PRESENCE.md) §7 for why this line was drawn where
it is, having previously been drawn to exclude the feature entirely.

Honest limit: presence needs the app process alive, exactly like calls and
message delivery (there is no push wakeup — see the notes on
`RelayConnectionService` below). A killed process is offline, and its
comrades' dots go grey when the claim lapses.

The **Progressive-Disclosure state machine** (`comrade_state`) gates which engines are active:

```
Base ──────── Sabha (public feed) + Vault (E2E DMs), relays tried first
  └─ OffGridTravel ─── the local mesh is tried first instead
  └─ CoupleSandbox ─── Sakha or Sakhi view of the shared ledger
```

## Repository layout

```
crates/
  comrade_state/   State machine (no I/O dependencies)
  comrade_core/    Protocol engines: crypto, sabha, vault, saathi, sakha, relay, media
                   plus dak (outbox + couriers), gcs (history reconciliation),
                   anon (unlinkable personas), geo (location channels),
                   pad / seen / metrics (padding, dedup, local counters)
  comrade_storage/ Encrypted-at-rest persistence (redb + Argon2id + AES-256-GCM)
  comrade_ui/      Framework-agnostic view-model / service layer (UiService + DTOs)
  comrade_jni/     JNI bridge — compiled to libcomrade_jni.so for Android
  comrade_py/      PyO3 bridge — Python bindings (pip/maturin) for local scripting
src/main.rs        Interactive CLI harness (development / testing)
android/           Kotlin + Jetpack Compose app — Telegram-like shell:
                   onboarding (@username + passcode → encrypted vault),
                   Chats (E2E DMs), Feed (public Chitthis), Settings
desktop/           Tauri 2 desktop shell (excluded from the workspace — see desktop/README.md)
.github/workflows/ CI (test + lint) and manual APK release
```

The bridge logic lives once in **`comrade_ui`** and backs the Android app (via
`comrade_jni`), the desktop app (via `#[tauri::command]` wrappers in `desktop/`),
and external Python scripts (via `comrade_py`, covering the Sabha/Vault subset —
see [`crates/comrade_py/README.md`](crates/comrade_py/README.md)).
This keeps the bridged UI contract unit-testable without a display server.
The CLI harness currently drives the core crates directly rather than going
through `comrade_ui`; unifying it is tracked in `AUDIT.md`.

## Building

### Prerequisites

- Rust stable — the committed `Cargo.lock` currently requires **rustc ≥ 1.88** — [rustup.rs](https://rustup.rs)
- Android NDK r27c — via Android Studio or `sdkmanager "ndk;27.2.12479018"`
- `cargo-ndk` — `cargo install cargo-ndk --locked`
- JDK 17 (for the Android build; Gradle comes from the committed wrapper)

### Run the CLI harness

```sh
cargo run
```

### Run all tests

```sh
cargo test --workspace
```

### Startup performance

App startup is dominated by loading `libcomrade_jni.so` (the entire statically
linked Rust core). Three things keep it fast — please don't regress them:

- **Release profile** — the root `Cargo.toml` sets thin LTO, one codegen unit,
  and debuginfo stripping for `[profile.release]`, which cuts the shipped `.so`
  by ~20%. `panic = "abort"` must stay **off**: uniffi's generated scaffolding
  catches panics at the FFI boundary, which needs unwinding.
- **Off-main-thread load** — `ComradeApplication` warms the library on a
  background thread at process start, and `ComradeApp` resolves core facts on
  `Dispatchers.IO` inside a `LaunchedEffect`, so the first Compose frame never
  blocks on the native call. `MainActivityUiTest` guards this on the CI device
  lanes.
- **mmap-from-APK packaging** — `useLegacyPackaging = false` stores the `.so`
  uncompressed so the linker maps it straight from the APK.

Observability: logcat shows `ComradeApplication: comrade_jni v… warmed in N ms`
and the framework's `Fully drawn` line (via `reportFullyDrawn()`); the Rust
side traces `vault unlocked: store opened and engines built` with `kdf_ms` /
`total_ms` fields on every unlock.

### Build the Android APK locally

```sh
# 1. Cross-compile the Rust JNI library for your target ABI
cargo ndk \
  --target aarch64-linux-android \
  --output-dir android/app/src/main/jniLibs \
  -- build --release -p comrade_jni

# 2. Build the APK (the committed Gradle wrapper fetches Gradle 8.5)
cd android
./gradlew assembleRelease
# APK → android/app/build/outputs/apk/release/app-release.apk
```

Step 2 also builds `comrade_jni` for your *host* (a separate, fast, non-NDK
build — not the cross-compiled `.so` from step 1) and runs it through
`comrade_uniffi_bindgen` to generate the Kotlin bindings, so `cargo` needs to
be on `PATH` even though step 1 already ran. This happens automatically as a
Gradle task dependency (`android/app/build.gradle.kts`); there's no separate
bindgen command to run by hand, and nothing to keep in sync as
`crates/comrade_jni`'s interface grows.

### Build the desktop app (Tauri 2)

The desktop shell lives in `desktop/` and is **excluded from the Cargo workspace**
because it needs the Tauri CLI and system webview libraries. See
[`desktop/README.md`](desktop/README.md) for full prerequisites. In short:

```sh
cargo install tauri-cli --version "^2.0.0"
cd desktop/src-tauri
cargo tauri dev      # run; or `cargo tauri build` for a distributable bundle
```

### Build the Python bindings

`crates/comrade_py` exposes the Sabha (public feed) and Vault (E2E DM) engines
to Python — fetch/publish only, see [its README](crates/comrade_py/README.md)
for the full scope and a usage example.

```sh
cd crates/comrade_py
python -m venv .venv && source .venv/bin/activate
pip install maturin
maturin develop            # build + install into the active venv
# or: maturin build --release   # wheel → target/wheels/
```

## CI / Releases

| Workflow | Trigger | What it does |
|----------|---------|--------------|
| **CI** | Every push | Rust lane (`cargo fmt --check`, `clippy -D warnings`, `cargo test --workspace --locked`) · Desktop lane (`clippy` on `desktop/src-tauri`) · Android lane (`./gradlew test`) · Python lane (builds the `comrade_py` wheel with maturin and smoke-tests importing it) · Supply-chain lane (`cargo-deny`: advisories, bans, sources, licenses) |
| **Android APK** | Push touching `android/`, `crates/`, `Cargo.*` · manual | Cross-compiles `libcomrade_jni.so` (arm64-v8a for handsets, x86_64 for emulators), assembles a sideloadable debug APK artifact, and runs the on-device smoke suite (`connectedDebugAndroidTest`) on two emulator lanes — Pixel 9 and Pixel 9 Pro XL (Android 15 / API 35) |
| **Release APK** | Manual — Actions → "Release APK" → Run workflow (also reusable via `workflow_call`) | Builds `.so` libs for arm64-v8a / armeabi-v7a / x86_64, assembles APK + AAB, creates GitHub Release |
| **Auto Release** | Every push to `main` (each merged PR) | Bumps the patch version `X.Y.Z → X.Y.(Z+1)` from the newest `v*` tag (seeds at `0.0.2`), then calls **Release APK** to build + publish. Put `[skip release]` in the merge commit to skip |

### On-device APK testing

The **Android APK** workflow installs the built APK on KVM-accelerated Android
emulators and runs `android/app/src/androidTest/…/DeviceSmokeTest.kt`: the JNI
library loads for the device ABI, keypair generation round-trips through real
Rust crypto, workspaces come back labelled, and `MainActivity` reaches
`RESUMED`. Two lanes model the target hardware:

| Lane | AVD profile | Android |
|------|-------------|---------|
| Google Pixel 9 | `pixel_9` (falls back to `pixel_7` on older SDK tools) | 15 (API 35) |
| Google Pixel 9 Pro XL | `pixel_9_pro_xl` (falls back to `pixel_7_pro`) | 15 (API 35) |

Honest limits: GitHub-hosted runners have no physical phones — real-hardware
runs need a device farm such as Firebase Test Lab (paid, service-account
secrets). For manual testing, download the `comrade-debug-apk` artifact from
any run and sideload it (real handsets are arm64-v8a, which the APK
includes). **iOS is out of scope**: an APK is an Android package and cannot
run on an iPhone, and Comrade currently has no iOS frontend — the non-Android
frontends are the Tauri desktop shell and the CLI.

Debug artifacts are signed with the keystore committed at
`android/debug.keystore` (the standard, deliberately-public Android debug
credentials), so every CI run and every local `assembleDebug` share one
signature — a newer sideloaded artifact installs directly over an older one
and keeps app data instead of tripping Android's "App not installed"
signature-conflict error. **One-time caveat**: any APK you installed *before*
this change carries an old, throwaway per-runner signature and must be
uninstalled once to make room for a debug-keystore-signed build; today that
uninstall discards its local app data, since there is no identity
export/import yet. This committed debug key is public by design and cannot
be used to sign anything for Play — see [`RELEASING.md`](RELEASING.md) for
production signing.

### Creating a release from the GitHub UI

1. Go to **Actions** → **Release APK**
2. Click **Run workflow**
3. Enter a version string (e.g. `1.0.0`) and optionally mark as pre-release
4. The workflow runs tests → cross-compiles Rust → assembles APK → creates a Release with the APK attached

> **Shipping to real users?** See [`RELEASING.md`](RELEASING.md) for the
> keystore setup, the Play-ready App Bundle, distribution options
> (Play / F-Droid / Obtainium), and an honest account of what Play Protect
> will still say about sideloaded builds.

### APK signing (optional)

Add these repository secrets (**Settings → Secrets → Actions**) for a production-signed build:

| Secret | Description |
|--------|-------------|
| `SIGNING_STORE_B64` | Base64-encoded `.jks` keystore |
| `SIGNING_KEY_ALIAS` | Key alias inside the keystore |
| `SIGNING_KEY_PASSWORD` | Key password |
| `SIGNING_STORE_PASSWORD` | Keystore password |

Without signing secrets the APK is signed with the Android debug key and can be sideloaded for testing.

## Voice — "Hey Comrade" (Android)

The Android app can respond to a spoken **"Hey Comrade"** wake word, take a
tap-to-talk command, and register as the device's assist app. Everything runs
**on-device** with the offline [Vosk](https://alphacephei.com/vosk/) recogniser
and Android's built-in text-to-speech — no audio ever leaves the phone, keeping
with Comrade's privacy-first design.

| Layer | What it is | Entry point |
|-------|-----------|-------------|
| **Wake word** | A foreground `Service` keeps the mic open, listens for "Hey Comrade", then routes the following utterance as a command. Shows a persistent notification; costs battery. | `voice/WakeWordService` |
| **Tap-to-talk** | One-shot capture from a mic button in the app UI, no always-on service. | `voice/OneShotRecognizer` + `MainActivity` |
| **Assist app** | Register Comrade as the default digital assistant so the assist gesture (long-press power) opens it. | `voice/ComradeInteractionService` |

Recognised commands (see `voice/VoiceCommand`): **post** _&lt;message&gt;_ ·
**read my timeline** · **switch to** _base / off-grid / sakha / sakhi_ ·
**new identity** · **help**. Parsing and command dispatch are Android-free and
unit-tested (`VoiceCommandTest`, `CommandDispatcherTest`).

> **Note.** `post` and `read my timeline` require an unlocked vault. The app's
> onboarding flow (username + passcode) unlocks it at startup, so these work
> once you're past the door; before unlocking they answer with a "vault is
> locked" error.

> **Honest scope.** This is an *app-scoped* wake word, not the OS-level
> "Hey Google" hotword. Stock (non-rooted) Pixels reserve the always-on,
> screen-off DSP hotword pipeline for Google's own keyphrases — a third-party
> app cannot inject a custom phrase there. So "Hey Comrade" works only while the
> foreground service is running (persistent notification, mic open), and the
> assist-app role responds to the assist *gesture*, not a spoken phrase.

### Voice setup

The Vosk model (~40 MB) is **not** committed, and voice works either way:

- **Model absent from the APK** (the default; CI doesn't bake it): the first
  tap on a voice feature — tap-to-talk, *Enable "Hey Comrade"*, journal
  dictation — offers a one-time in-app download (Google-style on-device
  speech-model prompt), with progress, cancel, and retry. The download is
  verified against a pinned sha256 before install; a mismatch is refused
  and deleted.
- **Model baked into the APK** (no first-use download): stage it before
  building —

  ```sh
  ./scripts/fetch-vosk-model.sh   # → android/app/src/main/assets/model-en-us/
  ```

  The script verifies the same pinned sha256 and writes the `uuid` marker
  `StorageService` needs at runtime.

On first use the app requests the `RECORD_AUDIO` (and, on Android 13+,
`POST_NOTIFICATIONS`) runtime permissions.

## Architecture notes

- **Zero `.unwrap()`/`.expect()` in network or parsing paths** — all fallible I/O returns `Result<T, E>` with `thiserror`-derived domain errors.
- **Thread safety** — shared state uses `Arc<RwLock<T>>` or `Arc<Mutex<T>>` across async tasks.
- **DH key agreement** — secp256k1 ECDH with x-coordinate-only SHA-256 hashing for parity-independence; HKDF-SHA256 for label-scoped key derivation.
- **Off-line resilience** — Saathi caches up to 256 outbound messages and drains them automatically on mDNS peer discovery. (Engine-level behavior; Saathi is not yet started by any frontend.)
- **CRDT convergence** — Sakha/Sakhi use Yrs (Yjs port) so concurrent edits on either device merge deterministically; relay sees only AES-256-GCM ciphertext.
- **Encrypted-at-rest persistence** — every stored value is sealed with AES-256-GCM under an Argon2id-derived key (zeroized in memory, never written to disk); a wrong passphrase fails closed with `StorageError::InvalidPin`. On startup the CLI detects an existing store and prompts for the passphrase to restore the profile rather than minting a throwaway keypair. Durability is verified by `comrade_storage`'s `tests/durability.rs` reboot suite.
  
