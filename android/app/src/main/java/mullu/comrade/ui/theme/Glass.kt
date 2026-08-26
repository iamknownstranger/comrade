package mullu.comrade.ui.theme

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.TopAppBarColors
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.addOutline
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/*
 * `docs/DESIGN_SYSTEM.md` §3.4/§5: the glass tier, for floating chrome only —
 * nav bars, top bars, sheets, dialogs, popovers (§2's list). NOT list rows,
 * bubbles, `FeedScreen` items, the reading column, or the media viewer.
 *
 * §5 rules out true backdrop blur on Android, deliberately and without a new
 * dependency: `Modifier.blur` blurs a composable's own content, not what's
 * behind it, and real backdrop blur needs API 31+ `RenderEffect` plumbing or
 * a haze library — the latter would need adding to two Gradle files
 * (`android/app/build.gradle.kts` *and* `app/android/app/build.gradle.kts`,
 * because `stagePreservedServices` recompiles this package into the Flutter
 * app) for a purely visual effect. So this is the same tokens with one
 * ingredient missing: translucent tint + specular top edge + depth shadow,
 * no blur.
 */

/** Which floating-chrome context a glass surface sits in — only the shadow depth differs. */
enum class GlassElevation(internal val shadowDp: Dp) {
    /** Nav bars, top bars — `blur: 20px` in the CSS/Flutter twins. */
    Chrome(8.dp),

    /** Sheets, dialogs, popovers — `blur: 28px` in the CSS/Flutter twins, and sit visually higher. */
    Sheet(12.dp),
}

/** §3.4's numbers, named rather than repeated at each call site. */
object GlassAlpha {
    /** `tint: the tier's fill at 72% alpha`. */
    const val tint = 0.72f

    /** `highlight: inset 0 1px 0 <foreground at 10%>` — the specular top edge. */
    const val highlight = 0.10f

    /** `shadow: 0 8px 32px <background at 40%>`. */
    const val shadow = 0.40f

    /** `border: 1px <border at 60%>`. */
    const val border = 0.60f
}

/**
 * Applies the glass tier: translucent [popover][ComradeSurfaces.popover] tint
 * at [GlassAlpha.tint], a specular highlight along the top edge, a depth
 * shadow, and a hairline border — everything §3.4 asks for except the blur
 * (§5). [shape] should match whatever shape the composable itself clips to
 * (a `TopAppBar`/`NavigationBar`'s default rectangle, or a sheet/dialog's
 * rounded corners) so the highlight traces the same outline the content does.
 *
 * The specular highlight is what separates this from "a translucent box" —
 * skipping it is the most common way this material ends up looking cheap
 * (§3.4). It is drawn as a single 1dp line inset from the top, clipped to
 * [shape] so it still traces rounded corners correctly.
 */
@Composable
fun Modifier.glassSurface(
    elevation: GlassElevation = GlassElevation.Chrome,
    shape: Shape = RectangleShape,
): Modifier {
    val surfaces = LocalComradeSurfaces.current
    val scheme = MaterialTheme.colorScheme
    // §4.3: under reduced motion the tint goes fully opaque rather than
    // staying translucent — Android's glass has no blur to begin with (§5),
    // so a still-translucent surface is the part of this material a user who
    // asked for no motion would still see moving behind it. See
    // MotionDecisions.glassTintAlpha and Theme.kt's LocalReducedMotion.
    val tintAlpha = MotionDecisions.glassTintAlpha(GlassAlpha.tint, LocalReducedMotion.current)
    val tint = surfaces.popover.copy(alpha = tintAlpha)
    val highlightColor = surfaces.popoverForeground.copy(alpha = GlassAlpha.highlight)
    val borderColor = surfaces.border.copy(alpha = GlassAlpha.border)
    val shadowColor = scheme.background.copy(alpha = GlassAlpha.shadow)
    return this
        .shadow(
            elevation = elevation.shadowDp,
            shape = shape,
            clip = false,
            ambientColor = shadowColor,
            spotColor = shadowColor,
        )
        .clip(shape)
        .background(tint)
        .border(BorderStroke(1.dp, borderColor), shape)
        .drawBehind { drawTopSpecularHighlight(shape, highlightColor) }
}

/** The 1dp "light catches the top edge" line §3.4 calls `highlight`. */
private fun DrawScope.drawTopSpecularHighlight(shape: Shape, color: Color, strokeWidth: Dp = 1.dp) {
    val outline = shape.createOutline(size, layoutDirection, this)
    val path = Path().apply { addOutline(outline) }
    val strokePx = strokeWidth.toPx()
    clipPath(path) {
        drawLine(
            color = color,
            start = Offset(0f, strokePx / 2f),
            end = Offset(size.width, strokePx / 2f),
            strokeWidth = strokePx,
        )
    }
}

/**
 * A `TopAppBar`'s own [colors][androidx.compose.material3.TopAppBar] paint a
 * solid `Surface` background under the content; glass call sites pair this
 * with [glassSurface] on the bar's `modifier` so the bar draws nothing of its
 * own and the tint/highlight/shadow underneath show through. Both container
 * colours (resting and scrolled) go transparent — M3's scroll-elevation
 * colour swap would otherwise fight the glass tint the moment content
 * scrolls under the bar.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun glassTopAppBarColors(): TopAppBarColors = TopAppBarDefaults.topAppBarColors(
    containerColor = Color.Transparent,
    scrolledContainerColor = Color.Transparent,
)

/** [glassTopAppBarColors] for `CenterAlignedTopAppBar`. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun glassCenterAlignedTopAppBarColors(): TopAppBarColors = TopAppBarDefaults.centerAlignedTopAppBarColors(
    containerColor = Color.Transparent,
    scrolledContainerColor = Color.Transparent,
)
