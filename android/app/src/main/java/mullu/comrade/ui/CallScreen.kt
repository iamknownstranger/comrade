package mullu.comrade.ui

import android.view.ViewGroup
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import mullu.comrade.call.CallManager
import org.webrtc.RendererCommon
import org.webrtc.SurfaceViewRenderer

/**
 * The full-screen call UI, overlaid on the app whenever [CallManager] holds an
 * active call. It renders four visually distinct states — Ringing (incoming),
 * Outgoing/Connecting, Active, and Ended — and binds its buttons straight to
 * the [CallManager] intents (accept / reject / hang up / mute / speaker /
 * camera). For a video call it hosts two WebRTC [SurfaceViewRenderer]s (remote
 * full-bleed, local picture-in-picture); audio calls show the peer's avatar.
 */
@Composable
fun CallScreen(
    onFinished: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    val state by CallManager.state.collectAsState()

    // Warm the WebRTC factory/EGL context so video renderers can init even
    // while a call is only ringing. Idempotent + off the main thread.
    LaunchedEffect(Unit) { withContext(Dispatchers.Default) { CallManager.init(context) } }

    // When the call returns to Idle, dismiss the overlay.
    LaunchedEffect(state.stage) {
        if (state.stage == CallManager.Stage.Idle) onFinished()
    }

    Surface(
        modifier = modifier.fillMaxSize(),
        color = MaterialTheme.colorScheme.scrim.copy(alpha = 0.96f),
    ) {
        Box(Modifier.fillMaxSize()) {
            if (state.video && (state.stage == CallManager.Stage.Active ||
                    state.stage == CallManager.Stage.Connecting)
            ) {
                VideoStage()
            }

            Column(
                modifier = Modifier
                    .fillMaxSize()
                    .padding(24.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.SpaceBetween,
            ) {
                CallHeader(state)
                Spacer(Modifier.height(8.dp))
                CallControls(state)
            }
        }
    }
}

@Composable
private fun CallHeader(state: CallManager.State) {
    val title = shortNpub(state.peer)
    val statusLine = when (state.stage) {
        CallManager.Stage.Incoming ->
            if (state.video) "Incoming video call" else "Incoming voice call"
        CallManager.Stage.Outgoing -> "Calling…"
        CallManager.Stage.Connecting -> "Connecting…"
        CallManager.Stage.Active -> "In call"
        CallManager.Stage.Ended -> "Call ended" + (state.endReason?.let { " · $it" } ?: "")
        CallManager.Stage.Idle -> ""
    }
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(14.dp),
        modifier = Modifier.padding(top = 40.dp),
    ) {
        // Hide the avatar behind a live video feed to keep the frame clean.
        if (!(state.video && state.stage == CallManager.Stage.Active)) {
            PeerAvatar(title = title, seed = state.peer, size = 108.dp)
        }
        Text(
            title,
            style = MaterialTheme.typography.headlineSmall,
            color = Color.White,
            textAlign = TextAlign.Center,
        )
        Text(
            statusLine,
            style = MaterialTheme.typography.bodyLarge,
            color = Color.White.copy(alpha = 0.75f),
            textAlign = TextAlign.Center,
        )
    }
}

