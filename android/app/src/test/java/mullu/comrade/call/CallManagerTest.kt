package mullu.comrade.call

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.webrtc.PeerConnection
import uniffi.comrade_core.CallSignal

/**
 * Pins the two wire-boundary conversions that used to crash or hang a call
 * (see CallManager.remoteIceCandidate/outgoingSdpMLineIndex): a missing
 * remote `sdpMid` must not reach the native `IceCandidate` constructor as
 * `null`, and a local candidate's `sdpMLineIndex == -1` must not wrap to
 * `65535` when narrowed to `UShort`.
 *
 * These two are the only pieces of [CallManager] pure enough to run as a
 * plain JVM unit test — everything else drives a real `org.webrtc.
 * PeerConnectionFactory` and crosses the real JNI boundary into the Rust
 * core, neither loadable here. The call-setup cancellation/lifecycle tests
 * (AUDIT.md COMMS-05: cancel-before-session, place failure, late callback
 * after cancellation, simultaneous offers) live in
 * `androidTest/.../call/CallManagerLifecycleTest.kt` instead, where the real
 * device runtime makes them meaningful.
 */
class CallManagerTest {

    @Test
    fun `remote ICE candidate maps a missing sdpMid to empty string, not null`() {
        val ice = CallSignal.Ice(
            candidate = "candidate:1 1 UDP 2130706431 192.0.2.1 54321 typ host",
            sdpMid = null,
            sdpMLineIndex = null,
        )

        val candidate = CallManager.remoteIceCandidate(ice)

        assertEquals("", candidate.sdpMid)
        assertEquals(0, candidate.sdpMLineIndex)
        assertEquals(ice.candidate, candidate.sdp)
    }

    @Test
    fun `remote ICE candidate preserves a present sdpMid and index`() {
        val ice = CallSignal.Ice(candidate = "cand", sdpMid = "0", sdpMLineIndex = 2.toUShort())

        val candidate = CallManager.remoteIceCandidate(ice)

        assertEquals("0", candidate.sdpMid)
        assertEquals(2, candidate.sdpMLineIndex)
    }

    @Test
    fun `outgoing sdpMLineIndex maps a negative index to null instead of overflowing`() {
        // A naive `(-1).toUShort()` wraps to 65535, which the remote peer
        // then rejects as a malformed line index — this is the exact bug
        // that left calls stuck on "Connecting...".
        assertNull(CallManager.outgoingSdpMLineIndex(-1))
    }

    @Test
    fun `outgoing sdpMLineIndex preserves non-negative values`() {
        assertEquals(0.toUShort(), CallManager.outgoingSdpMLineIndex(0))
        assertEquals(3.toUShort(), CallManager.outgoingSdpMLineIndex(3))
    }

    // ── T1: idempotent call signaling — pure decision functions ───────────────
    //
    // These, plus the glare/recovery decisions below, are exactly the "extract
    // the guard decisions into pure functions" ask: no live PeerConnection, no
    // Session, no synchronized state — just the enum/value inputs each
    // decision actually branches on.

    @Test
    fun `decideAnswer applies only in HAVE_LOCAL_OFFER — a fresh answer rings through`() {
        assertEquals(
            CallManager.AnswerDecision.APPLY,
            CallManager.decideAnswer(PeerConnection.SignalingState.HAVE_LOCAL_OFFER),
        )
    }

    @Test
    fun `decideAnswer ignores a duplicate answer once the pc has moved past HAVE_LOCAL_OFFER`() {
        // STABLE is exactly the state a pc settles into right after the first
        // Answer applies — a redelivered second Answer must not re-apply and
        // tear the live call down.
        assertEquals(
            CallManager.AnswerDecision.IGNORE,
            CallManager.decideAnswer(PeerConnection.SignalingState.STABLE),
        )
    }

    @Test
    fun `decideAnswer ignores a null signaling state (no pc yet)`() {
        assertEquals(CallManager.AnswerDecision.IGNORE, CallManager.decideAnswer(null))
    }

    @Test
    fun `decideOfferForExistingSession renegotiates the same call once a pc exists`() {
        assertEquals(
            CallManager.OfferDecision.RENEGOTIATE,
            CallManager.decideOfferForExistingSession("call-1", "call-1", existingHasPc = true),
        )
    }

