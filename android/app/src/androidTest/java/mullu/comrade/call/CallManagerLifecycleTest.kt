package mullu.comrade.call

import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import kotlinx.coroutines.runBlocking
import mullu.comrade.ComradeCore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.BeforeClass
import org.junit.Test
import org.junit.runner.RunWith
import uniffi.comrade_core.CallMediaKind
import java.io.File

/**
 * On-device lifecycle/reducer tests for [CallManager]'s call-setup state
 * machine (AUDIT.md COMMS-05) — specifically the cancel-before-session race
 * [startOutgoingCall] and [hangup] document. These need the real device
 * runtime: [CallManager] drives an actual `org.webrtc.PeerConnectionFactory`
 * and [ComradeCore.placeCallTyped] crosses the real JNI boundary into the
 * Rust core, neither of which a plain JVM unit test can load (see
 * `CallManagerTest.kt` for the pure-function tests that *can* run there).
 *
 * These tests share the process-wide [CallManager]/[ComradeCore] singletons
 * with every other instrumented test in this module — a fresh, disposable
 * vault directory is unlocked once for the whole class (idempotent, and
 * `placeCallTyped` only needs a vault to be unlocked, not any particular
 * identity) so other suites (e.g. [mullu.comrade.DeviceSmokeTest]) that also
 * touch these singletons are unaffected beyond "the vault happens to already
 * be unlocked" — which is itself a valid, already-handled state for them.
 */
@RunWith(AndroidJUnit4::class)
class CallManagerLifecycleTest {

    companion object {
        /** How long the [CallUiState.Ended] card lingers before [CallManager] itself returns to Idle. */
        private const val ENDED_LINGER_MARGIN_MS = 2_500L

        @JvmStatic
        @BeforeClass
        fun unlockATestVault() {
            val context = ApplicationProvider.getApplicationContext<android.content.Context>()
            val dir = File(context.filesDir, "call-lifecycle-test-vault")
            runCatching { ComradeCore.unlockVaultTyped(dir.absolutePath, "test-passphrase") }
            // These tests target the call *state machine*; the cancel-races
            // they exercise can still reach setupPeer and start the real
            // CallService foreground service, whose start→immediate-stop is
            // exactly the "did not call startForeground" process kill on a
            // loaded emulator — see [CallManager.disableCallServiceForTest].
            CallManager.disableCallServiceForTest = true
        }

        /** A syntactically valid, but unreachable/unknown, peer for tests that never expect real delivery. */
        private fun freshStrangerNpub(): String = ComradeCore.generateKeypairTyped().npub
    }

    /**
     * The exact bug this fix closes: `hangup()` called on the *same thread*,
     * immediately after `startOutgoingCall()` returns — before the
     * dispatcher has had any chance to run the `io.launch` continuation —
     * used to find `session == null` (it was only assigned once
     * `placeCallTyped` resolved) and silently no-op, leaving the UI stuck on
     * "Calling…" and letting the *later* continuation send an offer anyway.
     * With the provisional session created synchronously in
     * `startOutgoingCall`, `hangup()` here always has something to act on.
     */
    @Test
    fun hangup_immediately_after_start_never_sticks_on_ringing() {
        val peer = freshStrangerNpub()
        CallManager.startOutgoingCall(
            ApplicationProvider.getApplicationContext(),
            peer,
            "Cancel-before-session peer",
            CallMediaKind.AUDIO,
        )
        // No delay, no yield — this is the whole point of the test.
        CallManager.hangup()

        runBlocking { kotlinx.coroutines.delay(ENDED_LINGER_MARGIN_MS) }
        assertEquals(
            "must return to Idle, not stay stuck on Ringing/Connecting",
            CallUiState.Idle,
            CallManager.state.value,
        )
    }

