package mullu.comrade

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.view.WindowManager
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Create
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Lock
import androidx.compose.material.icons.filled.Menu
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.CenterAlignedTopAppBar
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationDrawerItem
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.rememberDrawerState
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import mullu.comrade.ble.BleMeshService
import mullu.comrade.ui.ArticleIcon
import mullu.comrade.ui.BookIcon
import mullu.comrade.ui.BreathingScreen
import mullu.comrade.ui.CallHistoryScreen
import mullu.comrade.ui.ChatBubbleIcon
import mullu.comrade.ui.ChatMenuAction
import mullu.comrade.ui.ChatsScreen
import mullu.comrade.ui.ComradesScreen
import mullu.comrade.ui.ConversationScreen
import mullu.comrade.ui.CloudIcon
import mullu.comrade.ui.CopyIcon
import mullu.comrade.ui.conversationMenu
import mullu.comrade.ui.ExploreIcon
import mullu.comrade.ui.FeedScreen
import mullu.comrade.ui.FocusScreen
import mullu.comrade.ui.HeartIcon
import mullu.comrade.ui.JournalScreen
import mullu.comrade.ui.NewChatScreen
import mullu.comrade.ui.NotificationsIcon
import mullu.comrade.ui.NotificationsOffIcon
import mullu.comrade.ui.OnboardingScreen
import mullu.comrade.ui.PeerAvatar
import mullu.comrade.ui.PresenceDot
import mullu.comrade.ui.presenceText
import mullu.comrade.ui.ReaderScreen
import mullu.comrade.ui.RequestsScreen
import mullu.comrade.ui.SettingsScreen
import mullu.comrade.ui.StarIcon
import mullu.comrade.ui.StretchScreen
import mullu.comrade.ui.StarOutlineIcon
import mullu.comrade.ui.TaraScreen
import mullu.comrade.ui.TaskListScreen
import mullu.comrade.ui.MotorcycleIcon
import mullu.comrade.ui.PeopleHugIcon
import mullu.comrade.ui.TimerIcon
import mullu.comrade.ui.peerTitle
import mullu.comrade.ui.purgeDecryptedMedia
import mullu.comrade.ui.shortNpub
import mullu.comrade.ui.CallIcon
import mullu.comrade.ui.LocalMeshState
import mullu.comrade.ui.TransportPrecedence
import mullu.comrade.ui.TransportRoute
import mullu.comrade.ui.VideocamIcon
import mullu.comrade.ui.WifiTetheringIcon
import mullu.comrade.ui.theme.ComradeRadii
import mullu.comrade.ui.theme.ComradeTheme
import mullu.comrade.ui.theme.GlassElevation
import mullu.comrade.ui.theme.comradeFocusRing
import mullu.comrade.ui.theme.glassCenterAlignedTopAppBarColors
import mullu.comrade.ui.theme.glassSurface
import mullu.comrade.ui.theme.glassTopAppBarColors
import mullu.comrade.update.UpdateChecker
import mullu.comrade.call.CallManager
import mullu.comrade.call.CallScreen
import mullu.comrade.call.CallUiState
import mullu.comrade.call.PipController
import mullu.comrade.call.Ringer
import uniffi.comrade_core.CallMediaKind

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Screenshots and screen recording work unless the user turned them off
        // in Settings. Applied before the first frame, because a window that
        // starts unprotected has already been captured by the recents
        // thumbnail. See [ScreenSecurity] for why the unconditional FLAG_SECURE
        // this used to set is gone.
        ScreenSecurity.applyPreference(this)
        // A tapped notification asks for a specific tab (or Settings), and a
        // tapped message notification asks for a conversation; park both so the
        // shell honours them once it exists (the vault may still need unlocking
        // first).
        AppNavigation.request(intent?.getStringExtra(AppNavigation.EXTRA_OPEN_TAB))
        AppNavigation.requestPeer(intent?.getStringExtra(AppNavigation.EXTRA_OPEN_PEER))
        intent?.let { offerSharedText(it) }
        // Picture-in-picture for a live video call — see [PipController]. The
        // Activity is the only thing that receives the PiP lifecycle callbacks.
        PipController.attachActivity(this)
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
     * Leaving the app during a video call floats it into a PiP window instead
     * of stopping it (pre-31; from 31 up the platform's own auto-enter flag,
     * which [PipController.applyAutoEnter] keeps current, handles it).
     */
    override fun onUserLeaveHint() {
        super.onUserLeaveHint()
        PipController.onUserLeaving()
    }

    override fun onPictureInPictureModeChanged(
        isInPictureInPictureMode: Boolean,
        newConfig: android.content.res.Configuration,
    ) {
        super.onPictureInPictureModeChanged(isInPictureInPictureMode, newConfig)
        PipController.onPipModeChanged(isInPictureInPictureMode)
        PipController.onWindowVisibilityChanged(isInPictureInPictureMode)
    }

    override fun onDestroy() {
        PipController.detachActivity(this)
        super.onDestroy()
    }

    /**
     * The activity is `singleTop`-launched from notifications, so a tap while
     * it is already running arrives here rather than in [onCreate].
     */
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        AppNavigation.request(intent.getStringExtra(AppNavigation.EXTRA_OPEN_TAB))
        AppNavigation.requestPeer(intent.getStringExtra(AppNavigation.EXTRA_OPEN_PEER))
        offerSharedText(intent)
    }

    /**
     * Text arriving from the system share sheet — the way into the reading
     * library from every other app. Parked in [AppNavigation] like the tab
     * requests, because the share may arrive with the vault still locked; the
     * shell then lands the user on the reader with the text *offered*, never
     * silently saved.
     */
    private fun offerSharedText(intent: Intent) {
        if (intent.action != Intent.ACTION_SEND) return
        if (intent.type?.startsWith("text/") != true) return
        AppNavigation.requestSharedText(
            title = intent.getStringExtra(Intent.EXTRA_SUBJECT)
                ?: intent.getStringExtra(Intent.EXTRA_TITLE),
            text = intent.getStringExtra(Intent.EXTRA_TEXT),
        )
    }

    /**
     * Becoming visible does two things.
     *
     * **It drains the native event queue.** While an Activity is on screen
     * this process needs [EventPump] running — that is what makes a message,
     * a call or a comrade coming online show up *now*. Deliberately not tied
     * to [RelayConnectionService]: that service is gated on the user's "stay
     * connected in the background" preference, and while it also owned the
     * drain loop, turning the preference off silently stopped delivery even
     * with the app open. The pump refcounts its holders, so exactly one loop
     * runs whether it is this Activity, the service, or both. Acquiring here
     * rather than after unlock is safe: with a locked vault nothing produces
     * events, so the loop is an idle tick.
     *
     * **It tells the comrades we are online**, and [onStop] tells them we are
     * not. That pairing is the whole definition: *online means the app is
     * open*. A phone in a pocket with the connection service running is still
     * receiving messages, but its owner is not at it — showing a green dot
     * for that is precisely the small lie this feature exists to avoid, and
     * it is what a user reported seeing.
     *
     * The native heartbeat refreshes the claim only while this flag holds
     * (see `ComradeRuntime::presence_active`), so a backgrounded app goes
     * quiet rather than re-announcing itself a few minutes later.
     *
     * Runs on the application scope, off the main thread: the beacon send is a
     * relay round-trip per comrade.
     */
    override fun onStart() {
        super.onStart()
        EventPump.acquire(this, PumpHolder.FOREGROUND)
        // "The conversation is on screen" is only a reason to skip a
        // notification while there *is* a screen — see
        // NotificationPolicy.messageDecision.
        ChatEventRouter.setAppVisible(true)
        // At most one request a day, and none at all with the preference off
        // (see UpdateChecker) — a sideloaded app otherwise has no way to tell
        // anyone that a fix shipped.
        UpdateChecker.maybeCheck(this)
        // The video surface is back: resume capture if a video call had it
        // suspended (a no-op otherwise, and idempotent — see PipController).
        PipController.onWindowVisibilityChanged(visible = true)
        announcePresence(online = true)
    }

    /**
     * Tell the comrades whether this device is at the user's attention right
     * now. Fire-and-forget on the application scope so a relay round-trip
     * never runs on a lifecycle callback, and silent on failure — presence is
     * a courtesy, and the beacon's own TTL covers a goodbye that never
     * lands.
     */
    private fun announcePresence(online: Boolean) {
        val app = application as? ComradeApplication ?: return
        app.appScope.launch(Dispatchers.IO) {
            if (ComradeCore.isVaultUnlocked()) {
                runCatching { ComradeCore.announcePresenceTyped(online = online) }
                    .onFailure { Log.w("ComradeApp", "presence announce failed", it) }
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
        // Nothing visible any more: the service keeps the drain loop alive if
        // the user wants background delivery, and the pump stops it if not.
        EventPump.release(PumpHolder.FOREGROUND)
        // Nothing is on screen, so nothing counts as already-read: a message
        // from the thread that was open must notify again from here.
        ChatEventRouter.setAppVisible(false)
        // …and the comrades are told, because "online" means the app is open.
        // Delivery continues (that is the service's job); what stops is the
        // claim that anyone is at the phone. A PiP video call does not reach
        // here — the Activity stays started — so a call still reads as online,
        // which is true.
        announcePresence(online = false)
        // Nothing is displaying the local video any more — stop capturing it
        // (unless a PiP window is showing the call). See PipController.
        PipController.onWindowVisibilityChanged(visible = false)
        purgeDecryptedMedia(this)
    }
}

/** Where the encrypted vault lives on this device. */
internal fun vaultPath(context: Context): File = File(context.filesDir, "comrade-vault")

/**
 * Keep a line from a focus session or a finished read as a journal entry.
 *
 * Saves the *prompt* rather than opening a pre-filled composer, deliberately:
 * the reflection is the invitation, and the user's own answer belongs in their
 * own words on the Journal tab whenever they get to it. Failure is silent —
 * missing a suggested note is not worth an error dialog after a session someone
 * just finished.
 */
private fun seedJournalNote(scope: kotlinx.coroutines.CoroutineScope, line: String) {
    if (line.isBlank()) return
    scope.launch(Dispatchers.IO) {
        runCatching { ComradeCore.addJournalEntryTyped(line, null) }
            .onFailure { Log.w("ComradeApp", "could not save the session note", it) }
    }
}

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
            // The store is readable now, so fill the feed/mesh/presence state
            // the screens observe. Independent of the service: with background
            // connectivity turned off there is no service to do it.
            ChatEventRouter.seedFromStore()
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
                // Comrades' dots are derived from an unlocked store; drop them
                // with it rather than letting a stale green dot outlive the
                // lock. (The native side has already told those comrades we are
                // going offline — see `ComradeRuntime::lock_vault`.)
                PresenceMonitor.clear()
                phase = AppPhase.Locked(vaultExists = true)
            },
        )
    }
}

