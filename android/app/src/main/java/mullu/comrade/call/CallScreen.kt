package mullu.comrade.call

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.PowerManager
import android.util.Log
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import kotlinx.coroutines.delay
import mullu.comrade.R
import mullu.comrade.ui.CallEndIcon
import mullu.comrade.ui.CallIcon
import mullu.comrade.ui.FlipCameraIcon
import mullu.comrade.ui.MicIcon
import mullu.comrade.ui.PeerAvatar
import mullu.comrade.ui.SpeakerIcon
import mullu.comrade.ui.VideocamIcon
import mullu.comrade.ui.VideocamOffIcon
import org.webrtc.RendererCommon
import org.webrtc.SurfaceViewRenderer
import org.webrtc.VideoTrack

/*
 * The full-screen call UI. It observes [CallManager.state] and renders the four
 * call phases the task specifies — Ringing, Connecting, Active, Ended — plus the
 * local/remote WebRTC video via [SurfaceViewRenderer]. Accept is routed through
 * [onAccept] (the host gates the mic/camera runtime permission there); every
 * other control talks to [CallManager] directly.
 */

private val CallBackground = Color(0xFF0E1621)
private val AcceptGreen = Color(0xFF2E7D32)
private val HangupRed = Color(0xFFC62828)
private val ControlIdle = Color(0x33FFFFFF)
private val ControlActive = Color(0xFFFFFFFF)
private val WeakConnectionAmber = Color(0xFFFFA000)

/**
 * Host entry point: shows the call overlay when a call is in flight, nothing
 * when [CallUiState.Idle]. Callers place this last in their layout stack so it
 * covers the app while ringing/connected.
 */
@Composable
fun CallScreen(onAccept: () -> Unit, modifier: Modifier = Modifier) {
    val state by CallManager.state.collectAsState()
    when (val s = state) {
        is CallUiState.Idle -> Unit
        is CallUiState.Ringing -> CallOverlay(modifier) { RingingContent(s, onAccept) }
        // One branch — one composition subtree — for both in-call phases: if
        // Connecting and Active were separate branches, the transition between
        // them would dispose and recreate the whole video subtree (each
        // SurfaceViewRenderer re-inits and the remote sink detaches) right as
        // the first video frames arrive.
        is CallUiState.Connecting, is CallUiState.Active -> CallOverlay(modifier) {
            val active = s as? CallUiState.Active
            val connecting = s as? CallUiState.Connecting
            InCallContent(
                peer = active?.peer ?: connecting!!.peer,
                peerLabel = active?.peerLabel ?: connecting!!.peerLabel,
                video = active?.video ?: connecting!!.video,
                status = if (active == null) stringOf(R.string.call_connecting) else null,
                connectedAtMs = active?.connectedAtMs ?: 0L,
            )
        }
        is CallUiState.Ended -> CallOverlay(modifier) { EndedContent(s) }
    }
}

@Composable
private fun CallOverlay(modifier: Modifier, content: @Composable () -> Unit) {
    Box(
        modifier = modifier
            .fillMaxSize()
            .background(CallBackground),
    ) { content() }
}

// ── Ringing ───────────────────────────────────────────────────────────────────

