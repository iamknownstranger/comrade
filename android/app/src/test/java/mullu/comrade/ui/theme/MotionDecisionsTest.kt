package mullu.comrade.ui.theme

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * `docs/DESIGN_SYSTEM.md` §4.3's reduced-motion decisions, pure so they run
 * under CLAUDE.md's kotlinc+JUnit recipe with no Android classpath.
 */
class MotionDecisionsTest {

    // ── isReducedMotion ──────────────────────────────────────────────────────

    @Test
    fun zeroScaleIsReducedMotion() {
        assertTrue(MotionDecisions.isReducedMotion(0f))
    }

    @Test
    fun normalScaleIsNotReducedMotion() {
        assertFalse(MotionDecisions.isReducedMotion(1f))
    }

    @Test
    fun aSlowedButNonZeroScaleIsNotReducedMotion() {
        // "Animator duration scale" set to e.g. 2x (a debug/accessibility
        // slow-motion setting) is not the same request as "off" — only exactly
        // 0f means the user asked for no animation at all.
        assertFalse(MotionDecisions.isReducedMotion(2f))
    }

    // ── durationMs ───────────────────────────────────────────────────────────

    @Test
    fun durationCollapsesToZeroUnderReducedMotion() {
        assertEquals(0, MotionDecisions.durationMs(baseMs = 320, reducedMotion = true))
    }

    @Test
    fun durationIsUnchangedOtherwise() {
        assertEquals(320, MotionDecisions.durationMs(baseMs = 320, reducedMotion = false))
    }

    // ── glassTintAlpha ───────────────────────────────────────────────────────

    @Test
    fun glassGoesFullyOpaqueUnderReducedMotion() {
        assertEquals(1f, MotionDecisions.glassTintAlpha(baseAlpha = 0.72f, reducedMotion = true))
    }

    @Test
    fun glassStaysAtTheDesignedAlphaOtherwise() {
        assertEquals(0.72f, MotionDecisions.glassTintAlpha(baseAlpha = 0.72f, reducedMotion = false))
    }
}
