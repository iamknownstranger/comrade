package mullu.comrade

import android.app.Activity
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Create
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Settings
import androidx.core.content.ContextCompat
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CenterAlignedTopAppBar
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import mullu.comrade.call.CallManager
import mullu.comrade.ui.ArticleIcon
import mullu.comrade.ui.BookIcon
import mullu.comrade.ui.CallIcon
import mullu.comrade.ui.CallScreen
import mullu.comrade.ui.CameraIcon
import mullu.comrade.ui.ChatBubbleIcon
import mullu.comrade.ui.ChatsScreen
import mullu.comrade.ui.ConversationScreen
import mullu.comrade.ui.FeedScreen
import mullu.comrade.ui.JournalScreen
import mullu.comrade.ui.NewChatScreen
import mullu.comrade.ui.OnboardingScreen
import mullu.comrade.ui.PeerAvatar
import mullu.comrade.ui.RequestsScreen
import mullu.comrade.ui.SettingsScreen
import mullu.comrade.ui.peerTitle
import mullu.comrade.ui.purgeDecryptedMedia
import mullu.comrade.ui.shortNpub
import mullu.comrade.ui.theme.ComradeTheme
import uniffi.comrade_core.CallSignal
import uniffi.comrade_ui.BridgeEvent
import uniffi.comrade_ui.CallSignalDto

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Screens can display key material — block screenshots and screen
        // recording for the whole activity (AUDIT S5 / M1-6).
        window.setFlags(
            WindowManager.LayoutParams.FLAG_SECURE,
            WindowManager.LayoutParams.FLAG_SECURE,
        )
        setContent {
            ComradeTheme {
                Surface(
                    modifier = Modifier.fillMaxSize(),
                    color = MaterialTheme.colorScheme.background,
                ) {
                    ComradeApp()
                }
            }
        }
    }

    /**
     * Backgrounding is our "session over" signal: drop every decrypted media
     * plaintext the app cached this session (received voice notes, images,
     * videos) from `cacheDir/media`. Anything the user reopens is transparently
     * re-decrypted, so this leaves nothing recoverable at rest yet costs the
     * user nothing (AUDIT S-4). The same call is the natural hook for an
     * explicit vault-lock action once one exists.
     */
    override fun onStop() {
        super.onStop()
        purgeDecryptedMedia(this)
    }
}

/** Where the encrypted vault lives on this device. */
internal fun vaultPath(context: Context): File = File(context.filesDir, "comrade-vault")

/** True if [permission] is already granted (runtime check). */
private fun hasPermission(context: Context, permission: String): Boolean =
    ContextCompat.checkSelfPermission(context, permission) == PackageManager.PERMISSION_GRANTED

/** Startup phases: resolve what's on disk, then either the door or the app. */
private sealed interface AppPhase {
    object Checking : AppPhase
    data class Locked(val vaultExists: Boolean) : AppPhase
    data class Ready(val profile: ComradeCore.Profile) : AppPhase
}

@Composable
fun ComradeApp() {
    val context = LocalContext.current
    val activity = context as? Activity
    var phase by remember { mutableStateOf<AppPhase>(AppPhase.Checking) }

    // First ComradeCore touch pays for System.loadLibrary of the Rust core —
    // resolved on IO so the first frame renders instantly. If the process
    // already holds an unlocked runtime (activity recreation), skip the door.
    LaunchedEffect(Unit) {
        phase = withContext(Dispatchers.IO) {
            runCatching { ComradeCore.currentProfileTyped() }.fold(
                onSuccess = { AppPhase.Ready(it) },
                onFailure = { AppPhase.Locked(vaultPath(context).exists()) },
            )
        }
    }

    // Startup observability: "Fully drawn" once real content replaced the spinner.
    LaunchedEffect(phase is AppPhase.Checking) {
        if (phase !is AppPhase.Checking) activity?.reportFullyDrawn()
    }

    when (val p = phase) {
        AppPhase.Checking -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            CircularProgressIndicator(Modifier.size(28.dp))
        }
        is AppPhase.Locked -> OnboardingScreen(
            vaultExists = p.vaultExists,
            unlock = { passcode ->
                ComradeCore.unlockVaultTyped(vaultPath(context).absolutePath, passcode)
                ComradeCore.currentProfileTyped()
            },
            claimUsername = { handle -> ComradeCore.setUsernameTyped(handle) },
            onReady = { phase = AppPhase.Ready(it) },
        )
        is AppPhase.Ready -> MainShell(
            profile = p.profile,
            onProfileChange = { phase = AppPhase.Ready(it) },
        )
    }
}

