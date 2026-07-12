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
     * Entering "OffGridTravel" really starts the Saathi mDNS mesh engine.
     * @return the new workspace JSON, or a typed `{"error":"…"}`.
     */
    external fun toggleWorkspace(target: String): String

    /**
     * Snapshot of the off-grid mesh's live status: whether it's running, and
     * how many peers are currently reachable via mDNS. Has no error case.
     * @return JSON `{"active":bool,"peer_count":n}`.
     */
    external fun meshStatus(): String

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

    /**
     * Save a contact pinned by npub (trust-on-first-use). An empty alias keeps
     * any alias already set. Returns the contact JSON.
     */
    external fun addContact(npub: String, alias: String): String

    /**
     * Set (non-empty) or clear (empty) the user-chosen alias for a contact.
     * Returns the contact JSON.
     */
    external fun setContactAlias(npub: String, alias: String): String

    /**
     * Remove a saved contact (message history stays).
     * @return JSON `{"removed":true|false}` or `{"error":"…"}`.
     */
    external fun removeContact(npub: String): String

    /**
     * Refresh the cached @handles of conversation peers and contacts from the
     * relays (bounded, TTL-gated).
     * @return JSON `{"changed":n}` — reload the chat list when n > 0.
     */
    external fun refreshPeerProfiles(): String

    /** All saved contacts as a JSON array of `{"npub","alias","name"}`. */
    external fun listContacts(): String

    /** The chat list (one entry per peer, newest first) as a JSON array. */
    external fun listConversations(): String

    /** Offline message history with [peer], oldest first, as a JSON array. */
    external fun messagesWith(peer: String): String

    // ── Replies, message requests & receipts ─────────────────────────────────

    /** Send a reply DM ([replyTo] = replied event id hex, or "" for none). */
    external fun sendDmReply(target: String, content: String, replyTo: String): String

    /** Pending message requests as a JSON array of `{"peer","last_message","last_at"}`. */
    external fun messageRequests(): String

    /** Accept a message request; returns `{"accepted":true}` or `{"error":"…"}`. */
    external fun acceptRequest(peer: String): String

    /** Block a peer; returns `{"blocked":true}` or `{"error":"…"}`. */
    external fun blockConversation(peer: String): String

    /** Send a read receipt for [peer]'s thread; returns `{"ok":true}`. */
    external fun markConversationRead(peer: String): String

    // ── Encrypted media (send/receive) ────────────────────────────────────────

    /** Encrypt + upload [base64] media and deliver the reference. Returns media JSON. */
    external fun sendMediaBytes(
        target: String,
        mimeType: String,
        caption: String,
        base64: String,
    ): String

    /** Resolve + decrypt a media reference by event id. Returns `{"mime_type","base64"}`. */
    external fun downloadMedia(eventId: String): String

    // ── Calls (voice/video signaling over the DM channel) ─────────────────────

    /** ICE servers for the WebRTC layer as a JSON array. */
    external fun callIceServers(): String

    /** Configure ("" url clears) the TURN relay; returns `{"ok":true}`. */
    external fun setTurnServer(url: String, username: String, credential: String): String

    /** Begin a call to [peer] ([media] = "audio"/"video"); returns call-session JSON. */
    external fun placeCall(peer: String, media: String): String

    /** Send one call-signaling payload ([signalJson] = a CallSignal). Returns `{"ok":true}`. */
    external fun sendCallSignal(peer: String, callId: String, media: String, signalJson: String): String

    /** Send a `Hangup` with [reason]; returns `{"ok":true}`. */
    external fun hangupCall(peer: String, callId: String, media: String, reason: String): String

    /** Persist a finished call to the log; returns the call-record JSON. */
    external fun logCall(
        peer: String,
        callId: String,
        media: String,
        incoming: Boolean,
        outcome: String,
        startedAt: Long,
        durationSecs: Long,
    ): String

    /** Call log as a JSON array ([peer] "" = all peers). */
    external fun callHistory(peer: String): String

    // ── Journal (strictly local, never networked) ────────────────────────────

    /**
     * Save a private journal entry ([mood] may be empty for none). The entry
     * never leaves the device — sealed in the encrypted store only.
     * @return stored entry JSON or `{"error":"…"}`.
     */
    external fun addJournalEntry(text: String, mood: String): String

    /** All journal entries, newest first, as a JSON array. */
    external fun listJournal(): String

    /** Delete a journal entry. @return JSON `{"removed":true|false}`. */
    external fun deleteJournalEntry(id: String): String

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

    /** Live status of the off-grid Saathi mesh (mDNS discovery + Gossipsub). */
    data class MeshStatus(val active: Boolean, val peerCount: Int)

    /** Snapshot of the mesh's current status. Never throws. */
    fun meshStatusTyped(): MeshStatus {
        val o = JSONObject(meshStatus())
        return MeshStatus(active = o.optBoolean("active"), peerCount = o.optInt("peer_count"))
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

    /**
     * A pinned contact. [alias] is the name *you* chose (blank = none);
     * [name] is the @handle *they* published, from the local profile cache.
     */
    data class ContactInfo(val npub: String, val alias: String, val name: String?)

    data class ConversationInfo(
        val peer: String,
        /** User-chosen alias for the peer, when one exists. */
        val alias: String?,
        /** The peer's own published @handle, from the local profile cache. */
        val peerName: String?,
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
        /** Delivery status for outgoing messages: "sent"/"delivered"/"read"; null for incoming. */
        val status: String? = null,
        /** Event id (hex) this message replies to, if any. */
        val replyTo: String? = null,
    )

    /** A pending message request — a stranger's DM gated until accepted. */
    data class MessageRequestInfo(val peer: String, val lastMessage: String, val lastAt: Long)

    /** A WebRTC ICE server for the call layer's RTCConfiguration. */
    data class IceServerInfo(
        val urls: List<String>,
        val username: String?,
        val credential: String?,
    )

    /** A minted call session: id, peer, media kind, and ICE servers. */
    data class CallSessionInfo(
        val callId: String,
        val peer: String,
        val media: String,
        val iceServers: List<IceServerInfo>,
    )

    /** A call-log entry. */
    data class CallRecordInfo(
        val id: String,
        val peer: String,
        val media: String,
        val incoming: Boolean,
        val outcome: String,
        val startedAt: Long,
        val durationSecs: Long,
    )

    /** An encrypted-media message (send result or incoming reference). */
    data class MediaMessageInfo(
        val eventId: String,
        val url: String,
        val mimeType: String,
        val caption: String,
        val sender: String,
        val createdAt: Long,
        val size: Long,
    )

    /** Decrypted media bytes (base64) plus MIME type. */
    data class MediaBytesInfo(val mimeType: String, val base64: String)

    /** A private journal entry — local-only, sealed by the passcode. */
    data class JournalEntryInfo(
        val id: String,
        val text: String,
        val mood: String?,
        val createdAt: Long,
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

    private fun JSONObject.toContact() = ContactInfo(
        npub = getString("npub"),
        alias = getString("alias"),
        name = optNullableString("name"),
    )

    /** Pin a contact by npub, returning the saved entry or throwing. */
    fun addContactTyped(npub: String, alias: String): ContactInfo =
        JSONObject(addContact(npub, alias)).failOnError("Add contact").toContact()

    /** Set (non-empty) or clear (empty) a contact's alias, or throw. */
    fun setContactAliasTyped(npub: String, alias: String): ContactInfo =
        JSONObject(setContactAlias(npub, alias)).failOnError("Set alias").toContact()

    /** Remove a saved contact; true if one existed. Throws on backend error. */
    fun removeContactTyped(npub: String): Boolean {
        val o = JSONObject(removeContact(npub)).failOnError("Remove contact")
        return o.getBoolean("removed")
    }

    /** Refresh cached peer @handles; returns how many display names changed. */
    fun refreshPeerProfilesTyped(): Int {
        val o = JSONObject(refreshPeerProfiles()).failOnError("Profile refresh")
        return o.getInt("changed")
    }

    /** All saved contacts. */
    fun contacts(): List<ContactInfo> {
        val parsed = JSONTokener(listContacts()).nextValue()
        if (parsed is JSONObject) parsed.failOnError("Contacts")
        val arr = parsed as JSONArray
        return (0 until arr.length()).map { i -> arr.getJSONObject(i).toContact() }
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
                peerName = o.optNullableString("peer_name"),
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

    /** Send a reply DM ([replyTo] null/"" for a normal message). */
    fun sendDmReplyTyped(target: String, content: String, replyTo: String?): MessageInfo =
        JSONObject(sendDmReply(target, content, replyTo ?: "")).failOnError("Send").toMessage()

    /** Pending message requests, newest first. */
    fun messageRequestsTyped(): List<MessageRequestInfo> {
        val parsed = JSONTokener(messageRequests()).nextValue()
        if (parsed is JSONObject) parsed.failOnError("Requests")
        val arr = parsed as JSONArray
        return (0 until arr.length()).map { i ->
            val o = arr.getJSONObject(i)
            MessageRequestInfo(
                peer = o.getString("peer"),
                lastMessage = o.getString("last_message"),
                lastAt = o.getLong("last_at"),
            )
        }
    }

    /** Accept a message request; throws on backend error. */
    fun acceptRequestTyped(peer: String) {
        JSONObject(acceptRequest(peer)).failOnError("Accept")
    }

    /** Block a peer; throws on backend error. */
    fun blockConversationTyped(peer: String) {
        JSONObject(blockConversation(peer)).failOnError("Block")
    }

    /** Send a read receipt for a conversation; throws on backend error. */
    fun markConversationReadTyped(peer: String) {
        JSONObject(markConversationRead(peer)).failOnError("Mark read")
    }

    private fun JSONObject.toMediaMessage() = MediaMessageInfo(
        eventId = getString("event_id"),
        url = getString("url"),
        mimeType = getString("mime_type"),
        caption = optString("caption"),
        sender = getString("sender"),
        createdAt = getLong("created_at"),
        size = getLong("size"),
    )

    /** Encrypt + send media, returning the stored reference or throwing. */
    fun sendMediaBytesTyped(
        target: String,
        mimeType: String,
        caption: String,
        base64: String,
    ): MediaMessageInfo =
        JSONObject(sendMediaBytes(target, mimeType, caption, base64))
            .failOnError("Send media")
            .toMediaMessage()

    /** Download + decrypt a media reference by event id. */
    fun downloadMediaTyped(eventId: String): MediaBytesInfo {
        val o = JSONObject(downloadMedia(eventId)).failOnError("Media download")
        return MediaBytesInfo(mimeType = o.getString("mime_type"), base64 = o.getString("base64"))
    }

    private fun JSONObject.toIceServer(): IceServerInfo {
        val urlsArr = getJSONArray("urls")
        val urls = (0 until urlsArr.length()).map { urlsArr.getString(it) }
        return IceServerInfo(
            urls = urls,
            username = optNullableString("username"),
            credential = optNullableString("credential"),
        )
    }

    /** ICE servers for the WebRTC layer. */
    fun callIceServersTyped(): List<IceServerInfo> {
        val parsed = JSONTokener(callIceServers()).nextValue()
        if (parsed is JSONObject) parsed.failOnError("ICE servers")
        val arr = parsed as JSONArray
        return (0 until arr.length()).map { i -> arr.getJSONObject(i).toIceServer() }
    }

    /** Configure ("" url clears) the TURN relay; throws on backend error. */
    fun setTurnServerTyped(url: String, username: String, credential: String) {
        JSONObject(setTurnServer(url, username, credential)).failOnError("TURN config")
    }

    /** Begin a call, returning the minted session (id + ICE servers). */
    fun placeCallTyped(peer: String, media: String): CallSessionInfo {
        val o = JSONObject(placeCall(peer, media)).failOnError("Place call")
        val iceArr = o.getJSONArray("ice_servers")
        return CallSessionInfo(
            callId = o.getString("call_id"),
            peer = o.getString("peer"),
            media = o.getString("media"),
            iceServers = (0 until iceArr.length()).map { iceArr.getJSONObject(it).toIceServer() },
        )
    }

    /** Send one call-signaling payload; throws on backend error. */
    fun sendCallSignalTyped(peer: String, callId: String, media: String, signalJson: String) {
        JSONObject(sendCallSignal(peer, callId, media, signalJson)).failOnError("Call signal")
    }

    /** Send a `Hangup` with [reason]; throws on backend error. */
    fun hangupCallTyped(peer: String, callId: String, media: String, reason: String) {
        JSONObject(hangupCall(peer, callId, media, reason)).failOnError("Hangup")
    }

    private fun JSONObject.toCallRecord() = CallRecordInfo(
        id = getString("id"),
        peer = getString("peer"),
        media = getString("media"),
        incoming = getBoolean("incoming"),
        outcome = getString("outcome"),
        startedAt = getLong("started_at"),
        durationSecs = getLong("duration_secs"),
    )

    /** Persist a finished call to the log, returning the stored record. */
    fun logCallTyped(
        peer: String,
        callId: String,
        media: String,
        incoming: Boolean,
        outcome: String,
        startedAt: Long,
        durationSecs: Long,
    ): CallRecordInfo =
        JSONObject(logCall(peer, callId, media, incoming, outcome, startedAt, durationSecs))
            .failOnError("Log call")
            .toCallRecord()

    /** The call log ([peer] null = all peers), newest first. */
    fun callHistoryTyped(peer: String? = null): List<CallRecordInfo> {
        val parsed = JSONTokener(callHistory(peer ?: "")).nextValue()
        if (parsed is JSONObject) parsed.failOnError("Call history")
        val arr = parsed as JSONArray
        return (0 until arr.length()).map { i -> arr.getJSONObject(i).toCallRecord() }
    }

    private fun JSONObject.toJournalEntry() = JournalEntryInfo(
        id = getString("id"),
        text = getString("text"),
        mood = optNullableString("mood"),
        createdAt = getLong("created_at"),
    )

    /** Save a journal entry, returning the stored record or throwing. */
    fun addJournalEntryTyped(text: String, mood: String?): JournalEntryInfo =
        JSONObject(addJournalEntry(text, mood ?: "")).failOnError("Journal").toJournalEntry()

    /** All journal entries, newest first. */
    fun journal(): List<JournalEntryInfo> {
        val parsed = JSONTokener(listJournal()).nextValue()
        if (parsed is JSONObject) parsed.failOnError("Journal")
        val arr = parsed as JSONArray
        return (0 until arr.length()).map { i -> arr.getJSONObject(i).toJournalEntry() }
    }

    /** Delete a journal entry; true if one existed. */
    fun deleteJournalEntryTyped(id: String): Boolean {
        val o = JSONObject(deleteJournalEntry(id)).failOnError("Journal delete")
        return o.getBoolean("removed")
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
        status = optNullableString("status"),
        replyTo = optNullableString("reply_to"),
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
