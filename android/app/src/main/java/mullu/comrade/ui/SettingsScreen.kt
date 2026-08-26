package mullu.comrade.ui

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.provider.Settings
import android.text.format.DateUtils
import android.text.format.Formatter
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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ElevatedCard
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedCard
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import mullu.comrade.BackgroundConnectivityPreference
import mullu.comrade.ComradeCore
import mullu.comrade.MutedChats
import mullu.comrade.R
import mullu.comrade.RelayConnectionService
import mullu.comrade.ScreenSecurity
import mullu.comrade.attention.QuietHours
import mullu.comrade.model.downloadPercent
import mullu.comrade.update.UpdateCheck
import mullu.comrade.update.UpdateChecker
import mullu.comrade.update.UpdateDownloadState
import mullu.comrade.update.UpdateDownloads
import mullu.comrade.update.UpdateStatus
import mullu.comrade.call.CallManager
import mullu.comrade.together.StreamingSources
import mullu.comrade.together.StreamingSourcesStore
import mullu.comrade.voice.CommandDispatcher
import mullu.comrade.voice.ComradeCoreBackend
import mullu.comrade.voice.ComradeTts
import mullu.comrade.voice.OneShotRecognizer
import mullu.comrade.voice.VoiceCommand
import mullu.comrade.voice.VoiceModelMissingException
import mullu.comrade.voice.VoskModel
import mullu.comrade.voice.WakeWordService
import mullu.comrade.ui.theme.ComradeRadii
import mullu.comrade.ui.theme.GlassElevation
import mullu.comrade.ui.theme.Spacing
import mullu.comrade.ui.theme.glassSurface

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
        ScreenshotSection()
        NotificationsSection()
        QuietHoursSection()
        UpdatesSection()
        TurnRelaySection()
        ShareRelaySection()

        MusicSourcesSection()

        TravelRatingsSection()
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
    val dialogShape = RoundedCornerShape(ComradeRadii.xl)

    AlertDialog(
        onDismissRequest = { if (!busy) onDismiss() },
        modifier = Modifier.glassSurface(GlassElevation.Sheet, shape = dialogShape),
        shape = dialogShape,
        containerColor = Color.Transparent,
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

// ── Screenshots (FLAG_SECURE) ────────────────────────────────────────────────

/**
 * Screenshots are allowed everywhere in the app by default; this is the switch
 * that turns them off.
 *
 * See [mullu.comrade.ScreenSecurity] for why the default changed — in short, the
 * unconditional `FLAG_SECURE` this replaces blocked every screen to protect key
 * material that no screen shows, and could never have stopped a photograph of
 * the display. The copy says exactly that, because a privacy control that
 * overstates itself is worse than none.
 *
 * Applied to the live window immediately, not on the next launch.
 */
@Composable
private fun ScreenshotSection() {
    val context = LocalContext.current
    val activity = context as? Activity
    var blocked by remember { mutableStateOf(ScreenSecurity.isBlocked(context)) }

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
                    stringResource(R.string.settings_block_screenshots_title),
                    style = MaterialTheme.typography.titleSmall,
                )
                Text(
                    stringResource(R.string.settings_block_screenshots_summary),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(top = 4.dp),
                )
            }
            Switch(
                checked = blocked,
                onCheckedChange = { checked ->
                    blocked = checked
                    ScreenSecurity.setBlocked(context, checked)
                    activity?.let { ScreenSecurity.apply(it, checked) }
                },
                modifier = Modifier.padding(start = 12.dp),
            )
        }
    }
}

// ── Sending files through a relay ────────────────────────────────────────────

/**
 * The 50 MB in the "small files only" option, as bytes.
 *
 * A plain `val`: Kotlin will not accept unsigned arithmetic in a `const val`,
 * and spelling the product out as a literal to satisfy that would trade a
 * readable number for one nobody can check at a glance.
 */
private val SHARE_SMALL_LIMIT_BYTES: ULong = 50uL * 1024uL * 1024uL

