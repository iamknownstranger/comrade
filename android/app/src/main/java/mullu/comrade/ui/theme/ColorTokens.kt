package mullu.comrade.ui.theme

/**
 * Raw ARGB hex tokens (`0xAARRGGBB`, packed into [Long]), kept framework-free
 * — Kotlin stdlib only, no `androidx.*` import — so [ContrastDecisions] and
 * its JUnit test can check every foreground/fill pair without an Android
 * classpath (CLAUDE.md's kotlinc+JUnit recipe). [Color.kt] wraps every
 * constant here as an `androidx.compose.ui.graphics.Color`; this file is the
 * one place the numbers are written down.
 *
 * The dark ramp is `desktop/ui/styles.css`'s `:root` / the old `Theme.kt`'s
 * `DarkColorScheme`, unchanged. The light ramp mirrors
 * `app/lib/src/theme/comrade_theme.dart`'s already-vetted light-mode
 * contrast fixes — its header comment records why the naive "same hue,
 * lighter" substitution failed AA (a Travel `SectionCard` title measured
 * 2.1:1; a `FilledButton` measured 2.15:1), and why `success`/`warning` in
 * light mode are the 700/800 steps of the dark ramp's hues rather than a
 * lightened version of them.
 */
object ColorTokens {
    // ── Dark (default posture) ──────────────────────────────────────────────
    const val darkBackground = 0xFF0A0E1AL
    const val darkOnBackground = 0xFFE6EBF5L
    const val darkSurface = 0xFF0F1525L
    const val darkOnSurface = 0xFFE6EBF5L
    const val darkSurfaceContainerLowest = 0xFF0A0E1AL
    const val darkSurfaceContainerLow = 0xFF0F1525L
    const val darkSurfaceContainer = 0xFF131B2EL
    const val darkSurfaceContainerHigh = 0xFF1A2438L
    const val darkSurfaceContainerHighest = 0xFF1A2438L
    const val darkOnSurfaceVariant = 0xFF9AA7C2L
    const val darkOutline = 0xFF8794B0L
    const val darkOutlineVariant = 0xFF34425FL

    const val darkPrimary = 0xFF818CF8L
    const val darkOnPrimary = 0xFF1E1B4BL
    const val darkPrimaryContainer = 0xFF3730A3L
    const val darkOnPrimaryContainer = 0xFFE0E7FFL

    const val darkSecondary = 0xFF34D399L
    const val darkOnSecondary = 0xFF022C22L
    const val darkSecondaryContainer = 0xFF1A2438L
    const val darkOnSecondaryContainer = 0xFFE6EBF5L

    const val darkTertiary = 0xFFFBBF24L
    const val darkOnTertiary = 0xFF2A1B06L

    const val darkError = 0xFFF87171L
    const val darkOnError = 0xFF3B0A0AL
    const val darkErrorContainer = 0xFF5A1A1AL
    const val darkOnErrorContainer = 0xFFFFE0E0L

    // ComradeSurfaces — the shadcn-named ramp M3's ColorScheme has no slot
    // for (docs/DESIGN_SYSTEM.md §3.1).
    const val darkCard = 0xFF131B2EL
    const val darkCardForeground = 0xFFE6EBF5L
    // `popover` and `muted` share this fill deliberately — the ramp has one
    // elevated step, not two (Surfaces.kt derives both from it so the two
    // can't drift independently of each other).
    const val darkElevated = 0xFF1A2438L
    const val darkPopoverForeground = 0xFFE6EBF5L
    const val darkMutedForeground = 0xFF9AA7C2L
    const val darkSuccess = 0xFF34D399L
    const val darkOnSuccess = 0xFF022C22L
    const val darkWarning = 0xFFFBBF24L
    const val darkOnWarning = 0xFF2A1B06L
    const val darkBorder = 0xFF243049L
    const val darkBorderStrong = 0xFF34425FL
    const val darkInput = 0xFF34425FL
    const val darkRing = 0xFF818CF8L