@Composable
private fun CallControls(state: CallManager.State) {
    when (state.stage) {
        CallManager.Stage.Incoming -> Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceEvenly,
        ) {
            RoundCallButton(
                icon = CallIcon,
                label = "Decline",
                container = MaterialTheme.colorScheme.error,
                onClick = { CallManager.reject() },
                testTag = "call-reject",
            )
            RoundCallButton(
                icon = CallIcon,
                label = "Accept",
                container = Color(0xFF10B981),
                onClick = { CallManager.accept(it) },
                testTag = "call-accept",
            )
        }

        CallManager.Stage.Outgoing, CallManager.Stage.Connecting -> Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.Center,
        ) {
            RoundCallButton(
                icon = CallIcon,
                label = "Cancel",
                container = MaterialTheme.colorScheme.error,
                onClick = { CallManager.hangup() },
                testTag = "call-cancel",
            )
        }

        CallManager.Stage.Active -> Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceEvenly,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            ToggleCallButton(
                icon = MicIcon,
                label = if (state.muted) "Unmute" else "Mute",
                active = state.muted,
                onClick = { CallManager.toggleMute() },
                testTag = "call-mute",
            )
            if (state.video) {
                ToggleCallButton(
                    icon = CameraIcon,
                    label = if (state.cameraOn) "Camera" else "Camera off",
                    active = !state.cameraOn,
                    onClick = { CallManager.toggleCamera() },
                    testTag = "call-camera",
                )
            }
            ToggleCallButton(
                icon = SpeakerIcon,
                label = "Speaker",
                active = state.speakerOn,
                onClick = { CallManager.toggleSpeaker() },
                testTag = "call-speaker",
            )
            RoundCallButton(
                icon = CallIcon,
                label = "End",
                container = MaterialTheme.colorScheme.error,
                onClick = { CallManager.hangup() },
                testTag = "call-hangup",
            )
        }

        else -> Spacer(Modifier.height(72.dp))
    }
}

/** The remote video (full-bleed) with the local preview as picture-in-picture. */
@Composable
private fun VideoStage() {
    val context = LocalContext.current
    Box(Modifier.fillMaxSize()) {
        // Remote — fills the screen.
        AndroidView(
            modifier = Modifier.fillMaxSize(),
            factory = { ctx ->
                SurfaceViewRenderer(ctx).apply {
                    layoutParams = ViewGroup.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT,
                        ViewGroup.LayoutParams.MATCH_PARENT,
                    )
                    CallManager.eglBaseContext?.let { init(it, null) }
                    setEnableHardwareScaler(true)
                    setScalingType(RendererCommon.ScalingType.SCALE_ASPECT_FILL)
                    CallManager.setRemoteRenderer(this)
                }
            },
            onRelease = { renderer ->
                CallManager.setRemoteRenderer(null)
                renderer.release()
            },
        )
        // Local — small preview, top-end, mirrored.
        Box(
            modifier = Modifier
                .align(Alignment.TopEnd)
                .padding(16.dp)
                .width(108.dp)
                .aspectRatio(3f / 4f)
                .clip(RoundedCornerShape(12.dp)),
        ) {
            AndroidView(
                modifier = Modifier.fillMaxSize(),
                factory = { ctx ->
                    SurfaceViewRenderer(ctx).apply {
                        CallManager.eglBaseContext?.let { init(it, null) }
                        setEnableHardwareScaler(true)
                        setScalingType(RendererCommon.ScalingType.SCALE_ASPECT_FILL)
                        setMirror(true)
                        setZOrderMediaOverlay(true)
                        CallManager.setLocalRenderer(this)
                    }
                },
                onRelease = { renderer ->
                    CallManager.setLocalRenderer(null)
                    renderer.release()
                },
            )
        }
    }
}

// ── Buttons ────────────────────────────────────────────────────────────────

@Composable
private fun RoundCallButton(
    icon: ImageVector,
    label: String,
    container: Color,
    onClick: (android.content.Context) -> Unit,
    testTag: String,
) {
    val context = LocalContext.current
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Box(
            modifier = Modifier
                .size(68.dp)
                .clip(CircleShape)
                .background(container)
                .clickable { onClick(context) }
                .testTag(testTag),
            contentAlignment = Alignment.Center,
        ) {
            Icon(icon, contentDescription = label, tint = Color.White)
        }
        Text(label, style = MaterialTheme.typography.labelMedium, color = Color.White)
    }
}

@Composable
private fun ToggleCallButton(
    icon: ImageVector,
    label: String,
    active: Boolean,
    onClick: () -> Unit,
    testTag: String,
) {
    val container = if (active) Color.White else Color.White.copy(alpha = 0.16f)
    val tint = if (active) Color.Black else Color.White
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Box(
            modifier = Modifier
                .size(60.dp)
                .clip(CircleShape)
                .background(container)
                .clickable { onClick() }
                .testTag(testTag),
            contentAlignment = Alignment.Center,
        ) {
            Icon(icon, contentDescription = label, tint = tint)
        }
        Text(label, style = MaterialTheme.typography.labelSmall, color = Color.White)
    }
}
