package mullu.comrade.call

import android.Manifest
import android.app.Activity
import android.media.projection.MediaProjectionManager
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.PowerManager
import android.util.Log
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.snap
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import kotlinx.coroutines.delay
import mullu.comrade.R
import mullu.comrade.ui.CallEndIcon
import mullu.comrade.ui.ScreenShareIcon
import mullu.comrade.ui.CallIcon
import mullu.comrade.ui.ChatBubbleIcon
import mullu.comrade.ui.FlipCameraIcon
import mullu.comrade.ui.MicIcon
import mullu.comrade.ui.MoreVertIcon
import mullu.comrade.ui.PeerAvatar
import mullu.comrade.ui.SpeakerIcon
import mullu.comrade.ui.VideocamIcon
import mullu.comrade.ui.theme.CallPalette
import mullu.comrade.ui.theme.comradeFocusRing
import org.webrtc.RendererCommon
import org.webrtc.SurfaceViewRenderer
import kotlin.math.roundToInt
import org.webrtc.VideoTrack

/*
 * The full-screen call UI. It observes [CallManager.state] and renders the four
 * call phases — Ringing, Connecting, Active, Ended — plus the local/remote
 * WebRTC video via [SurfaceViewRenderer]. Accept is routed through [onAccept]
 * (the host gates the mic/camera runtime permission there); every other control
 * talks to [CallManager] directly.
 *
 * UX rules, shared with the Flutter port (`app/lib/src/screens/call_screen.dart`):
 *  * a video call opens full screen and the controls fade out after a few
 *    seconds; a tap anywhere brings them back (FaceTime). Audio calls keep
 *    their controls — there is nothing underneath to reveal;
 *  * four signal-strength bars (Telegram-style, [ConnectionStrengthIndicator])
 *    sit where the 4-emoji verification row used to. The SAS was near-redundant
 *    here: the SDP rides the NIP-44 gift-wrapped DM channel, so the
 *    fingerprints are already authenticated by the peer's Nostr key
 *    (`comrade_core::call::derive_sas` still exists and is still tested);
 *  * mute/camera draw an animated slash over the glyph ([SlashedIcon]) rather
 *    than swapping to a different icon;
 *  * a side that stopped sending shows "Video paused" over the avatar — never
 *    a frozen frame ([CallManager.remoteVideoPaused], camera-off, suspension);
 *  * the chat button opens the conversation and shrinks the call into a corner
 *    tile in our own window ([MinimizedCallTile]) — only one window can show
 *    the call *and* the thread, and it has to be ours;
 *  * the bar holds camera, mic, output, ⋮ and End, in that order, and
 *    everything else lives behind the ⋮ ([layoutCallControls] in
 *    CallControls.kt). It used to hold every control, which on a video call was
 *    six buttons on a screen that fits five — and a `Row` does not wrap, it
 *    just runs off the right edge, taking End call with it.
 */

// Full-bleed dark on every theme, deliberately outside ComradeTheme's
// ColorScheme — docs/DESIGN_SYSTEM.md §5, mullu.comrade.ui.theme.CallPalette.
private val CallBackground = CallPalette.background
private val AcceptGreen = CallPalette.accept
private val HangupRed = CallPalette.hangup
private val ControlIdle = CallPalette.controlIdle
private val ControlActive = CallPalette.controlActive

/**
 * How long a video call's controls stay up before fading — long enough to hit
 * "mute" without hunting, short enough that the call is mostly the other
 * person's face. Matches `_chromeLinger` in the Flutter port.
 */
private const val CHROME_LINGER_MS = 4_000L

/**
 * Diameter of a control in the bar. Five of these in equal slots fit the ~336dp
 * a 360dp-wide phone leaves after padding — see [MAX_PRIMARY_CALL_CONTROLS].
 */
private val BAR_BUTTON_SIZE = 56.dp

/**
 * Host entry point: shows the call overlay when a call is in flight, nothing
 * when [CallUiState.Idle]. Callers place this last in their layout stack so it
 * covers the app while ringing/connected.
 */
