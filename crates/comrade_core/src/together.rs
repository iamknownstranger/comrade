/*!
 * Together — listening and watching in step, over the DM channel we already have.
 *
 * `AUDIT.md` §8.2 asks for shared media consumption with a loved one. The
 * content problem is the real constraint, not the sync problem: re-streaming or
 * proxying licensed audio/video between two people is both a copyright problem
 * and a bandwidth problem, and DRM'd platform content cannot be frame-synced
 * inside our app at all. So Comrade moves **no media**. Each side plays *their
 * own copy* — a local file they already have, or the same public video — and all
 * that travels is a small control envelope saying where the playhead is.
 *
 * That envelope rides the same NIP-44/NIP-17 gift-wrapped DM channel as
 * receipts, profile shares, call signals, presence beacons and nudges
 * ([`crate::dm`], [`crate::call`], [`crate::presence`], [`crate::nudge`]).
 *
 * Pure and framework-free like its neighbours: the wire shape, the clock
 * arithmetic and every timing rule live here and are unit-tested. The players,
 * the session bookkeeping and the UI live in `comrade_ui` and the frontends.
 *
 * ## The clock problem, and why the heartbeat is the answer
 * To compare two playheads you need to know how far apart the two *clocks* are.
 * Nothing in the transport tells you: a gift wrap's outer timestamp is
 * deliberately randomised, and the rumor's own timestamp is the sender's claim,
 * in whole seconds. So each heartbeat carries [`ClockEcho`] — the `at_ms` of the
 * last message we heard and our clock when we heard it — which turns the
 * heartbeat we were already sending into the classic four-timestamp NTP probe at
 * **zero extra messages**. [`ClockFilter`] keeps the recent probes and reports
 * the one with the lowest round trip, because the least-queued sample is the one
 * whose symmetry assumption holds best.
 *
 * ## What is actually achievable — §8.2's "< 300 ms" is not
 * Measuring the offset also measures how *wrong* it might be
 * ([`ClockEstimate::uncertainty_ms`]), and the rule that falls out is the one
 * this module is built around: **never correct by less than your own measurement
 * error** ([`sync_verdict`], rule 6). Half the round trip of a public relay
 * exceeds 300 ms in the common case, so a 300 ms target sits *below the noise
 * floor of the measurement* — we could not tell a 300 ms error from zero, let
 * alone correct it. What this design does hold, honestly:
 *
 * | | typical steady-state drift |
 * |---|---|
 * | local file (rate trim available) | ±0.3–0.8 s |
 * | an embedded player that only seeks | ±1.2–2.5 s |
 * | over a future WebRTC data channel (§8.1) | ±20–60 ms plausible |
 *
 * The clock estimate is an *input* to [`sync_verdict`], not baked into it, so a
 * lower-latency transport tightens all of this with no policy change.
 *
 * One more honesty point, because it decides what the UI may claim: **listening
 * together is perceptually much harder than watching together.** Two people in
 * one room half a second apart is unlistenable; two people in different cities
 * half a second apart are fine. The goal here is reacting to the same moment
 * together — laughing at the same joke inside a second — not phase-locked audio,
 * and no UI on top of this should imply otherwise.
 */

use std::collections::VecDeque;

use rand::RngCore;
use serde::{Deserialize, Serialize};

/// Envelope marker for a together payload riding inside a DM body — the same
/// detection pattern [`crate::call::CALL_ENVELOPE_MARKER`] and
/// [`crate::presence::PRESENCE_ENVELOPE_MARKER`] use, so
/// [`parse_together_envelope`] accepts only its own shape and the inbox
/// dispatcher can fall through to its other handlers.
pub const TOGETHER_ENVELOPE_MARKER: u8 = 1;

// ── Cadence and lifetime ─────────────────────────────────────────────────────

/// How often a playing session tells the other side where it is.
///
/// Every heartbeat is a persistent gift-wrapped event, and the vault inbox
/// rewinds two days on **every** launch, so this number decides how much each
/// later app start re-downloads and re-decrypts — that, not bandwidth, is the
/// binding cost. Ten seconds is the balance: crystal drift between two players
/// is negligible (~50 ppm, well under a second across a whole film), so the
/// heartbeat is not there to steer — it is there to notice a stall or a lost
/// command, and to keep [`ClockFilter`] supplied with probes. Slower than ~15 s
/// starves the filter; faster buys nothing the correction window would use.
pub const TOGETHER_HEARTBEAT_SECS: u64 = 10;

/// How long a session survives hearing nothing before we call it over.
///
/// Deliberately more than four heartbeats, so a couple of dropped or slow
/// beacons cannot end a session someone is still watching. A phone that dies
/// mid-film sends no goodbye, which is what this exists for.
pub const TOGETHER_SESSION_TTL_SECS: u64 = 45;

/// [`TOGETHER_SESSION_TTL_SECS`] in milliseconds — the session clock is
/// millisecond-based throughout, and one conversion in one place beats the same
/// multiplication scattered over three frontends.
pub const TOGETHER_SESSION_TTL_MS: u64 = TOGETHER_SESSION_TTL_SECS * 1000;

/// How long a direct peer channel may carry nothing back before we stop
/// believing it is carrying anything at all.
///
/// Two heartbeats. The frontend owns the socket and is *supposed* to report
/// `false` the moment it closes, but a report that never comes is exactly the
/// failure this guards: signals leave for a socket nobody reads, and the first
/// thing that notices is the session's own TTL — which ends the evening rather
/// than falling back to the relay that was there all along.
///
/// The number is derived from the cadence rather than picked, because that is
/// what makes it correct: a live channel carries the peer's heartbeat every
/// [`TOGETHER_HEARTBEAT_SECS`], so two of them passing in silence means at least
/// one was lost. It has to sit comfortably under
/// [`TOGETHER_SESSION_TTL_SECS`] as well, or the fallback would arrive after the
/// session it was meant to save — 20 s against 45 s leaves a full heartbeat on
/// the relay to refresh the TTL before it lapses.
pub const TOGETHER_DIRECT_SILENCE_MS: u64 = TOGETHER_HEARTBEAT_SECS * 2 * 1000;

/// A together signal older than this is meaningless and is dropped on arrival.
///
/// Relays redeliver at-least-once and the vault inbox backfills up to two days
/// on every launch. This is the **only** guard that protects
/// [`TogetherSignal::Start`], which is the sole signal that can create session
/// state from nothing — every other signal additionally needs a live session
/// with a matching id, and sessions never outlive the process.
///
/// Tighter than the call channel's 90 s on purpose: a replayed call offer
/// produces a ring a human can decline, while a replayed playhead moves someone
/// else's player with no confirmation step. A more disruptive failure earns a
/// tighter bound. Not tighter still, because a phone that briefly lost signal
/// must be able to drain its inbox on reconnect without every in-flight command
/// being discarded.
pub const TOGETHER_SIGNAL_MAX_AGE_SECS: u64 = 60;

/// The floor on how often a command goes out while someone drags a scrubber.
/// A drag emits positions at ~10 Hz; sending all of them would be a burst of
/// gift wraps describing a position the user never stopped at.
pub const TOGETHER_COMMAND_MIN_INTERVAL_MS: u64 = 400;

// ── The drift ladder ─────────────────────────────────────────────────────────

/// Deadband floor for a player with an accurate clock and a free playback rate
/// (an HTML5 media element on a local file).
pub const TOGETHER_DEADBAND_FINE_MS: u64 = 250;

/// Deadband floor for a player that reports position coarsely, cannot be trimmed
/// to an arbitrary rate, and rebuffers visibly on a seek — an embedded
/// third-party player. Correcting such a player often is worse than the drift.
pub const TOGETHER_DEADBAND_COARSE_MS: u64 = 1200;

/// Past this, closing the gap by trimming the rate would take longer than the
/// gap is worth; jump instead.
pub const TOGETHER_SEEK_THRESHOLD_MS: u64 = 2500;

/// How much the playback rate may be trimmed, either way. Four percent is under
/// the point where a pitch shift is noticeable, and invisible on video.
pub const TOGETHER_MAX_TRIM: f64 = 0.04;

/// The window a rate trim aims to close the gap over. Larger means gentler.
pub const TOGETHER_CORRECTION_WINDOW_MS: u64 = 10_000;

/// The floor between two automatic seeks. Seeking is the most disruptive thing
/// this feature can do to someone who is watching; doing it twice inside a few
/// seconds is worse than being a couple of seconds out.
pub const TOGETHER_MIN_SEEK_INTERVAL_MS: u64 = 5_000;

/// How far ahead a peer may schedule a command before we stop trusting it and
/// just apply it. Comfortably past a local-mesh round trip, nowhere near long
/// enough to be worth holding a timer for on a stranger's say-so.
pub const TOGETHER_MAX_SCHEDULE_MS: u64 = 2_000;

/// The uncertainty we assume before any round trip has been measured.
/// Deliberately pessimistic and deliberately **not zero**: "we have not
/// measured" must never read as "we are certain".
pub const TOGETHER_UNMEASURED_UNCERTAINTY_MS: u64 = 1_500;

/// How long a clock probe stays usable. Long enough to hold a dozen probes at
/// the heartbeat cadence, short enough that an NTP step on either device ages
/// out of the estimate within two windows.
pub const CLOCK_PROBE_WINDOW_MS: u64 = 120_000;

/// How many probes to gather before the clock is treated as converged.
///
/// Adopted from beatsync (github.com/freeman-jiang/beatsync), which bursts up
/// to 40 measurements at join. The problem it solves is real and this design had
/// it: at one probe per ten-second heartbeat, the first *minute* of a session
/// runs on [`TOGETHER_UNMEASURED_UNCERTAINTY_MS`] — a deliberately pessimistic
/// guess — so the two playheads are at their furthest apart exactly when both
/// people are looking. Eight is fewer than beatsync's forty because each probe
/// here is a persistent gift-wrapped event rather than a WebSocket frame, and
/// eight is enough for the best-half filter to have something to choose from.
pub const CLOCK_BURST_PROBES: usize = 8;

/// The heartbeat interval while bursting. Eight probes at this spacing converges
/// the clock in about four seconds instead of eighty, for eight extra events
/// once per session — a trade worth making, and the only place this design
/// deliberately spends relay traffic.
pub const TOGETHER_BURST_INTERVAL_MS: u64 = 500;

/// How many probes a frequency estimate needs before it means anything.
pub const CLOCK_MIN_SKEW_PROBES: usize = 4;

/// And how long they must span. Below this the slope is mostly network jitter:
/// a 1 ms jitter over a 10 s baseline reads as 100 ppm, which is five times any
/// real crystal error.
pub const CLOCK_MIN_SKEW_SPAN_MS: u64 = 45_000;

/// The ceiling on a believable frequency difference. Consumer crystals are
/// specified to a few tens of ppm; anything past this is a clock being *stepped*
/// (NTP correcting, someone changing the time), which is a phase event and must
/// not be re-read as a frequency and then extrapolated forever.
pub const CLOCK_MAX_SKEW_PPM: f64 = 200.0;

/// The frequency error assumed to survive the regression, used to grow the
/// uncertainty of a stale estimate. At 5 ppm a minute-old estimate has drifted
/// 0.3 ms — small, but it is the term that used to be silently zero.
pub const CLOCK_RESIDUAL_SKEW_PPM: f64 = 5.0;

/// How many probes [`ClockFilter`] keeps. The minimum filter needs candidates;
/// it does not need history.
pub const CLOCK_PROBE_RING: usize = 8;

// The invariants the constants above exist for, enforced at compile time.
// A session must survive several dropped heartbeats; the age gate must be wider
// than the TTL so the TTL stays the *single* authority on when a session ends
// (two rules disagreeing about that is how a UI ends up claiming someone is
// still watching); and the ladder has to be ordered or a verdict is unreachable.
const _: () = assert!(TOGETHER_SESSION_TTL_SECS > 4 * TOGETHER_HEARTBEAT_SECS);
const _: () = assert!(TOGETHER_SIGNAL_MAX_AGE_SECS > TOGETHER_SESSION_TTL_SECS);
const _: () = assert!(TOGETHER_SEEK_THRESHOLD_MS > TOGETHER_DEADBAND_COARSE_MS);
const _: () = assert!(TOGETHER_DEADBAND_COARSE_MS > TOGETHER_DEADBAND_FINE_MS);
// The burst must actually be a burst and the tail an actual tail; a change that
// quietly collapsed them into each other would otherwise cost either the first
// minute of every session or a two-hour film's worth of persistent events.
const _: () = assert!(TOGETHER_BURST_INTERVAL_MS * 10 < TOGETHER_HEARTBEAT_SECS * 1000);
// The direct-channel watchdog has to fire with a whole heartbeat to spare, or
// the relay it falls back to gets no chance to refresh the TTL before the
// session it was meant to save has already lapsed.
const _: () =
    assert!(TOGETHER_DIRECT_SILENCE_MS + TOGETHER_HEARTBEAT_SECS * 1000 < TOGETHER_SESSION_TTL_MS);

// ── What is being played ─────────────────────────────────────────────────────

/// What is being played, named the way a catalogue names it.
///
/// The idea, and specifically the ISRC-first shape, is adapted from
/// [Antra](https://github.com/anandprtp/Antra), which uses the International
/// Standard Recording Code to guarantee an exact-recording match and falls back
/// to a scored title/artist similarity when there isn't one. (Only that idea:
/// Antra is a downloader, and acquiring content is not something this app does.)
///
/// It is a **better** answer than the content hash this module deliberately does
/// not send, on both axes at once:
///
/// - *More useful.* A hash answers "is this the same bytes", which is not the
///   question — two people can be perfectly in step on different rips of the
///   same recording. An ISRC answers "is this the same recording", which is.
/// - *Less revealing.* A hash fingerprints the exact artefact someone holds —
///   which rip, which release group, which personal copy. An ISRC is public
///   catalogue data about a commercial release and says nothing about the file.
///
/// Every field is what the sender chose to disclose about something they are
/// already telling this person they are listening to. There is still no
/// filename, no path, no size and no digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct Recording {
    /// International Standard Recording Code, when known — the exact-match key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isrc: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub artist: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
}

impl Recording {
    /// Just a name, for something with no catalogue entry — a home video, a
    /// lecture recording, a file whose tags are empty.
    pub fn titled(title: impl Into<String>) -> Self {
        Self {
            isrc: None,
            title: title.into(),
            artist: String::new(),
            album: None,
        }
    }

    /// A one-line label for a UI that has nowhere to put the structure.
    pub fn label(&self) -> String {
        if self.artist.is_empty() {
            self.title.clone()
        } else {
            format!("{} — {}", self.artist, self.title)
        }
    }
}

/// Whether `s` is a well-formed ISRC: two-letter country, three-character
/// registrant, two-digit year, five-digit designation — `CCXXXYYNNNNN`.
/// Hyphens are accepted and ignored, because catalogues print it both ways.
pub fn valid_isrc(s: &str) -> bool {
    let bytes: Vec<u8> = s
        .bytes()
        .filter(|b| *b != b'-')
        .map(|b| b.to_ascii_uppercase())
        .collect();
    bytes.len() == 12
        && bytes[0..2].iter().all(|b| b.is_ascii_alphabetic())
        && bytes[2..5].iter().all(|b| b.is_ascii_alphanumeric())
        && bytes[5..12].iter().all(|b| b.is_ascii_digit())
}

/// Normalise an ISRC to its canonical, hyphen-free upper-case form.
pub fn normalise_isrc(s: &str) -> Option<String> {
    valid_isrc(s).then(|| {
        s.bytes()
            .filter(|b| *b != b'-')
            .map(|b| b.to_ascii_uppercase() as char)
            .collect()
    })
}

/// How well a candidate in the listener's own library matches what the peer
/// named, from 0.0 (no) to 1.0 (certainly).
///
/// Antra's fallback, and the ordering matters: an ISRC agreement is decisive and
/// nothing else needs consulting, because that is precisely what the code is
/// for. Without one it is a weighted title/artist comparison with duration as a
/// tiebreak — title carries most of the signal, artist disambiguates covers, and
/// duration separates the radio edit from the album version.
///
/// Deliberately conservative: this decides whether to open a file on someone's
/// behalf, and opening the wrong one is worse than asking.
pub fn match_score(want: &Recording, have: &Recording, want_ms: u64, have_ms: u64) -> f64 {
    if let (Some(a), Some(b)) = (&want.isrc, &have.isrc) {
        if let (Some(a), Some(b)) = (normalise_isrc(a), normalise_isrc(b)) {
            // Two ISRCs that disagree are a definite *no*, not a weak maybe:
            // they are different recordings and the catalogue says so.
            return if a == b { 1.0 } else { 0.0 };
        }
    }
    let title = title_similarity(&want.title, &have.title);
    let artist = if want.artist.is_empty() || have.artist.is_empty() {
        // Nothing to compare is not evidence either way, so it neither helps nor
        // hurts — it just leaves the decision to the title.
        title
    } else {
        title_similarity(&want.artist, &have.artist)
    };
    let known = want_ms > 0 && have_ms > 0;
    let delta = want_ms.abs_diff(have_ms);
    let duration = if known {
        (1.0 - delta as f64 / MATCH_DURATION_TOLERANCE_MS as f64).clamp(0.0, 1.0)
    } else {
        0.5
    };
    let score = 0.55 * title + 0.30 * artist + 0.15 * duration;

    // A length that disagrees by more than the tolerance is a veto, not a
    // deduction. Same title, same artist, two minutes shorter is the radio edit
    // — a different recording, and opening it on someone's behalf would put the
    // two of them on different cuts while the UI claimed they were together.
    if known && delta > MATCH_DURATION_TOLERANCE_MS {
        return score.min(MATCH_CONFIDENT - 0.2);
    }
    score
}

/// Words that mean "a different recording of the same song", as opposed to a
/// different *pressing* of the same recording.
///
/// This is Antra's "radio edits and censored versions are penalised" rule,
/// narrowed to the cases where it is unambiguous. "Remastered", "Deluxe" or a
/// year in brackets describe the same performance and should barely cost
/// anything; a live take or a remix is genuinely not what the other person is
/// listening to.
const VERSION_WORDS: [&str; 7] = [
    "live",
    "remix",
    "acoustic",
    "instrumental",
    "karaoke",
    "cover",
    "demo",
];

/// Above this a library candidate may be opened without asking; below it the
/// listener picks. Set high on purpose — see [`match_score`].
pub const MATCH_CONFIDENT: f64 = 0.82;