// ── Main shell: Chats · Journal · Feed · Tara (Settings & Call history via the drawer) ─

/**
 * Bottom-navigation destinations, in on-screen order. Tara sits **last**
 * (rightmost) deliberately: the messaging/journal/feed tabs are the daily
 * surfaces, and the companion is the one you reach for on purpose. Focus sits
 * next to it for the same reason — both are things you go to deliberately,
 * never things that should be competing for a glance.
 */
private enum class MainTab(val label: String, val icon: ImageVector) {
    Chats("Chats", ChatBubbleIcon),
    Journal("Journal", BookIcon),
    // Took the slot Feed had. Feed is not gone — it moved to the drawer, because
    // a public feed is somewhere you go and listening with someone is something
    // you do *with* the person you are already talking to, which makes it a
    // daily surface and the feed a deliberate one.
    //
    // 🫂 rather than a note over a list, since 2026-08-08 — when this tab stopped
    // being a screen you arrived at and became the only way in. A playlist glyph
    // named the medium; every other icon on this bar names the people it is for,
    // and this feature is about the person more than the music.
    Together("Together", PeopleHugIcon),
    Focus("Focus", TimerIcon),
    // Sixth slot, added 2026-08-16 with the Travel guide. A NavigationBar is
    // comfortable at five and tight at six — labels shrink rather than wrap —
    // and this took the sixth rather than pushing an existing tab into the
    // drawer, because deciding which of the five is least used is not a call
    // this change is entitled to make. If a seventh is ever wanted, that
    // decision has to be made first.
    Travel("Travel", ExploreIcon),
    Tara("Tara", HeartIcon),
}

/** Sub-navigation inside the Focus tab. */
private sealed interface FocusNav {
    data object Sessions : FocusNav
    data object Reader : FocusNav
    data object Breathing : FocusNav
    data object Stretch : FocusNav
}

/** Sub-navigation inside the Chats tab. */
private sealed interface ChatNav {
    data object List : ChatNav
    data object NewChat : ChatNav
    data object Requests : ChatNav
    data object CallHistory : ChatNav
    data object Comrades : ChatNav
    data object Tasks : ChatNav
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
    var focusNav by remember { mutableStateOf<FocusNav>(FocusNav.Sessions) }
    // Settings is a pushed screen (Telegram-style), reached from the drawer,
    // not a bottom-nav tab. The drawer is the app-wide navigation menu.
    var settingsOpen by rememberSaveable { mutableStateOf(false) }
    /// Feed is a pushed screen now, reached from the drawer — see [MainTab].
    var feedOpen by rememberSaveable { mutableStateOf(false) }
    var rideOpen by rememberSaveable { mutableStateOf(false) }
    val drawerState = rememberDrawerState(DrawerValue.Closed)
    val scope = rememberCoroutineScope()
    // Owned by RelayConnectionService/ChatEventRouter now — the single
    // consumer of the native event stream (see its doc comment) — rather
    // than each read locally here and reloaded by an Activity-scoped pump
    // loop, so a backgrounded Activity (or one recreated mid-session) never
    // duplicates, or simply stops, event handling.
    val chatTick by ChatEventRouter.chatTick.collectAsState()
    // Separate from `chatTick` on purpose — see `ChatEventRouter.topicTick`.
    val topicTick by ChatEventRouter.topicTick.collectAsState()
    val requestTick by ChatEventRouter.requestTick.collectAsState()
    val feedItems by ChatEventRouter.feedItems.collectAsState()

    // Tell the router which conversation (if any) is on screen, so it can
    // suppress a DM notification for the thread the user is already
    // looking at — mirrors what the old in-Activity pump loop checked
    // inline before every notification.
    LaunchedEffect(chatNav) {
        ChatEventRouter.setOpenConversation((chatNav as? ChatNav.Open)?.peer)
    }