@Composable
fun CallScreen(
    onAccept: () -> Unit,
    modifier: Modifier = Modifier,
    onOpenChat: (peer: String, label: String) -> Unit = { _, _ -> },
) {
    val state by CallManager.state.collectAsState()
    val inPip by PipController.inPip.collectAsState()
    val minimized by PipController.minimized.collectAsState()
    // A call that ends while minimised must not leave the tile behind, and the
    // next call must start full screen.
    LaunchedEffect(state) {
        if (state !is CallUiState.Active && state !is CallUiState.Connecting) {
            PipController.restoreFromMinimized()
        }
    }
    when (val s = state) {
        is CallUiState.Idle -> Unit
        is CallUiState.Ringing -> CallOverlay(modifier) { RingingContent(s, onAccept) }
        // One branch — one composition subtree — for both in-call phases: if
        // Connecting and Active were separate branches, the transition between
        // them would dispose and recreate the whole video subtree (each
        // SurfaceViewRenderer re-inits and the remote sink detaches) right as
        // the first video frames arrive.
        is CallUiState.Connecting, is CallUiState.Active -> {
            val active = s as? CallUiState.Active
            val connecting = s as? CallUiState.Connecting
            val peer = active?.peer ?: connecting!!.peer
            val peerLabel = active?.peerLabel ?: connecting!!.peerLabel
            val video = active?.video ?: connecting!!.video
            when {
                // Shrunk into our own window by the chat button: deliberately
                // NOT wrapped in CallOverlay, whose full-screen opaque
                // background would hide the conversation this mode exists to
                // show — and swallow every tap meant for it.
                minimized -> MinimizedCallTile(
                    peer = peer,
                    peerLabel = peerLabel,
                    video = video,
                    modifier = modifier,
                )
                // The OS is drawing us into a thumbnail: the picture is all
                // that fits — controls belong to the PiP window's own chrome,
                // and tapping it is the platform's restore gesture.
                inPip -> CallOverlay(modifier) {
                    PipVideoContent(video = video, peer = peer, peerLabel = peerLabel)
                }
                else -> CallOverlay(modifier) {
                    InCallContent(
                        peer = peer,
                        peerLabel = peerLabel,
                        video = video,
                        status = if (active == null) stringOf(R.string.call_connecting) else null,
                        connectedAtMs = active?.connectedAtMs ?: 0L,
                        onOpenChat = onOpenChat,
                    )
                }
            }
        }
        is CallUiState.Ended -> CallOverlay(modifier) { EndedContent(s) }
    }
}

/**
 * What the picture-in-picture window shows: the remote picture, with the "no
 * picture" state drawn over it.
 */
@Composable
private fun PipVideoContent(video: Boolean, peer: String, peerLabel: String) {
    val remoteVideo by CallManager.remoteVideo.collectAsState()
    val remotePaused by CallManager.remoteVideoPaused.collectAsState()
    CallVideoSurface(
        track = if (video) remoteVideo else null,
        mirror = false,
        paused = video && remotePaused,
        peer = peer,
        peerLabel = peerLabel,
        avatarSize = 56.dp,
        captionSize = 11.sp,
        modifier = Modifier.fillMaxSize(),
    )
}

/**
 * One video surface: the live picture, with the "no picture" state drawn *over*
 * it rather than in place of it.
 *
 * The distinction is the whole point. A `SurfaceViewRenderer` that leaves the
 * composition is released, and releasing it removes its sink from the track —
 * so the frames arriving when the peer un-pauses have nowhere to land until a
 * fresh renderer has been built and re-attached. That is how "they paused and
 * their video never came back" happened. Keeping the renderer composed and
 * covering it means resuming is instant: the cover just lifts.
 */