@Composable
private fun RingingContent(s: CallUiState.Ringing, onAccept: () -> Unit) {
    val status = when {
        s.incoming && s.video -> stringOf(R.string.call_incoming_video)
        s.incoming -> stringOf(R.string.call_incoming_voice)
        s.remoteRinging -> stringOf(R.string.call_ringing)
        else -> stringOf(R.string.call_calling)
    }
    Column(Modifier.fillMaxSize()) {
        PeerHeader(s.peer, s.peerLabel, status, Modifier.weight(1f).padding(top = 72.dp))
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 44.dp, vertical = 44.dp),
            horizontalArrangement = if (s.incoming) Arrangement.SpaceBetween else Arrangement.Center,
            verticalAlignment = Alignment.Top,
        ) {
            if (s.incoming) {
                CallActionButton(
                    icon = CallEndIcon,
                    desc = stringOf(R.string.call_decline),
                    bg = HangupRed,
                    size = 68.dp,
                    label = stringOf(R.string.call_decline),
                ) { CallManager.reject() }
                CallActionButton(
                    icon = if (s.video) VideocamIcon else CallIcon,
                    desc = stringOf(R.string.call_accept),
                    bg = AcceptGreen,
                    size = 68.dp,
                    label = stringOf(R.string.call_accept),
                    onClick = onAccept,
                )
            } else {
                CallActionButton(
                    icon = CallEndIcon,
                    desc = stringOf(R.string.call_cancel),
                    bg = HangupRed,
                    size = 68.dp,
                    label = stringOf(R.string.call_cancel),
                ) { CallManager.hangup() }
            }
        }
    }
}

// ── Connecting / Active ─────────────────────────────────────────────────────────

