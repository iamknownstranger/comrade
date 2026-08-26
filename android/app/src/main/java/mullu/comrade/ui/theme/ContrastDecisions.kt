package mullu.comrade.ui.theme

/**
 * WCAG 2.x contrast math, kept framework-free (Kotlin stdlib only) so it runs
 * under CLAUDE.md's kotlinc+JUnit recipe with no Android classpath.
 *
 * `docs/DESIGN_SYSTEM.md` §3.1's pairing rule is load-bearing — "a foreground
 * token is never used as a fill, and a foreground token is never used as
 * text" — and the rule that actually enforces it is a number: every
 * foreground token must clear 4.5:1 against every fill it is paired with.
 * `app/lib/src/theme/comrade_theme.dart` already recorded what happens when
 * that number is skipped (a Travel accent reused as light-mode text measured
 * 2.1:1). [ThemeContrastTest] checks every [ColorTokens] pair against this
 * object so a future hex edit can't quietly reintroduce it.
 */
object ContrastDecisions {
    private const val AA_NORMAL_TEXT = 4.5

    /** WCAG relative luminance of a packed `0xAARRGGBB` value. */
    fun relativeLuminance(argb: Long): Double {
        val r = channel(argb, 16)
        val g = channel(argb, 8)
        val b = channel(argb, 0)
        return 0.2126 * linearize(r) + 0.7152 * linearize(g) + 0.0722 * linearize(b)
    }

    /** WCAG contrast ratio between two packed `0xAARRGGBB` colours; always >= 1.0. */
    fun contrastRatio(a: Long, b: Long): Double {
        val la = relativeLuminance(a)
        val lb = relativeLuminance(b)
        val lighter = maxOf(la, lb)
        val darker = minOf(la, lb)
        return (lighter + 0.05) / (darker + 0.05)
    }

    /** True when [foreground] clears WCAG AA (4.5:1) as normal-size text over [fill]. */
    fun clearsNormalTextAa(foreground: Long, fill: Long): Boolean =
        contrastRatio(foreground, fill) >= AA_NORMAL_TEXT

    private fun channel(argb: Long, shift: Int): Double =
        ((argb shr shift) and 0xFF).toDouble() / 255.0

    private fun linearize(c: Double): Double =
        if (c <= 0.03928) c / 12.92 else Math.pow((c + 0.055) / 1.055, 2.4)
}
