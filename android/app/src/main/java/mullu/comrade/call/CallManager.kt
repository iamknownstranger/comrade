package mullu.comrade.call

import android.content.Context
import android.media.AudioManager
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import mullu.comrade.ComradeCore
import org.webrtc.AudioSource
import org.webrtc.AudioTrack
import org.webrtc.Camera2Enumerator
import org.webrtc.CameraEnumerator
import org.webrtc.DefaultVideoDecoderFactory
import org.webrtc.DefaultVideoEncoderFactory
import org.webrtc.EglBase
import org.webrtc.IceCandidate
import org.webrtc.MediaConstraints
import org.webrtc.MediaStream
import org.webrtc.PeerConnection
import org.webrtc.PeerConnectionFactory
import org.webrtc.RtpReceiver
import org.webrtc.SdpObserver
import org.webrtc.SessionDescription
import org.webrtc.SurfaceTextureHelper
import org.webrtc.SurfaceViewRenderer
import org.webrtc.VideoCapturer
import org.webrtc.VideoSource
import org.webrtc.VideoTrack
import uniffi.comrade_core.CallMediaKind
import uniffi.comrade_core.CallSignal
import uniffi.comrade_core.HangupReason
import uniffi.comrade_core.IceStrategy
import uniffi.comrade_ui.CallSignalDto

/**
 * The Android side of a WebRTC call: microphone/camera capture and an
 * `RTCPeerConnection`, negotiated against a peer entirely through the Rust
 * core's NIP-59 DM signaling ([`comrade_core::call`]).
 *
 * The Rust side is the *wire protocol* — it mints the call id, wraps each
 * [CallSignal] in an encrypted DM, and routes the peer's replies back as
 * `BridgeEvent.IncomingCallSignal`. This object is the *media plane* the Rust
 * side deliberately does not own: it turns those signals into `setRemote…` /
 * `addIceCandidate` calls on a real [PeerConnection], and turns the peer
 * connection's own offer/answer/ICE output back into [CallSignal]s to send.
 *
 * One call at a time (there is no group-call SFU — see the crate docs). The
 * public surface is a single [state] flow the UI observes plus the handful of
 * intents a call screen fires: [startOutgoing], [accept], [reject], [hangup],
 * [toggleMute], [toggleSpeaker], [toggleCamera].
 *
 * ## Threading
 * WebRTC's [PeerConnection.Observer] and [SdpObserver] callbacks arrive on
 * WebRTC's own signaling thread. Every FFI signal-send is a *blocking* call
 * ([ComradeCore] bridges uniffi's async exports with `runBlocking`), so it is
 * dispatched onto [scope] (IO) rather than run on the WebRTC thread. [state]
 * is a [MutableStateFlow], safe to publish to from any thread.
 */
object CallManager {

    private const val TAG = "CallManager"
    private const val STREAM_ID = "comrade-stream"
    private const val LOCAL_VIDEO_TRACK = "comrade-video"
    private const val LOCAL_AUDIO_TRACK = "comrade-audio"
    private const val VIDEO_WIDTH = 1280
    private const val VIDEO_HEIGHT = 720
    private const val VIDEO_FPS = 30

    /** Coarse call lifecycle the UI renders distinct screens for. */
    enum class Stage { Idle, Outgoing, Incoming, Connecting, Active, Ended }

    /** The one snapshot a call screen renders; published on every transition. */
    data class State(
        val stage: Stage = Stage.Idle,
        val peer: String = "",
        val video: Boolean = false,
        /** True when *we* are the callee (drives Accept/Reject vs Cancel UI). */
        val incoming: Boolean = false,
        val muted: Boolean = false,
        val speakerOn: Boolean = false,
        val cameraOn: Boolean = true,
        /** Present only in [Stage.Ended]: why the call finished, for the log line. */
        val endReason: String? = null,
    ) {
        val active: Boolean get() = stage != Stage.Idle
    }

