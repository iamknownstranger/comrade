package global.auros.comrade

import org.json.JSONArray
import org.json.JSONObject

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
}
