package mullu.comrade

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * On-device journey test for the Telegram-like flow: the onboarding door
 * renders without blocking on the native core, creating an identity (username
 * + passcode) unlocks the vault through real Rust crypto, and the main shell
 * (Chats / Feed / Settings) comes up with working bottom navigation.
 *
 * The test adapts to residual state: on a fresh emulator it walks the create
 * path; if a previous run on the same device already created the vault (or the
 * process still holds the unlocked runtime) it unlocks — with the same
 * passcode — or lands straight in the shell.
 */
@RunWith(AndroidJUnit4::class)
class MainActivityUiTest {

    @get:Rule
    val composeRule = createAndroidComposeRule<MainActivity>()

    private fun hasText(text: String) =
        composeRule.onAllNodesWithText(text).fetchSemanticsNodes().isNotEmpty()

    @Test
    fun onboardingLeadsToChatsShell() {
        // The startup check resolves into one of three doors.
        composeRule.waitUntil(timeoutMillis = 30_000) {
            hasText("Create my identity") || hasText("Unlock") || hasText("Chats")
        }

        if (hasText("Create my identity")) {
            composeRule.onNodeWithTag("onboarding-username").performTextInput(USERNAME)
            composeRule.onNodeWithTag("onboarding-passcode").performTextInput(PASSCODE)
            composeRule.onNodeWithTag("onboarding-confirm").performTextInput(PASSCODE)
            composeRule.onNodeWithTag("onboarding-submit").performClick()
        } else if (hasText("Unlock")) {
            composeRule.onNodeWithTag("onboarding-passcode").performTextInput(PASSCODE)
            composeRule.onNodeWithTag("onboarding-submit").performClick()
        }

        // Argon2 key stretching + engine construction run off the UI thread;
        // the shell appears when the vault is open.
        composeRule.waitUntil(timeoutMillis = 120_000) { hasText("Chats") }

        // Bottom navigation reaches every section.
        composeRule.onNodeWithText("Feed").performClick()
        composeRule
            .onNodeWithText("Public — anyone on the network can read this.")
            .assertIsDisplayed()

        composeRule.onNodeWithText("Settings").performClick()
        composeRule.onNodeWithText("Your identity key").assertIsDisplayed()
        composeRule.onNodeWithText("@$USERNAME").assertIsDisplayed()
    }

    private companion object {
        const val USERNAME = "ci_tester"
        const val PASSCODE = "comrade-ci-passcode"
    }
}
