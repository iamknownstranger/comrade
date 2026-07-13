# Voice/Video Calling — Telegram-Parity Roadmap (for a junior coding agent)

**Audience:** an AI coding agent (or new contributor) picking up the calling feature.
**Status:** the core call flow is **merged and working** (place/receive, offer/answer/ICE,
STUN→TURN fallback, mute, speaker, switch-camera, proximity, timeouts, honest end-states).
This doc is the backlog to reach Telegram-like parity, broken into small, shippable tasks.

> Read **§0–§2 in full before writing any code.** They contain the architecture, the
> hard constraints, and the mistakes already made on this feature. Skipping them will
> cost you a red-CI round-trip (each is ~15–20 min).

---

## §0 · Orientation — how calling works today

### Where the code lives
| Concern | File |
|---|---|
| WebRTC + call state machine + signaling glue | `android/app/src/main/java/mullu/comrade/call/CallManager.kt` |
| Compose call UI (Ringing/Connecting/Active/Ended) | `android/app/src/main/java/mullu/comrade/call/CallScreen.kt` |
| UI state model | `android/app/src/main/java/mullu/comrade/call/CallUiState.kt` |
| Event pump + call entry points + overlay | `android/app/src/main/java/mullu/comrade/MainActivity.kt` |
| Notifications (incoming call, etc.) | `android/app/src/main/java/mullu/comrade/Notifier.kt` |
| Icons (Call/CallEnd/Videocam/Speaker/Mic) | `android/app/src/main/java/mullu/comrade/ui/AppIcons.kt` |
| Kotlin facade over the Rust core | `android/app/src/main/java/mullu/comrade/ComradeCore.kt` |
| WebRTC dependency | `android/app/build.gradle.kts` |
| Permissions / services | `android/app/src/main/AndroidManifest.xml` |
| **Wire protocol (Rust, pure, unit-tested)** | `crates/comrade_core/src/call.rs` |
| **Runtime: place/send/hangup/ice/log/history** | `crates/comrade_ui/src/runtime.rs` |

### The signaling contract (do NOT reinvent this)
- A call is **peer-to-peer WebRTC**; there is **no media server**.
- **Signaling rides the encrypted Vault DM channel.** You send a `CallSignal`; the Rust
  core wraps it in a `CallEnvelope` and delivers it as a NIP-59 DM. **You never build the
  envelope in Kotlin** — you hand `ComradeCore.sendCallSignalTyped(peer, callId, media, signal)`
  a typed `CallSignal` and the core does the rest.
- `CallSignal` (generated Kotlin enum, package `uniffi.comrade_core`) has exactly:
  `Offer(sdp)`, `Answer(sdp)`, `Ice(candidate, sdpMid, sdpMLineIndex)`, `Ringing`, `Busy`,
  `Hangup(reason)`. `sdpMLineIndex` is a **`UShort?`** (see the gotcha in §2).
- Inbound signals arrive as `BridgeEvent.IncomingCallSignal(CallSignalDto)` and are drained
  by the pump in `MainActivity.drainEvents()` → `PumpEvent.CallSignal` → `CallManager.onIncomingSignal(dto)`.
