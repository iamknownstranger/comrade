package mullu.comrade.voice

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pins [MicHolderSet]'s "last one out actually resumes" contract — a call and
 * a voice-note recording can overlap (press-hold the record button, then
 * place/answer a call before releasing it), and the bug this guards against
 * is whichever one finishes first prematurely resuming the wake-word
 * recogniser while the other still holds the mic.
 */
class WakeWordServiceTest {

    @Test
    fun `a single holder pauses on acquire and resumes on release`() {
        val holders = MicHolderSet()

        assertTrue("the first holder must trigger an actual pause", holders.acquire(MicHolder.CALL))
        assertTrue("the last holder must trigger an actual resume", holders.release(MicHolder.CALL))
    }

    @Test
    fun `the recogniser stays paused until the last overlapping holder releases`() {
        val holders = MicHolderSet()

        assertTrue(holders.acquire(MicHolder.CALL))
        assertFalse(
            "a second concurrent holder must not re-trigger pause — it's already paused",
            holders.acquire(MicHolder.VOICE_NOTE),
        )
        assertFalse(
            "releasing the first holder while the second still holds it must not resume",
            holders.release(MicHolder.CALL),
        )
        assertTrue(
            "releasing the second (and now last) holder must resume",
            holders.release(MicHolder.VOICE_NOTE),
        )
    }

    @Test
    fun `releasing a holder that never acquired is a harmless no-op`() {
        // The exact "call ends before its own setup ever paused" case — endWith
        // calls resume() unconditionally regardless of whether setupPeer ran.
        assertFalse(MicHolderSet().release(MicHolder.CALL))
    }

    @Test
    fun `a duplicate acquire by the same holder is idempotent`() {
        val holders = MicHolderSet()

        assertTrue(holders.acquire(MicHolder.CALL))
        assertFalse("re-acquiring while already held must not re-trigger pause", holders.acquire(MicHolder.CALL))
        assertTrue(holders.release(MicHolder.CALL))
    }
}
