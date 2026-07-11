package mullu.comrade

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * On-device Compose test for the asynchronous startup path.
 *
 * The first frame must render without waiting for the native core
 * (`System.loadLibrary` runs off the main thread), and the workspace list must
 * stream in once the core is ready. A regression that blocks the first frame
 * on JNI, or that never completes the async load, fails here.
 */
@RunWith(AndroidJUnit4::class)
class MainActivityUiTest {

    @get:Rule
    val composeRule = createAndroidComposeRule<MainActivity>()

    @Test
    fun shellRendersImmediatelyAndWorkspacesStreamIn() {
        // Static shell content is on screen from the very first frame.
        composeRule.onNodeWithText("Comrade").assertIsDisplayed()
        composeRule.onNodeWithText("Workspaces").assertIsDisplayed()

        // The async core load completes and the Base workspace card appears.
        composeRule.waitUntil(timeoutMillis = 30_000) {
            composeRule.onAllNodesWithText("Base", substring = true)
                .fetchSemanticsNodes().isNotEmpty()
        }
        // The version footer flipped from the placeholder to the real value.
        composeRule.waitUntil(timeoutMillis = 5_000) {
            composeRule.onAllNodesWithText("core v", substring = true)
                .fetchSemanticsNodes().isNotEmpty()
        }
    }

    @Test
    fun bottomNavigationSwitchesSections() {
        composeRule.onNodeWithText("Voice").performClick()
        composeRule.onNodeWithText("Voice Assistant").assertIsDisplayed()

        composeRule.onNodeWithText("Keys").performClick()
        composeRule.onNodeWithText("Key Management").assertIsDisplayed()

        composeRule.onNodeWithText("Home").performClick()
        composeRule.onNodeWithText("Workspaces").assertIsDisplayed()
    }
}
