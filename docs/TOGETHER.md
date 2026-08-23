# Together — watching and listening in step

_Added 2026-08-03, from `AUDIT.md` §8.2 (owner request, 2026-07-12)._

You and your person can start something at the same moment and stay there:
play, pause and seek reach the other side, and the two playheads are held
together well enough to laugh at the same joke.

Comrade moves **no media**. Each side plays *their own copy*; all that travels
is a small control envelope saying where the playhead is. This document is the
design record — the wire protocol, what the model can and cannot promise, and
what is deliberately not built.

---

## 1. Why nothing streams

§8.2 put the constraint plainly and it has not softened: re-streaming or
proxying licensed audio or video between two people is a copyright problem and
a bandwidth problem, and DRM'd platform content cannot be frame-synced inside
our app at all — the platforms both prohibit and technically block it.

So the feature is a *clock*, not a pipe. That also happens to be the version
that fits this app: it needs no new infrastructure, no server, and no
relationship with anybody's content.

## 2. The wire protocol

`comrade_core::together` — pure, framework-free, 40 unit tests. A seventh
control envelope on the convention documented in `comrade_core::dm`, riding the
same NIP-44/NIP-17 gift-wrapped DM channel as receipts, profile shares, call
signals, presence beacons and nudges:

```jsonc
{ "comrade_together": 1, "session_id": "9f3c…", "seq": 7,
  "at_ms": 1754160000123,
  "echo": { "your_at_ms": 1754159990031, "my_recv_ms": 1754159990402 },
  "signal": { "kind": "state", "pos_ms": 2520000, "playing": true } }
```

| Signal | Meaning |
|---|---|
| `start` | "Watch this with me, and here is where I am." The invitation and the opening position in one, so a joiner never joins to nowhere. **The only signal that can create a session.** |
| `join` | "I'm in." Carries no position — the joiner adopts the leader's. |
| `state` | Play, pause *and* seek. |
| `heartbeat` | Where I am now, and the last command I had applied when I said so. |
| `end` | "I'm leaving." Also the decline. |

**Play, pause and seek are one signal** because all three reduce to a position
and whether it is running. Which one it *was* is derived by one tested function
(`describe_state_change`) rather than by three frontends each guessing.

**What the envelope deliberately does not carry.** `end` has no reason —
bored, phone rang, app closed is not the other person's to learn, and "they
left" is the whole signal. And a local file is **never** identified by a
filename, a path, a size or a digest: a hash would fingerprint the exact
artefact someone holds and cost seconds of work for an answer we do not need,
and a filename carries release group, language and sometimes a path fragment
while not even answering the question (two files called `movie.mkv` are not the
same film).

**What it does carry: a recording.** `{kind:"local", duration_ms, recording?}`,
where a `recording` is `{isrc?, title, artist, album?}`, read from the file's own
**tags** and shown before it goes out, so naming the thing stays a deliberate
act.