    // Honour a tab requested from outside the composition — a tapped
    // "the companion model is ready" notification lands the user back in the
    // Tara conversation (see AppNavigation).
    val requestedTab by AppNavigation.requestedTab.collectAsState()
    LaunchedEffect(requestedTab) {
        val key = requestedTab ?: return@LaunchedEffect
        when {
            // Settings is a pushed screen, not a tab — an update notice lands
            // on the card that offers the update.
            key.equals(AppNavigation.SCREEN_SETTINGS, ignoreCase = true) -> settingsOpen = true
            key.equals(AppNavigation.SCREEN_REQUESTS, ignoreCase = true) -> {
                tab = MainTab.Chats
                chatNav = ChatNav.Requests
                settingsOpen = false
            }
            else -> MainTab.entries.firstOrNull { it.name.equals(key, ignoreCase = true) }?.let {
                tab = it
                if (it == MainTab.Chats) chatNav = ChatNav.List
                settingsOpen = false
            }
        }
        AppNavigation.consume()
    }

    // A tapped message notification names a conversation: open it (WP11).
    // The peer key is all the intent carries, so the title is resolved the same
    // way the shade resolved it — off the main thread, because it reads the
    // contact/conversation trees — and the notification for that thread is
    // cleared, exactly as opening the chat from the list does.
    val requestedPeer by AppNavigation.requestedPeer.collectAsState()
    LaunchedEffect(requestedPeer) {
        val peer = requestedPeer ?: return@LaunchedEffect
        val label = withContext(Dispatchers.IO) {
            runCatching { ChatEventRouter.peerLabel(peer) }.getOrDefault("")
        }
        tab = MainTab.Chats
        chatNav = ChatNav.Open(peer = peer, alias = label.ifBlank { null }, username = null)
        settingsOpen = false
        AppNavigation.consumePeer()
    }

