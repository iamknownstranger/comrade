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
     * Non-blocking drain of the next bridge event (incoming Chitthi / DM /
     * call). Prefer [pollEvents] — one event per JNI crossing cannot keep up
     * with the public feed.
     * @return event JSON, `{"empty":true}` when idle, `{"lagged":n}` when the
     * consumer fell `n` events behind (recover from the encrypted caches:
     * timeline, DM history, call log), `{"closed":true}` when the bus is
     * gone, or `{"error":"…"}`.
     */
    external fun pollEvent(): String

    /**
     * Non-blocking batch drain of up to [max] bridge events in one crossing.
     * @return JSON `{"events":[…]}` (possibly empty), plus `"lagged":n` when
     * events were evicted before this poll, or `{"closed":true}`.
     */
    external fun pollEvents(max: Int): String

    // ── Pukar: audio/video calls (WebRTC signaling over encrypted Nostr) ─────
    //
    // The Rust core owns signaling + call state; this layer supplies SDP/ICE
    // strings from org.webrtc's PeerConnection and reacts to call events
    // drained via [pollEvent] (`{"type":"call","call":{...}}`).

    /**
     * Start ringing `peer` (npub or hex). @return the session JSON
     * (`call_id`, `state`:"ringing", …) or `{"error":"…"}`.
     */
    external fun placeCall(peer: String, video: Boolean, sdpOffer: String): String

    /** Accept the ringing incoming call. @return `{"ok":true}` or an error. */
    external fun answerCall(callId: String, sdpAnswer: String): String

    /** Decline the ringing incoming call. */
    external fun declineCall(callId: String): String

    /** Hang up the active call, or cancel an outgoing ring. */
    external fun endCall(callId: String): String

    /**
     * Forward a locally-gathered ICE candidate to the peer.
     * @param sdpMid pass `""` when the candidate has none.
     * @param sdpMlineIndex pass `-1` when the candidate has none.
     */
    external fun sendCallIce(
        callId: String,
        candidate: String,
        sdpMid: String,
        sdpMlineIndex: Int,
    ): String

    /** Report that WebRTC media is flowing (ICE completed). */
    external fun callConnected(callId: String): String

    /** The live call JSON, or `{"none":true}` when idle. */
    external fun activeCall(): String

    /** Ended calls (newest first) as a JSON array — the call log. */
    external fun fetchCallLog(): String

    // ── Companion: private, anonymous journal ────────────────────────────────

    /**
     * Write an anonymous companion entry into the encrypted store.
     * @param mode one of "journal", "vent", "brainstorm", "reflect".
     * @param voice `true` if this came from a voice recording (transcribed).
     * @param body the entry text (may be empty for a mood-only check-in).
     * @param mood -2..2, or [NO_MOOD] for no rating.
     * @return JSON `{entry, safety, prompt}` or `{"error":"…"}`.
     */
    external fun journalEntry(mode: String, voice: Boolean, body: String, mood: Int): String

    /** The private journal, newest first, as a JSON array (or `{"error":…}`). */
    external fun fetchJournal(): String

    /**
     * On-device journaling insights as JSON (or `{"error":…}`).
     * @param tzOffsetSecs device offset from UTC in seconds — pass
     * `TimeZone.getDefault().getOffset(System.currentTimeMillis()) / 1000`
     * so streaks roll at the user's midnight rather than UTC's.
     */
    external fun journalInsights(tzOffsetSecs: Int): String

    /** A supportive prompt for `mode`: JSON `{"prompt":"…"}` or `{"error":…}`. */
    external fun companionPrompt(mode: String): String

    // ── Kotlin convenience wrappers ──────────────────────────────────────────

    /** Sentinel meaning "no mood recorded" — mirrors `NO_MOOD` on the Rust side. */
    const val NO_MOOD: Int = Int.MIN_VALUE

    /**
     * The outcome of writing a companion entry: the mode it was filed under, a
     * fresh supportive prompt, and whether the text tripped the offline crisis
     * safety scan (so the UI can gently surface helpline resources).
     */
    data class CompanionOutcome(
        val mode: String,
        val prompt: String,
        val concerning: Boolean,
    )

    /**
     * Write an anonymous journal entry and return a typed [CompanionOutcome],
     * throwing on a backend error.
     */
    fun writeJournal(
        mode: String,
        voice: Boolean,
        body: String,
        mood: Int = NO_MOOD,
    ): CompanionOutcome {
        val json = JSONObject(journalEntry(mode, voice, body, mood))
        if (json.has("error")) error("Journal write failed: ${json.getString("error")}")
        val safety = json.optJSONObject("safety")
        return CompanionOutcome(
            mode = json.optJSONObject("entry")?.optString("mode") ?: mode,
            prompt = json.optString("prompt"),
            concerning = safety?.optBoolean("concerning") ?: false,
        )
    }

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
