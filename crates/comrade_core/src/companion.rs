/*!
 * companion — a private, on-device loneliness companion.
 *
 * Comrade's companion turns the app into a safe place to *write down anything*
 * — journal, vent, brainstorm, or reflect — as an anonymous **Chitthi** that
 * never leaves the device and is only ever stored inside the encrypted store
 * ([`comrade_storage`](../../comrade_storage)). Nothing here talks to a relay,
 * a network, or a cloud model: prompts, safety checks, and insights are all
 * computed locally so the most vulnerable words a person writes stay private.
 *
 * This module is intentionally I/O-free and dependency-light (serde + a regex
 * or two): the *domain* lives here and is unit-tested in isolation, while the
 * persistence wiring lives in `comrade_ui`/the CLI (which own the store).
 *
 * ## Safety, honestly
 *
 * A companion that invites people to write "any shit" will sometimes receive
 * words about self-harm or crisis. [`scan_safety`] does a *best-effort, offline*
 * keyword scan and, when it matches, surfaces real helpline resources
 * ([`crisis_resources`]). It is **not** a diagnostic tool, it will miss things,
 * and the companion is **not** a substitute for a human or a professional. That
 * limitation is stated to the user, not hidden.
 */

use serde::{Deserialize, Serialize};

// ── Modes ─────────────────────────────────────────────────────────────────────

/// What kind of companion session an entry belongs to. The mode shapes which
/// supportive prompt the companion offers back (see [`prompt_for`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompanionMode {
    /// Free-form journaling — write down anything, no structure required.
    Journal,
    /// Unload feelings. The companion listens and validates; it does not "fix".
    Vent,
    /// Idea generation — the companion nudges with divergent questions.
    Brainstorm,
    /// Gentle, CBT-flavoured reflection prompts. Reflection, not therapy.
    Reflect,
}

impl CompanionMode {
    /// Stable machine key (used in storage, JNI, and voice routing).
    pub fn key(self) -> &'static str {
        match self {
            CompanionMode::Journal => "journal",
            CompanionMode::Vent => "vent",
            CompanionMode::Brainstorm => "brainstorm",
            CompanionMode::Reflect => "reflect",
        }
    }

    /// Parse a mode from its [`key`](Self::key) (case-insensitive).
    pub fn from_key(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "journal" | "write" | "diary" => Some(CompanionMode::Journal),
            "vent" | "unload" => Some(CompanionMode::Vent),
            "brainstorm" | "ideas" | "idea" => Some(CompanionMode::Brainstorm),
            "reflect" | "reflection" | "therapy" => Some(CompanionMode::Reflect),
            _ => None,
        }
    }

    /// Human-readable label for UI headers.
    pub fn label(self) -> &'static str {
        match self {
            CompanionMode::Journal => "Journal",
            CompanionMode::Vent => "Vent",
            CompanionMode::Brainstorm => "Brainstorm",
            CompanionMode::Reflect => "Reflect",
        }
    }

    /// Every mode, for enumeration in a UI.
    pub fn all() -> [CompanionMode; 4] {
        [
            CompanionMode::Journal,
            CompanionMode::Vent,
            CompanionMode::Brainstorm,
            CompanionMode::Reflect,
        ]
    }
}

/// How an entry reached the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntrySource {
    /// Typed into the app.
    Typed,
    /// Dictated and transcribed on-device (voice recording → Vosk transcript).
    Voice,
}

/// A coarse mood rating on a −2..=2 scale (−2 very low … +2 very good).
pub type Mood = i8;

/// The lowest / highest valid [`Mood`] values.
pub const MOOD_MIN: Mood = -2;
pub const MOOD_MAX: Mood = 2;

// ── Journal entry ───────────────────────────────────────────────────────────

