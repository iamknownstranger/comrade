package mullu.comrade

import android.app.Activity
import android.content.Context
import android.os.Bundle
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Create
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.CenterAlignedTopAppBar
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext
import mullu.comrade.ui.ArticleIcon
import mullu.comrade.ui.ChatBubbleIcon
import mullu.comrade.ui.ChatsScreen
import mullu.comrade.ui.ConversationScreen
import mullu.comrade.ui.FeedScreen
import mullu.comrade.ui.NewChatScreen
import mullu.comrade.ui.OnboardingScreen
import mullu.comrade.ui.SettingsScreen
import mullu.comrade.ui.peerTitle
import mullu.comrade.ui.shortNpub
import mullu.comrade.ui.theme.ComradeTheme
import org.json.JSONObject

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
}

/** Where the encrypted vault lives on this device. */
internal fun vaultPath(context: Context): File = File(context.filesDir, "comrade-vault")

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
    Feed("Feed", ArticleIcon),
    Settings("Settings", Icons.Filled.Settings),
}

/** Sub-navigation inside the Chats tab. */
private sealed interface ChatNav {
    data object List : ChatNav
    data object NewChat : ChatNav
    data class Open(val peer: String, val alias: String?) : ChatNav
}

/** Events drained from the native bridge, reduced to what the shell reacts to. */
private sealed interface PumpEvent {
    data class Chitthi(val info: ComradeCore.ChitthiInfo) : PumpEvent
    data object DmChanged : PumpEvent
}

/** Bound on the in-memory public feed (the relay stream is unbounded). */
private const val FEED_CAP = 500

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun MainShell(
    profile: ComradeCore.Profile,
    onProfileChange: (ComradeCore.Profile) -> Unit,
) {
    var tab by rememberSaveable { mutableStateOf(MainTab.Chats) }
    var chatNav by remember { mutableStateOf<ChatNav>(ChatNav.List) }
    // Bumped whenever the DM history changed; list + open thread reload on it.
    var chatTick by remember { mutableStateOf(0) }
    val feedItems = remember { mutableStateListOf<ComradeCore.ChitthiInfo>() }
    val seenFeedIds = remember { HashSet<String>() }

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

        while (isActive) {
            val events = withContext(Dispatchers.IO) { drainEvents() }
            var dmChanged = false
            for (event in events) {
                when (event) {
                    is PumpEvent.Chitthi -> addToFeed(event.info, front = true)
                    PumpEvent.DmChanged -> dmChanged = true
                }
            }
            if (dmChanged) chatTick++
            delay(600)
        }
    }

    val openChat = chatNav as? ChatNav.Open
    BackHandler(enabled = tab == MainTab.Chats && chatNav != ChatNav.List) {
        chatNav = ChatNav.List
    }

    Scaffold(
        topBar = {
            when {
                tab == MainTab.Chats && openChat != null -> TopAppBar(
                    navigationIcon = {
                        IconButton(onClick = { chatNav = ChatNav.List }) {
                            Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                        }
                    },
                    title = {
                        Column {
                            Text(peerTitle(openChat.peer, openChat.alias))
                            Text(
                                shortNpub(openChat.peer),
                                style = MaterialTheme.typography.labelSmall,
                                fontFamily = FontFamily.Monospace,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
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
                else -> CenterAlignedTopAppBar(
                    title = {
                        Text(
                            when (tab) {
                                MainTab.Chats -> "Comrade"
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
                    onOpen = { peer, alias -> chatNav = ChatNav.Open(peer, alias) },
                    onNewChat = { chatNav = ChatNav.NewChat },
                    modifier = content,
                )
                ChatNav.NewChat -> NewChatScreen(
                    onOpen = { peer, alias -> chatNav = ChatNav.Open(peer, alias) },
                    modifier = content,
                )
                is ChatNav.Open -> ConversationScreen(
                    peer = nav.peer,
                    chatTick = chatTick,
                    modifier = content,
                )
            }
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

/**
 * Non-blocking drain of the native event bus (bounded per round). Chitthis are
 * parsed for the feed; DM/media arrivals just signal "history changed" — the
 * chat screens reload from the Rust-side offline history, the source of truth.
 */
private fun drainEvents(max: Int = 200): List<PumpEvent> {
    val out = mutableListOf<PumpEvent>()
    repeat(max) {
        val raw = runCatching { ComradeCore.pollEvent() }.getOrNull() ?: return out
        val obj = runCatching { JSONObject(raw) }.getOrNull() ?: return out
        when {
            obj.has("empty") || obj.has("closed") || obj.has("error") -> return out
            obj.has("lagged") -> Unit // dropped events; keep draining
            else -> when (obj.optString("type")) {
                "incoming_chitthi" -> out +=
                    PumpEvent.Chitthi(
                        ComradeCore.ChitthiInfo(
                            id = obj.optString("id"),
                            author = obj.optString("author"),
                            content = obj.optString("content"),
                            createdAt = obj.optLong("created_at"),
                            replyTo = null,
                        ),
                    )
                "incoming_direct_message", "incoming_media" -> out += PumpEvent.DmChanged
            }
        }
    }
    return out
}
