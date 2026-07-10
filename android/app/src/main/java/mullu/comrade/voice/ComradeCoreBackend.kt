package mullu.comrade.voice

import mullu.comrade.ComradeCore
import org.json.JSONObject

/**
 * Real [ComradeBackend] backed by the native `comrade_jni` library through
 * [ComradeCore]. Every call is wrapped in [runCatching] so a JNI-boundary error
 * (returned as `{"error":…}` and re-thrown by the typed wrappers) surfaces as a
 * [Result.failure] the [CommandDispatcher] can speak back, never a crash.
 */
class ComradeCoreBackend : ComradeBackend {

    override fun post(text: String): Result<String> =
        runCatching { ComradeCore.broadcastChitthiTyped(text) }

    override fun timeline(): Result<List<String>> =
        runCatching { ComradeCore.sabhaTimeline().map { it.content } }

    override fun switchWorkspace(key: String): Result<String> = runCatching {
        val json = JSONObject(ComradeCore.toggleWorkspace(key))
        if (json.has("error")) error(json.getString("error"))
        // The toggle payload shape is an implementation detail; resolve the
        // human label from the stable workspaceLabel lookup instead.
        ComradeCore.workspaceLabel(key) ?: key
    }

    override fun generateIdentity(): Result<String> =
        runCatching { ComradeCore.generateKeypairTyped().npub }
}