    // Text shared into Comrade from another app lands on the reader with the
    // compose box prefilled — offered, never silently saved. The flow is
    // consumed by the ReaderScreen once it has taken the text, so a
    // configuration change doesn't re-fill a box the user already edited.
    val sharedText by AppNavigation.sharedText.collectAsState()
    LaunchedEffect(sharedText) {
        if (sharedText != null) {
            tab = MainTab.Focus
            focusNav = FocusNav.Reader
            settingsOpen = false
            feedOpen = false
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


    // ── Calls ─────────────────────────────────────────────────────────────────
    // A call needs the mic (and, for video, the camera) granted before capture.
    // We gate the runtime permission here, then run the deferred action.
    // Watch/listen together: pick a local file both sides already have, then
    // either open a session or join the one we were invited to. The Rust core
    // never learns the filename — only the length, and whatever label the user
    // chose (docs/TOGETHER.md §2).
    val togetherState by mullu.comrade.together.TogetherManager.state.collectAsState()
    var togetherPeer by remember { mutableStateOf<Pair<String, String>?>(null) }
    val togetherPicker = rememberLauncherForActivityResult(
        ActivityResultContracts.OpenDocument(),
    ) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        runCatching {
            context.contentResolver.takePersistableUriPermission(
                uri,
                Intent.FLAG_GRANT_READ_URI_PERMISSION,
            )
        }
        val invited = mullu.comrade.together.TogetherManager.state.value
        if (invited is mullu.comrade.together.TogetherManager.UiState.Invited) {
            mullu.comrade.together.TogetherManager.join(context, uri)
        } else {
            togetherPeer?.let { (peer, label) ->
                // Name it from the file's own tags where it has them, so the
                // other side can find their copy instead of hunting for it.
                // Never from the filename — that is the one piece of local
                // filesystem vocabulary the sender never chose to share.
                mullu.comrade.together.TogetherManager.start(
                    context = context,
                    peer = peer,
                    peerLabel = label,
                    uri = uri,
                    recording = mullu.comrade.together.LibraryResolver.describe(context, uri),
                )
            }
        }
    }

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
    // Overflow-menu surfaces. Keyed on the peer so switching conversations can
    // never leave a sheet or a Block confirmation pointed at the previous one.
    var chatMenuOpen by remember(openChat?.peer) { mutableStateOf(false) }
    var showEncryption by remember(openChat?.peer) { mutableStateOf(false) }
    var confirmBlock by remember(openChat?.peer) { mutableStateOf(false) }
    val clipboard = LocalClipboardManager.current
    // Whether the open conversation's peer is a comrade, and whether they are
    // around — read from the store on open (and after a toggle), with the dot
    // then following the live presence flow.
    var comradeToggleTick by remember { mutableStateOf(0) }
    var isComrade by remember(openChat?.peer) { mutableStateOf(false) }
    LaunchedEffect(openChat?.peer, comradeToggleTick) {
        val peer = openChat?.peer
        isComrade = peer != null && withContext(Dispatchers.IO) {
            runCatching { ComradeCore.comrades().any { it.npub == peer } }.getOrDefault(false)
        }
    }
    // Whether this conversation is muted. A plain preference read rather than a
    // flow: it only ever changes from the menu below, and the tick re-reads it.
    var muteToggleTick by remember { mutableStateOf(0) }
    val isMuted = remember(openChat?.peer, muteToggleTick) {
        openChat?.peer?.let { MutedChats.isMuted(context, it) } ?: false
    }
    val presenceNow by PresenceMonitor.presence.collectAsState()
    val peerPresence = openChat?.peer?.let { presenceNow[it] }
    val comradeOnline = peerPresence?.online == true
    // Back priority, innermost first: an open drawer closes, then a pushed
    // Settings screen closes, then a Chats sub-screen returns to the list.
    BackHandler(
        enabled = drawerState.isOpen ||
            settingsOpen || feedOpen || rideOpen ||
            (tab == MainTab.Chats && chatNav != ChatNav.List) ||
            (tab == MainTab.Focus && focusNav != FocusNav.Sessions),
    ) {
        when {
            drawerState.isOpen -> scope.launch { drawerState.close() }
            settingsOpen -> settingsOpen = false
            feedOpen -> feedOpen = false
            rideOpen -> rideOpen = false
            tab == MainTab.Focus -> focusNav = FocusNav.Sessions
            else -> chatNav = ChatNav.List
        }
    }

    Box(modifier = Modifier.fillMaxSize()) {
        if (feedOpen) {
            // Reached from the drawer since Together took the bottom-nav slot.
            // A pushed screen with its own back arrow rather than a tab, because
            // that is what the drawer's other destinations are.
            FeedPushedScreen(
                feedItems = feedItems,
                onBack = { feedOpen = false },
                onOpenJournal = {
                    feedOpen = false
                    tab = MainTab.Journal
                },
                onOpenBreathing = {
                    feedOpen = false
                    tab = MainTab.Focus
                    focusNav = FocusNav.Breathing
                },
            )
        } else if (rideOpen) {
            // Pushed, with its own back arrow, like Feed — see the drawer item.
            RidePushedScreen(onBack = { rideOpen = false })
        } else if (settingsOpen) {
            SettingsPushedScreen(
                profile = profile,
                onProfileChange = onProfileChange,
                onLock = onLock,
                onBack = { settingsOpen = false },
            )
        } else {
            ModalNavigationDrawer(
                drawerState = drawerState,
                drawerContent = {
                    ComradeDrawerSheet(
                        profile = profile,
                        onOpenSettings = {
                            scope.launch { drawerState.close() }
                            settingsOpen = true
                        },
                        onOpenCallHistory = {
                            scope.launch { drawerState.close() }
                            tab = MainTab.Chats
                            chatNav = ChatNav.CallHistory
                        },
                        onOpenComrades = {
                            scope.launch { drawerState.close() }
                            tab = MainTab.Chats
                            chatNav = ChatNav.Comrades
                        },
                        onOpenTasks = {
                            scope.launch { drawerState.close() }
                            tab = MainTab.Chats
                            chatNav = ChatNav.Tasks
                        },
                        onOpenFeed = {
                            scope.launch { drawerState.close() }
                            feedOpen = true
                        },
                        onOpenRide = {
                            scope.launch { drawerState.close() }
                            rideOpen = true
                        },
                    )
                },
            ) {
                Column(modifier = Modifier.fillMaxSize()) {
                Scaffold(
                    modifier = Modifier.weight(1f),
                    topBar = {
                        when {
                            tab == MainTab.Chats && openChat != null -> TopAppBar(
                                modifier = Modifier.glassSurface(GlassElevation.Chrome),
                                colors = glassTopAppBarColors(),
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
                                        Box(contentAlignment = Alignment.BottomEnd) {
                                            PeerAvatar(title, seed = openChat.peer, size = 36.dp)
                                            if (isComrade) PresenceDot(comradeOnline, size = 10.dp)
                                        }
                                        Column {
                                            Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                            // Presence is the only thing under the name —
                                            // "online" while they are, a Telegram-style
                                            // "last seen …" once they aren't, and nothing
                                            // at all for someone who isn't a comrade.
                                            //
                                            // The key used to live here too, on the grounds
                                            // that handles are claims and keys are identity.
                                            // It now lives one tap away in the ⋮ menu (Copy
                                            // key, and Encryption info shows it in full),
                                            // which is a better home for it: the header
                                            // reads like a conversation instead of a key
                                            // dump, and the line no longer has to choose
                                            // between the key and what presence has to say.
                                            if (isComrade) {
                                                Text(
                                                    presenceText(
                                                        online = comradeOnline,
                                                        lastSeenAt = peerPresence?.lastSeenAt ?: 0L,
                                                        peerMarkedUs = peerPresence?.peerMarkedUs ?: false,
                                                    ),
                                                    style = MaterialTheme.typography.labelSmall,
                                                    maxLines = 1,
                                                    overflow = TextOverflow.Ellipsis,
                                                    color = if (comradeOnline) {
                                                        MaterialTheme.colorScheme.primary
                                                    } else {
                                                        MaterialTheme.colorScheme.onSurfaceVariant
                                                    },
                                                )
                                            }
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
                                    // No ▶ here any more. Listening together is
                                    // one flow in the Together tab (🫂) — pick
                                    // something, then pick who — because two
                                    // entry points meant "how do I listen with
                                    // someone" depended on which screen you were
                                    // on, and the one in this header could only
                                    // ever offer a file. The chat keeps `/play`,
                                    // which reaches the same session by the same
                                    // path.
                                    IconButton(onClick = {
                                        withCallPermissions(video = true) {
                                            CallManager.startOutgoingCall(
                                                context, openChat.peer, callLabel, CallMediaKind.VIDEO,
                                            )
                                        }
                                    }) {
                                        Icon(VideocamIcon, contentDescription = "Video call")
                                    }
                                    // Everything that isn't a call lives behind ⋮, so the bar
                                    // stays readable as options grow (the alias pencil and the
                                    // comrade star used to sit out here).
                                    Box {
                                        IconButton(
                                            onClick = { chatMenuOpen = true },
                                            modifier = Modifier.testTag("chat-menu"),
                                        ) {
                                            Icon(
                                                Icons.Filled.MoreVert,
                                                contentDescription = stringResource(R.string.chat_menu),
                                            )
                                        }
                                        DropdownMenu(
                                            expanded = chatMenuOpen,
                                            onDismissRequest = { chatMenuOpen = false },
                                        ) {
                                            conversationMenu(isComrade, isMuted).forEach { action ->
                                                ChatMenuRow(
                                                    action = action,
                                                    onClick = {
                                                        chatMenuOpen = false
                                                        when (action) {
                                                            ChatMenuAction.SetAlias ->
                                                                editingAlias = true
                                                            ChatMenuAction.Mute,
                                                            ChatMenuAction.Unmute,
                                                            -> {
                                                                MutedChats.setMuted(
                                                                    context,
                                                                    openChat.peer,
                                                                    !isMuted,
                                                                )
                                                                muteToggleTick++
                                                                if (!isMuted) {
                                                                    // Muting with a notice already
                                                                    // in the shade would otherwise
                                                                    // leave the buzz it was meant
                                                                    // to stop sitting there.
                                                                    Notifier.clearForPeer(
                                                                        context,
                                                                        openChat.peer,
                                                                    )
                                                                }
                                                            }
                                                            ChatMenuAction.AddComrade,
                                                            ChatMenuAction.RemoveComrade,
                                                            -> scope.launch {
                                                                val saved = withContext(Dispatchers.IO) {
                                                                    runCatching {
                                                                        ComradeCore.setComradeTyped(
                                                                            openChat.peer,
                                                                            !isComrade,
                                                                        )
                                                                    }.getOrNull()
                                                                }
                                                                if (saved != null) {
                                                                    comradeToggleTick++
                                                                    // Chat-list rows carry the dot too.
                                                                    ChatEventRouter.bumpChatTick()
                                                                }
                                                            }
                                                            ChatMenuAction.CopyKey -> {
                                                                clipboard.setText(
                                                                    AnnotatedString(openChat.peer),
                                                                )
                                                                // Android 13+ shows its own clipboard
                                                                // confirmation; a Toast there would
                                                                // just say it twice.
                                                                if (Build.VERSION.SDK_INT <
                                                                    Build.VERSION_CODES.TIRAMISU
                                                                ) {
                                                                    Toast.makeText(
                                                                        context,
                                                                        R.string.chat_menu_copied,
                                                                        Toast.LENGTH_SHORT,
                                                                    ).show()
                                                                }
                                                            }
                                                            ChatMenuAction.EncryptionInfo ->
                                                                showEncryption = true
                                                            ChatMenuAction.Block ->
                                                                confirmBlock = true
                                                        }
                                                    },
                                                )
                                            }
                                        }
                                    }
                                },
                            )
                            tab == MainTab.Chats && chatNav == ChatNav.NewChat -> TopAppBar(
                                modifier = Modifier.glassSurface(GlassElevation.Chrome),
                                colors = glassTopAppBarColors(),
                                navigationIcon = {
                                    IconButton(onClick = { chatNav = ChatNav.List }) {
                                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                                    }
                                },
                                title = { Text("New chat") },
                            )
                            tab == MainTab.Chats && chatNav == ChatNav.Requests -> TopAppBar(
                                modifier = Modifier.glassSurface(GlassElevation.Chrome),
                                colors = glassTopAppBarColors(),
                                navigationIcon = {
                                    IconButton(onClick = { chatNav = ChatNav.List }) {
                                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                                    }
                                },
                                title = { Text("Message requests") },
                            )
                            tab == MainTab.Chats && chatNav == ChatNav.CallHistory -> TopAppBar(
                                modifier = Modifier.glassSurface(GlassElevation.Chrome),
                                colors = glassTopAppBarColors(),
                                navigationIcon = {
                                    IconButton(onClick = { chatNav = ChatNav.List }) {
                                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                                    }
                                },
                                title = { Text(stringResource(R.string.call_history_title)) },
                            )
                            tab == MainTab.Chats && chatNav == ChatNav.Comrades -> TopAppBar(
                                modifier = Modifier.glassSurface(GlassElevation.Chrome),
                                colors = glassTopAppBarColors(),
                                navigationIcon = {
                                    IconButton(onClick = { chatNav = ChatNav.List }) {
                                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                                    }
                                },
                                title = { Text(stringResource(R.string.comrades_title)) },
                            )
                            tab == MainTab.Chats && chatNav == ChatNav.Tasks -> TopAppBar(
                                modifier = Modifier.glassSurface(GlassElevation.Chrome),
                                colors = glassTopAppBarColors(),
                                navigationIcon = {
                                    IconButton(onClick = { chatNav = ChatNav.List }) {
                                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                                    }
                                },
                                title = { Text(stringResource(R.string.tasks_title)) },
                            )
                            tab == MainTab.Chats && chatNav == ChatNav.List -> CenterAlignedTopAppBar(
                                modifier = Modifier.glassSurface(GlassElevation.Chrome),
                                colors = glassCenterAlignedTopAppBarColors(),
                                navigationIcon = {
                                    IconButton(
                                        onClick = { scope.launch { drawerState.open() } },
                                        modifier = Modifier.testTag("nav-drawer-button"),
                                    ) {
                                        Icon(Icons.Filled.Menu, contentDescription = "Open navigation menu")
                                    }
                                },
                                title = { Text("Comrade") },
                                // Exactly opposite the hamburger: which route a
                                // message takes is a property of the app, not of
                                // the screen you happen to be on.
                                actions = { TransportPrecedenceAction() },
                            )
                            // A Focus sub-screen gets a back arrow to the
                            // session list, like the Chats sub-screens do.
                            tab == MainTab.Focus && focusNav != FocusNav.Sessions -> TopAppBar(
                                modifier = Modifier.glassSurface(GlassElevation.Chrome),
                                colors = glassTopAppBarColors(),
                                navigationIcon = {
                                    IconButton(onClick = { focusNav = FocusNav.Sessions }) {
                                        Icon(
                                            Icons.AutoMirrored.Filled.ArrowBack,
                                            contentDescription = "Back",
                                        )
                                    }
                                },
                                title = {
                                    Text(
                                        stringResource(
                                            when (focusNav) {
                                                FocusNav.Reader -> R.string.reader_title
                                                FocusNav.Stretch -> R.string.stretch_title
                                                else -> R.string.breathe_title
                                            },
                                        ),
                                    )
                                },
                            )
                            else -> CenterAlignedTopAppBar(
                                modifier = Modifier.glassSurface(GlassElevation.Chrome),
                                colors = glassCenterAlignedTopAppBarColors(),
                                title = {
                                    Text(
                                        when (tab) {
                                            MainTab.Chats -> "Comrade"
                                            MainTab.Journal -> "Journal"
                                            MainTab.Tara -> "Tara"
                                            MainTab.Together -> "Together"
                                            MainTab.Focus -> stringResource(R.string.attention_tab)
                                            MainTab.Travel -> stringResource(R.string.travel_tab)
                                        },
                                    )
                                },
                                actions = { TransportPrecedenceAction() },
                            )
                        }
                    },
                    bottomBar = {
                        // The conversation view owns the whole screen, Telegram-style.
                        if (openChat == null || tab != MainTab.Chats) {
                            NavigationBar(
                                modifier = Modifier.glassSurface(GlassElevation.Chrome),
                                containerColor = Color.Transparent,
                            ) {
                                MainTab.entries.forEach { t ->
                                    NavigationBarItem(
                                        selected = tab == t,
                                        onClick = { tab = t },
                                        icon = { Icon(t.icon, contentDescription = null) },
                                        label = { Text(t.label) },
                                        // §4: a visible ring on focus — TV
                                        // remote/keyboard/switch-access
                                        // navigate this bar by focus, not touch.
                                        modifier = Modifier.comradeFocusRing(),
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
                            ChatNav.Comrades -> ComradesScreen(
                                chatTick = chatTick,
                                onOpen = { peer, alias, username ->
                                    chatNav = ChatNav.Open(peer, alias, username)
                                },
                                modifier = content,
                            )
                            ChatNav.Tasks -> TaskListScreen(modifier = content)
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
                                topicTick = topicTick,
                                // Same launcher the Together button in the chat
                                // header uses, so a file arriving from `/play`
                                // and one picked by hand start a session by the
                                // identical path — including the persistable
                                // read permission, which a second launcher
                                // would have had to remember to take.
                                onPickTogetherFile = { peer, label ->
                                    togetherPeer = peer to label
                                    togetherPicker.launch(arrayOf("video/*", "audio/*"))
                                },
                                modifier = content,
                            )
                        }
                        MainTab.Journal -> JournalScreen(modifier = content)
                        MainTab.Travel -> mullu.comrade.ui.TravelScreen(modifier = content)
                        MainTab.Tara -> TaraScreen(modifier = content)
                        MainTab.Focus -> when (focusNav) {
                            FocusNav.Sessions -> FocusScreen(
                                onOpenReader = { focusNav = FocusNav.Reader },
                                onOpenBreathing = { focusNav = FocusNav.Breathing },
                                onOpenStretch = { focusNav = FocusNav.Stretch },
                                onJournalNote = { seedJournalNote(scope, it) },
                                modifier = content,
                            )
                            FocusNav.Reader -> ReaderScreen(
                                onJournalNote = { seedJournalNote(scope, it) },
                                sharedTitle = sharedText?.title,
                                sharedText = sharedText?.text,
                                onSharedConsumed = { AppNavigation.consumeSharedText() },
                                modifier = content,
                            )
                            FocusNav.Breathing -> BreathingScreen(
                                onDone = { focusNav = FocusNav.Sessions },
                                modifier = content,
                            )
                            FocusNav.Stretch -> StretchScreen(modifier = content)
                        }
                        // The session's own surface, rather than an overlay over
                        // the whole app. A film or an album runs for hours, and
                        // covering every other screen for the length of it meant
                        // a session you could not leave without ending. Now the
                        // playing stays here and the rest of the app stays usable.
                        MainTab.Together -> mullu.comrade.ui.TogetherScreen(
                            // Two callbacks rather than one, because the picker's
                            // result means different things: with a person named
                            // it starts a session, and with an invitation on
                            // screen it answers one. The launcher below reads the
                            // session state to tell them apart, so the peer set
                            // here is only ever used by the first case.
                            onPickFileWith = { peer, label ->
                                togetherPeer = peer to label
                                togetherPicker.launch(arrayOf("video/*", "audio/*"))
                            },
                            onPickFileToJoin = {
                                togetherPicker.launch(arrayOf("video/*", "audio/*"))
                            },
                            modifier = content,
                        )
                    }
                }
                }
            }
        }
        // Watch-together overlay. Covers the app whenever a session exists —
        // including one we were invited to but have not opened a file for yet,
        // because "Ana wants to watch Solaris with you" is not something to
        // leave buried behind a tab.
        // Only an *invitation* still covers the app. "Ana wants to watch Solaris
        // with you" has to interrupt whatever is on screen — it expires, and a
        // missed one is a missed evening. A live session does not: it has a tab.
        if (togetherState is mullu.comrade.together.TogetherManager.UiState.Invited) {
            mullu.comrade.ui.TogetherScreen(
                // An invitation is the only thing this overlay draws, so the
                // "start with someone" route is unreachable from here — but it
                // is passed rather than stubbed, because a screen that silently
                // does nothing on a tap is worse than one that behaves.
                onPickFileWith = { peer, label ->
                    togetherPeer = peer to label
                    togetherPicker.launch(arrayOf("video/*", "audio/*"))
                },
                onPickFileToJoin = {
                    togetherPicker.launch(arrayOf("video/*", "audio/*"))
                },
            )
        }
        // Call overlay — covers the app while a call is ringing/connected.
        CallScreen(
            onAccept = {
                (CallManager.state.value as? CallUiState.Ringing)?.let { ringing ->
                    withCallPermissions(ringing.video) { CallManager.accept(context) }
                }
            },
            // The in-call chat button: open the conversation with the person on
            // the call, then shrink the call into a floating tile *inside this
            // window* so it sits over the thread.
            //
            // Deliberately minimizeInApp, not PipController.enter(): an OS
            // picture-in-picture window leaves the app, so the conversation
            // opened here would end up behind the launcher — which is exactly
            // the bug this replaces ("the video minimises and no chat opens").
            // Native PiP stays for leaving the app during a call.
            onOpenChat = { peer, label ->
                tab = MainTab.Chats
                chatNav = ChatNav.Open(peer = peer, alias = label.ifBlank { null }, username = null)
                settingsOpen = false
                PipController.minimizeInApp()
            },
        )
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

    if (showEncryption && openChat != null) {
        EncryptionDetailsDialog(
            peer = openChat.peer,
            onDismiss = { showEncryption = false },
        )
    }

    if (confirmBlock && openChat != null) {
        val blocked = openChat.peer
        BlockPeerDialog(
            onDismiss = { confirmBlock = false },
            onConfirm = {
                confirmBlock = false
                scope.launch {
                    val ok = withContext(Dispatchers.IO) {
                        runCatching { ComradeCore.blockConversationTyped(blocked) }.isSuccess
                    }
                    if (ok) {
                        // The thread is gone from the list, so staying on it would
                        // show a conversation that no longer exists.
                        chatNav = ChatNav.List
                        Notifier.clearForPeer(context, blocked)
                        ChatEventRouter.bumpChatTick()
                    }
                }
            },
        )
    }
}

/**
 * One row of the conversation ⋮ menu. The label and glyph are resolved here
 * from the [ChatMenuAction] so [conversationMenu] can stay a pure list.
 */
@Composable
private fun ChatMenuRow(action: ChatMenuAction, onClick: () -> Unit) {
    val label = stringResource(
        when (action) {
            ChatMenuAction.SetAlias -> R.string.chat_menu_set_alias
            ChatMenuAction.AddComrade -> R.string.comrade_add
            ChatMenuAction.RemoveComrade -> R.string.comrade_remove
            ChatMenuAction.Mute -> R.string.chat_menu_mute
            ChatMenuAction.Unmute -> R.string.chat_menu_unmute
            ChatMenuAction.CopyKey -> R.string.chat_menu_copy_key
            ChatMenuAction.EncryptionInfo -> R.string.chat_menu_encryption
            ChatMenuAction.Block -> R.string.chat_menu_block
        },
    )
    val icon = when (action) {
        ChatMenuAction.SetAlias -> Icons.Filled.Edit
        // Outline for "not yet", filled for "currently" — the same read the
        // star toggle in the bar used to give at a glance.
        ChatMenuAction.AddComrade -> StarOutlineIcon
        ChatMenuAction.RemoveComrade -> StarIcon
        // The glyph shows what tapping *does*, matching the label: a struck-out
        // bell on the row that turns notifications back on would read backwards.
        ChatMenuAction.Mute -> NotificationsOffIcon
        ChatMenuAction.Unmute -> NotificationsIcon
        ChatMenuAction.CopyKey -> CopyIcon
        ChatMenuAction.EncryptionInfo -> Icons.Filled.Lock
        ChatMenuAction.Block -> Icons.Filled.Warning
    }
    val tint = if (action.destructive) {
        MaterialTheme.colorScheme.error
    } else {
        MaterialTheme.colorScheme.onSurfaceVariant
    }
    DropdownMenuItem(
        text = {
            Text(
                label,
                color = if (action.destructive) {
                    MaterialTheme.colorScheme.error
                } else {
                    MaterialTheme.colorScheme.onSurface
                },
            )
        },
        leadingIcon = { Icon(icon, contentDescription = null, tint = tint) },
        onClick = onClick,
        modifier = Modifier.testTag("chat-menu-${action.name}"),
    )
}

/**
 * What protects this thread, plus the peer's key in full.
 *
 * The key is shown unabbreviated and selectable on purpose: comparing it out
 * of band is the only way to be sure who you're talking to, and a truncated
 * key can't be compared.
 */
@Composable
private fun EncryptionDetailsDialog(peer: String, onDismiss: () -> Unit) {
    val dialogShape = RoundedCornerShape(ComradeRadii.xl)
    AlertDialog(
        onDismissRequest = onDismiss,
        modifier = Modifier.glassSurface(GlassElevation.Sheet, shape = dialogShape),
        shape = dialogShape,
        containerColor = Color.Transparent,
        icon = { Icon(Icons.Filled.Lock, contentDescription = null) },
        title = { Text(stringResource(R.string.encryption_title)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(
                    stringResource(R.string.encryption_body),
                    style = MaterialTheme.typography.bodyMedium,
                )
                Text(
                    stringResource(R.string.encryption_key_label),
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.primary,
                )
                Surface(
                    color = MaterialTheme.colorScheme.surfaceVariant,
                    shape = RoundedCornerShape(8.dp),
                ) {
                    SelectionContainer {
                        Text(
                            peer,
                            style = MaterialTheme.typography.bodySmall,
                            fontFamily = FontFamily.Monospace,
                            modifier = Modifier
                                .padding(horizontal = 10.dp, vertical = 8.dp)
                                .testTag("encryption-key"),
                        )
                    }
                }
                Text(
                    stringResource(R.string.encryption_verify_hint),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.close)) }
        },
    )
}

/**
 * Block confirmation. Blocking is silent to the other person and drops their
 * future messages, so it says both of those things before doing it.
 */
@Composable
private fun BlockPeerDialog(onDismiss: () -> Unit, onConfirm: () -> Unit) {
    val dialogShape = RoundedCornerShape(ComradeRadii.xl)
    AlertDialog(
        onDismissRequest = onDismiss,
        modifier = Modifier.glassSurface(GlassElevation.Sheet, shape = dialogShape),
        shape = dialogShape,
        containerColor = Color.Transparent,
        icon = {
            Icon(
                Icons.Filled.Warning,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.error,
            )
        },
        title = { Text(stringResource(R.string.block_title)) },
        text = { Text(stringResource(R.string.block_body)) },
        confirmButton = {
            TextButton(
                onClick = onConfirm,
                modifier = Modifier.testTag("block-confirm"),
            ) {
                Text(
                    stringResource(R.string.block_confirm),
                    color = MaterialTheme.colorScheme.error,
                )
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) }
        },
    )
}

/**
 * The navigation drawer sheet: a profile header (tap → Settings) over the
 * app-wide destinations that don't belong in the bottom bar — Call history and
 * Settings — Telegram-style. Message requests deliberately stay in the chat
 * list, not here.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ComradeDrawerSheet(
    profile: ComradeCore.Profile,
    onOpenSettings: () -> Unit,
    onOpenCallHistory: () -> Unit,
    onOpenComrades: () -> Unit,
    onOpenTasks: () -> Unit,
    onOpenFeed: () -> Unit,
    onOpenRide: () -> Unit,
) {
    ModalDrawerSheet {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .clickable(onClick = onOpenSettings)
                .testTag("drawer-profile")
                .padding(horizontal = 20.dp, vertical = 20.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            PeerAvatar(profile.username ?: profile.npub, seed = profile.npub)
            Column {
                Text(
                    profile.username?.let { "@$it" } ?: "No username yet",
                    style = MaterialTheme.typography.titleMedium,
                )
                Text(
                    shortNpub(profile.npub),
                    style = MaterialTheme.typography.bodySmall,
                    fontFamily = FontFamily.Monospace,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        HorizontalDivider()
        NavigationDrawerItem(
            label = { Text(stringResource(R.string.comrades_title)) },
            icon = { Icon(StarIcon, contentDescription = null) },
            selected = false,
            onClick = onOpenComrades,
            modifier = Modifier.testTag("drawer-comrades"),
        )
        NavigationDrawerItem(
            label = { Text(stringResource(R.string.tasks_title)) },
            icon = { Icon(Icons.Filled.CheckCircle, contentDescription = null) },
            selected = false,
            onClick = onOpenTasks,
            modifier = Modifier.testTag("drawer-tasks"),
        )
        // Feed lives here rather than on the bottom nav — Together took that
        // slot. It is still one tap from anywhere, which is what "off the nav"
        // was supposed to mean and what deleting it would not have.
        NavigationDrawerItem(
            label = { Text("Feed") },
            icon = { Icon(ArticleIcon, contentDescription = null) },
            selected = false,
            onClick = onOpenFeed,
            modifier = Modifier.testTag("drawer-feed"),
        )
        // Ride mode is a place you *go* — you are about to get on a
        // motorcycle — which is the same argument that put Feed in the drawer
        // rather than on the bottom nav. It is deliberately not a tab: five
        // tabs is the nav, and a sixth for something used on a ride and never
        // otherwise would cost the four daily ones room.
        NavigationDrawerItem(
            label = { Text(stringResource(R.string.ride_title)) },
            icon = { Icon(MotorcycleIcon, contentDescription = null) },
            selected = false,
            onClick = onOpenRide,
            modifier = Modifier.testTag("drawer-ride"),
        )
        NavigationDrawerItem(
            label = { Text(stringResource(R.string.call_history_title)) },
            icon = { Icon(CallIcon, contentDescription = null) },
            selected = false,
            onClick = onOpenCallHistory,
        )
        NavigationDrawerItem(
            label = { Text("Settings") },
            icon = { Icon(Icons.Filled.Settings, contentDescription = null) },
            selected = false,
            onClick = onOpenSettings,
            modifier = Modifier.testTag("drawer-settings"),
        )
    }
}

/**
 * Ride mode, as a pushed screen (`docs/RIDE.md`).
 *
 * Loads its own comrade list rather than taking one from the caller: this is
 * reached from the drawer, not from a tab that already had one in hand, and
 * the list is small and read once when the screen opens.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun RidePushedScreen(onBack: () -> Unit) {
    var comrades by remember { mutableStateOf<List<ComradeCore.ComradeInfo>>(emptyList()) }
    LaunchedEffect(Unit) {
        comrades = withContext(Dispatchers.IO) {
            runCatching { ComradeCore.comrades() }.getOrDefault(emptyList())
        }
    }
    Scaffold(
        topBar = {
            TopAppBar(
                modifier = Modifier.glassSurface(GlassElevation.Chrome),
                colors = glassTopAppBarColors(),
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(
                            Icons.AutoMirrored.Filled.ArrowBack,
                            contentDescription = "Back",
                        )
                    }
                },
                title = { Text(stringResource(R.string.ride_title)) },
            )
        },
    ) { padding ->
        mullu.comrade.ui.RideScreen(
            peers = comrades,
            modifier = Modifier.padding(padding),
        )
    }
}

/**
 * The public feed, as a pushed screen.
 *
 * It was a bottom-nav tab until Together took that slot. Kept whole and one tap
 * from anywhere rather than removed: the argument for moving it was that a feed
 * is somewhere you *go*, which is exactly what the drawer is for.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun FeedPushedScreen(
    feedItems: List<ComradeCore.ChitthiInfo>,
    onBack: () -> Unit,
    onOpenJournal: () -> Unit,
    onOpenBreathing: () -> Unit,
) {
    Column(modifier = Modifier.fillMaxSize()) {
        Scaffold(
            modifier = Modifier.weight(1f),
            topBar = {
                TopAppBar(
                    modifier = Modifier.glassSurface(GlassElevation.Chrome),
                    colors = glassTopAppBarColors(),
                    navigationIcon = {
                        IconButton(onClick = onBack) {
                            Icon(
                                Icons.AutoMirrored.Filled.ArrowBack,
                                contentDescription = "Back",
                            )
                        }
                    },
                    title = { Text("Feed") },
                )
            },
        ) { padding ->
            FeedScreen(
                feedItems = feedItems,
                onPosted = { ChatEventRouter.addChitthi(it, front = true) },
                onOpenJournal = onOpenJournal,
                onOpenBreathing = onOpenBreathing,
                modifier = Modifier.padding(padding),
            )
        }
    }
}

/**
 * Settings as a full-screen pushed destination with a back arrow, replacing the
 * shell (and its bottom bar) entirely while open — not a bottom-nav tab.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SettingsPushedScreen(
    profile: ComradeCore.Profile,
    onProfileChange: (ComradeCore.Profile) -> Unit,
    onLock: () -> Unit,
    onBack: () -> Unit,
) {
    Column(modifier = Modifier.fillMaxSize()) {
        Scaffold(
            modifier = Modifier.weight(1f),
            topBar = {
                TopAppBar(
                    modifier = Modifier.glassSurface(GlassElevation.Chrome),
                    colors = glassTopAppBarColors(),
                    navigationIcon = {
                        IconButton(onClick = onBack) {
                            Icon(
                                Icons.AutoMirrored.Filled.ArrowBack,
                                contentDescription = "Back",
                            )
                        }
                    },
                    title = { Text("Settings") },
                )
            },
        ) { padding ->
            SettingsScreen(
                profile = profile,
                onProfileChange = onProfileChange,
                onLock = onLock,
                modifier = Modifier
                    .fillMaxSize()
                    .padding(padding),
            )
        }
    }
}

/**
 * Which transport a message tries first, as two glyphs in the top bar's
 * trailing slot — directly opposite the navigation menu.
 *
 * This replaces the always-on "Local mesh" banner. That banner spent a full row
 * of every screen on a line the user could read but not act on, and it only
 * ever appeared in the one workspace — so the signal that matters most with no
 * signal at all was both intrusive and, most of the time, absent.
 *
 * Here the two routes are drawn in the order they are tried: the preferred one
 * leads at full size, the fallback follows, smaller and dimmed, with a badge
 * for how many devices are actually nearby. That ordering *is* the setting;
 * tapping opens a menu that swaps it.
 *
 * Precedence is an order, not an exclusion — whichever route leads, a message
 * the preferred one cannot carry still takes the other (see
 * `docs/OFFLINE_DELIVERY.md`). Nothing here can strand a message. The ordering
 * itself lives in [TransportPrecedence], shared with the Flutter frontend and
 * tested on both.
 *
 * The workspace is read once on composition and thereafter tracked locally,
 * because the core exposes no workspace flow to collect — a change made
 * somewhere other than this control (a voice command, say) is picked up on the
 * next recomposition of the shell rather than live.
 */
@Composable
private fun TransportPrecedenceAction() {
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val mesh by MeshStatusMonitor.status.collectAsState()
    var workspaceKey by remember { mutableStateOf(TransportPrecedence.RELAY_FIRST_WORKSPACE) }
    var menuOpen by remember { mutableStateOf(false) }

    // Bluetooth needs scan + advertise, and this is the moment to ask for them:
    // the user has just said they want messages to go to whoever is nearby
    // first, which is the only sentence that makes "let Comrade find nearby
    // devices" mean anything. Asked here rather than at startup for the reason
    // `MediaLibraryAccess` gives about the music library — a permission prompt
    // on a cold app is a prompt with no reason attached — and because a dialog
    // that appears over onboarding is one the user dismisses without reading.
    //
    // Denied is a first-class answer: `BleMeshService.start` no-ops without the
    // grant, the core never offers Bluetooth as a route, and the WiFi mesh and
    // relays are unaffected. Nothing retries, because Android stops showing the
    // dialog after a refusal — a second ask would be a prompt that silently
    // does nothing.
    val blePermission = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions(),
    ) {
        // Whatever the answer, re-evaluate now rather than at the next unlock.
        MeshRadio.setActive(MeshStatusMonitor.status.value.active)
    }

    LaunchedEffect(Unit) {
        workspaceKey = withContext(Dispatchers.IO) {
            runCatching { ComradeCore.currentWorkspaceTyped().key }
                .getOrDefault(TransportPrecedence.RELAY_FIRST_WORKSPACE)
        }
    }
    if (!TransportPrecedence.isSwitchable(workspaceKey)) return

    val meshState = TransportPrecedence.meshStateOf(mesh.active, mesh.peerCount)
    val order = TransportPrecedence.orderOf(workspaceKey)
    val lead = order.first()
    val fallback = order.last()

    // Named `route`, not `lead`: it is the order the user is *asking* for, and
    // shadowing the one currently in force would make the guard below unreadable.
    fun choose(route: TransportRoute) {
        menuOpen = false
        val target = TransportPrecedence.workspaceFor(route)
        // The core's state machine rejects a self-transition, so re-picking the
        // order already in force must be a no-op rather than an error the user
        // caused by confirming what they already had.
        if (target == workspaceKey) return
        if (route == TransportRoute.LocalNetwork) {
            val needed = BleMeshService.requiredPermissions().filter {
                context.checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED
            }
            if (needed.isNotEmpty()) blePermission.launch(needed.toTypedArray())
        }
        scope.launch {
            val applied = withContext(Dispatchers.IO) {
                runCatching { ComradeCore.toggleWorkspaceTyped(target).key }.getOrNull()
            }
            if (applied != null) workspaceKey = applied
        }
    }

    Box {
        IconButton(
            onClick = { menuOpen = true },
            modifier = Modifier.testTag("transport-precedence"),
        ) {
            BadgedBox(
                badge = {
                    if (meshState == LocalMeshState.Reaching) {
                        Badge { Text("${mesh.peerCount}") }
                    }
                },
            ) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(2.dp),
                ) {
                    Icon(
                        transportGlyph(lead),
                        contentDescription = transportDescription(lead, meshState),
                        tint = leadTint(lead, meshState),
                        modifier = Modifier.size(22.dp),
                    )
                    Icon(
                        transportGlyph(fallback),
                        contentDescription = null,
                        tint = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.55f),
                        modifier = Modifier.size(14.dp),
                    )
                }
            }
        }
        DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }) {
            DropdownMenuItem(
                enabled = false,
                text = {
                    Text(
                        nearbyLabel(meshState, mesh.peerCount),
                        style = MaterialTheme.typography.labelMedium,
                    )
                },
                onClick = {},
            )
            HorizontalDivider()
            PrecedenceChoice(
                route = TransportRoute.Relay,
                selected = lead == TransportRoute.Relay,
                label = "Internet first",
                detail = "Relays, then nearby devices",
                testTag = "precedence-relay",
                onClick = { choose(it) },
            )
            PrecedenceChoice(
                route = TransportRoute.LocalNetwork,
                selected = lead == TransportRoute.LocalNetwork,
                label = "This network first",
                detail = "Nearby devices, then relays",
                testTag = "precedence-local",
                onClick = { choose(it) },
            )
        }
    }
}