- **Stranger-gating:** the Rust core only emits `IncomingCallSignal` if the conversation is
  already **accepted** (`IncomingGate::Accepted`). An un-accepted contact **cannot ring you**.
  This is by design; do not try to bypass it. (It is also the #1 reason a test call "does
  nothing": call between two devices whose conversation is mutually accepted.)

### The call state machine (mirrors the desktop webview in `desktop/ui/main.js`)
```
caller:  Idle → Ringing(out, "Calling…"/"Ringing…") → Connecting → Active → Ended → Idle
callee:  Idle → Ringing(in) → [Accept] → Connecting → Active → Ended → Idle
```
- There is **no explicit "accept" signal** — the callee's `Answer` *is* the accept. `Ringing`
  is informational (drives the "Ringing…" label on the caller).
- Exactly **one call at a time**. A second inbound `Offer` for a different call id is
  auto-rejected with `Busy`. An `Offer` for the *current* call id is treated as a
  **renegotiation** (used by the TURN ICE-restart) — see `CallManager.handleRemoteOffer`.
- ICE candidates are **buffered until the remote description is set** (`Session.pendingIce`,
  `flushPendingIce`). WebRTC throws if you `addIceCandidate` before `setRemoteDescription`.

### Threading model (respect it)
- `CallManager` is a **Kotlin `object` (singleton)**. Its call-state transitions are
  `@Synchronized` on the object monitor (reentrant). Hold that discipline: any method that
  reads/writes `session` or flips `_state` must run under the monitor.
- Blocking FFI calls (`sendCallSignalTyped`, `hangupCallTyped`, `placeCallTyped`,
  `logCallTyped`) run on the private `io` `CoroutineScope(Dispatchers.IO)`.
- WebRTC invokes `PeerConnection.Observer` / `SdpObserver` callbacks on **its own internal
  threads**. `StateFlow.value = …` is safe from any thread; anything touching `session` must
  re-enter `synchronized(this@CallManager)`.
- The UI observes `CallManager.state` / `muted` / `speakerphone` / `localVideo` / `remoteVideo`
  as `StateFlow`s via `collectAsState()`.

---

## §1 · Non-negotiable constraints & how to work

1. **You (probably) cannot compile the Android app locally.** The dev sandbox has no Android
   SDK and the proxy blocks `dl.google.com`. **CI is the compile gate.** Consequence: write
   carefully, review your own diff twice, and expect the first CI run to catch typos. Do
   **not** merge until CI is green.

2. **`CallManager` is an `object`, not a class.** Therefore:
   - **No `inner class`** inside it (`Modifier 'inner' is not applicable inside 'standalone object'`).
     Use an **anonymous-object factory function** instead (see `peerObserver(s)` and the
     `SdpObserver` helpers — copy that pattern). `this@CallManager` is valid inside those
     anonymous objects and refers to the singleton.
   - Nested helper classes that need the object's members won't compile the way an inner class would.

3. **WebRTC dependency is `io.github.webrtc-sdk:android`** (NOT `org.webrtc:google-webrtc`,
   which is dead — JCenter-only, removed 2021). It uses the **same `org.webrtc.*` package**,
   so imports are unchanged. Pinned in `android/app/build.gradle.kts`.

4. **Know the exact FFI types before you touch them.** Generate the Kotlin bindings and read
   them (this is the source of truth, not your memory):
   ```bash
   cargo build -p comrade_jni
   cargo run -p comrade_uniffi_bindgen -- generate \
     --library target/debug/libcomrade_jni.so --language kotlin --out-dir /tmp/uniffi
   # then read /tmp/uniffi/uniffi/comrade_core/comrade_core.kt (CallSignal, enums)
   #          /tmp/uniffi/uniffi/comrade_ui/comrade_ui.kt      (CallSignalDto, IceServerDto, …)
   ```

5. **Validate what you can, locally:** the Rust workspace **does** build here. Before pushing,
   if you touched Rust run all three (they gate CI):
   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets --locked -- -D warnings
   cargo test --workspace --locked
   ```
   Kotlin-only changes: rely on CI, but re-read your diff for the pitfalls in §2.

6. **Never initialize WebRTC at app startup.** `PeerConnectionFactory.initialize(...)` /
   `EglBase.create()` happen lazily in `CallManager.ensureFactory()`, only when a call starts.
   The on-device smoke tests (`DeviceSmokeTest`, `MainActivityUiTest`) walk onboarding →
   Chats → Feed → Settings and must **not** touch WebRTC. If you break startup, those go red.

7. **Git / PR workflow:**
   - Work on branch **`claude/voice-video-calls-frontend-pq1psw`**.
   - **A merged PR is finished.** For each new task, restart the branch from the latest default
     branch: `git fetch origin main && git checkout -B claude/voice-video-calls-frontend-pq1psw origin/main`,
     then build the task. Push with `git push --force-with-lease origin <branch>`.
   - One PR per task (or per closely-related pair). Keep PRs small — untestable Kotlin + a
     15-min CI cycle means small diffs converge faster.
   - Base every PR on `main`. Do not merge on red. Confirm all checks (`Rust`, `Desktop`,
     `Android — JVM unit tests`, `Android APK` assemble + both emulator device tests,
     `Python`, `cargo-deny`) are green.

8. **CI check names** (watch these on the PR): `Rust — fmt, clippy, test`, `Desktop — Tauri
   shell clippy`, `Android — JVM unit tests`, `Assemble debug APK`, `Device test — pixel-9`,
   `Device test — pixel-9-pro-xl`, `Build JNI lib (…)`, `Python bindings …`, `Dependencies — cargo-deny`.

9. **Manual verification needs two devices.** A real connect requires two devices whose
   conversation is mutually **accepted**, both online on the same relays. Single-device tests
   will only ever reach "No answer" (which is now the correct, non-hanging behavior).

---

## §2 · Pitfalls already hit on this feature (avoid these)

- **`inner class` in the `object`** → compile error. Use anonymous-object factory funcs (§1.2).
- **`org.webrtc:google-webrtc` does not resolve** → build break. Use `io.github.webrtc-sdk` (§1.3).
- **`CallSignal.Ice.sdpMLineIndex` is `UShort?`.** When building `org.webrtc.IceCandidate`
  use `ice.sdpMLineIndex?.toInt() ?: 0`; when sending, `candidate.sdpMLineIndex.toUShort()`.
- **Integer-literal typing:** `val x: Long = if (c) a/1000 else 0L` — use `0L`, not `0`, or
  the `if` infers a wider type and the `Long` argument won't type-check.
- **`main` may add overlapping call code.** A parallel PR once added a notification-only
  `PumpEvent.IncomingCall`; it had to be reconciled with `PumpEvent.CallSignal`. When you
  rebase, re-check `MainActivity`'s `PumpEvent` sealed interface and the pump `when` for
  duplicate/dangling call branches.
- **`AudioManager.isSpeakerphoneOn` is deprecated** (API 31+ wants `setCommunicationDevice`).
  Annotate `@Suppress("DEPRECATION")` at the function level (annotating a bare assignment is
  unreliable), or migrate to the new API (Task B1).
- **Compose `AndroidView` factory** takes `(Context) -> View`; a `{ renderer }` no-arg lambda
  is fine (implicit `it`). Always `release()` `SurfaceViewRenderer`s in `onDispose` and
  `addSink`/`removeSink` the `VideoTrack` in a `DisposableEffect(track)`.
- **`PeerConnection.Observer` requires the full method set** (`onSignalingChange`,
  `onIceConnectionChange`, `onIceConnectionReceivingChange`, `onIceGatheringChange`,
  `onIceCandidate`, `onIceCandidatesRemoved`, `onAddStream`, `onRemoveStream`, `onDataChannel`,
  `onRenegotiationNeeded`, `onAddTrack`). Java array params override as `Array<out T>`.

---

## §3 · Phase B — in-call parity

### Task B1 — Bluetooth / wired-headset audio routing
**Goal:** route in-call audio across earpiece / speaker / Bluetooth SCO / wired headset, with a
device picker and sensible auto-selection; auto-switch on plug/unplug.

**Why:** today `CallManager` only toggles `isSpeakerphoneOn` (earpiece ↔ speaker). Telegram
routes to a connected headset automatically and lets you pick.

**Files:** `CallManager.kt` (audio-routing section: `beginAudioRouting`, `endAudioRouting`,
`setSpeakerphone`, `toggleSpeaker`); `CallScreen.kt` (add an audio-device button/menu);
`AndroidManifest.xml` (add `BLUETOOTH_CONNECT` for API 31+ SCO control).

**Approach:**
- Add an `enum class AudioRoute { EARPIECE, SPEAKER, BLUETOOTH, WIRED }` and expose
  `val audioRoute: StateFlow<AudioRoute>` plus `val availableRoutes: StateFlow<List<AudioRoute>>`.
- **API 31+ (preferred):** use `AudioManager.availableCommunicationDevices`,
  `setCommunicationDevice(AudioDeviceInfo)`, `clearCommunicationDevice()`. Map
  `AudioDeviceInfo.type` (`TYPE_BLUETOOTH_SCO`, `TYPE_WIRED_HEADSET`/`TYPE_WIRED_HEADPHONES`,
  `TYPE_BUILTIN_EARPIECE`, `TYPE_BUILTIN_SPEAKER`) to `AudioRoute`.
- **< 31 fallback:** keep `isSpeakerphoneOn` + `startBluetoothSco()` / `stopBluetoothSco()` +
  `isBluetoothScoOn`; observe SCO state via `AudioManager.ACTION_SCO_AUDIO_STATE_UPDATED`.
- Register an `AudioDeviceCallback` (`registerAudioDeviceCallback`) while in a call to refresh
  `availableRoutes` on connect/disconnect and auto-switch (BT > wired > default). Unregister on teardown.
- Default: voice → earpiece (or BT/wired if present); video → speaker (or BT/wired).

**Acceptance:**
- Plugging a wired/BT headset mid-call moves audio to it; unplugging falls back.
- The picker lists only present devices; selecting one routes audio there.
- No regression to the existing earpiece/speaker toggle. Audio mode restored to normal on end.

**Gotchas:** BT SCO takes ~1s to come up (< 31); guard against races. `BLUETOOTH_CONNECT` is a
runtime permission on API 31+ for some operations — request it lazily, degrade gracefully if denied.

**Effort:** M. **Risk:** M (API-level branching; can't test locally — review carefully).

---

### Task B2 — Mid-call camera on/off
**Goal:** toggle the local camera during a call.

**Files:** `CallManager.kt` (add `toggleCamera()`, expose `val cameraOn: StateFlow<Boolean>`);
`CallScreen.kt` (camera on/off button in `InCallContent`).

**Approach — start simple, then optionally go full:**
1. **v1 (no renegotiation):** `Session.videoTrack?.setEnabled(false/true)`. This stops sending
   frames (remote sees a frozen/black frame) without renegotiating. When turning the camera
   fully off you may also `capturer.stopCapture()` to release the sensor and `startCapture(...)`
   to resume; keep the track and transceiver in place. Update `cameraOn`.
2. **v2 (stretch, real add/remove):** for audio calls that upgrade to video, add a video track
   and **renegotiate**: `pc.addTrack(videoTrack, listOf(STREAM_ID))`, create a new `Offer`,
   `setLocalDescription`, `sendSignal(Offer)`. The peer already handles a same-`callId` offer
   as a renegotiation (`handleRemoteOffer`). Watch for **glare** (both sides offer at once) —
   guard so only the caller re-offers, or add a simple "polite peer" rule.

**Acceptance:** turning the camera off stops outgoing video and updates the button; turning it
back on resumes; the audio path is unaffected; teardown still releases the camera.

**Gotchas:** don't leak the capturer/`SurfaceTextureHelper` on repeated toggles; reuse them.
Keep `_localVideo` `StateFlow` in sync so the self-preview hides/shows.

**Effort:** M (v1 is S). **Risk:** M for v2 (renegotiation).

---

### Task B3 — Tap-to-swap video PiP
**Goal:** tap the local self-preview to swap it with the remote full-screen view (and back).

**Files:** `CallScreen.kt` (`InCallContent` only). Pure Compose — no `CallManager` change.

**Approach:** add `var swapped by remember { mutableStateOf(false) }`. When `!swapped`,
remote → full (`VideoRenderer(remoteVideo, mirror=false, fillMaxSize)`), local → PiP box
(clickable). When `swapped`, reverse. Toggle `swapped` on PiP tap.

**Acceptance:** tapping the PiP swaps the two renderers; state holds for the call; audio
unaffected; no renderer leak (the `DisposableEffect(track)` keys handle re-attach).

**Effort:** S. **Risk:** Low. **Good first task.**

---

## §4 · Phase C — incoming-call UX

### Task C1 — Incoming notification with Accept/Decline actions (CallStyle)
**Goal:** the incoming-call notification shows **Accept** and **Decline** buttons and looks like
a real phone call, working from the shade and lock screen.

**Files:** `Notifier.kt` (`notifyIncomingCall` → add actions / use `CallStyle`); new
`CallActionReceiver.kt` (a `BroadcastReceiver`); `AndroidManifest.xml` (register the receiver).

**Approach:**
- On API 31+, build the notification with
  `NotificationCompat.CallStyle.forIncomingCall(person, declineIntent, answerIntent)`.
  Pre-31, fall back to two `NotificationCompat.Action`s.
- `declineIntent` → `PendingIntent.getBroadcast(...)` to `CallActionReceiver` with action
  `ACTION_DECLINE` → `CallManager.reject()`. No permission needed; works from anywhere.
- `answerIntent` → because **accepting needs mic/camera runtime permission** (which you can't
  request from a notification), route Answer through the **full-screen intent to `MainActivity`**
  (which shows the ringing `CallScreen`, where `withCallPermissions` already gates accept). If
  permission is already granted, `CallActionReceiver` may call `CallManager.accept(context)` directly.
- Keep the existing `setFullScreenIntent(...)` and `CATEGORY_CALL`.

**Acceptance:** incoming call posts a call-style notification with Accept/Decline; Decline ends
the call immediately; Accept opens the in-app ringing screen (and connects when perms granted);
notification is cleared on accept/decline/timeout (extend `Notifier.clearCall`).

**Gotchas:** register the receiver as `exported="false"`. Use a stable notification id
(`"call:$peer".hashCode()`, already the convention). On Android 14+, `USE_FULL_SCREEN_INTENT`
is restricted, but calling apps using `CallStyle` are exempt — note this in the manifest.

**Effort:** M. **Risk:** M.

---

### Task C2 — Ringtone + vibration
**Goal:** ring + vibrate on an incoming call; stop on accept/decline/timeout; honor silent mode.

**Files:** new `call/Ringer.kt` (small helper) or a section in `CallManager.kt`.

**Approach:** start when state becomes `Ringing(incoming = true)`; stop on any transition away
(observe `CallManager.state`, or call `ringer.start()/stop()` from `handleRemoteOffer` / `accept`
/ `reject` / `endWith`). Use `RingtoneManager.getRingtone(context, RingtoneManager.getDefaultUri(RingtoneManager.TYPE_RINGTONE))`
and `Vibrator`/`VibratorManager` with a repeating waveform. Respect `AudioManager.ringerMode`
(`RINGER_MODE_SILENT` → no sound; `RINGER_MODE_VIBRATE` → vibrate only). Optional: a ringback
tone for the caller while `Ringing(out)`.

**Acceptance:** incoming call rings + vibrates; stops reliably on every exit; silent mode
suppresses sound; the caller's own device does not ring for its outgoing call.

**Gotchas:** stop the ringtone in **all** end paths (accept, decline, remote hangup, timeout,
teardown) — leaks here are very noticeable. Don't fight the in-call `MODE_IN_COMMUNICATION`
audio mode; ring **before** `beginAudioRouting` takes the call audio mode.

**Effort:** S–M. **Risk:** M (lifecycle/leaks).

---

### Task C3 — Lock-screen full-screen incoming UI
**Goal:** an incoming call shows full-screen over the lock screen and turns the screen on.

**Files:** `MainActivity.kt` (window flags driven by call state).

**Approach:** when `CallManager.state` is `Ringing(incoming = true)`, call (API 27+)
`setShowWhenLocked(true)` + `setTurnScreenOn(true)` and request `KeyguardManager.requestDismissKeyguard`
if accepting; reset the flags when the call leaves the ringing/active states. The `CallScreen`
overlay already covers the app; the notification's full-screen intent (C1) launches `MainActivity`.

**Acceptance:** with the device locked, an incoming call lights up the screen and shows the
ringing UI over the keyguard; declining/accepting behaves correctly; flags are cleared after.

**Gotchas:** always reverse the window flags on call end (don't leave `showWhenLocked` on).
Pair with C1's full-screen intent.

**Effort:** S. **Risk:** Low–M.

---

## §5 · Phase D — robustness, trust, history

### Task D1 — Ongoing-call foreground service
**Goal:** a foreground service keeps an active call alive when the app is backgrounded, with an
ongoing notification (Hang up action) and tap-to-return; the call no longer dies if the process
is backgrounded.

**Why:** `CallManager` is a process singleton, but a backgrounded process with no foreground
service can be killed, dropping the call. Telegram runs the call in a foreground service.

**Files:** new `call/CallService.kt` (model it on `voice/WakeWordService.kt`); `CallManager.kt`
(start the service on call start/connect, stop on end); `AndroidManifest.xml` (declare the
service + `FOREGROUND_SERVICE_CAMERA` permission for video calls); `MainActivity.kt` (return-to-call).

**Approach:**
- `startForegroundService(...)` when a call begins; in `onStartCommand`, `startForeground(id,
  notif, FOREGROUND_SERVICE_TYPE_MICROPHONE [| FOREGROUND_SERVICE_TYPE_CAMERA])`. Must call
  `startForeground` within ~5s of start.
- Notification: `NotificationCompat.CallStyle.forOngoingCall(person, hangupIntent)` with a
  content intent that returns to `MainActivity`/the call screen.
- The service holds only the foreground + notification; `CallManager` still owns the media.
  Stop the service in `CallManager.endWith`/`teardownMedia`.

**Acceptance:** backgrounding an active call keeps it connected with an ongoing notification;
tapping it returns to the call screen; the notification Hang-up ends the call; no FGS crash on
Android 14 (correct types + permissions).

**Gotchas:** Android 14 requires the declared `foregroundServiceType` **and** the matching
permission; video calls need `FOREGROUND_SERVICE_CAMERA`. Start the service **after** permission
is granted (mic/camera). Ensure the service always stops (no lingering notification).

**Effort:** L. **Risk:** M–H (FGS rules; can't test locally — mirror `WakeWordService` closely).

---

### Task D2 — Call history / missed-call UI
**Goal:** a call-log screen backed by the already-wired `ComradeCore.callHistoryTyped`, plus
missed-call notifications.

**Why:** every call end already writes a record via `logCallTyped`; `callHistoryTyped(peer?)`
returns them but nothing renders them yet.

**Files:** new `ui/CallHistoryScreen.kt`; `MainActivity.kt` (a way to reach it — e.g. a header
action on the Chats list, or a small section); `Notifier.kt` (`notifyMissedCall`).

**Approach:** `ComradeCore.callHistoryTyped()` → `List<CallRecordInfo>` (`id, peer, media,
incoming, outcome, startedAt, durationSecs`). Render newest-first with an icon per
direction/outcome (incoming/outgoing/missed × voice/video), the peer title (`peerTitle(...)`),
relative time (`relativeTime(startedAt)` from `ui/DisplayName.kt`), and duration for connected
calls. Tapping a row opens the chat or offers "call back". On a missed incoming call (outcome
`missed`), post `Notifier.notifyMissedCall`.

**Acceptance:** the log lists past calls with correct direction/outcome/duration/time; missed
calls show a notification; call-back works.

**Gotchas:** `startedAt`/`durationSecs` are seconds (ULong in Rust, Long in the facade). Group
by day using `dayLabel(...)` if you want date headers.

**Effort:** M. **Risk:** Low (read-only over existing data; pure UI).

---

### Task D3 — Encryption-emoji verification (SAS) — **security-sensitive**
**Goal:** show a short emoji sequence derived from the call's DTLS-SRTP fingerprints so both
parties can verbally confirm there's no man-in-the-middle (Telegram's 4-emoji check).

**Why:** signaling is E2E-encrypted, but a verifiable SAS is the trust cherry-on-top and a
headline Telegram feature.

**Approach (recommended split):**
- Extract **both** DTLS fingerprints from the offer and answer SDP (`a=fingerprint:` lines).
- **Do the derivation in Rust** (`comrade_core`, e.g. a `call_sas(local_fp, remote_fp)` fn):
  hash the two fingerprints in a **canonical order** (e.g. sorted, or caller||callee), map the
  digest to N emojis from a fixed alphabet. Unit-test it (the repo convention is tested pure
  Rust). Expose via UniFFI; surface through `ComradeCore`.
- Kotlin just passes the two SDPs (or fingerprints) in and **displays** the emojis in
  `CallScreen` (a small row, tap to expand an explanation).

**Acceptance:** both devices in the same call render the **identical** emoji sequence; changing
either fingerprint changes it; derivation has Rust unit tests; the ordering is canonical so both
sides agree regardless of who is caller.

**Gotchas:** get the canonical ordering right or the two sides disagree. Never derive from only
one side's fingerprint. Have a human review the crypto. Fixed, unambiguous emoji alphabet.

**Effort:** L. **Risk:** H (security). **Do not merge without review.**

---

### Task D4 — Connection-quality indicator
**Goal:** show a "weak connection" indicator based on live WebRTC stats.

**Files:** `CallManager.kt` (poll stats, expose `val quality: StateFlow<CallQuality>`);
`CallScreen.kt` (indicator icon while `Active`).

**Approach:** while `Active`, poll `pc.getStats { report -> … }` every ~2s. From
`RTCStatsReport`, read the inbound-RTP stats: packet loss ratio, jitter, and round-trip time
(from `remote-inbound-rtp`/`candidate-pair`). Map to `GOOD / MEDIUM / POOR` with simple
thresholds; expose via `StateFlow`; cancel polling on teardown.

**Acceptance:** a lossy/high-RTT link surfaces a "weak connection" indicator; it clears when the
link recovers; polling stops on call end.

**Gotchas:** `getStats` is async and its callback runs off the main thread — publish via
`StateFlow`. Parse stat keys defensively (they vary). Throttle; don't spam.

**Effort:** M. **Risk:** M.

---

## §6 · Definition of Done (every task / PR)

- [ ] Branch restarted from the latest `main`; one task per PR; base = `main`.
- [ ] Diff re-read against **§2 pitfalls** (singleton rules, UShort, deprecations, observer set).
- [ ] If Rust touched: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test` all pass locally.
- [ ] WebRTC still initializes **lazily** (startup path and on-device smoke tests untouched).
- [ ] New user-facing strings added to `res/values/strings.xml` (no hard-coded literals in new UI).
- [ ] New permissions/services declared in `AndroidManifest.xml` with correct FGS types.
- [ ] PR description states what was changed, the manual test steps (**two accepted devices**),
      and any deviation from this doc.
- [ ] **All** CI checks green before merge. Never merge on red.
- [ ] Teardown still releases camera + mic + `PeerConnection` + EGL (no hardware left held).

## §7 · Suggested order
`B3` (warm-up, pure Compose) → `C2` + `C1` + `C3` (incoming UX) → `B1` (audio routing) →
`D1` (foreground service) → `B2` (mid-call video) → `D2` (call history) → `D4` (quality) →
`D3` (SAS, last — needs review). Ship each as its own small PR.