/**
 * What this device does when the only route to a peer is somebody else's relay.
 *
 * Separate from [TurnRelaySection] and deliberately so: a relayed *call* is a
 * few tens of kilobits and entirely reasonable, while a relayed film is
 * gigabytes through a machine that volunteered for neither. Turning this on
 * never affects calls, and the copy says so.
 *
 * The list is read back from core after every write rather than kept here, so
 * this screen cannot show a policy different from the one being enforced — and
 * a write that failed (a locked vault) is visible instead of silent.
 */
@Composable
private fun ShareRelaySection() {
    var policy by remember { mutableStateOf(ComradeCore.shareRelayPolicy()) }
    val options = listOf(
        uniffi.comrade_core.RelayPolicy.DirectOnly to R.string.settings_share_relay_never,
        uniffi.comrade_core.RelayPolicy.UnderBytes(SHARE_SMALL_LIMIT_BYTES) to
            R.string.settings_share_relay_small,
        uniffi.comrade_core.RelayPolicy.AskEachTime to R.string.settings_share_relay_ask,
        uniffi.comrade_core.RelayPolicy.Always to R.string.settings_share_relay_always,
    )

    OutlinedCard(Modifier.fillMaxWidth()) {
        Column(Modifier.fillMaxWidth().padding(16.dp)) {
            Text(
                stringResource(R.string.settings_share_relay_title),
                style = MaterialTheme.typography.titleSmall,
            )
            Text(
                stringResource(R.string.settings_share_relay_summary),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(top = 4.dp, bottom = 8.dp),
            )
            options.forEach { (option, label) ->
                Row(
                    modifier = Modifier.fillMaxWidth().padding(vertical = Spacing.space1),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    RadioButton(
                        selected = policy == option,
                        onClick = {
                            ComradeCore.setShareRelayPolicy(option)
                            // Read back rather than assume: what core reports is
                            // what core enforces.
                            policy = ComradeCore.shareRelayPolicy()
                        },
                    )
                    Text(
                        stringResource(label),
                        style = MaterialTheme.typography.bodyMedium,
                        modifier = Modifier.padding(start = 4.dp),
                    )
                }
            }
        }
    }
}

// ── Travel ratings ───────────────────────────────────────────────────────────

/**
 * The Google Places API key the Travel tab's ratings half needs.
 *
 * **Comrade ships no key and is not going to.** Review counts in the thousands
 * exist in one place the public can query, and it bills per request — a key
 * baked into an app binary is a key that gets scraped, billed to whoever owns
 * it, and revoked out from under every install. So the key is the user's, it
 * lives in the encrypted vault (`comrade_ui`'s `app_settings` tree, not a
 * plaintext preference), and it is write-only from this screen: the field never
 * reads an existing key back, only replaces or clears it.
 *
 * Without one the tab still works — OpenStreetMap places and Wikipedia facts
 * need no account — and says so on the guide itself rather than looking broken.
 */
@Composable
private fun TravelRatingsSection() {
    var configured by remember { mutableStateOf(runCatching { ComradeCore.travelRatingsConfigured() }.getOrDefault(false)) }
    var draft by remember { mutableStateOf("") }
    var failure by remember { mutableStateOf<String?>(null) }

    OutlinedCard(Modifier.fillMaxWidth()) {
        Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(
                stringResource(R.string.travel_api_key_title),
                style = MaterialTheme.typography.titleSmall,
            )
            Text(
                stringResource(R.string.travel_api_key_note),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                stringResource(
                    if (configured) R.string.travel_api_key_present else R.string.travel_api_key_absent,
                ),
                style = MaterialTheme.typography.labelMedium,
            )
            OutlinedTextField(
                value = draft,
                onValueChange = {
                    draft = it
                    failure = null
                },
                singleLine = true,
                label = { Text(stringResource(R.string.travel_api_key_hint)) },
                // Masked like any other credential field on this screen.
                visualTransformation = PasswordVisualTransformation(),
                modifier = Modifier.fillMaxWidth(),
            )
            failure?.let {
                Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(
                    onClick = {
                        runCatching { ComradeCore.setTravelApiKeyTyped(draft) }
                            .onSuccess {
                                draft = ""
                                failure = null
                            }
                            .onFailure { failure = it.message }
                        // Read back rather than assume: what core reports is
                        // what core will use.
                        configured = runCatching { ComradeCore.travelRatingsConfigured() }.getOrDefault(false)
                    },
                    enabled = draft.isNotBlank(),
                ) { Text(stringResource(R.string.travel_api_key_save)) }
                if (configured) {
                    OutlinedButton(
                        onClick = {
                            runCatching { ComradeCore.setTravelApiKeyTyped("") }
                                .onFailure { failure = it.message }
                            configured = runCatching { ComradeCore.travelRatingsConfigured() }.getOrDefault(false)
                        },
                    ) { Text(stringResource(R.string.travel_api_key_clear)) }
                }
            }
        }
    }
}