The ISRC-first shape is adapted from [Antra](https://github.com/anandprtp/Antra),
which uses the International Standard Recording Code to guarantee exact-recording
matches with a scored title/artist fallback. (Only that idea — Antra is a
downloader, and acquiring content is not something this app does; see §9.) It is
a better answer than the hash on both axes at once:

- **More useful.** A hash answers "is this the same bytes", which is not the
  question — two people can be perfectly in step on different rips of the same
  recording. An ISRC answers "is this the same recording", which is. And because
  it names a recording rather than a file, the receiver can find their **own**
  copy instead of being sent hunting for one.
- **Less revealing.** A hash fingerprints which rip, which release group, which
  personal copy. An ISRC is public catalogue data about a commercial release and
  says nothing about the file on anyone's disk.

`duration_ms` stays: it is needed to clamp an incoming position anyway, and it is
what separates a radio edit from the album cut.

That claim is a test, not a comment:
`a_local_file_is_identified_by_its_length_and_what_the_sender_chose_to_say`
asserts the exact JSON key set at both levels, so adding a field to the envelope
fails the build.

**Matching** (`match_score`, pure and shared by both frontends): an ISRC
agreement is decisive, and an ISRC *disagreement* is equally decisive the other
way. Without one it is a weighted title/artist comparison with duration as a
tiebreak, using **containment rather than Jaccard** — a symmetric measure scores
"Teardrop" against "Teardrop (Remastered)" the same as a different song sharing
one word, because it charges the extra token to both sides. Words that mean a
different *take* (`live`, `remix`, `acoustic`, `instrumental`, `karaoke`,
`cover`, `demo`) are penalised heavily; "Remastered" or a year is not. A length
disagreeing by more than 15 s is a **veto**, not a deduction. The bar for opening
a file on someone's behalf is set high deliberately: opening the wrong one is
worse than asking.

**Links.** `parse_music_link` recognises Spotify, Apple Music and YouTube URLs
and reduces them to what they identify — offline, metadata-only, no account and
no audio. Whether a link can then be *played* is not a property of the link:
`playhead_control(&ServiceAccess)` answers it against what this device is signed
in to, because the same Spotify URL is a session on a phone with Premium behind
it and a signpost on one without. YouTube is `Full` either way, needing no
account at all. **§11 is the long version, and it is the section to read before
touching any of this** — the predicate it replaced (`playable_in_place`, a
property of the URL) encoded a conclusion that was wrong.

A YouTube id is the asymmetric case and is named rather than hidden: it *is*
fully disclosing, because it is publicly resolvable. It is also validated in
core on send **and** receive, because a peer-supplied string ends up in an
`<iframe src>` and no UI should have to remember that.

That check is now `TogetherContent::admissible`, which **matches exhaustively**
rather than testing one variant. It replaced two separate `if let … Youtube`
arms, one on send and one on receive, and the reason is §11a: a new variant
carrying a peer-chosen string could otherwise be added, wired through three
frontends, and reach a `src` attribute without either arm noticing.

## 3. The clock, and why "< 300 ms" is not a target we can hold

To compare two playheads you must know how far apart the two *clocks* are.
Nothing in the transport tells you: a gift wrap's outer timestamp is
deliberately randomised, and the rumor's own timestamp is the sender's claim,
in whole seconds — a full second of noise in a system trying to hold a fraction
of one.

So every heartbeat carries `echo`: the `at_ms` of the last message we heard and
our clock when we heard it. That turns a message we were already sending into
the classic four-timestamp NTP probe at **zero extra messages**.

```
offset_ms = ((t2 - t1) + (t3 - t4)) / 2      // their clock − ours
rtt_ms    = (t4 - t1) - (t3 - t2)            // their turnaround removed
```

`ClockFilter` keeps eight probes in a two-minute window and averages the offsets
of the **best half by round trip** — [beatsync](https://github.com/freeman-jiang/beatsync)'s
filter, and better than a bare minimum for the reason any average is: one lucky
sample stops deciding the answer alone. The *lowest* round trip still sets the
uncertainty, because that is a claim about the best evidence we have rather than
about the mean. A clock *step* on either device shows up as an outlier and ages
out, rather than poisoning the estimate forever.

Two deliberate differences from beatsync, which is the closest prior art and
solves a harder version of the same problem in a browser:

- **Each offset is de-skewed to a common instant before averaging.** Offsets
  taken minutes apart are not samples of one quantity once the two clocks run at
  different rates; averaging them raw would smear the frequency difference back
  into the phase. beatsync has no frequency term to conflict with, so it never
  hits this — here it would be a real error.
- **The probe rides the heartbeat.** beatsync bursts up to 40 dedicated
  measurements at join, against a server. Here there is no server, and every
  message is a persistent gift-wrapped event, so the four timestamps ride traffic
  that was going out anyway.

What beatsync gets right and this design had to adopt is the **burst**: at one
probe per ten-second heartbeat, the first minute of a session runs on a
deliberately pessimistic guess, so the two playheads would be at their furthest
apart exactly when both people are looking. A session now probes every 500 ms
until it has eight of them — about four seconds instead of eighty — and then
settles to the slow tail. Eight rather than forty, because each one here costs a
persistent event rather than a WebSocket frame, and because a paused session
bursts too: the clock has to be converged *before* anyone presses play.

Measuring the offset also measures how wrong it might be, and that gives the
rule the whole module is built around:

> **Never correct by less than your own measurement error.**

The deadband is `max(the player's floor, half the measured round trip)`. Half
the round trip of a public relay routinely exceeds 300 ms, so §8.2's "target
< 300 ms" sits *below the noise floor of the measurement*: we could not tell a
300 ms error from zero, let alone correct it. What this design does hold:

| | typical steady-state drift |
|---|---|
| local file (playback rate can be trimmed) | ±0.3–0.8 s |
| an embedded player that can only seek | ±1.2–2.5 s |
| over a future WebRTC data channel (§8.1) | ±20–60 ms plausible |

`ClockFilter` also tracks **frequency**, not just phase. NTP disciplines both,
and an offset alone is correct only at the instant it was measured — it then
decays at whatever the two crystals differ by. Regressing offset against time
gives that rate in ppm and carries it forward, which is what lets the heartbeat
be slow *and* the sync be tight instead of trading one for the other. It refuses
to guess from fewer than four probes or a baseline under 45 s (over ten seconds,
a millisecond of jitter reads as 100 ppm — five times any real crystal), and
clamps the result, because a clock being *stepped* is a phase event and
extrapolating it as a frequency would push the playhead forever.

The clock estimate is an **input** to `sync_verdict`, not baked into it, so a
lower-latency transport tightens all of this with no policy change and no new
tests.

### Timelines, not positions — the thing everyone else gets wrong

Syncplay and the browser watch-party services trade *positions* — "I am at
42:00" — and act on arrival. That is wrong by exactly the flight time, on every
command, and it is invisible because both sides agree on the number they
exchanged. It is also why none of them beat about a hundred milliseconds.

A position means nothing without the instant it was true at. So a `state` command
carries `effective_at_ms`, and the receiver **evaluates that timeline at its own
now** rather than adopting a stale number: a command that took 400 ms to arrive
lands 400 ms further along. On a transport fast enough to schedule slightly ahead
— the local mesh — the sender instead names an instant a few tens of milliseconds
out and *both* players change state on the same tick, which is how SMPTE and
AES67 do it. `command_apply` returns exactly those two cases and nothing else.

### What a browser cannot do, and what it does better

beatsync schedules through the Web Audio API's `start(when, offset)`, which is
**sample-accurate** — the browser hands the exact frame to the mixer. Android's
`MediaPlayer` has no equivalent: `seekTo` lands on a sample boundary but the
start instant is best-effort, which is a real advantage beatsync holds and an
argument for an `AudioTrack`-based path later.

What a native app can do instead is see the rest of the chain:

### Ear to ear, not decoder to decoder

What a listener hears is the decoder position minus that device's audio output
latency: 20–100 ms on Android, and different between handsets. Two players
agreeing perfectly on decoder position can still be a tenth of a second apart in
the room — the error no browser-based implementation can even see. Both sides
report theirs in the heartbeat, comparison is ear-to-ear, and a seek targets the
decoder position that lands *audibly* in step.

Stated honestly: on Android this figure is currently an **estimate** from the
device's low-latency buffer properties, not a measurement, because `MediaPlayer`
does not hand out its `AudioTrack`. Zero means unmeasured. A true measurement
needs an `AudioTrack` we own — a Media3 migration — and is the honest follow-up.

One more thing that decides what the UI may claim: **listening together is
perceptually much harder than watching together.** Two people in one room half
a second apart is unlistenable; two people in different cities half a second
apart are fine. The goal is reacting to the same moment, not phase-locked
audio.

## 4. Two rules that stop the devices arguing

A shared playhead with no server is a feedback loop waiting to happen. Two
rules close it, and both are tested.

**Only the follower corrects.** The person who sent `start` leads for the life
of the session — which costs zero wire bytes, since it is simply who invited
whom. Only the other side drift-corrects, so the loop provably cannot
oscillate. Both sides still *command* freely; only the automatic correction is
one-sided. The honest cost: a leader with a stuttering connection drags the
follower, bounded by the deadband and the seek cooldown. There is no
serverless fix for that.

**A peer still on an older command is ignored, not answered.** Their position
follows from state we have already superseded, so correcting toward it would
undo our own command. We hold, and our next heartbeat brings them up. This is
`docs/PRESENCE.md`'s `reply: false` rule in another shape, and it is why the
loop converges in one round.

Commands are ordered by a **Lamport counter**, deliberately not a timestamp: a
device whose clock runs a few seconds slow would lose every tie forever, and
its owner would simply experience a pause button that does not work. Ties break
on *pause beats play* — the person reaching for pause has a reason — and then
on the greater npub, which is arbitrary but symmetric and needs no round trip.
The test asserts both devices name the same winner whichever side is asked.

When a command DM is lost entirely, the heartbeat is the repair path: it
carries `applied_seq`, so a peer who is *ahead* of us is adopted wholesale
rather than corrected against.

**A trim is sticky, so arriving back inside the deadband has to say so.** This
is the third rule, and it was missing until a soak found it. A player told to
run at 0.96× keeps running at 0.96× until it is told otherwise — neither
`MediaPlayer` nor a `<video>` element resets itself — and `trim_rate` cannot
return `1.0` for any drift large enough to have provoked a correction. So a
verdict of `Hold` on the way back into the deadband left the trim applied: the
follower ran permanently at least 2.5% off speed, coasted out the far side of
the deadband, and was trimmed the other way. Bounded, never divergent, and
never settled either.

`sync_verdict` therefore returns `Nudge { rate: 1.0 }` rather than `Hold` when
a trim is applied and the gap has closed, which is what `SyncSample::local_rate`
is for. Over two simulated hours (`together_soak`) that is **4 rate changes
instead of 191, and 271 ms of worst-case drift instead of 479 ms**. Worth
naming because of *where* it would have been noticed: a ±4% wobble is invisible
on video and an audible tempo and pitch error on music, so "listen together"
would have been the broken half while "watch together" looked fine.

## 5. Timing

| Constant | Value | Why |
|---|---|---|
| `TOGETHER_BURST_INTERVAL_MS` | 500 ms, for the first 8 probes | The clock must be converged before anyone presses play — see §3. A paused session bursts too, and only the burst does. |
| `TOGETHER_HEARTBEAT_SECS` | 10 s, and **none while paused** | Not a steering knob. Two local players drift by crystal error — tens of ppm, well under a second across a whole film — so this exists to notice a stall or a lost command and to keep the clock filter supplied. |
| `TOGETHER_SESSION_TTL_SECS` | 45 s | More than four heartbeats, so a couple of dropped beacons cannot end a session someone is still watching. A phone that dies mid-film sends no goodbye; this is what that costs. |
| `TOGETHER_SIGNAL_MAX_AGE_SECS` | 60 s | Tighter than the call channel's 90 s, because a replayed call offer produces a ring a human can decline while a replayed playhead moves someone's player with no confirmation step. Wider than the TTL, so the TTL stays the single authority on when a session ends. |
| `TOGETHER_COMMAND_MIN_INTERVAL_MS` | 400 ms | A scrub drag emits ~10 positions a second; sending all of them would be a burst of gift wraps describing somewhere nobody stopped. |

**Transport.** Together signals prefer the **local mesh** when the Saathi engine
is running, falling back to a relay otherwise. This is the single biggest lever
on how tight the sync can be, because the deadband is floored by half the round
trip and a LAN hop is ~1–5 ms against a relay's hundreds. The honest limitation:
the mesh is only up in the off-grid workspace today. Starting it for a session is
engine-lifecycle work (AUDIT A1 / `docs/COMMS_ARCHITECTURE.md` ADR-4) and was
deliberately left out of this change.

**Why the cadence is a real decision, not a default.** Every heartbeat is a
persistent gift-wrapped event, and the vault inbox rewinds two days on *every*
launch (`GIFT_WRAP_TIMESTAMP_SKEW_SECS`). So the cadence decides how much each
later app start re-downloads and re-decrypts — that, not bandwidth, is the
binding cost, and it is why a two-second tick was rejected. **Whether public
relays tolerate even this cadence for two hours is untested**: the in-process
test relay accepts anything. If they push back, the answer is 15–20 s and a
wider deadband, not a cleverer algorithm.

## 5a. Which transport carries it, and why that is the real latency lever

Every constant in §5 is chosen against a round trip, and the deadband is floored
by half of the measured one (§3). So the transport does not merely *affect* how
tight the sync can be — it **sets the floor**, and no amount of tuning gets under
it. `send_together` therefore tries three rungs in order:

| Rung | Round trip | When it is there |
|---|---|---|
| Local mesh (Saathi, LAN) | ~1–5 ms | Only while the mesh engine runs, which today means the off-grid workspace |
| **Direct peer channel** | ~20–80 ms | When a frontend reports one up for the session |
| Relay (gift-wrapped DM) | hundreds of ms | Always |

The relay rung is not a fallback in the apologetic sense — it is the one that
always works, needs no NAT traversal, and is what makes a session possible
between two people who can never reach each other directly. The other two are
what make it *tight*.

**Core does not send on the direct channel; it asks the frontend to.** The
connection belongs to the frontend — the same division of labour the file
handover already uses, and for the same reason: mirroring a connection state
machine in core would create two machines that must agree about something only
one of them can see. So core emits `BridgeEvent::TogetherOutbound` and the
frontend puts it on the wire. One event for the whole transport, not one per
signal kind.

**The direct path is deliberately less privileged than the relay path**, and this
is the part worth reading twice. A relay message proves who sent it: every signal
is individually NIP-44 authenticated. A data channel proves only "whoever is on
the far end of this DTLS connection", which is the peer precisely because the
connection was negotiated with them inside a live session. Two rules keep that
honest:

- **It cannot open a session.** `direct_signal_admissible` refuses `start` — the
  only signal that creates state from nothing, and so the only one that really
  needs per-message authentication. A channel that could open a session would
  invert the thing that makes it trustworthy.
- **The sender is the session's peer by definition, not by claim.** Nothing in
  the payload is consulted for identity.

Everything past that — the age gate, session scoping, `(seq, actor)` ordering —
is literally the same code the relay path runs, because it is the same call.

**`direct_ready` is a claim with an expiry, not a fact.** A frontend that reports
a live channel and then loses it should report `false` — but the failure worth
designing for is the one where it *cannot*: a crashed webview, a killed process,
a close handler that never ran. Nothing arrives to say so, so signals would keep
going into a socket nobody reads until the session died on its 45 s TTL, which is
strictly worse than never having had the fast path.

So the claim is checked against evidence. `direct_path_live` reads the last
moment the channel gave any sign of life — the frontend declaring it up, or an
envelope arriving *on* it — and after `TOGETHER_DIRECT_SILENCE_MS` (two
heartbeats, 20 s) `send_together` goes back to the relay on its own. Three
properties make that the right shape rather than a timer bolted on:

- **Silence is real evidence, not pessimism.** A data channel is symmetric, so a
  peer whose end is open sends *their* heartbeats down it every 10 s. Two of
  those passing with nothing to show means the path is not carrying, whichever
  end broke.
- **It fires with a heartbeat to spare.** 20 s of watchdog plus one 10 s
  heartbeat is still under the 45 s TTL, so the relay it falls back to gets a
  beat through before the session it was meant to save would have lapsed. A unit
  test asserts that inequality rather than trusting the arithmetic to stay true.
- **It heals by itself.** This is a per-send question, not a latch: one envelope
  arriving on the channel earns the fast path back with no re-declaration. That
  matters because a frontend that never noticed the outage has nothing to
  re-declare. There is no deadlock in it either — the evidence is the peer's
  traffic, not ours, so nothing we stop sending can stop them sending.

Reporting `false` promptly is still worth doing. It moves the fallback from
twenty seconds away to immediate, which is the difference between two dropped
commands and none.

## 6. Replay safety

The worst bug this feature could have is a two-day-old "seek to 42:00" coming
out of the inbox backfill and yanking someone's playhead. It dies three times
over, and each guard is tested on its own:

| Guard | What it alone prevents |
|---|---|
| **Acceptance gate** (accepted conversations only, returning either way) | A stranger driving your player — and a control envelope surfacing as a message request full of JSON. |
| **Age gate** (60 s) | A backfilled `start` re-inviting you. The **only** guard protecting `start`, which is the sole signal that creates state from nothing. |
| **Session scoping** (memory-only, one at a time) | Every other signal, always: after a relaunch this device is in no session, so the entire backfill is inert. |
| **Lamport total order** | A redelivered command being applied twice — exactly and without bound, unlike an LRU. |
| **Invite seen-set** (64 entries) | The one hole the above leaves: a `start` for a session we ended forty seconds ago, redelivered inside the age window. |
| **Session TTL** | A session with a peer whose phone died staying live forever. |

**Heartbeats need no dedup at all** — a statement of state applied twice is the
same state — and commands need none either, because the counter comparison *is*
an exact dedup. That matters practically: at 10 s a two-hour film would
otherwise churn the 512-entry call-signal dedup set several times over and break
call dedup as a side effect. Together adds no pressure to any shared seen-set.

**Nothing is persisted, and that is load-bearing rather than tidy.** The session
lives in memory and `lock_vault` clears it next to the farewell beacons.
Persisting it would reopen the replay hole above: "after a relaunch there is no
session" is one of the three guards. A locked vault is also not watching a film
with anyone, and a command landing after the goodbye would say otherwise.

## 7. How it reads on screen

The failure mode of a sync-play UI is a green tick beside two players eleven
seconds apart, so the vocabulary refuses the words it cannot back:

| State | What it says |
|---|---|
| Invited, no answer yet | `waiting for them to open it` |
| Joined, our copy not open | `open your copy to start` |
| Joined, their copy not open | `waiting for them to open their copy` |
| Both playing, inside the deadband | `together` |
| Correcting | `catching up…` |
| Nothing heard for 90 s | `we've lost track of them` |
| They paused | `Ana paused` |
| Lengths disagree | `their copy runs 4 seconds longer than yours` |

Two rules, both borrowed from `docs/PRESENCE.md` §5 and both pinned by tests:
**never "in sync" and never "synced"** — we do not know that; and when the
heartbeats stop we say we lost track of *them*, because that is what we
observed. We did not observe them diverging.

And a permanent line under the stage:

> Positions travel over the relay, so you'll be within about a second of each
> other — not frame-perfect.

### The two measured numbers, and when they may be shown

Beside the state, both players now show what was actually measured: the gap
between the playheads, and the error on that gap. Three rules govern them, and
they hold on desktop (`player_view.mjs`) and Android
(`TogetherDecisions.measurement`) against the same vectors, gated by
`together_parity.test.mjs`.

**A gap smaller than our own error is not reported at all.** Printing "0.4 s
apart" while the measurement error is 0.8 s is invention dressed as precision.
Below `max(error, 400 ms)` the line is blank — and blank covers both "genuinely
together" and "cannot tell", which is why the error is shown beside it rather
than instead of it: `direct · ±0.05s` and `relayed · ±0.6s` are what make the
number above them mean anything. Neither is colour-coded. "We've lost track of
them" is an honest report of poor measurement, not a fault, and red would say
otherwise.

**A direct path never claims to be tighter than 50 ms**, because neither player
can report a playhead more precisely than that (§3), and a relayed figure gets
one decimal place rather than two — the second digit is not real.

**And both age out together after two heartbeats.** This is the rule that was
missing when the readout first shipped, and it is not a detail: corrections
cross the bridge *only* when the verdict is not `hold`, so the steady state
emits nothing at all. A screen that simply keeps the last pair therefore prints
a gap that was closed minutes ago, underneath the word "together", for the rest
of the session. Twenty seconds — two heartbeats, so at least one verdict has
since said the gap was inside the deadband — and the lines go blank. A session
that has never been corrected shows blanks for the same reason, rather than a
reassuring pair of zeroes.

## 8. Where the code lives

| Layer | What it owns |
|---|---|
| `comrade_core::together` | Wire protocol, the clock filter and its NTP arithmetic, `sync_verdict`, the Lamport order, every timing constant and its compile-time invariants. Pure; 40 unit tests. |
| `comrade_ui::runtime` | `together_start` / `together_join` / `together_set_state` / `together_end` (each a `RuntimeHandles` twin, so no bridge holds the lock across a relay round trip), `together_report_position`, `together_session`, the receive arm in `dispatch_incoming_dm`, the session loop, and seven `BridgeEvent` variants. |
| `comrade_jni`, `desktop/src-tauri` | The same calls over uniffi / flutter_rust_bridge / Tauri commands. `together_report_position` is the one that is **synchronous and skipped under contention**, because a player calls it several times a second from its UI thread — the trade `note_draft` already makes. |
| `desktop/ui/together_sync.mjs` | Echo suppression, the verdict→player plan, and the status wording. Pure, 20 `node --test` cases. |
| `android/…/together/LibraryResolver.kt` | Finds the listener's own copy via `MediaStore`, scored by the shared `match_score`; reads a picked file's tags so an invitation can name what it is. |
| `android/…/together/MusicLibrary.kt` | **Lists** the phone's music for the Together tab to browse, with covers. Its sibling above **searches** for one named recording — the same provider, two different queries, and folding them together would mean one of the two doing the other's badly (§16). |
| `android/…/together/` | `TogetherDecisions` (pure: echo ledger, scrubber rules, the two `MediaPlayer` footguns, and since §16 the transport clock, the library filter, the source list and both orderings — 75 JVM tests, the echo/verdict half mirroring the desktop vectors), `TogetherPlayer` (`MediaPlayer` + `SEEK_CLOSEST`), `TogetherManager` (session, audio focus, service control), `TogetherService` (foreground `mediaPlayback` + framework `MediaSession`). |

Tests worth knowing about: `crates/comrade_ui/tests/two_peer_integration.rs`
drives two real runtimes over one in-process relay and proves both halves —
a full invite → join → pause → leave exchange, and that **a stranger cannot open
a session on someone else's device**. In `runtime.rs`, `a_steady_heartbeat_produces_no_bus_traffic`
pins the claim that a ten-second heartbeat is not a periodic producer on the
critical event bus: the runtime emits only when the verdict is not "hold", so a
session in step says nothing at all.

## 9. What is built, and what is not

Built and tested: the protocol and all of its arithmetic, the view-model, both
FFI bridges, the Tauri commands, the desktop decision module, and the **Android
frontend end to end** — player, screen, entry point in the conversation bar, and
a foreground service so a session keeps playing when the app is backgrounded.

Read "tested" narrowly, because it has already been read too widely once. Every
lane in this repo asserts about *values*; not one of them looks at a pixel. Two
bugs shipped through a green board on exactly that gap — a film that played as
sound because nothing gave the decoder a surface, and a session that drew as
floating text over whatever tab was behind it because the overlay had no
background — and both were obvious within a second of opening the app on a
device. What CI can hold is the decision underneath a rendering bug:
`pictureOf`, `aspectRatioOf` and `keepScreenOn` are pinned on both frontends
against the same vectors. Whether anything reached the screen is still a human
with a phone.

Android specifics worth knowing:

- **`MediaPlayer`, not Media3.** The deciding detail is `seekTo(long,
  SEEK_CLOSEST)`, which needs API 26 and `minSdk` is 26. The plain `seekTo(int)`
  lands on the nearest sync frame — with 5–10 s keyframe spacing that is a sync
  failure dressed up as a working feature. No new dependency.
- **The picture needs a surface, and it is not optional.** A `MediaPlayer` with
  no surface decodes video and discards it, so a film plays as sound with no
  error anywhere — which is exactly how this shipped and how it was reported.
  `TogetherPlayer.attachSurface` and `VideoSurface` in `TogetherScreen.kt` are
  the fix. The surface and the player have **independent lifetimes**: the
  surface is destroyed and recreated on every rotation while the session must
  survive both, so the player holds the last surface it was handed and
  re-attaches on `open`, and the holder callbacks are the only thing that
  decides what exists. Detaching passes `null` before the player is released,
  because a destroyed `Surface` the decoder still holds is a use-after-free in
  the media server rather than a leak.
- **Audio-only draws no surface at all.** `TogetherDecisions.pictureOf` reads
  the decoder's reported dimensions — `0` means no video track — because the
  picked MIME type cannot answer it: an `.mkv` of an album is
  `video/x-matroska` and a `.mp4` podcast is `video/mp4`. Desktop makes the
  same call in `together_sync.mjs` from `videoWidth`, against the same test
  vectors, so a `<video>` element does not show a black rectangle over
  someone's music either. Only a *playing* video holds the screen awake; two
  hours of audio must not.
- **Background playback is real**, via `FOREGROUND_SERVICE_MEDIA_PLAYBACK` and a
  **framework** `MediaSession` (`android.media.session`, API 21) rather than
  `androidx.media3.session` — same media-key routing, no ~2 MB dependency for
  adaptive streaming this feature does not use.
- **Audio focus is honoured**: losing it pauses *and tells the peer*, so what
  they see is "they paused" rather than an unexplained drift.
- **The measured drift and its error are on screen** as of 2026-08-08, in the
  same words desktop uses and under the same three rules (§7). `UiState.Live`
  carries the raw pair plus when it was taken, because whether any of it may be
  shown depends on how old it is at the moment the screen draws — so the
  decision is recomputed each recomposition rather than stored.

**Not built**, and stated here rather than discovered:

- **The transports for sharing the file** — see below.

- ~~**The desktop player surface.**~~ Built 2026-08-05. The `<video>` element,
  the file picker and the DOM wiring all landed, and with them `/play` — see §9b.
- **YouTube.** The envelope and the id validation support it; no frontend embeds
  one. When it lands on desktop it must be a **bare cross-origin iframe driven by
  `postMessage`**, with the CSP widened by exactly `frame-src
  https://www.youtube-nocookie.com` — and **not** by loading YouTube's
  `iframe_api`, which would need `script-src` and put Google-controlled
  JavaScript inside our own origin, the origin where `withGlobalTauri` exposes
  every registered Tauri command. That distinction is the one line to hold.

  It also needs saying plainly what a YouTube session costs, because it is the
  first time this app would contact a third party during ordinary use: both
  peers' devices reach Google, which learns each IP, the video, and the watch
  timeline — and because sync-play *works*, two IPs hit the same video and pause
  at the same moments. That correlation is the real cost, not the cookie, and it
  is unavoidable by construction. `youtube-nocookie.com` avoids ad cookies and
  watch history; it does not prevent the IP-level record. It should ship off by
  default, behind one disclosure that says this.
- **A measured output latency on Android** — see §3; today it is an estimate.
- **Auto-starting the mesh for a session**, so the millisecond tier is available
  outside the off-grid workspace — see §5.

**On acquiring content — deliberately not built.** Antra's resolution chain (its
own mirror servers, then Tidal / Qobuz / Amazon / Deezer / Apple Music adapters,
then Soulseek) is a downloader, and none of it is adopted. Tidal, Qobuz and Apple
Music do not serve unencrypted audio to third-party clients, so obtaining it
means defeating a technological protection measure — a separate liability from
infringement (DMCA §1201, EU InfoSoc Art. 6, India's Copyright Act §65A) — and §1
already rules the whole area out. What *is* adopted is the identity half: a link
resolves to a recording, and the recording is looked for in the library already
on the listener's device.

That lookup needs a permission, and until 2026-08-05 the app did not ask for one,
so it never matched anything — `AUDIT.md` Q15. Android now declares
`READ_MEDIA_AUDIO` and asks at the moment a song is actually named, at most once,
and only when a local copy would change the answer (`MediaLibraryAccess`). A
refusal costs the automatic match and nothing else: the file picker needs no
permission at all, so every route below still works, and the composer says
Comrade was not allowed to look rather than that the track is absent.

**On "nanosecond" sync**, since it was asked for directly: it is not reachable by
any software path on two phones, and the reason is three independent floors, each
orders of magnitude above it. The transport (a relay is hundreds of milliseconds;
even the LAN mesh is ~1–5 ms; PTP reaches tens of nanoseconds only with
hardware-timestamping NICs Android does not expose). The player (Android audio
output latency is 20–100 ms, and `AudioTrack.getTimestamp` is accurate to about a
millisecond — you cannot place a playhead more precisely than the player can
report or act on it). And perception (comb filtering becomes audible around
5–30 ms; lip-sync tolerance is ±22 ms). What this design does reach — roughly a
millisecond on a shared network — is below every one of those thresholds, which
is the point at which there is nothing left to win.

## 9a. When only one of you has it

The feature's original shape assumed both people already had the file, which is
often false. Two answers, and they cover different situations:

**Play it from somewhere you do have.** The invitation names a *recording*
(§2), so a device with no local copy can offer the same recording from a source
it can reach. Through the YouTube embed we control the playhead, so the sync is
exactly as tight as it would have been; a deep link into someone's own streaming
subscription degrades to "we started together", because no app lets us drive
another app's playhead. Nothing is transferred, it works between cities rather
than only on one network, and it costs no bandwidth. **This should never happen
silently** — a different master, an ad break, or a different mix is not what the
other person is hearing, and switching source without saying so would be the app
claiming a thing it cannot see.

**Or send it.** `comrade_core::share` is the protocol: chunked, receiver-driven,
resumable, and playable before it has finished arriving.

Receiver-driven is the load-bearing choice. Because the receiver asks for ranges
rather than the sender pushing them, **resume** costs nothing (ask again for
what is missing), **seek** costs nothing (ask for the chunks under the new
playhead first), and the sender holds no per-receiver state, so a dropped
connection leaves nothing to clean up. Requests anchor at the playhead and only
fall back to the earliest gap once the tail is complete — so seeking forward
costs one request, and the session still ends up with a whole file rather than
one with a hole in the middle.

Playable-early is the other half: playback waits for a few seconds of
*contiguous* runway rather than the first chunk (which would stutter a moment
later) or the whole file (which would be minutes for a film). A gap ends the
runway however much lies beyond it, because audio after a hole is not runway —
it is a stutter waiting to happen. The exception is the tail: the last two
seconds of a track are playable even though two seconds is under the threshold,
because nothing more is coming.

**No server, and that is the difference from beatsync.** beatsync has every
client upload to a room on its own backend, which then serves everyone. Comrade
has no backend and should not grow one for this: a server holding and
redistributing copies is both an architectural reversal and the most exposed
possible version of the copyright question. §8.2 rules out *proxying* media
between users; one person handing something to one other person, both already in
an end-to-end conversation, is the existing encrypted-attachment path in a
different shape.

**The transport is WebRTC.** None of the three already in the app can carry
bulk: relay DMs are gift-wrapped control traffic, the media pipeline caps at
10 MiB *and* uploads to a third-party host (exactly the intermediary this must
avoid), and Saathi is gossipsub — a 16 KiB frame broadcast to every peer on the
network, both too small and far too public. A data channel is already
peer-to-peer, already encrypted (DTLS/SCTP), already solves NAT traversal, and is
already a dependency on both frontends. A second bulk protocol of our own over
libp2p would duplicate all of that and still reach nobody outside the LAN.

### 9b. The relay rule, and why it is the centre of this design

`AUDIT.md` §8.1 measures the problem: STUN alone finds a direct path for perhaps
**60–70%** of real-world pairs, and the rest — CGNAT, very common on Indian
mobile carriers — need TURN. That relay is *our own* server (`deploy/coturn/`),
and pushing a film through it would mean paying for every byte twice, putting the
operator's machine in the path of content it has no business carrying, and doing
the exact thing §8.2 calls proxying media between users.

So **the default is direct-only**, and it is enforced twice:

1. **Structurally.** A transfer connection built under `RelayPolicy::DirectOnly`
   is given **no TURN servers at all** (`ice_servers_allowed`,
   `iceServersFor`). A relay candidate is never gathered, so the rule holds even
   if every later check were deleted. An ICE server entry that mixes STUN and
   TURN urls is dropped whole, so one `turn:` url cannot ride in on a `stun:`
   entry.
2. **After connection.** The selected candidate pair is read from
   `getStats()` and classified (`IcePathKind::classify`). **Either end being a
   relay makes the path relayed** — a pair is direct only if both halves are,
   because a remote relay candidate means our packets reach the peer by way of
   *their* TURN server. A pair that cannot be read is `Unknown`, which is
   **refused, never assumed direct**.

The policy is a value, not a branch: `DirectOnly` · `UnderBytes { limit }` ·
`AskEachTime` · `Always`. The transfer logic never learns which is in force — it
asks `decide(path, bytes, policy)` and does what it is told, which is what lets
the rule change without touching the code that moves bytes.

**What direct-only costs, plainly:** roughly a third of remote pairs will get no
direct path, and for them the transfer simply does not happen. The honest answer
for those pairs is §9a's substitute source — play the same recording from
somewhere each side already has — not a quiet fallback onto the operator's
bandwidth. A refusal says which of the three reasons it was, so the UI can be
specific rather than saying "failed".

### 9c. Flow control, and not degrading a call

**`bufferedAmount` is not optional.** A data channel accepts writes long after it
has stopped sending them; the bytes queue in the SCTP buffer. A naive
`for (chunk of file) send(chunk)` therefore queues a 2 GB film in memory in
milliseconds and either stalls the connection or gets the process killed — and it
*looks fine* on a 50 MB test file, which is how that bug reaches production. So
the pump fills to a 1 MiB high-water mark, stops, and waits for
`bufferedamountlow`. The threshold is set at 256 KiB rather than just under the
ceiling, because waking on every few drained bytes is an event per chunk, which
is the busy loop the threshold exists to prevent. The window is re-checked
*inside* each batch too, since `bufferedAmount` moves as we write and a batch
sized against a stale reading is how the ceiling gets overshot on a slow link.

Chunks are **16 KiB**, not 64: 64 KiB sits at the practical ceiling for a
reliable data channel and is refused outright by some implementations, and the
throughput difference is noise next to the window above.

**A transfer gets its own `RTCPeerConnection`.** Sharing the call's would put
bulk and live media under one congestion controller and one SCTP association,
where a 2 GB push and a voice stream compete and the voice loses. Separate
connections cost one extra ICE negotiation and buy complete isolation — a call
cannot be degraded by a transfer it knows nothing about. It is also what makes
the relay rule enforceable, since the transfer connection has its own ICE server
list: the *call* keeps its TURN fallback, because a relayed call is a few tens of
kilobits and entirely reasonable, while a relayed film is not.

### 9d. How the handover is negotiated

The pump and the policy are attached to a real `RTCPeerConnection` on both
frontends. What connects them is four signals, carried **inside the session
envelope** rather than under a marker of their own:

| Signal | Direction | Means |
| --- | --- | --- |
| `ask` | receiver → sender | "My copy of this is missing." |
| `offer` | sender → receiver | Size, hash and duration — before a single byte. |
| `accept` | receiver → sender | "Go ahead." Negotiation starts here. |
| `transport` | either | One step of the WebRTC negotiation. |
| `refuse` | either | Not happening, and why. |

Riding inside `TogetherSignal::Share` is the whole safety argument. Every guard
the session already has applies unchanged: the acceptance gate, the
sixty-second age gate, the session-id scoping, and the fact that sessions do
not survive a restart. A separate envelope would have needed its own copy of
all four, and **a stranger able to open a peer-to-peer connection to you by
sending one DM is a much worse bug than a stranger able to move your playhead.**
`a_transfer_cannot_be_negotiated_without_a_session_to_negotiate_it_in`
(`crates/comrade_ui/src/runtime.rs`) is that claim as a test.

A share signal is deliberately **not** a command
(`TogetherSignal::is_command`). A transfer trickles ICE candidates at its own
pace; if each counted as a command, a burst of them would outrank the pause
button and the person pressing it would watch it do nothing.

**Four steps rather than two,** because the two obvious shortcuts are both
wrong. Skipping `ask` means the side that *has* the file must guess whether the
other needs it — guess wrong and it is either an unwanted upload prompt or a
session that silently never starts. Skipping `offer` means the receiver learns
the size after the transfer rather than before it, which is exactly backwards
for the one decision they might want to make.

**`refuse` carries a reason and `end` does not**, and that asymmetry is
deliberate. The argument that keeps a reason off `end` is that why someone left
is nobody's business. The reason a transfer did not happen is a fact about the
network, not about the person, and it is the only thing that tells them whether
trying again could work.

**The runtime keeps no transfer state.** It relays signals and answers the
policy question; the peer connection, the data channel and the bytes live in
the frontend, because that is where WebRTC lives. Mirroring the negotiation in
the runtime as well would create two state machines that have to agree about a
connection only one of them can see — the shape of both call bugs this repo has
already fixed.

**Chunks carry their own index** (a four-byte big-endian header). A data
channel is ordered but a *transfer* is not: a receiver that seeks re-asks from
a new anchor while chunks from the old one are still in flight, so "the next
message is the next chunk I asked for" is false exactly when it matters. The
index and the payload length are both checked against the offer on arrival —
the whole-file hash would catch the same corruption, but only after the whole
file.

**The relay question, and who is allowed to answer it.** Under
`AskEachTime` the transfer stops and asks, naming the size; nothing moves and no
refusal is sent until someone answers, because neither has been decided. The
answer goes back to core as an argument rather than being acted on locally, and
core can only ever use it to turn `needs_consent` into `allow`. That asymmetry
is the whole design: `consent_granted` arrives from the least trustworthy caller
the policy has, and a frontend that passed `true` unconditionally — a bug, or a
dismiss wired to the wrong branch — must not be able to defeat `DirectOnly`. The
worst it can do is skip a question.

Consent is per-transfer and is never remembered. A yes that outlived its session
would be a yes to a file nobody was asked about.

**The policy is a stored preference**, inside the vault, seeded into the
in-memory cell on unlock — the cell exists because the WebRTC callbacks read it
and must not touch storage. A stored value this build does not recognise reads
as `DirectOnly`: the only safe reading of "I do not know what this device agreed
to" is the one that carries nobody's bytes.

**Verified, and not.** The framing, the tracker, the policy and the pump are
tested on all three sides — `comrade_core::share` (Rust), `share_transfer.mjs`
(desktop) and `ShareDecisions.kt` (JVM), with the Rust vectors ported verbatim
into both ports so a divergence is a red test rather than a corrupted file.
The consent path adds four Rust cases, including the one that matters — a
frontend claiming consent cannot move a refusal — and the round trip that proves
a chosen policy survives a restart. What has **not** happened is a run between
two real devices: no transfer has
crossed a live `RTCPeerConnection`, and the Kotlin path additionally cannot be
compiled in the development sandbox at all. Treat the connection handling as
reviewed rather than exercised.

## 9c. Where it lives

Together has the bottom-nav slot Feed had on Android, and the sidebar slot Sabha
had on desktop. Neither feed was removed — Feed is a pushed screen from the
drawer (`drawer-feed`, asserted on a device, because "off the nav, not removed"
is only true if something reaches it) and Sabha is a button in the desktop
sidebar's Modes section. The argument for the swap is that a public feed is
somewhere you *go*, which is what a drawer is for, while listening with someone
is a daily surface.

The tab is the **session's own screen**, which is a change on Android beyond the
nav: a live session used to be an overlay covering the whole app, so an album
running for an hour meant an hour of not being able to read anything else
without ending it. Now only an *invitation* covers the app — those expire, and a
missed one is a missed evening — and the playing itself stays in its tab.

**Music-first, one block.** A square sleeve with a note in it, and the video
surface inside that same block when the decoder reports a picture. So an album
gets a record cover and a film gets a screen, from one layout rather than two
kept in step, and the sleeve is the single owner of the aspect ratio (two things
applying one is how a film ends up letterboxed inside a box already the right
shape).

**The readout is measured, not predicted.** Desktop shows the drift and the
measurement quality off `TogetherCorrection`, and `player_view.mjs` decides what
that is worth saying: a gap smaller than our own error is **not reported at
all**, because printing "0.4 s apart" while the error is 0.8 s is invention
rather than precision. The path is named beside it — `direct · ±0.05s` against
`relayed · ±0.6s` — since that is what makes the number mean anything. Neither
chip is colour-coded: "we've lost track of them" is an honest report, not a
fault, and painting it red would say otherwise. Android does not show these two
figures yet; the plumbing to carry them into `UiState.Live` is the follow-up.

## 9b. Starting one — `/play`, and why it is one gesture

> **Superseded in part by §16 (2026-08-08).** The chat header's ▶ described below
> is gone, and the Together tab is now the deliberate way in; `/play` and the
> route table are unchanged and still current.

The protocol was finished long before the way in was. Getting a session going
meant *finding* the feature — a panel on desktop, a button in the chat header on
the phone — then choosing a file, then saying start. Three deliberate acts to
express one intention, and on desktop `/play` answered **"there is no player
here yet"**, which had quietly stopped being true.

So the way in is the command, in the conversation with the person you want to
listen with:

```
/play kun faya kun
/play https://open.spotify.com/track/…
```

`play_query` resolves the words or the link, `play_route` decides what is
possible, and each frontend decides what *it* does about that — separately, and
on purpose, because they can do different things:

| Route | Phone | Desktop |
|---|---|---|
| `start_together` | Found it in the music library — session opens | Not reachable: a webview has no library to search |
| `ask_for_file` | Opens the file picker, then starts | Opens the file picker, then starts |
| `open_elsewhere` | Names the service — DRM audio no third-party client may decode | Same |
| `play_embed` | "Comrade can't play YouTube here" | "…on the phone app, not this window yet" |
| `nothing` | Asks for a name or a link | Same |

Three things about that table are the substance rather than the layout.

**The picker is an answer, not an error before one.** `ask_for_file` is the
common route — desktop can never search, and a phone often has no copy — so it
opens the picker itself and starts the session on the file that comes back.
Naming a screen to go and open was asking someone to say the same thing twice.
On the phone it opens whether or not the music library could be read: "no copy
here" and "not allowed to look" are different sentences and the same next
action, and the picker needs no permission either way.

**The invitation says what it is.** The recording `/play` named travels with it,
so the other person sees *Kun Faya Kun* rather than a blank — desktop used to
send `recording: null` whatever had been typed. The **filename is still never
sent**; on the phone the tags are read from the file itself, and a file picked
by hand from the panel names nothing, because nothing was said about it.

**Every route ends somewhere.** A command that silently does nothing is the
failure this replaces, so `planPlay` returns an outcome for all five routes and
for one it has never heard of — release skew, when core gains a variant before
a frontend does. `play_flow.test.mjs` asserts that as a property over the whole
set rather than case by case, and asserts separately that no refusal ever
implies we are about to play something we cannot.

## 10. Deliberately out of scope

- **Group watch.** Two-party is what makes the arbitration analysis tractable
  and provable. N-way is a different problem, and nobody asked for it.
- **Streaming anything between peers.** §1. This is the constraint the whole
  design exists inside, not a limitation to be engineered around later — and
  note that §11 does not soften it. A service session moves no audio between
  peers either; each side streams from the vendor on its own subscription,
  which is the same shape as each side opening its own file.
- **Resuming a session across a restart.** A playhead is a claim about right
  now. "Pick up where we left off" is a media-player feature, and this app does
  not own the player — and persisting a session would reopen §6's replay hole.
- **Reporting buffering.** A stall signalled as a remote pause is the worst
  ping-pong available here: one side stalls, pauses the other, and that pause
  makes the first re-evaluate. A stall is ridden out locally and the next drift
  verdict closes the gap. This will occasionally look worse than it could; it is
  much better than the alternative.

## 11. Online tracks — what Spotify Jam actually does, and what we can copy

_Added 2026-08-08, after the owner asked for "something like Spotify Jam, should
work with online tracks as well"._

### The thing worth knowing first

**Spotify Jam does not share audio.** Every participant's own Spotify client
streams the track from Spotify's servers on their own subscription; what travels
between them is a queue and playback events carrying track-millisecond
timestamps, over Spotify's own infrastructure, with clock alignment on top.
It is a shared remote control, not a shared pipe.

That is worth sitting with, because it means **§1 was never the thing standing
between us and a Jam**. The clock-not-a-pipe model in this document *is* the Jam
model. Sync was never the hard part for anyone; Spotify's advantage is that it
owns the catalogue, so "the same track" resolves trivially on both ends and both
ends are already licensed to play it.

So the gap is not the engine. It is: **can each device independently play the
thing the invitation names?**

### How the third-party ones close that gap

JQBX and Vertigo are Jam-alikes built by people who own no catalogue at all, and
they close it the only way available: each participant connects **their own**
account, and the app drives **their own** client.

- **Spotify, in a browser** — the Web Playback SDK is a player you instantiate in
  a page; it exposes `seek(position_ms)` and a `player_state_changed` event.
  Premium-only, per participant.
- **Spotify, on Android** — the App Remote SDK connects to the *installed
  Spotify app* and drives it: `PlayerApi.seekTo`, `PlayerState.playbackPosition`.
  Premium-only again.
- **Apple Music** — Vertigo offers it, and it is a weaker deal (below).

None of this is a hack around DRM and none of it decodes anyone's bytes. Driving
playback inside a vendor's own client is exactly what these SDKs are *for*.

**This repo had that wrong, and the correction is the substance of this
section.** `MusicLink::playable_in_place` answered "only YouTube", on the
reasoning that Spotify and Apple Music serve DRM audio no third-party client may
decode. The decode half is true and unchanged. What does not follow is that we
cannot *drive* them — and because the old predicate was a property of the *link*,
it could not express the thing that actually decides the answer, which is what
the **device** is signed in to. The same Spotify URL is a full session on a phone
with Premium behind it and a signpost on one without.

### What replaced it

`MusicLink::playhead_control(&ServiceAccess) -> PlayheadControl`, and three
values rather than two, because the middle one is real:

| | Signed in | Not signed in |
|---|---|---|
| YouTube | `Full` (embed needs no account at all) | `Full` |
| Spotify | `Full` — seek and position both reach the player | `None` |
| Apple Music | `StartOnly` | `None` |

`Full` runs the drift ladder. `StartOnly` means it can be started and never
*placed*: the two devices agree on "now" and then drift with nothing able to pull
them back — which is what §9a already calls the honest degradation of a deep
link. `None` means the honest offer is a link to open, not a player.

**Apple Music is never `Full`, signed in or not**, and that is a finding rather
than caution: MusicKit exposes no precise scheduling, so there is no call that
places a playhead at a named moment. Its terms also restrict synchronising
MusicKit content with other content, which at minimum wants a legal read before
anyone ships it. `PlayheadControl::corrects()` is what keeps that from becoming a
UI bug — a ladder running against a player that cannot seek emits verdicts
nothing applies and a screen that says "catching up…" while nothing catches up.
For now `play_route` sends `StartOnly` to `OpenElsewhere`: opening a session that
cannot be held is a bigger promise than we can keep.

**Premium, not sign-in, is the real gate on Spotify.** Both SDKs refuse to play
on a free account, so a frontend that reports `spotify: true` for a free account
opens a session and leaves the other person waiting for a track that will never
start. `ServiceAccess` says this on itself.

### What travels, and what it costs

`TogetherContent::Service { link, recording }` — a public catalogue id and,
optionally, what it is. Fully disclosing, exactly like the YouTube variant and
for the same reason: the id is publicly resolvable, so the invitation says
precisely what is being played. `recording` rides along so a device that *cannot*
reach the service can fall back to looking for its own copy (§9a) rather than
failing.

The privacy cost is the one §9's YouTube note already states, and it applies
here identically and must be disclosed the same way: **both devices contact the
vendor**, who learns each IP, the track, and — because sync-play works — that two
IPs paused at the same moments. That correlation is unavoidable by construction.
It is the first time this app would contact a third party during ordinary
listening, so it ships **off by default, behind one disclosure that says this**.

### The session syncs coarsely, and why

A `Service` session takes the same `SyncTuning` as a YouTube embed: no rate trim
(not expressible through either SDK) and the coarse deadband. The deciding
detail is that **`PlayerState.playbackPosition` is not continuously updated** —
the Android SDK reports it on state changes, not on a tick (spotify/android-sdk
issue 143), so a follower's position has to be interpolated between events. A
deadband tuned for an HTML5 element on a local file would thrash against that
reporting granularity rather than against any real drift.

### What is built, and what is not

**Built and tested here**: the whole model above — `ServiceAccess`,
`PlayheadControl`, `MusicLink::playhead_control`, `TogetherContent::Service` and
its tuning, `play_route`'s new `PlayOnService` branch — plus the FFI surface for
all of it (uniffi and a regenerated frb bridge) and the three frontends' routing
and wording brought in line.

**Not built**: the actual account connection. No frontend authenticates to
anything, so every frontend passes `ServiceAccess::none()` and behaviour is
byte-for-byte what it was — a Spotify link still routes to "open it there", it
just now says *"no Spotify account connected here"* instead of blaming DRM,
because that is the true reason and it is a fixable one.

What each frontend needs next, in the order that makes them useful:

1. **Desktop** — OAuth (PKCE, no client secret in a shipped binary), the Web
   Playback SDK in the webview, `ServiceAccess { spotify: true }` once a Premium
   account is live. The CSP consequence needs the same care §9 demands of
   YouTube: the SDK is Spotify-hosted JavaScript, so this is a `script-src`
   widening in the origin where `withGlobalTauri` exposes every registered
   command — the exact thing the YouTube note refuses. **It must run in a
   sandboxed child frame with its own origin**, not in the main window.
2. **Android** — App Remote against the installed Spotify app, which needs no
   webview and no CSP argument at all. This is the cheaper and safer of the two,
   and it should land first.
3. **Both** — the token lifecycle. A token expires mid-album and a subscription
   can be downgraded, so `ServiceAccess` has to be able to go back to `false`
   during a session, and the session has to survive it becoming a signpost.

## 11a. The online source nobody asked about, which syncs best of all

_Added 2026-08-08, after "how do apps like BlackHole and Echo work, where do
they stream media from?"_

**How that category actually works**, because it is worth being precise rather
than vague. BlackHole took audio from JioSaavn's undocumented endpoints and from
YouTube; Spotify appeared in it only for *metadata and playlist import*, which is
the pattern across all of them — Spotify's public Web API hands out track names
and never audio bytes, so the catalogue and the sound come from different places.
ViMusic and InnerTune use YouTube's private **InnerTube** API (`youtubei/v1`), the
internal endpoint youtube.com's own frontend calls, plus stream-URL extraction of
the yt-dlp/NewPipe kind. Echo ships no sources at all and says so on itself:
*"This application hosts zero content… the user manages any external sources."*

None of them hold a licence; they impersonate a first-party client against a
private API. The practical consequences are as instructive as the legal one: they
break whenever the internal API shifts, and they get removed — BlackHole's GitHub
repository and F-Droid listing are both gone. §9 already declined this chain and
that has not changed.

**But looking at it surfaced the source this design had been walking past.**

A podcast episode is an ordinary MP3 over HTTPS, named by an RSS feed the
publisher wants clients to fetch. So are Internet Archive items, Jamendo tracks,
Free Music Archive tracks. There is no DRM, no account, no vendor SDK, no terms
question, and nothing is transferred between peers — **both devices pull the same
public URL themselves**, which is the same shape as both opening their own file.

That is `TogetherContent::Stream`, and it is the best-syncing online source
available to this app by some distance:

| Source | Deadband | Why |
|---|---|---|
| Local file | fine (250 ms) | accurate position, any `playbackRate` |
| **HTTPS stream** | **fine (250 ms)** | **it is the same media element** |
| YouTube embed | coarse (1200 ms) | discrete rates, coarse position |
| Service track | coarse (1200 ms) | position reported on state change, not on a tick |

A podcast session therefore holds **four times tighter than a Spotify session
ever can**, and needs no account on either side. The catalogue is the only thing
it gives up.

### The URL is the dangerous part, and it is guarded in core

Unlike a YouTube id — eleven characters of a known alphabet — this is a whole URL
the *other person* chose, and it becomes a media element's `src`. The device then
makes a request to wherever it points, from the listener's network position,
because that is what a media element does.

`valid_stream_url` runs on the way out **and** on the way in, and holds six
lines: HTTPS only (no `http:` downgrade a peer can force, no `file:`, no
`javascript:`, no `data:`); no credentials in the authority and no `@` at all,
which also kills `https://example.com@evil.test/`; a host with a dot in it, which
is what refuses `https://router/reboot` — a URL that resolves inside the
*listener's* house rather than ours; no literal IP addresses, v4 or bracketed v6,
which takes `127.0.0.1`, `192.168.1.1` and `169.254.169.254` with it; no control
characters, spaces, quotes, backslashes or angle brackets, for the frontend that
one day builds an attribute by concatenation; and a length bound.

**What it does not do is resolve the name**, and that limit is stated rather than
papered over: DNS can point a perfectly ordinary-looking name inside the
listener's network, and a pure function on a device with no network guarantee
cannot see that. This refuses the *stated* private target. Closing the rest
belongs to whatever actually makes the request.

The check moved into `TogetherContent::admissible`, which **matches
exhaustively**, replacing two separate `if let … Youtube` arms that each guarded
one variant. Those arms were the real hazard: a new variant carrying a
peer-chosen string could be added, wired through three frontends, and reach a
`src` attribute without either of them noticing. Now a new variant has to say
which side of the line it is on.

## 11b. Driving a player that only says where it is once a second

_Added 2026-08-08, when the owner made `android/` the standing priority frontend
and asked for the YouTube embed there first._

The embed is the only source that gets close to "any song, and neither of us has
it": effectively the whole commercial catalogue, no account on either side, and a
playhead we can actually drive. `com.pierfrancescosoffritti.androidyoutubeplayer:core`
is on Maven Central — unlike Spotify's App Remote, which is a hand-vendored
`.aar` (§11) — and it is a `WebView` around the official IFrame player, so the
embed stays the sanctioned one.

What makes it interesting is not the wiring but the **reporting rate**.
`onCurrentSecond` fires about once a second. `together_report_position` is called
by the poll four times a second, and the drift ladder compares whatever it was
last handed against the peer. Feed it a reading up to a second old and the
session invents a second of drift that is not there — on a source whose deadband
is already the coarse one.

`TogetherDecisions.CoarsePlayhead` is the answer and it is three rules, all of
them tested:

- **Interpolate between ticks while playing**, and not while paused.
- **Bank the elapsed time when the state changes.** A pause 900 ms after a tick
  means 900 ms of real playback; discarding it reports a position the video has
  already gone past.
- **Move on our own seek immediately, without waiting for the next tick.** This
  is the one that would be a bug rather than a rounding error: for a whole second
  after a correction the player would still report where it *was*, so the ladder
  would see the gap it had just closed and correct again. That is the sticky
  rate-trim sawtooth `AUDIT.md` already records, wearing a different costume.

And a cap: past two ticks of silence, stop guessing and report the last thing
actually known. A stalled video — or one the system froze when the app went to
the background — sends no ticks at all, and an uncapped estimate advances a
standing-still playhead forever, confidently, hiding the stall from the very
verdict that should see it.

**Buffering is not a pause, and this is where §10's rule is enforced rather than
merely stated.** `embedState` maps the embed's `buffering` to its own
`EmbedState.Stalled`, and `embedStateIsWorthSending` refuses to tell the peer
about it. Reporting a stall as a pause is the worst ping-pong available here:
they pause because we stalled, which makes us re-evaluate, which makes them
re-evaluate.

**This layer is verified, unusually for Android.** It has no Android imports, so
`kotlinc` plus JUnit compiles and runs it in this sandbox with no SDK — 47 tests,
green, before CI. The commands are in `CLAUDE.md`. That is the practical reason
to keep decision logic in framework-free files, and it matters more now that the
priority frontend is the one that cannot otherwise be built here.

**Built 2026-08-08.** `YoutubeSessionPlayer` wraps `YouTubePlayerView` behind
`SessionPlayer`, `TogetherScreen`'s sleeve hosts it, and `/play <youtube link>`
opens a real session instead of the refusal it used to print. Three things about
it are worth carrying forward rather than rediscovering.

**The library version in the plan would have turned the branch red.** The notes
named `androidyoutubeplayer:core:13.0.0` as *verified present on Maven Central*,
and it is — but present is not compatible. Its POM pulls
`lifecycle-runtime-ktx:2.9.4`, which is compiled against API 35 and makes AGP
fail a `compileSdk = 34` build outright, and `kotlin-stdlib:2.1.0`, whose
metadata the pinned 1.9.22 compiler refuses. **12.1.2** carries
`lifecycle-runtime-ktx:2.6.0` and `kotlin-stdlib-jdk8:1.8.0` and has the
identical API surface this code touches — checked class by class against both
AARs with `javap` rather than assumed, which is a thing this sandbox *can* do for
a dependency it cannot compile against. Moving to 13.0.0 is a compileSdk-35 and
Kotlin-2.x upgrade wearing a dependency bump's clothes.

**Ad breaks are handled, and the honest version of that is narrower than it
sounds.** §9a flags the problem: a break is per-viewer, so one side gets a
pre-roll and the session comes apart by exactly that much through no fault of the
clock. `TogetherDecisions.StallWatch` observes one fact — *the player says it is
playing and the position it reports is not advancing* — and the embed's
`isPlaying` reports false while that holds. It does **not** claim to know the
cause: an ad and an unlabelled stall look the same from here, and the code says
so rather than putting "they're in an ad break" on screen over an inference.
That costs nothing, because both causes want the same answer — stop telling the
other device we are playing — and the poll expresses it as
`together_report_position(pos, false, …)`, a heartbeat the peer's `sync_verdict`
answers by *holding*. Never `together_set_state`, which would pause them for the
length of our ad.

What the wire cannot do is say *why*, and that is left open rather than papered
over: a held peer sees a device that is not playing, which is indistinguishable
from a pause. Telling the two apart needs a reason on the heartbeat, and that is
a protocol change nobody has argued for yet.

**Background playback stays off**, and the screen says so. `enableBackgroundPlayback`
would keep an embed running when the app is backgrounded, which is a feature of
*their* client on *their* subscription and not one this app may grant on their
behalf. So the "this keeps playing when you leave the app" note — true of the
file player, which the foreground service carries — is swapped for one that says
YouTube pauses. A note that claimed otherwise would be the comment-shaped lie
this repo's conventions call a bug, printed at the user instead of at a reader.

## 12. Playing a handed-over file before it has finished arriving

_Also from the 2026-08-08 request: "even when one person has the file it should
be streamed."_

Today it is not streamed, and the honest statement of where that stands is that
**core has been ready for this since the transfer landed, and neither frontend
uses it**. `ShareTracker::playable_at(pos_ms)` answers "may playback start here",
and `runway_ms(pos_ms)` answers "how many milliseconds of *uninterrupted* audio
are available from here" — a gap ends the runway however much lies beyond it,
because audio after a hole is a stutter waiting to happen. Both are unit-tested;
both are dead code.

What is missing is per-frontend, and the two frontends need genuinely different
things:

- **Android** wants `MediaDataSource` (API 23+, and `minSdk` is 26). Its
  `readAt(position, buffer, offset, size)` is called by the decoder on demand, so
  a source backed by the tracker can hand over the bytes it has and block on the
  ones it does not. This is the closer of the two to working: `TogetherPlayer`
  already owns a `MediaPlayer`, and `setDataSource(MediaDataSource)` is a
  one-line change to how it is fed.
- **Desktop** cannot use a `blob:` URL, and that is the whole reason it waits
  today: a blob is a fixed-length snapshot and cannot grow. The clean answer is
  **not** MSE — that needs a fragmented container we do not control — but a Tauri
  custom protocol serving the partial file with `Range` support, pointed at by
  the `<video>` element's `src`. The element then does ordinary progressive
  playback and a range that runs past the received prefix is where the tracker's
  answer goes.

Both need one shared decision, and it now exists in core rather than in either
of them: `read_verdict` in `crates/comrade_core/src/share.rs`, with
`ShareTracker::read_verdict_at` as the tracker-side spelling. It answers what a
reader at a playhead should do — start, keep going, or hold for bytes — built
**on** `playable_at` and `runway_ms` rather than beside them, so the two answers
cannot drift apart (a test walks all 1024 arrangements of a ten-chunk file to
keep them honest).

The rule is two thresholds, not one. Starting costs `SHARE_PLAYABLE_RUNWAY_MS`,
five seconds of *uninterrupted* audio. Continuing costs only
`SHARE_STALL_FLOOR_MS`, one second — about one chunk at a typical music bitrate,
and also the quantum the runway is measured in. The gap between them is the
whole point: a reader that stopped and started at the same number would sit on
it and chatter, and one that stopped only at exactly zero would run out inside
the decoder rather than on a decision. And the end of a file plays at any runway
at all, because a two-second runway with the last chunk inside it is not a
transfer running behind — it is a track nearly over, and waiting for more would
wait forever.

§10's rule is kept by construction rather than by discipline: the verdict has
three arms and none of them means "tell the peer", so there is nothing to send.
What that leaves each frontend is one concrete instruction that is easy to get
wrong. A hold is `together_report_position(pos, playing: false, latency)` — a
heartbeat, which the peer's next `sync_verdict` reads as "they are not playing"
and answers by holding rather than correcting. It is **not**
`together_set_state(.., playing: false, ..)`: that is a command, it takes the
next sequence number, and it pauses the other person, which is precisely the
ping-pong §10 rules out.

What the verdict cannot do belongs in the same breath, because a frontend built
on a misreading of it will look buggy in a way that is not the frontend's fault.
**It is not a prediction.** Every input is about bytes already here; nothing in
it knows the transfer's throughput, and `Continue` on five seconds of runway is
not a promise about the sixth. A transfer that stops dead still stalls — one
second of playback later per second of runway that was banked. The answer only
changes when a chunk arrives or the playhead moves, so a reader that asks once
has learned nothing.

`ComradeRuntime::share_read_verdict` exposes it, stateless and lock-free so it
can be answered from inside a `MediaDataSource.readAt` or a `Range` handler —
the only place either frontend needs it, and the place where anything that could
block would deadlock. The division of labour is the transfer's usual one: the
frontend owns the bitmap because it owns the bytes, and core owns the
thresholds.

### Built on Android, 2026-08-08

`PartialFileDataSource` is the `MediaDataSource` §12 predicted, and
`TogetherPlayer.open(MediaDataSource)` genuinely was the one-line change. The
player opens when the transfer is **armed** — before a byte moves, and before
`Accept` goes out, so no wake-up for an early chunk is missed — and the decoder
blocks inside `readAt` on chunks that have not landed. `ShareReadPolicy` is the
Kotlin twin of `read_verdict`, in a file with **no imports at all** so it runs
under `kotlinc` here; its neighbour `ShareDecisions` cannot, because it needs the
generated uniffi types, which is precisely why the thresholds moved next door.

`ShareTracker` was **not** missing on Android, contrary to the handoff's note —
`ShareDecisions.Tracker` had shipped with the transfer and had `playableAt`. What
it lacked was `tailCompleteAt` and the verdict, and `playableAt` is now built on
the first of those rather than beside it, the same construction core uses so the
two answers cannot drift.

**Writing this uncovered a real ordering bug in code that had been correct
until now.** `FileTransfer.onChunk` recorded the chunk in the tracker *before*
writing its bytes to the file. Nothing read the partial file, so nothing could
tell. The moment a decoder does, that order leaves a window where the bitmap says
the bytes are there and the file still holds zeroes — and a decoder that reads
zeroes does not stall, it produces a corrupt frame or gives up on the file
entirely, neither of which looks like a transfer problem from a bug report. It
now writes first and records second, which also gives the `synchronized` block in
`readAt` a happens-before edge that publishes the bytes wherever the flag is
visible. A read outside that monitor can only be *stale* — it can miss a chunk
that arrived, costing one wait slice, and can never claim one that has not.

Two smaller things the wiring forced, both of the kind that only appear once
early playback works:

- **Reopening the finished file reset the playhead to zero.** `MediaPlayer`
  starts at the beginning, so a transfer *completing* threw the listener back to
  the start of a track they were halfway through. The position is carried across
  now, and the seek arms the echo suppressor — an unexplained `onSeekComplete` is
  re-broadcast as the user having seeked, which would move the other person for
  no reason anyone could explain.
- **A hold is a local pause and nothing else.** `TogetherManager.applyShareVerdict`
  pauses the player; the very next line of the poll reports
  `together_report_position(pos, false, latency)`. Resuming is conditional on the
  *session* wanting to play, because `Start` is permission and not an
  instruction: a player the person paused stays paused however many bytes arrive.

**Desktop has the numbers and not the plumbing.** `share_transfer.mjs` gained
`STALL_FLOOR_MS`, `readVerdict`, `tailCompleteAt` and `readVerdictAt`, with core's
vectors ported and a walk over all 1024 arrangements of a ten-chunk file
asserting the verdict never disagrees with `playableAt`. The Tauri custom
protocol with `Range` support that would actually feed a `<video>` element is not
built.

**The FFI is exposed for uniffi only.** `Comrade::share_read_verdict` is
synchronous, stateless and lock-free — deliberately, since the only place Android
would want it is inside `readAt`, where taking a lock on the runtime would
deadlock against the transfer writing the chunk being waited for. The
`#[frb(mirror(ReadVerdict))]` half is **not** added: `app/` has no together or
share surface at all, so it would mean regenerating the whole bridge for a type
no Dart code references, and regeneration cannot be verified in this sandbox
without installing Flutter. It belongs with the first Dart consumer.

**One thing this surfaced that is not about playback at all.** A session whose
clock has converged sends no heartbeat while paused, and the peer ends a session
after 45 s of silence — so any pause past the TTL ends the evening on both
devices. That was already true for a user pause; a byte-starved hold now reaches
it with nobody pressing anything. Recorded in `AUDIT.md`; it needs either a
keepalive that is not a position claim, or an ending a session can come back
from.

## 13. Following what the device is already playing

_Added 2026-08-08, from "if users already have Morphe or ReVanced installed,
can we use it somehow?"_

The literal question has a dull answer and a much better one behind it.

**Launching their patched client does nothing for a session.** An intent at a
YouTube URL resolves to whatever app handles it, which may well be a patched
one — and that hands playback away entirely. No seek, no position, no callbacks.
A video opens and the session is left behind.

**But Android publishes every app's media session system-wide**, and that is a
different proposition. With notification-listener access, Comrade can enumerate
active sessions and drive them — `seekTo`, `play`, `pause` — and read position
from `PlaybackState`. It is what a car head unit and a watch companion do.

### Why this is better than every integration §11 was reaching for

| | Per-vendor | Media session |
|---|---|---|
| Spotify | OAuth + Premium API, or a hand-vendored `.aar` | **their app, as installed** |
| YouTube / YT Music | embed + a CSP argument | **their app, as installed** |
| Podcast apps, VLC, local players | nothing | works |
| Implementations to maintain | one per service | **one** |

It removes the client id, the OAuth flow, the App Remote distribution problem
and the `script-src` widening, all at once. Comrade never learns what the other
app is or where its bytes came from — which is the posture, not a loophole.

**On patched clients specifically.** The feature is source-agnostic by
construction: it drives a published media session and touches no vendor's API.
That is defensible in a way that InnerTube extraction (§11a) is not. What would
*not* be defensible is documenting or marketing it as a route to ad-free
playback — that is inducement, and it would convert a neutral tool into a
targeted one regardless of what the code does. Build the neutral feature; do not
build the funnel. Nothing technical is lost by holding that line.

### What it costs, stated before anyone builds on it

- **Notification-listener access.** Confirmed *not* on Play's restricted-
  permissions list (unlike the Accessibility API, which is gated and would have
  been a blocker), so there is no declaration form in the way. That is not the
  same as approved — general policy still applies and this permission is
  scrutinised. Worth confirming against a real submission before it ships.
- **Playback lives in the other app.** Comrade becomes a sync layer with no
  player of its own: no video surface, no sleeve. The Together tab turns into
  control-and-status. That is a product change, not an implementation detail.
- **Coarse position**, so the coarse deadband — the same tier as an embed.
- **Android only for now.** MPRIS, SMTC and MediaRemote are the equivalents
  elsewhere, so it generalises, but not for free.

### The decisions, and the one bug that would have been undebuggable

`MediaSessionDecisions` is pure, and it runs under `kotlinc` + JUnit here with
no SDK — 19 tests, green before CI.

**`pick` excludes our own package, and that is not an optimisation.** Comrade
publishes a `MediaSession` for the foreground service that keeps a file session
alive. A picker that did not exclude it would find Comrade, sync Comrade to
Comrade, and feed every correction straight back into the player that produced
it. From a bug report that reads as "it randomly jumps".

The rest:

- **Playing beats paused**, and `buffering` counts as playing — demoting a
  stalling session would hand the session to a paused app in another tab the
  moment the network hiccuped. Among equals the framework's own priority order
  wins, because it puts the session the user last touched first.
- **Speed is honoured.** A podcast at 1.5× advances half again as fast;
  extrapolating it at 1.0 would drift half a second per second, which is worse
  than not extrapolating at all.
- **Extrapolation is capped** at two seconds past the last report, for the
  reason §11b gives: an app that stopped reporting has stalled or been frozen.
- **`ACTION_SEEK_TO` decides `Full` against `StartOnly`**, mapping onto the
  three-value `PlayheadControl` §11 already introduced. An app that cannot seek
  must not run the ladder.
- **A skip ends the claim.** `sameTrack` compares what each side is playing, and
  blank metadata is *not* a match — an app that publishes nothing has told us
  nothing, and reading silence as agreement is the invention this design
  refuses.
- **Two different playback speeds are not something a seek can fix**, so the
  session says so rather than chasing a gap that reopens as fast as it closes.

### Built 2026-08-08

`MediaSessionListenerService` is the grant and nothing else — it overrides no
callback, because `getActiveSessions` demands the component of an *enabled*
notification listener and that API is the entire reason it exists. An empty body
states that better than a comment could, and anyone adding an
`onNotificationPosted` is changing what this app claims about itself rather than
adding a feature.

`MediaSessionAccess` is the boundary and `ExternalSessionPlayer` is the
`SessionPlayer`. The division held: **every `PlaybackState.STATE_*` int is
mapped in exactly one function**, and `MediaSessionDecisions` still imports
nothing, so all of it runs under `kotlinc` here. `trackKey` joined it — which
fields identify a track is a decision, not a read, and a blank title yields a
blank key that `sameTrack` refuses, while a missing *artist* is a weaker key
rather than silence (podcast apps routinely publish no artist, and refusing them
would turn off sync for a whole category of app that works fine).

**How a session starts is deliberately narrow.** Comrade follows; it does not
start. There is no way to tell another app's session "play track X" — only to
drive what is already loaded — so the entry point is the *invited* side: an
invitation to content this device cannot play itself offers "follow what's
playing here", and `PlaybackModeDecision.ownershipFor` is what decides whether
that is available, asked by both the button and the action so the two cannot
disagree. Starting a session *from* what this phone is playing would need a
`TogetherContent` variant for "whatever is on now", which is a wire change and
is not made here.

The permission cannot be requested in-app, so the explainer comes *before* the
settings screen and the grant is re-checked on the next tap — there is no
callback when someone comes back from system settings, and pretending otherwise
would be a button that looks broken. "Access not granted" and "granted, and
nothing is playing" are separate refusals with separate sentences, because
sending someone back to a settings screen they have already used is how a button
teaches people it does nothing.

The screen for one of these is control-and-status, as this section promised: no
sleeve, no surface, and **no scrubber** — a `MediaSession` carries no duration
worth trusting, and a bar with no end on it lies about where the end is.

### Still not built

A `TogetherContent` variant for "whatever this phone is playing", which is what
starting a session from an external player would need. And the source-agnostic
line holds in the code and the copy: no string, doc or listing here names
ReVanced, Morphe or any patched client.

## 14. Both players, and the seam between them

_Added 2026-08-08, on "we should have both"._

Comrade keeps its own player **and** gains the ability to follow someone else's.
That is not a compromise between §11b and §13; it is the only arrangement that
makes either worth building. Comrade can genuinely play a file — with a sleeve,
a video surface, an accurate playhead and the fine deadband — and it can
genuinely never play a Spotify track. Picking one would either throw away the
good case or refuse the common one.

`SessionPlayer` is the seam. Three implementations sit behind it and each was
separately blocked on `TogetherManager` being concrete on `TogetherPlayer`:

| Implementation | Who holds the audio | What the screen draws |
|---|---|---|
| `TogetherPlayer` (`MediaPlayer`) | us | sleeve, video surface, scrubber |
| YouTube embed (§11b) | us, in a `WebView` we host | the same, picture in our window |
| External session (§13) | another app entirely | control and status |

`TogetherPlayer` implements it today, unchanged in behaviour — the interface was
derived from what the manager already used, so adopting it is `override`
keywords and nothing else.

**The mode is decided once, at the start, and never changes.**
`PlaybackModeDecision.ownershipFor` answers it from the content kind and what
this device can do, and `mayChangeMidSession` is a flat `false` with the reason
on it: swapping the player under a live session means tearing one playhead down
and standing another up, and the gap between them is a session that is neither
following nor playing. The handover finishing is not an exception — `OURS` was
already the answer there; the file simply arrived.

Two rules in it are worth reading twice:

- **A file we hold is always ours**, even when an external session is available.
  Following another app for something we could open ourselves would trade the
  sleeve, the surface and an accurate playhead for nothing.
- **A file we do *not* hold is not `EXTERNAL`** — it is nothing yet. The
  handover (§9a) exists precisely so it becomes ours, and claiming an external
  player at that moment would start a session against whatever happened to be
  playing on the phone, which is not what was invited.

The seam is framework-free, which is what lets the mode decision be checked
here rather than in CI: 74 Android tests across §11b, §13 and this section now
compile and run under `kotlinc` with no SDK.

`TogetherManager` holds a `SessionPlayer` as of 2026-08-08. The widening was
the three declarations expected, but there are **two** narrowing sites in the
manager rather than one, and the second was missed by the plan:

- `attachSurface` — expected, and correct. A surface is meaningful only to the
  player that decodes into one; an embed draws into a `WebView` we host and an
  external session draws in another app's window, so for those this is
  correctly nothing rather than an override that has to pretend.
- `openPlayer`'s **reuse arm** — not expected. `val p = player ?:
  TogetherPlayer(ctx)` takes its type from the left operand, so widening the
  field silently widened the local too and `setListener`/`open` stopped
  resolving. Found with a compiler rather than by reading: a negative probe
  against the real interface produces exactly the three unresolved references.

Both are the file path's `MediaPlayer` semantics, which is what this section
already says does not survive being made abstract — the plan simply did not
notice that Kotlin's type inference would drag the local along with the field.

**Both implementations landed 2026-08-08** — `YoutubeSessionPlayer` (§11b) and
`ExternalSessionPlayer` (§13). The seam held: neither needed the interface
widened for its own convenience, and the manager drives all three through it.

One member was added, and it is worth saying why it is not an exception to that.
`SessionPlayer.onPoll(nowMs)` is a default no-op that the session's poll calls
before reading. `TogetherPlayer` ignores it, because a decoder always knows where
it is. The other two are *told* where they are on somebody else's schedule — once
a second for an embed, on state changes for an external session — and both need a
clock that keeps running **when the reports stop**, because "the reports stopped"
is itself the thing worth noticing. Without it a frozen player goes on claiming
to play against the last position it happened to mention, and no correction can
see it. A default rather than an abstract member, so the one implementation with
nothing to do is not made to write an empty override saying otherwise.

Two narrowing sites remain the file path's, and a third joined them for the
embed: `attachSurface` and `openPlayer`'s reuse arm as before, plus
`attachEmbedView`, which hands a `YouTubePlayerView` to a session that is holding
one. All three are the same shape — a view belongs to the player that draws into
it — and all three say so where they narrow.

The work list, the exact call sites and the traps are in
[`docs/TOGETHER_PLAYERS_HANDOFF.md`](TOGETHER_PLAYERS_HANDOFF.md).


## 15. Sending the picture and the sound, not the clock

_Added 2026-08-08, from "ideally the app should just stream whatever is playing
on one device — video/audio — with the best sync"._

**The instinct is right, and it is worth saying why before saying what it
costs.** Everything hard in §3–§11b exists for one reason: two independent
players have to be kept in step. The drift ladder, the deadband, the coarse and
fine tiers, `CoarsePlayhead`, the ad-break `StallWatch` — all of it is machinery
for a problem that a stream simply does not have. Send the frames and there is
**one** playhead; A/V sync becomes the transport's job, and it is a job WebRTC
already does. "Best sync" is not something to tune here. It is exact by
construction.

So this section reverses §1 — but only as far as §1's actual reason goes, which
is worth reading precisely rather than as a blanket ban.

### What §1 rules out, and what it does not

§1 and `AUDIT.md` §8.2 rule out **re-streaming or proxying licensed content**:
a copyright problem, a bandwidth problem, and a technical wall on DRM'd
platforms. Every word of that still holds.

What it never ruled out is a person sending **their own file** to **one person
they invited**, which is a thing this app already does — that is the §9a
handover, and nobody thought it needed a different answer. Decoding that same
file and sending the pictures instead of the bytes is the same act with a
different codec. So:

| | Streamed? | Why |
|---|---|---|
| A file the leader holds | **yes** | Their own file, one invited peer. §1's reason does not reach it. |
| Comrade's YouTube embed (§11b) | no | Both sides get the real player free. Re-streaming it breaks YouTube's terms and looks worse. |
| A service track (§11, §13) | no | Licensed audio, and the platforms block capture anyway. |
| Anything else on the phone | **later, and honestly** | See "Screen capture" below. |

### The two halves, and which one was the risk

**Video** is the half that already exists in outline. `CallManager.startScreenShare`
already pushes a `ScreenCapturerAndroid` into a `VideoSource` over a live
`PeerConnection`, foreground-service dance and renegotiation included. A player's
frames reach a `VideoSource` the same way a screen's do.

**Audio was the real feasibility question**, and the answer is not obvious:
libwebrtc's Android audio path captures from the *microphone*, and there is no
supported way to hand it a buffer. Sharing a film with the machinery as it
stands would send the picture and the sound of the room.

It is possible with the build this repo already depends on, and it was checked
against the AAR rather than assumed. **The first answer was the wrong one**, and
it is worth keeping because the mistake is easy and the symptom is not obviously
a bug. `io.github.webrtc-sdk:android` adds
`JavaAudioDeviceModule.Builder.setAudioBufferCallback`, which is handed the
record buffer and can replace the microphone with anything — but it sits in
`WebRtcAudioRecord`'s read loop, **before** the audio processing module. A film
injected there goes through machinery built for speech: the noise suppressor
treats a sustained note as noise and gates it, and the automatic gain control
pumps the dynamics, lifting quiet scenes and flattening loud ones. Nobody would
file that as "the injection point is wrong"; they would file it as "the stream
sounds bad".

The right seam is `ExternalAudioProcessingFactory.setCapturePostProcessing`,
also absent from upstream, which runs on the capture path **after** the whole
chain and before the encoder. Both halves then get what they need, and they are
genuinely different needs:

- **the microphone keeps every calling feature** — echo canceller, noise
  suppressor, gain control — because it has already been through them by the
  time the media audio is added, and
- **the media audio is touched by none of them**, because nothing downstream
  processes it.

That `io.github.webrtc-sdk` is the fork already in use is therefore load-bearing
twice over, and swapping it would take this feature with it.

**The format check fails to silence, never to noise.** The processed buffer is
float and channel-major on libwebrtc's int16 scale, but the exact shape across
that JNI boundary is a property of the fork and cannot be checked in this
sandbox. So `AudioInjection` *derives* the sample width from the buffer's own
size — the APM works in fixed 10 ms frames, so the only free variable is bytes
per sample — and **leaves the buffer completely alone** when it does not
recognise the layout. A wrong guess that writes puts full-scale noise directly
into somebody's ear; a wrong guess that declines produces a stream with no film
audio. Those are not equally bad outcomes and the code is not neutral between
them.

Where the PCM comes from is the second half. `AudioPlaybackCapture` (API 29+)
with `addMatchingUid(Process.myUid())` captures **our own app's** playback, and
an app may always capture itself — so the `MediaPlayer` in `TogetherPlayer`
needs no replacing. The cost is that it needs a `MediaProjection`, which means
the system's recording-consent dialog even to capture ourselves.

### Screen capture, and the limits stated before anyone builds on them

Streaming *whatever* is playing — Spotify, a podcast app, someone's downloads
folder — is the same pipeline with a wider capture configuration, and it is
worth writing down now what it will and will not do, because discovering it in
the field reads as a bug:

- **`FLAG_SECURE` surfaces come through black.** Netflix, Prime Video and
  Disney+ set it. `MediaProjection` hands us black frames and there is nothing
  to fix — the platform is working as designed.
- **Apps can refuse to be recorded.** `ALLOW_CAPTURE_BY_NONE`, set by the app
  being captured, means silence with no error. Confidence here is about the
  mechanism, not about which specific apps use it: that needs a device, and this
  document should not guess.
- **The copyright question is the user's and is not made better by the code.**
  Pointing a capture at licensed content is the thing §8.2 declines. The feature
  may exist as *"show them your screen"* — a gesture that already ships inside a
  call — and it must never be documented, named or marketed as a way to watch
  somebody else's subscription. That is the same line §13 draws for patched
  clients, and for the same reason: a neutral tool and an induced one differ in
  what they are *for*, not in what they do.

> **Owner decision, 2026-08-08: proceed with screen capture**, on the reasoning
> that an app which does not want to be recorded already says so with
> `FLAG_SECURE`, and Comrade honouring that is the platform's protection working
> rather than being worked around. That is a sound reading and it settles the
> *technical* question: nothing here circumvents anything, and the black frames
> Netflix produces are the system doing its job.
>
> It does not settle the copyright one, which is separate and stays where this
> section already put it — the marketing constraint above holds regardless, and
> is the part to hold when somebody later suggests naming a service in a
> feature list.

### Talking over it, which is the actual point

_Owner, 2026-08-08: "the feature is intended for users to do things together —
we'd like a dedicated mic icon to enable or disable mic audio where users can
talk."_

This changes the shape of the audio path rather than adding to it, and it was
worth catching before the pipe was built: a session carries **one** audio track,
so the sender's voice and what they are playing have to arrive as one thing. The
first cut of `PlaybackCapture` *replaced* the microphone buffer with the film's
audio, which is exactly wrong for a feature whose point is that two people are
watching together. It **mixes**.

`PcmMix` is where that lives, and it is a separate tested file for one reason:
two 16-bit samples do not fit in 16 bits. `-20000 + -20000` is `-40000`, which
wraps to a large *positive* value — a full-scale sample where a loud one
belonged, which is an audible crack on every peak, inaudible in a quiet test and
obvious the moment two people talk over a loud scene. Every sum saturates.

The control is `TogetherManager.micEnabled`, shaped after `CallManager.muted` so
the two microphones behave alike, and **off by default**: a session that opened
with a live microphone would have decided something about a room it cannot see.
Off *overwrites* rather than attenuates, so nothing of the sender's room leaves
the device. The icon is drawn only in a streamed session, because in every other
mode there is no audio of ours going anywhere and a control that toggles nothing
is worse than no control.

**One limit that is not fixable here, and the UI says so rather than letting it
be discovered.** With the microphone on and the sound coming out of speakers,
the other person hears the film twice — once injected cleanly, once through the
sender's room, a fraction of a second later. WebRTC's echo canceller does not
help: it cancels what *it* played out, and the film goes through `MediaPlayer`,
which it knows nothing about. Headphones are the answer, and
`together_mic_note` is where that is said.

### What this does not replace

**The handover is still the better answer whenever it can run**, and it got
better on 2026-08-08 (§12): a file now plays while it arrives, so the follower
starts within about five seconds instead of after the whole transfer. Against
that, a stream is lossy, needs both sides online for its whole length, costs
continuous bandwidth, and leaves the follower with nothing afterwards. What it
buys is exactness of sync the deadband already makes imperceptible, and reach
into content the follower can never hold.

So streaming is the **third** answer to §9a's question, not a replacement for
the first two: *find your own copy*, *take mine*, and now *watch mine as I play
it*. The session picks one and says which; `PlaybackModeDecision` is where that
belongs, and it stays a decision made once per session (§14).

### Built so far

**The factory gains one thing: an audio processing factory** carrying
`AudioInjection`, a process-wide router that **does nothing at all when no
session has installed a capture**. A device that never opens a streamed session
is processed exactly as it was.

An earlier version also handed the factory a hand-built `JavaAudioDeviceModule`,
and removing it is worth recording. It existed to reach
`setAudioBufferCallback` — the wrong seam, as above — and became dead weight the
moment the injection moved after the processing chain. Dead weight with a cost:
`ensureFactory` runs *before* a call is placed, and building an audio device
module probes the platform's echo canceller and noise suppressor, so **every
first call paid for it**. The device test caught it as a call still sitting in
`Ended` 2.5 s after failing, which is that latency made visible rather than a
flaky assertion. Leaving the default module alone also means calls capture
exactly as they always did, which is a much easier claim to defend than a
hand-built module matching it option for option.

**One consequence to state rather than let someone find.** The injected audio
rides the *record* path, so sending the sound of what you are playing needs the
`RECORD_AUDIO` grant **even with the microphone off** — a permission prompt for
a feature that is not about the microphone. The UI owes the user that sentence.

**The video path is finished up to the wire.** `PlayerVideoCapturer` is the
joint between a `MediaPlayer`, which draws into a `Surface`, and WebRTC, which
takes frames from a `VideoCapturer`: a `SurfaceTextureHelper` owns the texture,
the player decodes into it, and every frame goes straight to the capturer
observer. There is no capture loop — the decoder's own cadence *is* the frame
rate, and a paused film simply stops producing frames. `isScreencast` is false
on both the capturer and the source, deliberately: WebRTC degrades a screencast's
frame rate and keeps its resolution, which for motion video is backwards and
produces a sharp slideshow.

The consequence is that **the sender cannot watch the surface any more**, since
a `MediaPlayer` has one output. They watch the outgoing `VideoTrack` instead —
the same one the other person receives — rendered by a `SurfaceViewRenderer`
exactly as the call screen renders local camera video. One picture path rather
than two, and it is why `TogetherManager.localVideo` exists.

`MediaPlayer` reports its dimensions only after opening the file, so capture
starts at 1280×720 and `onVideoSize` corrects it. Without that the whole session
is scaled to the guess.

The rest: `PcmRing` (9 tests) is the buffer between capture and encoder,
`PcmMix` (10 tests) the mixing and the saturation, `micEnabled`/`toggleMic` and
the icon the control.

### The transport, and the wire change it did not need

`StreamTransfer` is a second `PeerConnection` between the same two devices,
alongside the handover's, negotiated over the same signals — and **the SDP is
the intent**.

There is no new signal for "stream it instead of sending it". The handover's
exchange is `Ask` → `Offer` → `Accept` → `Transport…`, so a `Transport` **offer
arriving with nothing armed cannot be a file**: nobody asked for one and nobody
accepted one. That is the whole discriminator. It costs no protocol change, no
`frb` regeneration, and no version skew — an older build drops an offer it has
no session for, which is exactly what it already does. The offer's own m-lines
say the rest, because `FileTransfer` opens a data channel and this adds tracks.

Its **own** connection rather than the transfer's, for the reason the transfer
does not share the call's, one level along: a film is a continuous encode with a
deadline and a handover is a bulk push, and under one congestion controller the
bulk push wins and the film stutters.

The receiver's sink is installed **once, for the life of the process**, not per
session — a stream offer can reach a device before it has any idea one is
coming, which is what "the SDP is the intent" means in practice. Registering it
per session would require having guessed first.

There is no `SessionPlayer` for the receiving side and there does not need to
be: it holds no decoder and no playhead. The frames arrive already in step,
because there is one playhead and it is the sender's. That is §15's claim about
sync, and `TogetherManager.remoteVideo` is the whole of its implementation.

### The gesture, and the ordering trap under it

*"Let them watch mine"* sits on the live screen of the side that holds the file,
next to §9a's other two answers rather than in a menu — and only for our own
player, since an embed is already on both screens and an external session is
somebody else's audio to send.

**Two system prompts, in an order that matters.** `RECORD_AUDIO` first, because
the media audio joins on the *capture* path and there has to be a capture running
at all — a permission prompt for a feature that is not about the microphone,
which is why the screen explains it rather than letting it arrive bare. Then the
screen-capture consent. Both refusals are survivable and neither stops the
picture: decline the recording consent and the other person still sees it,
without its sound. Nothing about that is a failure path, so none of it is written
as one.

**The trap is Android 14's**, and it is the same one `CallService` records:
a `MediaProjection` may only begin while a foreground service *already*
declaring `mediaProjection` is running. So `TogetherService` now declares
`mediaPlayback|mediaProjection`, is re-announced before the projection is
fetched, and `promote` keeps announcing **both** types from then on — dropping
the projection type from a later promotion is how a capture dies mid-session.
The whole sequence lives in `TogetherManager.startStreamingFromConsent` rather
than in the screen, so no caller can get the order wrong: the re-announce is a
second `startForegroundService` to a live instance, which arms a fresh promotion
deadline, and `onStartCommand` promoting immediately is what keeps that safe.

### Not built

The desktop half, where receiving is a `srcObject` and sending is not.

**Nothing in this section has run.** It type-checks against the real AAR, and
`streaming` is never set true by any user-reachable path, so the renderer and
the mic icon do not draw yet. The audio device module change, by contrast, *is*
live for every call the moment this ships — which is the one part of §15 that
wants a device before it is trusted.

## 16. The tab, and why it became the only way in

_Added 2026-08-08._

§9b's table describes what a *route* does, and it stayed accurate. What it took
for granted was the sentence above it: that starting a session means being in a
conversation with the person you want to listen with, and either typing `/play`
or reaching for the ▶ in that chat's header.

That was backwards for music, and the ▶ is the thing that shows it. It could
offer exactly one source — the file picker — because it was the file picker's
button. So the answer to "listen to an album with a friend" was *open their chat,
tap ▶, find the album in a system document browser by filename*, while the phone
had a music library sitting right there that only the invitation path ever read.
And there were two entry points for one intention, so which sources you could
reach depended on which screen you happened to be on.

So the ▶ is gone and the Together tab (🫂) is the whole flow: **pick something,
pick someone.** Three sources, in the order they are offered
(`TogetherDecisions.sources`):

| Source | What it is | Needs |
|---|---|---|
| Music on this phone | `MediaStore`, listed with covers and searchable | the audio-library read |
| Open a file | the picker, as before | nothing — SAF grants per file |
| Paste a link | a YouTube video, or one public HTTPS media URL | nothing |

Then the "who with?" sheet, which is comrades first and online first
(`TogetherDecisions.listenersFor`) — starting a session is asking for someone's
attention *now*, so the person who is there is the one to offer first. Contacts
who are not comrades are still listed: an invitation is a DM like any other, and
presence is a thing you opt into mutually rather than a precondition for asking.

`/play` is untouched. It reaches the same session by the same path, and it
remains the faster gesture when you are already talking to the person — which is
the case it was designed for and the only one it was ever good at.

### The alert, which did not exist

`TogetherManager.onInvited` runs off a bridge event, so it has always worked with
the app closed — and until now the only sign of an invitation was an overlay the
person found the next time they opened Comrade. There is a notification now
(`Notifier.notifyTogetherInvite`, `CHANNEL_TOGETHER`, `IMPORTANCE_HIGH`), cleared
from the two points every route out of `Invited` passes through.

It **names who and deliberately not what**. The title an invitation carries is a
recording somebody chose or a URL's host, and either one on a lock screen other
people can see is a disclosure nobody asked for. Who asked is what makes it
actionable; the screen says the rest once the phone is unlocked.

### The third source, at last: `TogetherContent::Stream`

§11a worked out in August that a podcast episode is the best-syncing online
source there is and named `TogetherContent::Stream` as the shape for it. Core has
carried the variant, the guard and the tuning since; desktop grew the sending
half (`stream_link.mjs`); **Android could neither start one nor accept one.** An
invitation whose content was a `Stream` reached the phone, set `contentKind =
"stream"`, and got offered a file picker for a file nobody has.

Both halves are there now — `TogetherManager.startStream` and `joinStream` —
and three things about them are decisions rather than plumbing.

**Core sees the URL before the player does.** `together_start` runs
`TogetherContent::admissible`, which for a `Stream` is `valid_stream_url`, so a
URL naming the listener's own LAN, a literal address or a credential pair is
refused before any request leaves the device. Opening the player first to learn
its length would make that request *ahead of* the check that exists to prevent
it, and would buy a `duration_ms` that a source both sides fetch from the same
place does not need. `desktop/ui/main.js`'s `startStreamSession` made the same
call for the same reason; this is the two frontends agreeing.

**A stream invitation is never auto-joined**, and the reason is stronger than the
YouTube one. Joining makes a request to a host the *other* person named — a
decision about this device's network, and not one to take on somebody's behalf
however confidently core validated the string. It also must not fall through to
the library lookup: a stream that happens to name a recording this phone owns
would otherwise open the local file and report a playhead for something else.

**A URL that names no media is refused, and refusing is the useful half.**
`valid_stream_url` answers "is this safe to hand a player"; the new
`direct_media_url` answers the different question "is there any point".
`https://example.com/episodes/42` passes the first and fails the second, and
pointing a `MediaPlayer` at an HTML document is a hang the person who pasted it
reads as the feature being broken. So the extension is checked (query string
excluded — `…/ep12.mp3?token=…` is normal), and anything else becomes words to
search for, which is recoverable. `TogetherContent::stream` is the one parser,
reached from Kotlin through `together_stream_content`; the ordering against
YouTube is `TogetherDecisions.classifyLink`, because `https://youtu.be/…` is a
valid HTTPS URL too and a stream check running first would point a player at a
web page.

~~**Desktop still cannot tell a page link from an episode** before playing it~~
**Closed 2026-08-14.** `planStream` now draws core's media-suffix line before a
session opens: a pasted page link is refused at the paste, by name, instead of
opening a session, inviting the other person to it, and failing out of the media
element seconds later. Desktop has no FFI call to `TogetherContent::stream`, so
the rule is a **mirror** (`namesMedia` in `stream_link.mjs`), and what keeps a
mirror honest is the pin: `stream_link.test.mjs` reads `STREAM_MEDIA_SUFFIXES`
and the accept/refuse vectors of `only_a_url_that_names_media_becomes_a_stream`
out of `together.rs` itself, so an extension added in core without landing in the
mirror is a red test rather than a desktop that quietly sends real episodes down
the refusal path. `COULD_NOT_PLAY` stays, for the miss no pure function can
catch — a suffixed URL that turns out to be a page.

### What is checked before CI, and what is not

The pure half is `TogetherDecisions` as always — the clock, the library filter
and sort, the source list, the link ordering, the listener ordering, and the
scrubber's precondition, all JVM-testable and all pinned. That is deliberate and
it is why those decisions are *in* that file: `ui/TogetherScreen.kt` is 1,700
lines of Compose and no test in this repo executes any of it.

The type-checking is new though, and it is worth knowing the boundary moved:
`.claude/scripts/android-typecheck-compose.sh` resolves the Compose half against
the real Material3, which is what caught this screen's API mistakes before a
push. It checks types and nothing else — **how it looks is still unverified by
anything but a device**, and the 🫂 glyph in particular is hand-authored path
data that no lane here can render.

## 16. The pair is the session, not the track

Built on Android, 2026-08-08. Desktop does not have it.

Until this, the unit of the whole feature was *one thing, played with one
person*. Choosing a track and choosing a friend were one gesture, so every next
song asked again and sent the other side a fresh invitation — a shape that is
exactly right for "watch this film with me" and exactly wrong for the thing the
Together tab mostly is, which is a music player. Nobody picks a friend once per
song.

So the **pairing** is now the session and the content is what passes through it.
`TogetherManager.pairing` holds it, `TogetherDecisions.startStep` reads it, and
the who-with sheet appears when there is nobody yet and not again.

**None of this is a protocol change, and the reason is worth stating because it
looks like one.** `together_start` refuses while a session exists
(`runtime.rs`), so putting a second track on genuinely is an `End` and a `Start`
on the wire, exactly as before. What makes it one evening rather than two is
that both sides keep the pairing across it:

- the side changing content keeps it because it never left
  (`beginSession`), and swallows the `TogetherEnded { by_peer: false }` its own
  `together_end` raises — that event reaches the frontend through the event
  channel and therefore lands *after* the `together_start` that replaced it, so
  acting on it would tear down the session that is already running;
- the receiving side keeps it for `PAIRING_GRACE_MS` and treats the next
  invitation from the same person as a continuation
  (`TogetherDecisions.continuesSession`), which is what stops the follower being
  asked once per track.

Two things make that safe rather than merely convenient. Core drops an inbound
`Start` while a session exists and ignores an `End` whose session id is not the
one it is in, so a *stale peer* end cannot reach the frontend at all and only
our own can — which is why one counter, and not a session-id ledger, is enough
in `TogetherManager`. And `continuesSession` excludes `stream` content on
purpose: joining a stream fetches from a host the other person named, which §11a
already refuses to do unasked, and a pairing is agreement to listen together
rather than agreement to fetch from wherever they point next.

**The known cost**, recorded rather than discovered: if the `End` is lost or
overtaken, core drops the `Start` that follows it and the follower is left idle
until they are invited again. That is `together_start`'s existing single-session
rule, not something this added, and closing it properly means a core signal that
replaces content in place.

### Either of you may put something on

The follower's transport was never read-only — commands are ordered by a Lamport
counter and neither side is privileged — but *choosing what plays* used to be the
leader's alone, because it was the same gesture as choosing a person. Now both
can, and the one that interrupts somebody gets asked first:
`StartStep.ConfirmTakeover` carries the other person's name into the dialog. The
leader replacing their own track is deliberately not a question, because a dialog
on every press of next is what would make a queue unusable.

### Previous and next

`TogetherDecisions.Queue` is the list a track was picked out of — the library as
the search field had narrowed it, which is what the person was looking at when
they chose. Sources that are not a list get none, and the next button is drawn
and disabled rather than absent: a control that appears and disappears under the
thumb is worse than one that is visibly unavailable. Previous is never disabled,
because `backStep` restarts the current track when there is nothing behind it —
and restarting goes through `setState`, so the other person follows it, while
moving to the previous track replaces the content like any other change.

### Talking over it

The microphone is now offered in **every** mode rather than only in a streamed
one. The old argument — that in the other modes no audio of ours goes anywhere —
was a fact about the wire and beside the point for the person: listening to an
album together with no way to say "this bit" is a worse version of listening
alone.

What differs is only what the voice rides on. A streamed session already carries
the film's own sound on its outgoing track and `PlaybackCapture.micEnabled`
decides whether the voice is summed into it (§15, unchanged). Everything else
opens a voice-only `PeerConnection` through `StreamTransfer` — no new wire type,
because "the SDP is the intent" already covers an offer with no armed transfer,
and its m-lines say the rest.

**The track goes on at negotiation time and never afterwards, on both sides.**
`StreamTransfer.localAudio` is what the *answering* side adds before it answers,
so once a connection exists both microphones are on it and muting is
`setEnabled` — the same arrangement a call uses. Nothing here renegotiates, and
this is why it does not have to: a mid-session renegotiation over a relayed
signalling path is a stall in the middle of a film.

That leaves one honest gap, and the screen says so rather than offering a button
that does nothing: if the permission is granted only *after* a picture is already
arriving on that connection, the microphone cannot join it, because renegotiating
would take the picture away to add a microphone.

Off by default in every mode. A session that opened with a live microphone would
have decided something about a room it cannot see.

### Joining no longer opens a file picker

The bug this replaced was the plainest one in the feature: tapping **Join** on an
invitation to a local file opened the document picker and asked the person to
find their own copy — of the thing the invitation exists *because* they do not
have. `TogetherDecisions.joinAction` is the rule now, and for a local file the
answer is their copy over the session's own connection, playing as it lands
(§12). The picker is the second answer, offered as "I have my own copy" to
whoever does have the file and would rather not spend the bytes.

### The tab stopped looking like a different app

It painted its own dark-blue gradient and its own five colours, on the argument
that a picture wants a dark chrome around it whatever the system theme says.
True of a film — and this is a bottom-nav tab sitting next to four screens that
do follow the theme, so it read as a different app and ignored Material You.
Every colour on it is a `colorScheme` token now.

### When YouTube refuses

The IFrame player's error reached logcat and nowhere else, so a video its owner
does not allow outside YouTube left the session sitting under YouTube's own
"This video is unavailable" panel, still saying it was waiting for the other
person to open something that was never going to open. The panel is theirs and
§11a is why it may not be replaced or hidden — but the session's answer
underneath it is ours: `TogetherDecisions.embedFailure` picks the sentence, and
`watchUrl` offers the way over there for the common case. That URL is built only
from something that is actually an id, because the string arrived over the wire
and ends up in an `Intent`.

## 17. The main thread was doing the network, and offline is where it showed

_Added 2026-08-09, from a two-device test with both phones offline: the leader's
app hung with "Comrade isn't responding — Close / Wait", and the follower saw the
track's name and never heard it._

**Every `together_*` call blocks the thread it is made on.** `ComradeCore`
bridges the async FFI with `runBlocking` (its own header says so), and the last
rung of `RuntimeHandles::send_together` is `vault.send_dm(…).await` — a relay
round trip. So the chain was:

```
Compose onClick  →  TogetherManager.setState  →  runBlocking { together_set_state }
                                              →  send_together  →  vault.send_dm().await
```

on the **main thread**. With a relay reachable this is a fast enough round trip
that nobody noticed. With no relay reachable it is however long the send takes to
give up, and five seconds of it is an ANR. Every entry point had the same shape:
`start`, `join`, `leave`, and every play, pause and seek.

The worse instance was not on the main thread at all. `StreamTransfer.send` is
called from three WebRTC observer callbacks — `onCreateSuccess` (twice) and
`onIceCandidate` — which run on the peer connection's **signalling thread**, and
it made the same blocking relay send inside each one. `onIceCandidate` fires once
per candidate. Blocking that thread stalls ICE gathering, SDP handling and
candidate delivery together, which is a negotiation that stops rather than fails:
from outside, a stream that buffers forever and a follower who receives nothing.
That is the same class as the two callback deadlocks `.claude/rules/rust.md`
records, arriving from the Kotlin side.

### One queue, because ordering is part of the fix

`TogetherManager.sendOut` puts every outbound command on an unbounded `Channel`
drained by a single coroutine on `Dispatchers.IO`. A `launch(Dispatchers.IO)` per
call would have fixed the blocking and introduced a worse bug:

- `together_set_state` takes the session's next **Lamport sequence number inside
  the call**, so two commands racing on different IO threads can be numbered in
  the opposite order to the taps that made them. Pause-then-play arriving as
  play-then-pause is a session that plays when the person asked for silence.
- Handover signals are worse still: `ShareTransfer.send` *was* `io.launch { … }`
  per signal, so an ICE candidate could overtake the offer it belongs to — a
  negotiation that cannot complete. FIFO is what that needs, and a dispatcher
  does not give it.

Failures are logged and dropped, never retried: a session command is worthless
once stale, the next heartbeat carries the truth, and the drift ladder is what
closes the gap a lost one left. Retrying would deliver a play the person has
since undone. What the screen says instead is `sendFailed` — "couldn't reach them
just now" — because the player now starts regardless, and something has to admit
the other half did not happen.

### What this does not explain

**The follower still may not play, and this change is not a claim that it will.**
It removes a mechanism that would produce exactly the reported symptom, on the
thread that would produce it. It has not been run on two devices. If it persists,
the next thing to read is whether ICE gathered host candidates at all — two
phones with no shared network have no path for a stream regardless of what any
thread is doing, and `together_start` reaching the peer at all (the invitation
*did* arrive) only proves the mesh carried a DM, not that a media connection can
be built.

## 18. Listening alone, which is most of what a music player does

_Added 2026-08-09, owner request: "the idea is to use it as a standalone music
player as well"._

The tab could browse the phone's music, queue it, draw a cover and drive a
transport, and then insisted on a second person before any of it ran. That is a
strange thing for a music player to insist on, and it is the thing that makes the
feature unusable exactly when it should be at its most useful — one person, no
network.

**Alone is a pairing with nobody in it** (`TogetherDecisions.ALONE`), not a
second kind of session, and that is the whole of the change:

| Kept, unchanged | Removed |
|---|---|
| the player, the queue, prev/next, the scrubber | their name and the status line |
| the foreground service and its notification | the two measured readouts |
| the library, the picker, the link field | the offer to send them the picture |

One gate reads it — `sendOut` returns early — so there is no `solo` branch in the
transport, the service, the queue or the notification. The empty string is a safe
sentinel because a real peer is a bech32 `npub1…` that can never be blank, which
is pinned by a test rather than assumed, and it fails in the right direction: a
bug that let it reach `parse_pubkey` gets a refusal from core rather than a
message to the wrong person.

`startStep` answers alone **before** the takeover arm, and that was a real bug
found while writing its test: a solo session reaching that arm would have asked
"stop what … is playing?" with a blank where the name goes.

### Not done

No unit test covers the threading fix or the solo gate *in the manager* —
`TogetherManager` imports Android, so the JVM lane cannot see it, and there is no
Robolectric lane in this repo. What is tested is the pure half: `isAlone`, the
sentinel's safety, and `startStep`'s three answers.

## 19. The two-peer lane that was green and proving nothing

_Added 2026-08-09, after §17's bugs were found on handsets rather than in CI._

The obvious reading of §17 is "we need a two-phone check". The more useful
finding is that **one already existed and had never run.**

`android/app/src/androidTest/.../TwoPeerJniIntegrationTest.kt` stands up two
independent `Comrade` FFI instances, each with its own vault, against one relay,
and exchanges traffic across the real generated bindings. It has been there since
COMMS-03. It begins with `Assume.assumeTrue(comradeTestRelayUrl != null)`, and
`android-apk.yml` ran `connectedDebugAndroidTest` without ever passing that
argument — so every test in the file skipped on every run, and **a skipped test
is a green test.** The lane that was supposed to be the evidence for two peers
talking to each other was evidence of nothing.

Three things close that:

1. **The workflow starts the relay** (`deploy/test-relay/docker-compose.yml`,
   which also existed and was wired to nothing) and reaches it over
   `adb reverse tcp:8090 tcp:8090`, so the device's own `127.0.0.1:8090` is the
   address. The image is pinned rather than `:latest`, because this is a gate now
   and an upstream push should not be able to turn a branch red.

   **It used `10.0.2.2` first. An earlier revision of this section said that is
   "what the first run failed on" — that was a guess presented as a finding, and
   it was wrong.** The next run used `adb reverse` and failed identically: all
   three tests "nothing arrived", the relay logging a clean startup and no client
   traffic. The address was never the cause (§19.1 is). `adb reverse` is kept
   because it is the better mechanism on its own merits — no dependence on the
   emulated radio NAT, and it works on a physical handset, which `10.0.2.2` never
   can — but it fixed nothing, and the honest version of the sequence is that two
   plausible network causes were proposed and neither was real.

   Worth recording what was *also* not the cause, for the same reason: Android's
   `cleartextTrafficPermitted` policy does not apply here. It is enforced by the
   Java networking stack, and these peers connect through `nostr-sdk` on a native
   tokio socket, which the platform does not intercept. A `networkSecurityConfig`
   would have been a fix for a mechanism that was never running.

   **And the relay's log was not the witness it appeared to be.** "No client
   traffic whatsoever" was read three times as evidence the peers never arrived;
   it was nothing of the kind. At the default log level `nostr-rs-relay` records
   startup, migrations and a once-a-minute sqlite checkpoint, and an idle relay
   with no client at all produces exactly the log a busy one does. The compose
   file now sets `RUST_LOG=info,nostr_rs_relay=debug`, which logs connects and
   each EVENT/REQ, so "the peers never arrived" and "they arrived and the test is
   wrong" finally look different.
2. **Skipping has to be asked for**, which is the opposite polarity to the
   obvious one and the second thing this section got wrong. The first attempt was
   a `comradeRequireRelay=true` argument the workflow passed to *demand* a relay —
   and it could not work, because the flag and the URL travel by the same
   mechanism. Anything that stops the instrumentation arguments arriving drops the
   demand along with the thing it was demanding, and every test skips green again;
   it caught only a human deleting one line and leaving the other. So
   `relayUrlOrSkip` is inverted: CI passes nothing and gets a red test the moment
   the wiring breaks, and a laptop passes `-e comradeAllowRelaySkip true` to get
   its skip. Absence of configuration is strict, which is the only polarity that
   fails safe.

   And because no in-test guard can catch instrumentation that never ran at all,
   the workflow also asserts on the **results XML** rather than on its own intent:
   "Assert the two-peer tests actually ran" fails if any of the three is recorded
   as skipped, or if fewer than three are recorded. Intent is not evidence.
3. **A session is what it tests**, not just a DM: invite → join → a run of
   transport commands → end, with the arrival order asserted.

### 19.1 The lane was testing one peer twice

_The actual cause of the "nothing arrived" runs, found 2026-08-09 by reading the
FFI instead of the network._

`Comrade.newWithRelays(listOf(relay))`, called twice, did not produce two peers.
It produced **the same peer twice**, and every symptom follows from that:

- `comrade_jni` holds `static RUNTIME: OnceLock<Arc<RwLock<ComradeRuntime>>>` —
  one runtime for the process, which is correct and deliberate: both foreign
  ABIs (uniffi for Kotlin, flutter_rust_bridge for Dart) must see the same
  unlocked vault, and `comrade_storage` opens redb with an exclusive file lock,
  so two runtimes over one directory cannot both open it.
- `new_with_relays` bound *that* runtime, seeding its relay set only if it
  happened to be the first caller. So `alice` and `bob` were one runtime.
- `unlock_vault` is idempotent by design — "safe to call more than once, returns
  the already-loaded identity". So `bob.unlockVault(bobDir, "pin")` never opened
  `bobDir`; it handed back **alice's** identity.
- Therefore `aliceNpub == bobNpub`. `alice.sendDm(bobNpub, …)` was alice DMing
  herself, no `IncomingMessageRequest` was emitted, and all three tests failed on
  their first assertion with "nothing arrived" — a message about the wire,
  describing a bug that had nothing to do with the wire.

The fix is `Comrade::new_isolated_with_relays`, which builds a `ComradeRuntime`
of its own. Two runtimes in one process is safe **here and nowhere else**, and
the two reasons are worth stating rather than rediscovering: redb's lock is per
*directory* and each peer is given its own, and Saathi listens on
`/ip4/0.0.0.0/tcp/0` — an ephemeral port — so two instances do not collide. The
word `isolated` is in the name because its absence is what cost the time.

The regression test is `two_isolated_handles_are_two_separate_runtimes` in
`comrade_jni`, and it was verified failing against the old constructor body
before the fix rather than assumed to. It runs in the ordinary `cargo test`
lane, in about a second.

**The uncomfortable part is the shape, not the bug.** The `OnceLock` landed
`2026fce` on 2026-07-29, for good reasons, and silently invalidated a test
written `7ce86e7` on 2026-07-15 whose whole premise was two independent
instances. Nothing failed. Nothing could have: the only lane that ran that test
was the one skipping itself. A process-global and a test that assumes
independent instances are a contradiction no compiler and no lint will report,
and this repo still has no guard for that class — only this write-up and the
regression test above.

### The fast twin, and why it exists

`a_run_of_transport_commands_arrives_in_the_order_it_was_sent` in
`crates/comrade_ui/tests/two_peer_integration.rs` asserts the same ordering
property hermetically, over the in-process relay, in about a second. The device
lane answers the same question through the real bindings over a real socket, in
about forty-five minutes.

The device lane also captures `logcat` and uploads it on failure, added after
that first red run could not be diagnosed from Gradle's output — three "nothing
arrived" assertions and no reason for any of them. Guessing at causes at fifteen
minutes a round is not a debugging loop worth having.

Both are worth having, and the fast one immediately proved it: **written first
asserting exact positions, it failed** — `30_000` arrived as `30_016`. That is
not a bug, it is `TogetherCommandDto::pos_ms`'s documented contract (a *playing*
command is carried forward through the message's flight time so the receiver
applies it as-is instead of compensating twice; a paused playhead does not
advance). Had that assertion gone straight into the device lane, the first red
build would have been the test's fault and would have cost a 45-minute round trip
to find out. Both now assert what the contract supports: play/pause exactly in
order, positions floored at what was sent and ceilinged by a plausible flight
time, and a paused position exactly equal.

### What this lane still does not cover

Worth being precise, because "two phones interacting" sounds like it covers
everything and this covers one layer of it:

- **Two processes, not two apps.** Both peers are `Comrade` objects in one test
  process. `TogetherManager`, `ShareTransfer` and `StreamTransfer` are Kotlin
  `object` singletons, so a second instance of the *session layer* cannot exist
  in the same process at all — which means the manager, its outbound queue and
  the foreground service are not in this lane. `build.gradle.kts`'s
  `deviceHarnessRole` is the mechanism for two installable app IDs; it is still
  unwired.
- **No WebRTC between two devices.** The streaming and file-handover paths need
  a media connection, and two emulators sit behind separate NATs with no route to
  each other — a pair would need a TURN server both can reach and a host-side
  orchestrator running leader and follower roles. That is the lane that would
  cover §17's *follower-never-plays* symptom directly, and it is not built here.
- **Nothing about the player.** No `MediaPlayer`, no audio, no cover, no screen.

So: the protocol and the bindings are now checked between two peers on every
push. The session layer and the media path are not, and no green tick in this
repo should be read as saying otherwise.

## 20. Search by name, and the tier that is now pluggable

_Added 2026-08-09, at the owner's request to make Together "primarily like
BlackHole" — a music player first, with listening together as a mode of it._

BlackHole's architecture is a client plus a metadata/search layer plus
third-party source adapters plus a downloader. Most of that already existed here
under different names, and the mapping is worth writing down because the gap was
much smaller than it looked:

| BlackHole | Comrade |
| --- | --- |
| Local library | `together/MusicLibrary.kt` (MediaStore paging, artwork, byte-sized LRU) |
| Player UI | `PlayerHome` · `LibraryBrowser` · `Transport` · `Cover`/`Sleeve` |
| Search / metadata | `comrade_core::catalogue` — **existed, tested, and reachable from nothing** |
| Source adapters | the four-tier ladder, §9 |
| Streaming / download | `media-http`'s guards; no downloader yet |
| Playlist DB, favourites, queue, history | **nothing at all** — still the real hole |
| Listen together | the session protocol, §§1–8 |

### The search layer was already written and wired to nothing

`catalogue.rs` had `CatalogueResolver`, a MusicBrainz adapter, `Recording` with
an ISRC field, `choose_audio_plan`, a licence gate, `MAX_CANDIDATES`, and its own
CI lane under `catalogue-http`. What it did not have was a single caller: no
`comrade_ui` method, no FFI export, no screen. A module can be fully tested and
still be dead code, and nothing in this repo notices — the same shape as §19's
skipping test, one layer up.

It is now reachable end to end: `comrade_ui::catalogue_lookup` (the one call that
touches a socket) and `comrade_ui::audio_plan` (pure), both exported over uniffi
and flutter_rust_bridge, behind Together's fourth source card.

**Both are free functions, not `ComradeRuntime` methods, and that is load-bearing
rather than stylistic.** A method's returned future borrows the guard, so a
wrapper *cannot* release the lock before awaiting a network round trip — the
shape of the two deadlocks this repo has already fixed. Making them free
functions removes the lock from the type system's point of view, so the mistake
is not available.

### "Cannot search" and "not found" are different sentences

`UiError::CatalogueUnavailable` exists because `catalogue-http` is off in the
lean test build, and a lookup there cannot reach a socket. Returning an empty
list would render as *"we searched and that song does not exist"* — a wrong
answer, silently produced, from a Cargo feature being off.

So the distinction is carried the whole way: a distinct `UiError` variant, a
distinct `ComradeCore.CatalogueResult.Unavailable` (which is why that is a sealed
interface and not a `Result`, since a `Result` collapses exactly this pair), a
distinct `TogetherDecisions.SearchOutcome.NoCatalogue`, and two different
strings. The JVM test `aBuildWithNoCatalogueIsNotTheSameAsARecordingThatDoesNotExist`
is what stops them merging back together.

Adding the two variants also broke two exhaustive matches on purpose — the Kotlin
`when` in `ComradeCore.humanMessage` and the Dart `switch` in
`describeUiError` — which is the mechanism `ComradeCore`'s own comment says it
wants ("adding a variant in Rust should break *this* compile"). It has now done
that once, which is the comment working rather than a nuisance.

### What a search can actually do today

The catalogue answers *what the recording is*. `audio_plan` then picks the tier,
and this screen can act on exactly one of the four: `Library`. The catalogue
supplies the proper title and artist, `LibraryResolver` searches `MediaStore` for
it — using `comrade_core::together::match_score`, **not** a second scorer in the
screen, which an earlier draft of this work did have and which is the drift
`TogetherDecisions`' header exists to prevent — and a session opens on the copy
already on the phone.

The other three tiers are named honestly and have no button behind them yet.
A search that finds the song but no local copy says *"found the song, but no copy
of it on this phone"* rather than opening the nearest thing: `MATCH_CONFIDENT`
exists so that guessing on somebody's behalf is not what happens.

### The extractor tier: a decision reversed, on the record

`catalogue.rs`'s header argued against a pluggable `AudioSource` trait at all, so
that a DRM tier "would have to be written from scratch by whoever wanted it". **On
2026-08-09 the owner decided to add the seam**, and that reversal is recorded in
that header rather than left as a contradiction between a comment and the code.

The original analysis is not deleted, because it is still why the tier ships
**pluggable and empty**: obtaining audio from a service that does not serve it
unencrypted means defeating a protection measure, which is a liability distinct
from infringement (DMCA §1201, EU InfoSoc Art. 6, India's Copyright Act §65A).
The seam is provided; a circumvention adapter is not, and is not going to be
written here. Whoever wants one writes it against the interface and owns that
decision, which is a better place for it than an absence that reads as an
oversight.

The maintenance argument stands on its own and is the practical reason to prefer
a separately-maintained extractor to bytes parsed here: an extractor depends on
internals its upstream may change without notice, so a hand-rolled one inherits
that tail forever. This is the thing that breaks while the playlists, the UI and
the player are all fine.

### What is still missing, plainly

- **Playlists, favourites, queue and history.** Nothing. This is the half that
  makes it a music player rather than a session tool, and it has no storage, no
  FFI and no screen.
- **A downloader and ID3 tagging.** The `OpenLicence` tier decides that a fetch
  is permitted; nothing carries it out.
- **Adapters for the pluggable tier.** The seam is the deliverable here; the
  self-hosted sources worth having behind it (Subsonic/Navidrome/Jellyfin,
  Funkwhale, open-licence archives) are not written.
- **A second catalogue.** `MUSICBRAINZ` is a constant in `TogetherScreen.kt`
  because `CatalogueMatch` carries no source field. Adding a second resolver makes
  that constant a lie, and the field has to move onto the match — stated as a
  named constant with that comment rather than an inlined string so the next
  person finds it.

## 21. Streaming and downloading, BlackHole's other two halves

_Added 2026-08-09, completing §20's "what is still missing" for the media path._

### Streaming already worked; the stall did not show

`MediaPlayer.setDataSource(context, Uri)` on an HTTPS URL **is** progressive HTTP
— the same model BlackHole uses — and `TogetherContent::Stream` has driven it on
both sides of a session since §14. So there was no streaming to add.

What was missing is that a stall was invisible. `MEDIA_INFO_BUFFERING_START` /
`_END` were not wired to anything, so a stream that ran out of bytes looked
exactly like one that had broken: the transport still said "playing", the
playhead stopped, and the only visible consequence was **the drift line growing**
— the session reporting a sync problem whose actual cause was a network stall.

`UiState.Live.buffering` now carries it, and the screen shows it **above** the
drift line rather than below. That ordering is the point: the stall is the cause
of the gap the next line is about to report, and the other order reads as a sync
fault.

Nothing pauses or corrects on a stall, deliberately. A stall that clears in 300 ms
must not become a command the other side has to apply, and the drift ladder
already handles a playhead that has stopped moving. `MediaPlayer` may also end a
stall with no second event at all, so this is shown and never waited on.

### Downloading: the licence gate is a type, not a check

`comrade_core::download` is the "who carries it out" §9's tier table left open for
`SourceTier::OpenLicence`.

**`fetch_track` does not take a URL.** It takes a `PermittedDownload`, and the
only way to obtain one is `permit_download`, which refuses a metadata-only
answer, an undeclared licence, and a non-HTTPS URL. There is therefore no call
shape that downloads an arbitrary string, and no ordering bug where the bytes
arrive before the check — §9's *"licence checking happens before the fetch, not
after"* is a compile-time property here instead of a convention. `EmbedOnly` has
no path through the module at all.

That is the whole difference between this and the "download button that works
until somebody's lawyer notices" `catalogue.rs` warns about. `download_track` on
the FFI **re-runs the gate itself** rather than trusting that the UI asked
`download_verdict` first, so a frontend that skipped the verdict still cannot get
past it.

Guards are `media.rs`'s `fetch_guarded_bytes`, not a second opinion: HTTPS only
and fail-closed on any other or missing scheme, redirects disabled, a connect
timeout separate from the transfer budget, an audio content-type allowlist checked
before any body is buffered, and the cap checked against `Content-Length` **and
again while the body streams** — a cap that only reads the header is not a cap.

The filename is where a third party's strings stop being dangerous, because artist
and title come from a catalogue's JSON: separators, control characters and the
punctuation Windows rejects are removed, leading dots trimmed, the stem truncated
on a character boundary with the extension kept intact (a media scanner keys on
the extension), and a name that sanitises to nothing becomes `track` rather than
`.mp3`. `../../etc/passwd` as a title is a test case.

### Why the download goes into `MediaStore` and not app storage

`together/MusicDownloads.kt` writes through `MediaStore.Audio`, which is what
makes a downloaded track *a track*: visible to `MusicLibrary`, visible to every
other music app on the phone, and surviving uninstall. App-private storage would
have been shorter and would have made the download invisible. It also needs no
storage permission on API 29+ — an app may always insert its own media — which
matters because the library *read* permission is separately refusable and a
refusal must not cost the download.

`IS_PENDING` is set while writing and cleared afterwards, and that is not
optional: without it the media scanner can index a half-written file, and the
result is not a missing track but a **corrupt** one appearing in every music app
with the right name and a broken decode. A failed download deletes its own row,
because a pending row is invisible but still occupies the name — leaving one
would make the retry report "you already have this" for a file that was never
written.

Three outcomes, three sentences: `Saved`, `AlreadyThere`, `Failed`.
`AlreadyThere` is its own case rather than a failure because "you already have
this" is not one — the same argument as `ComradeCore.CatalogueResult`, and the
same argument §20 made for `NoCatalogue` versus `NothingFound`.

A row with no permitted download says **why** rather than showing a disabled
button: `NoAudio` ("this catalogue only knows the name") is the ordinary answer,
since MusicBrainz serves no audio at all. A greyed-out button invites a tap and
then explains a licence, which is worse than not offering one.

### What is still missing

- **In-file tags.** No ID3/Vorbis/MP4 frames are written. `id3` is MP3-only while
  archives serve FLAC, Ogg and M4A just as often, so it would tag one format in
  four and silently skip the rest. `MediaStore`'s `TITLE`/`ARTIST`/`ALBUM`
  columns make a download read correctly **on the phone**; a file copied off the
  device carries only whatever tags the archive already put in it. Closing this
  properly needs a multi-format writer (`lofty`-shaped).
- **The whole track is buffered in memory.** `MAX_TRACK_BYTES` is 96 MB to
  accommodate lossless album tracks, so a worst case is a 96 MB `ByteArray`
  crossing the FFI. Typical tracks are 3–12 MB. Streaming to a caller-supplied
  path is the fix; it is not done because it puts filesystem code in the core for
  a case no archive this serves has hit.
- **One download at a time**, by construction — the button is hidden while
  another is in flight. Not a queue, and there is no resume: a failed download
  starts over.
- **Adapters for the pluggable tier.** §20's seam is still empty.
- **Playlists, favourites, queue and history.** Still nothing at all, and still
  the largest gap between this and a music player.

## 22. Browsing a collection, and a tap that plays

_Added 2026-08-14, owner request: the library as a grid of covers, and a tap that
listens alone rather than asking who with._

### A flat list of two thousand tracks is a search result, not a library

`MusicLibrary.page` reads `MediaStore` ordered by **track** title, and the
browser drew exactly that: one record's third song next to a different record's
third song, in one column, 48 dp thumbnail on the left. It is the right shape for
a result set and the wrong one for a collection — the only way to find an album
in it is to remember the name of something on it, and the cover, which is the one
piece of metadata a phone reliably has, was too small to recognise.

So the browse surface is albums, and the flat list is what a **query** produces.
`TogetherDecisions.browse` is the single place that chooses:

| Typed | Shown |
| --- | --- |
| nothing | `Browse.Albums` — `albumsOf(tracks)`, a grid of covers |
| anything | `Browse.Tracks` — `filterTracks`, exactly as before |

Typing switches to the list because a search is for a song at least as often as
for a record, and a grid of covers cannot show *which* of an album's tracks
matched. That split also pays for itself twice: the grid is only ever the whole
library, so it never re-ranks under a moving finger, and the two "nothing"
sentences — "no music on this phone" versus "nothing matches that" — stop being
two conditions read off one list and fall out of which view came back.

**The grouping is the part worth arguing with**, and it is all in the pure file:

- **The album id is the key; the title is only the fallback.** Two records can
  share a name and `MediaStore` is the only thing that knows they are different.
  Where there is no id, the lowercased title is the *whole* key — so two untagged
  records called *Greatest Hits* merge. Accepted rather than fixed by adding the
  artist: the id-less path is the untagged remainder, and splitting *that* by
  artist would scatter a compilation nobody tagged into one tile per guest.
- **Leftovers are decided per *group*, not per row**, and getting that backwards
  costs a whole record. One file in a rip losing its `ALBUM` tag is ordinary — a
  re-encode, an edit by another app — and treating that row as unalbumed takes it
  out of its own record: a tile reading "11 tracks", a queue that can never reach
  the twelfth, and the twelfth alone at the bottom of the library.
  `MediaStore` still groups it by `ALBUM_ID`, so it stays there and the group is
  asked for the name. Only a group where *nobody* named a record is leftovers,
  and folding all of those into one is what lets `Album.title` promise `null` for
  exactly one group — an id is a number in a column, not evidence that anything
  was tagged, and two untitled tiles would be two rows headed the same thing.
  Both halves are tests. (Whether `MediaStore` on a current device really returns
  a null `ALBUM` for such a file, or substitutes the folder name, is not
  something this sandbox can answer; if it substitutes, this is a no-op rather
  than a fix.)
- **Alphabetical by album, not the order the tracks arrived in.** The opposite of
  `filterTracks`' rule, for the same underlying reason — this list is not
  recomputed under a finger, so ordering helps here where it would jump there.
  Grouping alone would have sorted records by whichever of their songs came first
  in the alphabet, which reads as no order at all. Inside a record the provider's
  order is kept, which is title order and **not** track order: `TRACK` is not in
  the projection, and that is the one thing that would make an album read the way
  the record does.
- **Tracks that name no album are kept, in one group, last.** A phone full of
  untagged rips would otherwise browse as an empty library. The group's `title` is
  `null` and the screen names it, because that file has no strings in it. That
  every track lands in exactly one album is pinned by a test rather than left to
  reading: a grouping that silently dropped what it could not classify would make
  part of someone's music unreachable, and the only symptom would be a library
  that looks smaller than it is.
- **"Various artists" and "nobody said" are different answers.**
  `AlbumArtist.One` / `Various` / `Unknown`, three arms rather than two. A tile
  reading *Various artists* over four untagged rips invents the one fact the files
  withheld, so `Unknown` says nothing at all instead.

### Grouping a page that was cut needs different sentences

`MusicLibrary.page` stops at 2,000 rows, and the flat list made that legible:
2,000 titles and one note at the bottom. Grouping does not, because **the cut
falls in track-title order across the whole library rather than at an album
boundary** — a twelve-track record whose last five titles sort past row 2,000 is
present with seven of them. So while `Page.truncated` is set, a tile says *"at
least 7 tracks"*: the count is a fact about the page and stating it as a fact
about the record is the kind of small confident lie this feature is written to
avoid. And inside an open record the note is a different sentence again, because
the ordinary one ends "search to find the rest" and the search field is
deliberately not drawn there — it names the library screen instead of an action
that is not on this one.

### The cover cache was size-blind, and the grid is what made it visible

`MusicLibrary.artwork` keyed on the album id alone. With one request size that is
invisible; with three — a 48 dp row, a 144 dp tile, the 320 dp sleeve — whichever
was decoded first was handed to all three. Browsing rows and then opening a
record drew the sleeve from a 48 px thumbnail, upscaled; going the other way held
a sleeve-sized bitmap for every row. The key now carries the size, and the budget
went from 4 MB to 12: a 144 dp tile at 3× is a 432 px square, about 750 kB as
`ARGB_8888`, so a screenful of six is 4.5 MB and the old budget could not hold
even one screen — it re-decoded every tile it had just evicted. Neither half of
that reads as a caching bug from a screenshot, which is why it is written down
here rather than only in the diff.

### A tap plays. Asking who with is something you ask for

§18 made a session with nobody in it a first-class pairing, so the tab works as
an ordinary music player once something is playing. It still opened with a
question: choose a song, and *"and who with?"* came up before a note played.
That is the same insistence §18 removed, one screen earlier — a music player
that will not play until you have named a friend.

So `startStepInLibrary` is the library's rule:

| Paired with | Person button | Answer |
| --- | --- | --- |
| nobody yet | not armed | `PlayNow(ALONE)` — **the change** |
| nobody yet | armed | `AskWho` — the sheet, as before |
| `ALONE` | armed | `AskWho` |
| `ALONE` | not armed | `PlayNow(ALONE)` |
| a person | either | `startStep`'s answer, unchanged |

**Only the library's rule changed.** A pasted link and a picked file are gestures
aimed at somebody — *watch this with me* — so those routes still ask through
`startStep`, and so does a catalogue search, which produces the same
`TogetherDecisions.Track` and is not the library. `Chosen.Track` carries
`fromLibrary` rather than the screen inferring it from the type.

Two things this had to avoid being:

- **A one-way door.** `startStep` answers `PlayNow` for a pairing that is already
  `ALONE`, so honouring the button only when nobody is chosen would have left a
  solo session with no route to a shared one short of ending it. It is honoured
  whenever `mayChoosePerson` is true, which is *nobody yet or nobody in it*.
- **A button that lies.** Once a real person is in the session §16's rule holds —
  the person is chosen once per session, not once per track — so there is nobody
  left to offer and the button is not drawn. Both its visibility and the routing
  ask `mayChoosePerson`, the same doubled question `PlaybackModeDecision.
  ownershipFor` is asked, so they cannot disagree. A flag left armed from before
  cannot smuggle the sheet back in and swap a peer mid-session; there is a test
  for exactly that.

The armed state survives a dismissed sheet — closing it without picking anybody
is "not them", not "never mind" — and clears when a session actually starts, so
the tap *after* the one that asked does not ask again.

### What is checked here, and what is not

The grouping, the ordering, the three artist answers, the leftovers, the
every-track-lands-somewhere property and all five rows of the table above are
`TogetherDecisionsTest`: **119 tests, green in this sandbox** in about a minute,
up from 105. Each new behaviour was checked by removing it and watching the test
go red, because a test that cannot fail is not a test.

Two things the grid needed that only a reviewer catches: the grid's
`LazyGridState` is hoisted out of the `when`, because opening a record removes the
grid from the composition and a state remembered inside it is discarded — coming
back out would land at the top of a library somebody had scrolled halfway down.
And the record level carries its own `BackHandler`: `MainActivity`'s single one
does not cover the Together tab at all, so the gesture people actually reach for
inside a drill-in would otherwise leave the screen entirely.

`TogetherScreen.kt` and `MusicLibrary.kt` **type-check** here — the Compose lane
resolves all 128 sources against real Compose 1.6.1 and Material3 1.2.0, and `R`
is generated from the real resource files, so the new plural and the two new
strings are checked to exist. That is the whole of what has been verified: no
device has drawn this grid, no `MediaStore` has answered it, and nothing here has
run a `LazyVerticalGrid` or decoded a cover. How it *looks* — tile size, how the
adaptive column count lands on a real 360 dp phone, whether 144 dp of cover is
enough — is CI's and then a handset's.

## 23. Streaming from your own server, and the player's own library

_Added 2026-08-24, owner request: the Together tab as a full music player that
streams online, in the way §20 said it could be filled._

### The source: Subsonic/Navidrome, and why that is the online streaming here

§20 left the pluggable tier pluggable and empty. It is now filled — by the one
shape of online source this app can carry without touching the line
`catalogue.rs`'s header draws: a **self-hosted Subsonic-compatible server**
(Navidrome, Gonic, Airsonic). The user points the app at a server they own;
`comrade_core::subsonic` searches its `search3`, builds `/rest/stream` URLs,
and every URL is run through `together::valid_stream_url` **inside core** before
a candidate exists. What reaches the screen is only what a Together session
will accept unchanged — the guard runs at construction, not again at the edge,
because a filter total at one layer must not depend on another layer remembering.

The auth is Subsonic's salted token (`md5(password + salt)`), implemented in
~90 inline lines pinned to the RFC 1321 suite rather than added as a
dependency — the same call `urlencode` made. The reason token auth matters more
here than in an ordinary client: **the stream URL travels to the peer** in a
Together invitation. A per-request token lets them fetch that file, which is
what being invited means; the password itself never leaves the phone, and the
source card says so before anything is shared.

What was *not* written, still: adapters that defeat a protection measure. That
decision of §20's stands; this module exists because Subsonic serves plain
progressive HTTPS by design, which is also why such sessions earn the fine
sync ladder (`TogetherContent::tuning`) — the same player, the same accurate
positions, the same rate-trimmable corrections as a local file.

### The second catalogue: Jamendo

`parse_jamendo` gives the by-name search a second resolver behind
[`CatalogueResolver`], and `CatalogueMatch` now carries a `source` field — the
exact move §20 predicted when it called TogetherScreen's `MUSICBRAINZ`
constant "a lie waiting to happen". The constant is gone; the row names the
catalogue that answered.

Jamendo declares a Creative Commons licence per track and serves direct files,
so for the first time [`choose_audio_plan`]'s OpenLicence tier has something
true to check: its rows get working download buttons through §21's gate with
zero new policy. Where an artist switched `audiodownload_allowed` off, the
match carries the stream URL but fails `is_openly_fetchable` — serving is not
licensing, now enforced end to end.

### The player library: favourites, history, playlists, queue

Four vault trees (`player_*`) and fifteen FFI methods close what §20 called
"nothing at all":

- **Favourites** keyed by track key; toggle answers what it now is.
- **History** is one entry per track, timestamp updated in place, capped at
  100 — recently played, not a diary of plays.
- **Playlists** are named ordered lists; removing by key takes out every copy,
  because the key is the identity and "one of the two" is not a request the
  API can understand.
- **Queue snapshots** save on pause and on leave (Android's `saveQueueSnapshot`),
  so the resume point survives.

All of it is vault-backed and therefore answers `VaultLocked` while locked.
That is deliberate and worth defending once more than usual: what somebody
plays and loves is diary data, and the app's existing bar for reading diaries
is an unlocked vault. An empty library would have been easier and would lie.

### Player extras: shuffle, repeat, sleep, speed, EQ, lyrics

The BlackHole-shaped control set, each landed where it is *true*:

- **Shuffle** keeps the current track first (`TogetherDecisions.shuffledOrder`,
  Fisher-Yates on an injected Random so tests pin orders); next and previous
  follow the order, not the files.
- **Repeat** OFF/ALL/ONE answers only what happens when a track ends by
  itself — a manual skip under repeat-one still moves, which is
  `manualNextIndex` and its own test.
- **Auto-advance is solo-only by definition**: a leader's completion is a
  playhead fact for the follower to follow, and both devices deciding would
  race.
- **Speed is solo-only** for a sharper reason: the correction ladder trims
  rate continuously to hold two playheads together; a user multiplier under
  that would fight it. `speedAllowed` gates the control to where nobody is
  following.
- **Sleep timer** pauses via `setState`, so both devices stop together.
- **Equalizer** attaches per audio session, only where our decoder mixes the
  sound (never an embed or a followed app); levels stored in millibels, the
  device's own unit, so no layer converts anything.
- **Lyrics** come from LRCLIB (keyless), parsed twice - Rust and Kotlin -
  pinned by shared fixtures; the highlight is the last line that started.

### Public collections: Internet Archive and podcast feeds

Two more keyless sources behind one card. Archive search leads to an item's
guarded file list (`direct_media_url` passes - these name real files); a
pasted feed URL yields every episode as a candidate. Every URL built here
passed the peer guard inside core before it was a row. Jellyfin users are
covered as well, through Jellyfin's official Subsonic-compatibility plugin
and the existing server adapter.

### What is still missing

- **Playlist editing depth** - create/open/play/remove shipped; reordering
  within a playlist did not.
- **Desktop and Flutter reach.** Android-first per the standing directive;
  desktop's panel can call the same free functions, and Flutter needs the frb
  mirror regenerated (`flutter_rust_bridge_codegen generate`) - deliberately
  not done blind, since codegen cannot run where Dart cannot build.
- **Adapters that defeat protection measures.** Still not written, still on
  purpose - repeating the request does not change what such code *is*.
  Spotify means Widevine; JioSaavn's URLs mean their secret DES key; YouTube
  Music means InnerTube extraction. The sanctioned set stands instead: your
  server, public collections, podcasts, open licences, embeds, and an
  external-app follow mode.
