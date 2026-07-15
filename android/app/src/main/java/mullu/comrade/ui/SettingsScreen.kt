package mullu.comrade.ui

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ElevatedCard
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedCard
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import mullu.comrade.BackgroundConnectivityPreference
import mullu.comrade.ComradeCore
import mullu.comrade.R
import mullu.comrade.RelayConnectionService
import mullu.comrade.call.CallManager
import mullu.comrade.voice.CommandDispatcher
import mullu.comrade.voice.ComradeCoreBackend
import mullu.comrade.voice.ComradeTts
import mullu.comrade.voice.OneShotRecognizer
import mullu.comrade.voice.VoiceCommand
import mullu.comrade.voice.WakeWordService

@Composable
fun SettingsScreen(
    profile: ComradeCore.Profile,
    onProfileChange: (ComradeCore.Profile) -> Unit,
    onLock: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val clipboard = LocalClipboardManager.current
    var editing by remember { mutableStateOf(false) }
    var copied by remember { mutableStateOf(false) }
    val coreVersion = remember { runCatching { ComradeCore.getVersion() }.getOrDefault("?") }

    Column(
        modifier = modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = 20.dp, vertical = 16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        // ── Profile ───────────────────────────────────────────────────────
        ElevatedCard(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    PeerAvatar(profile.username ?: profile.npub, seed = profile.npub)
                    Column(Modifier.weight(1f)) {
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
                    TextButton(onClick = { editing = true }) { Text("Edit") }
                }
                Text(
                    "Your identity key",
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.primary,
                )
                Text(
                    profile.npub,
                    style = MaterialTheme.typography.bodySmall,
                    fontFamily = FontFamily.Monospace,
                )
                OutlinedButton(
                    onClick = {
                        clipboard.setText(AnnotatedString(profile.npub))
                        copied = true
                    },
                    modifier = Modifier.fillMaxWidth(),
                ) { Text(if (copied) "Copied ✓" else "Copy key") }
                Text(
                    "Usernames are display names and can repeat across the network. " +
                        "This key is what makes you *you* — share it so people can " +
                        "reach the real you even if someone copies your name.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }

        BackgroundConnectivitySection()
        TurnRelaySection()
        VaultLockSection(onLock = onLock)

        VoiceSection()

        // ── Experimental features (honest status) ─────────────────────────
        OutlinedCard(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(
                    "In the lab",
                    style = MaterialTheme.typography.titleSmall,
                    color = MaterialTheme.colorScheme.primary,
                )
                Text(
                    "Off-grid mesh connectivity is real: switch by voice " +
                        "(\"hey comrade, go off grid\") and the status bar shows " +
                        "nearby devices live. Actually chatting over the mesh, and " +
                        "the private shared space for couples, are still built and " +
                        "tested only at the engine level, not usable from the app " +
                        "yet. They'll appear here when they actually work — no " +
                        "fake switches.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(
                    "core v$coreVersion",
                    style = MaterialTheme.typography.labelSmall,
                    fontFamily = FontFamily.Monospace,
                    color = MaterialTheme.colorScheme.outline,
                    modifier = Modifier.padding(top = 8.dp),
                )
            }
        }
    }

    if (editing) {
        EditUsernameDialog(
            current = profile.username,
            onDismiss = { editing = false },
            onSaved = {
                editing = false
                onProfileChange(it)
            },
        )
    }
}

@Composable
private fun EditUsernameDialog(
    current: String?,
    onDismiss: () -> Unit,
    onSaved: (ComradeCore.Profile) -> Unit,
) {
    var value by remember { mutableStateOf(current ?: "") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()

    AlertDialog(
        onDismissRequest = { if (!busy) onDismiss() },
        title = { Text("Username") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(
                    value = value,
                    onValueChange = { value = it },
                    prefix = { Text("@") },
                    singleLine = true,
                    enabled = !busy,
                )
                Text(
                    "3–24 characters: letters, numbers, underscore.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                error?.let {
                    Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
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
                            withContext(Dispatchers.IO) { ComradeCore.setUsernameTyped(value) }
                        }.onSuccess {
                            busy = false
                            onSaved(it)
                        }.onFailure {
                            busy = false
                            error = it.message
                        }
                    }
                },
            ) { Text(if (busy) "Saving…" else "Save") }
        },
        dismissButton = {
            TextButton(enabled = !busy, onClick = onDismiss) { Text("Cancel") }
        },
    )
}

// ── Background connectivity (AUDIT.md COMMS-01) ─────────────────────────────

/**
 * Toggle for [RelayConnectionService] — on by default (it's what makes an
 * accepted DM or an incoming call notify you while the app is backgrounded
 * but unlocked), but the persistent low-priority notification and background
 * battery use are a real, visible tradeoff the user should be able to turn
 * off.
 */
