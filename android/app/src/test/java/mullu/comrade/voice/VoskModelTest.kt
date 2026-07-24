package mullu.comrade.voice

import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pins [ModelRefCount]'s contract — the bookkeeping that decides when the
 * shared Vosk model (tens of MB of RAM) may actually be closed. The bugs
 * this guards against: closing the model while a recogniser still uses it
 * (wake word active + a dictation overlapping), and a delayed idle-close
 * firing after the user already tapped a voice button again.
 */
class VoskModelTest {

    @Test
    fun `only the last release yields a close token`() {
        val refs = ModelRefCount()

        refs.acquire() // wake-word service
        refs.acquire() // an overlapping one-shot dictation

        assertNull("a release with another holder left must not offer a close", refs.release())
        val token = refs.release()
        assertNotNull("the final release must offer a close", token)
        assertTrue(refs.isIdleAt(token!!))
    }

    @Test
    fun `re-acquiring during the linger invalidates the pending close`() {
        val refs = ModelRefCount()
        refs.acquire()
        val token = refs.release()!!

        refs.acquire() // user taps a voice button again before the linger elapses

        assertFalse("the stale close must not fire under the new holder", refs.isIdleAt(token))
    }

    @Test
    fun `each idle period gets its own close token`() {
        val refs = ModelRefCount()
        refs.acquire()
        val first = refs.release()!!

        refs.acquire()
        val second = refs.release()!!

        assertFalse("an older idle period's close must stay dead", refs.isIdleAt(first))
        assertTrue("the newest idle period's close must be live", refs.isIdleAt(second))
    }

    @Test
    fun `a stray release without a holder is a harmless no-op`() {
        val refs = ModelRefCount()

        assertNull(refs.release())

        refs.acquire()
        val token = refs.release()!!
        assertNull("a double release must not mint another close", refs.release())
        assertTrue("…nor disturb the pending one", refs.isIdleAt(token))
    }
}
