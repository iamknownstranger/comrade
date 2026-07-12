package mullu.comrade

import android.Manifest
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performTextInput
import androidx.test.espresso.Espresso
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.rule.GrantPermissionRule
import org.junit.Rule
import org.junit.Test
import org.junit.rules.RuleChain
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

    private val composeRule = createAndroidComposeRule<MainActivity>()

    // Grant POST_NOTIFICATIONS *before* the activity launches: the shell's
    // first-run notification prompt would otherwise pop a system dialog over
    // the app, pausing MainActivity — and a paused activity exposes no
    // queryable Compose hierarchy, which fails the semantics assertions below.
    // The outer rule runs first, so the permission is already granted when
    // MainShell mounts and the app never prompts.
    @get:Rule
    val rules: RuleChain = RuleChain
        .outerRule(GrantPermissionRule.grant(Manifest.permission.POST_NOTIFICATIONS))
        .around(composeRule)

    private fun hasText(text: String) =
        composeRule.onAllNodesWithText(text).fetchSemanticsNodes().isNotEmpty()

    private fun hasTag(tag: String) =
        composeRule.onAllNodesWithTag(tag).fetchSemanticsNodes().isNotEmpty()

    /** The onboarding error line's text, when one is showing. */
    private fun onboardingError(): String? {
        val node = composeRule.onAllNodesWithTag("onboarding-error").fetchSemanticsNodes()
            .firstOrNull() ?: return null
        if (!node.config.contains(SemanticsProperties.Text)) return null
        return node.config[SemanticsProperties.Text].joinToString()
    }

    private fun submitOnboarding() {
        // Typing opens the soft keyboard, which can cover the submit button and
        // swallow the injected tap — close it and scroll the button into view.
        Espresso.closeSoftKeyboard()
        composeRule.onNodeWithTag("onboarding-submit").performScrollTo().performClick()
    }

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
            submitOnboarding()
        } else if (hasText("Unlock")) {
            composeRule.onNodeWithTag("onboarding-passcode").performTextInput(PASSCODE)
            submitOnboarding()
        }

        // Argon2 key stretching + engine construction run off the UI thread;
        // the shell appears when the vault is open. Fail fast — with the
        // on-screen message — if onboarding surfaced an error instead.
        composeRule.waitUntil(timeoutMillis = 120_000) {
            hasText("Chats") || onboardingError() != null
        }
        onboardingError()?.let { message ->
            throw AssertionError("Onboarding reported an error: $message")
        }

        // The IME may still be up from the onboarding fields; drop it so taps
        // reach the bottom navigation.
        Espresso.closeSoftKeyboard()

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