    private val _state = MutableStateFlow(State())
    val state: StateFlow<State> = _state.asStateFlow()

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    // ── Process-wide WebRTC singletons (created once, never disposed) ─────────
    private var appContext: Context? = null
    private var eglBase: EglBase? = null
    private var factory: PeerConnectionFactory? = null
    private var initialised = false

    /** The shared EGL context the UI's [SurfaceViewRenderer]s must init with. */
    val eglBaseContext: EglBase.Context? get() = eglBase?.eglBaseContext

    // ── Per-call state (all reset by [teardown]) ─────────────────────────────
    private var pc: PeerConnection? = null
    private var audioSource: AudioSource? = null
    private var localAudioTrack: AudioTrack? = null
    private var videoSource: VideoSource? = null
    private var localVideoTrack: VideoTrack? = null
    private var videoCapturer: VideoCapturer? = null
    private var surfaceHelper: SurfaceTextureHelper? = null
    private var remoteVideoTrack: VideoTrack? = null

    private var callId: String = ""
    private var peer: String = ""
    private var mediaKind: CallMediaKind = CallMediaKind.AUDIO
    private var incoming: Boolean = false
    private var remoteDescriptionSet = false
    private var pendingOffer: SessionDescription? = null
    // ICE candidates that arrive before the remote description is applied must
    // be buffered — addIceCandidate before setRemoteDescription is dropped.
    private val pendingRemoteCandidates = mutableListOf<IceCandidate>()

    // Renderers are owned by the UI; we hold weak intent to attach tracks to
    // them as either side (track / renderer) becomes available.
    private var localRenderer: SurfaceViewRenderer? = null
    private var remoteRenderer: SurfaceViewRenderer? = null

    /** Call once (idempotent) with an application context before first use. */
    @Synchronized
    fun init(context: Context) {
        if (initialised) return
        val app = context.applicationContext
        appContext = app
        PeerConnectionFactory.initialize(
            PeerConnectionFactory.InitializationOptions.builder(app)
                .createInitializationOptions(),
        )
        val egl = EglBase.create()
        eglBase = egl
        factory = PeerConnectionFactory.builder()
            .setVideoEncoderFactory(DefaultVideoEncoderFactory(egl.eglBaseContext, true, true))
            .setVideoDecoderFactory(DefaultVideoDecoderFactory(egl.eglBaseContext))
            .createPeerConnectionFactory()
        initialised = true
    }

    // ── Intents from the UI ───────────────────────────────────────────────────

    /**
     * Place an outgoing call: ask the Rust core for a call id + ICE servers,
     * build the peer connection, capture local media, and send the SDP offer.
     */
    fun startOutgoing(context: Context, peerNpub: String, video: Boolean) {
        if (_state.value.active) return
        mediaKind = if (video) CallMediaKind.VIDEO else CallMediaKind.AUDIO
        peer = peerNpub
        incoming = false
        _state.value = State(stage = Stage.Outgoing, peer = peerNpub, video = video, incoming = false)
        scope.launch {
            init(context) // heavy factory/EGL setup — off the main thread
            val session = runCatching { ComradeCore.placeCallTyped(peerNpub, mediaKind) }
                .getOrElse {
                    Log.w(TAG, "placeCall failed", it)
                    endLocally("failed")
                    return@launch
                }
            callId = session.callId
            val iceServers = session.iceServers.map { it.toWebRtc() }
            if (!buildPeerConnection(iceServers, video)) {
                endLocally("failed")
                return@launch
            }
            createOffer()
        }
    }