@Composable
private fun BackgroundConnectivitySection() {
    val context = LocalContext.current
    var enabled by remember { mutableStateOf(BackgroundConnectivityPreference.isEnabled(context)) }

    OutlinedCard(Modifier.fillMaxWidth()) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(16.dp),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(Modifier.weight(1f)) {
                Text(
                    stringResource(R.string.settings_background_connectivity_title),
                    style = MaterialTheme.typography.titleSmall,
                )
                Text(
                    stringResource(R.string.settings_background_connectivity_summary),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(top = 4.dp),
                )
            }
            Switch(
                checked = enabled,
                onCheckedChange = { checked ->
                    enabled = checked
                    BackgroundConnectivityPreference.setEnabled(context, checked)
                    if (checked) RelayConnectionService.start(context) else RelayConnectionService.stop(context)
                },
                modifier = Modifier.padding(start = 12.dp),
            )
        }
    }
}

// ── Vault lock (AUDIT.md COMMS-01) ───────────────────────────────────────────

/** Lets the user drop the decrypted vault key from memory on demand, without waiting for the process to die. */
@Composable
private fun VaultLockSection(onLock: () -> Unit) {
    var confirming by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()

    OutlinedCard(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(stringResource(R.string.settings_lock_vault_title), style = MaterialTheme.typography.titleSmall)
            Text(
                stringResource(R.string.settings_lock_vault_summary),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            error?.let {
                Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
            }
            OutlinedButton(
                onClick = { confirming = true },
                modifier = Modifier.fillMaxWidth(),
            ) { Text(stringResource(R.string.settings_lock_vault_title)) }
        }
    }

    if (confirming) {
        AlertDialog(
            onDismissRequest = { if (!busy) confirming = false },
            title = { Text(stringResource(R.string.settings_lock_vault_title)) },
            text = { Text(stringResource(R.string.settings_lock_vault_summary)) },
            confirmButton = {
                TextButton(
                    enabled = !busy,
                    onClick = {
                        busy = true
                        error = null
                        scope.launch {
                            runCatching { withContext(Dispatchers.IO) { ComradeCore.lockVaultTyped() } }
                                .onSuccess {
                                    busy = false
                                    confirming = false
                                    onLock()
                                }
                                .onFailure {
                                    busy = false
                                    error = it.message ?: "Could not lock the vault."
                                }
                        }
                    },
                ) { Text(if (busy) "Locking…" else stringResource(R.string.settings_lock_vault_title)) }
            },
            dismissButton = {
                TextButton(enabled = !busy, onClick = { confirming = false }) { Text("Cancel") }
            },
        )
    }
}

// ── Calls relay / TURN (AUDIT.md COMMS-02) ───────────────────────────────────

@Composable
private fun TurnRelaySection() {
    var status by remember { mutableStateOf(ComradeCore.turnServerStatusTyped()) }
    var editing by remember { mutableStateOf(false) }
    var diagnostic by remember { mutableStateOf<CallManager.TurnDiagnostic?>(null) }
    var testing by remember { mutableStateOf(false) }
    val context = LocalContext.current
    val scope = rememberCoroutineScope()

    OutlinedCard(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(stringResource(R.string.settings_turn_title), style = MaterialTheme.typography.titleSmall)
            Text(
                stringResource(R.string.settings_turn_explainer),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                if (status.configured) {
                    stringResource(R.string.settings_turn_configured, status.url ?: "")
                } else {
                    stringResource(R.string.settings_turn_not_configured)
                },
                style = MaterialTheme.typography.bodySmall,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedButton(onClick = { editing = true }, modifier = Modifier.weight(1f)) {
                    Text(if (status.configured) "Edit" else "Configure")
                }
                OutlinedButton(
                    enabled = status.configured && !testing,
                    onClick = {
                        testing = true
                        diagnostic = null
                        scope.launch {
                            diagnostic = withContext(Dispatchers.IO) {
                                CallManager.testTurnConnectivity(context.applicationContext)
                            }
                            testing = false
                        }
                    },
                    modifier = Modifier.weight(1f),
                ) {
                    if (testing) {
                        CircularProgressIndicator(Modifier.size(16.dp))
                    } else {
                        Text(stringResource(R.string.settings_turn_test))
                    }
                }
            }
            diagnostic?.let {
                Text(
                    when (it) {
                        CallManager.TurnDiagnostic.NO_SERVER_CONFIGURED ->
                            stringResource(R.string.settings_turn_test_no_server)
                        CallManager.TurnDiagnostic.RELAY_AVAILABLE ->
                            stringResource(R.string.settings_turn_test_relay_available)
                        CallManager.TurnDiagnostic.RELAY_UNAVAILABLE ->
                            stringResource(R.string.settings_turn_test_relay_unavailable)
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color = if (it == CallManager.TurnDiagnostic.RELAY_UNAVAILABLE) {
                        MaterialTheme.colorScheme.error
                    } else {
                        MaterialTheme.colorScheme.onSurfaceVariant
                    },
                )
            }
        }
    }

    if (editing) {
        EditTurnServerDialog(
            current = status,
            onDismiss = { editing = false },
            onSaved = {
                status = it
                editing = false
                diagnostic = null
            },
        )
    }
}

