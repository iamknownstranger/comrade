package mullu.comrade

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

    // ── Chat, profile & contacts (Telegram-like flow) ────────────────────────

    /**
     * Send an E2E-encrypted DM to [target] (npub or hex pubkey); the message is
     * also persisted to the offline history.
     * @return stored message JSON, or `{"error":"…"}`.
     */
    external fun sendDm(target: String, content: String): String

    /**
     * Claim a display @handle for this identity (persisted locally, published
     * to relays best-effort).
     * @return profile JSON `{"npub":…,"username":…}` or `{"error":"…"}`.
     */
    external fun setUsername(name: String): String

    /** The local profile JSON `{"npub":…,"username":…}` or `{"error":"…"}`. */
    external fun currentProfile(): String

    /**
     * Best-effort people search by handle over NIP-50-capable relays.
     * @return JSON array of `{"npub","name","about"}` (empty = nothing found).
     */
    external fun searchProfiles(query: String): String

    /** Save (or re-alias) a contact pinned by npub. Returns the contact JSON. */
    external fun addContact(npub: String, alias: String): String

    /** All saved contacts as a JSON array of `{"npub","alias"}`. */
    external fun listContacts(): String

    /** The chat list (one entry per peer, newest first) as a JSON array. */
    external fun listConversations(): String

    /** Offline message history with [peer], oldest first, as a JSON array. */
    external fun messagesWith(peer: String): String

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

    data class Profile(val npub: String, val username: String?)

    data class FoundProfile(val npub: String, val name: String?, val about: String?)

    data class ContactInfo(val npub: String, val alias: String)

    data class ConversationInfo(
        val peer: String,
        val alias: String?,
        val lastMessage: String,
        val lastAt: Long,
        val lastOutgoing: Boolean,
    )

    data class MessageInfo(
        val id: String,
        val peer: String,
        val content: String,
        val createdAt: Long,
        val outgoing: Boolean,
    )

    private fun JSONObject.failOnError(what: String): JSONObject {
        if (has("error")) error("$what failed: ${getString("error")}")
        return this
    }

    private fun JSONObject.optNullableString(key: String): String? =
        if (isNull(key)) null else optString(key)

    /** Send a DM, returning the stored message or throwing the backend error. */
    fun sendDmTyped(target: String, content: String): MessageInfo =
        JSONObject(sendDm(target, content)).failOnError("Send").toMessage()

    /** Claim a @handle, returning the updated profile or throwing. */
    fun setUsernameTyped(name: String): Profile =
        JSONObject(setUsername(name)).failOnError("Username").toProfile()

    /** The local profile, or throwing while the vault is still locked. */
    fun currentProfileTyped(): Profile =
        JSONObject(currentProfile()).failOnError("Profile").toProfile()

    /** Best-effort people search; an empty list is a normal outcome. */
    fun searchProfilesTyped(query: String): List<FoundProfile> {
        val parsed = JSONTokener(searchProfiles(query)).nextValue()
        if (parsed is JSONObject) parsed.failOnError("Search")
        val arr = parsed as JSONArray
        return (0 until arr.length()).map { i ->
            val o = arr.getJSONObject(i)
            FoundProfile(
                npub = o.getString("npub"),
                name = o.optNullableString("name"),
                about = o.optNullableString("about"),
            )
        }
    }

    /** Pin a contact by npub, returning the saved entry or throwing. */
    fun addContactTyped(npub: String, alias: String): ContactInfo {
        val o = JSONObject(addContact(npub, alias)).failOnError("Add contact")
        return ContactInfo(npub = o.getString("npub"), alias = o.getString("alias"))
    }

    /** All saved contacts. */
    fun contacts(): List<ContactInfo> {
        val parsed = JSONTokener(listContacts()).nextValue()
        if (parsed is JSONObject) parsed.failOnError("Contacts")
        val arr = parsed as JSONArray
        return (0 until arr.length()).map { i ->
            val o = arr.getJSONObject(i)
            ContactInfo(npub = o.getString("npub"), alias = o.getString("alias"))
        }
    }

    /** The chat list, newest thread first. */
    fun conversations(): List<ConversationInfo> {
        val parsed = JSONTokener(listConversations()).nextValue()
        if (parsed is JSONObject) parsed.failOnError("Conversations")
        val arr = parsed as JSONArray
        return (0 until arr.length()).map { i ->
            val o = arr.getJSONObject(i)
            ConversationInfo(
                peer = o.getString("peer"),
                alias = o.optNullableString("alias"),
                lastMessage = o.getString("last_message"),
                lastAt = o.getLong("last_at"),
                lastOutgoing = o.getBoolean("last_outgoing"),
            )
        }
    }

    /** Message history with [peer], oldest first. */
    fun messages(peer: String): List<MessageInfo> {
        val parsed = JSONTokener(messagesWith(peer)).nextValue()
        if (parsed is JSONObject) parsed.failOnError("Messages")
        val arr = parsed as JSONArray
        return (0 until arr.length()).map { i -> arr.getJSONObject(i).toMessage() }
    }

    private fun JSONObject.toProfile() = Profile(
        npub = getString("npub"),
        username = optNullableString("username"),
    )

    private fun JSONObject.toMessage() = MessageInfo(
        id = getString("id"),
        peer = getString("peer"),
        content = getString("content"),
        createdAt = getLong("created_at"),
        outgoing = getBoolean("outgoing"),
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