    /**
     * Register a freshly-arrived incoming [CallSignal.Offer] and start ringing.
     * Fired by the event pump; the peer connection is only built once the user
     * [accept]s, so a declined call never opens the mic/camera.
     */
    fun onIncomingOffer(dto: CallSignalDto, sdp: String) {
        if (_state.value.active) {
            // Already busy — auto-reject so the caller isn't left ringing.
            scope.launch {
                runCatching {
                    ComradeCore.sendCallSignalTyped(
                        dto.peer, dto.callId, mediaFromStr(dto.media), CallSignal.Busy,
                    )
                }
            }
            return
        }
        peer = dto.peer
        callId = dto.callId
        mediaKind = mediaFromStr(dto.media)
        incoming = true
        pendingOffer = SessionDescription(SessionDescription.Type.OFFER, sdp)
        _state.value = State(
            stage = Stage.Incoming,
            peer = dto.peer,
            video = mediaKind == CallMediaKind.VIDEO,
            incoming = true,
        )
        // Let the caller know we're ringing (pre-answer).
        sendSignal(CallSignal.Ringing)
    }

    /** Accept the ringing call: build the connection, answer the stored offer. */
    fun accept(context: Context) {
        val offer = pendingOffer ?: return
        if (_state.value.stage != Stage.Incoming) return
        _state.update { it.copy(stage = Stage.Connecting) }
        scope.launch {
            init(context) // heavy factory/EGL setup — off the main thread
            val iceServers = runCatching {
                ComradeCore.callIceServersForTyped(IceStrategy.STUN_ONLY).map { it.toWebRtc() }
            }.getOrDefault(emptyList())
            val video = mediaKind == CallMediaKind.VIDEO
            if (!buildPeerConnection(iceServers, video)) {
                endLocally("failed")
                return@launch
            }
            val connection = pc ?: return@launch
            connection.setRemoteDescription(
                object : SimpleSdpObserver("setRemote(offer)") {
                    override fun onSetSuccess() {
                        remoteDescriptionSet = true
                        drainPendingCandidates()
                        createAnswer()
                    }
                },
                offer,
            )
            pendingOffer = null
        }
    }

    /** Reject a ringing incoming call. */
    fun reject() {
        if (_state.value.stage != Stage.Incoming) return
        sendHangup(HangupReason.DECLINED)
        endLocally("declined")
    }

    /** Hang up an outgoing/connecting/active call from this side. */
    fun hangup() {
        if (!_state.value.active) return
        val reason = when (_state.value.stage) {
            Stage.Outgoing -> HangupReason.CANCELLED
            else -> HangupReason.NORMAL
        }
        sendHangup(reason)
        endLocally(reason.name.lowercase())
    }

    fun toggleMute() {
        val track = localAudioTrack ?: return
        val muted = !_state.value.muted
        track.setEnabled(!muted)
        _state.update { it.copy(muted = muted) }
    }

    fun toggleCamera() {
        val track = localVideoTrack ?: return
        val on = !_state.value.cameraOn
        track.setEnabled(on)
        _state.update { it.copy(cameraOn = on) }
    }

    fun toggleSpeaker() {
        val on = !_state.value.speakerOn
        setSpeaker(on)
    }

    // ── Signal delivery from the event pump ───────────────────────────────────

    /**
     * Feed a peer's call signal into the active connection. Offers are handled
     * by [onIncomingOffer]; this routes the rest (answer / ICE / ringing /
     * busy / hangup) for a call that matches the current [callId].
     */
    fun onRemoteSignal(dto: CallSignalDto) {
        if (dto.signal is CallSignal.Offer) {
            onIncomingOffer(dto, (dto.signal as CallSignal.Offer).sdp)
            return
        }
        // Ignore stray signals for a call we're not in (e.g. a late hangup).
        if (!_state.value.active || dto.callId != callId) return
        when (val signal = dto.signal) {
            is CallSignal.Answer -> onRemoteAnswer(signal.sdp)
            is CallSignal.Ice -> onRemoteIce(signal)
            is CallSignal.Ringing -> Unit // caller UI already shows "Ringing…"
            is CallSignal.Busy -> endLocally("busy")
            is CallSignal.Hangup -> endLocally(signal.reason.name.lowercase())
            is CallSignal.Offer -> Unit // handled above
        }
    }

