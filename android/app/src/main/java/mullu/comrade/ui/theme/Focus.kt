package mullu.comrade.ui.theme

import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.unit.dp

/**
 * `docs/DESIGN_SYSTEM.md` §4: "Every interactive element shows a 2px `ring`
 * offset 2px on `:focus-visible`, in every tier including glass." TV remotes,
 * keyboards and switch access all drive focus rather than touch, and this app
 * had no visible focus indicator at all before this pass — a 2dp
 * [ring][ComradeSurfaces.ring] stroke, drawn 2dp *outside* the element's own
 * bounds so it never steals layout space or reflows the row when focus
 * arrives.
 *
 * Drawing outside the bounds has one cost worth stating rather than
 * discovering: a parent that clips (`Modifier.clip`, a `Card`, a rounded
 * sheet) clips this ring too, so on an element flush against such an edge
 * the ring can be trimmed. Give the element a little padding inside the
 * clipping parent where that matters — the alternative, drawing the ring
 * inside the bounds, trades a rare trim for permanently covering the
 * element's own edge.
 *
 * Order matters: put this **before** `clickable`/`selectable` in the chain so
 * it draws around whatever focus target the interaction modifier establishes,
 * rather than requesting a second, redundant one of its own — this modifier
 * only observes focus, it never requests it.
 *
 * [shape] should be [RoundedCornerShape] or [RectangleShape] — the two Compose
 * ships a closed-form corner radius for; anything else falls back to a square
 * ring, which is still visible, just not shape-matched.
 */
@Composable
fun Modifier.comradeFocusRing(shape: Shape = RectangleShape): Modifier {
    var focused by remember { mutableStateOf(false) }
    val ring = LocalComradeSurfaces.current.ring
    return this
        .onFocusChanged { focused = it.isFocused }
        .drawWithContent {
            drawContent()
            if (!focused) return@drawWithContent
            val offset = 2.dp.toPx()
            val strokeWidth = 2.dp.toPx()
            val expandedSize = Size(size.width + offset * 2, size.height + offset * 2)
            val topLeft = Offset(-offset, -offset)
            if (shape is RoundedCornerShape) {
                val radius = shape.topStart.toPx(size, this) + offset
                drawRoundRect(
                    color = ring,
                    topLeft = topLeft,
                    size = expandedSize,
                    cornerRadius = CornerRadius(radius, radius),
                    style = Stroke(width = strokeWidth),
                )
            } else {
                drawRect(
                    color = ring,
                    topLeft = topLeft,
                    size = expandedSize,
                    style = Stroke(width = strokeWidth),
                )
            }
        }
}