/// A single anonymous companion entry.
///
/// There is **no author field**: entries are deliberately identity-free so the
/// journal cannot be tied back to the user's Nostr key. It is persisted only
/// inside the encrypted-at-rest store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Opaque local id (the caller supplies it — e.g. a ULID or timestamp+rand).
    pub id: String,
    /// Unix seconds when the entry was written.
    pub created_at: u64,
    pub mode: CompanionMode,
    pub source: EntrySource,
    /// The raw words. May be empty for a mood-only check-in.
    pub body: String,
    /// Optional mood rating captured with the entry.
    #[serde(default)]
    pub mood: Option<Mood>,
    /// Free-form tags — auto-extracted `#hashtags` plus any the caller adds.
    #[serde(default)]
    pub tags: Vec<String>,
}

impl JournalEntry {
    /// Create an entry, auto-extracting `#hashtags` from the body and clamping
    /// any mood into the valid range.
    pub fn new(
        id: impl Into<String>,
        created_at: u64,
        mode: CompanionMode,
        source: EntrySource,
        body: impl Into<String>,
    ) -> Self {
        let body = body.into();
        let tags = extract_hashtags(&body);
        Self {
            id: id.into(),
            created_at,
            mode,
            source,
            body,
            mood: None,
            tags,
        }
    }

    /// Attach a mood, clamped to [`MOOD_MIN`]..=[`MOOD_MAX`].
    pub fn with_mood(mut self, mood: Mood) -> Self {
        self.mood = Some(mood.clamp(MOOD_MIN, MOOD_MAX));
        self
    }

    /// Add extra tags (deduplicated, lowercased) on top of the auto-extracted.
    pub fn with_tags<I, S>(mut self, tags: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for t in tags {
            let t = t.into().trim_start_matches('#').trim().to_lowercase();
            if !t.is_empty() && !self.tags.contains(&t) {
                self.tags.push(t);
            }
        }
        self
    }

    /// The Unix day-number this entry falls on (used for streaks), UTC.
    pub fn day(&self) -> u64 {
        self.day_at(0)
    }

    /// The local day-number for a device at `tz_offset_secs` from UTC —
    /// streaks must roll over at the user's midnight, not UTC midnight
    /// (which lands mid-morning for this app's India-first audience).
    pub fn day_at(&self, tz_offset_secs: i32) -> u64 {
        local_day(self.created_at, tz_offset_secs)
    }
}

/// Unix-seconds timestamp → day number in a timezone `tz_offset_secs` from UTC.
fn local_day(unix_secs: u64, tz_offset_secs: i32) -> u64 {
    let shifted = (unix_secs as i64) + i64::from(tz_offset_secs);
    (shifted.max(0) as u64) / SECS_PER_DAY
}

/// Extract `#hashtags` from free text, lowercased and de-duplicated in order.
/// Tags are recognised anywhere in a token, so "(#work)" and "so:#tired" count.
pub fn extract_hashtags(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in text.split(|c: char| c.is_whitespace()) {
        for piece in token.split('#').skip(1) {
            let tag: String = piece
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect::<String>()
                .to_lowercase();
            if !tag.is_empty() && !out.contains(&tag) {
                out.push(tag);
            }
        }
    }
    out
}

const SECS_PER_DAY: u64 = 86_400;

// ── Prompts ───────────────────────────────────────────────────────────────────

/// The prompt bank for a mode. Curated, supportive, non-clinical.
pub fn prompts(mode: CompanionMode) -> &'static [&'static str] {
    match mode {
        CompanionMode::Journal => &[
            "What happened today that you want to remember?",
            "Write down anything on your mind — no need to make it tidy.",
            "What is taking up the most space in your head right now?",
            "If today had a title, what would it be?",
            "What is one small thing that went okay today?",
        ],
        CompanionMode::Vent => &[
            "Let it out — what is frustrating you right now?",
            "You don't have to fix it. What does it feel like in your body?",
            "Who or what is this really about?",
            "Say the thing you can't say out loud anywhere else.",
            "What would you want someone to understand about this?",
        ],
        CompanionMode::Brainstorm => &[
            "What are three completely different ways this could go?",
            "If there were no constraints, what would you try first?",
            "What would you do if you knew it couldn't fail?",
            "Who has solved something like this, and how?",
            "What is the smallest experiment you could run tomorrow?",
        ],
        CompanionMode::Reflect => &[
            "What thought is heaviest right now — and is it a fact or a fear?",
            "What would you say to a friend who felt this way?",
            "What is one thing within your control here?",
            "When did you last feel a little lighter? What was different?",
            "What do you need right now that you could actually ask for?",
        ],
    }
}

