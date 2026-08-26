package mullu.comrade.ui.theme

import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Shapes
import androidx.compose.ui.unit.dp

/**
 * `docs/DESIGN_SYSTEM.md` §3.2's derived radius scale, `--radius: 12px` and
 * everything stepped off it. One knob: change [base] here and the whole app
 * re-proportions, which is the point of a derived scale over six independent
 * numbers.
 */
object ComradeRadii {
    val base = 12.dp
    /** Chips, ticks, small controls. */
    val sm = 8.dp
    /** Inputs, buttons. */
    val md = 10.dp
    /** Cards. */
    val lg = base
    /** Sheets, dialogs, bubbles. */
    val xl = 18.dp
    /** The vault card, full-screen surfaces. */
    val xxl = 28.dp
    /** Chat bubbles: 18dp everywhere except the "tail" corner, which is 6dp. */
    val bubbleTail = 6.dp
}

/**
 * M3 [Shapes] has five slots; §3.2's scale has six named steps, so `2xl`
 * (the vault card / full-screen surfaces) has no M3 role to sit in and is
 * used directly from [ComradeRadii] at its one call site instead. The other
 * five map onto M3's own slots in the same order §3.2 lists them.
 */
val ComradeShapes = Shapes(
    extraSmall = RoundedCornerShape(ComradeRadii.sm),
    small = RoundedCornerShape(ComradeRadii.md),
    medium = RoundedCornerShape(ComradeRadii.lg),
    large = RoundedCornerShape(ComradeRadii.xl),
    extraLarge = RoundedCornerShape(ComradeRadii.xxl),
)
