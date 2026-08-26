package mullu.comrade.ui.theme

import androidx.compose.runtime.Immutable
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.ui.graphics.Color

/**
 * The shadcn-named ramp M3's [androidx.compose.material3.ColorScheme] has no
 * slot for (`docs/DESIGN_SYSTEM.md` §3.1): `card`/`popover`/`muted` and their
 * foregrounds, `success`/`warning`, `border`/`borderStrong`, `input`, `ring`.
 *
 * Mirrors `app/lib/src/theme/comrade_theme.dart`'s `ComradeSurfaces`
 * `ThemeExtension` — same idea (a second, hand-carried palette next to the
 * platform one) and, independently, the same numbers, ported to Compose's
 * [androidx.compose.runtime.CompositionLocal] because Compose has no
 * `ThemeExtension` equivalent. Read via [LocalComradeSurfaces].
 *
 * [popover], [muted] and [input] are computed rather than stored, for the
 * same reason the Flutter twin derives them from `panel`/`panelAlt`/
 * `borderStrong`: the ramp has one elevated step and one stronger border, not
 * two of either, and a getter can't drift out of sync with the value it
 * mirrors the way two independently-edited constructor arguments could.
 */
@Immutable
data class ComradeSurfaces(
    /** Content surfaces: list rows, message bubbles, section cards (the `card` tier). */
    val card: Color,
    val cardForeground: Color,
    /**
     * The one elevated step above [card] — floating chrome's flat fill
     * before the glass tint/specular/shadow in [Glass.kt] are added, and
     * also a de-emphasised fill (the `raised` tier, selected rows). [popover]
     * and [muted] both read this.
     */
    private val elevated: Color,
    val popoverForeground: Color,
    val mutedForeground: Color,
    val success: Color,
    val onSuccess: Color,
    val warning: Color,
    val onWarning: Color,
    /** Hairlines. */
    val border: Color,
    /** A step stronger than [border] — dividers/emphasis that must read as a line, not a hint. */
    val borderStrong: Color,
    /** Focus indicator. Never the same value as [border] (§3.1) — see [ThemeContrastTest]. */
    val ring: Color,
) {
    /** §3.1 `popover` — the glass tier's base fill. Numerically [elevated]. */
    val popover: Color get() = elevated

    /** §3.1 `muted` — de-emphasised fills, also the `raised` tier's fill. Numerically [elevated]. */
    val muted: Color get() = elevated

    /** §3.1 `input` — field borders, "a step stronger than border". Exactly [borderStrong]. */
    val input: Color get() = borderStrong

    companion object {
        val Dark = ComradeSurfaces(
            card = Color(ColorTokens.darkCard),
            cardForeground = Color(ColorTokens.darkCardForeground),
            elevated = Color(ColorTokens.darkElevated),
            popoverForeground = Color(ColorTokens.darkPopoverForeground),
            mutedForeground = Color(ColorTokens.darkMutedForeground),
            success = Color(ColorTokens.darkSuccess),
            onSuccess = Color(ColorTokens.darkOnSuccess),
            warning = Color(ColorTokens.darkWarning),
            onWarning = Color(ColorTokens.darkOnWarning),
            border = Color(ColorTokens.darkBorder),
            borderStrong = Color(ColorTokens.darkBorderStrong),
            ring = Color(ColorTokens.darkRing),
        )

        /**
         * `good`/`warn`/`bad`'s note in `comrade_theme.dart` applies here too:
         * `success`/`warning` are rendered as *text* (a status pill's label
         * over an 18%-alpha wash of the same colour), so the dark ramp's
         * bright versions are too light to reuse — these are the 700/800
         * steps of the same hues. [ThemeContrastTest] asserts the 4.5:1 bar
         * for both.
         */
        val Light = ComradeSurfaces(
            card = Color(ColorTokens.lightCard),
            cardForeground = Color(ColorTokens.lightCardForeground),
            elevated = Color(ColorTokens.lightElevated),
            popoverForeground = Color(ColorTokens.lightPopoverForeground),
            mutedForeground = Color(ColorTokens.lightMutedForeground),
            success = Color(ColorTokens.lightSuccess),
            onSuccess = Color(ColorTokens.lightOnSuccess),
            warning = Color(ColorTokens.lightWarning),
            onWarning = Color(ColorTokens.lightOnWarning),
            border = Color(ColorTokens.lightBorder),
            borderStrong = Color(ColorTokens.lightBorderStrong),
            ring = Color(ColorTokens.lightRing),
        )
    }
}

/**
 * `context.surfaces` in `comrade_theme.dart`'s words. Falls back to the dark
 * ramp — [ComradeTheme] always provides this, so hitting the fallback would
 * mean a screen was composed outside it, and dark is the ramp every other
 * fallback in this app already assumes (`Theme.kt`'s old defaults were
 * dark-only).
 */
val LocalComradeSurfaces = compositionLocalOf { ComradeSurfaces.Dark }