/// Pick a supportive prompt for `mode`, deterministically indexed by `seed`
/// (e.g. the entry count, or a hash of the last entry). Deterministic so the
/// behaviour is testable and reproducible.
pub fn prompt_for(mode: CompanionMode, seed: u64) -> &'static str {
    let bank = prompts(mode);
    bank[(seed % bank.len() as u64) as usize]
}

// ── Safety ──────────────────────────────────────────────────────────────────

/// A crisis helpline resource surfaced when [`scan_safety`] flags an entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrisisResource {
    pub region: String,
    pub name: String,
    pub contact: String,
}

impl CrisisResource {
    fn new(region: &str, name: &str, contact: &str) -> Self {
        Self {
            region: region.to_string(),
            name: name.to_string(),
            contact: contact.to_string(),
        }
    }
}

/// A best-effort, offline assessment of whether an entry contains language that
/// suggests the writer may be in crisis. Never a diagnosis — see module docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafetyAssessment {
    /// Whether any crisis signal was matched.
    pub concerning: bool,
    /// The phrases that matched (for transparency / testing, not for display).
    pub matched: Vec<String>,
    /// A short, warm message to show alongside resources when `concerning`.
    pub message: Option<String>,
    /// Helpline resources to surface when `concerning`.
    pub resources: Vec<CrisisResource>,
}

impl SafetyAssessment {
    fn clear() -> Self {
        Self {
            concerning: false,
            matched: Vec::new(),
            message: None,
            resources: Vec::new(),
        }
    }
}

/// Crisis phrases scanned for, in lowercase. Deliberately high-recall (it is
/// better to over-offer help than to miss someone); the UI shows resources
/// gently rather than blocking anything.
const CRISIS_PHRASES: &[&str] = &[
    "kill myself",
    "killing myself",
    "want to die",
    "wanna die",
    "end my life",
    "ending my life",
    "end it all",
    "take my own life",
    "suicidal",
    "suicide",
    "self harm",
    "self-harm",
    "hurt myself",
    "harm myself",
    "cut myself",
    "no reason to live",
    "nothing to live for",
    "better off dead",
    "can't go on",
    "cant go on",
    "don't want to be here",
    "dont want to be here",
    "give up on life",
];

/// Return the standard set of crisis resources. International + India-first,
/// matching the app's audience; extend per locale as needed.
pub fn crisis_resources() -> Vec<CrisisResource> {
    vec![
        CrisisResource::new(
            "India",
            "KIRAN Mental Health Helpline",
            "1800-599-0019 (24/7, toll-free)",
        ),
        CrisisResource::new(
            "India",
            "iCall Psychosocial Helpline",
            "9152987821 (Mon–Sat, 8am–10pm)",
        ),
        CrisisResource::new("US", "988 Suicide & Crisis Lifeline", "Call or text 988"),
        CrisisResource::new("UK & ROI", "Samaritans", "116 123 (free, 24/7)"),
        CrisisResource::new(
            "International",
            "Befrienders Worldwide",
            "https://befrienders.org (find a local helpline)",
        ),
    ]
}

/// The gentle message shown above resources. Kept in code so wording is
/// reviewed, not improvised.
const CRISIS_MESSAGE: &str =
    "It sounds like you're carrying something really heavy right now, and \
I'm glad you wrote it down. I'm just an app on your phone — but you deserve to talk to someone who \
can truly be there. If you're thinking about harming yourself, please reach out to one of these, \
any time:";

