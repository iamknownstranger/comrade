package mullu.comrade

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.Lock
import androidx.compose.material.icons.materialIcon
import androidx.compose.material.icons.materialPath
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import mullu.comrade.ui.theme.ComradeTheme
import mullu.comrade.voice.CommandDispatcher
import mullu.comrade.voice.ComradeCoreBackend
import mullu.comrade.voice.ComradeTts
import mullu.comrade.voice.OneShotRecognizer
import mullu.comrade.voice.VoiceCommand
import mullu.comrade.voice.WakeWordService

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // This screen can display an nsec — block screenshots and screen
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

/** Facts served by the native core, resolved off the main thread at startup. */
sealed interface CoreState {
    object Loading : CoreState
    data class Ready(
        val version: String,
        val workspaces: List<ComradeCore.WorkspaceInfo>,
    ) : CoreState
    data class Failed(val reason: String) : CoreState
}

/**
 * Top-level destinations of the app shell, one per bottom-navigation item.
 * Home carries the workspace overview; Voice and Keys hold the assistant and
 * key-management tools that previously shared one long scrolling column.
 */
private enum class ComradeDestination(val label: String, val icon: ImageVector) {
    Home("Home", Icons.Filled.Home),
    Voice("Voice", MicIcon),
    Keys("Keys", Icons.Filled.Lock),
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ComradeApp() {
    // The first ComradeCore touch pays for System.loadLibrary of the full Rust
    // core (the Application warm-up usually races ahead of this, but must not
    // be relied on). Resolving these on the IO dispatcher keeps the first
    // frame free of JNI work — the shell renders immediately and the
    // workspace list streams in when the native library is ready.
    val core by produceState<CoreState>(initialValue = CoreState.Loading) {
        value = withContext(Dispatchers.IO) {
            runCatching {
                CoreState.Ready(ComradeCore.getVersion(), ComradeCore.workspaces())
            }.getOrElse {
                CoreState.Failed(it.message ?: "native core unavailable")
            }
        }
    }

    // Startup observability: logs an ActivityTaskManager "Fully drawn" line
    // (visible in logcat / Perfetto) once real content replaced the placeholder.
    val activity = LocalContext.current as? Activity
    LaunchedEffect(core is CoreState.Loading) {
        if (core !is CoreState.Loading) activity?.reportFullyDrawn()
    }

    var destination by rememberSaveable { mutableStateOf(ComradeDestination.Home) }

    Scaffold(
        topBar = {
            CenterAlignedTopAppBar(
                title = { Text("Comrade") },
            )
        },
        bottomBar = {
            NavigationBar {
                ComradeDestination.entries.forEach { dest ->
                    NavigationBarItem(
                        selected = destination == dest,
                        onClick = { destination = dest },
                        icon = { Icon(dest.icon, contentDescription = null) },
                        label = { Text(dest.label) },
                    )
                }
            }
        },
    ) { padding ->
        val content = Modifier
            .fillMaxSize()
            .padding(padding)
        when (destination) {
            ComradeDestination.Home -> HomeScreen(core, content)
            ComradeDestination.Voice -> VoiceScreen(content)
            ComradeDestination.Keys -> KeysScreen(content)
        }
    }
}

// ── Home: identity of the app + workspace overview ──────────────────────────

@Composable
private fun HomeScreen(core: CoreState, modifier: Modifier = Modifier) {
    LazyColumn(
        modifier = modifier,
        contentPadding = PaddingValues(horizontal = 20.dp, vertical = 16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        item {
            ElevatedCard(modifier = Modifier.fillMaxWidth()) {
                Column(
                    modifier = Modifier.padding(16.dp),
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    Text(
                        text = "Privacy-first social client",
                        style = MaterialTheme.typography.titleMedium,
                    )
                    Text(
                        text = "Public Chitthi feed, end-to-end encrypted DMs, and " +
                            "an off-grid mesh — all driven by an on-device Rust core.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Spacer(Modifier.height(4.dp))
                    Text(
                        text = when (core) {
                            is CoreState.Ready -> "core v${core.version}"
                            is CoreState.Failed -> "core unavailable — ${core.reason}"
                            CoreState.Loading -> "starting core…"
                        },
                        style = MaterialTheme.typography.labelSmall,
                        fontFamily = FontFamily.Monospace,
                        color = when (core) {
                            is CoreState.Failed -> MaterialTheme.colorScheme.error
                            else -> MaterialTheme.colorScheme.outline
                        },
                    )
                }
            }
        }

        item { SectionHeader("Workspaces") }

        when (core) {
            CoreState.Loading -> item {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.Center,
                ) {
                    CircularProgressIndicator(modifier = Modifier.size(24.dp))
                }
            }
            is CoreState.Failed -> item {
                Text(
                    text = core.reason,
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            is CoreState.Ready -> items(core.workspaces) { ws ->
                WorkspaceCard(info = ws)
            }
        }
    }
}

@Composable
private fun SectionHeader(title: String) {
    Text(
        text = title,
        style = MaterialTheme.typography.titleSmall,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.fillMaxWidth(),
    )
}

@Composable
fun WorkspaceCard(info: ComradeCore.WorkspaceInfo) {
    // Labels come from comrade_state as "Title — Description"; split them so
    // the card reads as a heading plus supporting line.
    val title = info.label.substringBefore(" — ")
    val detail = info.label.substringAfter(" — ", missingDelimiterValue = "")

    OutlinedCard(modifier = Modifier.fillMaxWidth()) {
        Row(
            modifier = Modifier.padding(12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Surface(
                shape = CircleShape,
                color = MaterialTheme.colorScheme.secondaryContainer,
                modifier = Modifier.size(40.dp),
            ) {
                Box(contentAlignment = Alignment.Center) {
                    Text(workspaceEmoji(info.key), style = MaterialTheme.typography.titleMedium)
                }
            }
            Column {
                Text(
                    text = title,
                    style = MaterialTheme.typography.titleSmall,
                )
                if (detail.isNotEmpty()) {
                    Text(
                        text = detail,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }
}

private fun workspaceEmoji(key: String): String = when (key) {
    "Base" -> "🏛"
    "OffGridTravel" -> "📡"
    "CoupleSandboxSakha" -> "💙"
    "CoupleSandboxSakhi" -> "❤️"
    else -> "◆"
}

// ── Voice: on-device assistant controls ─────────────────────────────────────

@Composable
private fun VoiceScreen(modifier: Modifier = Modifier) {
    val context = LocalContext.current
    // Seeded from the service so the toggle survives tab switches — this
    // composable is disposed whenever the user navigates away.
    var wakeEnabled by remember { mutableStateOf(WakeWordService.isRunning) }
    var lastReply by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }

    // Voice helpers live for the lifetime of this screen.
    val tts = remember { ComradeTts(context) }
    val dispatcher = remember { CommandDispatcher(ComradeCoreBackend()) }
    DisposableEffect(Unit) { onDispose { tts.shutdown() } }

    fun runTapToTalk() {
        busy = true
        lastReply = null
        OneShotRecognizer(context).listen(
            onText = { heard ->
                if (heard.isBlank()) {
                    lastReply = "I didn't catch that."
                    busy = false
                } else {
                    val reply = runCatching { dispatcher.handle(VoiceCommand.parse(heard)) }
                        .getOrElse { "Something went wrong." }
                    lastReply = "“$heard” → $reply"
                    tts.speak(reply)
                    busy = false
                }
            },
            onError = { lastReply = "Voice unavailable: ${it.message}"; busy = false },
        )
    }

    // Request RECORD_AUDIO (and POST_NOTIFICATIONS on 33+) before any capture.
    val permissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions(),
    ) { grants ->
        val micGranted = grants[Manifest.permission.RECORD_AUDIO] == true ||
            hasMic(context)
        if (micGranted) {
            if (wakeEnabled) WakeWordService.start(context) else runTapToTalk()
        } else {
            lastReply = context.getString(R.string.voice_permission_needed)
            wakeEnabled = false
        }
    }

    fun ensurePermissionThen(action: () -> Unit) {
        if (hasMic(context)) {
            action()
        } else {
            permissionLauncher.launch(voicePermissions())
        }
    }

    Column(
        modifier = modifier
            .verticalScroll(rememberScrollState())
            .padding(horizontal = 20.dp, vertical = 16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        SectionHeader(stringResource(R.string.voice_section_title))

        ElevatedCard(modifier = Modifier.fillMaxWidth()) {
            Column(
                modifier = Modifier.padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(
                    text = "Everything runs on this phone",
                    style = MaterialTheme.typography.titleSmall,
                )
                Text(
                    text = "Recognition uses the offline Vosk model and Android's " +
                        "built-in text-to-speech — no audio ever leaves the device.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )

                Button(
                    onClick = {
                        if (wakeEnabled) {
                            WakeWordService.stop(context)
                            wakeEnabled = false
                        } else {
                            wakeEnabled = true
                            ensurePermissionThen { WakeWordService.start(context) }
                        }
                    },
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text(
                        stringResource(
                            if (wakeEnabled) R.string.voice_wake_disable else R.string.voice_wake_enable,
                        ),
                    )
                }

                OutlinedButton(
                    onClick = { ensurePermissionThen { runTapToTalk() } },
                    enabled = !busy,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text(stringResource(R.string.voice_tap_to_talk))
                }

                TextButton(
                    onClick = { context.startActivity(assistSettingsIntent()) },
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text(stringResource(R.string.voice_set_default_assistant))
                }
            }
        }

        lastReply?.let {
            Text(
                it,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }

        OutlinedCard(modifier = Modifier.fillMaxWidth()) {
            Column(
                modifier = Modifier.padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                Text(
                    text = "Things you can say",
                    style = MaterialTheme.typography.titleSmall,
                )
                Text(
                    text = "“post <message>” · “read my timeline” · “switch to " +
                        "base / off-grid / sakha / sakhi” · “new identity” · “help”",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(
                    text = "post and read my timeline need an unlocked vault; the " +
                        "unlock screen hasn't landed on Android yet, so they " +
                        "currently answer with an error.",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.outline,
                )
            }
        }
    }
}

private fun hasMic(context: Context): Boolean =
    ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) ==
        PackageManager.PERMISSION_GRANTED

private fun voicePermissions(): Array<String> =
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
        arrayOf(Manifest.permission.RECORD_AUDIO, Manifest.permission.POST_NOTIFICATIONS)
    } else {
        arrayOf(Manifest.permission.RECORD_AUDIO)
    }

private fun assistSettingsIntent(): Intent =
    Intent(Settings.ACTION_VOICE_INPUT_SETTINGS)
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

// ── Keys: identity / keypair management ─────────────────────────────────────

@Composable
private fun KeysScreen(modifier: Modifier = Modifier) {
    var keypair by remember { mutableStateOf<ComradeCore.Keypair?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    // The nsec is masked by default and only shown on an explicit reveal tap.
    var revealNsec by remember { mutableStateOf(false) }

    Column(
        modifier = modifier
            .verticalScroll(rememberScrollState())
            .padding(horizontal = 20.dp, vertical = 16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        SectionHeader("Key Management")

        Text(
            text = "A keypair is your Nostr identity: the npub is shareable, the " +
                "nsec signs everything and must stay secret.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )

        Button(
            onClick = {
                runCatching { ComradeCore.generateKeypairTyped() }
                    .onSuccess { keypair = it; error = null; revealNsec = false }
                    .onFailure { error = it.message }
            },
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text("Generate New Keypair")
        }

        keypair?.let { kp ->
            OutlinedCard(modifier = Modifier.fillMaxWidth()) {
                Column(modifier = Modifier.padding(12.dp)) {
                    Text("npub", style = MaterialTheme.typography.labelSmall)
                    Text(
                        kp.npub,
                        style = MaterialTheme.typography.bodySmall,
                        fontFamily = FontFamily.Monospace,
                    )
                    Spacer(Modifier.height(8.dp))
                    Text("nsec (keep secret!)", style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.error)
                    Text(
                        if (revealNsec) kp.nsec else "••••••••••••  (hidden)",
                        style = MaterialTheme.typography.bodySmall,
                        fontFamily = FontFamily.Monospace,
                    )
                    TextButton(onClick = { revealNsec = !revealNsec }) {
                        Text(if (revealNsec) "Hide secret key" else "Reveal secret key")
                    }
                }
            }
        }

        error?.let {
            Text(it, color = MaterialTheme.colorScheme.error,
                style = MaterialTheme.typography.bodySmall)
        }
    }
}

// Material's standard "mic" glyph, inlined so the navigation bar doesn't pull
// in the multi-megabyte material-icons-extended artifact for one icon.
private val MicIcon: ImageVector = materialIcon(name = "Filled.Mic") {
    materialPath {
        moveTo(12.0f, 14.0f)
        curveToRelative(1.66f, 0.0f, 3.0f, -1.34f, 3.0f, -3.0f)
        verticalLineTo(5.0f)
        curveToRelative(0.0f, -1.66f, -1.34f, -3.0f, -3.0f, -3.0f)
        reflectiveCurveTo(9.0f, 3.34f, 9.0f, 5.0f)
        verticalLineToRelative(6.0f)
        curveToRelative(0.0f, 1.66f, 1.34f, 3.0f, 3.0f, 3.0f)
        close()
        moveTo(17.0f, 11.0f)
        curveToRelative(0.0f, 2.76f, -2.24f, 5.0f, -5.0f, 5.0f)
        reflectiveCurveToRelative(-5.0f, -2.24f, -5.0f, -5.0f)
        horizontalLineTo(5.0f)
        curveToRelative(0.0f, 3.53f, 2.61f, 6.43f, 6.0f, 6.92f)
        verticalLineTo(21.0f)
        horizontalLineToRelative(2.0f)
        verticalLineToRelative(-3.08f)
        curveToRelative(3.39f, -0.49f, 6.0f, -3.39f, 6.0f, -6.92f)
        horizontalLineToRelative(-2.0f)
        close()
    }
}