    @Test
    fun `decideOfferForExistingSession no-ops a same-call duplicate offer received pre-accept`() {
        assertEquals(
            CallManager.OfferDecision.DUPLICATE_NOOP,
            CallManager.decideOfferForExistingSession("call-1", "call-1", existingHasPc = false),
        )
    }

    @Test
    fun `decideOfferForExistingSession treats a different call id as busy regardless of pc state`() {
        assertEquals(
            CallManager.OfferDecision.BUSY,
            CallManager.decideOfferForExistingSession("call-2", "call-1", existingHasPc = true),
        )
        assertEquals(
            CallManager.OfferDecision.BUSY,
            CallManager.decideOfferForExistingSession("call-2", "call-1", existingHasPc = false),
        )
    }

    @Test
    fun `isOfferForEndedCall drops an ended-id offer and rings a fresh one`() {
        val ended = listOf("call-1", "call-2")
        assertTrue(CallManager.isOfferForEndedCall("call-1", ended))
        assertFalse(
            "a callId not in the ended set must still ring",
            CallManager.isOfferForEndedCall("call-3", ended),
        )
        assertFalse(CallManager.isOfferForEndedCall("call-1", emptyList()))
    }

    // ── T3: glare tiebreak + connection-recovery decisions ────────────────────

    @Test
    fun `decideGlare picks the lexicographically smaller npub as caller, both directions`() {
        val lower = "npub1aaaa"
        val higher = "npub1zzzz"
        assertEquals(
            "the lower npub keeps its outgoing call",
            CallManager.GlareDecision.WE_WIN_KEEP_OUTGOING,
            CallManager.decideGlare(lower, higher),
        )
        assertEquals(
            "the higher npub yields to the incoming offer",
            CallManager.GlareDecision.WE_LOSE_TAKE_INCOMING,
            CallManager.decideGlare(higher, lower),
        )
    }

    @Test
    fun `decideConnectionStateAction arms immediate recovery only for a previously-connected FAILED`() {
        assertEquals(
            CallManager.ConnectionStateAction.RECOVER_NOW,
            CallManager.decideConnectionStateAction(
                PeerConnection.PeerConnectionState.FAILED,
                hasConnectedBefore = true,
            ),
        )
        assertEquals(
            "a pre-connect FAILED keeps using the caller's existing TURN fallback",
            CallManager.ConnectionStateAction.TRY_TURN_FALLBACK,
            CallManager.decideConnectionStateAction(
                PeerConnection.PeerConnectionState.FAILED,
                hasConnectedBefore = false,
            ),
        )
    }

    @Test
    fun `decideConnectionStateAction only starts the disconnect grace after having connected`() {
        assertEquals(
            CallManager.ConnectionStateAction.RECOVER_AFTER_GRACE,
            CallManager.decideConnectionStateAction(
                PeerConnection.PeerConnectionState.DISCONNECTED,
                hasConnectedBefore = true,
            ),
        )
        assertEquals(
            "a pre-connect DISCONNECTED is not a failure worth acting on",
            CallManager.ConnectionStateAction.NONE,
            CallManager.decideConnectionStateAction(
                PeerConnection.PeerConnectionState.DISCONNECTED,
                hasConnectedBefore = false,
            ),
        )
    }

    // ── T3.10: busy-reject call-history timestamp ─────────────────────────────

    @Test
    fun `busy-reject timestamp is never the epoch`() {
        // Regression guard: a hard-coded startedAt = 0 used to render as
        // 1 Jan 1970 in CallHistoryScreen.
        assertTrue(CallManager.nowEpochSecs() > 0)
    }

    @Test
    fun `decideConnectionStateAction is a no-op for every other connection state`() {
        assertEquals(
            CallManager.ConnectionStateAction.NONE,
            CallManager.decideConnectionStateAction(
                PeerConnection.PeerConnectionState.CONNECTING,
                hasConnectedBefore = true,
            ),
        )
        assertEquals(
            CallManager.ConnectionStateAction.NONE,
            CallManager.decideConnectionStateAction(
                PeerConnection.PeerConnectionState.CLOSED,
                hasConnectedBefore = false,
            ),
        )
    }
}
