package mullu.comrade.ui

/**
 * Peer-chosen text, on its way to a screen.
 *
 * The Kotlin mirror of `desktop/ui/display_text.mjs`; the Dart port is
 * `app/lib/src/util/display_text.dart`. All three are pinned by mirrored tests,
 * so a change here is a change in three places.
 *
 * Why this is separate from `handoff.HandoffDecisions.displayFileName`, which
 * already does something similar: that one strips C0 controls and **not** the
 * bidirectional overrides, so it is strictly weaker than this. It is left alone
 * deliberately — it is on a shipped transfer path that cannot be compiled or
 * tested in the cloud sandbox, and widening it unverified is a worse trade than
 * naming the gap. Recorded rather than quietly diverged.
 */

/**
 * C0/C1 controls, and the bidirectional overrides.
 *
 * The controls because a newline in a "name" turns one row into two lines of
 * forged UI; the overrides because U+202E makes `holiday<RLO>gnp.exe` read as
 * `holiday.png` in every renderer that honours it. Neither is an injection —
 * Compose draws text, it does not interpret it — and both are lies on a screen,
 * which is enough.
 */
private fun isUnsafeDisplayChar(c: Char): Boolean {
    val n = c.code
    return n <= 0x1F ||
        (n in 0x7F..0x9F) ||
        n == 0x200E ||
        n == 0x200F ||
        (n in 0x202A..0x202E) ||
        (n in 0x2066..0x2069)
}

/**
 * A peer's free-text field, made safe to *draw*: controls and bidi overrides
 * removed, runs of whitespace collapsed to single spaces, trimmed, and bounded.
 *
 * Whitespace is collapsed rather than preserved because every caller here draws
 * into a fixed row. A "bio" containing forty newlines is not a bio; it is a way
 * to push the rest of a profile off the screen and put whatever you like where
 * the app's own chrome used to be.
 *
 * An unsafe character becomes a **space**, not nothing. Two reasons, and the
 * first is a correctness one: `\n` is itself a control, so deleting outright
 * turns "Bob\nSmith" into "BobSmith" and silently invents a name nobody has. The
 * second is that a space leaves the tampering *visible* — `holiday<RLO>gnp.exe`
 * reads as `holiday gnp.exe`, which looks wrong, where deleting produces
 * `holidaygnp.exe`, which looks merely unusual.
 *
 * Returns `""` for nothing usable, so callers decide between an empty row and no
 * row — this function never invents placeholder wording.
 *
 * @param maxChars zero or negative means no bound.
 */
fun sanitizeDisplayText(text: String?, maxChars: Int): String {
    val stripped = (text ?: "")
        .map { if (isUnsafeDisplayChar(it)) ' ' else it }
        .joinToString("")
        .replace(WHITESPACE_RUN, " ")
        .trim()
    if (maxChars <= 0 || stripped.length <= maxChars) return stripped
    return stripped.take(maxChars - 1) + "…"
}

/**
 * What counts as whitespace, spelled out rather than left to `\s`.
 *
 * Java's `\s` is **ASCII only** — `[ \t\n\x0B\f\r]` — where the JavaScript and
 * Dart ports' `\s` also covers U+00A0, U+1680, U+2000–200A, U+2028, U+2029,
 * U+202F, U+205F, U+3000 and U+FEFF. Left at `\s`, this function silently
 * disagreed with its own mirrors, and the disagreement had teeth:
 * [extractLinks] splits on the single spaces this leaves behind, so
 * `https://good.example/<U+3000>https://evil.example/` collapsed to **one**
 * link on Android — drawn with `good.example` as its prominent host and the
 * whole two-URL token copied to the clipboard. U+3000 is also the ordinary
 * word space in Japanese and Chinese, so a link sent in either never reached
 * the Links tab at all.
 */
private val WHITESPACE_RUN =
    Regex("[\\s\\u00A0\\u1680\\u2000-\\u200A\\u2028\\u2029\\u202F\\u205F\\u3000\\uFEFF]+")
