package mullu.comrade.voice

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class VoiceCommandTest {

    @Test
    fun `post prefix captures the body`() {
        assertEquals(
            VoiceCommand.Post("hello world"),
            VoiceCommand.parse("post hello world"),
        )
        assertEquals(
            VoiceCommand.Post("gm from the mesh"),
            VoiceCommand.parse("broadcast gm from the mesh"),
        )
    }

    @Test
    fun `wake phrase is stripped before parsing`() {
        assertEquals(
            VoiceCommand.Post("running late"),
            VoiceCommand.parse("hey comrade post running late"),
        )
        assertEquals(
            VoiceCommand.ReadTimeline,
            VoiceCommand.parse("Hey Comrade, read my feed"),
        )
    }

    @Test
    fun `timeline synonyms all resolve`() {
        for (phrase in listOf("read timeline", "show feed", "what's new", "catch me up")) {
            assertEquals(
                "phrase='$phrase'",
                VoiceCommand.ReadTimeline,
                VoiceCommand.parse(phrase),
            )
        }
    }

    @Test
    fun `workspace aliases map to core keys`() {
        assertEquals(
            VoiceCommand.SwitchWorkspace("OffGridTravel"),
            VoiceCommand.parse("switch to off grid"),
        )
        assertEquals(
            VoiceCommand.SwitchWorkspace("OffGridTravel"),
            VoiceCommand.parse("go to travel"),
        )
        assertEquals(
            VoiceCommand.SwitchWorkspace("CoupleSandboxSakhi"),
            VoiceCommand.parse("switch to sakhi"),
        )
        assertEquals(
            VoiceCommand.SwitchWorkspace("Base"),
            VoiceCommand.parse("open home"),
        )
    }

    @Test
    fun `keygen and help phrases resolve`() {
        assertEquals(VoiceCommand.GenerateKeypair, VoiceCommand.parse("new identity"))
        assertEquals(VoiceCommand.Help, VoiceCommand.parse("what can you do"))
    }

    @Test
    fun `empty or wake-only utterance is Empty`() {
        assertEquals(VoiceCommand.Empty, VoiceCommand.parse("hey comrade"))
        assertEquals(VoiceCommand.Empty, VoiceCommand.parse(""))
        assertEquals(VoiceCommand.Empty, VoiceCommand.parse("post"))
    }

    @Test
    fun `unrecognised speech becomes Unknown with normalised text`() {
        val result = VoiceCommand.parse("please order me a pizza")
        assertTrue(result is VoiceCommand.Unknown)
        assertEquals("please order me a pizza", (result as VoiceCommand.Unknown).transcript)
    }

    @Test
    fun `punctuation and casing are normalised away`() {
        assertEquals("post hello", VoiceCommand.normalise("  Post, HELLO!! "))
    }
}