// ── Notifications ────────────────────────────────────────────────────────────

/**
 * What Comrade may notify about, and what it has been told to stay quiet about.
 *
 * The per-channel detail lives in system settings (Android owns that UI and
 * duplicating it would drift), so this card's job is the three things the OS
 * screen cannot tell the user: that notifications are switched off entirely,
 * how many conversations *this app* is muting, and that muting is device-local.
 */
@Composable
private fun NotificationsSection() {
    val context = LocalContext.current
    var enabled by remember { mutableStateOf(NotificationManagerCompat.from(context).areNotificationsEnabled()) }
    var mutedCount by remember { mutableStateOf(MutedChats.muted(context).size) }
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
        enabled = NotificationManagerCompat.from(context).areNotificationsEnabled()
    }
    val lifecycleOwner = LocalLifecycleOwner.current

    // Coming back from the system screen (or from a chat that was just muted)
    // must not leave a stale answer on this card.
    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) {
                enabled = NotificationManagerCompat.from(context).areNotificationsEnabled()
                mutedCount = MutedChats.muted(context).size
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }

    OutlinedCard(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(
                stringResource(R.string.settings_notifications_title),
                style = MaterialTheme.typography.titleSmall,
            )
            Text(
                stringResource(R.string.settings_notifications_summary),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            if (!enabled) {
                Text(
                    stringResource(R.string.settings_notifications_blocked),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.error,
                )
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    // A revoked permission can be re-requested; a channel the
                    // user switched off in system settings cannot, which is why
                    // the button below exists as well.
                    OutlinedButton(
                        onClick = { permission.launch(Manifest.permission.POST_NOTIFICATIONS) },
                        modifier = Modifier.fillMaxWidth(),
                    ) { Text(stringResource(R.string.settings_notifications_grant)) }
                }
            }
            Text(
                if (mutedCount == 0) {
                    stringResource(R.string.settings_notifications_muted_none)
                } else {
                    pluralStringResource(R.plurals.settings_notifications_muted_count, mutedCount, mutedCount)
                },
                style = MaterialTheme.typography.bodySmall,
            )
            Text(
                stringResource(R.string.settings_notifications_mute_scope),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            if (mutedCount > 0) {
                OutlinedButton(
                    onClick = {
                        MutedChats.unmuteAll(context)
                        mutedCount = 0
                    },
                    modifier = Modifier.fillMaxWidth(),
                ) { Text(stringResource(R.string.settings_notifications_unmute_all)) }
            }
            OutlinedButton(
                onClick = { context.startActivity(notificationSettingsIntent(context)) },
                modifier = Modifier.fillMaxWidth(),
            ) { Text(stringResource(R.string.settings_notifications_open_system)) }
        }
    }
}

// ── Quiet hours (docs/ATTENTION.md phase 0) ─────────────────────────────────

/**
 * The nightly window in which Comrade stays quiet.
 *
 * Opt-in, unlike the feed's gentle stop: silencing notifications can make
 * someone miss something they wanted, so it is theirs to choose. The copy says
 * plainly that a ringing call still comes through — a quiet-hours feature that
 * quietly swallowed a call would be a serious thing to discover by accident.
 *
 * The window is adjusted in whole hours from this card. A minute-precision time
 * picker would be more configurable and no more useful: nobody's sleep depends
 * on 22:15 versus 22:00, and the extra dialog is one more thing to get wrong.
 */
