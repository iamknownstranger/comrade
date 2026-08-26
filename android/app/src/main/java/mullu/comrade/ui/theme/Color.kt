package mullu.comrade.ui.theme

import androidx.compose.material3.ColorScheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.ui.graphics.Color

/*
 * Every M3 [ColorScheme] this app builds, plus the palettes M3 has no slot
 * for. [ColorTokens] carries the numbers (framework-free, so they are
 * JUnit-testable); this file is where they become `Color`.
 */

private fun c(argb: Long) = Color(argb)

val ComradeDarkColorScheme: ColorScheme = darkColorScheme(
    primary = c(ColorTokens.darkPrimary),
    onPrimary = c(ColorTokens.darkOnPrimary),
    primaryContainer = c(ColorTokens.darkPrimaryContainer),
    onPrimaryContainer = c(ColorTokens.darkOnPrimaryContainer),
    secondary = c(ColorTokens.darkSecondary),
    onSecondary = c(ColorTokens.darkOnSecondary),
    secondaryContainer = c(ColorTokens.darkSecondaryContainer),
    onSecondaryContainer = c(ColorTokens.darkOnSecondaryContainer),
    tertiary = c(ColorTokens.darkTertiary),
    onTertiary = c(ColorTokens.darkOnTertiary),
    background = c(ColorTokens.darkBackground),
    onBackground = c(ColorTokens.darkOnBackground),
    surface = c(ColorTokens.darkSurface),
    onSurface = c(ColorTokens.darkOnSurface),
    surfaceContainerLowest = c(ColorTokens.darkSurfaceContainerLowest),
    surfaceContainerLow = c(ColorTokens.darkSurfaceContainerLow),
    surfaceContainer = c(ColorTokens.darkSurfaceContainer),
    surfaceContainerHigh = c(ColorTokens.darkSurfaceContainerHigh),
    surfaceContainerHighest = c(ColorTokens.darkSurfaceContainerHighest),
    onSurfaceVariant = c(ColorTokens.darkOnSurfaceVariant),
    outline = c(ColorTokens.darkOutline),
    outlineVariant = c(ColorTokens.darkOutlineVariant),
    error = c(ColorTokens.darkError),
    onError = c(ColorTokens.darkOnError),
    errorContainer = c(ColorTokens.darkErrorContainer),
    onErrorContainer = c(ColorTokens.darkOnErrorContainer),
)

/**
 * Was a 9-line stub with no surface/background of its own, so light mode fell
 * through to M3's stock defaults and did not match the brand at all. Filled
 * out to the same ramp `app/lib/src/theme/comrade_theme.dart`'s
 * `ColorScheme.light` already carries — same ordering rationale: `success`/
 * `warning`/`secondary` here are the 700/800 steps of the dark ramp's hues,
 * not a lightened version of them, because they are read as *text* (status
 * pills) as often as they are read as a fill.
 */
val ComradeLightColorScheme: ColorScheme = lightColorScheme(
    primary = c(ColorTokens.lightPrimary),
    onPrimary = c(ColorTokens.lightOnPrimary),
    primaryContainer = c(ColorTokens.lightPrimaryContainer),
    onPrimaryContainer = c(ColorTokens.lightOnPrimaryContainer),
    secondary = c(ColorTokens.lightSecondary),
    onSecondary = c(ColorTokens.lightOnSecondary),
    secondaryContainer = c(ColorTokens.lightSecondaryContainer),
    onSecondaryContainer = c(ColorTokens.lightOnSecondaryContainer),
    tertiary = c(ColorTokens.lightTertiary),
    onTertiary = c(ColorTokens.lightOnTertiary),
    background = c(ColorTokens.lightBackground),
    onBackground = c(ColorTokens.lightOnBackground),
    surface = c(ColorTokens.lightSurface),
    onSurface = c(ColorTokens.lightOnSurface),
    surfaceContainerLowest = c(ColorTokens.lightSurfaceContainerLowest),
    surfaceContainerLow = c(ColorTokens.lightSurfaceContainerLow),
    surfaceContainer = c(ColorTokens.lightSurfaceContainer),
    surfaceContainerHigh = c(ColorTokens.lightSurfaceContainerHigh),
    surfaceContainerHighest = c(ColorTokens.lightSurfaceContainerHighest),
    onSurfaceVariant = c(ColorTokens.lightOnSurfaceVariant),
    outline = c(ColorTokens.lightOutline),
    outlineVariant = c(ColorTokens.lightOutlineVariant),
    error = c(ColorTokens.lightError),
    onError = c(ColorTokens.lightOnError),
    errorContainer = c(ColorTokens.lightErrorContainer),
    onErrorContainer = c(ColorTokens.lightOnErrorContainer),
)

/**
 * Colours the call overlay owns outright, full-bleed dark on **every** theme
 * (`docs/DESIGN_SYSTEM.md` §5's twin, `CallScreen.kt`'s call surfaces): a
 * bright call screen at 3am held to a face is a bug, so these are
 * deliberately not part of [ColorScheme]. Mirrors
 * `app/lib/src/theme/comrade_theme.dart`'s `CallPalette` field for field.
 */
object CallPalette {
    val background = c(ColorTokens.callBackground)
    val accept = c(ColorTokens.callAccept)
    val hangup = c(ColorTokens.callHangup)
    val controlIdle = c(ColorTokens.callControlIdle)
    val controlActive = c(ColorTokens.callControlActive)
    /** Half-black scrim behind a pill/button drawn over live video. */
    val scrim = c(ColorTokens.callScrim)
    /** The ⋮ options dock's panel. */
    val dockBackground = c(ColorTokens.callDockBackground)
    /** Self-preview tile / minimised call tile background, before video attaches. */
    val tileBackground = c(ColorTokens.callTileBackground)
    val secondaryText = c(ColorTokens.callSecondaryText)
    /** Bottom-gradient scrim behind the control bar over a video call. */
    val captionOverlay = c(ColorTokens.callCaptionOverlay)
    /** Dimmed white — control labels under the bar. */
    val labelDim = c(ColorTokens.callLabelDim)
    val weakSignal = c(ColorTokens.callWeakSignal)
    val poorSignal = c(ColorTokens.callPoorSignal)
}

/**
 * Identity-stable avatar hues: the same key renders the same colour on every
 * device (Telegram-style), so people become recognisable at a glance. Used
 * with [avatarColorIndex] (`ui/DisplayName.kt`). Mirrors `app/`'s
 * `kAvatarPalette` verbatim, same order.
 */
val AvatarPalette: List<Color> = ColorTokens.avatarPalette.map(::c)

/** The green of a live presence dot; muted grey when the peer isn't around. */
val OnlineGreen: Color = c(ColorTokens.onlineGreen)
