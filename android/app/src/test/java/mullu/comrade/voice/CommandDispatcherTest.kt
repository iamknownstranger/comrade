package mullu.comrade.voice

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class CommandDispatcherTest {

    private class FakeBackend(
        var postResult: Result<String> = Result.success("evt123"),
        var journalResult: Result<String> = Result.success("entry123"),
        var timelineResult: Result<List<String>> = Result.success(emptyList()),
        var switchResult: Result<String> = Result.success("Off-Grid Travel"),
        var identityResult: Result<String> = Result.success("npub1abc"),
    ) : ComradeBackend {
        var lastPost: String? = null
        var lastJournal: String? = null
        var lastSwitchKey: String? = null
        override fun post(text: String): Result<String> { lastPost = text; return postResult }
        override fun journal(text: String): Result<String> {
            lastJournal = text; return journalResult
        }
        override fun timeline(): Result<List<String>> = timelineResult
        override fun switchWorkspace(key: String): Result<String> {
            lastSwitchKey = key; return switchResult
        }
        override fun generateIdentity(): Result<String> = identityResult
    }

    @Test
    fun `post forwards body to backend and confirms`() {
        val backend = FakeBackend()
        val reply = CommandDispatcher(backend).handle(VoiceCommand.Post("gm"))
        assertEquals("gm", backend.lastPost)
        assertTrue(reply.contains("Posted", ignoreCase = true))
    }

    @Test
    fun `post failure is spoken back with the error`() {
        val backend = FakeBackend(postResult = Result.failure(RuntimeException("relay down")))
        val reply = CommandDispatcher(backend).handle(VoiceCommand.Post("gm"))
        assertTrue(reply.contains("couldn't post", ignoreCase = true))
        assertTrue(reply.contains("relay down"))
    }

    @Test
    fun `journal saves privately and never posts`() {
        val backend = FakeBackend()
        val reply = CommandDispatcher(backend).handle(VoiceCommand.Journal("felt anxious"))
        assertEquals("felt anxious", backend.lastJournal)
        assertEquals("a journal entry must never hit the public feed", null, backend.lastPost)
        assertTrue(reply.contains("journal", ignoreCase = true))
        assertTrue(reply.contains("this phone", ignoreCase = true))
    }

    @Test
    fun `journal failure is spoken back`() {
        val backend = FakeBackend(journalResult = Result.failure(RuntimeException("vault locked")))
        val reply = CommandDispatcher(backend).handle(VoiceCommand.Journal("hello"))
        assertTrue(reply.contains("couldn't save", ignoreCase = true))
        assertTrue(reply.contains("vault locked"))
    }

    @Test
    fun `empty timeline is reported`() {
        val reply = CommandDispatcher(FakeBackend()).handle(VoiceCommand.ReadTimeline)
        assertTrue(reply.contains("empty", ignoreCase = true))
    }

    @Test
    fun `timeline reads at most three items`() {
        val backend = FakeBackend(
            timelineResult = Result.success(listOf("one", "two", "three", "four", "five")),
        )
        val reply = CommandDispatcher(backend).handle(VoiceCommand.ReadTimeline)
        assertTrue(reply.contains("one"))
        assertTrue(reply.contains("three"))
        assertTrue("should cap at three spoken items", !reply.contains("four"))
    }

    @Test
    fun `switch forwards the core key`() {
        val backend = FakeBackend()
        val reply = CommandDispatcher(backend)
            .handle(VoiceCommand.SwitchWorkspace("OffGridTravel"))
        assertEquals("OffGridTravel", backend.lastSwitchKey)
        assertTrue(reply.contains("Off-Grid Travel"))
    }

    @Test
    fun `identity response never leaks the secret key`() {
        val backend = FakeBackend(identityResult = Result.success("npub1public"))
        val reply = CommandDispatcher(backend).handle(VoiceCommand.GenerateKeypair)
        assertTrue(!reply.contains("nsec"))
        assertTrue(!reply.contains("npub1public")) // spoken responses stay off the key material
    }

    @Test
    fun `unknown command points at help`() {
        val reply = CommandDispatcher(FakeBackend()).handle(VoiceCommand.Unknown("order pizza"))
        assertTrue(reply.contains("help", ignoreCase = true))
    }
}