@Composable
private fun QuietHoursSection() {
    val context = LocalContext.current
    var enabled by remember { mutableStateOf(QuietHours.isEnabled(context)) }
    var start by remember { mutableStateOf(QuietHours.startMinute(context)) }
    var end by remember { mutableStateOf(QuietHours.endMinute(context)) }

    OutlinedCard(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(Modifier.weight(1f)) {
                    Text(
                        stringResource(R.string.settings_quiet_hours_title),
                        style = MaterialTheme.typography.titleSmall,
                    )
                    Text(
                        stringResource(R.string.settings_quiet_hours_summary),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(top = 4.dp),
                    )
                }
                Switch(
                    checked = enabled,
                    onCheckedChange = {
                        enabled = it
                        QuietHours.setEnabled(context, it)
                    },
                    modifier = Modifier.padding(start = 12.dp),
                )
            }
            if (enabled) {
                Text(
                    stringResource(
                        R.string.settings_quiet_hours_window,
                        QuietHours.formatMinute(start),
                        QuietHours.formatMinute(end),
                    ),
                    style = MaterialTheme.typography.bodyMedium,
                )
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    HourStepper(
                        label = stringResource(R.string.settings_quiet_hours_start),
                        minute = start,
                        onChange = {
                            start = it
                            QuietHours.setWindow(context, it, end)
                        },
                        modifier = Modifier.weight(1f),
                    )
                    HourStepper(
                        label = stringResource(R.string.settings_quiet_hours_end),
                        minute = end,
                        onChange = {
                            end = it
                            QuietHours.setWindow(context, start, it)
                        },
                        modifier = Modifier.weight(1f),
                    )
                }
                Text(
                    stringResource(R.string.settings_quiet_hours_device_only),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.outline,
                )
            }
        }
    }
}

/** −/+ one hour, wrapping at midnight. */
@Composable
private fun HourStepper(
    label: String,
    minute: Int,
    onChange: (Int) -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(modifier, verticalArrangement = Arrangement.spacedBy(Spacing.space1)) {
        Text(
            label,
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            TextButton(onClick = { onChange(stepHour(minute, -1)) }) { Text("−") }
            Text(QuietHours.formatMinute(minute), style = MaterialTheme.typography.bodyMedium)
            TextButton(onClick = { onChange(stepHour(minute, +1)) }) { Text("+") }
        }
    }
}

/** Move [minute] by [hours], wrapping within the day. */
private fun stepHour(minute: Int, hours: Int): Int {
    val stepped = minute + hours * 60
    val day = QuietHours.MINUTES_PER_DAY
    return ((stepped % day) + day) % day
}

/** The system's own per-app notification screen — where the channels live. */
private fun notificationSettingsIntent(context: Context): Intent =
    Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
        .putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName)
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

// ── App updates ──────────────────────────────────────────────────────────────

/**
 * Whether a newer Comrade exists, and how to get it.
 *
 * The card is the whole update UI: the notification only exists to bring
 * someone here. The update is downloaded **in-app** by
 * [mullu.comrade.update.UpdateDownloadService] — a foreground service, so
 * leaving this screen (or the app) does not stop it, and the shade shows the
 * progress and then a tappable "ready to install". The release page remains as
 * a fallback for a release with no single APK, and for anyone who would rather
 * not grant the install permission.
 */