/// How far apart two durations can be before they stop supporting each other.
/// Wide enough for a different master or a gapless trim, narrow enough to
/// separate a radio edit from an album cut.
pub const MATCH_DURATION_TOLERANCE_MS: u64 = 15_000;

/// Containment, not Jaccard, over lower-cased alphanumeric word tokens.
///
/// Not an edit distance, because the differences that matter here are whole
/// extra words — "(Remastered 2011)", "feat. …", "- Single Version" — rather
/// than transposed letters, and a set comparison is unbothered by word order.
///
/// And not a symmetric Jaccard either, which was the first attempt and was
/// wrong: it scores "Teardrop" against "Teardrop (Remastered)" at 0.5, the same
/// as a genuinely different song sharing one word, because it counts the extra
/// token against *both* sides. Containment asks the right question — is the
/// shorter title entirely inside the longer one — and then charges a small
/// amount per extra word, so a remaster stays a near-match while an unrelated
/// title does not.
///
/// [`VERSION_WORDS`] are the exception: an extra word that means a different
/// take is not a decoration and is charged heavily.
fn title_similarity(a: &str, b: &str) -> f64 {
    let ta = tokens(a);
    let tb = tokens(b);
    if ta.is_empty() || tb.is_empty() {
        return 0.0;
    }
    let (small, large) = if ta.len() <= tb.len() {
        (&ta, &tb)
    } else {
        (&tb, &ta)
    };
    let shared = small.iter().filter(|t| large.contains(*t)).count() as f64;
    let containment = shared / small.len() as f64;
    let extra: Vec<&String> = large.iter().filter(|t| !small.contains(*t)).collect();
    if extra.iter().any(|t| VERSION_WORDS.contains(&t.as_str())) {
        return containment * 0.4;
    }
    containment * (1.0 - 0.05 * extra.len().min(4) as f64)
}

fn tokens(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

/// What the two people are watching or listening to.
///
/// Carried on [`TogetherSignal::Start`] only — the session id binds every later
/// message, so repeating this would be bytes spent on a chance for the two sides
/// to disagree about what they are watching.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TogetherContent {
    /// A file both sides already have. Identified by **its length and nothing
    /// else**.
    ///
    /// Not by a hash: a digest of the bytes would identify the exact release
    /// someone holds, and computing one over a multi-gigabyte file is seconds to
    /// minutes of work on the device for an answer we do not need. Not by
    /// filename either — a filename carries release group, language, sometimes a
    /// path fragment, and it does not actually answer the question (two files
    /// called `movie.mkv` are not the same film).
    ///
    /// Length is enough to warn *"their copy runs four seconds longer than
    /// yours"*, and is deliberately not enough to claim "same file" — which we
    /// cannot know and must not imply. It is needed anyway, to clamp an incoming
    /// position against something.
    LocalFile {
        duration_ms: u64,
        /// What is being played, when the sender chose to say.
        ///
        /// Optional, and the sending UI shows it before it goes out, so naming
        /// the thing is a deliberate act rather than a side effect of picking a
        /// file. When present it is what lets the *receiver* find their own copy
        /// instead of hunting for it — see [`Recording`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recording: Option<Recording>,
    },
    /// A public video, by its id. Unlike a local file this **is** fully
    /// disclosing — the id is publicly resolvable, so the invite says exactly
    /// what is being watched. That asymmetry is a property of the two sources,
    /// and the inviting UI should name it rather than hide it.
    Youtube { video_id: String },
    /// A recording on a streaming service, which **each side plays from their
    /// own subscription**.
    ///
    /// This is the shape every working listen-together product has, Spotify's
    /// own Jam included: no audio is shared, each participant's client streams
    /// the track itself, and only control events travel. It is also the only
    /// shape available to us — §1 rules out moving the bytes, and the platforms
    /// rule it out harder.
    ///
    /// Whether a session on one of these can actually be *held* together is not
    /// a property of the link. It depends on what each device can drive, which
    /// is [`ServiceAccess`] and [`MusicLink::playhead_control`] — a Spotify
    /// track is a full session on two devices with signed-in Premium accounts
    /// and a bare "here's where to open it" on a device with neither.
    ///
    /// Fully disclosing, like [`Self::Youtube`] and for the same reason: the id
    /// is publicly resolvable, so the invitation says exactly what is being
    /// played.
    Service {
        link: MusicLink,
        /// What it is, when the inviting side could name it — so the other
        /// device can look for its own copy if it cannot reach the service.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recording: Option<Recording>,
    },
    /// One public HTTPS URL that both sides fetch for themselves — a podcast
    /// episode off its RSS feed, an Internet Archive item, a Jamendo track.
    ///
    /// **The best-syncing online source there is, and it took the longest to
    /// notice.** The work went to streaming services first because that is what
    /// "listen to something online together" sounds like, and the whole time the
    /// tightest answer was the one with no account, no vendor SDK and no terms
    /// question in it. A podcast is an ordinary MP3 the publisher wants clients
    /// to fetch; nothing is transferred between peers, because both devices pull
    /// the same public URL themselves.
    ///
    /// It also **syncs four times tighter than a service track ever can**. This
    /// plays in a plain media element, which reports an accurate position and
    /// takes any `playbackRate` — so it earns the fine deadband and the whole
    /// correction ladder, where a vendor SDK reporting position only on state
    /// changes is stuck with the coarse one. See [`Self::tuning`].
    ///
    /// Fully disclosing, and more so than the other two: the URL names not only
    /// what is being played but where it came from.
    Stream {
        /// Validated by [`valid_stream_url`] on the way out **and** on the way
        /// in. Not a nicety — this string ends up in a media element's `src` on
        /// the strength of a peer having sent it, so the guard belongs here
        /// rather than in three UIs that each have to remember.
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recording: Option<Recording>,
        /// When the feed said. `None` is normal and costs nothing but the
        /// length-disagreement warning.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
    },
}

impl TogetherContent {
    /// A local file of `duration_ms`, optionally saying what it is.
    pub fn local_file(duration_ms: u64, recording: Option<Recording>) -> Self {
        Self::LocalFile {
            duration_ms,
            recording,
        }
    }

    /// A YouTube session from a link or a bare id, or `None` if it is neither.
    pub fn youtube(input: &str) -> Option<Self> {
        youtube_id_from_url(input).map(|video_id| Self::Youtube { video_id })
    }

    /// A session on one public HTTPS media URL, or `None` if the text is not one.
    ///
    /// The sibling of [`Self::youtube`], and it exists for the same reason: one
    /// parser, reached by every frontend, rather than three UIs each deciding
    /// what counts as a playable link. [`direct_media_url`] is the whole test —
    /// both halves of it, safety *and* "is there media at the end of this".
    ///
    /// Neither the recording nor the length is guessed from the URL. A filename
    /// is not a title (the same position [`Self::LocalFile`] takes about
    /// filenames), and a length is the player's to discover.
    pub fn stream(input: &str) -> Option<Self> {
        let url = input.trim();
        direct_media_url(url).then(|| Self::Stream {
            url: url.to_string(),
            recording: None,
            duration_ms: None,
        })
    }

    /// How long the content runs, when we know. A YouTube duration is the
    /// player's to report, not the inviter's to claim.
    pub fn duration_ms(&self) -> Option<u64> {
        match self {
            Self::LocalFile { duration_ms, .. } => Some(*duration_ms),
            Self::Stream { duration_ms, .. } => *duration_ms,
            // Both of these are the player's to report, not the inviter's to
            // claim — and for a service track the two devices may legitimately
            // be handed different masters of the same recording.
            Self::Youtube { .. } | Self::Service { .. } => None,
        }
    }

    /// Whether this content may be acted on at all — checked on the way **out**
    /// and again on the way **in**.
    ///
    /// One predicate rather than a check at each call site, because the two used
    /// to be separate `if let` arms that each named `Youtube` and nothing else:
    /// a variant carrying a peer-chosen string could be added, wired through
    /// three frontends, and reach a `src` attribute without either arm noticing.
    /// A `match` here means a new variant has to say which side of this line it
    /// is on.
    ///
    /// Deliberately not an error type. There is exactly one reason a peer's
    /// invitation is refused here — it carried something we will not put in a
    /// player — and the sender's copy of the same check is what turns it into a
    /// message for the person who typed it.
    pub fn admissible(&self) -> bool {
        match self {
            // Nothing peer-chosen reaches a URL: a duration is a number and a
            // recording is only ever text in a label.
            Self::LocalFile { .. } => true,
            // Eleven characters into an `<iframe src>`.
            Self::Youtube { video_id } => valid_youtube_id(video_id),
            // A catalogue id, used to *look something up*, never concatenated
            // into a request by this crate. The frontends that resolve it are
            // the ones that must escape it, and `MusicLink`'s shape — separate
            // storefront and id fields, not a URL — is what keeps that possible.
            Self::Service { .. } => true,
            // A whole URL the other person chose, going into a media element.
            // The strict one.
            Self::Stream { url, .. } => valid_stream_url(url),
        }
    }

    /// What the invitation names, when it names anything — so a device that
    /// cannot reach the source can look for its own copy instead.
    pub fn recording(&self) -> Option<&Recording> {
        match self {
            Self::LocalFile { recording, .. }
            | Self::Service { recording, .. }
            | Self::Stream { recording, .. } => recording.as_ref(),
            Self::Youtube { .. } => None,
        }
    }

    /// The correction policy this source can actually support.
    ///
    /// An HTML5 media element on a local file reports an accurate position and
    /// accepts any `playbackRate`, so the full ladder is available. An embedded
    /// YouTube player accepts only the discrete rates it advertises — a 4% trim
    /// is not expressible — reports position coarsely, and rebuffers visibly on
    /// a seek. Its ladder collapses to deadband-or-seek, so the deadband has to
    /// be wide enough not to thrash.
    pub fn tuning(&self) -> SyncTuning {
        match self {
            // A plain media element on an HTTPS URL is the same player as one on
            // a local file — accurate position, any `playbackRate` — so it earns
            // the same ladder. This is the whole practical argument for the
            // `Stream` variant: it is the only *online* source that syncs as
            // tightly as a file on disk.
            Self::LocalFile { .. } | Self::Stream { .. } => SyncTuning {
                min_deadband_ms: TOGETHER_DEADBAND_FINE_MS,
                can_rate_trim: true,
            },
            // A third-party player we drive through an SDK: no rate trim is
            // expressible, and the position it reports is event-driven rather
            // than continuous, so the deadband has to be wide enough not to
            // thrash against its own reporting granularity.
            Self::Youtube { .. } | Self::Service { .. } => SyncTuning {
                min_deadband_ms: TOGETHER_DEADBAND_COARSE_MS,
                can_rate_trim: false,
            },
        }
    }
}

/// A streaming-service link, reduced to what it identifies.
///
/// This is the *adapter* half of the Antra idea, minus the part this app will
/// not do. Antra resolves a link and then acquires the audio; here a link is
/// resolved only to **which recording it names**, so the listener's own copy can
/// be found. No audio is fetched, no DRM is involved, and nothing but a public
/// catalogue id leaves the device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(tag = "service", rename_all = "snake_case")]
pub enum MusicLink {
    /// A Spotify track id, resolvable through their public Web API.
    Spotify { track_id: String },
    /// An Apple Music track, resolvable through their catalogue API. The
    /// storefront is part of the key — the same recording has different ids in
    /// different countries.
    AppleMusic {
        storefront: String,
        track_id: String,
    },
    /// A YouTube (or YouTube Music) video. Unlike the two above this one is also
    /// *playable* in place, through the embed player — see [`TogetherContent`].
    Youtube { video_id: String },
}

impl MusicLink {
    /// The service's own name, for a UI that has to say where a link came from.
    pub fn service(&self) -> &'static str {
        match self {
            Self::Spotify { .. } => "Spotify",
            Self::AppleMusic { .. } => "Apple Music",
            Self::Youtube { .. } => "YouTube",
        }
    }

    /// How well this device could hold a playhead on this link, given what it
    /// is signed in to.
    ///
    /// **This used to be a property of the link, and that was the mistake.**
    /// `playable_in_place` answered "only YouTube", reasoning that Spotify and
    /// Apple Music serve DRM audio no third-party client may decode. The decode
    /// part is true and unchanged — we never touch their bytes. What does not
    /// follow is that we cannot *drive* them: both vendors ship SDKs whose whole
    /// purpose is letting another app control playback happening inside their
    /// own client, on the listener's own subscription. That is how every
    /// third-party listen-together product works, and how Spotify's own Jam
    /// works — nobody shares audio, each participant streams the track from
    /// their own client, and only control events travel.
    ///
    /// So the answer depends on the device, not the URL, and it has three values
    /// rather than two — see [`PlayheadControl`].
    pub fn playhead_control(&self, access: &ServiceAccess) -> PlayheadControl {
        match self {
            // The embed player needs no account at all, which is what keeps it
            // the only source that works for two strangers.
            Self::Youtube { .. } => PlayheadControl::Full,
            // Web Playback SDK in a webview, App Remote against the installed
            // app on Android. Both expose a seek and a position, so the whole
            // correction ladder short of a rate trim is available.
            Self::Spotify { .. } => {
                if access.spotify {
                    PlayheadControl::Full
                } else {
                    PlayheadControl::None
                }
            }
            // Deliberately never `Full`, even signed in. MusicKit offers no
            // precise scheduling, so there is no call that places a playhead at
            // a named moment — and its terms restrict synchronising MusicKit
            // content with other content, which at minimum needs a legal read
            // before anyone tries. `StartOnly` is what §9a already calls the
            // honest degradation: we started together, and nothing can pull us
            // back afterwards.
            Self::AppleMusic { .. } => {
                if access.apple_music {
                    PlayheadControl::StartOnly
                } else {
                    PlayheadControl::None
                }
            }
        }
    }
}

/// What this device is signed in to, and may therefore drive playback on.
///
/// Answered by the frontend, because only it knows: an OAuth token on desktop,
/// a connected `SpotifyAppRemote` on Android. Never persisted here and never
/// inferred — a device that has not said is a device that cannot.
///
/// **Premium is the real gate on Spotify**, not sign-in: both SDKs refuse to
/// play on a free account. A frontend must report `false` for a free account
/// rather than `true`-and-hope, or the session opens and the other person waits
/// for a track that will never start.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct ServiceAccess {
    /// A signed-in Spotify **Premium** account this device may drive.
    #[serde(default)]
    pub spotify: bool,
    /// A signed-in Apple Music subscription.
    #[serde(default)]
    pub apple_music: bool,
}

impl ServiceAccess {
    /// Nothing connected — what every device reports until a frontend says
    /// otherwise, and what a frontend with no integration reports forever.
    pub fn none() -> Self {
        Self::default()
    }
}

/// How tightly a source's playhead can be held.
///
/// Three values because the middle one is real and pretending it is not is how
/// a UI ends up claiming a precision it does not have. A deep link into someone
/// else's subscription genuinely does start two people together and genuinely
/// cannot pull them back afterwards, and saying so is better than either
/// refusing it or dressing it up as a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum PlayheadControl {
    /// Play, pause and *seek* all reach the player, and it reports where it is.
    /// A full session: the drift ladder applies, at whatever tuning the source
    /// supports.
    Full,
    /// It can be started, and that is all. The two devices agree on "now" and
    /// then drift with nothing able to close the gap — so a session must not
    /// claim to be holding them together, and must not emit corrections it
    /// cannot apply.
    StartOnly,
    /// Not drivable at all. The honest offer is a link to open, not a player.
    None,
}

impl PlayheadControl {
    /// Whether a session on this source should run the drift ladder at all.
    ///
    /// Load-bearing rather than cosmetic: emitting corrections to a player that
    /// cannot seek produces a stream of verdicts nothing applies, and a screen
    /// that says "catching up…" forever while nothing catches up.
    pub fn corrects(&self) -> bool {
        matches!(self, Self::Full)
    }

    /// Whether a session can be opened on this source at all.
    pub fn playable(&self) -> bool {
        !matches!(self, Self::None)
    }
}

/// Recognise a streaming-service link and reduce it to what it identifies.
///
/// Pure and offline: this only reads the URL. Turning the id into a title and an
/// ISRC needs the service's public catalogue API and is the caller's job — see
/// `docs/TOGETHER.md`.
pub fn parse_music_link(input: &str) -> Option<MusicLink> {
    let trimmed = input.trim();
    if let Some(video_id) = youtube_id_from_url(trimmed) {
        return Some(MusicLink::Youtube { video_id });
    }
    let rest = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);

    // open.spotify.com/track/<22-char base62>  (optionally /intl-xx/ first)
    if let Some(path) = rest.strip_prefix("open.spotify.com/") {
        let path = path.split_once("intl-").map_or(path, |(_, after)| {
            after.split_once('/').map_or(after, |(_, p)| p)
        });
        if let Some(after) = path.strip_prefix("track/") {
            let id: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            if id.len() == 22 {
                return Some(MusicLink::Spotify { track_id: id });
            }
        }
        return None;
    }

    // music.apple.com/<storefront>/album/<slug>/<album-id>?i=<track-id>
    if let Some(path) = rest.strip_prefix("music.apple.com/") {
        let storefront: String = path
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect();
        if storefront.len() != 2 {
            return None;
        }
        let track_id: String = path
            .split_once("?i=")
            .map(|(_, q)| q.chars().take_while(|c| c.is_ascii_digit()).collect())
            .unwrap_or_default();
        if !track_id.is_empty() {
            return Some(MusicLink::AppleMusic {
                storefront,
                track_id,
            });
        }
        return None;
    }
    None
}

/// Whether `id` is a well-formed YouTube video id.
///
/// This is not decoration. A video id arrives from the peer and goes straight
/// into an `<iframe src>` on the desktop webview; a value carrying a quote or an
/// angle bracket is an injection if any frontend ever builds that URL by
/// concatenation. Checking it once here — on send *and* on receive — is the same
/// "do it where no UI can forget" argument the media branch already makes for
/// MIME types.
pub fn valid_youtube_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The longest URL a peer may hand us for [`TogetherContent::Stream`].
///
/// A bound rather than a limit that matters: real episode URLs with tracking
/// query strings run long, and the point is only that a session invitation
/// cannot carry a megabyte of string into three UIs' DOM.
pub const STREAM_URL_MAX_LEN: usize = 2048;

