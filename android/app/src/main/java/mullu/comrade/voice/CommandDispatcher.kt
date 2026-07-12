package mullu.comrade.voice

/**
 * Backend surface the voice layer needs. Abstracted behind an interface so the
 * [CommandDispatcher] decision logic is unit-testable with a fake, without
 * loading the native `comrade_jni` library. [ComradeCoreBackend] is the real,
 * JNI-backed implementation.
 */
interface ComradeBackend {
    /** Broadcast a public Chitthi; returns the new event id. */
    fun post(text: String): Result<String>

    /** Save a private, local-only journal entry; returns its id. */
    fun journal(text: String): Result<String>

    /** Most recent cached Chitthi bodies, newest first. */
    fun timeline(): Result<List<String>>

    /** Switch workspace by `ComradeCore` key; returns its human label. */
    fun switchWorkspace(key: String): Result<String>

    /** Mint a fresh identity; returns the public `npub` (never the nsec). */
    fun generateIdentity(): Result<String>
}

/**
 * Turns a parsed [VoiceCommand] into a backend action plus the sentence the
 * assistant should speak back. Pure with respect to Android — the only
 * dependency is the injected [ComradeBackend].
 */
class CommandDispatcher(private val backend: ComradeBackend) {

    fun handle(command: VoiceCommand): String = when (command) {
        is VoiceCommand.Post -> backend.post(command.text).fold(
            onSuccess = { "Posted your Chitthi." },
            onFailure = { "I couldn't post that. ${it.message ?: "Unknown error"}." },
        )

        is VoiceCommand.Journal -> backend.journal(command.text).fold(
            onSuccess = { "Saved to your journal. It stays on this phone." },
            onFailure = { "I couldn't save that. ${it.message ?: "Unknown error"}." },
        )

        is VoiceCommand.ReadTimeline -> backend.timeline().fold(
            onSuccess = { bodies ->
                if (bodies.isEmpty()) {
                    "Your timeline is empty."
                } else {
                    val head = bodies.take(MAX_SPOKEN_ITEMS)
                    buildString {
                        append("Here are your latest ")
                        append(head.size)
                        append(if (head.size == 1) " Chitthi. " else " Chitthis. ")
                        head.forEachIndexed { i, body ->
                            append(i + 1); append(". "); append(body); append(". ")
                        }
                    }.trim()
                }
            },
            onFailure = { "I couldn't read your timeline. ${it.message ?: "Unknown error"}." },
        )

        is VoiceCommand.SwitchWorkspace -> backend.switchWorkspace(command.workspaceKey).fold(
            onSuccess = { label -> "Switched to $label." },
            onFailure = { "I couldn't switch workspace. ${it.message ?: "Unknown error"}." },
        )

        VoiceCommand.GenerateKeypair -> backend.generateIdentity().fold(
            onSuccess = { "Created a new identity. Your public key is on screen." },
            onFailure = { "I couldn't create an identity. ${it.message ?: "Unknown error"}." },
        )

        VoiceCommand.Help -> HELP_SENTENCE

        VoiceCommand.Empty ->
            "I'm listening. Try: journal, post, read my timeline, or switch workspace."

        is VoiceCommand.Unknown ->
            "Sorry, I can't do that yet. Say \"help\" to hear what I understand."
    }

    private companion object {
        const val MAX_SPOKEN_ITEMS = 3
        const val HELP_SENTENCE =
            "You can say: journal, followed by a private thought; post, followed " +
                "by your message; read my timeline; switch to off grid, base, " +
                "sakha or sakhi; or create a new identity."
    }
}
