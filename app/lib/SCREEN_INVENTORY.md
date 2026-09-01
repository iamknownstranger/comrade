# Screen inventory — what the two frontends do, and what the unified app does

Phase 3 (UI reconstruction) working document. Every screen the Flutter app has
to carry, read directly out of the two implementations it replaces:

- **Android / Jetpack Compose** — `android/app/src/main/java/mullu/comrade/`
  (4,838 LOC of UI across 13 files)
- **Desktop / vanilla-JS SPA** — `desktop/ui/` (`index.html` 431,
  `main.js` 2,335, `styles.css` 1,755)

Two things this document is for: (1) so nothing is silently dropped in the
port, and (2) so every place the two platforms **disagree** is decided on
purpose, in writing, rather than by whichever file the porter happened to read
first. §3 is the divergence ledger; it is the part worth arguing with.

> **Status.** `flutter analyze --fatal-infos` is clean and `flutter test` is
> green (240 tests, 4 skipped) against the in-memory fake. The media seams now
> have a real Android implementation behind them (picker, camera, in-memory
> playback, `FLAG_SECURE`), and `flutter build apk --debug` **completes** —
> so that Kotlin is compiled and packaged, with both ABI slices of the Rust core
> in the APK. What none of it has done is *run*: no device, no emulator, so not
> one of those paths has been exercised. Read §5 before believing anything is
> finished.

---

## 1. Feature matrix, before and after

| Surface | Android | Desktop | Unified app |
|---|---|---|---|
| Vault door / onboarding | ✅ username + passcode + confirm | ⚠️ passphrase only | ✅ Android's flow + desktop's reveal toggle |
| Chat list | ✅ | ✅ (sidebar list) | ✅ list, two-pane ≥840 px |
| Conversation | ✅ | ✅ | ✅ |
| Message requests | ✅ banner + inbox | ✅ inline in sidebar | ✅ banner + inbox |
| New chat / directory search | ✅ | ✗ | ✅ |
| Encrypted media | ✅ inline image/audio/video | ⚠️ download-then-view | ✅ inline images; audio in memory; video/files handed to the system viewer from memory |
| Attach / camera | ✅ `GetContent` picker | ⚠️ `<input type=file>` | ✅ document picker + camera photo/video, capability-gated |
| Voice notes (record) | ✅ hold-to-talk | ✗ | ✅ tap-to-record — see D13 |
| Sabha feed | ✅ | ✅ + char cap | ✅ union |
| Journal | ✅ | ✗ | ✅ |
| Tara | ✅ | ✗ | ✅ |
| Call UI | ✅ full (routes, camera, PiP, SAS, quality) | ⚠️ mute + hangup + SAS | ✅ Android's, engine behind a seam |
| Call history | ✅ | ✗ | ✅ |
| Settings | ✅ | ⚠️ TURN modal only | ✅ Android's superset |
| Profile page | ✅ collapsing header, info rows, Media/Files/Links/Voice | ✅ same rules, DOM | ✗ — rules ported, no page yet (D43) |
| Couple sandbox / Partner Portal | ✗ | ✅ | ✅ |
| Off-Grid / Travel workspace toggle | ⚠️ voice command only | ✅ switch | ✅ |
| Mesh status | ✅ banner | ✅ sidebar pills | ✅ both |
| Voice / wake word / model download | ✅ | ✗ | ✗ — stays native, see D21 |

---

## 2. Screen by screen

Each entry: **tree · state · interactions · backend · notes.** Backend names are
`ComradeRepository` methods (`app/lib/src/data/comrade_repository.dart`), which
map 1:1 onto `ComradeCore.kt`'s methods and `commands.rs`'s commands.

### 2.1 Onboarding / vault door
`onboarding_screen.dart` ← `ui/OnboardingScreen.kt` (233) + `index.html#screen-vault`