// ── Main shell: Chats · Feed · Settings ──────────────────────────────────────

private enum class MainTab(val label: String, val icon: ImageVector) {
    Chats("Chats", ChatBubbleIcon),
    Journal("Journal", BookIcon),
    Feed("Feed", ArticleIcon),
    Settings("Settings", Icons.Filled.Settings),
}

/** Sub-navigation inside the Chats tab. */
private sealed interface ChatNav {
    data object List : ChatNav
    data object NewChat : ChatNav
    data object Requests : ChatNav
    data class Open(
        val peer: String,
        /** User-chosen alias for the peer, when one exists. */
        val alias: String?,
        /** The peer's own published @handle, when known. */
        val username: String?,
    ) : ChatNav
}

/** Events drained from the native bridge, reduced to what the shell reacts to. */
private sealed interface PumpEvent {
    data class Chitthi(val info: ComradeCore.ChitthiInfo) : PumpEvent

    /** A DM from an accepted conversation — reload + notify. */
    data class IncomingDm(val peer: String, val preview: String) : PumpEvent

    /** A stranger's gated DM — refresh requests + notify. */
    data class Request(val peer: String, val preview: String) : PumpEvent

    /** Any WebRTC call signal (offer/answer/ICE/hangup) for the call layer. */
    data class CallSignalReceived(val dto: CallSignalDto) : PumpEvent

    /** Media / receipt / profile update — just reload the chat lists. */
    data object HistoryChanged : PumpEvent

    /** The off-grid mesh's live connectivity changed. */
    data class MeshStatusChanged(val status: ComradeCore.MeshStatus) : PumpEvent
}

/** Bound on the in-memory public feed (the relay stream is unbounded). */
private const val FEED_CAP = 500