@Composable
private fun EditTurnServerDialog(
    current: ComradeCore.TurnServerStatus,
    onDismiss: () -> Unit,
    onSaved: (ComradeCore.TurnServerStatus) -> Unit,
) {
    var url by remember { mutableStateOf(current.url ?: "") }
    // Username/credential are write-only (see ComradeCore.setTurnServerTyped) —
    // never pre-filled from a read-back value, because there isn't one.
    var username by remember { mutableStateOf("") }
    var credential by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()

    fun save(clear: Boolean) {
        busy = true
        error = null
        scope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    if (clear) {
                        ComradeCore.setTurnServerTyped("", "", "")
                    } else {
                        ComradeCore.setTurnServerTyped(url.trim(), username, credential)
                    }
                }
            }.onSuccess {
                busy = false
                onSaved(ComradeCore.turnServerStatusTyped())
            }.onFailure {
                busy = false
                // The Rust-side validation message only ever describes the
                // URL's shape — never the credential — so it's always safe
                // to show directly.
                error = it.message ?: "Could not save."
            }
        }
    }

    AlertDialog(
        onDismissRequest = { if (!busy) onDismiss() },
        title = { Text(stringResource(R.string.settings_turn_title)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(
                    value = url,
                    onValueChange = { url = it },
                    label = { Text(stringResource(R.string.settings_turn_url_label)) },
                    singleLine = true,
                    enabled = !busy,
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedTextField(
                    value = username,
                    onValueChange = { username = it },
                    label = { Text(stringResource(R.string.settings_turn_username_label)) },
                    singleLine = true,
                    enabled = !busy,
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedTextField(
                    value = credential,
                    onValueChange = { credential = it },
                    label = { Text(stringResource(R.string.settings_turn_credential_label)) },
                    singleLine = true,
                    enabled = !busy,
                    visualTransformation = PasswordVisualTransformation(),
                    modifier = Modifier.fillMaxWidth(),
                )
                error?.let {
                    Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                }
            }
        },
        confirmButton = {
            TextButton(enabled = !busy && url.isNotBlank(), onClick = { save(clear = false) }) {
                Text(if (busy) "Saving…" else stringResource(R.string.settings_turn_save))
            }
        },
        dismissButton = {
            Row {
                if (current.configured) {
                    TextButton(enabled = !busy, onClick = { save(clear = true) }) {
                        Text(stringResource(R.string.settings_turn_clear))
                    }
                }
                TextButton(enabled = !busy, onClick = onDismiss) { Text("Cancel") }
            }
        },
    )
}

// ── Voice assistant (moved from the old home column) ────────────────────────

@Composable
fun VoiceSection() {
    val context = LocalContext.current
    // Seeded from the service so the toggle survives navigation — this
    // composable is disposed whenever the user switches tabs.
    var wakeEnabled by remember { mutableStateOf(WakeWordService.isRunning) }
    var lastReply by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }

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

    val permissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions(),
    ) { grants ->
        val micGranted = grants[Manifest.permission.RECORD_AUDIO] == true || hasMic(context)
        if (micGranted) {
            if (wakeEnabled) WakeWordService.start(context) else runTapToTalk()
        } else {
            lastReply = context.getString(R.string.voice_permission_needed)
            wakeEnabled = false
        }
    }

    fun ensurePermissionThen(action: () -> Unit) {
        if (hasMic(context)) action() else permissionLauncher.launch(voicePermissions())
    }

    OutlinedCard(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(
                stringResource(R.string.voice_section_title),
                style = MaterialTheme.typography.titleSmall,
                color = MaterialTheme.colorScheme.primary,
            )
            Text(
                "Recognition runs offline on this phone — no audio ever leaves it. " +
                    "Try “post hello sabha” or “read my timeline”.",
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
            ) { Text(stringResource(R.string.voice_tap_to_talk)) }

            TextButton(
                onClick = { context.startActivity(assistSettingsIntent()) },
                modifier = Modifier.fillMaxWidth(),
            ) { Text(stringResource(R.string.voice_set_default_assistant)) }

            lastReply?.let {
                Text(
                    it,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
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
