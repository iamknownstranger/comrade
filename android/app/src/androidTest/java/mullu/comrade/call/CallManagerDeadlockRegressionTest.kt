package mullu.comrade.call

import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.webrtc.PeerConnection
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * Regression test for the signaling-thread ↔ monitor deadlock that froze
 * calls on "Connecting…" with an unresponsive End button.
 *
 * `org.webrtc` delivers [PeerConnection.Observer] callbacks on its signaling
 * thread — the same thread every blocking `PeerConnection` proxy call
 * (`addIceCandidate`, `signalingState`, `setConfiguration`, …) waits on. The
 * event pump makes exactly such calls *while holding the [CallManager]
 * monitor* (`onIncomingSignal` is `@Synchronized`), so an observer callback
 * that `synchronized`s on [CallManager] inline parks the signaling thread on
 * the monitor while the monitor holder parks on the signaling thread:
 * permanent deadlock, after which every `synchronized` transition — hangup,
 * reject, the armed timeouts — blocks forever.
 *
 * The fix routes all monitor-taking callback work through an ordered lane
 * (`CallManager.webRtcLane`) instead of blocking the delivering thread. This
 * test pins that invariant from the outside: with the monitor deliberately
 * held by one thread, observer callbacks delivered on another (standing in
 * for the signaling thread) must return promptly rather than queue up on the
 * lock. Before the fix, the FAILED/DISCONNECTED deliveries below blocked
 * until the holder released the monitor and this test timed out.
 */
@RunWith(AndroidJUnit4::class)
class CallManagerDeadlockRegressionTest {

    /** How long the holder thread keeps the monitor — far longer than the callbacks are allowed to take. */
    private val monitorHoldMs = 10_000L

    /** The prompt-return budget for all callback deliveries combined. */
    private val callbackBudgetMs = 2_000L

    @Test
    fun observer_callbacks_return_promptly_while_the_monitor_is_held() {
        val observer = CallManager.peerConnectionObserverForTest()

        val releaseMonitor = CountDownLatch(1)
        val monitorHeld = CountDownLatch(1)
        val holder = Thread {
            synchronized(CallManager) {
                monitorHeld.countDown()
                releaseMonitor.await(monitorHoldMs, TimeUnit.MILLISECONDS)
            }
        }
        holder.start()
        assertTrue("monitor holder never started", monitorHeld.await(5, TimeUnit.SECONDS))

        // Stand-in for the WebRTC signaling thread: deliver the exact state
        // changes seen in the field freeze (CONNECTING → FAILED while the
        // pump holds the monitor applying trickled ICE), plus CONNECTED and
        // DISCONNECTED for the remaining monitor-touching branches.
        val callbacksReturned = CountDownLatch(1)
        val signalingStandIn = Thread {
            observer.onConnectionChange(PeerConnection.PeerConnectionState.CONNECTING)
            observer.onConnectionChange(PeerConnection.PeerConnectionState.FAILED)
            observer.onConnectionChange(PeerConnection.PeerConnectionState.CONNECTED)
            observer.onConnectionChange(PeerConnection.PeerConnectionState.DISCONNECTED)
            callbacksReturned.countDown()
        }
        signalingStandIn.start()

        val returnedInTime = callbacksReturned.await(callbackBudgetMs, TimeUnit.MILLISECONDS)
        releaseMonitor.countDown()
        holder.join(monitorHoldMs)
        signalingStandIn.join(monitorHoldMs)

        assertTrue(
            "observer callbacks blocked on the held CallManager monitor — " +
                "on the real signaling thread this is the Connecting…-freeze deadlock",
            returnedInTime,
        )
    }
}