/** Floor between peer-name refreshes; the Rust side is TTL-gated too. */
private const val NAME_REFRESH_MIN_INTERVAL_MS = 30_000L

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun MainShell(
    profile: ComradeCore.Profile,
    onProfileChange: (ComradeCore.Profile) -> Unit,
) {
    val context = LocalContext.current
    var tab by rememberSaveable { mutableStateOf(MainTab.Chats) }
    var chatNav by remember { mutableStateOf<ChatNav>(ChatNav.List) }
    // Bumped whenever the DM history changed; list + open thread reload on it.
    var chatTick by remember { mutableStateOf(0) }
    // Bumped when a new message request arrives; the requests list reloads on it.
    var requestTick by remember { mutableStateOf(0) }
    val feedItems = remember { mutableStateListOf<ComradeCore.ChitthiInfo>() }
    val seenFeedIds = remember { HashSet<String>() }

    // Active voice/video call — drives the full-screen call overlay.
    val callState by CallManager.state.collectAsState()
    // A call awaiting the mic/camera grant before it can start (peer, isVideo).
    var pendingCall by remember { mutableStateOf<Pair<String, Boolean>?>(null) }
    val callPermissions = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions(),
    ) { result ->
        val call = pendingCall
        pendingCall = null
        if (call != null) {
            val micOk = result[android.Manifest.permission.RECORD_AUDIO]
                ?: hasPermission(context, android.Manifest.permission.RECORD_AUDIO)
            val camOk = !call.second ||
                (result[android.Manifest.permission.CAMERA]
                    ?: hasPermission(context, android.Manifest.permission.CAMERA))
            if (micOk && camOk) CallManager.startOutgoing(context, call.first, call.second)
        }
    }

    // Place a call, requesting only the permissions it actually needs first.
    fun placeCall(peer: String, video: Boolean) {
        val needed = buildList {
            if (!hasPermission(context, android.Manifest.permission.RECORD_AUDIO)) {
                add(android.Manifest.permission.RECORD_AUDIO)
            }
            if (video && !hasPermission(context, android.Manifest.permission.CAMERA)) {
                add(android.Manifest.permission.CAMERA)
            }
        }
        if (needed.isEmpty()) {
            CallManager.startOutgoing(context, peer, video)
        } else {
            pendingCall = peer to video
            callPermissions.launch(needed.toTypedArray())
        }
    }

    // Notification channels + runtime permission (Android 13+). Notifications
    // fire for incoming DMs/requests while the app process is alive.
    val notifPermission = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { }
    LaunchedEffect(Unit) {
        Notifier.ensureChannels(context)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU && !Notifier.hasPermission(context)) {
            notifPermission.launch(android.Manifest.permission.POST_NOTIFICATIONS)
        }
    }

    fun addToFeed(item: ComradeCore.ChitthiInfo, front: Boolean) {
        if (!seenFeedIds.add(item.id)) return
        if (front) feedItems.add(0, item) else feedItems.add(item)
        while (feedItems.size > FEED_CAP) {
            seenFeedIds.remove(feedItems.removeAt(feedItems.size - 1).id)
        }
    }

    // Offline-first load of the cached feed, then the live event pump.
    LaunchedEffect(Unit) {
        val cached = withContext(Dispatchers.IO) {
            runCatching { ComradeCore.sabhaTimeline() }.getOrDefault(emptyList())
        }
        for (item in cached.sortedByDescending { it.createdAt }) addToFeed(item, front = false)

        // Seed the mesh indicator with a real snapshot before the first
        // mesh_status_changed event arrives (e.g. a fresh process that was
        // already off-grid — an activity recreation, not a cold identity).
        val initialMesh = withContext(Dispatchers.IO) {
            runCatching { ComradeCore.meshStatusTyped() }
                .getOrDefault(ComradeCore.MeshStatus(active = false, peerCount = 0))
        }
        MeshStatusMonitor.update(initialMesh)

        // Fetch the published @handles of people we already talk to, so chats
        // are titled by name instead of key. Launched on its own coroutine —
        // never awaited by the pump, so a slow relay can't stall event
        // draining — single-flight, and rate-limited (the Rust side is also
        // TTL-gated). > 0 changes means the chat list needs a reload.
        var refreshingNames = false
        var lastNameRefreshAt = 0L
        fun maybeRefreshNames() {
            val now = System.currentTimeMillis()
            if (refreshingNames || now - lastNameRefreshAt < NAME_REFRESH_MIN_INTERVAL_MS) return
            refreshingNames = true
            lastNameRefreshAt = now
            launch {
                try {
                    val changed = withContext(Dispatchers.IO) {
                        runCatching { ComradeCore.refreshPeerProfilesTyped() }.getOrDefault(0)
                    }
                    if (changed > 0) chatTick++
                } finally {
                    refreshingNames = false
                }
            }
        }
        maybeRefreshNames()

        while (isActive) {
            val events = withContext(Dispatchers.IO) { drainEvents() }
            var historyChanged = false
            for (event in events) {
                when (event) {
                    is PumpEvent.Chitthi -> addToFeed(event.info, front = true)
                    is PumpEvent.IncomingDm -> {
                        historyChanged = true
                        // Suppress a notification for the conversation on screen.
                        val openPeer = (chatNav as? ChatNav.Open)?.peer
                        if (event.peer != openPeer) {
                            Notifier.notifyMessage(
                                context,
                                event.peer,
                                shortNpub(event.peer),
                                event.preview,
                            )
                        }
                    }
                    is PumpEvent.Request -> {
                        requestTick++
                        Notifier.notifyRequest(context, event.peer, event.preview)
                    }
                    is PumpEvent.CallSignalReceived -> {
                        val dto = event.dto
                        // Drive the WebRTC state machine: offer → ring, answer/
                        // ICE → feed the peer connection, hangup → tear down.
                        CallManager.onRemoteSignal(dto)
                        // A fresh offer also raises a system notification so a
                        // backgrounded user sees the ringing call.
                        if (dto.signal is CallSignal.Offer) {
                            Notifier.notifyIncomingCall(
                                context,
                                dto.peer,
                                shortNpub(dto.peer),
                                dto.media == "video",
                            )
                        }
                    }
                    PumpEvent.HistoryChanged -> historyChanged = true
                    is PumpEvent.MeshStatusChanged -> MeshStatusMonitor.update(event.status)
                }
            }
            if (historyChanged) {
                chatTick++
                // A DM from an unknown key may now be nameable.
                maybeRefreshNames()
            }
            delay(600)
        }
    }

    val openChat = chatNav as? ChatNav.Open
    var editingAlias by remember { mutableStateOf(false) }
    BackHandler(enabled = tab == MainTab.Chats && chatNav != ChatNav.List) {
        chatNav = ChatNav.List
    }

    Box(modifier = Modifier.fillMaxSize()) {
    Column(modifier = Modifier.fillMaxSize()) {
        MeshStatusBanner()
        Scaffold(
            modifier = Modifier.weight(1f),
            topBar = {
                when {
                    tab == MainTab.Chats && openChat != null -> TopAppBar(
                        navigationIcon = {
                            IconButton(onClick = { chatNav = ChatNav.List }) {
                                Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                            }
                        },
                        title = {
                            val title = peerTitle(openChat.peer, openChat.alias, openChat.username)
                            Row(
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(10.dp),
                            ) {
                                PeerAvatar(title, seed = openChat.peer, size = 36.dp)
                                Column {
                                    Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                    Text(
                                        shortNpub(openChat.peer),
                                        style = MaterialTheme.typography.labelSmall,
                                        fontFamily = FontFamily.Monospace,
                                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                                    )
                                }
                            }
                        },
                        actions = {
                            IconButton(
                                onClick = { placeCall(openChat.peer, false) },
                                modifier = Modifier.testTag("call-voice"),
                            ) {
                                Icon(CallIcon, contentDescription = "Voice call")
                            }
                            IconButton(
                                onClick = { placeCall(openChat.peer, true) },
                                modifier = Modifier.testTag("call-video"),
                            ) {
                                Icon(CameraIcon, contentDescription = "Video call")
                            }
                            IconButton(
                                onClick = { editingAlias = true },
                                modifier = Modifier.testTag("edit-alias"),
                            ) {
                                Icon(Icons.Filled.Edit, contentDescription = "Set alias")
                            }
                        },
                    )
                    tab == MainTab.Chats && chatNav == ChatNav.NewChat -> TopAppBar(
                        navigationIcon = {
                            IconButton(onClick = { chatNav = ChatNav.List }) {
                                Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                            }
                        },
                        title = { Text("New chat") },
                    )
                    tab == MainTab.Chats && chatNav == ChatNav.Requests -> TopAppBar(
                        navigationIcon = {
                            IconButton(onClick = { chatNav = ChatNav.List }) {
                                Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                            }
                        },
                        title = { Text("Message requests") },
                    )
                    else -> CenterAlignedTopAppBar(
                        title = {
                            Text(
                                when (tab) {
                                    MainTab.Chats -> "Comrade"
                                    MainTab.Journal -> "Journal"
                                    MainTab.Feed -> "Feed"
                                    MainTab.Settings -> "Settings"
                                },
                            )
                        },
                    )
                }
            },
            bottomBar = {
                // The conversation view owns the whole screen, Telegram-style.
                if (openChat == null || tab != MainTab.Chats) {
                    NavigationBar {
                        MainTab.entries.forEach { t ->
                            NavigationBarItem(
                                selected = tab == t,
                                onClick = { tab = t },
                                icon = { Icon(t.icon, contentDescription = null) },
                                label = { Text(t.label) },
                            )
                        }
                    }
                }
            },
            floatingActionButton = {
                if (tab == MainTab.Chats && chatNav == ChatNav.List) {
                    FloatingActionButton(onClick = { chatNav = ChatNav.NewChat }) {
                        Icon(Icons.Filled.Create, contentDescription = "New chat")
                    }
                }
            },
        ) { padding ->
            val content = Modifier
                .fillMaxSize()
                .padding(padding)
            when (tab) {
                MainTab.Chats -> when (val nav = chatNav) {
                    ChatNav.List -> ChatsScreen(
                        chatTick = chatTick,
                        requestTick = requestTick,
                        onOpen = { peer, alias, username ->
                            chatNav = ChatNav.Open(peer, alias, username)
                        },
                        onNewChat = { chatNav = ChatNav.NewChat },
                        onOpenRequests = { chatNav = ChatNav.Requests },
                        modifier = content,
                    )
                    ChatNav.NewChat -> NewChatScreen(
                        onOpen = { peer, alias, username ->
                            chatNav = ChatNav.Open(peer, alias, username)
                        },
                        modifier = content,
                    )
                    ChatNav.Requests -> RequestsScreen(
                        chatTick = requestTick,
                        onOpen = { peer, alias, username ->
                            chatNav = ChatNav.Open(peer, alias, username)
                        },
                        modifier = content,
                    )
                    is ChatNav.Open -> ConversationScreen(
                        peer = nav.peer,
                        chatTick = chatTick,
                        modifier = content,
                    )
                }
                MainTab.Journal -> JournalScreen(modifier = content)
                MainTab.Feed -> FeedScreen(
                    feedItems = feedItems,
                    onPosted = { addToFeed(it, front = true) },
                    modifier = content,
                )
                MainTab.Settings -> SettingsScreen(
                    profile = profile,
                    onProfileChange = onProfileChange,
                    modifier = content,
                )
            }
        }
    }

        // Full-screen call overlay, above the app, whenever a call is live.
        if (callState.active) {
            CallScreen(onFinished = {})
        }
    }

    if (editingAlias && openChat != null) {
        EditAliasDialog(
            peer = openChat.peer,
            currentAlias = openChat.alias,
            onDismiss = { editingAlias = false },
            onSaved = { saved ->
                editingAlias = false
                chatNav = ChatNav.Open(
                    peer = openChat.peer,
                    alias = saved.alias.ifBlank { null },
                    username = openChat.username ?: saved.name,
                )
                chatTick++ // the chat list titles change too
            },
        )
    }
}