@Composable
private fun CallVideoSurface(
    track: VideoTrack?,
    mirror: Boolean,
    paused: Boolean,
    peer: String,
    peerLabel: String,
    modifier: Modifier = Modifier,
    avatarSize: Dp = 96.dp,
    captionSize: androidx.compose.ui.unit.TextUnit = 16.sp,
    zOrderMediaOverlay: Boolean = false,
) {
    Box(modifier) {
        if (track != null) {
            VideoRenderer(
                track = track,
                mirror = mirror,
                modifier = Modifier.fillMaxSize(),
                zOrderMediaOverlay = zOrderMediaOverlay,
            )
        }
        // Opaque, so a frozen last frame is never left showing underneath.
        if (track == null || paused) {
            Column(
                modifier = Modifier
                    .fillMaxSize()
                    .background(CallBackground),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.Center,
            ) {
                PeerAvatar(peerLabel, seed = peer, size = avatarSize)
                if (paused) {
                    Spacer(Modifier.height(6.dp))
                    Text(
                        stringOf(R.string.call_video_paused),
                        color = Color.White,
                        fontSize = captionSize,
                    )
                }
            }
        }
    }
}

/**
 * A callback that starts or stops sharing the screen.
 *
 * Starting needs the system's own capture-consent dialog, which only an
 * Activity result can deliver — hence the launcher rather than a direct
 * [CallManager] call. Dismissing that dialog returns `RESULT_CANCELED` and
 * nothing happens, which is why the row's lit state follows
 * [CallManager.screenSharing] and never the tap.
 *
 * Returned as a lambda rather than drawn as a button because the control moved
 * into the ⋮ dock, and `rememberLauncherForActivityResult` has to be created by
 * a composable that stays composed — a launcher created inside the dock would
 * be disposed the moment the dock closed, which for the *stop* direction is the
 * very next frame.
 */
@Composable
private fun rememberScreenShareToggle(sharing: Boolean): () -> Unit {
    val context = LocalContext.current
    val launcher = rememberLauncherForActivityResult(
        ActivityResultContracts.StartActivityForResult(),
    ) { result ->
        val data = result.data
        if (result.resultCode == Activity.RESULT_OK && data != null) {
            CallManager.startScreenShare(context, data)
        }
    }
    return {
        if (sharing) {
            CallManager.stopScreenShare()
        } else {
            val manager = context.getSystemService(MediaProjectionManager::class.java)
            if (manager != null) launcher.launch(manager.createScreenCaptureIntent())
        }
    }
}

/**
 * The panel the ⋮ opens: everything the bar deliberately does not hold.
 *
 * A panel anchored above the bar rather than a `ModalBottomSheet`, so it
 * matches the Flutter and desktop docks exactly and adds no second dismissable
 * surface on top of a running call.
 */
@Composable
private fun CallOptionsDock(
    items: List<CallControl>,
    sharing: Boolean,
    onScreenShare: () -> Unit,
    onSwitchCamera: () -> Unit,
    onChat: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(
        modifier = modifier
            .widthIn(min = 216.dp)
            .clip(RoundedCornerShape(16.dp))
            .background(CallPalette.dockBackground)
            .padding(vertical = 6.dp),
    ) {
        items.forEach { item ->
            when (item) {
                // Screen share is offered on **voice calls too** — that is the
                // point of it, and it is why the stage can appear on a call
                // that started without video.
                CallControl.SCREEN_SHARE -> DockItem(
                    icon = ScreenShareIcon,
                    label = stringOf(
                        if (sharing) R.string.call_screen_share_stop else R.string.call_screen_share,
                    ),
                    active = sharing,
                    onClick = onScreenShare,
                )
                CallControl.SWITCH_CAMERA -> DockItem(
                    icon = FlipCameraIcon,
                    label = stringOf(R.string.call_switch_camera),
                    onClick = onSwitchCamera,
                )
                CallControl.CHAT -> DockItem(
                    icon = ChatBubbleIcon,
                    label = stringOf(R.string.call_chat),
                    onClick = onChat,
                )
                // The bar's own controls never appear here.
                else -> Unit
            }
        }
    }
}