/// Whether `url` is something we will hand to a media element on a peer's word.
///
/// The strictest validator in this module, and deliberately so: unlike a YouTube
/// id — eleven characters of a known alphabet — this is a whole URL that the
/// *other person* chose, and it ends up as a media element's `src`. The
/// device will then make a request to wherever it points, with the user's
/// network position and cookies-for-that-origin, because that is what a media
/// element does.
///
/// Six rules, each of them a thing a hostile invitation would otherwise buy:
///
/// - **HTTPS only.** `http:` is a downgrade a peer must not be able to choose,
///   `file:` reads the listener's disk, `javascript:` and `data:` are script
///   execution in whichever frontend is least careful about where it puts the
///   string. The same HTTPS-only line `media-http` already holds.
/// - **No credentials in the authority.** `https://user:pass@host/` is a
///   phishing shape and some fetchers hand the credentials on through a
///   redirect.
/// - **A real host, with a dot in it.** This is what stops
///   `https://router/reboot` — a peer-supplied URL that resolves on the
///   listener's *LAN*, not ours. Combined with the literal-IP refusal below it
///   is a blunt instrument, and it is the right blunt instrument: a session
///   invitation has no business naming a host that only exists inside somebody
///   else's house.
/// - **No literal IP addresses**, for the same reason and because a name is
///   what makes the disclosure meaningful. "Play this from 10.0.0.7" tells the
///   listener nothing about what they are fetching.
/// - **No control characters, spaces, quotes or angle brackets.** Any frontend
///   that ever builds an attribute by concatenation is an injection otherwise,
///   and this is the "do it where no UI can forget" argument the MIME branch
///   already makes.
/// - **Bounded length** ([`STREAM_URL_MAX_LEN`]).
///
/// What this deliberately does **not** do is resolve the name or follow the
/// request. A DNS answer can point inside the listener's network whatever the
/// name looks like, and this runs on a device with no network guarantee at all.
/// The honest statement is that this refuses the *stated* private target, not
/// every possible one — closing the rest belongs to whatever actually makes the
/// request, not to a pure function.
pub fn valid_stream_url(url: &str) -> bool {
    if url.len() > STREAM_URL_MAX_LEN {
        return false;
    }
    // Case-insensitive on the scheme only: the path is a peer's to case as they
    // like, and lowercasing it would corrupt URLs that are case-sensitive.
    let Some(rest) = url
        .get(..8)
        .filter(|p| p.eq_ignore_ascii_case("https://"))
        .map(|_| &url[8..])
    else {
        return false;
    };
    if rest
        .bytes()
        .any(|b| b.is_ascii_control() || matches!(b, b' ' | b'"' | b'\'' | b'<' | b'>' | b'\\'))
    {
        return false;
    }
    // The authority ends at the first `/`, `?` or `#`; everything after is the
    // server's business and not ours to judge.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // Credentials, and also the case where a peer tries to hide the real host
    // behind an `@` — the authority is what a browser resolves, so the part
    // *after* the last `@` is what actually matters and we refuse the shape
    // outright rather than parsing it.
    if authority.contains('@') {
        return false;
    }
    let host = authority.rsplit_once(':').map_or(authority, |(h, _)| h);
    if host.is_empty() || host.starts_with('.') || host.ends_with('.') {
        return false;
    }
    // A bracketed IPv6 literal, refused with the IPv4 ones below.
    if host.starts_with('[') {
        return false;
    }
    // A dotted host that parses as an address is a literal, not a name.
    if host.parse::<std::net::IpAddr>().is_ok() {
        return false;
    }
    // A name with no dot is a LAN name — `router`, `nas`, `localhost`.
    if !host.contains('.') {
        return false;
    }
    host.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
}

/// What a path has to end in for us to believe it names media rather than a page
/// about media.
///
/// Not a MIME negotiation and not a `HEAD` request: this runs on a device with no
/// network guarantee, in a pure function, at the moment someone pastes a link.
/// The extension is the only evidence available at that point.
const STREAM_MEDIA_SUFFIXES: [&str; 12] = [
    ".mp3", ".m4a", ".aac", ".ogg", ".oga", ".opus", ".flac", ".wav", ".mp4", ".m4v", ".webm",
    ".mkv",
];

/// Whether `url` names media a player can open, rather than a page that talks
/// about it.
///
/// [`valid_stream_url`] answers "is this safe to hand a media element"; this
/// answers the different question "is there any point". Both are needed and
/// neither implies the other: `https://example.com/article` passes the safety
/// check and would open a player on an HTML document, which fails as a decode
/// error several seconds later and reads to the person who pasted it as the
/// feature being broken.
///
/// **Refusing is the useful behaviour**, because the caller's fallback is to
/// treat the text as a search query — which is the right answer for a page link
/// and the wrong answer for an episode URL. So this is the line between the two,
/// and it is deliberately drawn conservatively: a real media URL with an
/// unlisted extension becomes a search that finds nothing, which is recoverable;
/// a page treated as media is a player that hangs.
///
/// The query string is excluded before the suffix is checked, because the
/// tracking parameters podcast hosts append (`…/ep12.mp3?token=…`) are exactly
/// the shape §11a says is normal.
pub fn direct_media_url(url: &str) -> bool {
    if !valid_stream_url(url) {
        return false;
    }
    // Everything after `?` or `#` belongs to the server, not to the filename.
    let rest = url.split(['?', '#']).next().unwrap_or_default();
    // The suffix has to be in the *path*, not in the host. `https://example.mp3`
    // is a well-formed URL with a nonexistent TLD, and calling it media would
    // route a typo into a player instead of into a search. `valid_stream_url`
    // has already established the `https://` prefix, so the authority is
    // whatever precedes the next `/`.
    let path = match rest.get(8..).and_then(|after| after.find('/')) {
        Some(slash) => &rest[8 + slash..],
        None => return false,
    };
    // Compared lowercased: `EPISODE.MP3` is the same file as `episode.mp3`, and
    // a case-sensitive check would send it down the search path instead.
    let lowered = path.to_ascii_lowercase();
    STREAM_MEDIA_SUFFIXES.iter().any(|s| lowered.ends_with(s))
}

/// Pull a video id out of a bare id or any of the usual link shapes
/// (`watch?v=`, `youtu.be/`, `shorts/`, `embed/`). In core rather than in three
/// frontends, so there is one parser and one answer.
pub fn youtube_id_from_url(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if valid_youtube_id(trimmed) {
        return Some(trimmed.to_string());
    }
    // Strip scheme and host, then look for the id in the shapes YouTube uses.
    let rest = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);

    let candidate = if let Some(q) = rest.find("v=") {
        &rest[q + 2..]
    } else {
        let path = rest.split_once('/').map(|(_, p)| p)?;
        for marker in ["shorts/", "embed/", "v/"] {
            if let Some(idx) = path.find(marker) {
                let start = idx + marker.len();
                return take_id(&path[start..]);
            }
        }
        // `youtu.be/<id>` — the id is the whole first path segment.
        path
    };
    take_id(candidate)
}

/// The leading id-shaped run of `s`, if it is exactly an id's worth.
fn take_id(s: &str) -> Option<String> {
    let id: String = s
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    valid_youtube_id(&id).then_some(id)
}

// ── The wire ─────────────────────────────────────────────────────────────────

/// One statement about a shared session.
///
/// Note what is *not* here. There is no separate play, pause and seek: all three
/// reduce to a position and whether it is running, so they are one
/// [`TogetherSignal::State`] and the receiver derives which happened
/// ([`describe_state_change`]) in one tested place rather than three frontends
/// each guessing. And [`TogetherSignal::End`] carries no reason — bored, phone
/// rang, app closed is not the other person's to learn, and "they left" is the
/// whole signal. (Whether *we* saw an `End` or merely stopped hearing them is
/// something we observed, not something they told us, so it stays local.)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TogetherSignal {
    /// "Watch this with me, and here is where I am." Invitation and opening
    /// position in one, so the joiner never joins to nowhere.
    Start {
        content: TogetherContent,
        pos_ms: u64,
        playing: bool,
    },
    /// "I'm in." Carries no position: the joiner adopts the leader's, and
    /// reporting one would only tempt the leader to correct toward it.
    Join,
    /// Play, pause and seek — see the note above.
    ///
    /// `pos_ms` is the playhead **at `effective_at_ms`**, not at send time and
    /// not on arrival. That is the difference between this and every
    /// position-swapping watch-party protocol: a command that takes 300 ms to
    /// arrive costs nothing, because the receiver evaluates the timeline at its
    /// own now instead of adopting a stale number.
    ///
    /// Absent means "effective when I sent it" — the envelope's own `at_ms` —
    /// which is what a relay-speed sender should say. A sender on a transport
    /// fast enough to schedule ahead (the local mesh) sets it a little in the
    /// future, and then *both* players change state on the same instant rather
    /// than one chasing the other.
    State {
        pos_ms: u64,
        playing: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effective_at_ms: Option<u64>,
    },
    /// Where I am now, and the last command I had applied when I said so.
    ///
    /// `applied_seq` is what lets the receiver tell "their position follows from
    /// a command I never received" (repair it) from "we simply drifted"
    /// (correct it) — and it is what stops a peer who is *behind* dragging a
    /// peer who is right backwards.
    Heartbeat {
        pos_ms: u64,
        playing: bool,
        applied_seq: u64,
        /// How far behind `pos_ms` the sound actually leaving this device's
        /// speaker is. Zero means "not measured" — honest, and what any player
        /// that cannot ask its audio stack will send.
        #[serde(default)]
        output_latency_ms: u64,
    },
    /// "I'm leaving." Also the decline: a `Start` answered with `End` is a no,
    /// and the initiator deliberately cannot tell the two apart.
    End,
    /// Handing the file over, for the case this module otherwise assumes away:
    /// only one of you has it.
    ///
    /// It rides *inside* the session envelope rather than getting a marker of
    /// its own, and that is the whole reason it is safe. Every guard the session
    /// already has — the acceptance gate, the sixty-second age gate, the
    /// session-id scoping, and the fact that sessions do not survive a restart —
    /// applies unchanged to a transfer negotiation. A separate envelope would
    /// have needed its own copy of all four, and a stranger able to open a
    /// peer-to-peer connection to you by sending one DM is a much worse bug than
    /// a stranger able to move your playhead.
    ///
    /// It is deliberately **not** a command: see [`Self::is_command`].
    Share { signal: crate::share::ShareSignal },
}

impl TogetherSignal {
    /// Stable discriminant string, for logging.
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::Start { .. } => "start",
            Self::Join => "join",
            Self::State { .. } => "state",
            Self::Heartbeat { .. } => "heartbeat",
            Self::End => "end",
            Self::Share { .. } => "share",
        }
    }

    /// Whether this signal is a command — something a person did, which the
    /// arbitration in [`CommandStamp`] has to order. Heartbeats and joins are
    /// not: applying one twice changes nothing.
    ///
    /// Nor is a share signal, and that one is worth stating rather than reading
    /// off the pattern. A transfer negotiation is a conversation in its own
    /// right, running alongside playback and at its own pace: an ICE candidate
    /// carries no opinion about where the playhead is, so ranking it against a
    /// pause would let a burst of candidates outrank the pause button.
    pub fn is_command(&self) -> bool {
        matches!(self, Self::Start { .. } | Self::State { .. })
    }
}

/// The two timestamps that turn an ordinary heartbeat into an NTP probe.
/// Both names are from the point of view of whoever *sends* the message
/// carrying it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClockEcho {
    /// The `at_ms` of the last message I received from you — your clock.
    pub your_at_ms: u64,
    /// My clock when I received it.
    pub my_recv_ms: u64,
}

/// The JSON envelope carried inside an E2E DM that drives a shared session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TogetherEnvelope {
    /// Format marker / version; must equal [`TOGETHER_ENVELOPE_MARKER`].
    pub comrade_together: u8,
    /// Groups every signal of one session. Also the primary replay guard: a
    /// command naming a session this device does not have is dropped.
    pub session_id: String,
    /// The Lamport counter behind [`CommandStamp`].
    pub seq: u64,
    /// The sender's own clock, in milliseconds, when this was composed.
    ///
    /// The DM's own timestamp cannot do this job: it is in whole seconds, which
    /// is a full second of noise in a system trying to hold a fraction of one.
    pub at_ms: u64,
    /// The probe half of the clock estimate. Absent on the first message of a
    /// session, when there is nothing yet to echo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub echo: Option<ClockEcho>,
    pub signal: TogetherSignal,
}

impl TogetherEnvelope {
    pub fn new(
        session_id: impl Into<String>,
        seq: u64,
        at_ms: u64,
        echo: Option<ClockEcho>,
        signal: TogetherSignal,
    ) -> Self {
        Self {
            comrade_together: TOGETHER_ENVELOPE_MARKER,
            session_id: session_id.into(),
            seq,
            at_ms,
            echo,
            signal,
        }
    }

    /// Serialise to the JSON string that becomes the DM body.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Detect and parse a together envelope out of a decrypted DM body. Returns
/// `None` for chat text, receipts, profile shares, presence beacons, nudges,
/// call or media envelopes — so the runtime falls through to its other handlers.
pub fn parse_together_envelope(content: &str) -> Option<TogetherEnvelope> {
    let env: TogetherEnvelope = serde_json::from_str(content).ok()?;
    (env.comrade_together == TOGETHER_ENVELOPE_MARKER).then_some(env)
}

/// Mint a fresh, unguessable session id (128 bits of randomness, hex-encoded).
pub fn new_session_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Whether a signal may arrive over a **direct peer channel** rather than a
/// relay.
///
/// Everything except [`TogetherSignal::Start`]. A direct channel only exists
/// because a session was negotiated inside one, so a channel able to *open* a
/// session would invert that — and `start` is the single signal the relay's
/// per-message NIP-44 authentication is genuinely load-bearing for, since it is
/// the only one that creates state from nothing.
///
/// Stated honestly about what is doing the work: today a direct `start` is
/// already unreachable, because the receiver has no way to attribute a sender
/// without a live session and refuses on that ground first. This predicate is
/// the *explicit* statement of the rule, so that a later change which learns to
/// attribute a peer some other way cannot quietly turn a data channel into an
/// invitation path. Its own unit test is what pins it; the runtime test above it
/// can only observe the behaviour both rules share.
pub fn direct_signal_admissible(signal: &TogetherSignal) -> bool {
    !matches!(signal, TogetherSignal::Start { .. })
}

/// Whether a direct peer channel the frontend declared open should still be
/// used, given when it last produced evidence of being alive.
///
/// `last_evidence_ms` is the later of two things: when the frontend said the
/// channel was up, and when an envelope last arrived over it. The first is a
/// grace window — a channel deserves a chance to prove itself before anything
/// is concluded from silence — and the second is the only proof available,
/// because a send on this path is fire-and-forget by construction: the socket
/// belongs to the frontend, so "did it arrive" is not a question core can ask.
///
/// Silence is read as a dead channel rather than as a quiet one, and that is
/// sound rather than pessimistic: a data channel is symmetric, so a peer whose
/// channel is open sends *their* heartbeats down it, every
/// [`TOGETHER_HEARTBEAT_SECS`]. Two of those passing with nothing to show for
/// them means the path is not carrying, whichever end broke.
///
/// **Self-healing, deliberately.** This is a per-send question, not a latch, so
/// a channel that goes quiet and comes back is trusted again the moment an
/// envelope arrives on it — no re-declaration from the frontend needed. There is
/// no deadlock hiding in that, because the evidence is the peer's traffic and
/// not our own: nothing we stop sending can stop them sending.
pub fn direct_path_live(last_evidence_ms: u64, now_ms: u64) -> bool {
    now_ms.saturating_sub(last_evidence_ms) < TOGETHER_DIRECT_SILENCE_MS
}

/// Whether a signal sent at `created_at` (seconds) is still worth acting on.
/// The age gate described on [`TOGETHER_SIGNAL_MAX_AGE_SECS`].
pub fn signal_is_fresh(created_at: u64, now: u64) -> bool {
    now.saturating_sub(created_at) <= TOGETHER_SIGNAL_MAX_AGE_SECS
}

/// Whether a session last heard from at `last_heard_ms` is still live at
/// `now_ms`. Every read recomputes this against the current clock, so a session
/// can never outlive its own deadline just because nothing swept it yet.
pub fn session_is_live_at(last_heard_ms: u64, now_ms: u64) -> bool {
    now_ms.saturating_sub(last_heard_ms) < TOGETHER_SESSION_TTL_MS
}

/// How long to wait before the next heartbeat, given how many usable clock
/// probes this session has gathered.
///
/// Fast until the clock has converged, then slow for the rest of the session —
/// the burst is what makes the *first* minute as tight as the rest, and the
/// slow tail is what keeps a two-hour film from writing thousands of persistent
/// events that every later app launch re-scans.
pub fn heartbeat_interval_ms(probes: usize) -> u64 {
    if probes < CLOCK_BURST_PROBES {
        TOGETHER_BURST_INTERVAL_MS
    } else {
        TOGETHER_HEARTBEAT_SECS * 1000
    }
}

// ── Command arbitration ──────────────────────────────────────────────────────

/// One command's place in the total order both devices compute independently.
///
/// The counter is a Lamport clock, deliberately **not** a timestamp: a device
/// whose clock runs a few seconds slow would lose every tie forever, and the
/// person holding it would experience "my pause button doesn't work". A counter
/// is skew-free by construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct CommandStamp {
    pub seq: u64,
    /// Who issued it — the DM sender, so it costs nothing on the wire.
    pub actor: String,
    /// Whether this command *stops* playback. Only used to break a tie.
    pub pausing: bool,
}

impl CommandStamp {
    pub fn new(seq: u64, actor: impl Into<String>, pausing: bool) -> Self {
        Self {
            seq,
            actor: actor.into(),
            pausing,
        }
    }

    /// Whether this command should displace `applied`.
    ///
    /// A strict total order, so the two devices always agree on the winner:
    /// higher counter first; then **a pause beats a play**, because the person
    /// reaching for pause has a reason and the one reaching for play can simply
    /// press it again; then the greater actor id, which is arbitrary but
    /// symmetric and needs no round trip.
    ///
    /// Identical stamps return `false`, which is what makes a redelivered
    /// command an exact no-op — no dedup set required, and none that could be
    /// evicted.
    pub fn wins_over(&self, applied: &CommandStamp) -> bool {
        if self.seq != applied.seq {
            return self.seq > applied.seq;
        }
        if self.pausing != applied.pausing {
            return self.pausing;
        }
        self.actor > applied.actor
    }
}

