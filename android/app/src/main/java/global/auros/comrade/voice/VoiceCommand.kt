package global.auros.comrade.voice

/**
 * Pure, Android-free parsing of a recognised utterance into a [VoiceCommand].
 *
 * Kept deliberately free of any Android or JNI dependency so the whole
 * command grammar is unit-testable on a plain JVM (see `VoiceCommandTest`).
 * The [WakeWordService] / assist session layers are the only things that
 * actually touch the microphone, TTS, or [global.auros.comrade.ComradeCore].
 */
sealed interface VoiceCommand {

    /** Broadcast a public Chitthi with [text] as its body. */
    data class Post(val text: String) : VoiceCommand

    /**
     * Write an anonymous companion entry — the "write down anything" path.
     * [mode] is a `CompanionMode` key ("journal"/"vent"/"brainstorm"/"reflect").
     */
    data class Journal(val mode: String, val text: String) : VoiceCommand

    /** Read the cached Sabha timeline aloud. */
    data object ReadTimeline : VoiceCommand

    /** Switch the active workspace to [workspaceKey] (a `ComradeCore` key). */
    data class SwitchWorkspace(val workspaceKey: String) : VoiceCommand

    /** Mint a fresh secp256k1 identity. */
    data object GenerateKeypair : VoiceCommand

    /** Enumerate what the assistant can do. */
    data object Help : VoiceCommand

    /** The wake word fired but nothing intelligible followed. */
    data object Empty : VoiceCommand

    /** Heard something, but it matched no known command. */
    data class Unknown(val transcript: String) : VoiceCommand

    companion object {
        /** The keyphrase the wake-word recogniser listens for. */
        const val WAKE_PHRASE: String = "hey comrade"

        private val POST_PREFIXES = listOf("post", "broadcast", "chitthi", "share", "say")
        private val TIMELINE_PHRASES = listOf(
            "read timeline", "read my timeline", "read feed", "read my feed",
            "show feed", "show timeline", "what's new", "whats new", "catch me up",
        )
        private val KEYGEN_PHRASES = listOf(
            "generate key", "generate keypair", "new key", "new keypair",
            "new identity", "create identity",
        )
        private val HELP_PHRASES = listOf("help", "what can you do", "what can i say", "commands")

        // Companion journaling verbs → CompanionMode key, longest phrase first so
        // "write down" wins over any shorter overlap.
        private val JOURNAL_PREFIXES: List<Pair<String, String>> = listOf(
            "write down" to "journal",
            "journal" to "journal",
            "diary" to "journal",
            "note" to "journal",
            "brainstorm" to "brainstorm",
            "reflect on" to "reflect",
            "reflect" to "reflect",
            "vent" to "vent",
            "unload" to "vent",
        )

        // Spoken workspace names → ComradeCore workspace keys, longest phrase first
        // so "couple sakhi" wins over the bare "couple" prefix.
        private val WORKSPACE_ALIASES: List<Pair<String, String>> = listOf(
            "off grid travel" to "OffGridTravel",
            "off-grid travel" to "OffGridTravel",
            "off grid" to "OffGridTravel",
            "off-grid" to "OffGridTravel",
            "travel" to "OffGridTravel",
            "couple sakhi" to "CoupleSandboxSakhi",
            "sakhi" to "CoupleSandboxSakhi",
            "couple sakha" to "CoupleSandboxSakha",
            "sakha" to "CoupleSandboxSakha",
            "couple" to "CoupleSandboxSakha",
            "base" to "Base",
            "home" to "Base",
        )

        /**
         * Normalise a raw recogniser transcript: lowercase, collapse whitespace,
         * strip surrounding punctuation, and drop a leading "hey comrade" (the
         * wake phrase is sometimes captured together with the command).
         */
        fun normalise(raw: String): String {
            var text = raw.lowercase().trim()
            // Vosk emits bare words, but tap-to-talk / assist recognisers can add
            // punctuation — strip anything that isn't a letter, digit, or space.
            text = text.replace(Regex("[^\\p{L}\\p{N}\\s']"), " ")
            text = text.replace(Regex("\\s+"), " ").trim()
            if (text.startsWith(WAKE_PHRASE)) {
                text = text.removePrefix(WAKE_PHRASE).trim()
            } else if (text.startsWith("comrade")) {
                text = text.removePrefix("comrade").trim()
            }
            return text
        }

        /** Parse a recognised utterance (with or without the wake phrase). */
        fun parse(raw: String): VoiceCommand {
            val text = normalise(raw)
            if (text.isEmpty()) return Empty

            for (phrase in HELP_PHRASES) {
                if (text == phrase) return Help
            }
            for (phrase in TIMELINE_PHRASES) {
                if (text == phrase) return ReadTimeline
            }
            for (phrase in KEYGEN_PHRASES) {
                if (text == phrase) return GenerateKeypair
            }

            parseJournal(text)?.let { return it }

            parseSwitch(text)?.let { return it }

            for (prefix in POST_PREFIXES) {
                if (text == prefix) return Empty // "post" with no body
                val withSpace = "$prefix "
                if (text.startsWith(withSpace)) {
                    val body = text.removePrefix(withSpace).trim()
                    return if (body.isEmpty()) Empty else Post(body)
                }
            }

            return Unknown(text)
        }

        private fun parseJournal(text: String): VoiceCommand? {
            for ((prefix, mode) in JOURNAL_PREFIXES) {
                if (text == prefix) return Empty // verb with no body — needs content
                if (text.startsWith("$prefix ")) {
                    val body = text.removePrefix("$prefix ").trim()
                    return if (body.isEmpty()) Empty else Journal(mode, body)
                }
            }
            return null
        }

        private fun parseSwitch(text: String): VoiceCommand? {
            val switchPrefixes = listOf("switch to", "switch", "go to", "open", "workspace")
            var remainder: String? = null
            for (prefix in switchPrefixes) {
                if (text == prefix) return null
                if (text.startsWith("$prefix ")) {
                    remainder = text.removePrefix("$prefix ").trim()
                    break
                }
            }
            // Also accept a bare "go off grid" style phrasing.
            val target = remainder ?: text
            for ((alias, key) in WORKSPACE_ALIASES) {
                if (target == alias || target == "the $alias" ||
                    (remainder != null && target.startsWith(alias))
                ) {
                    return SwitchWorkspace(key)
                }
            }
            return if (remainder != null) Unknown(text) else null
        }
    }
}