    // ── Light ────────────────────────────────────────────────────────────────
    const val lightBackground = 0xFFFBFCFFL
    const val lightOnBackground = 0xFF141A28L
    const val lightSurface = 0xFFFBFCFFL
    const val lightOnSurface = 0xFF141A28L
    const val lightSurfaceContainerLowest = 0xFFFFFFFFL
    const val lightSurfaceContainerLow = 0xFFF7F9FEL
    const val lightSurfaceContainer = 0xFFF4F6FBL
    const val lightSurfaceContainerHigh = 0xFFE8ECF6L
    const val lightSurfaceContainerHighest = 0xFFE8ECF6L
    const val lightOnSurfaceVariant = 0xFF4A5468L
    const val lightOutline = 0xFF566072L
    const val lightOutlineVariant = 0xFFD5DBE8L

    const val lightPrimary = 0xFF4F46E5L
    const val lightOnPrimary = 0xFFFFFFFFL
    const val lightPrimaryContainer = 0xFFE0E7FFL
    const val lightOnPrimaryContainer = 0xFF1E1B4BL

    const val lightSecondary = 0xFF065F46L
    const val lightOnSecondary = 0xFFFFFFFFL
    const val lightSecondaryContainer = 0xFFE8ECF6L
    const val lightOnSecondaryContainer = 0xFF1E1B4BL

    const val lightTertiary = 0xFF92400EL
    const val lightOnTertiary = 0xFFFFFFFFL

    const val lightError = 0xFFB91C1CL
    const val lightOnError = 0xFFFFFFFFL
    const val lightErrorContainer = 0xFFFEE2E2L
    const val lightOnErrorContainer = 0xFF7F1D1DL

    const val lightCard = 0xFFF4F6FBL
    const val lightCardForeground = 0xFF141A28L
    const val lightElevated = 0xFFE8ECF6L
    const val lightPopoverForeground = 0xFF141A28L
    const val lightMutedForeground = 0xFF4A5468L
    const val lightSuccess = 0xFF065F46L
    const val lightOnSuccess = 0xFFFFFFFFL
    const val lightWarning = 0xFF92400EL
    const val lightOnWarning = 0xFFFFFFFFL
    const val lightBorder = 0xFFD5DBE8L
    const val lightBorderStrong = 0xFFB6BFD2L
    const val lightInput = 0xFFB6BFD2L
    const val lightRing = 0xFF4F46E5L

    // ── Call overlay — full-bleed dark on every theme, deliberately outside
    // the ColorScheme. A bright call screen at 3am held to a face is a bug
    // (see CallPalette in Color.kt, and its Flutter twin `CallPalette` in
    // `app/lib/src/theme/comrade_theme.dart`). ─────────────────────────────
    const val callBackground = 0xFF0E1621L
    const val callAccept = 0xFF2E7D32L
    const val callHangup = 0xFFC62828L
    const val callControlIdle = 0x33FFFFFFL
    const val callControlActive = 0xFFFFFFFFL
    const val callScrim = 0x66000000L
    const val callDockBackground = 0xF217212BL
    const val callTileBackground = 0xFF17212BL
    const val callSecondaryText = 0xFFB0BEC5L
    const val callCaptionOverlay = 0xB3000000L
    const val callLabelDim = 0xB3FFFFFFL
    const val callWeakSignal = 0xFFFFA000L
    const val callPoorSignal = 0xFFC62828L

    /**
     * Identity-stable avatar hues: the same key renders the same colour on
     * every device (Telegram-style). Copied verbatim into `app/`'s
     * `kAvatarPalette` (`app/lib/src/theme/comrade_theme.dart`) — keep both
     * lists in the same order, [avatarColorIndex] depends on the index.
     */
    val avatarPalette: List<Long> = listOf(
        0xFF6366F1L, // indigo
        0xFF0EA5E9L, // sky
        0xFF10B981L, // emerald
        0xFFF59E0BL, // amber
        0xFFEF4444L, // coral
        0xFF8B5CF6L, // violet
        0xFFEC4899L, // rose
        0xFF14B8A6L, // teal
    )

    /** The green of a live presence dot. */
    const val onlineGreen = 0xFF10B981L
}