@Composable
private fun UpdatesSection() {
    val context = LocalContext.current
    val status by UpdateChecker.status.collectAsState()
    val download by UpdateDownloads.state.collectAsState()
    var autoCheck by remember { mutableStateOf(UpdateChecker.isAutoCheckEnabled(context)) }
    var skipped by remember { mutableStateOf(UpdateChecker.skippedVersion(context)) }
    val lastChecked = remember(status) { UpdateChecker.lastCheckedAt(context) }
    // The install permission is granted outside this app, so it has to be
    // re-read when we come back — and a downloaded APK can vanish with the
    // cache, which would otherwise leave an Install button pointing at nothing.
    var canInstall by remember { mutableStateOf(UpdateChecker.canInstall(context)) }
    val lifecycleOwner = LocalLifecycleOwner.current
    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) {
                canInstall = UpdateChecker.canInstall(context)
                UpdateChecker.refreshDownloadState(context)
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }

    OutlinedCard(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(stringResource(R.string.settings_updates_title), style = MaterialTheme.typography.titleSmall)
            Text(
                stringResource(R.string.settings_updates_current, UpdateChecker.currentVersion),
                style = MaterialTheme.typography.bodySmall,
            )

            when (val current = status) {
                is UpdateStatus.Checking -> Text(
                    stringResource(R.string.settings_updates_checking),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )

                is UpdateStatus.Available -> {
                    Text(
                        stringResource(R.string.settings_updates_available, current.release.versionName),
                        style = MaterialTheme.typography.titleSmall,
                        color = MaterialTheme.colorScheme.primary,
                    )
                    if (current.release.notes.isNotBlank()) {
                        Text(
                            // Bounded: release notes in this repo run long, and
                            // a settings card is not a changelog viewer — the
                            // full text is one tap away on the release page.
                            current.release.notes.lineSequence().take(12).joinToString("\n"),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    UpdateDownloadControls(
                        release = current.release,
                        download = download,
                        canInstall = canInstall,
                        onGrantInstall = { UpdateChecker.openInstallPermissionSettings(context) },
                    )
                    OutlinedButton(
                        onClick = {
                            UpdateChecker.skip(context, current.release.versionName)
                            skipped = current.release.versionName
                        },
                        modifier = Modifier.fillMaxWidth(),
                    ) { Text(stringResource(R.string.settings_updates_skip)) }
                }

                is UpdateStatus.UpToDate -> Text(
                    stringResource(R.string.settings_updates_up_to_date, formatCheckedAt(context, current.checkedAt)),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )

                is UpdateStatus.Failed -> Text(
                    stringResource(R.string.settings_updates_failed, current.message),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.error,
                )

                UpdateStatus.Unknown -> Text(
                    if (lastChecked > 0L) {
                        stringResource(R.string.settings_updates_up_to_date, formatCheckedAt(context, lastChecked))
                    } else {
                        stringResource(R.string.settings_updates_never_checked)
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }

            skipped?.let { version ->
                Text(
                    stringResource(R.string.settings_updates_skipped, version),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                TextButton(
                    onClick = {
                        UpdateChecker.unskip(context)
                        skipped = null
                    },
                ) { Text(stringResource(R.string.settings_updates_unskip)) }
            }

            OutlinedButton(
                onClick = { UpdateChecker.maybeCheck(context, force = true) },
                enabled = status !is UpdateStatus.Checking,
                modifier = Modifier.fillMaxWidth(),
            ) { Text(stringResource(R.string.settings_updates_check_now)) }

            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(Modifier.weight(1f)) {
                    Text(
                        stringResource(R.string.settings_updates_auto_title),
                        style = MaterialTheme.typography.bodyMedium,
                    )
                    Text(
                        stringResource(R.string.settings_updates_auto_summary),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Switch(
                    checked = autoCheck,
                    onCheckedChange = {
                        autoCheck = it
                        UpdateChecker.setAutoCheckEnabled(context, it)
                    },
                    modifier = Modifier.padding(start = 12.dp),
                )
            }

            Text(
                stringResource(R.string.settings_updates_sideload_note),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.outline,
            )
        }
    }
}

/**
 * The download → verify → install run of buttons, driven by
 * [UpdateDownloads.state].
 *
 * One control at a time, always naming what the next tap does: **Download
 * update (12 MB)** → a progress bar with **Cancel download** → **Install
 * 0.0.51**. The release page sits underneath as a link, not a button, because
 * it is now the fallback rather than the path.
 */
@Composable
private fun UpdateDownloadControls(
    release: UpdateCheck.ReleaseInfo,
    download: UpdateDownloadState,
    canInstall: Boolean,
    onGrantInstall: () -> Unit,
) {
    val context = LocalContext.current

    when (download) {
        is UpdateDownloadState.Downloading -> {
            Text(
                stringResource(
                    R.string.settings_updates_downloading,
                    Formatter.formatShortFileSize(context, download.bytesRead),
                    Formatter.formatShortFileSize(
                        context,
                        download.totalBytes.coerceAtLeast(download.bytesRead),
                    ),
                ),
                style = MaterialTheme.typography.bodySmall,
            )
            if (download.totalBytes > 0) {
                LinearProgressIndicator(
                    progress = { downloadPercent(download.bytesRead, download.totalBytes) / 100f },
                    modifier = Modifier.fillMaxWidth(),
                )
            } else {
                LinearProgressIndicator(Modifier.fillMaxWidth())
            }
            OutlinedButton(
                onClick = { UpdateChecker.cancelDownload() },
                modifier = Modifier.fillMaxWidth(),
            ) { Text(stringResource(R.string.settings_updates_cancel_download)) }
        }

        UpdateDownloadState.Verifying -> {
            Text(
                stringResource(R.string.settings_updates_verifying),
                style = MaterialTheme.typography.bodySmall,
            )
            LinearProgressIndicator(Modifier.fillMaxWidth())
        }

        is UpdateDownloadState.Ready -> {
            Button(
                onClick = {
                    if (canInstall) UpdateChecker.install(context) else onGrantInstall()
                },
                modifier = Modifier.fillMaxWidth(),
            ) { Text(stringResource(R.string.settings_updates_install, download.version)) }
            if (!canInstall) InstallPermissionNote(onGrantInstall)
        }

        is UpdateDownloadState.Installing -> {
            // Two phases, and only the second is a wait with nothing to show:
            // while the APK is still being handed over there is real progress to
            // report, and reporting it is the difference between "installing" and
            // the several silent seconds this used to be.
            if (download.totalBytes > 0) {
                Text(
                    stringResource(
                        R.string.settings_updates_staging,
                        Formatter.formatShortFileSize(context, download.bytesStaged),
                        Formatter.formatShortFileSize(
                            context,
                            download.totalBytes.coerceAtLeast(download.bytesStaged),
                        ),
                    ),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                LinearProgressIndicator(
                    progress = { downloadPercent(download.bytesStaged, download.totalBytes) / 100f },
                    modifier = Modifier.fillMaxWidth(),
                )
            } else {
                Text(
                    stringResource(R.string.settings_updates_installing),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                // The app cannot tell a confirmation dialog that is up from one
                // the platform declined to show — a dropped activity start
                // reports nothing at all. Rather than leave someone stuck on a
                // line that says "waiting" forever, this hands it back to them.
                // Deliberately not offered during the copy above, where "nothing
                // is happening" is visibly false.
                OutlinedButton(
                    onClick = { UpdateChecker.retryInstall(context) },
                    modifier = Modifier.fillMaxWidth(),
                ) { Text(stringResource(R.string.settings_updates_install_retry)) }
            }
        }

        is UpdateDownloadState.Failed -> {
            Text(
                download.message,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.error,
            )
            DownloadButton(release)
            if (!canInstall) InstallPermissionNote(onGrantInstall)
        }

        UpdateDownloadState.Idle -> {
            DownloadButton(release)
            if (!canInstall) InstallPermissionNote(onGrantInstall)
        }
    }

    // Always available, and deliberately a text button: the release page is the
    // fallback (a release with no single APK, or someone who would rather not
    // grant the install permission), not the recommended path any more.
    TextButton(
        onClick = { UpdateChecker.openRelease(context, release) },
        modifier = Modifier.fillMaxWidth(),
    ) { Text(stringResource(R.string.settings_updates_open_page)) }
}

@Composable
private fun DownloadButton(release: UpdateCheck.ReleaseInfo) {
    val context = LocalContext.current
    Button(
        onClick = { UpdateChecker.download(context) },
        // A release with no single identifiable APK cannot be downloaded here;
        // the link below is the honest path for it.
        enabled = release.apkUrl != null,
        modifier = Modifier.fillMaxWidth(),
    ) {
        Text(
            if (release.apkBytes > 0) {
                stringResource(
                    R.string.settings_updates_download_sized,
                    Formatter.formatShortFileSize(context, release.apkBytes),
                )
            } else {
                stringResource(R.string.settings_updates_download)
            },
        )
    }
}

/**
 * Says why an install cannot happen yet, and offers the one screen that fixes
 * it. Shown next to the button rather than as a dialog on tap: the permission
 * is knowable in advance, so surprising someone with a system screen after they
 * tapped "Install" would be worse.
 */
@Composable
private fun InstallPermissionNote(onGrant: () -> Unit) {
    Text(
        stringResource(R.string.settings_updates_install_permission_title),
        style = MaterialTheme.typography.bodyMedium,
    )
    Text(
        stringResource(R.string.settings_updates_install_permission_body),
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
    OutlinedButton(onClick = onGrant, modifier = Modifier.fillMaxWidth()) {
        Text(stringResource(R.string.settings_updates_install_permission_open))
    }
}

/** "as of" wording for a check timestamp, in the device's own date/time format. */
private fun formatCheckedAt(context: Context, at: Long): String =
    if (at <= 0L) {
        context.getString(R.string.settings_updates_never_checked)
    } else {
        DateUtils.getRelativeTimeSpanString(
            at,
            System.currentTimeMillis(),
            DateUtils.MINUTE_IN_MILLIS,
        ).toString()
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
        val dialogShape = RoundedCornerShape(ComradeRadii.xl)
        AlertDialog(
            onDismissRequest = { if (!busy) confirming = false },
            modifier = Modifier.glassSurface(GlassElevation.Sheet, shape = dialogShape),
            shape = dialogShape,
            containerColor = Color.Transparent,
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

    val dialogShape = RoundedCornerShape(ComradeRadii.xl)
    AlertDialog(
        onDismissRequest = { if (!busy) onDismiss() },
        modifier = Modifier.glassSurface(GlassElevation.Sheet, shape = dialogShape),
        shape = dialogShape,
        containerColor = Color.Transparent,
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
    // The voice action waiting on the speech-model download; non-null keeps
    // the download dialog up, and the action runs once the model lands.
    var awaitingModel by remember { mutableStateOf<(() -> Unit)?>(null) }

    val tts = remember { ComradeTts(context) }
    val dispatcher = remember { CommandDispatcher(ComradeCoreBackend()) }
    val scope = rememberCoroutineScope()
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
                    // Off the main thread — dispatcher.handle can do blocking
                    // I/O (store reads, a relay round-trip for e.g. /pay),
                    // same as WakeWordService.dispatch()'s worker executor;
                    // the recognizer's onText callback runs on the main
                    // looper, which must stay free for the UI.
                    scope.launch {
                        val reply = withContext(Dispatchers.IO) {
                            runCatching { dispatcher.handle(VoiceCommand.parse(heard)) }
                                .getOrElse { "Something went wrong." }
                        }
                        lastReply = "“$heard” → $reply"
                        tts.speak(reply)
                        busy = false
                    }
                }
            },
            onError = {
                busy = false
                // Backstop for a model that vanished between the isAvailable
                // gate and listening (or an assets/uuid-less legacy build):
                // offer the download rather than a dead-end error.
                if (it is VoiceModelMissingException) {
                    awaitingModel = { runTapToTalk() }
                } else {
                    lastReply = "Voice unavailable: ${it.message}"
                }
            },
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

    // Voice needs the offline model AND the mic. Offer the model download
    // first (no permission needed to fetch it), then the permission prompt —
    // the Google-style on-device speech model flow.
    fun ensureVoiceReadyThen(action: () -> Unit) {
        if (VoskModel.isAvailable(context)) ensurePermissionThen(action) else awaitingModel = action
    }

    awaitingModel?.let { pending ->
        VoiceModelDownloadDialog(
            onReady = {
                awaitingModel = null
                ensurePermissionThen(pending)
            },
            onDismiss = {
                awaitingModel = null
                // An enable-wake tap that never started the service must not
                // leave the toggle looking on.
                wakeEnabled = WakeWordService.isRunning
            },
        )
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
                        ensureVoiceReadyThen { WakeWordService.start(context) }
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
                onClick = { ensureVoiceReadyThen { runTapToTalk() } },
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

// ── Music sources (docs/TOGETHER.md §23) ─────────────────────────────────────

/**
 * Where the Together tab's streaming sources get their credentials.
 *
 * One card, two resolvers:
 *
 * - **Your server** — a Subsonic-compatible host (Navidrome, Gonic, …) whose
 *   library streams to the Together tab and across a session. All three
 *   fields are required before the source card offers anything; core
 *   re-checks the address shape at search time and says which part is wrong.
 * - **Jamendo** — an optional key for the openly-licensed catalogue that
 *   gives the by-name search real, downloadable answers alongside
 *   MusicBrainz's metadata-only ones.
 *
 * Saved as one JSON blob through [StreamingSourcesStore], so the card can
 * never half-remember a config: it is either all there or not at all. The
 * password field is masked on screen like every other secret here, and it
 * travels only as a salted token inside stream URLs (see core's subsonic
 * module header for why that ordering matters).
 */
@Composable
private fun MusicSourcesSection() {
    val context = LocalContext.current
    var sources by remember {
        mutableStateOf(StreamingSourcesStore.load(context))
    }
    // Drafts, edited independently of what is saved: a search started from the
    // Together tab uses whatever was last saved here, never the unsent draft.
    var server by remember(sources) { mutableStateOf(sources.server) }
    var username by remember(sources) { mutableStateOf(sources.username) }
    var password by remember(sources) { mutableStateOf(sources.password) }
    var jamendo by remember(sources) { mutableStateOf(sources.jamendoClientId) }
    var saved by remember { mutableStateOf(false) }

    ElevatedCard(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(
                stringResource(R.string.settings_music_sources_title),
                style = MaterialTheme.typography.titleMedium,
            )
            Text(
                stringResource(R.string.settings_music_sources_explainer),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )

            OutlinedTextField(
                value = server,
                onValueChange = { server = it; saved = false },
                label = { Text(stringResource(R.string.settings_music_sources_server)) },
                placeholder = { Text("https://music.example.com") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedTextField(
                value = username,
                onValueChange = { username = it; saved = false },
                label = { Text(stringResource(R.string.settings_music_sources_username)) },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedTextField(
                value = password,
                onValueChange = { password = it; saved = false },
                label = { Text(stringResource(R.string.settings_music_sources_password)) },
                singleLine = true,
                visualTransformation = PasswordVisualTransformation(),
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedTextField(
                value = jamendo,
                onValueChange = { jamendo = it; saved = false },
                label = { Text(stringResource(R.string.settings_music_sources_jamendo)) },
                placeholder = { Text(stringResource(R.string.settings_music_sources_jamendo_hint)) },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            Text(
                stringResource(R.string.settings_music_sources_jamendo_note),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )

            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Button(
                    enabled = server.isNotBlank() || username.isNotBlank() ||
                        password.isNotBlank() || jamendo.isNotBlank(),
                    onClick = {
                        StreamingSourcesStore.save(
                            context,
                            StreamingSources(
                                server = server.trim(),
                                username = username.trim(),
                                password = password,
                                jamendoClientId = jamendo.trim(),
                            ),
                        )
                        sources = StreamingSourcesStore.load(context)
                        saved = true
                    },
                ) {
                    Text(stringResource(R.string.settings_music_sources_save))
                }
                OutlinedButton(
                    enabled = sources != StreamingSources.EMPTY,
                    onClick = {
                        StreamingSourcesStore.clear(context)
                        sources = StreamingSourcesStore.load(context)
                        server = ""; username = ""; password = ""; jamendo = ""
                        saved = false
                    },
                ) {
                    Text(stringResource(R.string.settings_music_sources_clear))
                }
            }
            if (saved) {
                Text(
                    stringResource(R.string.settings_music_sources_saved),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.primary,
                )
            }
        }
    }
}
