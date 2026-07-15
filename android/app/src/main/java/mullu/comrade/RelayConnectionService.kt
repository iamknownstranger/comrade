package mullu.comrade

import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.SharedPreferences
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import mullu.comrade.call.CallManager
import mullu.comrade.call.CallUiState
import mullu.comrade.ui.shortNpub
import uniffi.comrade_ui.BridgeEvent

/**
 * Keeps the relay connection (and therefore notification delivery) alive
 * while the vault is unlocked but no Activity is visible — an accepted DM or
 * incoming call must still surface a notification 30 minutes into the
 * background, which a plain Activity-scoped coroutine cannot guarantee: once
 * nothing is visible, the process is "cached" priority and the OS can and
 * will reclaim it at any time. A foreground service (with the ongoing,
 * deliberately minimal notification Android requires for one) buys the
 * process a real priority floor for as long as this runs.
 *
 * This is also, now, the **sole** consumer of [ComradeCore.pollEvent] — it
 * used to be drained from inside [MainActivity]'s Compose tree
 * (Activity-scoped, so it stopped the moment nothing was visible, and would
 * have raced a second drainer had one ever been added). Moving the drain
 * loop here, with [ChatEventRouter] holding the resulting state as
 * `StateFlow`s the UI observes instead of running its own pump, is what
 * makes "Activity recreation never creates duplicate listeners or duplicate
 * notifications" hold structurally: there is exactly one drain loop, owned
 * by this service, independent of how many times `MainActivity` is
 * recreated.
 *
 * ## Lifecycle
 * Started ([start]) once the vault is unlocked (see `ComradeApp`'s
 * `AppPhase.Ready` transition) — a no-op if the user has turned the feature
 * off (see [BackgroundConnectivityPreference]). Stopped ([stop]) on vault
 * lock, an explicit disconnect, or logout — today the app only has "lock
 * vault", but the same call covers whichever of those a future screen adds.
 * Starting it twice, or stopping it when not running, is harmless.
 *
 * ## Security boundary — read this before assuming more than it promises
 * This service supports **backgrounded-but-unlocked** operation: the vault's
 * decrypted key stays in the native process's memory, exactly as it would
 * with the app merely open, for as long as the OS keeps this process alive.
 * It does **not** change what happens on process death (planned or OOM-kill)
 * — the in-memory key is gone either way, same as before this service
 * existed, and the app returns to the locked/passphrase screen on next
 * launch. It does **not** implement push notifications: nothing here wakes a
 * *killed* process. Delivering a message while the process is not merely
 * backgrounded but actually dead needs a push-notification wakeup path — a
 * separate product and privacy decision (a push token identifies the device
 * to whatever relay/push-provider sends it, which is a real metadata
 * tradeoff for a privacy-first app), deliberately out of scope here.
 */
class RelayConnectionService : Service() {

    private var pumpJob: Job? = null
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    override fun onCreate() {
        super.onCreate()
        Notifier.ensureChannels(this)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        startForegroundNotified()
        if (pumpJob == null) {
            pumpJob = scope.launch { pump() }
        }
        return START_STICKY
    }

    override fun onDestroy() {
        super.onDestroy()
        pumpJob?.cancel()
        pumpJob = null
        scope.cancel()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun startForegroundNotified() {
        val openApp = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java).apply {
                flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
            },
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        // Deliberately minimal: no peer names, no message previews, no
        // counts — just "Comrade is running", matching the notification
        // content rules the message/request/call notifications already
        // follow (see Notifier's doc comment).
        val notification = NotificationCompat.Builder(this, Notifier.CHANNEL_CONNECTION)
            .setSmallIcon(android.R.drawable.stat_sys_download_done)
            .setContentTitle(getString(R.string.relay_connection_notification_title))
            .setContentText(getString(R.string.relay_connection_notification_text))
            .setOngoing(true)
            .setPriority(NotificationCompat.PRIORITY_MIN)
            .setContentIntent(openApp)
            .build()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    /**
     * Seed offline-first state, then drain [ComradeCore.pollEvent] until
     * cancelled. Draining is immediate while events are queued (no
     * artificial batching delay a call/DM would sit behind) and only backs
     * off to [POLL_IDLE_MS] once the queue is actually empty.
     */
    private suspend fun pump() {
        val cached = withContext(Dispatchers.IO) {
            runCatching { ComradeCore.sabhaTimeline() }.getOrDefault(emptyList())
        }
        ChatEventRouter.seedCachedFeed(cached)

        val initialMesh = withContext(Dispatchers.IO) {
            runCatching { ComradeCore.meshStatusTyped() }
                .getOrDefault(ComradeCore.MeshStatus(active = false, peerCount = 0))
        }
        MeshStatusMonitor.update(initialMesh)
        ChatEventRouter.maybeRefreshNames()

        val appContext = applicationContext
        while (currentCoroutineContext().isActive) {
            val event = ComradeCore.pollEvent()
            if (event == null) {
                delay(POLL_IDLE_MS)
                continue
            }
            ChatEventRouter.route(appContext, event)
        }
    }

    companion object {
        private const val NOTIFICATION_ID = 0xC0A1EC7
        private const val POLL_IDLE_MS = 200L

        /** Start the service — a no-op if the user has disabled the feature. */
        fun start(context: Context) {
            if (!BackgroundConnectivityPreference.isEnabled(context)) return
            context.startForegroundService(Intent(context, RelayConnectionService::class.java))
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, RelayConnectionService::class.java))
        }
    }
}