/// Scan `text` for crisis signals. Case-insensitive substring match over
/// [`CRISIS_PHRASES`]; when anything matches, the assessment carries a warm
/// message and [`crisis_resources`].
pub fn scan_safety(text: &str) -> SafetyAssessment {
    // Speech keyboards and voice transcripts emit typographic apostrophes
    // (\u{2019}) and quotes; the phrase list uses ASCII. Normalise first —
    // a missed match here silently withholds helpline resources.
    let haystack: String = text
        .to_lowercase()
        .chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '`' | '\u{02BC}' => '\'',
            '\u{2013}' | '\u{2014}' => '-',
            c => c,
        })
        .collect();
    let matched: Vec<String> = CRISIS_PHRASES
        .iter()
        .filter(|p| haystack.contains(*p))
        .map(|p| (*p).to_string())
        .collect();

    if matched.is_empty() {
        SafetyAssessment::clear()
    } else {
        SafetyAssessment {
            concerning: true,
            matched,
            message: Some(CRISIS_MESSAGE.to_string()),
            resources: crisis_resources(),
        }
    }
}

// ── Insights ──────────────────────────────────────────────────────────────────

/// Lightweight, on-device insights over a set of entries — enough to make the
/// journal feel alive (streaks, momentum, mood trend) without profiling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Insights {
    pub total: usize,
    /// Consecutive days (ending today or yesterday) with at least one entry.
    pub current_streak_days: u64,
    pub entries_this_week: usize,
    /// Mean mood over the last 7 days, if any moods were recorded.
    pub avg_mood_recent: Option<f32>,
    /// Most frequent tags, most-common first (ties broken alphabetically).
    pub top_tags: Vec<(String, usize)>,
}

impl Insights {
    /// Compute insights as of `now` (Unix seconds), with day boundaries at
    /// UTC midnight. Prefer [`from_entries_at`](Self::from_entries_at) with
    /// the device's timezone offset so streaks roll at local midnight.
    pub fn from_entries(entries: &[JournalEntry], now: u64) -> Self {
        Self::from_entries_at(entries, now, 0)
    }

    /// Compute insights as of `now` (Unix seconds) for a device at
    /// `tz_offset_secs` from UTC (e.g. +19800 for IST). Pure — clock and
    /// timezone are injected so the result is deterministic and testable.
    pub fn from_entries_at(entries: &[JournalEntry], now: u64, tz_offset_secs: i32) -> Self {
        let total = entries.len();
        let today = local_day(now, tz_offset_secs);
        let week_ago = now.saturating_sub(7 * SECS_PER_DAY);

        // Distinct (local) days with an entry.
        let mut days: Vec<u64> = entries.iter().map(|e| e.day_at(tz_offset_secs)).collect();
        days.sort_unstable();
        days.dedup();

        let current_streak_days = streak_ending_now(&days, today);

        let entries_this_week = entries.iter().filter(|e| e.created_at >= week_ago).count();

        let recent_moods: Vec<i16> = entries
            .iter()
            .filter(|e| e.created_at >= week_ago)
            .filter_map(|e| e.mood.map(i16::from))
            .collect();
        let avg_mood_recent = if recent_moods.is_empty() {
            None
        } else {
            Some(recent_moods.iter().sum::<i16>() as f32 / recent_moods.len() as f32)
        };

        let top_tags = rank_tags(entries);

        Self {
            total,
            current_streak_days,
            entries_this_week,
            avg_mood_recent,
            top_tags,
        }
    }
}

/// Count consecutive entry-days ending at `today` or `today-1`. If the newest
/// entry is older than yesterday the streak is 0 (it has lapsed).
fn streak_ending_now(sorted_unique_days: &[u64], today: u64) -> u64 {
    let Some(&newest) = sorted_unique_days.last() else {
        return 0;
    };
    if newest < today.saturating_sub(1) {
        return 0;
    }
    // Walk backwards from `newest` counting contiguous days.
    let mut streak = 0u64;
    let mut expected = newest;
    for &day in sorted_unique_days.iter().rev() {
        if day == expected {
            streak += 1;
            expected = expected.saturating_sub(1);
        } else if day < expected {
            break;
        }
    }
    streak
}