    private fun onRemoteAnswer(sdp: String) {
        val connection = pc ?: return
        _state.update { it.copy(stage = Stage.Connecting) }
        connection.setRemoteDescription(
            object : SimpleSdpObserver("setRemote(answer)") {
                override fun onSetSuccess() {
                    remoteDescriptionSet = true
                    drainPendingCandidates()
                }
            },
            SessionDescription(SessionDescription.Type.ANSWER, sdp),
        )
    }

    private fun onRemoteIce(ice: CallSignal.Ice) {
        val candidate = IceCandidate(
            ice.sdpMid ?: "",
            ice.sdpMLineIndex?.toInt() ?: 0,
            ice.candidate,
        )
        val connection = pc
        if (connection == null || !remoteDescriptionSet) {
            pendingRemoteCandidates += candidate
        } else {
            connection.addIceCandidate(candidate)
        }
    }

    // ── Renderer wiring (called by the call screen) ──────────────────────────

    fun setLocalRenderer(renderer: SurfaceViewRenderer?) {
        localRenderer = renderer
        val track = localVideoTrack
        if (renderer != null && track != null) runCatching { track.addSink(renderer) }
    }

    fun setRemoteRenderer(renderer: SurfaceViewRenderer?) {
        remoteRenderer = renderer
        val track = remoteVideoTrack
        if (renderer != null && track != null) runCatching { track.addSink(renderer) }
    }

    // ── Internals ─────────────────────────────────────────────────────────────

    private fun buildPeerConnection(iceServers: List<PeerConnection.IceServer>, video: Boolean): Boolean {
        val f = factory ?: return false
        val rtcConfig = PeerConnection.RTCConfiguration(iceServers).apply {
            sdpSemantics = PeerConnection.SdpSemantics.UNIFIED_PLAN
            continualGatheringPolicy =
                PeerConnection.ContinualGatheringPolicy.GATHER_CONTINUALLY
        }
        val connection = f.createPeerConnection(rtcConfig, pcObserver) ?: return false
        pc = connection

        // Microphone track — always present.
        val audio = f.createAudioSource(MediaConstraints())
        audioSource = audio
        val audioTrack = f.createAudioTrack(LOCAL_AUDIO_TRACK, audio)
        localAudioTrack = audioTrack
        connection.addTrack(audioTrack, listOf(STREAM_ID))

        if (video) {
            startCameraCapture(f)?.let { track ->
                connection.addTrack(track, listOf(STREAM_ID))
            }
        }

        // Default audio route: speaker for video calls, earpiece for voice.
        configureAudioForCall(speaker = video)
        return true
    }

    private fun startCameraCapture(f: PeerConnectionFactory): VideoTrack? {
        val ctx = appContext ?: return null
        val egl = eglBase ?: return null
        val enumerator: CameraEnumerator = Camera2Enumerator(ctx)
        val deviceName = enumerator.deviceNames.firstOrNull { enumerator.isFrontFacing(it) }
            ?: enumerator.deviceNames.firstOrNull()
            ?: return null
        val capturer = enumerator.createCapturer(deviceName, null) ?: return null
        val helper = SurfaceTextureHelper.create("CaptureThread", egl.eglBaseContext)
        val source = f.createVideoSource(false)
        capturer.initialize(helper, ctx, source.capturerObserver)
        runCatching { capturer.startCapture(VIDEO_WIDTH, VIDEO_HEIGHT, VIDEO_FPS) }
            .onFailure { Log.w(TAG, "startCapture failed", it) }
        val track = f.createVideoTrack(LOCAL_VIDEO_TRACK, source)
        videoCapturer = capturer
        surfaceHelper = helper
        videoSource = source
        localVideoTrack = track
        localRenderer?.let { runCatching { track.addSink(it) } }
        return track
    }