@Composable
private fun InCallContent(
    peer: String,
    peerLabel: String,
    video: Boolean,
    status: String?,
    connectedAtMs: Long = 0L,
) {
    val muted by CallManager.muted.collectAsState()
    val cameraOn by CallManager.cameraOn.collectAsState()
    val audioRoute by CallManager.audioRoute.collectAsState()
    val availableRoutes by CallManager.availableRoutes.collectAsState()
    val remoteVideo by CallManager.remoteVideo.collectAsState()
    val localVideo by CallManager.localVideo.collectAsState()
    val connectionQuality by CallManager.connectionQuality.collectAsState()
    val sasEmojis by CallManager.sasEmojis.collectAsState()

    // Audio calls: let the proximity sensor blank the screen when held to the ear.
    ProximityScreenControl(active = !video && status == null)

    val label = if (status != null) status else durationLabel(connectedAtMs)
    // Quality stats are only meaningful once the call is actually Active —
    // status == null is this composable's existing "are we Active" signal
    // (see ProximityScreenControl above). GOOD/UNKNOWN stay silent so the
    // indicator doesn't add visual noise to the common case.
    val showWeakConnection = status == null &&
        (connectionQuality == CallQuality.MEDIUM || connectionQuality == CallQuality.POOR)
    // Same Active-only gating as showWeakConnection; a missing fingerprint on
    // either side is a valid "can't verify" and shown as simply nothing.
    val sas = sasEmojis?.takeIf { status == null && it.isNotEmpty() }

    Box(Modifier.fillMaxSize()) {
        if (video) {
            // Tap the picture-in-picture tile to swap it with the full-screen
            // view (Telegram-style). `swapped` only changes which track renders
            // where — the underlying tracks/renderers are unaffected.
            var swapped by remember { mutableStateOf(false) }
            val mainTrack = if (swapped) localVideo else remoteVideo
            val pipTrack = if (swapped) remoteVideo else localVideo
            val pipIsLocal = !swapped

            if (mainTrack != null) {
                // The local track is the front camera preview, so it mirrors
                // wherever it renders; the remote track never mirrors.
                VideoRenderer(mainTrack, mirror = swapped, modifier = Modifier.fillMaxSize())
            } else {
                // No frames for the big view yet (the peer's video hasn't
                // arrived, typically while still Connecting) — show who the
                // call is with instead of a raw black screen.
                PeerHeader(peer, peerLabel, status = null, modifier = Modifier.align(Alignment.Center))
            }

            // Self-preview tile: swaps on tap; hosts the camera-flip control so
            // the bottom bar keeps exactly one camera button.
            Box(
                modifier = Modifier
                    .align(Alignment.TopEnd)
                    .padding(16.dp)
                    .size(width = 110.dp, height = 156.dp)
                    .clip(RoundedCornerShape(14.dp))
                    .background(Color(0xFF17212B))
                    .clickable(onClickLabel = stringOf(R.string.call_swap_video)) { swapped = !swapped },
            ) {
                if (pipTrack != null && (cameraOn || !pipIsLocal)) {
                    VideoRenderer(
                        track = pipTrack,
                        mirror = pipIsLocal,
                        modifier = Modifier.fillMaxSize(),
                        // Two overlapping SurfaceViews have no defined z-order
                        // between their surfaces unless the small one is
                        // explicitly marked as a media overlay — without this
                        // the tile can composite under the full-screen surface
                        // and simply never show.
                        zOrderMediaOverlay = true,
                    )
                } else {
                    Icon(
                        VideocamOffIcon,
                        contentDescription = null,
                        tint = Color(0x66FFFFFF),
                        modifier = Modifier
                            .size(30.dp)
                            .align(Alignment.Center),
                    )
                }
                if (pipIsLocal && cameraOn) {
                    CallActionButton(
                        icon = FlipCameraIcon,
                        desc = stringOf(R.string.call_switch_camera),
                        bg = Color(0x66000000),
                        size = 34.dp,
                        modifier = Modifier
                            .align(Alignment.BottomCenter)
                            .padding(bottom = 6.dp),
                    ) { CallManager.switchCamera() }
                }
            }

            // Name + duration/status pill, kept clear of the self-preview tile.
            Column(
                modifier = Modifier
                    .align(Alignment.TopStart)
                    .padding(start = 16.dp, top = 20.dp, end = 142.dp),
            ) {
                Column(
                    modifier = Modifier
                        .clip(RoundedCornerShape(14.dp))
                        .background(Color(0x66000000))
                        .padding(horizontal = 14.dp, vertical = 8.dp),
                ) {
                    Text(
                        peerLabel,
                        color = Color.White,
                        fontSize = 15.sp,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Text(
                        text = label,
                        color = Color(0xFFB0BEC5),
                        fontSize = 13.sp,
                        fontFamily = FontFamily.Monospace,
                    )
                }
                if (showWeakConnection) {
                    Spacer(Modifier.height(6.dp))
                    ConnectionQualityBadge(connectionQuality)
                }
                if (sas != null) {
                    Spacer(Modifier.height(6.dp))
                    SasRow(emojis = sas)
                }
            }
        } else {
            PeerHeader(peer, peerLabel, label, Modifier.align(Alignment.Center)) {
                if (showWeakConnection) {
                    Spacer(Modifier.height(6.dp))
                    ConnectionQualityBadge(connectionQuality)
                }
                if (sas != null) {
                    Spacer(Modifier.height(6.dp))
                    SasRow(emojis = sas)
                }
            }
        }

        // Controls: mute, audio route, (camera on/off), hang up — uniform
        // sizes with labels beneath; over video they sit on a scrim so they
        // stay legible on top of bright frames.
        Column(
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .then(
                    if (video) {
                        Modifier.background(
                            Brush.verticalGradient(listOf(Color.Transparent, Color(0xB3000000))),
                        )
                    } else {
                        Modifier
                    },
                ),
        ) {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 24.dp)
                    .padding(top = 28.dp, bottom = 36.dp),
                horizontalArrangement = Arrangement.spacedBy(26.dp, Alignment.CenterHorizontally),
                verticalAlignment = Alignment.Top,
            ) {
                CallActionButton(
                    icon = MicIcon,
                    desc = stringOf(if (muted) R.string.call_unmute else R.string.call_mute),
                    bg = if (muted) ControlActive else ControlIdle,
                    tint = if (muted) CallBackground else Color.White,
                    size = 60.dp,
                    label = stringOf(if (muted) R.string.call_unmute else R.string.call_mute),
                ) { CallManager.toggleMute() }
                AudioRouteButton(audioRoute, availableRoutes)
                if (video) {
                    CallActionButton(
                        icon = if (cameraOn) VideocamIcon else VideocamOffIcon,
                        desc = stringOf(if (cameraOn) R.string.call_camera_off else R.string.call_camera_on),
                        bg = if (!cameraOn) ControlActive else ControlIdle,
                        tint = if (!cameraOn) CallBackground else Color.White,
                        size = 60.dp,
                        label = stringOf(R.string.call_camera),
                    ) { CallManager.toggleCamera() }
                }
                CallActionButton(
                    icon = CallEndIcon,
                    desc = stringOf(R.string.call_hang_up),
                    bg = HangupRed,
                    size = 60.dp,
                    label = stringOf(R.string.call_end_label),
                ) { CallManager.hangup() }
            }
        }
    }
}

