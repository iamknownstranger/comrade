package mullu.comrade

import android.Manifest
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
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
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

@Composable
fun ComradeApp() {
    val version = remember { ComradeCore.getVersion() }
    val workspaces = remember { ComradeCore.workspaces() }

    LazyColumn(
        modifier = Modifier
            .fillMaxSize()
            .padding(horizontal = 24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp),
        contentPadding = PaddingValues(vertical = 48.dp),
    ) {
        item {
            Text(
                text = "Comrade",
                style = MaterialTheme.typography.displayMedium,
            )
            Text(
                text = "Privacy-first social client",
                style = MaterialTheme.typography.titleMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(8.dp))
            Text(
                text = "core v$version",
                style = MaterialTheme.typography.labelSmall,
                fontFamily = FontFamily.Monospace,
                color = MaterialTheme.colorScheme.outline,
            )
        }

        item { HorizontalDivider(modifier = Modifier.padding(vertical = 8.dp)) }

        item {
            Text(
                text = "Workspaces",
                style = MaterialTheme.typography.titleSmall,
                color = MaterialTheme.colorScheme.primary,
                modifier = Modifier.fillMaxWidth(),
            )
        }

        items(workspaces) { ws ->
            WorkspaceCard(info = ws)
        }

        item { HorizontalDivider(modifier = Modifier.padding(vertical = 8.dp)) }

        item { VoiceSection() }

        item { HorizontalDivider(modifier = Modifier.padding(vertical = 8.dp)) }

        item { KeygenSection() }
    }
}

@Composable
fun VoiceSection() {
    val context = LocalContext.current
    var wakeEnabled by remember { mutableStateOf(false) }
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
        modifier = Modifier.fillMaxWidth(),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text(
            text = stringResource(R.string.voice_section_title),
            style = MaterialTheme.typography.titleSmall,
            color = MaterialTheme.colorScheme.primary,
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

        lastReply?.let {
            Text(
                it,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
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

@Composable
fun WorkspaceCard(info: ComradeCore.WorkspaceInfo) {
    OutlinedCard(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Text(
                text = info.key,
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.primary,
            )
            Spacer(Modifier.height(4.dp))
            Text(
                text = info.label,
                style = MaterialTheme.typography.bodyMedium,
            )
        }
    }
}

@Composable
fun KeygenSection() {
    var keypair by remember { mutableStateOf<ComradeCore.Keypair?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    // The nsec is masked by default and only shown on an explicit reveal tap.
    var revealNsec by remember { mutableStateOf(false) }

    Column(
        modifier = Modifier.fillMaxWidth(),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text(
            text = "Key Management",
            style = MaterialTheme.typography.titleSmall,
            color = MaterialTheme.colorScheme.primary,
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
