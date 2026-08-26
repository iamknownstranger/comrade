package mullu.comrade.ui.theme

import androidx.compose.foundation.background
import androidx.compose.foundation.interaction.InteractionSource
import androidx.compose.foundation.interaction.collectIsDraggedAsState
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.interaction.collectIsHoveredAsState
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.RectangleShape

/**
 * `docs/DESIGN_SYSTEM.md` §3.3: the M3 state-layer opacities, as constants —
 * "these are constants, not per-component choices. A component that wants a
 * different hover strength is wrong about being a different component."
 * Applied as an overlay of the *foreground* colour over the surface.
 */
object ComradeStateLayer {
    const val hover = 0.08f
    const val focus = 0.10f
    const val pressed = 0.10f
    const val dragged = 0.16f
    const val selected = 0.12f
    const val disabledContent = 0.38f
    const val disabledContainer = 0.12f
}

/**
 * Draws [foreground] over the composable at the §3.3 opacity for whichever
 * interaction [interactionSource] is currently reporting, highest-priority
 * state first (dragged > pressed > focus > hover). A component with no state
 * layer of its own — a custom row, a glass chrome control — reaches for this
 * instead of inventing its own hover alpha.
 */
@Composable
fun Modifier.comradeStateLayer(
    interactionSource: InteractionSource,
    foreground: Color,
    shape: Shape = RectangleShape,
): Modifier {
    val dragged by interactionSource.collectIsDraggedAsState()
    val pressed by interactionSource.collectIsPressedAsState()
    val focused by interactionSource.collectIsFocusedAsState()
    val hovered by interactionSource.collectIsHoveredAsState()
    val alpha = when {
        dragged -> ComradeStateLayer.dragged
        pressed -> ComradeStateLayer.pressed
        focused -> ComradeStateLayer.focus
        hovered -> ComradeStateLayer.hover
        else -> 0f
    }
    return if (alpha == 0f) {
        this
    } else {
        this.background(foreground.copy(alpha = alpha), shape)
    }
}
