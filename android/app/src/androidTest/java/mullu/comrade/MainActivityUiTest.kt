package mullu.comrade

import android.Manifest
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.ComposeTimeoutException
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithContentDescription
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
 * (Chats / Journal / Together / Tara, with Settings and Feed reached from the navigation
 * drawer) comes up with working bottom navigation.
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
    // RECORD_AUDIO joins it for the journal's voice-entry leg below: without
    // the grant, tapping record pops the runtime permission dialog over the
    // app, which pauses the Activity and empties the Compose hierarchy the
    // assertions read.
    // ACCESS_COARSE_LOCATION joins it for the Travel leg below: granting it
    // before launch means that leg actually walks Locating → Loading → a
    // terminal state instead of parking on the permission prompt
    // (`TravelDecisions.State.NeedsPermission`), which is the one branch a
    // real device run needs to get past to prove the state-machine
    // transitions are safe.
    @get:Rule
    val rules: RuleChain = RuleChain
        .outerRule(
            GrantPermissionRule.grant(
                Manifest.permission.POST_NOTIFICATIONS,
                Manifest.permission.RECORD_AUDIO,
                Manifest.permission.ACCESS_COARSE_LOCATION,
            ),
        )
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

    /**
     * Hide the soft keyboard before tapping something it might be covering.
     *
     * [Espresso.closeSoftKeyboard] resolves a root view **with window focus**
     * (`RootViewPicker`), and on a cold emulator that focus can lag the Compose
     * hierarchy — the app is drawn and queryable while the window has not been
     * granted focus yet. Espresso only waits 10s and then fails with
     * `RootViewWithoutFocusException`, which is what flaked this test on CI.
     * Waiting for focus first makes the precondition explicit instead of racing
     * it; if focus genuinely never arrives that is a real problem and this
     * fails with a message that says so, rather than an opaque root dump.
     */
    private fun dismissKeyboard() {
        // Read the flag straight off the activity rather than hopping to the UI
        // thread: it is a null-safe field read, and because this is a polling
        // loop a stale `false` costs one extra poll rather than a wrong answer.
        composeRule.waitUntil(timeoutMillis = FOCUS_TIMEOUT_MS) {
            composeRule.activity.hasWindowFocus()
        }
        Espresso.closeSoftKeyboard()
    }

    private fun submitOnboarding() {
        // Typing opens the soft keyboard, which can cover the submit button and
        // swallow the injected tap — close it and scroll the button into view.
        dismissKeyboard()
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
        try {
            composeRule.waitUntil(timeoutMillis = 120_000) {
                hasText("Chats") || onboardingError() != null
            }
        } catch (timeout: ComposeTimeoutException) {
            // Say what was actually on screen: a stuck onboarding form (the tap
            // never landed) and a genuinely slow unlock are very different bugs,
            // and the bare timeout distinguishes neither.
            val screen = if (hasTag("onboarding-submit")) {
                "the onboarding form is still showing — the submit tap did not take effect"
            } else {
                "neither the shell nor the onboarding form is showing"
            }
            throw AssertionError(
                "Vault never opened within 120s: $screen " +
                    "(error line: ${onboardingError() ?: "none"})",
                timeout,
            )
        }
        onboardingError()?.let { message ->
            throw AssertionError("Onboarding reported an error: $message")
        }

        // The IME may still be up from the onboarding fields; drop it so taps
        // reach the bottom navigation.
        dismissKeyboard()

        // Bottom navigation reaches Together, which took the slot Feed had.
        composeRule.onNodeWithText("Together").performClick()
        composeRule.waitForIdle()

        // The Journal tab, and it has to stay up *past* its loads. Three
        // separate effects land asynchronously here — the entry list, the
        // attention mirror, and (since the video journal) the orphan sweep —
        // and each one is a recomposition that a wrong group count would kill.
        // A bare "the composer exists" assertion passes against that build,
        // which is why this sits through them and asserts again afterwards.
        composeRule.onNodeWithText("Journal").performClick()
        composeRule.waitForIdle()
        composeRule.waitUntil(timeoutMillis = 15_000) { hasTag("journal-input") }
        Thread.sleep(1_500)
        composeRule.waitForIdle()
        composeRule.onNodeWithTag("journal-save").assertIsDisplayed()

        // Recording a voice entry, which is the sharpest version of the same
        // hazard: while it runs, an elapsed counter recomposes the composer
        // four times a second, and a status line appears where none was. That
        // is a changing composable-group count on a timer — the exact shape
        // that killed TaskListScreen and RideScreen, and the exact shape no
        // lane but this one can see.
        //
        // Conditional because the button is capability-gated on a microphone
        // and an emulator image without one is a legitimate configuration, not
        // a failure. RECORD_AUDIO is pre-granted above so no system dialog can
        // cover the app mid-test.
        if (hasTag("journal-record-audio")) {
            composeRule.onNodeWithTag("journal-record-audio").performClick()
            composeRule.waitForIdle()
            // Sit through several ticks of the counter, then assert the screen
            // is still there. A "the button exists" check would pass against a
            // build that dies on the first tick.
            Thread.sleep(1_500)
            composeRule.waitForIdle()
            composeRule.onNodeWithTag("journal-record-audio").assertIsDisplayed()
            // Stop, and let it settle. Either the title dialog comes up over a
            // real recording or an error line says the mic gave nothing — on a
            // headless emulator both are legitimate. What is asserted is that
            // the composer survived the whole round trip.
            composeRule.onNodeWithTag("journal-record-audio").performClick()
            composeRule.waitForIdle()
            Thread.sleep(1_000)
            composeRule.waitForIdle()
            if (hasTag("journal-recording-title-dialog")) {
                // A recording was actually captured: throw it away rather than
                // leaving a file behind for the next run to sweep.
                composeRule.onNodeWithText("Discard").performClick()
                composeRule.waitForIdle()
            }
            composeRule.onNodeWithTag("journal-input").assertIsDisplayed()
        }

        // Settings is a pushed screen now, reached from the navigation drawer
        // (Telegram-style) rather than a bottom-nav tab. The hamburger only
        // lives on the chat list, so return there before opening the drawer.
        composeRule.onNodeWithText("Chats").performClick()
        composeRule.waitForIdle()
        composeRule.onNodeWithTag("nav-drawer-button").performClick()
        composeRule.waitForIdle()

        // The drawer's profile header shows our @handle…
        composeRule.onNodeWithText("@$USERNAME").assertIsDisplayed()

        // …and Feed is still reachable from it. Asserted because "off the nav,
        // not removed" is only true if there is a way back to it, and a feature
        // no route reaches is a deleted feature with its code left behind.
        composeRule.onNodeWithTag("drawer-feed").performClick()
        composeRule.waitForIdle()
        composeRule
            .onNodeWithText("Public — anyone on the network can read this.")
            .assertIsDisplayed()
        Espresso.pressBack()
        composeRule.waitForIdle()
        composeRule.onNodeWithTag("nav-drawer-button").performClick()
        composeRule.waitForIdle()

        // …and its Tasks item opens the task list. Asserted on a device because
        // this screen shipped in a state where opening it killed the process, and
        // nothing in the JVM lane could see that: `TaskList`'s decisions are all
        // unit-tested, and the crash was in the *composition* around them. The
        // empty-state node is the proof it survived a full load — it only appears
        // once the store has answered, which is the recomposition that failed.
        composeRule.onNodeWithTag("drawer-tasks").performClick()
        composeRule.waitForIdle()
        composeRule.waitUntil(timeoutMillis = 15_000) {
            hasTag("tasks-empty") || hasTag("task-list")
        }

        // Back to the list, then into Settings — a pushed destination, unlike the
        // Chats sub-screen above.
        composeRule.onNodeWithContentDescription("Back").performClick()
        composeRule.waitForIdle()
        composeRule.onNodeWithTag("nav-drawer-button").performClick()
        composeRule.waitForIdle()

        // Ride opens, and *stays* open across a recomposition. Asserted on a
        // device for the same reason the Tasks tap above is: this screen
        // shipped killing the process on open, from the same defect
        // (`AUDIT.md` 2026-08-04) — an early `return@Column`, so the Column
        // emitted a different number of composable groups once state flipped,
        // which throws on the recomposition and not on the first frame.
        //
        // So a bare "the tag exists" assertion would have passed against the
        // broken build. The waits are what make this a regression test: the
        // comrade list arrives asynchronously and the card's clock ticks once a
        // second, so sitting here past a tick forces the recomposition that
        // used to crash, and the seat buttons must still be there afterwards.
        composeRule.onNodeWithTag("drawer-ride").performClick()
        composeRule.waitForIdle()
        composeRule.onNodeWithTag("ride-seat-driver").assertIsDisplayed()
        composeRule.onNodeWithTag("ride-card").assertDoesNotExist()
        Thread.sleep(1_500)
        composeRule.waitForIdle()
        composeRule.onNodeWithTag("ride-seat-driver").assertIsDisplayed()

        // Picking a seat is the other group-count flip: the setup arm is
        // replaced by the card, the music controls and the phrase grid. It has
        // to survive its own tick too.
        composeRule.onNodeWithTag("ride-seat-pillion").performClick()
        composeRule.waitForIdle()
        Thread.sleep(1_500)
        composeRule.waitForIdle()
        // Still alive, and still showing *a* ride screen — with no comrade
        // chosen the setup arm is correct, so assert the process survived
        // rather than which arm won, which depends on the device's contacts.
        composeRule.onNodeWithText("Ride").assertIsDisplayed()

        Espresso.pressBack()
        composeRule.waitForIdle()

        composeRule.onNodeWithTag("nav-drawer-button").performClick()
        composeRule.waitForIdle()

        // Travel, reached from the drawer rather than the bottom bar: it moved
        // there when the bar went back to five (`docs/DESIGN_SYSTEM.md` §7.7).
        // This leg is one thing, not two — it proves the route still exists
        // (the Feed assertion's purpose: "off the nav, not removed" only holds
        // if something still reaches it) *and* that the screen survives its own
        // load, which is why it was written in the first place. Travel had
        // never been opened on a device — the gap docs/TRAVEL.md's TRAVEL-1
        // names, and the bug class Tasks and Ride both shipped: an early
        // return@Column passes every lane that runs before CI and only fails on
        // a device mid-recomposition. Coarse location is pre-granted above, so
        // the screen really walks Locating → Loading → a terminal state instead
        // of parking on the permission prompt.
        composeRule.onNodeWithTag("drawer-travel").performClick()
        composeRule.waitForIdle()
        // The screen root, not the word "Travel": `travel_tab` labels the
        // drawer item too, and a closed `ModalNavigationDrawer` keeps its
        // content composed, so a text finder matches two nodes and
        // `assertIsDisplayed` throws on the ambiguity rather than on anything
        // to do with Travel. Feed escapes this only because its string happens
        // to be unique to the screen.
        composeRule.onNodeWithTag("travel-screen").assertIsDisplayed()
        composeRule.waitUntil(timeoutMillis = 60_000) {
            hasTag("travel_guide") || hasText("Try again")
        }
        // Whichever terminal state this emulator's fix and network produced —
        // a guide, or "no fix"/"failed" with a retry — the screen is still
        // alive and takes a follow-up tap: the same "survived the
        // recomposition" proof the Ride leg above relies on.
        if (hasTag("travel_guide")) {
            composeRule.onNodeWithTag("travel_guide").assertIsDisplayed()
        } else {
            composeRule.onNodeWithText("Try again").performClick()
            composeRule.waitForIdle()
        }
        // Back out the way Tasks and Ride do: Travel is a pushed destination
        // now, not a tab, so there is no bottom-bar entry to return through.
        Espresso.pressBack()
        composeRule.waitForIdle()
        composeRule.onNodeWithTag("nav-drawer-button").performClick()
        composeRule.waitForIdle()
        composeRule.onNodeWithTag("drawer-settings").performClick()
        composeRule.waitForIdle()
        composeRule.onNodeWithText("Your identity key").assertIsDisplayed()
        composeRule.onNodeWithText("@$USERNAME").assertIsDisplayed()

        // The profile page, reached where Telegram puts it: the name and key at
        // the top of the drawer. This is the screen D35 moved the public key to,
        // so the key row is what proves it drew.
        Espresso.pressBack()
        composeRule.waitForIdle()
        composeRule.onNodeWithTag("nav-drawer-button").performClick()
        composeRule.waitForIdle()
        composeRule.onNodeWithTag("drawer-profile").performClick()
        composeRule.waitForIdle()
        composeRule.onNodeWithTag("profile-screen").assertIsDisplayed()
        // Wait past a state change, not just the first frame: the screen reads
        // the avatar and the shared history off the vault *after* composing, and
        // an early return in a composable throws on that second pass — the bug
        // class Tasks and Ride both shipped, which a bare "the node exists"
        // assertion passes straight through.
        composeRule.waitUntil(timeoutMillis = 10_000) { hasText("Public key") }
        composeRule.onNodeWithText("Public key").assertIsDisplayed()
        // Still alive after the recomposition, and still takes a tap.
        composeRule.onNodeWithText("Copy key").performClick()
        composeRule.waitForIdle()
        composeRule.onNodeWithTag("profile-screen").assertIsDisplayed()
        Espresso.pressBack()
        composeRule.waitForIdle()
    }

    private companion object {
        const val USERNAME = "ci_tester"
        const val PASSCODE = "comrade-ci-passcode"

        /**
         * How long to wait for the activity window to gain focus. Generous
         * because it is a cold-emulator warm-up, not app work — but bounded, so
         * a window that never focuses fails loudly instead of hanging.
         */
        const val FOCUS_TIMEOUT_MS = 20_000L
    }
}