/** One row of the precedence menu; the chosen order is carried by colour. */
@Composable
private fun PrecedenceChoice(
    route: TransportRoute,
    selected: Boolean,
    label: String,
    detail: String,
    testTag: String,
    onClick: (TransportRoute) -> Unit,
) {
    val accent = if (selected) {
        MaterialTheme.colorScheme.primary
    } else {
        MaterialTheme.colorScheme.onSurfaceVariant
    }
    DropdownMenuItem(
        modifier = Modifier.testTag(testTag),
        leadingIcon = {
            Icon(transportGlyph(route), contentDescription = null, tint = accent)
        },
        text = {
            Column {
                Text(label, color = accent)
                Text(
                    detail,
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        },
        onClick = { onClick(route) },
    )
}

/**
 * Only the local-network route carries live state, and it carries it in colour
 * and a badge rather than a third silhouette (see [WifiTetheringIcon]). The
 * relay glyph is deliberately constant: the workspace's `relayConnected` flag
 * is a label, not a reachability probe, and drawing it as one would be a lie.
 */
private fun transportGlyph(route: TransportRoute): ImageVector = when (route) {
    TransportRoute.Relay -> CloudIcon
    TransportRoute.LocalNetwork -> WifiTetheringIcon
}

/**
 * The leading glyph's colour, which is where the local network's live state
 * goes now that it has only one silhouette: the accent while it is actually
 * reaching someone, the ordinary foreground while it is looking, and dimmed
 * when the engine is not running at all. The relay glyph never changes.
 */
@Composable
private fun leadTint(route: TransportRoute, mesh: LocalMeshState): Color = when {
    route == TransportRoute.Relay -> MaterialTheme.colorScheme.onSurface
    mesh == LocalMeshState.Reaching -> MaterialTheme.colorScheme.primary
    mesh == LocalMeshState.Searching -> MaterialTheme.colorScheme.onSurface
    else -> MaterialTheme.colorScheme.onSurface.copy(alpha = 0.45f)
}

private fun transportDescription(route: TransportRoute, mesh: LocalMeshState): String =
    when (route) {
        TransportRoute.Relay -> "Sending over the internet first"
        TransportRoute.LocalNetwork -> when (mesh) {
            LocalMeshState.Reaching -> "Sending over this network first, devices nearby"
            LocalMeshState.Searching -> "Sending over this network first, looking for devices"
            LocalMeshState.Off -> "Sending over this network first, network off"
        }
    }

private fun nearbyLabel(mesh: LocalMeshState, peerCount: Int): String = when (mesh) {
    LocalMeshState.Off -> "Local network off"
    LocalMeshState.Searching -> "Looking for nearby devices…"
    LocalMeshState.Reaching ->
        if (peerCount == 1) "1 device nearby" else "$peerCount devices nearby"
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
    val dialogShape = RoundedCornerShape(ComradeRadii.xl)

    AlertDialog(
        onDismissRequest = { if (!busy) onDismiss() },
        modifier = Modifier.glassSurface(GlassElevation.Sheet, shape = dialogShape),
        shape = dialogShape,
        containerColor = Color.Transparent,
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

