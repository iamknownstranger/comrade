package mullu.comrade.ui.theme

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.graphics.Shape

/**
 * `docs/DESIGN_SYSTEM.md` §7.3: lists load as skeletons, not spinners. Every
 * loading state under `ui/` used to be a `CircularProgressIndicator` centred
 * over a blank screen — dated, and it also *hides* the layout, so arrival is a
 * jump instead of content filling in where it was always going to be.
 *
 * A call site draws its row's real geometry — the avatar circle, the title
 * line, the trailing timestamp, whatever the loaded row actually has — and
 * paints each piece with this modifier instead of real content. That is
 * deliberately the whole primitive: it has no opinion on row shape, because
 * §7.3 asks for the *real* geometry of each list, not one generic shimmer box
 * reused everywhere.
 *
 * Pulses opacity, never a travelling gradient sweep — a sweep draws the eye to
 * the loader itself rather than the content it is standing in for, which is
 * the thing §7.3 is trying to get away from. [initial]/[target] both collapse
 * to the same resting alpha under [LocalReducedMotion] rather than branching
 * which animation runs: `rememberInfiniteTransition` is called unconditionally
 * either way, so reduced motion turning on or off mid-composition can never
 * change how many composables this call site emits (see `.claude/rules/android.md`
 * on early returns — the same hazard, from the same cause: a composable whose
 * group count depends on a condition).
 */
private const val SkeletonMinAlpha = 0.35f
private const val SkeletonMaxAlpha = 0.75f
private const val SkeletonRestAlpha = (SkeletonMinAlpha + SkeletonMaxAlpha) / 2f
private const val SkeletonPulseMs = 1100

/** §7.3's "3–6 rows" — one number so every list's loading state fills the same amount of screen. */
const val ComradeSkeletonRowCount = 5

/**
 * Paints [shape] filled with [muted][ComradeSurfaces.muted] at a slow opacity
 * pulse — a placeholder for content whose shape is already known. [shape]
 * should match what will actually render there (a circle for an avatar, a
 * rounded rect for a text line) so the loading state previews the real layout
 * rather than a generic block.
 */
@Composable
fun Modifier.comradeSkeleton(shape: Shape = RectangleShape): Modifier {
    val surfaces = LocalComradeSurfaces.current

    // Under reduced motion the animation is not started at all, rather than
    // started with its ends set equal. That distinction is not cosmetic: an
    // infinite transition keeps running whether or not its value changes, and
    // Compose counts a running animation as pending work — so the test clock
    // never reports idle, and every `waitForIdle`/`waitUntil` in a UI test
    // hangs for as long as one skeleton is on screen. `MainActivityUiTest`
    // died exactly that way, 60s into waiting for the shell, because CI runs
    // the emulator with `disable-animations: true` and the chat list shows
    // skeletons while it loads.
    if (LocalReducedMotion.current) {
        return this.background(surfaces.muted.copy(alpha = SkeletonRestAlpha), shape)
    }

    val transition = rememberInfiniteTransition(label = "comradeSkeleton")
    val alpha by transition.animateFloat(
        initialValue = SkeletonMinAlpha,
        targetValue = SkeletonMaxAlpha,
        animationSpec = infiniteRepeatable(
            animation = tween(SkeletonPulseMs, easing = ComradeMotion.easing),
            repeatMode = RepeatMode.Reverse,
        ),
        label = "comradeSkeletonAlpha",
    )
    return this.background(surfaces.muted.copy(alpha = alpha), shape)
}
