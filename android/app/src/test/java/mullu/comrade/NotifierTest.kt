package mullu.comrade

import org.junit.Assert.assertNotEquals
import org.junit.Test

/**
 * Pins [Notifier.CHANNEL_CALLS]'s id bump (T3.8: single ring source). Channel
 * settings (including sound) are sticky once created — the OS never lets an
 * app change them after the fact — so silencing the channel only takes
 * effect for existing installs if the id itself changes; this guards against
 * a future edit accidentally reverting to the old, jingling id.
 */
class NotifierTest {

    @Test
    fun `calls channel id was bumped so existing installs pick up the silent channel`() {
        assertNotEquals("comrade_calls", Notifier.CHANNEL_CALLS)
    }
}
