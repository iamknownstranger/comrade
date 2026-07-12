package mullu.comrade.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pins the chat-title precedence: user alias → published @username → key.
 * The alias is the only name the user chose themself; the username is a
 * self-declared claim by the peer; the key is the identity.
 */
class DisplayNameTest {

    private val key = "npub1w4laefqx0av9y8gm7vk2xspwjnvyxydr0hjfpnr4x9dvw2l3jd2qtqy3gq"

    @Test
    fun aliasOutranksPublishedUsername() {
        assertEquals("Mom", peerTitle(key, "Mom", "charlie"))
    }

    @Test
    fun publishedUsernameOutranksKey() {
        assertEquals("@charlie", peerTitle(key, null, "charlie"))
        assertEquals("@charlie", peerTitle(key, "   ", "charlie"))
        // A handle already carrying '@' is not doubled.
        assertEquals("@charlie", peerTitle(key, null, "@charlie"))
    }

    @Test
    fun keyIsTheFallback() {
        assertEquals(shortNpub(key), peerTitle(key, null, null))
        assertEquals(shortNpub(key), peerTitle(key, "", " "))
    }

    @Test
    fun shortNpubKeepsHeadAndTail() {
        assertEquals("npub1w4lae…y3gq", shortNpub(key))
        assertEquals("short", shortNpub("short"))
    }

    @Test
    fun avatarColorIsStableAndInBounds() {
        val first = avatarColorIndex(key, 8)
        assertEquals("same key → same colour", first, avatarColorIndex(key, 8))
        assertTrue(first in 0 until 8)
        assertEquals("degenerate palette is safe", 0, avatarColorIndex(key, 0))
    }
}