/**
 * Persistent off-grid mesh connectivity indicator, shown directly under the
 * top bar on every screen while the Saathi mDNS mesh is running. This is the
 * one signal that still works with zero cellular or relay reachability, so it
 * stays visible rather than a one-off toast — exactly what to check when
 * navigating somewhere with no signal at all.
 */
@Composable
private fun MeshStatusBanner() {
    val status by MeshStatusMonitor.status.collectAsState()
    if (!status.active) return

    val connected = status.peerCount > 0
    Surface(
        modifier = Modifier.fillMaxWidth(),
        color = if (connected) {
            MaterialTheme.colorScheme.primaryContainer
        } else {
            MaterialTheme.colorScheme.surfaceVariant
        },
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp),
        ) {
            Box(
                Modifier
                    .size(8.dp)
                    .background(
                        color = if (connected) {
                            MaterialTheme.colorScheme.primary
                        } else {
                            MaterialTheme.colorScheme.onSurfaceVariant
                        },
                        shape = CircleShape,
                    ),
            )
            Text(
                if (connected) {
                    "Local mesh · ${status.peerCount} nearby"
                } else {
                    "Local mesh · searching for nearby devices…"
                },
                style = MaterialTheme.typography.labelMedium,
                color = if (connected) {
                    MaterialTheme.colorScheme.onPrimaryContainer
                } else {
                    MaterialTheme.colorScheme.onSurfaceVariant
                },
            )
        }
    }
}

