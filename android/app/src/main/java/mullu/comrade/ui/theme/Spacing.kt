package mullu.comrade.ui.theme

import androidx.compose.ui.unit.dp

/**
 * `docs/DESIGN_SYSTEM.md` §7.1: a 4dp grid, named rather than typed at each
 * call site. Android alone had carried 138 off-grid values (10, 6, 18, 14, 3,
 * 1, 7, 9, 11…) against 52 uses of 16 and 28 of 8 — nobody reads "14dp" on a
 * screen, what shows is two screens whose edges don't line up, which is
 * exactly the "unfinished" read this section exists to fix.
 *
 * Every padding, gap and inset should resolve to one of these. §7.1 permits
 * off-grid values in exactly two places, and both must say which at the call
 * site: a 1px/1dp hairline, and an optical correction compensating for a
 * glyph or icon's own bearing.
 */
object Spacing {
    val space1 = 4.dp
    val space2 = 8.dp
    val space3 = 12.dp
    val space4 = 16.dp
    val space5 = 20.dp
    val space6 = 24.dp
    val space8 = 32.dp
    val space10 = 40.dp
    val space12 = 48.dp
    val space16 = 64.dp
}