// ── Ended ─────────────────────────────────────────────────────────────────────

@Composable
private fun EndedContent(s: CallUiState.Ended) {
    val labelRes = when (s.outcome) {
        "missed" -> R.string.call_no_answer
        "failed" -> R.string.call_couldnt_connect
        "declined" -> R.string.call_declined_ended
        "busy" -> R.string.call_busy_ended
        "cancelled" -> R.string.call_cancelled_ended
        else -> R.string.call_ended
    }
    Column(
        modifier = Modifier.fillMaxSize(),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        PeerAvatar(s.peerLabel, seed = s.peer, size = 96.dp)
        Spacer(Modifier.height(20.dp))
        Text(s.peerLabel, color = Color.White, fontSize = 22.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Spacer(Modifier.height(8.dp))
        Text(stringOf(labelRes), color = Color(0xFFB0BEC5), fontSize = 15.sp)
    }
}

// ── Shared pieces ───────────────────────────────────────────────────────────────

@Composable
private fun PeerHeader(
    peer: String,
    peerLabel: String,
    status: String?,
    modifier: Modifier,
    extra: @Composable () -> Unit = {},
) {
    Column(
        modifier = modifier.fillMaxWidth(),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        PeerAvatar(peerLabel, seed = peer, size = 112.dp)
        Spacer(Modifier.height(24.dp))
        Text(
            peerLabel,
            color = Color.White,
            fontSize = 26.sp,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            textAlign = TextAlign.Center,
        )
        if (status != null) {
            Spacer(Modifier.height(10.dp))
            Text(status, color = Color(0xFFB0BEC5), fontSize = 16.sp, textAlign = TextAlign.Center)
        }
        extra()
    }
}

/**
 * A small, unobtrusive dot + label shown only while a call is Active and the
 * connection has degraded — see [CallManager.connectionQuality]. Callers gate
 * on Active/MEDIUM-POOR themselves; this composable just renders the dot
 * (amber for MEDIUM, red for POOR) and the "weak connection" text next to it.
 */
@Composable
private fun ConnectionQualityBadge(quality: CallQuality) {
    val dotColor = if (quality == CallQuality.POOR) HangupRed else WeakConnectionAmber
    Row(verticalAlignment = Alignment.CenterVertically) {
        Box(
            modifier = Modifier
                .size(8.dp)
                .clip(CircleShape)
                .background(dotColor),
        )
        Spacer(Modifier.width(6.dp))
        Text(stringOf(R.string.call_weak_connection), color = Color.White, fontSize = 13.sp)
    }
}

/**
 * The 4-emoji short authentication string ("Verify: 🐶 🦊 …"), shown only
 * while [CallManager.sasEmojis] has a non-empty value for the current
 * (Active) call. This is a real security signal, not decoration: it is
 * derived from both sides' DTLS-SRTP certificate fingerprints, so the same 4
 * emoji appearing on both phones is what rules out a man-in-the-middle on
 * the call's media path — tapping the row explains that rather than leaving
 * it unexplained.
 */
@Composable
private fun SasRow(emojis: List<String>, modifier: Modifier = Modifier) {
    var showInfo by remember { mutableStateOf(false) }
    Row(
        modifier = modifier
            .clip(RoundedCornerShape(14.dp))
            .background(ControlIdle)
            .clickable(onClickLabel = stringOf(R.string.call_sas_explain_title)) { showInfo = true }
            .padding(horizontal = 14.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(stringOf(R.string.call_sas_label), color = Color(0xFFB0BEC5), fontSize = 13.sp)
        Spacer(Modifier.width(8.dp))
        Text(emojis.joinToString(" "), fontSize = 20.sp)
    }
    if (showInfo) {
        AlertDialog(
            onDismissRequest = { showInfo = false },
            confirmButton = {
                TextButton(onClick = { showInfo = false }) { Text(stringOf(R.string.call_sas_dismiss)) }
            },
            title = { Text(stringOf(R.string.call_sas_explain_title)) },
            text = { Text(stringOf(R.string.call_sas_explain_body)) },
        )
    }
}

@Composable
private fun CallActionButton(
    icon: ImageVector,
    desc: String,
    bg: Color,
    tint: Color = Color.White,
    size: Dp = 64.dp,
    label: String? = null,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    Column(modifier = modifier, horizontalAlignment = Alignment.CenterHorizontally) {
        Box(
            modifier = Modifier
                .size(size)
                .clip(CircleShape)
                .background(bg)
                .clickable(onClick = onClick),
            contentAlignment = Alignment.Center,
        ) {
            Icon(icon, contentDescription = desc, tint = tint, modifier = Modifier.size(size * 0.44f))
        }
        if (label != null) {
            Spacer(Modifier.height(6.dp))
            Text(label, color = Color(0xB3FFFFFF), fontSize = 12.sp, maxLines = 1)
        }
    }
}

/**
 * The audio-output control: a button showing the current [AudioRoute] that
 * opens a menu of every currently-present route ([availableRoutes] — earpiece
 * and speaker are always listed; Bluetooth/wired only while connected).
 * Tapping an entry calls [CallManager.setAudioRoute] — except Bluetooth on
 * API 31+, which needs `BLUETOOTH_CONNECT` requested first (AUDIT.md
 * COMMS-06): granted, it proceeds exactly the same; denied, this reports it
 * to [CallManager.onBluetoothPermissionDenied] (which drops Bluetooth from
 * [availableRoutes] for the rest of the call) and explains the fallback via
 * a toast rather than leaving a tap silently do nothing.
 */
@Composable
private fun AudioRouteButton(current: AudioRoute, availableRoutes: List<AudioRoute>) {
    var expanded by remember { mutableStateOf(false) }
    val active = current != AudioRoute.EARPIECE
    val context = LocalContext.current
    val bluetoothPermission = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { granted ->
        if (granted) {
            CallManager.setAudioRoute(AudioRoute.BLUETOOTH)
        } else {
            CallManager.onBluetoothPermissionDenied()
            Toast.makeText(context, R.string.call_route_bluetooth_denied, Toast.LENGTH_LONG).show()
        }
    }
    fun selectRoute(route: AudioRoute) {
        val needsBluetoothPermission = route == AudioRoute.BLUETOOTH &&
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.S &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH_CONNECT) !=
                PackageManager.PERMISSION_GRANTED
        if (needsBluetoothPermission) {
            bluetoothPermission.launch(Manifest.permission.BLUETOOTH_CONNECT)
        } else {
            CallManager.setAudioRoute(route)
        }
    }
    Box {
        CallActionButton(
            icon = SpeakerIcon,
            desc = stringOf(R.string.call_speaker) + ": " + audioRouteLabel(current),
            bg = if (active) ControlActive else ControlIdle,
            tint = if (active) CallBackground else Color.White,
            size = 60.dp,
            label = audioRouteLabel(current),
        ) { expanded = true }
        DropdownMenu(expanded = expanded, onDismissRequest = { expanded = false }) {
            availableRoutes.forEach { route ->
                DropdownMenuItem(
                    text = { Text(audioRouteLabel(route)) },
                    onClick = {
                        selectRoute(route)
                        expanded = false
                    },
                )
            }
        }
    }
}

@Composable
private fun audioRouteLabel(route: AudioRoute): String = when (route) {
    AudioRoute.EARPIECE -> stringOf(R.string.call_route_earpiece)
    AudioRoute.SPEAKER -> stringOf(R.string.call_route_speaker)
    AudioRoute.BLUETOOTH -> stringOf(R.string.call_route_bluetooth)
    AudioRoute.WIRED -> stringOf(R.string.call_route_wired)
}

/**
 * A [SurfaceViewRenderer] wrapped for Compose. Initialises against the shared
 * [CallManager.eglBaseContext], sinks [track] while it's present, and releases
 * the renderer on disposal — the GPU/native cleanup the webview got for free.
 */
@Composable
private fun VideoRenderer(
    track: VideoTrack?,
    mirror: Boolean,
    modifier: Modifier,
    zOrderMediaOverlay: Boolean = false,
) {
    val egl = CallManager.eglBaseContext
    if (egl == null) {
        Box(modifier.background(Color.Black))
        return
    }
    val context = LocalContext.current
    val renderer = remember {
        SurfaceViewRenderer(context).apply {
            init(egl, null)
            setEnableHardwareScaler(true)
            setScalingType(RendererCommon.ScalingType.SCALE_ASPECT_FILL)
            // Must be set before the view attaches: the picture-in-picture
            // tile's surface has to be explicitly stacked above the
            // full-screen renderer's surface, or their z-order is undefined.
            setZOrderMediaOverlay(zOrderMediaOverlay)
        }
    }
    DisposableEffect(renderer) {
        onDispose { renderer.release() }
    }
    DisposableEffect(track, renderer) {
        track?.addSink(renderer)
        onDispose { track?.removeSink(renderer) }
    }
    // Mirror is applied in update (not just at creation): it flips whenever the
    // user swaps which tile shows the front-camera preview.
    AndroidView(factory = { renderer }, update = { it.setMirror(mirror) }, modifier = modifier)
}

/**
 * Screen-off proximity wake lock, held while [active] (an audio call in
 * progress): brings the screen down when the phone is at the ear so a cheek
 * can't hang up, and keeps audio on the earpiece. Video calls keep the screen on.
 */
@Composable
private fun ProximityScreenControl(active: Boolean) {
    val context = LocalContext.current
    DisposableEffect(active) {
        var lock: PowerManager.WakeLock? = null
        if (active) {
            val pm = context.getSystemService(Context.POWER_SERVICE) as? PowerManager
            if (pm != null && pm.isWakeLockLevelSupported(PowerManager.PROXIMITY_SCREEN_OFF_WAKE_LOCK)) {
                lock = pm.newWakeLock(PowerManager.PROXIMITY_SCREEN_OFF_WAKE_LOCK, "comrade:call")
                lock.acquire(60 * 60 * 1000L)
            }
        }
        onDispose {
            // Some vendor ROMs silently clear proximity wake locks themselves;
            // releasing an already-cleared lock throws RuntimeException.
            runCatching { if (lock?.isHeld == true) lock.release() }
                .onFailure { Log.w("ProximityControl", "wake lock already released by the platform", it) }
        }
    }
}

@Composable
private fun durationLabel(connectedAtMs: Long): String {
    if (connectedAtMs <= 0L) return stringOf(R.string.call_connecting)
    var now by remember { mutableStateOf(System.currentTimeMillis()) }
    LaunchedEffect(connectedAtMs) {
        while (true) {
            now = System.currentTimeMillis()
            delay(500)
        }
    }
    val secs = ((now - connectedAtMs) / 1000).coerceAtLeast(0)
    val hours = secs / 3600
    return if (hours > 0) {
        "%d:%02d:%02d".format(hours, (secs % 3600) / 60, secs % 60)
    } else {
        "%d:%02d".format(secs / 60, secs % 60)
    }
}

@Composable
private fun stringOf(resId: Int): String = LocalContext.current.getString(resId)
