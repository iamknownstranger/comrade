package mullu.comrade.call

import android.content.Context
import android.os.PowerManager
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
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
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
import kotlinx.coroutines.delay
import mullu.comrade.R
import mullu.comrade.ui.CallEndIcon
import mullu.comrade.ui.CallIcon
import mullu.comrade.ui.MicIcon
import mullu.comrade.ui.PeerAvatar
import mullu.comrade.ui.SpeakerIcon
import mullu.comrade.ui.VideocamIcon
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
        is CallUiState.Connecting -> CallOverlay(modifier) {
            InCallContent(
                peer = s.peer,
                peerLabel = s.peerLabel,
                video = s.video,
                status = stringOf(R.string.call_connecting),
            )
        }
        is CallUiState.Active -> CallOverlay(modifier) {
            InCallContent(
                peer = s.peer,
                peerLabel = s.peerLabel,
                video = s.video,
                status = null,
                connectedAtMs = s.connectedAtMs,
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
        else -> stringOf(R.string.call_calling)
    }
    Column(Modifier.fillMaxSize()) {
        PeerHeader(s.peer, s.peerLabel, status, Modifier.weight(1f))
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 32.dp, vertical = 48.dp),
            horizontalArrangement = if (s.incoming) Arrangement.SpaceBetween else Arrangement.Center,
        ) {
            if (s.incoming) {
                CallActionButton(CallEndIcon, stringOf(R.string.call_decline), HangupRed) { CallManager.reject() }
                CallActionButton(
                    icon = if (s.video) VideocamIcon else CallIcon,
                    desc = stringOf(R.string.call_accept),
                    bg = AcceptGreen,
                    onClick = onAccept,
                )
            } else {
                CallActionButton(CallEndIcon, stringOf(R.string.call_cancel), HangupRed) { CallManager.hangup() }
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
    val speaker by CallManager.speakerphone.collectAsState()
    val remoteVideo by CallManager.remoteVideo.collectAsState()
    val localVideo by CallManager.localVideo.collectAsState()

    // Audio calls: let the proximity sensor blank the screen when held to the ear.
    ProximityScreenControl(active = !video && status == null)

    val label = if (status != null) status else durationLabel(connectedAtMs)

    Box(Modifier.fillMaxSize()) {
        if (video) {
            VideoRenderer(remoteVideo, mirror = false, modifier = Modifier.fillMaxSize())
            // Self-preview, picture-in-picture, top-end.
            VideoRenderer(
                track = localVideo,
                mirror = true,
                modifier = Modifier
                    .align(Alignment.TopEnd)
                    .padding(16.dp)
                    .size(width = 108.dp, height = 152.dp)
                    .clip(RoundedCornerShape(12.dp)),
            )
            Text(
                text = label,
                color = Color.White,
                fontFamily = FontFamily.Monospace,
                modifier = Modifier
                    .align(Alignment.TopStart)
                    .padding(20.dp),
            )
        } else {
            PeerHeader(peer, peerLabel, label, Modifier.align(Alignment.Center))
        }

        // Controls: mute, speaker, (switch camera), hang up.
        Row(
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .padding(horizontal = 24.dp, vertical = 40.dp),
            horizontalArrangement = Arrangement.spacedBy(20.dp, Alignment.CenterHorizontally),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            CallActionButton(
                icon = MicIcon,
                desc = stringOf(if (muted) R.string.call_unmute else R.string.call_mute),
                bg = if (muted) ControlActive else ControlIdle,
                tint = if (muted) CallBackground else Color.White,
                size = 56.dp,
            ) { CallManager.toggleMute() }
            CallActionButton(
                icon = SpeakerIcon,
                desc = stringOf(R.string.call_speaker),
                bg = if (speaker) ControlActive else ControlIdle,
                tint = if (speaker) CallBackground else Color.White,
                size = 56.dp,
            ) { CallManager.toggleSpeaker() }
            if (video) {
                CallActionButton(
                    icon = VideocamIcon,
                    desc = stringOf(R.string.call_switch_camera),
                    bg = ControlIdle,
                    size = 56.dp,
                ) { CallManager.switchCamera() }
            }
            CallActionButton(CallEndIcon, stringOf(R.string.call_hang_up), HangupRed, size = 64.dp) {
                CallManager.hangup()
            }
        }
    }
}

// ── Ended ─────────────────────────────────────────────────────────────────────

@Composable
private fun EndedContent(s: CallUiState.Ended) {
    Column(
        modifier = Modifier.fillMaxSize(),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Text(s.peerLabel, color = Color.White, fontSize = 22.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Spacer(Modifier.height(8.dp))
        Text(stringOf(R.string.call_ended), color = Color(0xFFB0BEC5), fontSize = 15.sp)
    }
}

// ── Shared pieces ───────────────────────────────────────────────────────────────

@Composable
private fun PeerHeader(peer: String, peerLabel: String, status: String, modifier: Modifier) {
    Column(
        modifier = modifier
            .fillMaxWidth()
            .padding(top = 72.dp),
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
        Spacer(Modifier.height(10.dp))
        Text(status, color = Color(0xFFB0BEC5), fontSize = 16.sp, textAlign = TextAlign.Center)
    }
}

@Composable
private fun CallActionButton(
    icon: ImageVector,
    desc: String,
    bg: Color,
    tint: Color = Color.White,
    size: Dp = 64.dp,
    onClick: () -> Unit,
) {
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
}

/**
 * A [SurfaceViewRenderer] wrapped for Compose. Initialises against the shared
 * [CallManager.eglBaseContext], sinks [track] while it's present, and releases
 * the renderer on disposal — the GPU/native cleanup the webview got for free.
 */
@Composable
private fun VideoRenderer(track: VideoTrack?, mirror: Boolean, modifier: Modifier) {
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
            setMirror(mirror)
        }
    }
    DisposableEffect(renderer) {
        onDispose { renderer.release() }
    }
    DisposableEffect(track, renderer) {
        track?.addSink(renderer)
        onDispose { track?.removeSink(renderer) }
    }
    AndroidView(factory = { renderer }, modifier = modifier)
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
        onDispose { if (lock?.isHeld == true) lock.release() }
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
    return "%d:%02d".format(secs / 60, secs % 60)
}

@Composable
private fun stringOf(resId: Int): String = LocalContext.current.getString(resId)
