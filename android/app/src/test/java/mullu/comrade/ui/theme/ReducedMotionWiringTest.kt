package mullu.comrade.ui.theme

import java.io.File
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * [MotionDecisions] is pure and thoroughly tested, and that is exactly what
 * makes this file necessary: every one of those tests passed while
 * `LocalReducedMotion` was never provided from anywhere, so the composable
 * accessors read the `compositionLocalOf { false }` default forever and
 * §4.3's escape hatch was inert on a real device. The Compose type-check
 * lane could not see it either — a CompositionLocal left at its default
 * compiles perfectly. `AUDIT.md` V4 had already been written claiming the
 * hatch was wired.
 *
 * That is the same failure `desktop/ui/dom_bindings.test.mjs` was added for
 * on the other frontend: code that resolves, tests that pass, and a feature
 * that does nothing. Like that test, this one asserts against the source
 * text, because the thing being checked is a wiring fact no unit-level
 * assertion in this lane can observe.
 */
class ReducedMotionWiringTest {

    private fun themeSource(): String {
        // Gradle runs tests from `android/app`; the kotlinc recipe in
        // CLAUDE.md runs from elsewhere. Walk up to whichever ancestor holds
        // the file rather than assuming either.
        val relative = "src/main/java/mullu/comrade/ui/theme/Theme.kt"
        var dir: File? = File(".").absoluteFile
        while (dir != null) {
            for (candidate in listOf(File(dir, relative), File(dir, "android/app/$relative"))) {
                if (candidate.isFile) return candidate.readText()
            }
            dir = dir.parentFile
        }
        throw AssertionError("could not locate Theme.kt from ${File(".").absolutePath}")
    }

    @Test
    fun themeProvidesTheReducedMotionLocal() {
        assertTrue(
            "ComradeTheme must provide LocalReducedMotion — without it the " +
                "§4.3 hatch silently keeps its `false` default and never fires",
            themeSource().contains("LocalReducedMotion provides"),
        )
    }

    @Test
    fun themeReadsThePlatformSignalRatherThanHardcodingIt() {
        val source = themeSource()
        assertTrue(
            "ComradeTheme must read Settings.Global.ANIMATOR_DURATION_SCALE — " +
                "the one reduced-motion signal Android exposes (AUDIT.md V4)",
            source.contains("ANIMATOR_DURATION_SCALE"),
        )
        assertTrue(
            "the scale must be interpreted by MotionDecisions.isReducedMotion, " +
                "so the threshold stays on the framework-free, testable side",
            source.contains("MotionDecisions.isReducedMotion("),
        )
    }
}
