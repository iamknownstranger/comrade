package global.auros.comrade

import androidx.lifecycle.Lifecycle
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * On-device smoke suite, run against the assembled APK on an emulator or a
 * physical device via `./gradlew connectedDebugAndroidTest` (CI: the
 * "Android APK" workflow's device lanes).
 *
 * These cover what the JVM unit tests cannot: that libcomrade_jni.so actually
 * loads for the device's ABI, that real Rust crypto executes through the JNI
 * bridge, and that the Compose UI reaches RESUMED on a real Android runtime.
 */
@RunWith(AndroidJUnit4::class)
class DeviceSmokeTest {

    @Test
    fun jniLibraryLoadsAndReportsVersion() {
        // First touch of ComradeCore triggers System.loadLibrary("comrade_jni");
        // an UnsatisfiedLinkError here means the .so for this ABI is missing.
        assertTrue(ComradeCore.getVersion().isNotBlank())
    }

    @Test
    fun keypairGenerationRoundTripsThroughRust() {
        val keypair = ComradeCore.generateKeypairTyped()
        assertTrue(keypair.npub.startsWith("npub1"))
        assertTrue(keypair.nsec.startsWith("nsec1"))
        assertEquals(keypair.npub, ComradeCore.getNpubFromNsec(keypair.nsec))
    }

    @Test
    fun invalidNsecIsRejected() {
        assertNull(ComradeCore.getNpubFromNsec("nsec1notavalidkey"))
    }

    @Test
    fun workspacesAreExposedWithLabels() {
        val workspaces = ComradeCore.workspaces()
        assertTrue(workspaces.any { it.key == "Base" })
        workspaces.forEach { workspace ->
            assertTrue(workspace.label.isNotBlank())
            assertEquals(workspace.label, ComradeCore.workspaceLabel(workspace.key))
        }
    }

    @Test
    fun mainActivityLaunchesToResumed() {
        ActivityScenario.launch(MainActivity::class.java).use { scenario ->
            scenario.moveToState(Lifecycle.State.RESUMED)
            scenario.onActivity { activity -> assertFalse(activity.isFinishing) }
        }
    }
}