    /**
     * A second call must be placeable right after the first was cancelled —
     * proving the provisional session was actually cleared (`session ==
     * null`), not merely hidden behind a stuck UI state.
     */
    @Test
    fun a_new_call_can_be_placed_right_after_cancelling_the_previous_one() {
        val firstPeer = freshStrangerNpub()
        CallManager.startOutgoingCall(
            ApplicationProvider.getApplicationContext(),
            firstPeer,
            "First",
            CallMediaKind.AUDIO,
        )
        CallManager.hangup()
        runBlocking { kotlinx.coroutines.delay(ENDED_LINGER_MARGIN_MS) }

        val secondPeer = freshStrangerNpub()
        CallManager.startOutgoingCall(
            ApplicationProvider.getApplicationContext(),
            secondPeer,
            "Second",
            CallMediaKind.AUDIO,
        )
        // Capture the state and cancel again immediately — startOutgoingCall
        // sets Ringing synchronously before any async work begins, so the
        // assertion needs no delay to observe it truthfully. Any intervening
        // work here (even just the assertion itself) is enough time for the
        // background placeCallTyped continuation to reach setupPeer on a real
        // device, which this test doesn't need and which only risks a real
        // CallService foreground-service start this test isn't exercising.
        val state = CallManager.state.value
        CallManager.hangup()
        assertTrue(
            "starting a new call after a cancelled one must actually ring, not be ignored as \"already in progress\"",
            state is CallUiState.Ringing && state.peer == secondPeer,
        )
    }

    /**
     * `placeCallTyped` itself failing (here: an unparseable peer key) must
     * still resolve the call to Idle/Ended, not leave it hanging.
     */
    @Test
    fun place_call_failure_resolves_to_ended_not_stuck_ringing() {
        CallManager.startOutgoingCall(
            ApplicationProvider.getApplicationContext(),
            "not-a-valid-npub",
            "Bad peer",
            CallMediaKind.AUDIO,
        )
        runBlocking { kotlinx.coroutines.delay(ENDED_LINGER_MARGIN_MS) }
        assertEquals(CallUiState.Idle, CallManager.state.value)
    }

    /**
     * An incoming offer that arrives while our own outgoing call is still in
     * its provisional phase (placeCallTyped in flight) must not clobber the
     * outgoing call's state — the documented policy (AUDIT.md COMMS-05) is
     * the same "already busy" auto-reject a fully-established call already
     * gets, which falls out of the provisional session occupying `session`
     * from the very start rather than needing special-case handling.
     */
    @Test
    fun incoming_offer_during_outgoing_provisional_phase_does_not_override_local_state() {
        // Both peers (and the DTO built from the second) are generated before
        // startOutgoingCall, and everything below is captured into locals and
        // hung up immediately — minimizing how long the outgoing call's
        // background placeCallTyped continuation has to reach setupPeer/
        // CallService on a real device, which this test doesn't exercise and
        // only risks a real foreground-service start.
        val outgoingPeer = freshStrangerNpub()
        val incomingDto = uniffi.comrade_ui.CallSignalDto(
            callId = "unrelated-incoming-call",
            peer = freshStrangerNpub(),
            media = "audio",
            signal = uniffi.comrade_core.CallSignal.Offer(sdp = "v=0\r\n"),
            createdAt = (System.currentTimeMillis() / 1000).toULong(),
        )

        CallManager.startOutgoingCall(
            ApplicationProvider.getApplicationContext(),
            outgoingPeer,
            "Outgoing",
            CallMediaKind.AUDIO,
        )
        val stateBeforeIncoming = CallManager.state.value
        val raisedNotification = CallManager.onIncomingSignal(incomingDto)
        val stateAfterIncoming = CallManager.state.value
        CallManager.hangup()

        assertTrue(stateBeforeIncoming is CallUiState.Ringing && !stateBeforeIncoming.incoming)
        assertTrue(
            "an incoming offer while already busy must not ask for a ringing notification",
            !raisedNotification,
        )
        assertTrue(
            "the outgoing call's own ringing state must survive an unrelated incoming offer",
            stateAfterIncoming is CallUiState.Ringing &&
                stateAfterIncoming.peer == outgoingPeer &&
                !stateAfterIncoming.incoming,
        )
        assertNotEquals(incomingDto.peer, (stateAfterIncoming as CallUiState.Ringing).peer)
    }
}
