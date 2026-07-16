package mullu.comrade

import android.app.Activity
import android.content.Context
import android.os.Build
import android.os.Bundle
import android.util.Log
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
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import mullu.comrade.ui.ArticleIcon
import mullu.comrade.ui.BookIcon
import mullu.comrade.ui.CallHistoryScreen
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
import mullu.comrade.ui.CallIcon
import mullu.comrade.ui.VideocamIcon
import mullu.comrade.ui.theme.ComradeTheme
import mullu.comrade.call.CallManager
import mullu.comrade.call.CallScreen
import mullu.comrade.call.CallUiState
import mullu.comrade.call.Ringer
import uniffi.comrade_core.CallMediaKind

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

/**
 * Show (or stop showing) the whole activity over the lock screen and wake the
 * display — an incoming call rings and is answerable without first unlocking
 * the device, exactly like the platform dialer. `FLAG_SHOW_WHEN_LOCKED`/
 * `FLAG_TURN_SCREEN_ON` are deprecated in favour of `Activity.setShowWhenLocked`/
 * `setTurnScreenOn` (API 27+), but minSdk is 26 and the flags still work
 * correctly on every version this app supports, so a single code path is used
 * instead of an API-level branch.
 */
@Suppress("DEPRECATION")
private fun Activity.setShowOverLockScreen(show: Boolean) {
    val flags = WindowManager.LayoutParams.FLAG_SHOW_WHEN_LOCKED or WindowManager.LayoutParams.FLAG_TURN_SCREEN_ON
    if (show) window.addFlags(flags) else window.clearFlags(flags)
}

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

    // Start the background relay-connection service as soon as (and every
    // time) the vault is unlocked — including an activity recreation that
    // finds the runtime already unlocked, not just a fresh passphrase entry.
    // Starting it twice is harmless (RelayConnectionService no-ops a second
    // startForegroundService while already running); it is stopped
    // explicitly on lock, below.
    LaunchedEffect(phase is AppPhase.Ready) {
        if (phase is AppPhase.Ready) {
            // Off the main/Compose dispatcher: LaunchedEffect otherwise runs
            // this on the same thread Compose needs free to keep recomposing
            // and to answer test/semantics queries, and a foreground-service
            // start (context.startForegroundService, plus whatever the
            // platform does around it) has no need to be on it. Foreground
            // -service starts can also throw on platform restrictions
            // (background-start limits, quota, …) — never let that crash the
            // composition either; the app is just as usable without it, only
            // without the background-delivery guarantee. Matches
            // CallService.start's own guard in CallManager.setupPeer.
            withContext(Dispatchers.IO) {
                // Apply the baked-in default TURN relay (if the build has one and
                // the user hasn't set their own) now that the vault/store is open.
                CallRelayDefaults.seedIfNeeded(context)
                runCatching { RelayConnectionService.start(context) }
                    .onFailure { Log.w("ComradeApp", "Failed to start RelayConnectionService", it) }
            }
        }
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
            onLock = {
                RelayConnectionService.stop(context)
                phase = AppPhase.Locked(vaultExists = true)
            },
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
    data object CallHistory : ChatNav
    data class Open(
        val peer: String,
        /** User-chosen alias for the peer, when one exists. */
        val alias: String?,
        /** The peer's own published @handle, when known. */
        val username: String?,
    ) : ChatNav
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun MainShell(
    profile: ComradeCore.Profile,
    onProfileChange: (ComradeCore.Profile) -> Unit,
    onLock: () -> Unit,
) {
    val context = LocalContext.current
    val activity = context as? Activity
    var tab by rememberSaveable { mutableStateOf(MainTab.Chats) }
    var chatNav by remember { mutableStateOf<ChatNav>(ChatNav.List) }
    // Owned by RelayConnectionService/ChatEventRouter now — the single
    // consumer of the native event stream (see its doc comment) — rather
    // than each read locally here and reloaded by an Activity-scoped pump
    // loop, so a backgrounded Activity (or one recreated mid-session) never
    // duplicates, or simply stops, event handling.
    val chatTick by ChatEventRouter.chatTick.collectAsState()
    val requestTick by ChatEventRouter.requestTick.collectAsState()
    val feedItems by ChatEventRouter.feedItems.collectAsState()

    // Tell the router which conversation (if any) is on screen, so it can
    // suppress a DM notification for the thread the user is already
    // looking at — mirrors what the old in-Activity pump loop checked
    // inline before every notification.
    LaunchedEffect(chatNav) {
        ChatEventRouter.setOpenConversation((chatNav as? ChatNav.Open)?.peer)
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

    // ── Calls ─────────────────────────────────────────────────────────────────
    // A call needs the mic (and, for video, the camera) granted before capture.
    // We gate the runtime permission here, then run the deferred action.
    val callState by CallManager.state.collectAsState()
    var pendingCall by remember { mutableStateOf<(() -> Unit)?>(null) }
    val callPermissions = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions(),
    ) { grants ->
        val action = pendingCall
        pendingCall = null
        if (action != null && grants.values.all { it }) action()
    }
    fun withCallPermissions(video: Boolean, action: () -> Unit) {
        val needed = buildList {
            add(android.Manifest.permission.RECORD_AUDIO)
            if (video) add(android.Manifest.permission.CAMERA)
        }
        val missing = needed.filter {
            context.checkSelfPermission(it) != android.content.pm.PackageManager.PERMISSION_GRANTED
        }
        if (missing.isEmpty()) action() else {
            pendingCall = action
            callPermissions.launch(missing.toTypedArray())
        }
    }

    // Once the ring is answered/over, drop the incoming-call notification (leaving
    // message notifications untouched). The peer is only known off the ringing/
    // in-call states, so remember the last one to clear on the terminal states.
    // The same transitions also drive the ringtone/vibration (Ringer) and the
    // lock-screen bypass, so an incoming call rings and lights up the screen
    // even while the device is locked, exactly like a real phone call.
    var lastCallPeer by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(callState) {
        when (val st = callState) {
            is CallUiState.Ringing -> {
                lastCallPeer = st.peer
                if (st.incoming) {
                    Ringer.start(context)
                    activity?.setShowOverLockScreen(true)
                } else {
                    Ringer.stop()
                }
            }
            is CallUiState.Connecting -> {
                lastCallPeer = st.peer
                Notifier.clearCall(context, st.peer)
                Ringer.stop()
            }
            is CallUiState.Active -> {
                lastCallPeer = st.peer
                Notifier.clearCall(context, st.peer)
                Ringer.stop()
            }
            is CallUiState.Ended -> {
                // Missed from *this* device's perspective only when this device
                // was the callee and the ring timed out unanswered — the
                // caller's own unanswered outgoing call is not "missed" here.
                if (st.outcome == "missed" && st.incoming) {
                    Notifier.notifyMissedCall(context, st.peer, st.peerLabel)
                }
                lastCallPeer?.let { Notifier.clearCall(context, it) }
                Ringer.stop()
                activity?.setShowOverLockScreen(false)
            }
            CallUiState.Idle -> {
                lastCallPeer?.let { Notifier.clearCall(context, it) }
                Ringer.stop()
                activity?.setShowOverLockScreen(false)
            }
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
                            val callLabel = peerTitle(openChat.peer, openChat.alias, openChat.username)
                            IconButton(onClick = {
                                withCallPermissions(video = false) {
                                    CallManager.startOutgoingCall(
                                        context, openChat.peer, callLabel, CallMediaKind.AUDIO,
                                    )
                                }
                            }) {
                                Icon(CallIcon, contentDescription = "Voice call")
                            }
                            IconButton(onClick = {
                                withCallPermissions(video = true) {
                                    CallManager.startOutgoingCall(
                                        context, openChat.peer, callLabel, CallMediaKind.VIDEO,
                                    )
                                }
                            }) {
                                Icon(VideocamIcon, contentDescription = "Video call")
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
                    tab == MainTab.Chats && chatNav == ChatNav.CallHistory -> TopAppBar(
                        navigationIcon = {
                            IconButton(onClick = { chatNav = ChatNav.List }) {
                                Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                            }
                        },
                        title = { Text(stringResource(R.string.call_history_title)) },
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
                        onOpenCallHistory = { chatNav = ChatNav.CallHistory },
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
                    ChatNav.CallHistory -> CallHistoryScreen(
                        onCallBack = { peer, peerLabel, video ->
                            withCallPermissions(video) {
                                CallManager.startOutgoingCall(
                                    context, peer, peerLabel,
                                    if (video) CallMediaKind.VIDEO else CallMediaKind.AUDIO,
                                )
                            }
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
                    onPosted = { ChatEventRouter.addChitthi(it, front = true) },
                    modifier = content,
                )
                MainTab.Settings -> SettingsScreen(
                    profile = profile,
                    onProfileChange = onProfileChange,
                    onLock = onLock,
                    modifier = content,
                )
            }
        }
        }
        // Call overlay — covers the app while a call is ringing/connected.
        CallScreen(onAccept = {
            (CallManager.state.value as? CallUiState.Ringing)?.let { ringing ->
                withCallPermissions(ringing.video) { CallManager.accept(context) }
            }
        })
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
                ChatEventRouter.bumpChatTick() // the chat list titles change too
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

