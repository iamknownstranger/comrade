package mullu.comrade.call

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
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
}
