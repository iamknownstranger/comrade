package mullu.comrade.ui.theme

import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Every named foreground/fill pair in [ColorTokens], checked against WCAG AA
 * (4.5:1) for both ramps. `docs/DESIGN_SYSTEM.md` §3.1: "a foreground token is
 * never used as a fill, and a foreground token is never used as text. Every
 * foreground token must clear 4.5:1 against every fill it is paired with."
 *
 * `app/lib/src/theme/comrade_theme.dart` records the regression this guards:
 * a light-mode accent reused as text measured 2.1:1. This is the Android
 * version of that same check (`theme_test.dart` is the Flutter one) — pure
 * Kotlin, runnable with no Android SDK via CLAUDE.md's kotlinc+JUnit recipe.
 */
class ThemeContrastTest {

    private data class Pair(val name: String, val foreground: Long, val fill: Long)

    private val darkPairs = listOf(
        Pair("dark background/onBackground", ColorTokens.darkOnBackground, ColorTokens.darkBackground),
        Pair("dark surface/onSurface", ColorTokens.darkOnSurface, ColorTokens.darkSurface),
        Pair("dark primary/onPrimary", ColorTokens.darkOnPrimary, ColorTokens.darkPrimary),
        Pair(
            "dark primaryContainer/onPrimaryContainer",
            ColorTokens.darkOnPrimaryContainer,
            ColorTokens.darkPrimaryContainer,
        ),
        Pair("dark secondary/onSecondary", ColorTokens.darkOnSecondary, ColorTokens.darkSecondary),
        Pair("dark tertiary/onTertiary", ColorTokens.darkOnTertiary, ColorTokens.darkTertiary),
        Pair("dark error/onError", ColorTokens.darkOnError, ColorTokens.darkError),
        Pair(
            "dark errorContainer/onErrorContainer",
            ColorTokens.darkOnErrorContainer,
            ColorTokens.darkErrorContainer,
        ),
        Pair("dark card/cardForeground", ColorTokens.darkCardForeground, ColorTokens.darkCard),
        Pair("dark popover/popoverForeground", ColorTokens.darkPopoverForeground, ColorTokens.darkElevated),
        Pair("dark muted/mutedForeground", ColorTokens.darkMutedForeground, ColorTokens.darkElevated),
        Pair("dark success/onSuccess", ColorTokens.darkOnSuccess, ColorTokens.darkSuccess),
        Pair("dark warning/onWarning", ColorTokens.darkOnWarning, ColorTokens.darkWarning),
    )

    private val lightPairs = listOf(
        Pair("light background/onBackground", ColorTokens.lightOnBackground, ColorTokens.lightBackground),
        Pair("light surface/onSurface", ColorTokens.lightOnSurface, ColorTokens.lightSurface),
        Pair("light primary/onPrimary", ColorTokens.lightOnPrimary, ColorTokens.lightPrimary),
        Pair(
            "light primaryContainer/onPrimaryContainer",
            ColorTokens.lightOnPrimaryContainer,
            ColorTokens.lightPrimaryContainer,
        ),
        Pair("light secondary/onSecondary", ColorTokens.lightOnSecondary, ColorTokens.lightSecondary),
        Pair("light tertiary/onTertiary", ColorTokens.lightOnTertiary, ColorTokens.lightTertiary),
        Pair("light error/onError", ColorTokens.lightOnError, ColorTokens.lightError),
        Pair(
            "light errorContainer/onErrorContainer",
            ColorTokens.lightOnErrorContainer,
            ColorTokens.lightErrorContainer,
        ),
        Pair("light card/cardForeground", ColorTokens.lightCardForeground, ColorTokens.lightCard),
        Pair("light popover/popoverForeground", ColorTokens.lightPopoverForeground, ColorTokens.lightElevated),
        Pair("light muted/mutedForeground", ColorTokens.lightMutedForeground, ColorTokens.lightElevated),
        Pair("light success/onSuccess", ColorTokens.lightOnSuccess, ColorTokens.lightSuccess),
        Pair("light warning/onWarning", ColorTokens.lightOnWarning, ColorTokens.lightWarning),
        // `success`/`warning` are also rendered as *text* directly on the page
        // background (status pills, `docs/DESIGN_SYSTEM.md`'s "de-emphasised
        // fills and secondary text"), which is exactly the substitution that
        // failed before — check that too.
        Pair("light success as text on background", ColorTokens.lightSuccess, ColorTokens.lightBackground),
        Pair("light warning as text on background", ColorTokens.lightWarning, ColorTokens.lightBackground),
    )

    @Test
    fun everyDarkPairClearsAaNormalText() {
        for (p in darkPairs) {
            assertTrue(
                "${p.name} is ${"%.2f".format(ContrastDecisions.contrastRatio(p.foreground, p.fill))}:1, " +
                    "wants >= 4.5:1",
                ContrastDecisions.clearsNormalTextAa(p.foreground, p.fill),
            )
        }
    }

    @Test
    fun everyLightPairClearsAaNormalText() {
        for (p in lightPairs) {
            assertTrue(
                "${p.name} is ${"%.2f".format(ContrastDecisions.contrastRatio(p.foreground, p.fill))}:1, " +
                    "wants >= 4.5:1",
                ContrastDecisions.clearsNormalTextAa(p.foreground, p.fill),
            )
        }
    }

    // ── Sanity checks on the contrast primitive itself ──────────────────────

    @Test
    fun blackOnWhiteIsTheMaximumRatio() {
        val ratio = ContrastDecisions.contrastRatio(0xFF000000L, 0xFFFFFFFFL)
        assertTrue("expected ~21:1, got $ratio", ratio > 20.9 && ratio < 21.1)
    }

    @Test
    fun sameColourIsRatioOne() {
        val ratio = ContrastDecisions.contrastRatio(0xFF6366F1L, 0xFF6366F1L)
        assertTrue(ratio in 0.999..1.001)
    }

    @Test
    fun ringIsNeverTheSameValueAsBorder() {
        // docs/DESIGN_SYSTEM.md §3.1: "ring — focus indicator — never the same
        // value as border."
        assertTrue(ColorTokens.darkRing != ColorTokens.darkBorder)
        assertTrue(ColorTokens.lightRing != ColorTokens.lightBorder)
    }
}
