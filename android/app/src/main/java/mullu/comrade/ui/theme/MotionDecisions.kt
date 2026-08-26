package mullu.comrade.ui.theme

/**
 * `docs/DESIGN_SYSTEM.md` §4.3: `prefers-reduced-motion: reduce` — "drift/
 * ambient animations stop and transitions collapse to `fast`." Kept
 * framework-free (Kotlin stdlib only) so it runs under CLAUDE.md's
 * kotlinc+JUnit recipe with no Android classpath; [Theme.kt] reads the actual
 * platform signal and [Glass.kt]/`ComradeMotion`'s composable accessors call
 * these functions with it.
 *
 * Android has no [Settings.Global.ANIMATOR_DURATION_SCALE] equivalent for
 * §4.2's `prefers-reduced-transparency` — that gap is real and stays real
 * (there is no documented API for it). What Android *does* have is the
 * animator-duration-scale signal that already backs Flutter's
 * `MediaQuery.disableAnimations` on this same platform, and reduced motion is
 * made to cover most of what reduced transparency would have: alongside
 * collapsing durations, the glass tint goes opaque, because Android's glass
 * has no blur to begin with (§5) and a translucent surface that keeps moving
 * content visible behind it is exactly the failure mode §4.2 exists to catch.
 */
object MotionDecisions {
    /**
     * `Settings.Global.getFloat(resolver, ANIMATOR_DURATION_SCALE, 1f)`, read
     * by the caller — `0f` means the user turned animations off system-wide.
     */
    fun isReducedMotion(animatorDurationScale: Float): Boolean = animatorDurationScale == 0f

    /** Durations collapse to zero rather than merely "fast" when reduced motion is on. */
    fun durationMs(baseMs: Int, reducedMotion: Boolean): Int = if (reducedMotion) 0 else baseMs

    /**
     * The glass tint's alpha: [baseAlpha] normally (§3.4's 72%), fully opaque
     * (`1f`) under reduced motion — see the class doc for why this is also
     * this app's answer to §4.2, which Android has no direct signal for.
     */
    fun glassTintAlpha(baseAlpha: Float, reducedMotion: Boolean): Float =
        if (reducedMotion) 1f else baseAlpha
}