/**
 * Whether the user wants [RelayConnectionService] to run at all — default
 * on, since the acceptance bar for this feature ("an accepted DM notifies
 * you 30 minutes into the background") only holds if it's running, but the
 * persistent low-priority notification and background battery use are a
 * real, visible tradeoff a user should be able to opt out of.
 */
object BackgroundConnectivityPreference {
    private const val PREFS_NAME = "comrade_prefs"
    private const val KEY_ENABLED = "background_connectivity_enabled"

    private fun prefs(context: Context): SharedPreferences =
        context.applicationContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)

    fun isEnabled(context: Context): Boolean = prefs(context).getBoolean(KEY_ENABLED, true)

    fun setEnabled(context: Context, enabled: Boolean) {
        prefs(context).edit().putBoolean(KEY_ENABLED, enabled).apply()
    }
}

/**
 * App-level (not Activity-level) home for state derived from the native
 * event stream, and for the notification-triggering side effects that must
 * keep working without a visible Activity. [RelayConnectionService.pump] is
 * the only caller of [route]; every screen instead collects the `StateFlow`s
 * below, the same pattern [MeshStatusMonitor] and
 * [mullu.comrade.call.CallManager] already use.
 */
object ChatEventRouter {
    /** Bound on the in-memory public feed (the relay stream is unbounded). */
    private const val FEED_CAP = 500

    /** Floor between peer-name refreshes; the Rust side is TTL-gated too. */
    private const val NAME_REFRESH_MIN_INTERVAL_MS = 30_000L

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    private val _feedItems = MutableStateFlow<List<ComradeCore.ChitthiInfo>>(emptyList())
    val feedItems: StateFlow<List<ComradeCore.ChitthiInfo>> = _feedItems.asStateFlow()
    private val seenFeedIds = HashSet<String>()

    /** Bumped whenever the DM history changed; list + open thread reload on it. */
    private val _chatTick = MutableStateFlow(0)
    val chatTick: StateFlow<Int> = _chatTick.asStateFlow()

    /**
     * Force a chat-list reload from outside the event-routing path — e.g. a
     * contact alias was edited locally, which changes chat-list titles
     * without any native event having fired.
     */
    fun bumpChatTick() {
        _chatTick.update { it + 1 }
    }

    /** Bumped when a new message request arrives; the requests list reloads on it. */
    private val _requestTick = MutableStateFlow(0)
    val requestTick: StateFlow<Int> = _requestTick.asStateFlow()

    /**
     * The peer (npub) of the conversation currently on screen, if any — set
     * by [mullu.comrade.MainActivity] so a DM notification is suppressed for
     * the thread the user is already looking at, exactly as before this
     * lived in the Activity's own pump loop.
     */
    private val _openConversationPeer = MutableStateFlow<String?>(null)
    fun setOpenConversation(peer: String?) {
        _openConversationPeer.value = peer
    }

    @Volatile private var refreshingNames = false
    @Volatile private var lastNameRefreshAt = 0L

    /** Add a freshly-arrived (or cached, on seed) Chitthi to the front of the feed, capped at [FEED_CAP]. */
    fun addChitthi(item: ComradeCore.ChitthiInfo, front: Boolean = true) {
        if (!seenFeedIds.add(item.id)) return
        _feedItems.update { current ->
            val updated = if (front) listOf(item) + current else current + item
            if (updated.size > FEED_CAP) {
                val dropped = if (front) updated.last() else updated.first()
                seenFeedIds.remove(dropped.id)
                if (front) updated.dropLast(1) else updated.drop(1)
            } else {
                updated
            }
        }
    }