@Composable
private fun DockItem(
    icon: ImageVector,
    label: String,
    onClick: () -> Unit,
    active: Boolean = false,
) {
    val tint = if (active) ControlActive else Color.White
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .comradeFocusRing()
            .clickable(onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Icon(icon, contentDescription = null, tint = tint, modifier = Modifier.size(22.dp))
        Text(label, color = tint, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/**
 * The call shrunk into a corner of our own window — what the chat button
 * produces. Tap to restore.
 *
 * Top-right by default, and that corner matters: the bottom right of a
 * conversation is the send button, and a call tile parked on it would defeat the
 * one thing this mode exists for. The root [Box] draws nothing and takes no
 * pointer input of its own, so every tap outside the tile reaches the
 * conversation underneath.
 */
@Composable
private fun MinimizedCallTile(
    peer: String,
    peerLabel: String,
    video: Boolean,
    modifier: Modifier = Modifier,
) {
    val remoteVideo by CallManager.remoteVideo.collectAsState()
    val remotePaused by CallManager.remoteVideoPaused.collectAsState()
    val muted by CallManager.muted.collectAsState()
    val quality by CallManager.connectionQuality.collectAsState()

    val density = LocalDensity.current
    val tileWidth = 132.dp
    val tileHeight = 186.dp
    val margin = 12.dp

    BoxWithConstraints(modifier.fillMaxSize()) {
        // Telegram's rules, in pixels because that is what `offset` speaks:
        // the tile goes wherever it is dragged, never leaves the screen, and
        // on release flies to whichever side edge its centre is nearer.
        val maxX = with(density) { (maxWidth - tileWidth - margin).toPx() }
        val maxY = with(density) { (maxHeight - tileHeight - margin).toPx() }
        val minPx = with(density) { margin.toPx() }
        val halfTile = with(density) { (tileWidth / 2).toPx() }
        val centreOfScreen = with(density) { (maxWidth / 2).toPx() }

        // Starts top-right: the bottom right of a conversation is the send
        // button, and a tile parked there defeats the mode's whole purpose.
        var offsetX by remember(maxX) { mutableFloatStateOf(maxX.coerceAtLeast(minPx)) }
        var offsetY by remember { mutableFloatStateOf(minPx) }
        var dragging by remember { mutableStateOf(false) }
        // Instant while the finger is down, a short glide when it lets go.
        val animatedX by animateFloatAsState(
            targetValue = offsetX,
            animationSpec = if (dragging) snap() else tween(180),
            label = "tile-x",
        )
        val animatedY by animateFloatAsState(
            targetValue = offsetY,
            animationSpec = if (dragging) snap() else tween(180),
            label = "tile-y",
        )

        Box(
            modifier = Modifier
                .offset { IntOffset(animatedX.roundToInt(), animatedY.roundToInt()) }
                .size(width = tileWidth, height = tileHeight)
                .clip(RoundedCornerShape(14.dp))
                .background(CallPalette.tileBackground)
                .pointerInput(maxX, maxY) {
                    detectDragGestures(
                        onDragStart = { dragging = true },
                        onDragEnd = {
                            dragging = false
                            offsetX = if (offsetX + halfTile < centreOfScreen) {
                                minPx
                            } else {
                                maxX.coerceAtLeast(minPx)
                            }
                        },
                        onDragCancel = { dragging = false },
                    ) { change, drag ->
                        change.consume()
                        offsetX = (offsetX + drag.x).coerceIn(minPx, maxX.coerceAtLeast(minPx))
                        offsetY = (offsetY + drag.y).coerceIn(minPx, maxY.coerceAtLeast(minPx))
                    }
                }
                .clickable(onClickLabel = stringOf(R.string.call_restore)) {
                    PipController.restoreFromMinimized()
                },
        ) {
            CallVideoSurface(
                track = if (video) remoteVideo else null,
                mirror = false,
                paused = video && remotePaused,
                peer = peer,
                peerLabel = peerLabel,
                avatarSize = 48.dp,
                captionSize = 11.sp,
                modifier = Modifier.fillMaxSize(),
                // Over the conversation's own surfaces.
                zOrderMediaOverlay = true,
            )
            SignalStrengthBars(
                quality,
                modifier = Modifier
                    .align(Alignment.TopStart)
                    .padding(6.dp),
                height = 10.dp,
            )
            Row(
                modifier = Modifier
                    .align(Alignment.BottomCenter)
                    .padding(bottom = 6.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                CallActionButton(
                    icon = MicIcon,
                    desc = stringOf(if (muted) R.string.call_unmute else R.string.call_mute),
                    bg = if (muted) ControlActive else CallPalette.scrim,
                    tint = if (muted) CallBackground else Color.White,
                    size = 34.dp,
                    slashed = muted,
                ) { CallManager.toggleMute() }
                CallActionButton(
                    icon = CallEndIcon,
                    desc = stringOf(R.string.call_hang_up),
                    bg = HangupRed,
                    size = 34.dp,
                ) { CallManager.hangup() }
            }
        }
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
    onOpenChat: (peer: String, label: String) -> Unit = { _, _ -> },
) {
    val muted by CallManager.muted.collectAsState()
    val cameraOn by CallManager.cameraOn.collectAsState()
    val audioRoute by CallManager.audioRoute.collectAsState()
    val availableRoutes by CallManager.availableRoutes.collectAsState()
    val remoteVideo by CallManager.remoteVideo.collectAsState()
    val localVideo by CallManager.localVideo.collectAsState()
    val connectionQuality by CallManager.connectionQuality.collectAsState()
    val screenSharing by CallManager.screenSharing.collectAsState()
    val videoSuspended by CallManager.videoSuspended.collectAsState()
    val remoteVideoPaused by CallManager.remoteVideoPaused.collectAsState()

    // Audio calls: let the proximity sensor blank the screen when held to the ear.
    ProximityScreenControl(active = !video && status == null)

    val label = if (status != null) status else durationLabel(connectedAtMs)
    // Quality stats are only meaningful once the call is actually Active —
    // status == null is this composable's existing "are we Active" signal
    // (see ProximityScreenControl above). The bars render UNKNOWN as four
    // empty bars and GOOD without a text label, so showing the indicator for
    // every Active reading adds no noise to the healthy case.
    val showIndicator = status == null

    // Our own camera is not being sent, for whichever of the two reasons —
    // the user turned it off, or capture is suspended because nothing was
    // displaying it. From the peer's side they are the same thing.
    val localVideoPaused = !cameraOn || videoSuspended

    // Which controls are in the bar and which are behind the ⋮ (CallControls.kt).
    val layout = layoutCallControls(video = video, cameraOn = cameraOn)
    var dockOpen by remember { mutableStateOf(false) }
    val toggleScreenShare = rememberScreenShareToggle(screenSharing)

    // FaceTime-style chrome: on a video call the controls and the name pill
    // linger for a few seconds, then fade; a tap anywhere toggles them back.
    // Audio calls keep their controls permanently.
    var chromeVisible by remember { mutableStateOf(true) }
    LaunchedEffect(video, status, chromeVisible, dockOpen) {
        // An open dock is someone mid-decision — fading the controls out from
        // under them would be the rudest possible moment for it.
        if (video && status == null && chromeVisible && !dockOpen) {
            delay(CHROME_LINGER_MS)
            chromeVisible = false
        }
    }
    // Chrome that went away for any reason takes the dock with it, so it never
    // reappears already open on the next tap.
    LaunchedEffect(chromeVisible) {
        if (!chromeVisible) dockOpen = false
    }

    Box(Modifier.fillMaxSize()) {
        if (video) {
            // Tap the picture-in-picture tile to swap it with the full-screen
            // view (Telegram-style). `swapped` only changes which track renders
            // where — the underlying tracks/renderers are unaffected.
            var swapped by remember { mutableStateOf(false) }
            val mainTrack = if (swapped) localVideo else remoteVideo
            val pipTrack = if (swapped) remoteVideo else localVideo
            val pipIsLocal = !swapped

            // Whichever track each surface shows, "paused" means that side
            // stopped *sending* — not a renderer that has no frames yet.
            val mainPaused = if (swapped) localVideoPaused else remoteVideoPaused
            val tilePaused = if (pipIsLocal) localVideoPaused else remoteVideoPaused

            // The local track is the front camera preview, so it mirrors
            // wherever it renders; the remote track never mirrors. The paused
            // state is drawn *over* the renderer, never in place of it — see
            // CallVideoSurface for why that difference is load-bearing.
            CallVideoSurface(
                track = mainTrack,
                mirror = swapped,
                paused = mainPaused,
                peer = peer,
                peerLabel = peerLabel,
                modifier = Modifier.fillMaxSize(),
            )

            // The tap target for revealing/dismissing the controls. Sits above
            // the video but below the tile and the controls, so those keep
            // their own taps; `indication = null` because a full-screen ripple
            // on every chrome toggle would be absurd.
            Box(
                Modifier
                    .fillMaxSize()
                    .clickable(
                        interactionSource = remember { MutableInteractionSource() },
                        indication = null,
                    ) {
                        // An open dock is what the first tap dismisses; only
                        // once it is shut does a tap toggle the whole chrome.
                        if (dockOpen) dockOpen = false else chromeVisible = !chromeVisible
                    },
            )

            // Self-preview tile: swaps on tap; hosts the camera-flip control so
            // the bottom bar keeps exactly one camera button. Deliberately NOT
            // part of the fading chrome — it is the only feedback that your
            // own camera works.
            Box(
                modifier = Modifier
                    .align(Alignment.TopEnd)
                    .padding(16.dp)
                    .size(width = 110.dp, height = 156.dp)
                    .clip(RoundedCornerShape(14.dp))
                    .background(CallPalette.tileBackground)
                    .clickable(onClickLabel = stringOf(R.string.call_swap_video)) { swapped = !swapped },
            ) {
                CallVideoSurface(
                    track = pipTrack,
                    mirror = pipIsLocal,
                    paused = tilePaused,
                    peer = if (pipIsLocal) "" else peer,
                    peerLabel = if (pipIsLocal) stringOf(R.string.call_you) else peerLabel,
                    avatarSize = 44.dp,
                    captionSize = 11.sp,
                    modifier = Modifier.fillMaxSize(),
                    // Two overlapping SurfaceViews have no defined z-order
                    // between their surfaces unless the small one is explicitly
                    // marked as a media overlay — without this the tile can
                    // composite under the full-screen surface and never show.
                    zOrderMediaOverlay = true,
                )
                if (pipIsLocal && !localVideoPaused) {
                    CallActionButton(
                        icon = FlipCameraIcon,
                        desc = stringOf(R.string.call_switch_camera),
                        bg = CallPalette.scrim,
                        size = 34.dp,
                        modifier = Modifier
                            .align(Alignment.BottomCenter)
                            .padding(bottom = 6.dp),
                    ) { CallManager.switchCamera() }
                }
            }

            // Name + duration/status pill and the signal bars, kept clear of
            // the self-preview tile. Fades with the rest of the chrome.
            androidx.compose.animation.AnimatedVisibility(
                visible = chromeVisible,
                enter = fadeIn(),
                exit = fadeOut(),
                modifier = Modifier
                    .align(Alignment.TopStart)
                    .padding(start = 16.dp, top = 20.dp, end = 142.dp),
            ) {
                Column {
                    Column(
                        modifier = Modifier
                            .clip(RoundedCornerShape(14.dp))
                            .background(CallPalette.scrim)
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
                            color = CallPalette.secondaryText,
                            fontSize = 13.sp,
                            fontFamily = FontFamily.Monospace,
                        )
                    }
                    if (showIndicator) {
                        Spacer(Modifier.height(6.dp))
                        ConnectionStrengthIndicator(
                            connectionQuality,
                            modifier = Modifier
                                .clip(RoundedCornerShape(14.dp))
                                .background(CallPalette.scrim)
                                .padding(horizontal = 10.dp, vertical = 6.dp),
                        )
                    }
                }
            }
        } else {
            PeerHeader(peer, peerLabel, label, Modifier.align(Alignment.Center)) {
                if (showIndicator) {
                    Spacer(Modifier.height(10.dp))
                    ConnectionStrengthIndicator(connectionQuality)
                }
            }
        }

        // The bar — camera, mic, output, ⋮, End — with the dock stacked above
        // it when open. Over video the pair sit on a scrim so they stay legible
        // on top of bright frames, and they fade with the chrome.
        //
        // Which controls are in the bar and which are behind the ⋮ is
        // `layoutCallControls` (CallControls.kt), not a list written out here:
        // the bar used to be every control in a Row, six of them on a video
        // call, and the sixth — End call — ran off the right edge of a phone.
        androidx.compose.animation.AnimatedVisibility(
            visible = chromeVisible || !video,
            enter = fadeIn(),
            exit = fadeOut(),
            modifier = Modifier.align(Alignment.BottomCenter),
        ) {
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .then(
                        if (video) {
                            Modifier.background(
                                Brush.verticalGradient(listOf(Color.Transparent, CallPalette.captionOverlay)),
                            )
                        } else {
                            Modifier
                        },
                    ),
                horizontalAlignment = Alignment.End,
            ) {
                if (dockOpen) {
                    CallOptionsDock(
                        items = layout.dock,
                        sharing = screenSharing,
                        onScreenShare = {
                            dockOpen = false
                            toggleScreenShare()
                        },
                        onSwitchCamera = {
                            dockOpen = false
                            CallManager.switchCamera()
                        },
                        onChat = {
                            dockOpen = false
                            onOpenChat(peer, peerLabel)
                        },
                        modifier = Modifier.padding(end = 20.dp, top = 20.dp, bottom = 14.dp),
                    )
                }
                // Equal-width slots rather than a centred row with fixed gaps,
                // so the bar is exactly as wide as the screen and a control
                // physically cannot be pushed off the end of it.
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 12.dp)
                        .padding(top = if (dockOpen) 0.dp else 28.dp, bottom = 36.dp),
                    verticalAlignment = Alignment.Top,
                ) {
                    layout.primary.forEach { control ->
                        Box(Modifier.weight(1f), contentAlignment = Alignment.TopCenter) {
                            when (control) {
                                CallControl.CAMERA -> CallActionButton(
                                    icon = VideocamIcon,
                                    desc = stringOf(
                                        if (cameraOn) R.string.call_camera_off else R.string.call_camera_on,
                                    ),
                                    bg = if (!cameraOn) ControlActive else ControlIdle,
                                    tint = if (!cameraOn) CallBackground else Color.White,
                                    size = BAR_BUTTON_SIZE,
                                    label = stringOf(R.string.call_camera),
                                    slashed = !cameraOn,
                                ) { CallManager.toggleCamera() }
                                // The mic slashes itself rather than swapping
                                // glyph — a struck-through mic is "your mic is
                                // off"; a different picture is just "the icon
                                // changed".
                                CallControl.MIC -> CallActionButton(
                                    icon = MicIcon,
                                    desc = stringOf(if (muted) R.string.call_unmute else R.string.call_mute),
                                    bg = if (muted) ControlActive else ControlIdle,
                                    tint = if (muted) CallBackground else Color.White,
                                    size = BAR_BUTTON_SIZE,
                                    label = stringOf(if (muted) R.string.call_unmute else R.string.call_mute),
                                    slashed = muted,
                                ) { CallManager.toggleMute() }
                                CallControl.SPEAKER -> AudioRouteButton(audioRoute, availableRoutes)
                                // Lit while something behind it is running, so
                                // "you are sharing your screen" is legible with
                                // the dock shut.
                                CallControl.MORE -> CallActionButton(
                                    icon = MoreVertIcon,
                                    desc = stringOf(R.string.call_more_options),
                                    bg = if (screenSharing) ControlActive else ControlIdle,
                                    tint = if (screenSharing) CallBackground else Color.White,
                                    size = BAR_BUTTON_SIZE,
                                    label = stringOf(R.string.call_more),
                                ) { dockOpen = !dockOpen }
                                CallControl.HANGUP -> CallActionButton(
                                    icon = CallEndIcon,
                                    desc = stringOf(R.string.call_hang_up),
                                    bg = HangupRed,
                                    size = BAR_BUTTON_SIZE,
                                    label = stringOf(R.string.call_end_label),
                                ) { CallManager.hangup() }
                                // Dock-only; layoutCallControls never puts
                                // these in the bar.
                                else -> Unit
                            }
                        }
                    }
                }
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
        Text(stringOf(labelRes), color = CallPalette.secondaryText, fontSize = 15.sp)
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
            Text(status, color = CallPalette.secondaryText, fontSize = 16.sp, textAlign = TextAlign.Center)
        }
        extra()
    }
}

// The connection-quality indicator (four Telegram-style signal bars) and the
// animated slashed icons live in CallWidgets.kt, shared with nothing UI-toolkit
// specific in their contracts so the Flutter port mirrors them exactly.

@Composable
private fun CallActionButton(
    icon: ImageVector,
    desc: String,
    bg: Color,
    tint: Color = Color.White,
    size: Dp = 64.dp,
    label: String? = null,
    slashed: Boolean = false,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    Column(modifier = modifier, horizontalAlignment = Alignment.CenterHorizontally) {
        Box(
            modifier = Modifier
                .size(size)
                .clip(CircleShape)
                .background(bg)
                .comradeFocusRing(CircleShape)
                .clickable(onClick = onClick),
            contentAlignment = Alignment.Center,
        ) {
            // The slash is drawn over the glyph (and animated) rather than the
            // glyph swapped — see [SlashedIcon].
            SlashedIcon(
                icon = icon,
                slashed = slashed,
                color = tint,
                cutoutColor = bg,
                contentDescription = desc,
                size = size * 0.44f,
            )
        }
        if (label != null) {
            Spacer(Modifier.height(6.dp))
            // Ellipsised, not clipped: the bar gives each control an equal
            // slice of the screen, and "Wired headset" is wider than a fifth
            // of a phone.
            Text(
                label,
                color = CallPalette.labelDim,
                fontSize = 12.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                textAlign = TextAlign.Center,
            )
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
            size = BAR_BUTTON_SIZE,
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
    // The frame's own dimensions, reported by WebRTC as they change (the peer
    // rotating, or starting a screen share). Drives the fill-vs-letterbox
    // decision below; 0×0 until the first frame, which reads as "fill".
    var frameWidth by remember { mutableIntStateOf(0) }
    var frameHeight by remember { mutableIntStateOf(0) }
    val renderer = remember {
        SurfaceViewRenderer(context).apply {
            init(
                egl,
                object : RendererCommon.RendererEvents {
                    override fun onFirstFrameRendered() = Unit

                    // Fired on a WebRTC thread; Compose state writes are safe
                    // from any thread, and the recomposition it schedules lands
                    // on the main one.
                    override fun onFrameResolutionChanged(width: Int, height: Int, rotation: Int) {
                        // Report the frame the way it will be *drawn*: at 90°
                        // or 270° the sides swap, and comparing an un-rotated
                        // frame against the box would letterbox exactly the
                        // portrait video that should fill.
                        val rotated = rotation == 90 || rotation == 270
                        frameWidth = if (rotated) height else width
                        frameHeight = if (rotated) width else height
                    }
                },
            )
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
    // Mirror and scaling are applied in update (not just at creation): mirror
    // flips whenever the user swaps which tile shows the front camera, and the
    // scaling type changes the moment a peer starts sharing a 16:9 screen onto
    // this portrait surface.
    AndroidView(
        factory = { renderer },
        update = { view ->
            view.setMirror(mirror)
            val letterbox = shouldLetterbox(
                frameWidth = frameWidth,
                frameHeight = frameHeight,
                boxWidth = view.width,
                boxHeight = view.height,
            )
            view.setScalingType(
                if (letterbox) {
                    RendererCommon.ScalingType.SCALE_ASPECT_FIT
                } else {
                    RendererCommon.ScalingType.SCALE_ASPECT_FILL
                },
            )
        },
        modifier = modifier,
    )
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