/**
 * The contact-alias editor: a local petname for this key, shown above their
 * self-published @handle. Clearing the field removes the alias.
 */
@Composable
private fun EditAliasDialog(
    peer: String,
    currentAlias: String?,
    onDismiss: () -> Unit,
    onSaved: (ComradeCore.ContactInfo) -> Unit,
) {
    var value by remember { mutableStateOf(currentAlias ?: "") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()

    AlertDialog(
        onDismissRequest = { if (!busy) onDismiss() },
        title = { Text("Alias for this contact") },
        text = {
            Column {
                OutlinedTextField(
                    value = value,
                    onValueChange = { value = it },
                    label = { Text("Alias") },
                    singleLine = true,
                    enabled = !busy,
                    modifier = Modifier
                        .fillMaxWidth()
                        .testTag("alias-input"),
                )
                Text(
                    "Only you see this name. It's pinned to the key " +
                        "${shortNpub(peer)} — leave it empty to fall back to " +
                        "their public username.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(top = 8.dp),
                )
                error?.let {
                    Text(
                        it,
                        color = MaterialTheme.colorScheme.error,
                        style = MaterialTheme.typography.bodySmall,
                        modifier = Modifier.padding(top = 4.dp),
                    )
                }
            }
        },
        confirmButton = {
            TextButton(
                enabled = !busy,
                onClick = {
                    busy = true
                    error = null
                    scope.launch {
                        runCatching {
                            withContext(Dispatchers.IO) {
                                ComradeCore.setContactAliasTyped(peer, value.trim())
                            }
                        }.onSuccess {
                            busy = false
                            onSaved(it)
                        }.onFailure {
                            busy = false
                            error = it.message ?: "Could not save."
                        }
                    }
                },
                modifier = Modifier.testTag("alias-save"),
            ) { Text(if (busy) "Saving…" else "Save") }
        },
        dismissButton = {
            TextButton(enabled = !busy, onClick = onDismiss) { Text("Cancel") }
        },
    )
}

/**
 * Non-blocking drain of the native event bus (bounded per round). Chitthis are
 * parsed for the feed; DM/media arrivals just signal "history changed" — the
 * chat screens reload from the Rust-side offline history, the source of truth.
 *
 * [ComradeCore.pollEvent] hands back a typed [BridgeEvent] straight from the
 * native core's push listener — no JSON parsing here any more.
 */
private fun drainEvents(max: Int = 200): List<PumpEvent> {
    val out = mutableListOf<PumpEvent>()
    repeat(max) {
        val event = ComradeCore.pollEvent() ?: return out
        when (event) {
            is BridgeEvent.IncomingChitthi -> out += PumpEvent.Chitthi(
                ComradeCore.ChitthiInfo(
                    id = event.v1.id,
                    author = event.v1.author,
                    content = event.v1.content,
                    createdAt = event.v1.createdAt.toLong(),
                    replyTo = event.v1.replyTo,
                ),
            )
            is BridgeEvent.IncomingDirectMessage -> out += PumpEvent.IncomingDm(
                peer = event.v1.sender,
                preview = event.v1.content.ifBlank { "New message" },
            )
            is BridgeEvent.IncomingMessageRequest -> out += PumpEvent.Request(
                peer = event.v1.peer,
                preview = event.v1.lastMessage.ifBlank { "New message request" },
            )
            is BridgeEvent.IncomingMedia -> out += PumpEvent.IncomingDm(
                peer = event.v1.sender,
                preview = "📎 " + event.v1.caption.ifBlank { "Attachment" },
            )
            // Receipt + profile updates only need a chat-list reload (ticks,
            // titles).
            is BridgeEvent.MessageStatus, is BridgeEvent.PeerProfileUpdated -> out += PumpEvent.HistoryChanged
            // Every call signal (offer/answer/ICE/hangup) is forwarded to the
            // call layer; the pump handler drives the WebRTC state machine and
            // raises a notification for a fresh offer.
            is BridgeEvent.IncomingCallSignal -> out += PumpEvent.CallSignalReceived(event.v1)
            is BridgeEvent.MeshStatusChanged -> out += PumpEvent.MeshStatusChanged(
                ComradeCore.MeshStatus(active = event.v1.active, peerCount = event.v1.peerCount.toInt()),
            )
            // Sakha/ledger sync isn't wired into the Android UI yet (desktop-only
            // via Tauri commands, see comrade_jni::lib.rs) — drop, like call signals.
            is BridgeEvent.LedgerUpdated -> Unit
        }
    }
    return out
}