/// Whether a fresh command may go out yet, given when the last one did.
/// Coalesces a scrub drag into the position the user actually settled on.
pub fn command_is_due(last_sent_ms: u64, now_ms: u64) -> bool {
    now_ms.saturating_sub(last_sent_ms) >= TOGETHER_COMMAND_MIN_INTERVAL_MS
}

// ── The shared queue ─────────────────────────────────────────────────────────
//
// §16 made the pairing the session, and §16's "either of you may put something
// on" made both sides able to choose what plays — but only *one thing at a
// time*: putting a track on is still an `End` and a `Start`, and the "up next"
// list (`TogetherDecisions.Queue` on Android) is each device's own, invisible to
// the other. That is the gap against a listen-together built as a music player
// rather than a watch-party: the thing you build together is a queue, and both
// people add to it, reorder it and watch it advance.
//
// `SharedQueue` is that queue as one snapshot both devices converge on. The rule
// it converges by is **not** a new one: it is the same [`CommandStamp`] total
// order the playback commands already use. A queue edit is a command like a
// pause is — the whole list, stamped `(seq, actor)`, and the higher stamp wins.
// So two devices that each edited independently do not merge op-by-op (a CRDT is
// a great deal of machinery for a two-person list); the later edit replaces the
// earlier one wholesale, exactly as a later pause replaces an earlier play. A
// lost edit is superseded by the next snapshot the same way a lost command is
// superseded by the next heartbeat — the philosophy §17 states for commands,
// applied to the list. The cost, stated rather than discovered: an add the peer
// makes in the same beat as your reorder can be dropped, and the answer is the
// same as everywhere else here — the next snapshot carries the truth.

/// One entry in the shared queue.
///
/// `id` is what survives a reorder: it is assigned once by whoever added the
/// item (`actor` plus a per-actor counter, e.g. `"npub1…:7"`) and never
/// reassigned, so "the track I dragged" and "the track the peer removed" name
/// the same row on both devices even after the list around it has moved. Two
/// devices minting ids from disjoint actor namespaces cannot collide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct QueueItem {
    /// Stable identity, assigned by the adder and never reused. See the type doc.
    pub id: String,
    /// What plays. Held to the same admission bar as a `Start`'s content.
    pub content: TogetherContent,
    /// Who put it on — the DM sender, as with [`CommandStamp::actor`]. What the
    /// UI badges a row with ("added by …") and costs nothing extra on the wire.
    pub added_by: String,
}

/// The queue both devices converge on, as one stamped snapshot.
///
/// `cursor` is the index of the item that is *playing* — not a separate piece
/// of state from the [`TogetherSignal::Start`] that put it on, but the queue's
/// own record of where "next" and "previous" count from. An empty queue has
/// `cursor == 0`, which is out of range and therefore names nothing, which is
/// the honest answer for "nothing is on".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct SharedQueue {
    pub stamp: CommandStamp,
    pub items: Vec<QueueItem>,
    pub cursor: u32,
}

impl SharedQueue {
    /// An empty queue at the very start of the order, stamped by its maker.
    pub fn empty(seq: u64, actor: impl Into<String>) -> Self {
        Self {
            // A queue edit is never a "pause", so the tie-breaker bit is false;
            // ordering falls through to seq then actor exactly as a play does.
            stamp: CommandStamp::new(seq, actor, false),
            items: Vec::new(),
            cursor: 0,
        }
    }

    /// The item currently playing, or `None` when the cursor names no row
    /// (an empty queue, or one whose cursor ran off the end).
    pub fn current(&self) -> Option<&QueueItem> {
        self.items.get(self.cursor as usize)
    }

    /// Whether this snapshot is safe to adopt from a peer.
    ///
    /// Every item is held to [`TogetherContent::admissible`] — the queue is a
    /// list of things that will each become a `Start`, so a URL that would be
    /// refused as an invitation must be refused as a queue entry, at the same
    /// bar and for the same reason. Ids must be present and unique, because a
    /// blank or duplicated id is a reorder that moves the wrong row. The cursor
    /// may sit one past the end (a queue played to its end) but no further.
    pub fn admissible(&self) -> bool {
        if self.cursor as usize > self.items.len() {
            return false;
        }
        let mut seen = std::collections::HashSet::with_capacity(self.items.len());
        self.items
            .iter()
            .all(|it| !it.id.is_empty() && it.content.admissible() && seen.insert(it.id.as_str()))
    }

    /// Adopt `incoming` if and only if its stamp wins the total order, and only
    /// if it is admissible. Returns whether `self` changed.
    ///
    /// This is the whole of reconciliation: no field is merged, the winning
    /// snapshot is taken entire. An inadmissible snapshot is dropped rather than
    /// adopted, so a peer cannot slip a refused URL into the list by attaching a
    /// high sequence number to it — the guard is on the value, not only on the
    /// order.
    pub fn reconcile(&mut self, incoming: SharedQueue) -> bool {
        if incoming.admissible() && incoming.stamp.wins_over(&self.stamp) {
            *self = incoming;
            true
        } else {
            false
        }
    }

    /// Re-stamp after an edit, so the changed list wins over the one it changed.
    fn restamp(&mut self, seq: u64, actor: impl Into<String>) {
        self.stamp = CommandStamp::new(seq, actor, false);
    }

    /// Append an item at the end. An append shifts nothing before it, so the
    /// cursor is untouched — the same reasoning `TogetherDecisions.addToQueue`
    /// records on Android.
    pub fn add(&mut self, item: QueueItem, seq: u64, actor: impl Into<String>) {
        self.items.push(item);
        self.restamp(seq, actor);
    }

    /// Insert an item immediately after the playing one ("play next"). The
    /// current item's own position never moves, so the cursor is carried
    /// through unchanged.
    pub fn play_next(&mut self, item: QueueItem, seq: u64, actor: impl Into<String>) {
        let at = (self.cursor as usize + 1).min(self.items.len());
        self.items.insert(at, item);
        self.restamp(seq, actor);
    }

    /// Remove the item with `id`, keeping the cursor pointing at the same track
    /// wherever the removal moved it — the invariant every mutation here holds,
    /// mirroring `TogetherDecisions.removeAt`. Removing the current item lands
    /// the cursor on what would have played next (or the new last row at the
    /// end). A no-op, un-stamped, when no row has that id.
    pub fn remove(&mut self, id: &str, seq: u64, actor: impl Into<String>) -> bool {
        let Some(at) = self.items.iter().position(|it| it.id == id) else {
            return false;
        };
        self.items.remove(at);
        let cursor = self.cursor as usize;
        self.cursor = match at.cmp(&cursor) {
            std::cmp::Ordering::Less => cursor.saturating_sub(1),
            std::cmp::Ordering::Greater => cursor,
            std::cmp::Ordering::Equal => cursor.min(self.items.len().saturating_sub(1)),
        } as u32;
        self.restamp(seq, actor);
        true
    }

    /// Move to the next item, returning what now plays (or `None` at the end).
    /// Advancing past the last item leaves the cursor one past the end, which
    /// [`Self::current`] reports as nothing — a queue that finished, not one
    /// that loops.
    pub fn advance(&mut self, seq: u64, actor: impl Into<String>) -> Option<&QueueItem> {
        self.cursor = (self.cursor + 1).min(self.items.len() as u32);
        self.restamp(seq, actor);
        self.items.get(self.cursor as usize)
    }
}

// ── The clock estimate ───────────────────────────────────────────────────────

/// One round-trip measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockProbe {
    /// Peer clock minus our clock, in milliseconds.
    pub offset_ms: i64,
    /// Round trip with the peer's own turnaround removed.
    pub rtt_ms: u64,
    /// Our clock when this probe landed, so it can be aged out.
    pub at_ms: u64,
}

/// What we currently believe about the peer's clock, and how sure we are.
///
/// Two terms, not one. NTP disciplines *phase* and *frequency*, and so does
/// this: an offset alone is only correct at the instant it was measured, and
/// decays at the rate the two crystals differ. Tracking the rate as well means a
/// projection stays good between probes — which is what lets the heartbeat be
/// slow *and* the sync be tight, instead of trading one for the other.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClockEstimate {
    /// Peer clock minus ours, in milliseconds, **at [`Self::measured_at_ms`]**.
    pub offset_ms: i64,
    /// `None` until a round trip has actually been measured.
    pub rtt_ms: Option<u64>,
    /// How fast the peer's clock runs relative to ours, in parts per million.
    /// Positive means theirs gains. Zero until enough probes span enough time to
    /// measure it — a guessed frequency is worse than none.
    pub skew_ppm: f64,
    /// Our clock when the offset was measured; the skew projects from here.
    pub measured_at_ms: u64,
}

impl ClockEstimate {
    /// What we believe before measuring anything: nothing, pessimistically.
    pub fn unmeasured() -> Self {
        Self {
            offset_ms: 0,
            rtt_ms: None,
            skew_ppm: 0.0,
            measured_at_ms: 0,
        }
    }

    /// The offset projected to `now_ms`, carrying the measured frequency
    /// forward. With a typical ±20 ppm crystal pair this is worth ~1 ms per
    /// minute of probe age — small, but it is exactly the error that used to
    /// grow silently between heartbeats.
    pub fn offset_at(&self, now_ms: u64) -> i64 {
        if self.rtt_ms.is_none() {
            return self.offset_ms;
        }
        let elapsed = now_ms.saturating_sub(self.measured_at_ms) as f64;
        self.offset_ms + (self.skew_ppm * elapsed / 1_000_000.0).round() as i64
    }

    /// How wrong a projection built on this could be, **at `now_ms`**.
    ///
    /// Half the round trip — the one-way delay we cannot observe directly —
    /// plus what the frequency estimate itself could have drifted since it was
    /// taken. Never zero: [`sync_verdict`] refuses to correct inside this
    /// figure, so a zero would mean "correct on any evidence at all".
    pub fn uncertainty_at(&self, now_ms: u64) -> u64 {
        let Some(rtt) = self.rtt_ms else {
            return TOGETHER_UNMEASURED_UNCERTAINTY_MS;
        };
        let elapsed = now_ms.saturating_sub(self.measured_at_ms) as f64;
        // A residual frequency error of this much is assumed to survive the
        // regression; it bounds how fast a stale estimate goes bad.
        let residual = (CLOCK_RESIDUAL_SKEW_PPM * elapsed / 1_000_000.0).round() as u64;
        rtt / 2 + residual
    }
}

/// A bounded ring of recent probes, reporting the least-delayed one and the
/// frequency difference across all of them.
///
/// The minimum filter is the standard NTP trick and it earns its place twice
/// here: the probe that queued least is the one whose "both directions took the
/// same time" assumption holds best, and a clock *step* on either device shows
/// up as an outlier that ages out of the window rather than poisoning an average
/// forever.
///
/// Plain `&mut self`, no interior mutability: one of these belongs to exactly
/// one session, which already sits behind a lock.
#[derive(Debug, Clone, Default)]
pub struct ClockFilter {
    probes: VecDeque<ClockProbe>,
}

impl ClockFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold in one four-timestamp observation:
    /// `t1` our send, `t2` their receive, `t3` their send, `t4` our receive.
    ///
    /// Returns the probe if it was usable. A measurement where either side's
    /// clock appears to run backwards is discarded rather than clamped — it
    /// means an assumption broke, and a fabricated zero would be worse than no
    /// sample at all.
    pub fn observe(&mut self, t1: u64, t2: u64, t3: u64, t4: u64) -> Option<ClockProbe> {
        if t4 < t1 || t3 < t2 {
            return None;
        }
        let (t1, t2, t3, t4) = (t1 as i64, t2 as i64, t3 as i64, t4 as i64);
        let rtt = (t4 - t1) - (t3 - t2);
        if rtt < 0 {
            return None;
        }
        let probe = ClockProbe {
            offset_ms: ((t2 - t1) + (t3 - t4)) / 2,
            rtt_ms: rtt as u64,
            at_ms: t4 as u64,
        };
        self.probes.push_back(probe);
        while self.probes.len() > CLOCK_PROBE_RING {
            self.probes.pop_front();
        }
        Some(probe)
    }

    /// The best estimate at `now_ms`, ignoring probes that have aged out.
    ///
    /// Offsets are averaged over the **best half by round trip**, not taken from
    /// the single least-delayed probe — beatsync's filter, and it is better than
    /// a bare minimum for the same reason any average is: one lucky sample stops
    /// deciding the answer alone. The lowest round trip still sets the
    /// *uncertainty*, because that is a claim about the best evidence we have,
    /// not about the mean.
    ///
    /// With one improvement on it: each offset is **de-skewed to a common
    /// instant before averaging**. Offsets measured minutes apart are not
    /// samples of one quantity when the two clocks run at different rates —
    /// averaging them raw would smear the frequency difference back into the
    /// phase and partly undo [`Self::estimate`]'s own skew term. beatsync has no
    /// frequency term to conflict with, so it does not hit this; here it would
    /// be a real error.
    pub fn estimate(&self, now_ms: u64) -> ClockEstimate {
        // Insertion order is time order, which `skew_ppm` relies on.
        let live: Vec<&ClockProbe> = self
            .probes
            .iter()
            .filter(|p| now_ms.saturating_sub(p.at_ms) <= CLOCK_PROBE_WINDOW_MS)
            .collect();
        if live.is_empty() {
            return ClockEstimate::unmeasured();
        }
        let skew = skew_ppm(&live);

        let mut by_rtt = live.clone();
        by_rtt.sort_by_key(|p| p.rtt_ms);
        let best = by_rtt[0];
        let keep = by_rtt.len().div_ceil(2); // the best half, at least one
        let reference = best.at_ms;
        let sum: f64 = by_rtt[..keep]
            .iter()
            .map(|p| {
                let dt = p.at_ms as f64 - reference as f64;
                p.offset_ms as f64 - skew * dt / 1_000_000.0
            })
            .sum();

        ClockEstimate {
            offset_ms: (sum / keep as f64).round() as i64,
            rtt_ms: Some(best.rtt_ms),
            skew_ppm: skew,
            measured_at_ms: reference,
        }
    }

    /// How many probes are being kept. Bounded by [`CLOCK_PROBE_RING`].
    pub fn len(&self) -> usize {
        self.probes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.probes.is_empty()
    }
}

/// Least-squares slope of offset against time, in ppm — the two crystals'
/// frequency difference.
///
/// Returns zero unless there are enough probes over a long enough span to mean
/// anything: over a short baseline the slope is dominated by network jitter, and
/// a frequency estimate built from noise is worse than admitting we have none,
/// because it keeps *accumulating* after the probes stop.
///
/// The result is clamped to [`CLOCK_MAX_SKEW_PPM`]. Consumer crystals are
/// specified to a few tens of ppm; anything past this is a clock being stepped
/// (NTP correcting, a user changing the time), which is a *phase* event and must
/// not be mistaken for a frequency.
fn skew_ppm(probes: &[&ClockProbe]) -> f64 {
    if probes.len() < CLOCK_MIN_SKEW_PROBES {
        return 0.0;
    }
    let (first, last) = (probes[0], probes[probes.len() - 1]);
    let span = last.at_ms.saturating_sub(first.at_ms);
    if span < CLOCK_MIN_SKEW_SPAN_MS {
        return 0.0;
    }
    let n = probes.len() as f64;
    let mean_t = probes.iter().map(|p| p.at_ms as f64).sum::<f64>() / n;
    let mean_o = probes.iter().map(|p| p.offset_ms as f64).sum::<f64>() / n;
    let mut num = 0.0;
    let mut den = 0.0;
    for p in probes {
        let dt = p.at_ms as f64 - mean_t;
        num += dt * (p.offset_ms as f64 - mean_o);
        den += dt * dt;
    }
    if den <= 0.0 {
        return 0.0;
    }
    // Slope is ms of offset per ms of time; ppm is that times 1e6.
    let slope = num / den * 1_000_000.0;
    if !slope.is_finite() {
        return 0.0;
    }
    slope.clamp(-CLOCK_MAX_SKEW_PPM, CLOCK_MAX_SKEW_PPM)
}

/// Where a playhead is at `at_ms`, given that it was at `anchor_pos_ms` at
/// `anchor_at_ms` and whether it is running. All four times must be on the same
/// clock; the caller converts.
///
/// This is the semantic the whole design rests on, and it is the one thing every
/// browser-based watch-party gets wrong: a position is meaningless without the
/// instant it was true at. "I am at 42:00" acted on 300 ms later puts you 300 ms
/// behind, every single time, and the error is invisible because both sides
/// agree on the number they exchanged. An *anchor* has no such failure mode —
/// arriving late costs nothing, because the receiver evaluates the timeline at
/// its own now rather than adopting a stale instant.
pub fn timeline_pos_ms(anchor_pos_ms: u64, anchor_at_ms: u64, playing: bool, at_ms: u64) -> u64 {
    if !playing {
        return anchor_pos_ms;
    }
    anchor_pos_ms.saturating_add(at_ms.saturating_sub(anchor_at_ms))
}

/// What to do with a command that has just arrived.
///
/// Commands carry the instant they take effect, so a receiver has two honest
/// options and never needs a third: if that instant is still ahead, wait for it
/// and both players change state on the same tick; if it has passed, evaluate
/// the timeline and land where the sender already is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandApply {
    /// Apply now, at this position — the effective instant has passed, so the
    /// position has been carried forward to compensate for the flight.
    Now { pos_ms: u64, playing: bool },
    /// Wait `in_ms`, then apply. Only reachable when the transport is fast
    /// enough for the sender to schedule ahead of it — the local mesh, not a
    /// relay.
    At {
        pos_ms: u64,
        playing: bool,
        in_ms: u64,
    },
}