    /** Offline-first seed of the cached feed, oldest-loaded-last so it renders newest-first. */
    fun seedCachedFeed(cached: List<ComradeCore.ChitthiInfo>) {
        for (item in cached.sortedByDescending { it.createdAt }) addChitthi(item, front = false)
    }

    /**
     * Fetch peers' published @handles so chats are titled by name instead of
     * key — single-flight and rate-limited (the Rust side is also
     * TTL-gated), and never awaited by [RelayConnectionService.pump], so a
     * slow relay can't stall event draining.
     */
    fun maybeRefreshNames() {
        val now = System.currentTimeMillis()
        if (refreshingNames || now - lastNameRefreshAt < NAME_REFRESH_MIN_INTERVAL_MS) return
        refreshingNames = true
        lastNameRefreshAt = now
        scope.launch {
            try {
                val changed = withContext(Dispatchers.IO) {
                    runCatching { ComradeCore.refreshPeerProfilesTyped() }.getOrDefault(0)
                }
                if (changed > 0) _chatTick.update { it + 1 }
            } finally {
                refreshingNames = false
            }
        }
    }

    private fun uniffi.comrade_ui.ChitthiDto.toInfo() = ComradeCore.ChitthiInfo(
        id = id,
        author = author,
        content = content,
        createdAt = createdAt.toLong(),
        replyTo = replyTo,
    )

    /** Route one drained [BridgeEvent]: update shared state and fire any notification. */
    fun route(context: Context, event: BridgeEvent) {
        when (event) {
            is BridgeEvent.IncomingChitthi -> addChitthi(event.v1.toInfo(), front = true)
            is BridgeEvent.IncomingDirectMessage -> {
                _chatTick.update { it + 1 }
                val peer = event.v1.sender
                if (peer != _openConversationPeer.value) {
                    Notifier.notifyMessage(
                        context,
                        peer,
                        shortNpub(peer),
                        event.v1.content.ifBlank { "New message" },
                    )
                }
            }
            is BridgeEvent.IncomingMessageRequest -> {
                _requestTick.update { it + 1 }
                Notifier.notifyRequest(context, event.v1.peer, event.v1.lastMessage.ifBlank { "New message request" })
            }
            is BridgeEvent.IncomingMedia -> {
                _chatTick.update { it + 1 }
                val peer = event.v1.sender
                if (peer != _openConversationPeer.value) {
                    Notifier.notifyMessage(
                        context,
                        peer,
                        shortNpub(peer),
                        "📎 " + event.v1.caption.ifBlank { "Attachment" },
                    )
                }
            }
            is BridgeEvent.MessageStatus -> {
                _chatTick.update { it + 1 }
            }
            is BridgeEvent.PeerProfileUpdated -> {
                _chatTick.update { it + 1 }
                // A DM from an unknown key may now be nameable.
                maybeRefreshNames()
            }
            is BridgeEvent.MeshStatusChanged -> MeshStatusMonitor.update(
                ComradeCore.MeshStatus(active = event.v1.active, peerCount = event.v1.peerCount.toInt()),
            )
            is BridgeEvent.IncomingCallSignal -> {
                // Feed every signal into the WebRTC layer (answers + ICE land
                // in the live PeerConnection); a fresh incoming offer returns
                // true → raise the ringing notification so a call is visible
                // even when the app isn't in the foreground.
                val freshIncoming = CallManager.onIncomingSignal(event.v1)
                if (freshIncoming) {
                    // CallManager already resolved the caller's alias/published
                    // name (the same precedence the chat list and call history
                    // use) into the ringing state's peerLabel — read it back
                    // instead of falling to the bare key here too, so the
                    // notification and the ringing screen agree.
                    val title = (CallManager.state.value as? CallUiState.Ringing)?.peerLabel
                        ?: shortNpub(event.v1.peer)
                    Notifier.notifyIncomingCall(
                        context,
                        event.v1.peer,
                        title,
                        video = event.v1.media == "video",
                    )
                }
            }
            // Sakha/ledger sync isn't wired into the Android UI yet
            // (desktop-only via Tauri commands) — drop, like before.
            is BridgeEvent.LedgerUpdated -> Unit
        }
    }
}