- **Tree**: centred card (max 380 px, desktop's `.vault-card` measure) — brand
  mark, subtitle, `username` field (create/claim only), `passcode`, `confirm`
  (create only), inline error, submit.
- **State**: `vaultExists` (from `AppPhase`), local `_claimOnly`, `_busy`,
  `_reveal`, `_error`.
- **Interactions**: submit; reveal/hide passphrase; a legacy vault that unlocks
  with no username falls into the claim-a-handle step.
- **Backend**: `unlockVault`, `setUsername`.
- **Validation** (client-side, mirrored from Kotlin, re-validated in Rust):
  handle 3–24 chars `[A-Za-z0-9_]`, passcode ≥6, passcode == confirm.

### 2.2 Shell / navigation
`home_shell.dart` ← `MainActivity.kt` (896) + `index.html` sidebar + `styles.css:311-390,1665-1740`

- **Tree, <840 px**: `Scaffold` → app bar (hamburger / back / conversation
  header) · mesh banner · body · `NavigationBar` (4 destinations) · FAB ·
  `Drawer` (profile header + Call history, Partner Portal, Settings).
- **Tree, ≥840 px**: `Row` → sidebar (brand + workspace badge · 4 destinations ·
  MORE group · relay/mesh pills · identity chip) │ divider │ column(mesh banner,
  header, body). Chats becomes list+detail.
- **State**: `_tab`, `_chatNav` (list/newChat/requests), `_secondary`,
  `openConversationProvider`.
- **Interactions**: tab switch; drawer; back chain (secondary → chat sub-screen
  → open conversation → exit); voice/video call from the conversation header;
  alias editor; copy npub from the identity chip.
- **Backend**: none directly; delegates to the screen providers.
- **Detail kept**: the conversation view owns the whole screen on a phone
  (no bottom bar under it) — Telegram-style, as in Compose.

### 2.3 Chat list
`chats/chats_list_screen.dart` ← `ChatsScreen.kt:142-238`

- **Tree**: requests banner (only when count > 0) → rows of
  `PeerAvatar · title / "You: "+last · relativeTime`, divider inset 76 px.
- **State**: `conversationsProvider` (`AsyncValue<List<ConversationInfo>>`),
  `messageRequestCountProvider`; `selectedPeer` highlights the two-pane row.
- **Interactions**: open a conversation; open the requests inbox; new chat
  (FAB / header action); empty-state CTA.
- **Backend**: `conversations`, `messageRequests`.
- **Events**: reloads on `IncomingDirectMessage`, `IncomingMedia`,
  `PeerProfileUpdated`, `MessageStatusChanged`.

### 2.4 Conversation
`chats/conversation_screen.dart` ← `ChatsScreen.kt:412-993` + `main.js:580-717`

- **Tree**: `Stack`[ `ListView` of merged items · jump-to-latest FAB ] → error
  line → reply chip → `MessageComposer`.
- **The composer is Telegram's** (`widgets/composer.dart`): emoji **inside the
  field on the left**, paper clip **inside it on the right**, and one round
  button outside on the right that is **Send** whenever there is text and
  otherwise the current capture control — with a small **swap** button beside it
  cycling voice → photo → video through whatever this device actually has.
  Telegram hides that switch behind a press-and-hold on the mic; here it is
  visible, because a hidden gesture sharing a target with "start recording" is
  the worst of both. Recording replaces the whole row with discard · elapsed ·
  send. Every control is capability-gated: a platform with no recorder and no
  camera shows a plain Send button rather than one that fails on tap.
- **Item**: optional `DaySeparator` (inside the item, so indices still match
  the merged list) then either a `MediaAttachmentBubble` or a `MessageBubble`
  (quoted preview · text · `clockTime` · status ticks when outgoing). Both are
  wrapped in the same `ReplyAffordance`, so an attachment is repliable exactly
  the way a message is; a tap on a photo or a video opens `MediaViewerPage`.
  Sending one goes through `AttachmentPreviewSheet` first — see the interaction
  note below.
- **State**: `conversationProvider(peer)` → `{messages, media, items,
  replyingTo, sending, attaching, error}`, where `replyingTo` is a `ChatItem`
  (text *or* media — the `e` tag does not care which kind of event it names);
  local `_loadedOnce`,
  `_knownItemCount`, `_newMessagesBelow`, `_atBottom`.
- **Interactions**: send; long-press or hover → reply (to a message or to an
  attachment); cancel reply; attach → **confirm in the preview sheet** → send;
  tap a photo/video → full screen; jump to latest; scroll.
- **Attaching is two steps, not one** (`widgets/attachment_preview.dart`). A pick,
  a capture or a document opens a sheet holding the file itself, its name and
  size, and a caption box seeded from the composer
  (`util/attachment_caption.dart`); nothing is encrypted or uploaded until Send,
  and backing out leaves the draft untouched. A file that cannot be sent — empty,
  or over 10 MB — is refused *before* the sheet rather than after the upload
  begins. A voice note skips the sheet: the recording strip it came from already
  holds the clip with a discard button and a send button.
- **Backend**: `messages`, `media`, `sendDm`, `sendMedia`,
  `markConversationRead`.
- **Load-bearing detail — the scroll rule.** Commit `a76bacf` ("stop yanking
  readers to the bottom"). Auto-scroll happens **only** on first load, or when
  the reader was already near the bottom; otherwise the jump-to-latest button
  lights up. The rule is `isNearBottom` / `isNearBottomByOffset` in
  `util/chat_thread.dart`, unit-tested. The desktop SPA does the opposite —
  `log.scrollTop = log.scrollHeight` on every render (`main.js:624`) — and that
  is the bug the Compose fix was written against.

### 2.5 Message requests
`chats/requests_screen.dart` ← `ChatsScreen.kt:995-1095`

- **Tree**: rows of `PeerAvatar · shortNpub (monospace) · last message` +
  Block / Accept.
- **Backend**: `messageRequests`, `acceptRequest`, `blockConversation`.
- **Detail kept**: a request row shows the **raw key**, never a published
  handle. A stranger's self-declared name is exactly what an impersonation
  attempt would set, and this screen is where the trust decision happens.

### 2.6 New chat
`chats/new_chat_screen.dart` ← `ChatsScreen.kt:240-410`

- **Tree**: query field → (key detected ? "Start chat with npub1…" : Search) →
  the honesty paragraph about directory relays → results → Contacts.
- **Backend**: `searchProfiles`, `addContact`, `contacts`.
- **Detail kept**: starting a chat pins the **key only** (trust-on-first-use);
  the alias stays the user's to set.

### 2.7 Call history
`chats/call_history_screen.dart` ← `ui/CallHistoryScreen.kt` (147)

- **Tree**: rows of `PeerAvatar · title · "Incoming · 3:05 · 2h"` · media icon.
- **State**: `callHistoryProvider`, `contactsByNpubProvider`.
- **Backend**: `callHistory`, `contacts`.
- **Detail kept**: `missed|declined|busy|failed` tint the media icon with the
  error colour, so a missed call reads at a glance. Newest first, no
  client-side sort (the core already returns that order).

### 2.8 Journal
`journal_screen.dart` ← `ui/JournalScreen.kt` (330)

- **Tree**: composer card (multiline field · 5 mood chips · Save · the
  "only on this device" line) → day-grouped entry cards with share and delete.
- **Backend**: `journal`, `addJournalEntry`, `deleteJournalEntry`,
  `shareJournalEntry`.
- **Sharing**: one entry to one saved contact, as an encrypted DM. Comrade's own
  picker, never the platform share sheet — the system sheet would offer every
  app on the device a plaintext copy of the most private thing here. The bubble
  it becomes is `SharedNoteBody` in `message_bubble.dart`.
- **Not ported**: Vosk dictation (D21).

### 2.9 Sabha feed
`feed_screen.dart` ← `ui/FeedScreen.kt` (155) + `main.js:307-407`

- **Tree**: composer (field · "Public — anyone can read this" · counter ·
  Post) → Chitthi cards (`author` / `You` · relative time · body · reply hint).
- **Backend**: `sabhaTimeline`, `broadcastChitthi`.
- **Events**: `IncomingChitthi` is **prepended**, never re-fetched, so a busy
  relay cannot reset the reader's scroll.

### 2.10 Tara
`tara_screen.dart` + `state/tara_providers.dart` ← `ui/TaraScreen.kt` (415) + `ui/TaraStream.kt` (71)

- **Tree**: opt-in explainer, or thread(bubbles · crisis cards · pending user ·
  thinking · streaming) + composer card(field · Send · the "not a therapist"
  footer · Clear).
- **State**: `taraOptInProvider`, `taraProvider` → `{thread, opener, resources,
  pendingUser, thinking, streaming, error}`.
- **Backend**: `taraThread`, `taraSend`, `taraOpener`, `taraCrisisResources`,
  `clearTaraThread`.
- **Load-bearing detail — streaming vs crisis.** An ordinary reply is paced out
  word by word (`streamTaraReply`, cumulative-prefix `Stream<String>`) so the
  companion reads as thinking out loud. A reply the engine flagged `crisis` is
  published **whole, in one state update** — helpline numbers must never be
  half-rendered while an animation catches up. Both branches are covered by
  `test/tara_screen_test.dart`, which asserts a crisis turn publishes exactly
  one *distinct* frame and that no frame is ever a strict prefix.
- The chunker is lossless by construction and property-tested for it
  (`chunkText(t).join() == t` at every chunk size). It iterates **runes**, not
  UTF-16 code units, so an emoji cannot be split in half — a small improvement
  on the Kotlin original.

### 2.11 Settings
`settings_screen.dart` ← `ui/SettingsScreen.kt` (691) + `index.html#modal-turn`

- **Cards**: profile (avatar · @handle · full key · Copy key · the
  "names repeat, keys don't" paragraph) → appearance (System · Light · Dark) →
  background connectivity switch → block-screenshots switch → TURN relay → lock
  vault → "In the lab" + `core vX`.
- **The screenshots card is absent where the platform cannot honour it**, and it
  is the one preference here stored *natively* rather than through
  `AppPreferences` — `FLAG_SECURE` has to be right before the first frame,
  earlier than even the preference store is open, and the Compose app reads the
  same key. Every other card on this screen persists too, through
  `PersistentPreferences` (D26b); the screenshots switch is no longer the only
  one that survives a restart, only the only one the system stores. See §5.8.
- **Backend**: `setUsername`, `turnServerStatus`, `setTurnServer`,
  `testTurnConnectivity`, `lockVault`, `version`.
- **Load-bearing detail — the TURN card is write-only** (AUDIT COMMS-02). The
  URL round-trips; the username and credential go in and are **never read
  back**, because nothing in the core exposes them. Re-opening the editor shows
  those two fields blank, and the dialog says so. There is no getter to add
  later — that is the property, not a gap.

### 2.12 Call overlay
`call_screen.dart` + `state/call_providers.dart` ← `call/CallScreen.kt` (676) + `call/CallUiState.kt`

- **Phases**: Ringing (accept/decline or cancel) · Connecting/Active (one
  subtree — see below) · Ended (terminal card).
- **Active tree, video**: main renderer · tappable self-view tile (swaps; hosts
  the camera-flip control) · name+timer pill · signal-strength bars · control
  bar (mute · camera · chat · route · end) over a scrim — the pill and controls
  are FaceTime-style self-hiding chrome (fade after ~4 s, tap to toggle).
- **State**: `callProvider` → `{state, muted, cameraOn, audioRoute,
  availableRoutes, quality, pip, videoSuspended, remoteVideoPaused}`.
- **Details kept**: one composition subtree for Connecting **and** Active, so
  the video surfaces are not destroyed and recreated exactly as the first
  frames arrive; the local track mirrors wherever it renders and the remote
  never does; the four bars render UNKNOWN as empty (nothing measured is not
  zero signal); a side that stopped sending shows "Video paused", never a
  frozen frame; the chat button shrinks the call into an in-app corner tile
  (native window, or the draggable in-app `FloatingCallTile`).
- **Not implemented**: the media engine itself. See D29 / §5.

### 2.13 Couple sandbox
`couple_screen.dart` ← `index.html#screen-couple` + `main.js:1835-2012`

- **Tree**: pairing form (partner npub · Sakha/Sakhi · Pair & enter), or the
  sandbox (header · ledger panel · media note). Two panels side by side above
  1600 px, mirroring `styles.css:1735`.
- **Backend**: `sakhaStatus`, `pairSakha`, `sakhaAddEntry`, `sakhaReadLedger`,
  `syncLedger`; live refresh on `LedgerUpdated`.

---

## 3. Divergence ledger

Every place the two frontends disagreed, and what the unified app does. "Why"
is the part that matters; a divergence resolved without a reason is a
divergence that gets re-litigated.

| # | Divergence | Android | Desktop | Decision | Why |
|---|---|---|---|---|---|
| D1 | **Display-name order** | alias → @handle → key (`peerTitle`) | @handle → key (`displayName`; no alias concept at all) | **Android** | Desktop's is a strict subset. The alias is the only name the *user* chose; a published handle is a self-declared claim by the peer. Dropping the alias would make every name spoofable. |
| D2 | `shortNpub` cut | head 10 + tail 4, above 16 chars | head 11 + tail 5, above 18 | **Android** | Its exact output is pinned by a unit test, and the conversation header renders it beside a 36 dp avatar where 4 tail chars still fit on a phone. |
| D3 | **Conversation header** | avatar + title + npub tail (monospace) | `displayName(peer)` only — no key anywhere | **Android** | The key is the identity; a header showing only a self-declared handle is the exact shape of an impersonation. The unified header **always** shows the tail. Tested. |
| D4 | **Bubble timestamps** | wall clock `HH:mm` under day separators | relative "3m ago" per bubble, no separators | **Android** | Deliberate fix (`a76bacf`). A relative stamp drifts while the screen is open; a clock under a day header does not. |
| D5 | **Auto-scroll on reload** | only if first load or already near the bottom | `scrollTop = scrollHeight` unconditionally | **Android** | The Compose behaviour *is* the fix for the desktop behaviour. Reading history must not be interrupted by someone else's message. |
| D6 | Delivery ticks | glyph from status; **missing status → ✓** | `✓` only for exactly `"sent"`; `STATUS_RANK` stops a late "delivered" undoing "read" | **Union** | Android's glyph rule (a tick-less outgoing bubble reads as "didn't send") **plus** desktop's rank guard, which Android never needed because it re-read the whole thread from the store. Live-event updates need it. Tested. |
| D7 | Reply affordance | long-press | hover button | **Both** | One codebase serves touch and pointer. Long-press has no discoverable equivalent with a mouse; a hover target has none with a finger. |
| D8 | Unknown reply target | render no quote | render "Original message" | **Android** | A placeholder implies we know something about a message we cannot see. |
| D9 | **Message requests** | banner → dedicated inbox screen | inline list in the contacts sidebar, Accept/Block per row | **Android** | Accept shares your @handle with a stranger. Inline buttons in a scrolling sidebar make that one mis-tap away. |
| D10 | Request row identity | `shortNpub` only | `displayName(peer)` (may show a handle) | **Android** | Same reasoning as D3, at the moment it matters most. |
| D11 | **Media plaintext** | images → bounded in-memory LRU; audio/video → `cacheDir/media`, purged on background (AUDIT S-4) | object URLs, never revoked | **Android, improved** | Flutter decodes images from bytes, so the unified app keeps *everything* in a bounded in-memory LRU and writes **nothing** to disk — audio plays from a `MediaDataSource` over the bytes, and anything handed to another app is served from memory by `InMemoryMediaProvider` (a seekable proxy descriptor, not a pipe, because viewers seek). One whole class of at-rest leak stops existing. The cache is still dropped on background and on vault lock, because the recents thumbnail and an unlocked phone are the other half of the same concern. Desktop's never-revoked object URLs were a real leak of the same shape and are now a bounded, revocable cache too. |
| D12 | Media auto-load | images auto-load | everything needs "Download & view" | **Android** | Images are the common, low-risk case; a tap-to-load image feed is worse UX for no privacy gain (the fetch is E2E either way). |
| D13 | **Voice notes (record)** | hold-to-talk mic in the composer | none | **Android's feature, not its gesture** | Revisited: press-and-hold is what has no meaning with a mouse — the *feature* was worth keeping. The composer's mic is tap-to-start / tap-to-send, with an explicit discard, and it is hidden entirely where no recorder exists. The recorder itself is the preserved native `VoiceRecorder`; the Dart side reads its clip once and deletes it, which is the deletion discipline that channel's path-not-bytes contract asks for. |
| D14 | Attachment picker filter | `*/*` | `image/*,audio/*` | **Android** | The core sends arbitrary MIME types already; the desktop filter is narrower than the backend. The unified picker is `ACTION_OPEN_DOCUMENT` with an any-type filter. |
| D15 | UPI `/pay` preview | none (voice command only) | live debounced `extract_payments` in the composer + chips under bubbles | **Desktop — carried but not yet wired** | `extractPayments` is on the repository interface; the composer preview is **not** re-implemented in this pass. An honest gap, not a decision. |
| D16 | Feed length cap | none | 2,000 chars + live counter | **Desktop** | Without it a post is silently rejected by the relay. |
| D17 | Feed "is this mine?" | compares `author == "you"` — a sentinel it invents when optimistically prepending, which never matches a real npub | compares against `state.identity.npub` | **Desktop** | Android's comparison cannot match a real event; it only ever works on the optimistic local copy. |
| D18b | **Reply to an attachment · caption an attachment · full-screen viewer** | none — media bubbles had no reply, sends were captioned with the *file name*, and a tap did nothing | none — same three gaps | **New, in all three** | Three gaps neither frontend had closed, fixed the same way everywhere rather than only in the migration target. A reply target is any chat item, because a nostr `e` tag names an event and a NIP-94 attachment is one — no core or DTO change was needed. The caption is whatever is in the composer when you attach (Telegram's rule), except while a reply is pending, when those words are a half-written reply and taking them would lose it. Photos and videos open full screen on tap, against black, from the bytes the bubble already holds. The rules are pure and tested three times over: `util/attachment_caption.dart`, `ui/AttachmentCaption.kt`, `ui/attachment_caption.mjs`. |
| D18c | **Preview before sending media** | none — a pick went straight to encrypt-and-upload | none — same gap | **New, in all three** | The first sight of what you had chosen was in your own thread, already delivered, and the 10 MB cap only surfaced *after* the upload began, throwing away the caption you had just written. Now a pick opens a sheet with the file, its name and size, and a caption box seeded from the composer; nothing leaves the device until Send and backing out costs nothing. Refusals (empty, oversize) happen before the sheet, from one shared message. Video plays only where a player needs no file — a blob URL on desktop; Android and Flutter show a card, because staging an unsent attachment's plaintext to disk to feed a player would break AUDIT S-4. Voice notes are exempt: the recording strip is already that confirmation. Rules and their tests are mirrored three ways alongside D18b's. |
<!-- The large-attachment rows are D34/a/b. They were first written as D21, which
     was already the voice/wake-word row further down this table — and a duplicate
     id makes every citation to it ambiguous. Renumbered to the free block rather
     than renaming the older row, whose citations are all still good. -->
| D34 | **Large attachments (over 10 MB)** | protocol + gate landed, transfer inert | protocol + gate landed, transfer inert | **Neither, yet** | The hosted path cannot carry these and the ceiling is not ours to raise: it encrypts and buffers the whole file in memory, and the Blossom host has its own cap (`blossom.band` publishes 20 MiB free / 100 MiB paid; `nostr.download` advertises none, so the real limit is whatever that operator configured today). So over the cap the file goes **peer to peer**, reusing `comrade_core::share`'s chunked, resumable, receiver-driven WebRTC transfer rather than a second copy of it. Landed so far: `comrade_core::handoff` (its own envelope, since a handoff has no together session; offer → accept → transport, no `Ask`, because the sender picked the file), the accepted-conversation gate on receipt — the same bar a call signal clears, and for the same reason, since both make this device gather ICE for whoever is on the other end — and `BridgeEvent::AttachmentHandoff`. **Not** landed: the data-channel half, which still has to be lifted out of `together.ShareTransfer` (today hard-wired to a listening session's file and playhead). Both frontends route the event and do nothing with it, deliberately, so no UI offers a large send that would never complete. Two costs this road cannot avoid and the UI must state: the recipient has to be **online now**, and a **direct path must exist** — `RelayPolicy::DirectOnly` is the default and `AUDIT.md` §8.1 puts 30-40% of remote pairs behind CGNAT. |
| D34a | **Large attachments — the Android transfer** | ✅ picks, offers, receives and stages a file of any size, peer to peer | protocol + gate landed, transfer inert (D21) | **Android, first** | D21's missing half, on one frontend. `together.ShareTransfer` was generalised into `transfer.FileTransfer` — the same chunking, the same 256 KiB flow-control watermark, the same `shareChunksToSend` budget, the same per-chunk `chunkFrameFits` check, the same whole-file hash and the same two-line relay policy — with four session-shaped facts lifted out into `FileTransfer.Wiring`: how a signal reaches the far side, where the sender's bytes come from, where the receiver's land, and who is told when it finishes. `handoff.AttachmentHandoffManager` drives it from `BridgeEvent::AttachmentHandoff` and owns the transfer, so a 400 MB send does not die because a Compose screen was disposed. **The sender never holds the file in memory**: a picked `content://` URI is read by offset through `ContentUriSource`, which is also why the pick probes the last byte first — a provider handing back a pipe cannot serve a receiver-driven transfer, and finding that out before an offer exists beats a stalled progress bar afterwards. Incoming bytes stage in `cacheDir/attachment-handoff/` under `HandoffDecisions.stagedFileName` (the Kotlin mirror of the Rust, with the same `../../etc/passwd` / wrong-length / non-hex refusals), one directory so the sweep is one sweep, and dismissing the finished card deletes the plaintext — AUDIT S-4. Every peer-chosen field is checked before it reaches a filesystem or a screen: the chunk size must be the 16 KiB both senders use (a 1-byte chunk size on a 4 GB offer is a four-billion-entry tracker), the total must fit `MAX_HANDOFF_BYTES` and the free space actually available, and the filename is display text with separators and control characters stripped. **Decline is not Refuse** — a person's no sends `Decline`, and only the network's no sends `Refuse`. The preview sheet asks `attachment_route_for_bytes` which road a file takes and says so, including both costs of the direct one (they must be online *now*; a direct path must exist, which `AUDIT.md` §8.1 puts at 30-40% failing behind CGNAT); a refusal names the size that would work instead. **Still missing, named rather than implied**: an incoming offer is visible only in the open thread — there is no notification for one yet, so an offer can go unanswered while the sender waits; the protocol has no completion acknowledgement, so the *sender's* panel keeps showing the transfer as running until they dismiss it while the receiver's says it arrived; a large send produces no chat bubble, because there is no NIP-94 event to make one from — the panel is the only record; camera captures stay on the hosted road and are still refused over 10 MB, since routing them otherwise would mean keeping a 400 MB plaintext capture on disk; and no transfer has run between two real devices. Flutter still routes the event and does nothing (D21); the desktop half is **D21b**. |
| D34b | **Large attachments — the transfer itself (desktop)** | ✅ built too, and it stages to disk (D34a) — which is why it needs no ceiling | **built** — the same pump `together` uses, pointed at an attachment | **Android for the sink; desktop matches it everywhere else** | D21 recorded the protocol as landed and the transfer as "not landed: the data-channel half, which still has to be lifted out of `together.ShareTransfer`". On the desktop it is lifted. `share_transfer.mjs` (pump, framing, tracker, watermarks, path verdict) is unchanged; what was Together-specific came out of the *driving* code in `main.js`, and the new `handoff_transfer.mjs` holds the seam — one codec per protocol, so the driver never writes `{"share":…}` or `{"handoff":…}` itself and a change to it cannot serve one encoding while breaking the other. Two new Tauri commands: `attachment_handoff_send` and `attachment_route_for_bytes` (the road comes from the core, never from a 10 MB comparison in the frontend). **The divergence a porter must decide about is the sink.** A webview has no filesystem and a `.part` file of decrypted plaintext in the app data directory is what AUDIT S-4 forbids, so the desktop holds an incoming file in memory and caps *both* directions at 256 MiB (`MAX_HANDOFF_BYTES`) — `SubtleCrypto` has no streaming digest, so each end must fit the whole file in memory once to fingerprint or verify it. Android stages to disk under `handoff::staged_file_name` and needs no such cap; a unified app should follow Android and treat the desktop's ceiling as a webview limitation, not as the feature's shape. The rest transfers as-is: an offer card with kind, sanitised filename, size and caption, Accept only when the offer can actually complete, `Decline` (never `Refuse`) for a person saying no, progress from the tracker's fraction, and both honest failures said out loud — they must be online now, and a direct path must exist (§8.1: CGNAT, 30-40% of remote pairs). |
| D18 | Feed reply marker | ignored | `↳ reply to abc123…` | **Desktop** | The DTO carries `reply_to`; dropping it loses real information. |
| D19 | TURN card | status + Edit + **Test relay connectivity** diagnostic | modal only, no status, no test | **Android** | Strict superset, and the diagnostic is the difference between "calls fail" and "calls fail *because the relay is unreachable*". |
| D20 | Vault lock | "Lock vault now" | none | **Android** | The deliberate, user-initiated version of what process death does by accident. |
| D21 | **Voice / wake word / model download** | ~1,300 LOC of Android services | none | **Neither — stays native** | No cross-platform on-device recogniser. The Android settings screen's own rule is "no fake switches"; a mic button that cannot listen is worse than no mic button. The "In the lab" copy now says so on every platform. |
| D22 | Journal · Tara · Call history · Onboarding | ✅ | ✗ (commands registered, no caller) | **Ported** | This is the actual parity debt `docs/FRONTEND_STRATEGY.md` §2 identified. |
| D23 | **Couple sandbox** | ✗ ("engine level only, not usable from the app yet") | ✅ working against real commands | **Ported** | Both statements were true *of their own platform*. Desktop proves the engine works end to end, so Android's "in the lab" copy is the stale half — and has been updated. |
| D24 | **Onboarding** | username + passcode + **confirm**, validated | one passphrase field, "any passphrase forges a brand-new vault" | **Android + desktop's reveal toggle** | On desktop a typo'd passphrase silently creates a *second empty vault* rather than reporting a wrong password. Confirm-on-create is the fix. The reveal toggle is desktop's and worth keeping: a long passphrase typed blind on a desktop keyboard is not otherwise verifiable. |
| D25 | **Colour source** | Material You dynamic colour on Android 12+, brand palette as fallback | brand palette always | **Brand palette everywhere** | Otherwise the two platforms render visibly different products. The call, crisis and status colours are load-bearing — a wallpaper-derived "error" container is not guaranteed to read as alarming. Dynamic colour can return later as an explicit opt-in. |
| D26 | Light theme | full light + dark schemes | dark only | **Both: system by default, overridable in Settings → Appearance** | Desktop's dark-only look is a stylistic default, not a requirement; a phone in daylight is a real use case. Following the OS cannot be the *only* rule, though — a Linux/Windows session that reports no preference resolves to **light**, which is how a dark-first product ends up bright with no way back — so the choice is explicit and the override outranks the OS. Stored through `AppPreferences`, which now **persists** (`PersistentPreferences`, `shared_preferences`) — see D26b. |
| D26a | **Light-mode accents** | — | `--accent` (`#6366f1`, `#f59e0b`, `#38bdf8`, `#fb7185`) | **Darker steps of the same hues** | `styles.css` has no `prefers-color-scheme` block, so every value in it was tuned against near-black, and there `--accent` is a *fill* carrying dark `--accent-contrast` text — never text itself. The first port reused it as light-mode `primary`, which `SectionCard` renders titles in: Travel measured **2.1:1** on white against AA's 4.5:1, and white-on-amber in a `FilledButton` was 2.15:1. Same for `ComradeSurfaces.light`'s good/warn/bad, which the sidebar pills use as *label* colours. `theme_test.dart` now asserts 4.5:1 across every text role × every background × all four skins × both brightnesses, so a palette edit cannot quietly reintroduce it. |
| D26b | **Where client preferences live** | Android `SharedPreferences` | nowhere — re-asked every launch | **`shared_preferences`, not the encrypted store** | The `AppPreferences` docstring used to point at the vault as the better home. It cannot be: every setting here has to be readable *before* the vault is unlocked — the theme has to be right for the onboarding screen's first frame, and background connectivity decides whether to connect at all — and none of it is secret. Opened once in `main.dart` (`SharedPreferencesWithCache`, allowlisted to `PrefKeys.all`) so the seam's getters can stay synchronous and a provider's `build()` can read one without a loading state. A store that fails to open **throws** rather than falling back to in-memory, on the same grounds `main.dart` refuses to fall back to the fake repository: settings that silently do not stick, under a UI that says they do, is the failure this seam's history warns about. |
| D26c | **`ColorScheme.outline`** | — | — | **Per-brightness, held to 4.5:1** | One shared `#6B7894` cannot be legible on a near-black surface and a white one at once; it reached **3.5:1** over `panelAlt`, under AA even for large text. All fifteen readers of `outline` render text or a status tick with it (clock times, delivery ticks, the "mDNS off" pill, the `core vX` stamp), so Material's "quiet role" was never a licence to be unreadable. Each scheme gets its own step, still measurably quieter than `onSurfaceVariant` — `theme_test.dart` asserts both the floor and the hierarchy. |
| D27 | **Navigation model** | bottom nav (Chats · Journal · Feed · Tara) + drawer (Call history, Settings) | sidebar (Sabha · Vault) + Modes group + status footer | **Both, by width** | Below 840 px (the width `styles.css:1668` already folds at) the Android chrome; at or above it, the desktop sidebar. Same widget tree, same state. |
| D28 | Section naming | Chats / Feed | Vault / Sabha | **Chats / Feed** | Plain-language labels for navigation; the product's own vocabulary ("Chitthi", "Sabha", "Hisab-Kitab") stays in body copy where it teaches rather than gatekeeps. |
| D29 | **Call controls** | mute · audio route menu · camera · flip · PiP swap · SAS · quality · proximity blank | mute · hangup · SAS | **Android's UI** | Desktop's is a subset because a webview has no audio-route API, not because anyone decided a call shouldn't have one. |
| D30 | Ended call | terminal card with the outcome | overlay just disappears | **Android** | "No answer" vs "Declined" vs "Couldn't connect" are different facts. |
| D31 | Error reporting | inline text next to the control | toast for everything (`safeInvoke`) | **Inline, plus SnackBar for background events** | A toast for a failed send disappears before it can be acted on, and does not say *which* send. |
| D32 | Mesh status | persistent banner under the top bar | two pills in the sidebar footer | **Both** | The banner is the mobile-relevant one (it is what you check with no signal); the pills fit the desktop chrome. |
| D33 | Event delivery | `ChatEventRouter` + integer "tick" counters screens re-read on | direct listeners per handler | **Direct listeners** | Riverpod invalidation is the tick counter, minus the global-refresh problem (a tick fired for *any* conversation reloaded *every* open one). |
| D35 | **Where the key lives** | conversation header showed the npub tail until 2026-07-30, then stopped (`AUDIT.md`) | never showed it (`displayName(peer)` only) | **Neither header; the profile page, in full** | The owner call that removed it from the Android header reasoned that the ⋮ menu put it "one tap away" — a conditional argument, and until now there was nowhere it resolved to. A profile page is that somewhere, and it is the only screen whose whole subject is who this person is. So the header carries presence and the profile carries the key: monospace, selectable, never truncated, never behind a disclosure. D3 is **re-scoped, not retired** — its reasoning (a self-declared handle shown with the key unreachable "is the exact shape of an impersonation") is what makes the key row unconditional, and `infoRows` is the first place that invariant is enforced by a test rather than by review. |
| D36 | **A blocked peer's action row** | — | — | **No actions at all, and a sentence saying why** | The obvious design offers Unblock. There is no unblock command in the core and no getter for the state to drive one — `STATE_BLOCKED` is written in `runtime.rs` and read only by `IncomingGate` — so the button would be a fake switch, which is the one thing `SettingsScreen`'s own rule forbids. `actionRow` returns an empty list and the page states the fact instead. The test asserts the emptiness *and* names the reason, so when an unblock command lands it fails and says what to change. |
| D37 | **Calls are not offered to an unaccepted stranger** | — | — | **New, in all three** | Placing a call makes this device gather ICE for whoever is on the other end. That is the same bar `comrade_core::handoff`'s accepted-conversation gate already holds an *incoming* call signal to (D34), for the same reason, so offering the outgoing half earlier is a button whose only outcomes are a leak or an error. A stranger's row is Message · Add contact · Block. |
| D38 | **Links are copied, not opened, and the host is the prominent field** | — | — | **New, in all three** | `extractLinks` returns `{url, host}` separately and the UI must render the host large: `https://evil.example/login?next=paypal.com` must not be presentable as a PayPal link. Only `http`/`https` survive — `javascript:`, `data:`, `file:` and scheme-relative `//host` are refused in the rule, not at the call site, because on the desktop an `href` is the one place `el()`'s `textContent` discipline protects nothing. Nothing auto-opens: fetching a sender-chosen URL leaks this device's IP and an implicit read receipt to whoever sent it. |
| D39 | **Shared media is a list, not a thumbnail grid** | — | — | **New, in all three** | A grid means downloading and decrypting every blob on the tab to draw it. Nobody asked a profile page to do that, the bubble in the thread already loads on demand, and on Android and Flutter the bytes would land in the bounded in-memory cache that D11 sized for a conversation, not for a back-catalogue. The row names kind, caption, size and date. |
| D40 | **Remote profile pictures load by default, for accepted contacts only** | avatars are generated initials (no image loader at all) | same, plus no per-peer avatar of any kind | **Fetch, guarded, with an off switch** | A Kind-0 `picture` is a URL the peer chose, so fetching it discloses the user's IP to a host they picked. Owner chose default-on over opt-in explicitly; the fetch is narrowed instead — accepted contacts or yourself, never a stranger, never a blocked peer — and `comrade_core::avatar` refuses non-HTTPS, private/link-local/CGNAT/unique-local addresses (via `url::Host`, which normalises `https://2130706433/` and `https://[::ffff:127.0.0.1]/` that a string check misses), `.local`/`.onion`/`.internal`, dotless hostnames and any URL carrying credentials. Bytes land in the *encrypted* store, content-addressed, never on the filesystem (AUDIT S-4). `set_remote_avatars_enabled(false)` stops all of it. **Android draws one since 2026-09-01** (`ui/ProfileScreen.kt`), which is also what made the off switch worth building: Settings' "Load profile pictures" is `set_remote_avatars_enabled`. **Not closed:** DNS rebinding, and Flutter draws no avatar because it has no profile page (D43); the desktop page has drawn one since `0492ad6`. |
| D41 | **Publishing your own picture is PNG-only** | — | — | **PNG in, JPEG/WebP still render on the way back** | An avatar goes out unencrypted and public, because a Kind-0 `picture` has to be readable by every client — there is nobody to share a key with. A JPEG off a camera roll carries EXIF that routinely includes GPS, so "set your picture" would quietly publish where the photo was taken, permanently. `strip_png_ancillary` keeps only IHDR/PLTE/tRNS/IDAT/IEND and is small, total and tested; an equivalent JPEG APPn stripper is a bigger piece of hostile-input parsing and is scoped out rather than half-done. `upload_public_blob` exists so the one public upload in the codebase is not spelled `upload_encrypted_blob` — a name asserting a guarantee the code does not provide is the bug, even when the bytes are identical. |
| D42 | **The bidi/control character class has one home per frontend** | `handoff.HandoffDecisions` strips C0 controls but **not** U+202E | `handoff_transfer.mjs` stripped both, privately | **Extracted: `display_text.{mjs,kt,dart}`** | A profile draws a peer's name and bio at heading size, which is the same problem a transfer card's filename has. Two maintained copies of a security-relevant character class eventually differ, and the difference is invisible until someone uses it. Desktop's copy moved and `handoff_transfer.mjs` now imports it — its 209 existing tests prove nothing broke. **Android's copy was deliberately left weaker**: `displayFileName` still misses the bidi overrides, and widening a shipped transfer path that cannot be compiled or tested in the cloud sandbox is a worse trade than naming the gap. Named here rather than quietly diverged. |
| D43 | **Where the link and media-bucket rules live** | `ui/ProfileView.kt` (`bucketMedia`, `mediaTabCounts`, `extractLinks`, `hostOf`, `collectLinks`) | `profile_view.mjs`, the original | **Kotlin and desktop have them; Dart does not yet** | The Android profile page needed all five, and writing them at the call site is how a security-relevant rule drifts — `extractLinks` decides what a link *is* and `hostOf` decides what a user is shown it points at, which is D38's whole subject. They are ported character for character from the desktop module and pinned by the same cases in `ProfileViewTest`. Dart is left short deliberately, not accidentally: `app/` has no profile page to consume them, nothing in this sandbox can run `flutter test`, and an unrun Dart port of a phishing-relevant parser is worse than a named gap. Porting them is the first step of the Flutter page, and this row is the note that says so. |

---

## 4. Architecture as built

```
lib/
  main.dart                     composition root — picks the repository
  src/
    app.dart                    theme + door/app phase switch
    data/       models · ComradeRepository (interface) · FakeComradeRepository
    state/      Riverpod: providers · chat · tara · content · settings · call
    theme/      comrade_theme.dart (Theme.kt + styles.css) · breakpoints.dart
    util/       display_name · chat_thread · tara_stream  ← pure, unit-tested
    widgets/    peer_avatar · message_bubble · media_attachment · app_chrome
    screens/    one file per surface
```

**The backend seam.** Every screen depends on `ComradeRepository`, never on the
generated bridge. `FakeComradeRepository` (in-memory, seeded with two days of
believable history) makes the whole app runnable and every screen widget-
testable with no native library — which the Compose screens never were, since
they called the `ComradeCore` singleton directly and so needed instrumented
tests. When `package:comrade/src/rust/api.dart` lands, add one adapter class
and change one override in `main.dart`.

**Responsive.** `Breakpoints` encodes the widths the existing CSS already uses
(840 fold, 1600 ultrawide, the `clamp()` measures for the sidebar, the
conversation list and the reading column). `ListDetailPane` is the single
primitive that makes Chats a pushed screen on a phone and a two-pane layout on
a desktop window — and it re-evaluates on `MediaQuery`, so dragging a window
across 840 px swaps the chrome live. Tested.

**Platform seams.** Three things pure Dart cannot do are declared as interfaces
with do-nothing defaults that say so out loud rather than failing silently:
`CallEngine` (media capture + video views), `MediaPlaybackDelegate` (audio/video
playback, open-externally), `AttachmentPicker`. A sibling workstream is building
the Kotlin side of these in `app/lib/src/platform/` + `app/android/.../channel/`;
wiring them together is a `main.dart` override each.

---

## 5. What is *not* done

Stated plainly, because a UI that looks finished and isn't is the expensive
kind of wrong.

1. **No real backend.** Everything runs against `FakeComradeRepository`. The
   Rust bridge adapter is deliberately not written (inventing the generated
   API's shape would create a third export surface the real codegen would then
   contradict — `docs/FRONTEND_STRATEGY.md` D5).
2. **No media engine.** `CallEngine`'s default does nothing. The call *UI* is
   complete and driveable; a call is not. This is D3 of the strategy document
   and it is the largest single unresolved item in the migration.
3. ~~**No audio/video playback, no file picker, no file open-out.**~~ **Done on
   Android** (`channel/MediaChannel.kt` + `platform/media_channel.dart`): the
   document picker, camera photo/video, in-memory audio playback, and
   open-in-another-app through an in-memory content provider. Still absent on
   desktop, where the seams keep their honest defaults — there is no in-app
   **video** player on any platform (video goes out to the system player), which
   is the one piece of this deliberately not built: it needs a PlatformView and a
   media engine of its own.
4. **No dictation, no wake word** (D21). Voice-note *recording* now exists
   (D13); dictation is the Vosk path, which stays native and unreachable.
5. **No UPI `/pay` composer preview** (D15).
6. **Preferences are in-memory.** Tara's opt-in and the background-connectivity
   toggle do not survive a relaunch yet — `AppPreferences` is the seam.
7. **Vault path is a constant.** Android resolved `filesDir/comrade-vault`,
   desktop `appDataDir()/comrade-vault`; that belongs in the bridge, not in the
   UI doing path arithmetic.
8. **Screenshot blocking (`FLAG_SECURE`) is reimplemented, with the default
   reversed.** It is an Android window flag and it lives natively
   (`channel/ScreenSecurityChannel.kt`), but **screenshots now work everywhere by
   default**. The Compose app blocked them for its entire window to protect key
   material no screen renders — an npub is public — while blocking every ordinary
   use (screenshot a plan, keep a journal entry, attach a picture to a bug
   report) and stopping no realistic threat, since a phone can be photographed.
   Blocking is now a Settings switch (off by default, shared with the Compose
   frontend through `ScreenSecurity`), plus a screen-scoped `SecureScreen` hold —
   used in one place, the passphrase field while it is revealed.
9. **Never run.** `flutter analyze --fatal-infos` is clean, 240 tests pass
   (4 skipped) in the Flutter test harness, and a debug APK builds with the
   media and screen-security Kotlin compiled into it. No device, no emulator, no
   desktop window, no golden tests, no real relay — so "it builds" is the whole
   of the claim: not one attachment has been picked, played, or opened out, and
   `FLAG_SECURE` has never been applied to a real window.
10. **Five Compose screens are missing from this document, not just from the
    app** — added 2026-08-04, when a parity count for
    `docs/FLUTTER_WEB_MIGRATION.md` found that §1's matrix and §2's
    screen-by-screen walk do not mention them at all. That is worse than a
    porting gap: the whole point of §1 was that nothing gets silently dropped,
    and these were.

    | Compose screen | LOC | In `app/` |
    |---|---:|---|
    | `ui/BreathingScreen.kt` (+ `BreathHaptics.kt`) | 423 + 162 | ✗ |
    | `ui/FocusScreen.kt` (+ `MirrorCard.kt`) | 393 + 265 | ✗ |
    | `ui/ComradesScreen.kt` | 300 | ✗ |
    | `ui/ReaderScreen.kt` | 228 | ✗ |
    | `ui/TogetherScreen.kt` | 134 | ✗ |
    | `ui/VoiceModelDownloadDialog.kt` | 119 | ✗ |

    Android has 16 top-level screen composables; `app/` has counterparts for 11.
    And they are **not** all view work, which was this document's first guess and
    is wrong. Measured per bridge:

    | Surface | uniffi (Android) | Tauri | FRB (Flutter) | Dart `ComradeRepository` |
    |---|---:|---:|---:|---:|
    | Attention (focus + rollups) | 11 | 11 | **0** | **0** |
    | Together | ✅ | ✅ | 6 | **0** |

    So `ComradesScreen` is view-only (`comrades()`, `peerPresence()`,
    `setComrade()` are already on the interface); `TogetherScreen` needs six
    repository methods over exports that exist; and **the whole Attention feature
    — Focus, Reader, Breathing, Mirror — is unreachable from Flutter at the ABI.**
    `active_focus_session`, `start_focus_session`, `finish_focus_session`,
    `focus_presets`, `focus_prompt`, `focus_reflection`, `focus_sessions`,
    `suggested_focus_minutes`, `attention_days`, `attention_summary` and
    `record_attention_day` are exported by *both* shipping bridges and by neither
    line of `crates/comrade_jni/src/api.rs`. That is the same hole as Sakha, four
    times the size, and it cannot start without a codegen run. Until it closes,
    "parity" is not the right word for where `app/` is.

    One threshold to move while porting: the feed's gentle stop lives only in
    `android/.../attention/ScrollSitting.kt` (`THRESHOLD_MS`, ten minutes), tested
    only in Kotlin and absent from `comrade_core`, `desktop/ui/` and
    `feed_screen.dart`. Port it into `comrade_core::attention` rather than
    restating the number in Dart — see `docs/FLUTTER_WEB_MIGRATION.md` Phase P2.

11. **The LOC figures in the header are stale.** It says 4,838 lines of Compose
    UI across 13 files, measured when the document was written. Today
    `android/.../ui/` plus `CallScreen.kt` is 10,400 lines, and `desktop/ui/` is
    11,283. Re-measure before quoting these; Appendix A of
    `docs/FLUTTER_WEB_MIGRATION.md` has the commands.

12. **No web target parity at all.** `app/web/` now exists and
    `flutter build web --release` has a CI lane, so the target *builds* — but a
    browser has no Comrade core (measured: `comrade_core` does not compile for
    `wasm32-unknown-unknown`), so it reaches `FakeComradeRepository` and nothing
    else. Signing in is a device-link handshake against a phone or laptop that
    holds the vault; the protocol is `comrade_core::link` and the transport is
    not built. See `docs/FLUTTER_WEB_MIGRATION.md` §§2, 4 and 5.