/// Turn an incoming command into one of the two above.
///
/// `effective_at_ms` is on the *peer's* clock; `clock` converts it. A command
/// scheduled further ahead than [`TOGETHER_MAX_SCHEDULE_MS`] is applied now
/// instead of trusted — a peer that asks us to pause in an hour is either broken
/// or hostile, and either way we are not holding a timer for it.
pub fn command_apply(
    anchor_pos_ms: u64,
    effective_at_ms: u64,
    playing: bool,
    clock: &ClockEstimate,
    now_ms: u64,
) -> CommandApply {
    // Peer instant → our clock.
    let effective_here = effective_at_ms as i64 - clock.offset_at(now_ms);
    let ahead = effective_here - now_ms as i64;
    if ahead > 0 && (ahead as u64) <= TOGETHER_MAX_SCHEDULE_MS {
        return CommandApply::At {
            pos_ms: anchor_pos_ms,
            playing,
            in_ms: ahead as u64,
        };
    }
    // It has already taken effect (or is absurdly far off). Evaluate the
    // timeline rather than adopting the sender's stale number.
    let elapsed = (-ahead).max(0) as u64;
    CommandApply::Now {
        pos_ms: timeline_pos_ms(anchor_pos_ms, 0, playing, elapsed),
        playing,
    }
}

// ── The verdict ──────────────────────────────────────────────────────────────

/// What a given player can be asked to do about drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncTuning {
    pub min_deadband_ms: u64,
    pub can_rate_trim: bool,
}

/// Everything [`sync_verdict`] needs, gathered by the caller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SyncSample {
    pub local_pos_ms: u64,
    pub local_playing: bool,
    /// The counter of the last command *we* have applied.
    pub local_seq: u64,
    pub peer_pos_ms: u64,
    pub peer_playing: bool,
    /// The counter the peer said they had applied when they sent this.
    pub peer_seq: u64,
    /// The peer's clock when they sent it.
    pub peer_at_ms: u64,
    /// Our clock now.
    pub now_ms: u64,
    /// Our clock when we last moved the playhead automatically.
    pub last_seek_ms: u64,
    pub clock: ClockEstimate,
    /// Whether *we* started this session. The starter leads.
    pub we_lead: bool,
    /// How far *behind* our reported position the sound actually leaving the
    /// speaker is, in milliseconds.
    ///
    /// A player's `getCurrentPosition` is where the decoder is, not where the
    /// listener is: on Android the audio HAL buffer alone is 20-100 ms, and it
    /// differs between two handsets. Two players agreeing perfectly on decoder
    /// position can therefore still be a tenth of a second apart in the room,
    /// which is exactly the error a browser-based implementation cannot even
    /// see. `AudioTrack.getTimestamp` measures it; zero means "not measured",
    /// which costs nothing but is not a claim that it is zero.
    pub local_output_latency_ms: u64,
    /// The same figure for the peer, as they reported it.
    pub peer_output_latency_ms: u64,
    /// The rate trim currently applied to our player, as an absolute
    /// multiplier — `1.0` when none is.
    ///
    /// Needed because a trim is *sticky*: a player told to run at 0.96 keeps
    /// running at 0.96 until it is told otherwise, and nothing else in this
    /// sample says whether that has happened. Without it the ladder can only
    /// ever say "trim harder", never "stop trimming".
    pub local_rate: f64,
}

/// What to do about the peer's reported position.
///
/// Internally tagged (`{"kind":"seek","pos_ms":…}`) like
/// [`crate::call::CallSignal`], so a frontend switches on `kind` and uniffi —
/// which has no "arbitrary JSON" type — hands over a closed enum.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, uniffi::Enum)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SyncVerdict {
    /// Leave the player alone.
    Hold,
    /// Take their state wholesale — we missed a command. A repair, not drift.
    Adopt {
        pos_ms: u64,
        playing: bool,
        seq: u64,
    },
    /// Trim the playback rate to close a small gap. An absolute multiplier.
    Nudge { rate: f64 },
    /// Too far out to trim; jump.
    Seek { pos_ms: u64 },
}

/// The whole correction policy, in one pure function so every frontend inherits
/// the same answer and a test can pin it.
///
/// The rules are ordered, and each exists for a case that actually happens:
///  1. **They are behind our latest command.** Their position follows from state
///     we have already superseded, so correcting toward it would undo our own
///     command. We hold, and our next heartbeat brings them up. *This is the
///     ping-pong guard: the side that is right stays silent rather than
///     answering, which is [`crate::presence`]'s `reply: false` rule in another
///     shape.*
///  2. **They are ahead of us.** A command reached them and not us; the
///     heartbeat is the repair path for a lost DM.
///  3. **We lead.** Exactly one side corrects, so the loop provably cannot
///     oscillate. Both sides may still *command* freely — only the automatic
///     correction is one-sided.
///  4. **Either side is paused.** Correcting a paused player is jarring and
///     means nothing.
///  5. **Project and compare** through the clock offset.
///  6. **Never correct by less than the measurement error.** If the round trip
///     is 700 ms, a 200 ms "drift" is indistinguishable from zero and acting on
///     it is chasing noise.
///  7. Inside the deadband, hold.
///  8. Small gap and a player that can be trimmed: trim.
///  9. Otherwise jump — unless we jumped moments ago.
pub fn sync_verdict(s: &SyncSample, t: SyncTuning) -> SyncVerdict {
    if s.peer_seq < s.local_seq {
        return SyncVerdict::Hold;
    }
    if s.peer_seq > s.local_seq {
        return SyncVerdict::Adopt {
            pos_ms: s.peer_pos_ms,
            playing: s.peer_playing,
            seq: s.peer_seq,
        };
    }
    if s.we_lead || !s.local_playing || !s.peer_playing {
        return SyncVerdict::Hold;
    }

    let peer_pos_now = projected_peer_pos_ms(s);
    let drift_ms = local_audible_pos_ms(s) - peer_pos_now;
    let deadband = t.min_deadband_ms.max(s.clock.uncertainty_at(s.now_ms)) as i64;
    if drift_ms.abs() <= deadband {
        // Caught up — so stop trimming. A trim is sticky, and this rule is the
        // only thing that ever takes one off: `trim_rate` cannot return 1.0 for
        // any drift large enough to have provoked it, so without this the
        // player stays permanently at least 2.5% off speed after its first
        // correction, sails through the deadband, and is trimmed back the other
        // way — a sawtooth that never settles. `together_soak` measures it: 191
        // rate changes over two simulated hours before this branch existed, 4
        // after, with the worst drift down from 479 ms to 271 ms. Invisible on
        // video, and an audible tempo wobble on music.
        return if rate_is_neutral(s.local_rate) {
            SyncVerdict::Hold
        } else {
            SyncVerdict::Nudge { rate: 1.0 }
        };
    }

    if drift_ms.unsigned_abs() <= TOGETHER_SEEK_THRESHOLD_MS && t.can_rate_trim {
        return SyncVerdict::Nudge {
            rate: trim_rate(drift_ms),
        };
    }

    if s.now_ms.saturating_sub(s.last_seek_ms) < TOGETHER_MIN_SEEK_INTERVAL_MS {
        // We have just yanked this player. Trimming is the gentler option if it
        // is available at all; otherwise wait rather than seek twice in a breath.
        return if t.can_rate_trim {
            SyncVerdict::Nudge {
                rate: trim_rate(drift_ms),
            }
        } else {
            SyncVerdict::Hold
        };
    }

    SyncVerdict::Seek {
        // Their audible position, expressed as the decoder position *we* must
        // ask for to be audible in the same place.
        pos_ms: (peer_pos_now + s.local_output_latency_ms as i64).max(0) as u64,
    }
}

/// Where the peer's playhead is *now*, in our clock, given what they told us and
/// how long ago that was. Exposed because a UI wants the same number to show a
/// drift figure.
pub fn projected_peer_pos_ms(s: &SyncSample) -> i64 {
    // Their anchor instant, on our clock, with the frequency difference carried
    // forward rather than assumed away.
    let anchored_here = s.peer_at_ms as i64 - s.clock.offset_at(s.now_ms);
    let elapsed = (s.now_ms as i64 - anchored_here).max(0);
    let decoder = s.peer_pos_ms as i64 + if s.peer_playing { elapsed } else { 0 };
    // Compare ear to ear, not decoder to decoder: what each side *hears* is its
    // decoder position minus its own output latency, and those differ per
    // handset.
    decoder - s.peer_output_latency_ms as i64
}

/// Where our own listener actually is — decoder position less our output
/// latency, the same correction applied to the peer above.
pub fn local_audible_pos_ms(s: &SyncSample) -> i64 {
    s.local_pos_ms as i64 - s.local_output_latency_ms as i64
}

/// Whether a reported rate is "no trim applied".
///
/// A tolerance rather than `== 1.0` because the value makes a round trip
/// through a player that is free to quantise it — `MediaPlayer` and the DOM
/// both report back something close to, not identical to, what they were
/// given, and an exact comparison would leave a trim on forever.
fn rate_is_neutral(rate: f64) -> bool {
    !rate.is_finite() || (rate - 1.0).abs() < 0.001
}

/// The rate that closes `drift_ms` over [`TOGETHER_CORRECTION_WINDOW_MS`],
/// clamped so a correction is never audible. Ahead of them means slow down.
fn trim_rate(drift_ms: i64) -> f64 {
    let raw = 1.0 - (drift_ms as f64 / TOGETHER_CORRECTION_WINDOW_MS as f64);
    raw.clamp(1.0 - TOGETHER_MAX_TRIM, 1.0 + TOGETHER_MAX_TRIM)
}

// ── Reading a state change back ──────────────────────────────────────────────

/// What a [`TogetherSignal::State`] turned out to mean, for the UI to narrate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum StateChange {
    Played,
    Paused,
    Seeked,
    /// Both at once — someone scrubbed while paused, or paused after jumping.
    PausedAndSeeked,
    /// Nothing worth telling anyone about.
    Unchanged,
}