    private fun createOffer() {
        val connection = pc ?: return
        connection.createOffer(
            object : SimpleSdpObserver("createOffer") {
                override fun onCreateSuccess(desc: SessionDescription) {
                    connection.setLocalDescription(
                        object : SimpleSdpObserver("setLocal(offer)") {
                            override fun onSetSuccess() = sendSignal(CallSignal.Offer(desc.description))
                        },
                        desc,
                    )
                }
            },
            MediaConstraints(),
        )
    }

    private fun createAnswer() {
        val connection = pc ?: return
        connection.createAnswer(
            object : SimpleSdpObserver("createAnswer") {
                override fun onCreateSuccess(desc: SessionDescription) {
                    connection.setLocalDescription(
                        object : SimpleSdpObserver("setLocal(answer)") {
                            override fun onSetSuccess() = sendSignal(CallSignal.Answer(desc.description))
                        },
                        desc,
                    )
                }
            },
            MediaConstraints(),
        )
    }

    private fun drainPendingCandidates() {
        val connection = pc ?: return
        val queued = pendingRemoteCandidates.toList()
        pendingRemoteCandidates.clear()
        queued.forEach { connection.addIceCandidate(it) }
    }

    private val pcObserver = object : PeerConnection.Observer {
        override fun onSignalingChange(state: PeerConnection.SignalingState?) {}

        override fun onIceConnectionChange(newState: PeerConnection.IceConnectionState?) {
            when (newState) {
                PeerConnection.IceConnectionState.CONNECTED,
                PeerConnection.IceConnectionState.COMPLETED,
                -> _state.update {
                    if (it.stage == Stage.Ended) it else it.copy(stage = Stage.Active)
                }
                PeerConnection.IceConnectionState.FAILED -> {
                    sendHangup(HangupReason.FAILED)
                    endLocally("failed")
                }
                PeerConnection.IceConnectionState.DISCONNECTED -> {
                    // A brief blip can recover; a real drop follows as FAILED/CLOSED.
                }
                else -> Unit
            }
        }

        override fun onIceConnectionReceivingChange(receiving: Boolean) {}

        override fun onIceGatheringChange(state: PeerConnection.IceGatheringState?) {}

        override fun onIceCandidate(candidate: IceCandidate) {
            sendSignal(
                CallSignal.Ice(
                    candidate = candidate.sdp,
                    sdpMid = candidate.sdpMid,
                    sdpMLineIndex = candidate.sdpMLineIndex.toUShort(),
                ),
            )
        }

        override fun onIceCandidatesRemoved(candidates: Array<out IceCandidate>?) {}

        override fun onAddStream(stream: MediaStream?) {}

        override fun onRemoveStream(stream: MediaStream?) {}

        override fun onDataChannel(channel: org.webrtc.DataChannel?) {}

        override fun onRenegotiationNeeded() {}

        override fun onAddTrack(receiver: RtpReceiver?, streams: Array<out MediaStream>?) {
            val track = receiver?.track() ?: return
            if (track is VideoTrack) {
                remoteVideoTrack = track
                remoteRenderer?.let { runCatching { track.addSink(it) } }
            }
        }
    }

    private fun sendSignal(signal: CallSignal) {
        val p = peer
        val id = callId
        val m = mediaKind
        if (p.isEmpty() || id.isEmpty()) return
        scope.launch {
            runCatching { ComradeCore.sendCallSignalTyped(p, id, m, signal) }
                .onFailure { Log.w(TAG, "send signal failed", it) }
        }
    }

    private fun sendHangup(reason: HangupReason) {
        val p = peer
        val id = callId
        val m = mediaKind
        if (p.isEmpty() || id.isEmpty()) return
        scope.launch {
            runCatching { ComradeCore.hangupCallTyped(p, id, m, reason) }
                .onFailure { Log.w(TAG, "hangup send failed", it) }
        }
    }