/// Rank tags by frequency (desc), breaking ties alphabetically for stability.
fn rank_tags(entries: &[JournalEntry]) -> Vec<(String, usize)> {
    use std::collections::HashMap;
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for e in entries {
        for tag in &e.tags {
            *counts.entry(tag.as_str()).or_insert(0) += 1;
        }
    }
    let mut ranked: Vec<(String, usize)> = counts
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, at: u64, body: &str) -> JournalEntry {
        JournalEntry::new(id, at, CompanionMode::Journal, EntrySource::Typed, body)
    }

    #[test]
    fn mode_key_roundtrip_and_aliases() {
        for m in CompanionMode::all() {
            assert_eq!(CompanionMode::from_key(m.key()), Some(m));
        }
        assert_eq!(
            CompanionMode::from_key("THERAPY"),
            Some(CompanionMode::Reflect)
        );
        assert_eq!(
            CompanionMode::from_key("diary"),
            Some(CompanionMode::Journal)
        );
        assert_eq!(CompanionMode::from_key("nope"), None);
    }

    #[test]
    fn new_entry_extracts_hashtags_lowercased_and_deduped() {
        let e = entry(
            "1",
            100,
            "feeling #Lonely but also #lonely and #Grateful today",
        );
        assert_eq!(e.tags, vec!["lonely".to_string(), "grateful".to_string()]);
    }

    #[test]
    fn with_mood_clamps_range() {
        let e = entry("1", 0, "x").with_mood(9);
        assert_eq!(e.mood, Some(MOOD_MAX));
        let e = entry("1", 0, "x").with_mood(-9);
        assert_eq!(e.mood, Some(MOOD_MIN));
    }

    #[test]
    fn with_tags_merges_and_dedupes_against_auto() {
        let e = entry("1", 0, "note #work").with_tags(["#Work", "family", "family"]);
        assert_eq!(e.tags, vec!["work".to_string(), "family".to_string()]);
    }

    #[test]
    fn prompt_for_is_deterministic_and_in_bank() {
        let p = prompt_for(CompanionMode::Reflect, 3);
        assert_eq!(p, prompt_for(CompanionMode::Reflect, 3));
        assert!(prompts(CompanionMode::Reflect).contains(&p));
        // Seed wraps around the bank.
        let n = prompts(CompanionMode::Vent).len() as u64;
        assert_eq!(
            prompt_for(CompanionMode::Vent, 0),
            prompt_for(CompanionMode::Vent, n)
        );
    }

    #[test]
    fn safety_scan_is_clear_for_ordinary_text() {
        let a = scan_safety("Had a rough day at work but dinner with a friend helped.");
        assert!(!a.concerning);
        assert!(a.matched.is_empty());
        assert!(a.resources.is_empty());
        assert!(a.message.is_none());
    }

    #[test]
    fn safety_scan_flags_crisis_language_and_offers_resources() {
        let a = scan_safety("some days I honestly want to die and feel better off dead");
        assert!(a.concerning);
        assert!(a.matched.contains(&"want to die".to_string()));
        assert!(a.matched.contains(&"better off dead".to_string()));
        assert!(!a.resources.is_empty());
        assert!(a.message.is_some());
        // India-first resources are present for the app's primary audience.
        assert!(a.resources.iter().any(|r| r.region == "India"));
    }

    #[test]
    fn safety_scan_is_case_insensitive() {
        assert!(scan_safety("I am SUICIDAL").concerning);
    }

    #[test]
    fn safety_scan_survives_typographic_apostrophes() {
        // Android keyboards emit U+2019, not ASCII apostrophes.
        assert!(scan_safety("some days I can\u{2019}t go on").concerning);
        assert!(scan_safety("I don\u{2019}t want to be here").concerning);
        assert!(scan_safety("i can`t go on").concerning);
    }

    #[test]
    fn hashtags_survive_surrounding_punctuation() {
        assert_eq!(
            extract_hashtags("felt low (#lonely), then better: #Grateful!"),
            vec!["lonely".to_string(), "grateful".to_string()]
        );
        assert!(extract_hashtags("## #").is_empty());
    }

    #[test]
    fn streaks_roll_at_local_midnight_not_utc() {
        const IST: i32 = 19_800; // UTC+5:30
        let day = SECS_PER_DAY;
        // 23:00 IST on local day D falls on UTC day D-…: two entries either
        // side of *local* midnight collapse to one UTC day but form a genuine
        // 2-day streak in IST.
        let e1 = entry("a", 10 * day + 63_000, "evening"); // 17:30 UTC
        let e2 = entry("b", 10 * day + 70_200, "past local midnight"); // 19:30 UTC
        assert_eq!(e1.day_at(IST) + 1, e2.day_at(IST));
        assert_eq!(e1.day(), e2.day());

        let now = 10 * day + 70_500;
        let utc = Insights::from_entries(&[e1.clone(), e2.clone()], now);
        let ist = Insights::from_entries_at(&[e1, e2], now, IST);
        assert_eq!(utc.current_streak_days, 1);
        assert_eq!(ist.current_streak_days, 2);
    }

    #[test]
    fn insights_empty() {
        let i = Insights::from_entries(&[], 1_000_000);
        assert_eq!(i.total, 0);
        assert_eq!(i.current_streak_days, 0);
        assert_eq!(i.entries_this_week, 0);
        assert!(i.avg_mood_recent.is_none());
        assert!(i.top_tags.is_empty());
    }

    #[test]
    fn insights_streak_counts_consecutive_days_ending_today() {
        let day = SECS_PER_DAY;
        let now = 10 * day + 100; // "today" = day 10
        let entries = vec![
            entry("a", 10 * day + 5, "today"),
            entry("b", 9 * day + 5, "yesterday"),
            entry("c", 8 * day + 5, "day before"),
            entry("d", 5 * day + 5, "gap"),
        ];
        let i = Insights::from_entries(&entries, now);
        assert_eq!(i.current_streak_days, 3);
        assert_eq!(i.total, 4);
    }

    #[test]
    fn insights_streak_lapses_if_newest_older_than_yesterday() {
        let day = SECS_PER_DAY;
        let now = 10 * day;
        let entries = vec![entry("a", 7 * day, "stale")];
        assert_eq!(Insights::from_entries(&entries, now).current_streak_days, 0);
    }

    #[test]
    fn insights_streak_holds_when_newest_is_yesterday() {
        let day = SECS_PER_DAY;
        let now = 10 * day + 50;
        let entries = vec![
            entry("a", 9 * day, "yesterday"),
            entry("b", 8 * day, "day before"),
        ];
        assert_eq!(Insights::from_entries(&entries, now).current_streak_days, 2);
    }

    #[test]
    fn insights_week_window_and_mood_average() {
        let day = SECS_PER_DAY;
        let now = 30 * day;
        let entries = vec![
            entry("a", 30 * day, "in").with_mood(2),
            entry("b", 28 * day, "in").with_mood(-1),
            entry("c", 20 * day, "out").with_mood(2), // older than a week
        ];
        let i = Insights::from_entries(&entries, now);
        assert_eq!(i.entries_this_week, 2);
        assert_eq!(i.avg_mood_recent, Some(0.5));
    }

    #[test]
    fn insights_top_tags_ranked_by_frequency_then_alpha() {
        let entries = vec![
            entry("a", 1, "#work #stress"),
            entry("b", 2, "#work #family"),
            entry("c", 3, "#work #family"),
        ];
        let i = Insights::from_entries(&entries, 1_000_000);
        assert_eq!(i.top_tags[0], ("work".to_string(), 3));
        assert_eq!(i.top_tags[1], ("family".to_string(), 2));
        assert_eq!(i.top_tags[2], ("stress".to_string(), 1));
    }

    #[test]
    fn entry_serde_roundtrip() {
        let e = entry("id1", 42, "hello #calm").with_mood(1);
        let json = serde_json::to_string(&e).unwrap();
        let back: JournalEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
        // mode/source serialise as snake_case strings.
        assert!(json.contains("\"mode\":\"journal\""));
        assert!(json.contains("\"source\":\"typed\""));
    }
}