/// Derive which of play / pause / seek a state message was, by comparing it with
/// where the playhead was expected to be.
///
/// One tested function rather than three frontends each guessing. The honest
/// imprecision, stated rather than hidden: a pause that happens to coincide with
/// more than [`TOGETHER_SEEK_THRESHOLD_MS`] of accrued drift reads as "paused and
/// skipped". A `was_seek` bit on the wire would remove the ambiguity and was
/// judged not worth a field.
pub fn describe_state_change(
    was_playing: bool,
    now_playing: bool,
    expected_pos_ms: u64,
    actual_pos_ms: u64,
) -> StateChange {
    let jumped = expected_pos_ms.abs_diff(actual_pos_ms) > TOGETHER_SEEK_THRESHOLD_MS;
    match (was_playing, now_playing, jumped) {
        (false, true, _) => StateChange::Played,
        (true, false, true) => StateChange::PausedAndSeeked,
        (true, false, false) => StateChange::Paused,
        (_, _, true) => StateChange::Seeked,
        _ => StateChange::Unchanged,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn env_json(signal: TogetherSignal) -> String {
        TogetherEnvelope::new("abc123", 1, 1_700_000_000_000, None, signal)
            .to_json()
            .unwrap()
    }

    // ── Wire shape ───────────────────────────────────────────────────────────

    #[test]
    fn every_signal_round_trips_through_json() {
        let signals = vec![
            TogetherSignal::Start {
                content: TogetherContent::local_file(7_200_000, Some(Recording::titled("Solaris"))),
                pos_ms: 0,
                playing: false,
            },
            TogetherSignal::Start {
                content: TogetherContent::Youtube {
                    video_id: "dQw4w9WgXcQ".into(),
                },
                pos_ms: 12_000,
                playing: true,
            },
            TogetherSignal::Join,
            TogetherSignal::State {
                pos_ms: 2_520_000,
                playing: true,
                effective_at_ms: Some(1_700_000_000_500),
            },
            TogetherSignal::Heartbeat {
                pos_ms: 2_530_000,
                playing: true,
                applied_seq: 4,
                output_latency_ms: 45,
            },
            TogetherSignal::End,
        ];
        for signal in signals {
            let json = env_json(signal.clone());
            let back = parse_together_envelope(&json).expect("parses");
            assert_eq!(back.signal, signal);
            assert_eq!(back.session_id, "abc123");
            assert_eq!(back.comrade_together, TOGETHER_ENVELOPE_MARKER);
        }
    }

    #[test]
    fn the_clock_echo_round_trips_and_is_absent_when_unset() {
        let with = TogetherEnvelope::new(
            "s",
            2,
            1_000,
            Some(ClockEcho {
                your_at_ms: 900,
                my_recv_ms: 950,
            }),
            TogetherSignal::Join,
        );
        let json = with.to_json().unwrap();
        assert!(json.contains("\"echo\""));
        assert_eq!(parse_together_envelope(&json).unwrap(), with);

        let without = TogetherEnvelope::new("s", 2, 1_000, None, TogetherSignal::Join);
        assert!(!without.to_json().unwrap().contains("echo"));
    }

    /// The privacy claim, asserted rather than promised in a comment: a local
    /// file is described by its length and — only if the sender chose to say —
    /// what recording it is. Never by a filename, a path, a size or a digest.
    /// Adding any of those fails this test.
    #[test]
    fn a_local_file_is_identified_by_its_length_and_what_the_sender_chose_to_say() {
        let bare = serde_json::to_value(TogetherContent::local_file(7_200_000, None)).unwrap();
        let mut keys: Vec<_> = bare.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, vec!["duration_ms", "kind"]);

        let named = serde_json::to_value(TogetherContent::local_file(
            7_200_000,
            Some(Recording::titled("Solaris")),
        ))
        .unwrap();
        let mut keys: Vec<_> = named.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, vec!["duration_ms", "kind", "recording"]);

        // And the recording itself carries only catalogue facts.
        let mut inner: Vec<_> = named["recording"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        inner.sort();
        assert_eq!(inner, vec!["title"]);
    }

    /// `End` says someone left and deliberately not why.
    #[test]
    fn leaving_carries_no_reason() {
        let value = serde_json::to_value(TogetherSignal::End).unwrap();
        let keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, vec!["kind"]);
    }

    #[test]
    fn parse_rejects_chat_text_and_every_sibling_envelope() {
        let not_ours = [
            "hello there",
            "",
            "{}",
            r#"{"comrade_nudge":1,"ttl_secs":480}"#,
            r#"{"comrade_presence":1,"state":"online","ttl_secs":480,"reply":false}"#,
            r#"{"comrade_call":1,"call_id":"x","media":"audio","signal":{"kind":"ringing"}}"#,
            r#"{"comrade_receipt":1}"#,
            // Right shape, wrong marker value — a future version must not be
            // silently reinterpreted as this one.
            r#"{"comrade_together":2,"session_id":"s","seq":1,"at_ms":1,"signal":{"kind":"join"}}"#,
        ];
        for candidate in not_ours {
            assert!(
                parse_together_envelope(candidate).is_none(),
                "should not parse: {candidate}"
            );
        }
    }

    #[test]
    fn a_together_envelope_is_not_mistaken_for_a_sibling() {
        let json = env_json(TogetherSignal::Join);
        assert!(crate::nudge::parse_nudge(&json).is_none());
        assert!(crate::presence::parse_presence_beacon(&json).is_none());
        assert!(crate::call::parse_call_envelope(&json).is_none());
    }

    #[test]
    fn session_ids_are_unguessable_and_distinct() {
        let a = new_session_id();
        let b = new_session_id();
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }

    // ── YouTube ids ──────────────────────────────────────────────────────────

    #[test]
    fn youtube_id_rejects_injection_shaped_input() {
        for bad in [
            "",
            "short",
            "waytoolongforanid",
            "dQw4w9WgXc",                   // ten
            "dQw4w9WgXcQ\"",                // quote
            "\"><script>alert(1)</script>", // the shape that matters
            "dQw4w9WgX<Q",
            "dQw4w9Wg XcQ",
        ] {
            assert!(!valid_youtube_id(bad), "should be invalid: {bad:?}");
            assert!(TogetherContent::youtube(bad).is_none(), "accepted: {bad:?}");
        }
        assert!(valid_youtube_id("dQw4w9WgXcQ"));
        assert!(valid_youtube_id("_-aB3cD4eF5"));
    }

    #[test]
    fn youtube_id_is_found_in_every_link_shape_we_accept() {
        for input in [
            "dQw4w9WgXcQ",
            "  dQw4w9WgXcQ  ",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42s",
            "http://youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ?t=42",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
            "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ",
        ] {
            assert_eq!(
                youtube_id_from_url(input).as_deref(),
                Some("dQw4w9WgXcQ"),
                "failed for {input}"
            );
        }
        assert_eq!(youtube_id_from_url("https://example.com/"), None);
        assert_eq!(youtube_id_from_url("not a link"), None);
    }

    // ── Freshness ────────────────────────────────────────────────────────────

    #[test]
    fn a_signal_from_the_backfill_window_is_never_fresh() {
        let now = 1_800_000_000;
        assert!(signal_is_fresh(now, now));
        assert!(signal_is_fresh(now - TOGETHER_SIGNAL_MAX_AGE_SECS, now));
        assert!(!signal_is_fresh(
            now - TOGETHER_SIGNAL_MAX_AGE_SECS - 1,
            now
        ));
        // The two-day inbox backfill, which is the case this exists for.
        assert!(!signal_is_fresh(now - 172_800, now));
    }

    #[test]
    fn a_session_survives_a_few_dropped_heartbeats_but_not_silence() {
        let t0 = 1_000_000u64;
        let beat = TOGETHER_HEARTBEAT_SECS * 1000;
        assert!(session_is_live_at(t0, t0 + 4 * beat));
        assert!(!session_is_live_at(t0, t0 + TOGETHER_SESSION_TTL_MS));
        assert!(!session_is_live_at(t0, t0 + 10 * beat));
    }

    // ── Arbitration ──────────────────────────────────────────────────────────

    fn stamp(seq: u64, actor: &str, pausing: bool) -> CommandStamp {
        CommandStamp::new(seq, actor, pausing)
    }

    #[test]
    fn a_higher_counter_wins() {
        assert!(stamp(6, "npub1a", false).wins_over(&stamp(5, "npub1b", true)));
        assert!(!stamp(4, "npub1b", true).wins_over(&stamp(5, "npub1a", false)));
    }

    #[test]
    fn a_replayed_command_never_wins_twice() {
        let applied = stamp(5, "npub1a", false);
        assert!(!applied.wins_over(&applied));
    }

    /// The property the whole design rests on: whichever side evaluates it, the
    /// two devices name the same winner. Asserted from both directions.
    #[test]
    fn a_tie_is_resolved_identically_on_both_devices() {
        let cases = [
            (stamp(5, "npub1a", true), stamp(5, "npub1b", false)),
            (stamp(5, "npub1a", false), stamp(5, "npub1b", false)),
            (stamp(5, "npub1b", true), stamp(5, "npub1a", true)),
            (stamp(7, "npub1a", false), stamp(5, "npub1b", true)),
        ];
        for (x, y) in cases {
            assert_ne!(
                x.wins_over(&y),
                y.wins_over(&x),
                "the two devices disagreed about {x:?} vs {y:?}"
            );
        }
    }

    #[test]
    fn a_pause_beats_a_play_issued_at_the_same_moment() {
        let pause = stamp(5, "npub1a", true);
        let play = stamp(5, "npub1z", false);
        assert!(pause.wins_over(&play));
        assert!(!play.wins_over(&pause));
    }

    #[test]
    fn a_scrub_drag_is_coalesced_into_one_command() {
        let t = 10_000u64;
        assert!(command_is_due(0, t));
        assert!(!command_is_due(t, t + TOGETHER_COMMAND_MIN_INTERVAL_MS - 1));
        assert!(command_is_due(t, t + TOGETHER_COMMAND_MIN_INTERVAL_MS));
    }

    // ── The clock ────────────────────────────────────────────────────────────

    /// Synthesise a round trip with a known offset and delay, and demand the
    /// filter recover both exactly.
    #[test]
    fn the_filter_recovers_a_known_offset_and_round_trip() {
        let offset = 4_000i64; // peer's clock is four seconds ahead
        let one_way = 120i64;
        let think = 30i64;

        let t1 = 1_000_000i64;
        let t2 = t1 + one_way + offset;
        let t3 = t2 + think;
        let t4 = t3 - offset + one_way;

        let mut filter = ClockFilter::new();
        let probe = filter
            .observe(t1 as u64, t2 as u64, t3 as u64, t4 as u64)
            .expect("usable probe");
        assert_eq!(probe.offset_ms, offset);
        assert_eq!(probe.rtt_ms, (2 * one_way) as u64);

        let estimate = filter.estimate(t4 as u64);
        assert_eq!(estimate.offset_ms, offset);
        assert_eq!(estimate.uncertainty_at(t4 as u64), one_way as u64);
    }

    #[test]
    fn the_filter_prefers_the_least_delayed_probe() {
        let mut filter = ClockFilter::new();
        // A noisy probe: 2s each way, and a badly skewed apparent offset.
        filter.observe(0, 5_000, 5_010, 4_010).unwrap();
        // A clean one: 50ms each way, true offset 1000.
        let t1 = 10_000i64;
        let t2 = t1 + 50 + 1_000;
        let t3 = t2 + 5;
        let t4 = t3 - 1_000 + 50;
        filter
            .observe(t1 as u64, t2 as u64, t3 as u64, t4 as u64)
            .unwrap();

        let estimate = filter.estimate(t4 as u64);
        assert_eq!(estimate.offset_ms, 1_000);
        assert_eq!(estimate.rtt_ms, Some(100));
    }

    #[test]
    fn the_filter_forgets_probes_older_than_its_window() {
        let mut filter = ClockFilter::new();
        filter.observe(0, 1_000, 1_010, 20).unwrap();
        assert!(filter.estimate(1_000).rtt_ms.is_some());
        assert!(filter
            .estimate(CLOCK_PROBE_WINDOW_MS + 1_000)
            .rtt_ms
            .is_none());
    }

    #[test]
    fn the_filter_ring_stays_bounded_however_long_a_session_runs() {
        let mut filter = ClockFilter::new();
        for i in 0..720u64 {
            let t1 = i * 10_000;
            filter.observe(t1, t1 + 60, t1 + 65, t1 + 125);
        }
        assert_eq!(filter.len(), CLOCK_PROBE_RING);
    }

    #[test]
    fn a_measurement_whose_clocks_run_backwards_is_discarded_not_clamped() {
        let mut filter = ClockFilter::new();
        assert!(filter.observe(1_000, 500, 400, 900).is_none());
        assert!(filter.observe(1_000, 2_000, 1_500, 2_500).is_none());
        assert!(filter.is_empty());
    }

    #[test]
    fn an_unmeasured_clock_never_reports_certainty() {
        let estimate = ClockEstimate::unmeasured();
        assert_eq!(estimate.rtt_ms, None);
        assert_eq!(
            estimate.uncertainty_at(0),
            TOGETHER_UNMEASURED_UNCERTAINTY_MS
        );
        assert!(estimate.uncertainty_at(0) > 0);
        assert_eq!(ClockFilter::new().estimate(0), estimate);
    }

    // ── Frequency, not just phase ────────────────────────────────────────────

    /// Two crystals differing by a known amount must be measured as that
    /// amount — this is the term that used to be silently zero and let a
    /// projection rot between heartbeats.
    #[test]
    fn the_filter_measures_the_two_clocks_frequency_difference() {
        let mut filter = ClockFilter::new();
        let skew_ppm = 40.0; // their clock gains 40µs per second
        let one_way = 25i64;
        let base = 1_000_000i64;
        for i in 0..8i64 {
            let t1 = base + i * 15_000;
            let offset = 1_000.0 + skew_ppm * (t1 - base) as f64 / 1_000_000.0;
            let t2 = t1 + one_way + offset as i64;
            let t3 = t2 + 5;
            let t4 = t3 - offset as i64 + one_way;
            filter.observe(t1 as u64, t2 as u64, t3 as u64, t4 as u64);
        }
        let est = filter.estimate((base + 7 * 15_000) as u64);
        assert!(
            (est.skew_ppm - skew_ppm).abs() < 8.0,
            "expected ~{skew_ppm} ppm, got {}",
            est.skew_ppm
        );
    }

    #[test]
    fn a_frequency_is_not_guessed_from_too_few_or_too_short_a_baseline() {
        // Plenty of probes, but all within a few seconds.
        let mut filter = ClockFilter::new();
        for i in 0..8u64 {
            let t1 = 1_000_000 + i * 500;
            filter.observe(t1, t1 + 60, t1 + 65, t1 + 125);
        }
        assert_eq!(
            filter.estimate(1_004_000).skew_ppm,
            0.0,
            "a slope over a short baseline is jitter, and extrapolating it is worse than nothing"
        );

        // A long baseline but only two probes.
        let mut filter = ClockFilter::new();
        filter.observe(0, 60, 65, 125);
        filter.observe(120_000, 120_060, 120_065, 120_125);
        assert_eq!(filter.estimate(120_125).skew_ppm, 0.0);
    }

    /// A clock being *stepped* is a phase event. Mistaking it for a frequency
    /// and extrapolating would keep pushing the playhead long after the step.
    #[test]
    fn a_clock_step_is_clamped_rather_than_extrapolated_forever() {
        let mut filter = ClockFilter::new();
        let base = 1_000_000i64;
        for i in 0..8i64 {
            let t1 = base + i * 15_000;
            // Half-way through, the peer's clock jumps a whole second.
            let offset = if i < 4 { 0i64 } else { 1_000 };
            let t2 = t1 + 25 + offset;
            let t3 = t2 + 5;
            let t4 = t3 - offset + 25;
            filter.observe(t1 as u64, t2 as u64, t3 as u64, t4 as u64);
        }
        let est = filter.estimate((base + 7 * 15_000) as u64);
        assert!(
            est.skew_ppm.abs() <= CLOCK_MAX_SKEW_PPM,
            "a step must not read as an unbounded frequency: {}",
            est.skew_ppm
        );
    }

    #[test]
    fn the_offset_is_carried_forward_at_the_measured_rate() {
        let est = ClockEstimate {
            offset_ms: 100,
            rtt_ms: Some(40),
            skew_ppm: 50.0,
            measured_at_ms: 1_000_000,
        };
        assert_eq!(est.offset_at(1_000_000), 100);
        // 60 s later at 50 ppm the peer's clock has gained 3 ms.
        assert_eq!(est.offset_at(1_060_000), 103);
    }

    #[test]
    fn an_estimate_gets_less_certain_as_it_ages() {
        let est = ClockEstimate {
            offset_ms: 0,
            rtt_ms: Some(40),
            skew_ppm: 0.0,
            measured_at_ms: 1_000_000,
        };
        let fresh = est.uncertainty_at(1_000_000);
        let stale = est.uncertainty_at(1_600_000);
        assert_eq!(fresh, 20);
        assert!(
            stale > fresh,
            "a ten-minute-old estimate is not as good as new"
        );
        // An unmeasured clock is pessimistic and never claims to age well.
        assert_eq!(
            ClockEstimate::unmeasured().uncertainty_at(9_999_999),
            TOGETHER_UNMEASURED_UNCERTAINTY_MS
        );
    }

    /// beatsync's filter: the best half by round trip, averaged — so one lucky
    /// sample does not decide the answer on its own.
    #[test]
    fn the_offset_averages_the_best_half_rather_than_one_lucky_probe() {
        let mut filter = ClockFilter::new();
        // Four clean probes around a true offset of 1000, and four badly
        // delayed ones whose apparent offsets are far off.
        for (i, (rtt, offset)) in [
            (40i64, 1_010i64),
            (44, 990),
            (42, 1_004),
            (46, 996),
            (900, 1_400),
            (950, 600),
            (1_000, 1_500),
            (1_100, 500),
        ]
        .into_iter()
        .enumerate()
        {
            let t1 = 1_000_000i64 + i as i64 * 1_000;
            let t2 = t1 + rtt / 2 + offset;
            let t3 = t2;
            let t4 = t3 - offset + rtt / 2;
            filter.observe(t1 as u64, t2 as u64, t3 as u64, t4 as u64);
        }
        let est = filter.estimate(1_010_000);
        assert!(
            (est.offset_ms - 1_000).abs() <= 15,
            "the delayed probes should not move the answer: {}",
            est.offset_ms
        );
        // The uncertainty still comes from the *best* evidence, not the mean.
        assert_eq!(est.rtt_ms, Some(40));
    }

    /// The improvement on beatsync's filter: offsets taken minutes apart are not
    /// samples of one quantity once the clocks differ in rate, so they are
    /// de-skewed to a common instant before averaging. Without this the average
    /// would smear the frequency back into the phase.
    #[test]
    fn offsets_are_de_skewed_before_they_are_averaged() {
        let mut filter = ClockFilter::new();
        let skew = 100.0f64; // deliberately large, so the effect is visible
        let base = 1_000_000i64;
        for i in 0..8i64 {
            let t1 = base + i * 15_000;
            let offset = 1_000.0 + skew * (t1 - base) as f64 / 1_000_000.0;
            let t2 = t1 + 20 + offset as i64;
            let t3 = t2;
            let t4 = t3 - offset as i64 + 20;
            filter.observe(t1 as u64, t2 as u64, t3 as u64, t4 as u64);
        }
        let est = filter.estimate((base + 7 * 15_000) as u64);
        // The reported offset must be the one that holds at `measured_at_ms`,
        // and projecting it forward must land on the true offset there.
        let latest = (base + 7 * 15_000) as u64;
        let projected = est.offset_at(latest);
        let truth = 1_000.0 + skew * (7 * 15_000) as f64 / 1_000_000.0;
        assert!(
            (projected as f64 - truth).abs() < 3.0,
            "projected {projected} vs true {truth}"
        );
    }

    #[test]
    fn the_heartbeat_bursts_until_the_clock_has_converged_then_settles() {
        assert_eq!(heartbeat_interval_ms(0), TOGETHER_BURST_INTERVAL_MS);
        assert_eq!(
            heartbeat_interval_ms(CLOCK_BURST_PROBES - 1),
            TOGETHER_BURST_INTERVAL_MS
        );
        assert_eq!(
            heartbeat_interval_ms(CLOCK_BURST_PROBES),
            TOGETHER_HEARTBEAT_SECS * 1000
        );
    }

    // ── Recording identity (adapted from Antra) ─────────────────────────────

    #[test]
    fn an_isrc_is_recognised_in_either_printed_form() {
        assert!(valid_isrc("GBAYE0601498"));
        assert!(valid_isrc("GB-AYE-06-01498"));
        assert_eq!(
            normalise_isrc("gb-aye-06-01498").as_deref(),
            Some("GBAYE0601498")
        );
        for bad in [
            "",
            "GBAYE060149",
            "GBAYE06014988",
            "GB-AYE-06-0149X",
            "12AYE0601498",
        ] {
            assert!(!valid_isrc(bad), "should be invalid: {bad}");
            assert!(normalise_isrc(bad).is_none());
        }
    }

    fn rec(isrc: Option<&str>, title: &str, artist: &str) -> Recording {
        Recording {
            isrc: isrc.map(|s| s.to_string()),
            title: title.into(),
            artist: artist.into(),
            album: None,
        }
    }

    /// The whole reason to carry an ISRC: it is decisive, in both directions.
    #[test]
    fn two_agreeing_isrcs_settle_it_and_two_disagreeing_ones_settle_it_too() {
        let want = rec(Some("GBAYE0601498"), "Anything", "Anyone");
        let same = rec(
            Some("gb-aye-06-01498"),
            "Totally Different Title",
            "Someone Else",
        );
        assert_eq!(match_score(&want, &same, 200_000, 999_000), 1.0);

        let other = rec(Some("USRC17607839"), "Anything", "Anyone");
        assert_eq!(
            match_score(&want, &other, 200_000, 200_000),
            0.0,
            "two ISRCs that disagree are different recordings, however alike they look"
        );
    }

    #[test]
    fn without_an_isrc_a_close_title_and_artist_is_enough_to_open() {
        let want = rec(None, "Teardrop", "Massive Attack");
        let have = rec(None, "Teardrop (Remastered)", "Massive Attack");
        let score = match_score(&want, &have, 330_000, 331_000);
        assert!(score >= MATCH_CONFIDENT, "score was {score}");
    }

    #[test]
    fn a_different_song_by_the_same_artist_is_not_opened_on_someones_behalf() {
        let want = rec(None, "Teardrop", "Massive Attack");
        let have = rec(None, "Angel", "Massive Attack");
        let score = match_score(&want, &have, 330_000, 380_000);
        assert!(
            score < MATCH_CONFIDENT,
            "opening the wrong track is worse than asking: {score}"
        );
    }

    /// A remaster is the same performance; a live take is not. The scorer has
    /// to tell those apart or it will cheerfully open the wrong one.
    #[test]
    fn a_remaster_is_the_same_recording_but_a_live_take_is_not() {
        let want = rec(None, "Teardrop", "Massive Attack");
        let remaster = rec(None, "Teardrop (2011 Remastered Version)", "Massive Attack");
        assert!(
            match_score(&want, &remaster, 330_000, 330_000) >= MATCH_CONFIDENT,
            "a remaster should still open"
        );
        for variant in [
            "Teardrop (Live)",
            "Teardrop - Live at Glastonbury",
            "Teardrop (Acoustic)",
            "Teardrop (Tricky Remix)",
        ] {
            let other = rec(None, variant, "Massive Attack");
            let score = match_score(&want, &other, 330_000, 330_000);
            assert!(
                score < MATCH_CONFIDENT,
                "{variant} is a different recording, scored {score}"
            );
        }
    }

    #[test]
    fn a_radio_edit_is_separated_from_the_album_cut_by_its_length() {
        let want = rec(None, "Paranoid Android", "Radiohead");
        let have = rec(None, "Paranoid Android", "Radiohead");
        let album = match_score(&want, &have, 383_000, 383_000);
        let edit = match_score(&want, &have, 383_000, 240_000);
        assert!(
            album > edit,
            "duration must break the tie: {album} vs {edit}"
        );
    }

    #[test]
    fn a_missing_artist_neither_helps_nor_hurts() {
        let want = rec(None, "Solaris", "");
        let have = rec(None, "Solaris", "Cliff Martinez");
        let score = match_score(&want, &have, 100_000, 100_000);
        assert!(score >= MATCH_CONFIDENT, "score was {score}");
    }

    #[test]
    fn a_label_reads_the_way_a_person_would_write_it() {
        assert_eq!(
            rec(None, "Teardrop", "Massive Attack").label(),
            "Massive Attack — Teardrop"
        );
        assert_eq!(Recording::titled("home video").label(), "home video");
    }

    // ── Streaming links, resolved to identity and nothing else ──────────────

    #[test]
    fn a_spotify_track_link_is_recognised() {
        for url in [
            "https://open.spotify.com/track/6habFhsOp2NvshLv26DqMb",
            "https://open.spotify.com/track/6habFhsOp2NvshLv26DqMb?si=abc123",
            "open.spotify.com/track/6habFhsOp2NvshLv26DqMb",
        ] {
            assert_eq!(
                parse_music_link(url),
                Some(MusicLink::Spotify {
                    track_id: "6habFhsOp2NvshLv26DqMb".into()
                }),
                "failed for {url}"
            );
        }
        // A playlist or album is not a track.
        assert_eq!(parse_music_link("https://open.spotify.com/album/abc"), None);
    }

    #[test]
    fn an_apple_music_track_link_keeps_its_storefront() {
        assert_eq!(
            parse_music_link("https://music.apple.com/in/album/mezzanine/1440931401?i=1440931493"),
            Some(MusicLink::AppleMusic {
                storefront: "in".into(),
                track_id: "1440931493".into(),
            }),
        );
        // An album link names no single recording.
        assert_eq!(
            parse_music_link("https://music.apple.com/in/album/mezzanine/1440931401"),
            None
        );
    }

    #[test]
    fn a_youtube_link_is_the_one_that_is_also_playable() {
        let link = parse_music_link("https://music.youtube.com/watch?v=dQw4w9WgXcQ").unwrap();
        assert_eq!(
            link,
            MusicLink::Youtube {
                video_id: "dQw4w9WgXcQ".into()
            }
        );
        // The embed needs no account, so it is the one source that works
        // between two people who share nothing but a link.
        assert_eq!(
            link.playhead_control(&ServiceAccess::none()),
            PlayheadControl::Full
        );
    }

    // ── What a device can actually drive ────────────────────────────────────

    fn spotify() -> MusicLink {
        MusicLink::Spotify {
            track_id: "6habFhsOp2NvshLv26DqMb".into(),
        }
    }

    fn apple() -> MusicLink {
        MusicLink::AppleMusic {
            storefront: "in".into(),
            track_id: "1440931493".into(),
        }
    }

    /// The correction this whole model turns on: it is the *device* that decides
    /// whether a service track is playable, not the URL. This is how Spotify's
    /// own Jam works and how every third-party clone of it works — each
    /// participant plays the track from their own client on their own
    /// subscription, and only control events travel.
    #[test]
    fn a_service_track_is_playable_exactly_when_this_device_is_signed_in() {
        assert_eq!(
            spotify().playhead_control(&ServiceAccess::none()),
            PlayheadControl::None,
            "no account is no session, and must not read as one",
        );
        assert_eq!(
            spotify().playhead_control(&ServiceAccess {
                spotify: true,
                apple_music: false,
            }),
            PlayheadControl::Full,
        );
    }

    /// Apple Music is never `Full`, signed in or not. MusicKit exposes no way to
    /// place a playhead at a named moment, so a session on it could be started
    /// and never held — and a correction ladder running against a player that
    /// cannot seek produces verdicts nothing applies.
    #[test]
    fn apple_music_can_be_started_and_never_placed() {
        let connected = ServiceAccess {
            spotify: false,
            apple_music: true,
        };
        assert_eq!(
            apple().playhead_control(&connected),
            PlayheadControl::StartOnly
        );
        assert!(!apple().playhead_control(&connected).corrects());
        assert!(apple().playhead_control(&connected).playable());
        assert_eq!(
            apple().playhead_control(&ServiceAccess::none()),
            PlayheadControl::None
        );
    }

    #[test]
    fn one_service_being_connected_says_nothing_about_the_other() {
        // The two are separate subscriptions and separate SDKs; a shared
        // boolean here would open a Spotify session on an Apple Music account.
        let only_spotify = ServiceAccess {
            spotify: true,
            apple_music: false,
        };
        assert_eq!(
            apple().playhead_control(&only_spotify),
            PlayheadControl::None
        );
        let only_apple = ServiceAccess {
            spotify: false,
            apple_music: true,
        };
        assert_eq!(
            spotify().playhead_control(&only_apple),
            PlayheadControl::None
        );
    }

    #[test]
    fn only_a_full_playhead_runs_the_drift_ladder() {
        // `corrects()` gates the ladder, so a source that cannot seek must not
        // pass it — otherwise the screen says "catching up…" forever while
        // nothing catches up.
        assert!(PlayheadControl::Full.corrects());
        assert!(!PlayheadControl::StartOnly.corrects());
        assert!(!PlayheadControl::None.corrects());
        assert!(PlayheadControl::Full.playable());
        assert!(PlayheadControl::StartOnly.playable());
        assert!(!PlayheadControl::None.playable());
    }

    // ── A public URL both sides fetch for themselves ────────────────────────

    #[test]
    fn a_stream_syncs_as_tightly_as_a_local_file() {
        // The point of the variant. A podcast episode is an ordinary media
        // element on an HTTPS URL, so it reports an accurate position and takes
        // any rate — the fine ladder, not the coarse one a vendor SDK forces.
        let stream = TogetherContent::Stream {
            url: "https://feeds.example.com/ep/42.mp3".into(),
            recording: None,
            duration_ms: Some(3_600_000),
        };
        assert_eq!(
            stream.tuning(),
            TogetherContent::local_file(3_600_000, None).tuning(),
        );
        assert!(stream.tuning().can_rate_trim);
        assert_eq!(stream.duration_ms(), Some(3_600_000));
    }

    #[test]
    fn a_feed_that_did_not_say_how_long_costs_only_the_warning() {
        let stream = TogetherContent::Stream {
            url: "https://feeds.example.com/ep/42.mp3".into(),
            recording: None,
            duration_ms: None,
        };
        assert_eq!(stream.duration_ms(), None);
    }

    #[test]
    fn an_ordinary_episode_url_is_accepted() {
        for url in [
            "https://feeds.example.com/ep/42.mp3",
            "https://archive.org/download/item/track%2001.flac",
            "https://cdn.example.co.uk:8443/a/b.m4a?token=abc-123&t=9",
            "https://example.com",
            "https://sub.domain.example.org/x#t=30",
        ] {
            assert!(valid_stream_url(url), "refused a real one: {url}");
        }
    }

    /// Each case here is a thing a hostile invitation would otherwise buy,
    /// because this string becomes a media element's `src` on the strength of a
    /// peer having sent it.
    #[test]
    fn a_stream_url_refuses_everything_that_is_not_a_public_https_name() {
        for url in [
            // Scheme: a downgrade, the listener's disk, and two flavours of
            // script execution in whichever frontend is least careful.
            "http://feeds.example.com/ep.mp3",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:audio/mp3;base64,AAAA",
            "ftp://example.com/a.mp3",
            "//example.com/a.mp3",
            "",
            // Credentials, and the `@` trick that hides the real host.
            "https://user:pass@example.com/a.mp3",
            "https://example.com@evil.test/a.mp3",
            // Hosts that only exist inside the listener's house.
            "https://localhost/a.mp3",
            "https://router/reboot",
            "https://127.0.0.1/a.mp3",
            "https://10.0.0.7/a.mp3",
            "https://192.168.1.1/admin",
            "https://169.254.169.254/latest/meta-data/",
            "https://[::1]/a.mp3",
            // Malformed authorities.
            "https:///a.mp3",
            "https://.example.com/a.mp3",
            "https://example.com./a.mp3",
            // Injection shapes, for the frontend that builds an attribute by
            // concatenation.
            "https://example.com/a\"onerror=x.mp3",
            "https://example.com/<script>.mp3",
            "https://example.com/a b.mp3",
            "https://example.com/a\nb.mp3",
            "https://exa mple.com/a.mp3",
        ] {
            assert!(!valid_stream_url(url), "accepted a hostile one: {url:?}");
        }
    }

    #[test]
    fn a_stream_url_is_bounded() {
        let long = format!("https://example.com/{}", "a".repeat(STREAM_URL_MAX_LEN));
        assert!(!valid_stream_url(&long));
        // And a realistically long one with tracking junk still passes.
        let real = format!("https://cdn.example.com/ep.mp3?{}", "k=v&".repeat(200));
        assert!(real.len() < STREAM_URL_MAX_LEN);
        assert!(valid_stream_url(&real));
    }

    #[test]
    fn the_scheme_is_matched_without_regard_to_case_and_the_path_is_not() {
        assert!(valid_stream_url("HTTPS://example.com/A.MP3"));
        // The path's case is the server's business — this must not normalise it
        // and it must not refuse it.
        assert!(valid_stream_url(
            "https://example.com/CaseSensitive/Path.mp3"
        ));
    }

    /// The line between "paste an episode" and "search for a song", which is
    /// what [`direct_media_url`] exists to draw. Note `https://example.com`
    /// among the refusals: it passes [`valid_stream_url`], and opening a player
    /// on it is the hang this branch exists to avoid.
    #[test]
    fn only_a_url_that_names_media_becomes_a_stream() {
        for url in [
            "https://feeds.example.com/ep/42.mp3",
            "https://cdn.example.co.uk:8443/a/b.m4a?token=abc-123&t=9",
            "https://archive.org/download/item/side-a.flac",
            "https://example.org/v/clip.webm#t=30",
            // Case is the server's business; the suffix test must not care.
            "https://example.com/EPISODE.MP3",
        ] {
            assert!(direct_media_url(url), "refused an episode: {url}");
            assert!(matches!(
                TogetherContent::stream(url),
                Some(TogetherContent::Stream { .. })
            ));
        }
        for url in [
            // Safe, and there is no media at the end of it.
            "https://example.com",
            "https://example.com/episodes/42",
            "https://example.com/show.html",
            "https://example.com/feed.xml",
            // A query string that merely mentions an extension is not one.
            "https://example.com/watch?file=a.mp3",
            // A *host* that ends in one is not one either — a well-formed URL
            // with a nonexistent TLD, i.e. a typo, which belongs in a search
            // rather than in a player.
            "https://example.mp3",
            "https://example.mp3?x=1",
            // Everything valid_stream_url already refuses stays refused.
            "http://example.com/ep.mp3",
            "https://127.0.0.1/ep.mp3",
            "https://example.com/a b.mp3",
            "",
        ] {
            assert!(!direct_media_url(url), "accepted a non-episode: {url:?}");
            assert!(TogetherContent::stream(url).is_none());
        }
    }

    #[test]
    fn a_pasted_stream_names_nothing_it_was_not_told() {
        // A filename is not a title — the position `LocalFile` already takes —
        // and a length is the player's to discover.
        let Some(TogetherContent::Stream {
            url,
            recording,
            duration_ms,
        }) = TogetherContent::stream("  https://example.com/Kun-Faya-Kun.mp3  ")
        else {
            panic!("a real episode URL was refused");
        };
        assert_eq!(url, "https://example.com/Kun-Faya-Kun.mp3");
        assert!(recording.is_none());
        assert!(duration_ms.is_none());
    }

    #[test]
    fn a_service_session_syncs_as_coarsely_as_an_embed() {
        // Both are third-party players driven through an SDK: no rate trim is
        // expressible and the reported position is event-driven rather than
        // continuous, so the deadband must not be the local-file one.
        let service = TogetherContent::Service {
            link: spotify(),
            recording: None,
        };
        assert_eq!(
            service.tuning(),
            TogetherContent::Youtube {
                video_id: "dQw4w9WgXcQ".into()
            }
            .tuning(),
        );
        assert!(!service.tuning().can_rate_trim);
        // And it claims no duration: the two devices may be handed different
        // masters of the same recording.
        assert_eq!(service.duration_ms(), None);
    }

    #[test]
    fn anything_else_is_not_a_music_link() {
        for url in [
            "",
            "https://example.com/track/123",
            "not a link",
            "https://open.spotify.com/",
        ] {
            assert_eq!(parse_music_link(url), None, "should not parse: {url}");
        }
    }

    // ── Timelines, not positions ─────────────────────────────────────────────

    #[test]
    fn a_timeline_is_evaluated_at_the_asking_time() {
        assert_eq!(timeline_pos_ms(60_000, 1_000, true, 1_500), 60_500);
        assert_eq!(timeline_pos_ms(60_000, 1_000, false, 9_999), 60_000);
        // Asking about the past never runs the playhead backwards.
        assert_eq!(timeline_pos_ms(60_000, 5_000, true, 1_000), 60_000);
    }

    /// The improvement over every position-swapping protocol, asserted: a
    /// command that took 300 ms to arrive lands where the sender *is*, not where
    /// they *were*.
    #[test]
    fn a_late_command_lands_where_the_sender_is_now() {
        let clock = ClockEstimate {
            offset_ms: 0,
            rtt_ms: Some(600),
            skew_ppm: 0.0,
            measured_at_ms: 1_000_000,
        };
        // They said "playing, at 60.000s" effective at their 1_000_000; we are
        // reading it 300 ms later.
        let apply = command_apply(60_000, 1_000_000, true, &clock, 1_000_300);
        assert_eq!(
            apply,
            CommandApply::Now {
                pos_ms: 60_300,
                playing: true
            },
            "the flight time must be added back, or every command lands late"
        );
        // A paused command has no timeline to advance.
        assert_eq!(
            command_apply(60_000, 1_000_000, false, &clock, 1_000_300),
            CommandApply::Now {
                pos_ms: 60_000,
                playing: false
            }
        );
    }

    /// On a transport fast enough to schedule ahead, both players change state
    /// on the same instant instead of one chasing the other.
    #[test]
    fn a_command_scheduled_ahead_is_waited_for() {
        let clock = ClockEstimate {
            offset_ms: 0,
            rtt_ms: Some(4),
            skew_ppm: 0.0,
            measured_at_ms: 1_000_000,
        };
        assert_eq!(
            command_apply(60_000, 1_000_050, true, &clock, 1_000_000),
            CommandApply::At {
                pos_ms: 60_000,
                playing: true,
                in_ms: 50
            }
        );
    }

    #[test]
    fn a_command_scheduled_absurdly_far_ahead_is_applied_instead_of_trusted() {
        let clock = ClockEstimate {
            offset_ms: 0,
            rtt_ms: Some(4),
            skew_ppm: 0.0,
            measured_at_ms: 1_000_000,
        };
        let apply = command_apply(60_000, 1_000_000 + 3_600_000, true, &clock, 1_000_000);
        assert!(
            matches!(apply, CommandApply::Now { .. }),
            "we do not hold an hour-long timer on a peer's say-so"
        );
    }

    #[test]
    fn the_clock_offset_is_applied_when_scheduling() {
        // Their clock reads four seconds ahead of ours.
        let clock = ClockEstimate {
            offset_ms: 4_000,
            rtt_ms: Some(4),
            skew_ppm: 0.0,
            measured_at_ms: 1_000_000,
        };
        // "Effective at my 1_004_050" == our 1_000_050.
        assert_eq!(
            command_apply(60_000, 1_004_050, true, &clock, 1_000_000),
            CommandApply::At {
                pos_ms: 60_000,
                playing: true,
                in_ms: 50
            }
        );
    }

    // ── Ear to ear, not decoder to decoder ───────────────────────────────────

    /// Two players agreeing perfectly on decoder position can still be a tenth
    /// of a second apart in the room, because their audio stacks buffer
    /// differently. Nothing browser-based can see this; a native player can.
    #[test]
    fn output_latency_is_compensated_on_both_sides() {
        let mut s = follower(60_000, 60_000, measured(20));
        // Decoders agree exactly, so a naive implementation sees zero drift…
        assert_eq!(sync_verdict(&s, fine()), SyncVerdict::Hold);
        // …but ours is 30 ms behind the speaker and theirs is 130 ms behind, so
        // what the two people *hear* is 100 ms apart.
        s.local_output_latency_ms = 30;
        s.peer_output_latency_ms = 130;
        let drift = local_audible_pos_ms(&s) - projected_peer_pos_ms(&s);
        assert_eq!(
            drift, 100,
            "we are ahead by the difference in their latencies"
        );
    }

    #[test]
    fn a_seek_targets_the_decoder_position_that_lands_audibly_in_step() {
        let mut s = follower(90_000, 60_000, measured(20));
        s.local_output_latency_ms = 200;
        let SyncVerdict::Seek { pos_ms } = sync_verdict(&s, fine()) else {
            panic!("expected a seek");
        };
        // Their ear is at 60_000; ours lags its decoder by 200 ms, so the
        // decoder has to be asked for 60_200 to be audible in the same place.
        assert_eq!(pos_ms, 60_200);
    }

    // ── The verdict ──────────────────────────────────────────────────────────

    fn measured(rtt_ms: u64) -> ClockEstimate {
        ClockEstimate {
            offset_ms: 0,
            rtt_ms: Some(rtt_ms),
            skew_ppm: 0.0,
            measured_at_ms: 1_000_000,
        }
    }

    /// A follower, both playing, clocks agreed, nothing seeked recently.
    fn follower(local_pos_ms: u64, peer_pos_ms: u64, clock: ClockEstimate) -> SyncSample {
        SyncSample {
            local_pos_ms,
            local_playing: true,
            local_seq: 3,
            peer_pos_ms,
            peer_playing: true,
            peer_seq: 3,
            peer_at_ms: 1_000_000,
            now_ms: 1_000_000,
            last_seek_ms: 0,
            clock,
            we_lead: false,
            local_output_latency_ms: 0,
            peer_output_latency_ms: 0,
            local_rate: 1.0,
        }
    }

    fn fine() -> SyncTuning {
        TogetherContent::local_file(1, None).tuning()
    }

    fn coarse() -> SyncTuning {
        TogetherContent::Youtube {
            video_id: "dQw4w9WgXcQ".into(),
        }
        .tuning()
    }

    // ── What a direct channel may carry ─────────────────────────────────────

    #[test]
    fn a_direct_channel_may_not_carry_an_invitation() {
        assert!(!direct_signal_admissible(&TogetherSignal::Start {
            content: TogetherContent::local_file(1000, None),
            pos_ms: 0,
            playing: true,
        }));
    }

    /// Every other signal must pass, and the list is exhaustive on purpose: a
    /// new signal kind should have to decide which side of this line it is on
    /// rather than inheriting an answer.
    #[test]
    fn a_direct_channel_may_carry_every_other_signal() {
        let allowed = [
            TogetherSignal::Join,
            TogetherSignal::State {
                pos_ms: 1,
                playing: true,
                effective_at_ms: None,
            },
            TogetherSignal::Heartbeat {
                pos_ms: 1,
                playing: true,
                applied_seq: 1,
                output_latency_ms: 0,
            },
            TogetherSignal::End,
        ];
        for signal in allowed {
            assert!(
                direct_signal_admissible(&signal),
                "{} was refused the fast path",
                signal.kind_str(),
            );
        }
    }

    // ── When a declared channel stops being believed ────────────────────────

    #[test]
    fn a_freshly_declared_channel_is_trusted_before_it_has_proved_anything() {
        // The grace window. A frontend that has just negotiated a channel has
        // received nothing on it yet, and refusing it on that ground would mean
        // the fast path could never be taken at all.
        assert!(direct_path_live(1_000_000, 1_000_000));
        assert!(direct_path_live(1_000_000, 1_000_000 + 9_999));
    }

    #[test]
    fn a_channel_silent_for_two_heartbeats_is_no_longer_believed() {
        let declared = 1_000_000;
        // One heartbeat of silence is ordinary: a beat can be late.
        assert!(direct_path_live(
            declared,
            declared + TOGETHER_HEARTBEAT_SECS * 1000
        ));
        // Two is not. This is the case the whole constant exists for — the
        // frontend never reported the close, so nothing else would notice.
        assert!(!direct_path_live(
            declared,
            declared + TOGETHER_DIRECT_SILENCE_MS
        ));
        assert!(!direct_path_live(
            declared,
            declared + TOGETHER_DIRECT_SILENCE_MS + 1
        ));
    }

    // The property that makes the watchdog safe to have at all — it must fire
    // with a full heartbeat to spare — is a compile-time invariant beside the
    // constants rather than a test here, matching the four already there.

    #[test]
    fn traffic_on_the_channel_earns_it_back_without_the_frontend_saying_anything() {
        let declared = 1_000_000;
        let long_gone = declared + TOGETHER_DIRECT_SILENCE_MS + 5_000;
        assert!(!direct_path_live(declared, long_gone));
        // One envelope arriving over the channel is the evidence, and it is the
        // peer's traffic rather than ours — so nothing we stopped sending can
        // keep this from happening.
        assert!(direct_path_live(long_gone, long_gone + 1_000));
    }

    #[test]
    fn a_clock_that_went_backwards_does_not_read_as_silence() {
        // `now_ms` is a wall clock and can step behind a stamp taken moments
        // ago. Saturating means that reads as "no time has passed", which keeps
        // the channel; the alternative underflows to a huge gap and drops a
        // perfectly good path over an NTP correction.
        assert!(direct_path_live(1_000_000, 999_000));
    }

    #[test]
    fn a_gap_inside_the_deadband_is_left_alone() {
        let s = follower(60_000, 60_100, measured(100));
        assert_eq!(sync_verdict(&s, fine()), SyncVerdict::Hold);
    }

    /// A rate trim is sticky — the player keeps it until told otherwise — so
    /// arriving back inside the deadband has to *say* so. Without this the
    /// follower stays permanently off-speed after its first correction and
    /// sawtooths across the deadband forever; `together_soak` measures the
    /// difference over two hours (191 rate changes against 4).
    #[test]
    fn coming_back_inside_the_deadband_takes_the_trim_off() {
        let mut s = follower(60_000, 60_100, measured(100));
        s.local_rate = 0.96;
        assert_eq!(sync_verdict(&s, fine()), SyncVerdict::Nudge { rate: 1.0 });
    }

    #[test]
    fn an_untrimmed_player_inside_the_deadband_is_still_left_alone() {
        // The steady state has to stay silent, or a ten-second heartbeat
        // becomes a periodic producer on the critical bus.
        let s = follower(60_000, 60_100, measured(100));
        assert_eq!(sync_verdict(&s, fine()), SyncVerdict::Hold);
    }

    /// The rate makes a round trip through a player free to quantise it, so
    /// "neutral" is a tolerance. An exact comparison against 1.0 would leave a
    /// trim on forever on any device that rounds.
    #[test]
    fn a_rate_a_player_rounded_still_counts_as_neutral() {
        let mut s = follower(60_000, 60_100, measured(100));
        for reported in [1.0, 0.9999, 1.0001] {
            s.local_rate = reported;
            assert_eq!(
                sync_verdict(&s, fine()),
                SyncVerdict::Hold,
                "{reported} should read as no trim applied",
            );
        }
    }

    /// Taking a trim off must not resurrect a player the user paused, and must
    /// not fight a leader who is not correcting at all.
    #[test]
    fn a_trim_is_not_taken_off_by_someone_who_is_not_correcting() {
        let mut paused = follower(60_000, 60_100, measured(100));
        paused.local_rate = 0.96;
        paused.local_playing = false;
        assert_eq!(sync_verdict(&paused, fine()), SyncVerdict::Hold);

        let mut leader = follower(60_000, 60_100, measured(100));
        leader.local_rate = 0.96;
        leader.we_lead = true;
        assert_eq!(sync_verdict(&leader, fine()), SyncVerdict::Hold);
    }

    /// The central honesty claim of this module, pinned: with a two-second round
    /// trip, a 400 ms disagreement is indistinguishable from none, and acting on
    /// it would be chasing noise. This is also why §8.2's "< 300 ms" target is
    /// unreachable over a relay — 300 ms sits under the measurement error.
    #[test]
    fn never_corrects_below_its_own_measurement_error() {
        let s = follower(60_400, 60_000, measured(2_000));
        assert_eq!(sync_verdict(&s, fine()), SyncVerdict::Hold);
        // The very same drift, measured well, is worth correcting.
        let s = follower(60_400, 60_000, measured(100));
        assert!(matches!(
            sync_verdict(&s, fine()),
            SyncVerdict::Nudge { .. }
        ));
    }

    #[test]
    fn a_small_gap_is_trimmed_and_never_audibly_so() {
        // We are 1s ahead: slow down, but only within the trim band.
        let ahead = follower(61_000, 60_000, measured(100));
        match sync_verdict(&ahead, fine()) {
            SyncVerdict::Nudge { rate } => {
                assert!(rate < 1.0, "should slow down, got {rate}");
                assert!(rate >= 1.0 - TOGETHER_MAX_TRIM);
            }
            other => panic!("expected a trim, got {other:?}"),
        }
        // We are 1s behind: speed up, still bounded.
        let behind = follower(59_000, 60_000, measured(100));
        match sync_verdict(&behind, fine()) {
            SyncVerdict::Nudge { rate } => {
                assert!(rate > 1.0, "should speed up, got {rate}");
                assert!(rate <= 1.0 + TOGETHER_MAX_TRIM);
            }
            other => panic!("expected a trim, got {other:?}"),
        }
    }

    #[test]
    fn a_gap_past_trimming_is_jumped() {
        let s = follower(90_000, 60_000, measured(100));
        assert_eq!(
            sync_verdict(&s, fine()),
            SyncVerdict::Seek { pos_ms: 60_000 }
        );
    }

    #[test]
    fn a_player_that_cannot_be_trimmed_only_ever_holds_or_seeks() {
        // Inside the coarse deadband: hold.
        let near = follower(60_000, 60_900, measured(100));
        assert_eq!(sync_verdict(&near, coarse()), SyncVerdict::Hold);
        // Outside it: a seek, never a trim it could not perform.
        let far = follower(62_000, 60_000, measured(100));
        assert_eq!(
            sync_verdict(&far, coarse()),
            SyncVerdict::Seek { pos_ms: 60_000 }
        );
    }

    #[test]
    fn a_seek_storm_is_capped_by_the_cooldown() {
        let mut s = follower(90_000, 60_000, measured(100));
        s.last_seek_ms = s.now_ms - 1_000;
        // A trimmable player rides it out gently rather than jumping again.
        assert!(matches!(
            sync_verdict(&s, fine()),
            SyncVerdict::Nudge { .. }
        ));
        // One that cannot trim simply waits, rather than yanking twice.
        assert_eq!(sync_verdict(&s, coarse()), SyncVerdict::Hold);
    }

    #[test]
    fn nothing_is_corrected_while_either_side_is_paused() {
        let mut s = follower(90_000, 60_000, measured(100));
        s.local_playing = false;
        assert_eq!(sync_verdict(&s, fine()), SyncVerdict::Hold);

        let mut s = follower(90_000, 60_000, measured(100));
        s.peer_playing = false;
        assert_eq!(sync_verdict(&s, fine()), SyncVerdict::Hold);
    }

    #[test]
    fn the_leader_never_corrects_toward_the_follower() {
        let mut s = follower(90_000, 60_000, measured(100));
        s.we_lead = true;
        assert_eq!(sync_verdict(&s, fine()), SyncVerdict::Hold);
    }

    /// The ping-pong guard. A peer still on an older command is describing state
    /// we have already superseded; correcting toward it would undo our own
    /// command and start the two devices arguing.
    #[test]
    fn a_peer_still_on_an_older_command_is_ignored() {
        let mut s = follower(90_000, 60_000, measured(100));
        s.local_seq = 5;
        s.peer_seq = 4;
        assert_eq!(sync_verdict(&s, fine()), SyncVerdict::Hold);
        // …and it holds even for the leader-shaped case, i.e. rule order.
        s.we_lead = true;
        assert_eq!(sync_verdict(&s, fine()), SyncVerdict::Hold);
    }

    /// The repair path: a command DM never reached us, so their heartbeat is the
    /// only way we learn about it.
    #[test]
    fn a_peer_ahead_of_us_is_adopted_wholesale() {
        let mut s = follower(90_000, 60_000, measured(100));
        s.local_seq = 4;
        s.peer_seq = 5;
        s.peer_playing = false;
        assert_eq!(
            sync_verdict(&s, fine()),
            SyncVerdict::Adopt {
                pos_ms: 60_000,
                playing: false,
                seq: 5,
            }
        );
    }

    /// A repair outranks the leader rule — otherwise a leader that missed a
    /// command would sit on stale state forever, and only the follower would
    /// ever heal.
    #[test]
    fn even_the_leader_adopts_a_command_it_missed() {
        let mut s = follower(90_000, 60_000, measured(100));
        s.we_lead = true;
        s.local_seq = 4;
        s.peer_seq = 9;
        assert!(matches!(
            sync_verdict(&s, fine()),
            SyncVerdict::Adopt { .. }
        ));
    }

    #[test]
    fn a_peers_playhead_is_projected_forward_by_how_long_the_message_took() {
        let mut s = follower(0, 60_000, measured(400));
        s.peer_at_ms = 1_000_000;
        s.now_ms = 1_000_500; // half a second later on our clock
        assert_eq!(projected_peer_pos_ms(&s), 60_500);

        // A paused peer has not moved, however long the message took.
        s.peer_playing = false;
        assert_eq!(projected_peer_pos_ms(&s), 60_000);
    }

    #[test]
    fn a_peers_playhead_is_projected_through_the_clock_offset() {
        let mut s = follower(0, 60_000, ClockEstimate::unmeasured());
        // Their clock reads four seconds ahead of ours.
        s.clock = ClockEstimate {
            offset_ms: 4_000,
            rtt_ms: Some(100),
            skew_ppm: 0.0,
            measured_at_ms: 1_000_000,
        };
        s.peer_at_ms = 1_004_000; // = our 1_000_000
        s.now_ms = 1_000_250;
        assert_eq!(projected_peer_pos_ms(&s), 60_250);
    }

    /// The frontends switch on these strings, so the shape is pinned here
    /// rather than trusted to stay put.
    #[test]
    fn the_verdict_crosses_to_a_frontend_as_a_tagged_object() {
        let cases = [
            (SyncVerdict::Hold, r#"{"kind":"hold"}"#),
            (
                SyncVerdict::Nudge { rate: 1.04 },
                r#"{"kind":"nudge","rate":1.04}"#,
            ),
            (
                SyncVerdict::Seek { pos_ms: 60_000 },
                r#"{"kind":"seek","pos_ms":60000}"#,
            ),
            (
                SyncVerdict::Adopt {
                    pos_ms: 1,
                    playing: true,
                    seq: 2,
                },
                r#"{"kind":"adopt","pos_ms":1,"playing":true,"seq":2}"#,
            ),
        ];
        for (verdict, wire) in cases {
            assert_eq!(serde_json::to_string(&verdict).unwrap(), wire);
            assert_eq!(serde_json::from_str::<SyncVerdict>(wire).unwrap(), verdict);
        }
        assert_eq!(
            serde_json::to_string(&StateChange::PausedAndSeeked).unwrap(),
            r#""paused_and_seeked""#
        );
    }

    // ── Narrating a state change ─────────────────────────────────────────────

    #[test]
    fn a_state_change_is_named_from_what_the_playhead_did() {
        assert_eq!(
            describe_state_change(false, true, 1_000, 1_000),
            StateChange::Played
        );
        assert_eq!(
            describe_state_change(true, false, 1_000, 1_000),
            StateChange::Paused
        );
        assert_eq!(
            describe_state_change(true, true, 1_000, 500_000),
            StateChange::Seeked
        );
        assert_eq!(
            describe_state_change(true, false, 1_000, 500_000),
            StateChange::PausedAndSeeked
        );
        assert_eq!(
            describe_state_change(true, true, 1_000, 1_200),
            StateChange::Unchanged
        );
    }

    #[test]
    fn ordinary_drift_is_never_mistaken_for_a_seek() {
        // Anything inside the seek threshold is drift, not a jump.
        assert_eq!(
            describe_state_change(true, true, 60_000, 60_000 + TOGETHER_SEEK_THRESHOLD_MS),
            StateChange::Unchanged
        );
        assert_eq!(
            describe_state_change(true, true, 60_000, 60_001 + TOGETHER_SEEK_THRESHOLD_MS),
            StateChange::Seeked
        );
    }

    // ── Tuning ───────────────────────────────────────────────────────────────

    #[test]
    fn each_source_is_tuned_to_what_its_player_can_actually_do() {
        assert!(fine().can_rate_trim);
        assert!(!coarse().can_rate_trim);
        assert!(coarse().min_deadband_ms > fine().min_deadband_ms);
        assert_eq!(
            TogetherContent::local_file(42, None).duration_ms(),
            Some(42)
        );
        assert_eq!(
            TogetherContent::youtube("dQw4w9WgXcQ")
                .unwrap()
                .duration_ms(),
            None
        );
    }

    #[test]
    fn only_a_command_is_ordered() {
        assert!(TogetherSignal::State {
            pos_ms: 0,
            playing: true,
            effective_at_ms: None,
        }
        .is_command());
        assert!(!TogetherSignal::Heartbeat {
            pos_ms: 0,
            playing: true,
            applied_seq: 1,
            output_latency_ms: 0,
        }
        .is_command());
        assert!(!TogetherSignal::Join.is_command());
        assert_eq!(TogetherSignal::End.kind_str(), "end");
    }

    // ── The shared queue ─────────────────────────────────────────────────────

    fn item(id: &str, who: &str) -> QueueItem {
        QueueItem {
            id: id.into(),
            content: TogetherContent::local_file(180_000, None),
            added_by: who.into(),
        }
    }

    #[test]
    fn an_empty_queue_names_nothing() {
        let q = SharedQueue::empty(0, "alice");
        assert!(q.current().is_none());
        assert!(q.admissible());
    }

    #[test]
    fn add_appends_and_never_moves_the_cursor() {
        let mut q = SharedQueue::empty(0, "alice");
        q.add(item("alice:1", "alice"), 1, "alice");
        q.add(item("alice:2", "alice"), 2, "alice");
        assert_eq!(q.cursor, 0);
        assert_eq!(q.current().unwrap().id, "alice:1");
        assert_eq!(q.items.len(), 2);
    }

    #[test]
    fn play_next_inserts_after_the_playing_item() {
        let mut q = SharedQueue::empty(0, "a");
        q.add(item("a:1", "a"), 1, "a");
        q.add(item("a:2", "a"), 2, "a");
        q.play_next(item("a:3", "a"), 3, "a");
        // a:3 sits at index 1, right after the playing a:1, and a:1 still plays.
        assert_eq!(q.cursor, 0);
        assert_eq!(q.items[1].id, "a:3");
        assert_eq!(q.current().unwrap().id, "a:1");
    }

    #[test]
    fn removing_before_the_cursor_keeps_the_same_track_playing() {
        let mut q = SharedQueue::empty(0, "a");
        for n in 1..=3 {
            q.add(item(&format!("a:{n}"), "a"), n, "a");
        }
        q.advance(4, "a"); // now playing a:2 at index 1
        assert_eq!(q.current().unwrap().id, "a:2");
        q.remove("a:1", 5, "a");
        // a:2 still plays; only its numeric slot shifted down by one.
        assert_eq!(q.current().unwrap().id, "a:2");
    }

    #[test]
    fn removing_the_current_item_lands_on_what_would_have_played_next() {
        let mut q = SharedQueue::empty(0, "a");
        for n in 1..=3 {
            q.add(item(&format!("a:{n}"), "a"), n, "a");
        }
        q.advance(4, "a"); // playing a:2
        q.remove("a:2", 5, "a");
        assert_eq!(q.current().unwrap().id, "a:3");
    }

    #[test]
    fn removing_the_last_current_item_clamps_to_the_new_end() {
        let mut q = SharedQueue::empty(0, "a");
        q.add(item("a:1", "a"), 1, "a");
        q.add(item("a:2", "a"), 2, "a");
        q.advance(3, "a"); // playing a:2, the last row
        q.remove("a:2", 4, "a");
        assert_eq!(q.current().unwrap().id, "a:1");
    }

    #[test]
    fn removing_an_unknown_id_is_a_no_op() {
        let mut q = SharedQueue::empty(0, "a");
        q.add(item("a:1", "a"), 1, "a");
        let before = q.clone();
        assert!(!q.remove("ghost", 9, "a"));
        assert_eq!(q, before); // un-stamped: an unchanged list must not win a later order
    }

    #[test]
    fn advancing_past_the_end_finishes_rather_than_loops() {
        let mut q = SharedQueue::empty(0, "a");
        q.add(item("a:1", "a"), 1, "a");
        assert!(q.advance(2, "a").is_none());
        assert!(q.current().is_none());
    }

    #[test]
    fn the_later_edit_wins_reconciliation_whole() {
        // Two devices edit independently; the higher stamp replaces the other
        // entirely, the same total order a pause-vs-play uses.
        let mut mine = SharedQueue::empty(0, "alice");
        mine.add(item("alice:1", "alice"), 1, "alice");

        let mut theirs = SharedQueue::empty(0, "bob");
        theirs.add(item("bob:1", "bob"), 2, "bob"); // seq 2 > seq 1

        assert!(mine.reconcile(theirs.clone()));
        assert_eq!(mine.items, theirs.items);

        // Reconciling the now-stale lower snapshot back in changes nothing.
        let mut older = SharedQueue::empty(0, "alice");
        older.add(item("alice:1", "alice"), 1, "alice");
        assert!(!mine.reconcile(older));
    }

    #[test]
    fn a_high_seq_cannot_smuggle_an_inadmissible_url_into_the_queue() {
        let mut q = SharedQueue::empty(0, "alice");
        let mut poisoned = SharedQueue::empty(999, "mallory");
        poisoned.items.push(QueueItem {
            id: "m:1".into(),
            content: TogetherContent::Stream {
                url: "http://insecure.example/track.mp3".into(),
                recording: None,
                duration_ms: None,
            },
            added_by: "mallory".into(),
        });
        assert!(!poisoned.admissible());
        assert!(!q.reconcile(poisoned)); // the value is guarded, not just the order
        assert!(q.items.is_empty());
    }

    #[test]
    fn duplicate_or_blank_ids_are_inadmissible() {
        let mut dup = SharedQueue::empty(1, "a");
        dup.items.push(item("same", "a"));
        dup.items.push(item("same", "a"));
        assert!(!dup.admissible());

        let mut blank = SharedQueue::empty(1, "a");
        blank.items.push(item("", "a"));
        assert!(!blank.admissible());
    }

    #[test]
    fn a_cursor_past_the_end_by_more_than_one_is_inadmissible() {
        let mut q = SharedQueue::empty(1, "a");
        q.items.push(item("a:1", "a"));
        q.cursor = 1; // one past the end: a finished queue, allowed
        assert!(q.admissible());
        q.cursor = 2; // two past: names a gap that was never there
        assert!(!q.admissible());
    }

    #[test]
    fn a_shared_queue_round_trips_through_json() {
        let mut q = SharedQueue::empty(3, "alice");
        q.add(item("alice:1", "alice"), 4, "alice");
        q.play_next(
            QueueItem {
                id: "bob:1".into(),
                content: TogetherContent::Youtube {
                    video_id: "dQw4w9WgXcQ".into(),
                },
                added_by: "bob".into(),
            },
            5,
            "bob",
        );
        let json = serde_json::to_string(&q).unwrap();
        let back: SharedQueue = serde_json::from_str(&json).unwrap();
        assert_eq!(q, back);
    }
}
