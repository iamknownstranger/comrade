package global.auros.comrade

import org.json.JSONArray
import org.json.JSONObject
import org.json.JSONTokener

/**
 * JNI bridge to the Rust comrade_core library.
 *
 * The native library is compiled from crates/comrade_jni via cargo-ndk and
 * placed in app/src/main/jniLibs/<abi>/libcomrade_jni.so.
 */
object ComradeCore {

    init {
        System.loadLibrary("comrade_jni")
    }

    // ── Native declarations ──────────────────────────────────────────────────

    /** Library version string (e.g. "0.1.0"). */
    external fun getVersion(): String

    /**
     * Generate a new secp256k1 keypair.
     * @return JSON `{"npub":"npub1…","nsec":"nsec1…"}` or `{"error":"…"}`.
     */
    external fun generateKeypair(): String

    /**
     * Derive the npub Bech32 string from a raw nsec Bech32 string.
     * @return npub string, or `null` if the nsec is invalid.
     */
    external fun getNpubFromNsec(nsec: String): String?

    /**
     * Return the human-readable label for a workspace key.
     * @param workspace One of: "Base", "OffGridTravel", "CoupleSandboxSakha", "CoupleSandboxSakhi".
     * @return Label string, or `null` for unknown keys.
     */
    external fun workspaceLabel(workspace: String): String?

    /**
     * Return a JSON array of all workspace keys and labels.
     * ```json
     * [{"key":"Base","label":"…"}, …]
     * ```
     */
    external fun allWorkspaces(): String

    // ── IPC bridge: vault, timeline, broadcast, workspace, events ────────────

    /**
     * Unlock the encrypted vault at [path] with [passphrase] and start the
     * background relay/DM loops.
     * @return JSON `{"npub":"npub1…","has_secret":true}` or `{"error":"…"}`.
     */
    external fun unlockVault(path: String, passphrase: String): String

    /**
     * Broadcast a Chitthi to the public relays, optionally as a reply.
     * @param replyTo parent event id (hex), or empty/`""` for a top-level post.
     * @return JSON `{"event_id":"…"}` or `{"error":"…"}`.
     */
    external fun broadcastChitthi(content: String, replyTo: String): String

    /**
     * Load the Sabha timeline from the encrypted offline cache.
     * @return JSON array of Chitthis, or `{"error":"…"}`.
     */
    external fun fetchSabhaTimeline(): String

    /**
     * Toggle the active workspace, enforcing the transition state machine.
     * @return the new workspace JSON, or a typed `{"error":"…"}`.
     */
    external fun toggleWorkspace(target: String): String

    /**
     * Non-blocking drain of the next bridge event (incoming Chitthi / DM).
     * @return event JSON, `{"empty":true}` when idle, or `{"error":"…"}`.
     */
    external fun pollEvent(): String

    // ── Kotlin convenience wrappers ──────────────────────────────────────────

    data class Keypair(val npub: String, val nsec: String)

    /**
     * Generate a new keypair, returning a typed [Keypair] or throwing on error.
     */
    fun generateKeypairTyped(): Keypair {
        val json = JSONObject(generateKeypair())
        if (json.has("error")) error("Keypair generation failed: ${json.getString("error")}")
        return Keypair(
            npub = json.getString("npub"),
            nsec = json.getString("nsec"),
        )
    }

    data class WorkspaceInfo(val key: String, val label: String)

    /** All workspaces as a list of [WorkspaceInfo]. */
    fun workspaces(): List<WorkspaceInfo> {
        val arr = JSONArray(allWorkspaces())
        return (0 until arr.length()).map { i ->
            val obj = arr.getJSONObject(i)
            WorkspaceInfo(key = obj.getString("key"), label = obj.getString("label"))
        }
    }

    /**
     * Unlock the vault, returning the active identity npub or throwing the
     * backend's error message.
     */
    fun unlockVaultTyped(path: String, passphrase: String): String {
        val json = JSONObject(unlockVault(path, passphrase))
        if (json.has("error")) error("Vault unlock failed: ${json.getString("error")}")
        return json.getString("npub")
    }

    /** Broadcast a Chitthi, returning the new event id or throwing on error. */
    fun broadcastChitthiTyped(content: String, replyTo: String = ""): String {
        val json = JSONObject(broadcastChitthi(content, replyTo))
        if (json.has("error")) error("Broadcast failed: ${json.getString("error")}")
        return json.getString("event_id")
    }

    data class ChitthiInfo(
        val id: String,
        val author: String,
        val content: String,
        val createdAt: Long,
        val replyTo: String?,
    )

    /** The cached Sabha timeline as a list of [ChitthiInfo]. */
    fun sabhaTimeline(): List<ChitthiInfo> {
        val raw = fetchSabhaTimeline()
        val parsed = JSONTokener(raw).nextValue()
        if (parsed is JSONObject && parsed.has("error")) {
            error("Timeline fetch failed: ${parsed.getString("error")}")
        }
        val arr = parsed as JSONArray
        return (0 until arr.length()).map { i ->
            val obj = arr.getJSONObject(i)
            ChitthiInfo(
                id = obj.getString("id"),
                author = obj.getString("author"),
                content = obj.getString("content"),
                createdAt = obj.getLong("created_at"),
                replyTo = if (obj.isNull("reply_to")) null else obj.optString("reply_to"),
            )
        }
    }
}
