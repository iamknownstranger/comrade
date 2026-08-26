package mullu.comrade.call

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
// `by animateFloatAsState(...)` needs the property-delegate operator; without
// it the State<Float> is not delegable and every *use* of the delegated value
// then reports as an unrelated "overload resolution ambiguity" on Float.times.
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Fill
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import mullu.comrade.R
import mullu.comrade.ui.theme.CallPalette

/**
 * The Compose twins of the Flutter `signal_bars.dart` / `slashed_icon.dart`
 * widgets. Same numbers, same rules, so a call looks identical on the frontend
 * that ships today (Compose) and the one it is migrating to (Flutter):
 *
 *  * signal-strength bars replace the old 4-emoji verification row;
 *  * mute/camera slash themselves rather than swapping glyph.
 *
 * Kept in `call/` (not `ui/`) so the import-scan build filter still classifies
 * every call surface together, and so `PipController`/`CallManager`'s package
 * neighbours don't have to reach across into `ui/`.
 */

/** How many of the four bars a quality reading fills. Mirrors `signalBarsFor`. */
fun signalBarsFor(quality: CallQuality): Int = when (quality) {
    CallQuality.GOOD -> 4
    CallQuality.MEDIUM -> 2
    CallQuality.POOR -> 1
    CallQuality.UNKNOWN -> 0
}

/** The colour the filled bars take — white when healthy, warning tones when not. */
fun signalColorFor(quality: CallQuality): Color = when (quality) {
    CallQuality.POOR -> CallPalette.poorSignal
    CallQuality.MEDIUM -> CallPalette.weakSignal
    CallQuality.GOOD, CallQuality.UNKNOWN -> Color.White
}

/**
 * Four ascending bars, [signalBarsFor] of them filled. Empty bars stay drawn at
 * low opacity so "one bar" reads as one-of-four; [CallQuality.UNKNOWN] fills
 * none, because "nothing measured yet" is not "zero signal" but is also not a
 * number to invent.
 */
@Composable
fun SignalStrengthBars(
    quality: CallQuality,
    modifier: Modifier = Modifier,
    height: Dp = 14.dp,
    barWidth: Dp = 3.dp,
    spacing: Dp = 2.dp,
) {
    val filled = signalBarsFor(quality)
    val color = signalColorFor(quality)
    val description = if (quality == CallQuality.UNKNOWN) {
        stringResource(R.string.call_signal_measuring)
    } else {
        stringResource(R.string.call_signal_strength, filled)
    }
    Row(
        modifier = modifier.semantics { contentDescription = description },
        verticalAlignment = Alignment.Bottom,
    ) {
        for (i in 0 until BAR_COUNT) {
            if (i > 0) Spacer(Modifier.width(spacing))
            val fraction = 0.45f + 0.55f * (i / (BAR_COUNT - 1f))
            // Animate the fill/colour: quality is sampled every 2 s and an
            // un-animated flick between 2 and 4 bars reads as a glitch.
            val alpha by animateFloatAsState(
                targetValue = if (i < filled) 1f else 0.25f,
                animationSpec = tween(220),
                label = "signal-bar-$i",
            )
            Canvas(Modifier.size(barWidth, height * fraction)) {
                drawRoundRect(
                    color = if (alpha > 0.5f) color else Color.White,
                    alpha = alpha,
                    cornerRadius = androidx.compose.ui.geometry.CornerRadius(size.width / 2),
                    style = Fill,
                )
            }
        }
    }
}

private const val BAR_COUNT = 4

/**
 * An [icon] with a slash *drawn over* it (and animated on/off) rather than a
 * swap to a different glyph — the explicit "this is off" every video app uses.
 * The slash is painted twice: a wider stroke in [cutoutColor] under a thinner
 * one in [color], so the line reads as cut out of the glyph.
 */
@Composable
fun SlashedIcon(
    icon: ImageVector,
    slashed: Boolean,
    color: Color,
    cutoutColor: Color,
    contentDescription: String?,
    size: Dp,
) {
    val progress by animateFloatAsState(
        targetValue = if (slashed) 1f else 0f,
        animationSpec = tween(220),
        label = "slash",
    )
    Box(contentAlignment = Alignment.Center) {
        Icon(icon, contentDescription = contentDescription, tint = color, modifier = Modifier.size(size))
        if (progress > 0f) {
            Canvas(Modifier.size(size)) {
                val start = Offset(this.size.width * 0.16f, this.size.height * 0.14f)
                val end = Offset(this.size.width * 0.84f, this.size.height * 0.86f)
                val tip = Offset(
                    start.x + (end.x - start.x) * progress,
                    start.y + (end.y - start.y) * progress,
                )
                val stroke = this.size.width * 0.09f
                drawLine(cutoutColor, start, tip, strokeWidth = stroke * 2.1f, cap = StrokeCap.Round)
                drawLine(color, start, tip, strokeWidth = stroke, cap = StrokeCap.Round)
            }
        }
    }
}

/** Bars plus, only when degraded, the word for what they mean. */
@Composable
fun ConnectionStrengthIndicator(quality: CallQuality, modifier: Modifier = Modifier) {
    val label = when (quality) {
        CallQuality.POOR -> stringResource(R.string.call_poor_connection)
        CallQuality.MEDIUM -> stringResource(R.string.call_weak_signal)
        CallQuality.GOOD, CallQuality.UNKNOWN -> null
    }
    Row(
        modifier = modifier,
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        SignalStrengthBars(quality)
        if (label != null) {
            Text(label, color = signalColorFor(quality), fontSize = 13.sp)
        }
    }
}

/**
 * Above this much of the picture lost to cropping, a renderer letterboxes
 * (`SCALE_ASPECT_FIT`) instead of filling (`SCALE_ASPECT_FILL`).
 *
 * A third is the useful line, and the cases either side of it are why: a 4:3
 * camera frame on a tall phone loses 25% to the crop, and every call app fills
 * there — bars around a face look broken; a 16:9 *screen* on a portrait phone
 * loses 68%, which is not a crop so much as a different picture.
 *
 * Kept in step with `kMaxCroppedFraction` (Dart) and `MAX_CROPPED_FRACTION`
 * (desktop).
 */
const val MAX_CROPPED_FRACTION = 1.0 / 3.0

/**
 * Whether a frame should be letterboxed rather than cropped to fill its box.
 *
 * Deliberately a question about geometry, not about what the far end is doing:
 * "is the peer sharing a screen" would need a wire-format field the protocol
 * does not have, and guessing it from orientation gets a landscape tablet
 * wrong. "Would filling this box destroy the picture" is answerable from the
 * frame that already arrived.
 *
 * Answers false for anything it cannot judge — no frame yet, a box not laid out
 * — because filling is the safe default and what every other surface does.
 */
fun shouldLetterbox(
    frameWidth: Int,
    frameHeight: Int,
    boxWidth: Int,
    boxHeight: Int,
    maxCropped: Double = MAX_CROPPED_FRACTION,
): Boolean {
    if (frameWidth <= 0 || frameHeight <= 0 || boxWidth <= 0 || boxHeight <= 0) return false
    val frameAspect = frameWidth.toDouble() / frameHeight.toDouble()
    val boxAspect = boxWidth.toDouble() / boxHeight.toDouble()
    // Cover scales until the box is covered, so the fraction of the source
    // still visible is the ratio of the smaller aspect to the larger.
    val visible = if (frameAspect < boxAspect) frameAspect / boxAspect else boxAspect / frameAspect
    return (1.0 - visible) > maxCropped
}