    /** Tear down local media + connection and move to [Stage.Ended] → Idle. */
    private fun endLocally(reason: String) {
        val stage = _state.value.stage
        // Idempotent: a FAILED followed by a peer Hangup must only end once.
        if (stage == Stage.Idle || stage == Stage.Ended) return
        val wasConnected = stage == Stage.Active
        val logIncoming = incoming
        val logPeer = peer
        val logCallId = callId
        val logMedia = mediaKind
        _state.value = _state.value.copy(stage = Stage.Ended, endReason = reason)
        teardown()
        // Persist a call-log line (best-effort). Outcome mirrors the reason.
        scope.launch {
            val outcome = if (wasConnected) "connected" else reason
            runCatching {
                ComradeCore.logCallTyped(
                    logPeer, logCallId, logMedia, logIncoming, outcome, 0L, 0L,
                )
            }
        }
        // Hold "Ended" briefly so the screen can show it, then return to Idle.
        scope.launch {
            delay(RESET_DELAY_MS)
            if (_state.value.stage == Stage.Ended) _state.value = State()
        }
    }

    private fun teardown() {
        runCatching { videoCapturer?.stopCapture() }
        runCatching { videoCapturer?.dispose() }
        videoCapturer = null
        runCatching { surfaceHelper?.dispose() }
        surfaceHelper = null
        runCatching { localVideoTrack?.dispose() }
        localVideoTrack = null
        runCatching { videoSource?.dispose() }
        videoSource = null
        runCatching { localAudioTrack?.dispose() }
        localAudioTrack = null
        runCatching { audioSource?.dispose() }
        audioSource = null
        remoteVideoTrack = null
        runCatching { pc?.close() }
        runCatching { pc?.dispose() }
        pc = null
        pendingOffer = null
        pendingRemoteCandidates.clear()
        remoteDescriptionSet = false
        restoreAudio()
    }

    // ── Audio routing ─────────────────────────────────────────────────────────

    private fun audioManager(): AudioManager? =
        appContext?.getSystemService(Context.AUDIO_SERVICE) as? AudioManager

    private fun configureAudioForCall(speaker: Boolean) {
        val am = audioManager() ?: return
        am.mode = AudioManager.MODE_IN_COMMUNICATION
        setSpeaker(speaker)
    }

    private fun setSpeaker(on: Boolean) {
        val am = audioManager() ?: return
        @Suppress("DEPRECATION")
        am.isSpeakerphoneOn = on
        _state.update { it.copy(speakerOn = on) }
    }

    private fun restoreAudio() {
        val am = audioManager() ?: return
        @Suppress("DEPRECATION")
        am.isSpeakerphoneOn = false
        am.mode = AudioManager.MODE_NORMAL
    }

    // ── Small helpers ─────────────────────────────────────────────────────────

    private fun mediaFromStr(s: String): CallMediaKind =
        if (s == "video") CallMediaKind.VIDEO else CallMediaKind.AUDIO

    private fun ComradeCore.IceServerInfo.toWebRtc(): PeerConnection.IceServer {
        val builder = PeerConnection.IceServer.builder(urls)
        username?.let { builder.setUsername(it) }
        credential?.let { builder.setPassword(it) }
        return builder.createIceServer()
    }

    /** How long the "Call ended" state lingers before the screen returns to Idle. */
    private const val RESET_DELAY_MS = 1500L
}

/**
 * An [SdpObserver] whose create/set failures are logged and whose successes
 * default to no-op — subclasses override just the callback they care about.
 * WebRTC requires all four methods, so this removes the boilerplate at each
 * offer/answer step.
 */
private open class SimpleSdpObserver(private val tag: String) : SdpObserver {
    override fun onCreateSuccess(desc: SessionDescription) {}
    override fun onSetSuccess() {}
    override fun onCreateFailure(error: String?) {
        Log.w("CallManager", "$tag createFailure: $error")
    }
    override fun onSetFailure(error: String?) {
        Log.w("CallManager", "$tag setFailure: $error")
    }
}
