package mullu.comrade.ui.theme

import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.Easing
import androidx.compose.runtime.Composable
import androidx.compose.runtime.compositionLocalOf

/**
 * `docs/DESIGN_SYSTEM.md` §3.5. Named durations instead of the per-component
 * `tween(180)` / `tween(220)` this file's siblings used before this pass —
 * new call sites should reach for one of these three rather than inventing a
 * fourth.
 *
 * [fast]/[base]/[slow] are the composable accessors: they read
 * [LocalReducedMotion] and, under §4.3, collapse to `0` rather than merely
 * returning the "fast" tier — see [MotionDecisions.durationMs]. The bare
 * `const val`s stay available for call sites that build an `AnimationSpec`
 * outside composition (rare) and must do the reduced-motion check themselves.
 */
object ComradeMotion {
    /** State layers, ticks. */
    const val fastMs = 120
    /** Most transitions. */
    const val baseMs = 200
    /** Sheets, dialogs, tier changes. */
    const val slowMs = 320

    /** M3 emphasised-decelerate. */
    val easing: Easing = CubicBezierEasing(0.2f, 0f, 0f, 1f)
}

/** [ComradeMotion.fastMs], collapsed to 0 under reduced motion. */
@Composable
fun ComradeMotion.fast(): Int = MotionDecisions.durationMs(fastMs, LocalReducedMotion.current)

/** [ComradeMotion.baseMs], collapsed to 0 under reduced motion. */
@Composable
fun ComradeMotion.base(): Int = MotionDecisions.durationMs(baseMs, LocalReducedMotion.current)

/** [ComradeMotion.slowMs], collapsed to 0 under reduced motion. */
@Composable
fun ComradeMotion.slow(): Int = MotionDecisions.durationMs(slowMs, LocalReducedMotion.current)

/**
 * `docs/DESIGN_SYSTEM.md` §4.3: `prefers-reduced-motion: reduce`, read from
 * `Settings.Global.ANIMATOR_DURATION_SCALE` (`Theme.kt`) — the platform
 * signal that already backs `MediaQuery.disableAnimations` in `app/` on this
 * same device. Defaults to `false` (motion on) so anything composed outside
 * [ComradeTheme] — a preview, a test — behaves as it did before this existed.
 */
val LocalReducedMotion = compositionLocalOf { false }
