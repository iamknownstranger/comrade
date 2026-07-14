package mullu.comrade.call

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.os.Build
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import mullu.comrade.ComradeCore
import org.webrtc.AudioSource
import org.webrtc.AudioTrack
import org.webrtc.Camera2Enumerator
import org.webrtc.CameraVideoCapturer
import org.webrtc.DefaultVideoDecoderFactory
import org.webrtc.DefaultVideoEncoderFactory
import org.webrtc.EglBase
import org.webrtc.IceCandidate
import org.webrtc.MediaConstraints
import org.webrtc.MediaStream
import org.webrtc.PeerConnection
import org.webrtc.PeerConnectionFactory
import org.webrtc.RTCStatsReport
import org.webrtc.RtpReceiver
import org.webrtc.SdpObserver
import org.webrtc.SessionDescription
import org.webrtc.SurfaceTextureHelper
import org.webrtc.VideoCapturer
import org.webrtc.VideoSource
import org.webrtc.VideoTrack
import uniffi.comrade_core.CallMediaKind
import uniffi.comrade_core.CallSignal
import uniffi.comrade_core.HangupReason
import uniffi.comrade_core.IceStrategy
import uniffi.comrade_ui.CallSignalDto

/**
 * The Android side of a WebRTC voice/video call.
 *
 * The Rust core ([`comrade_core::call`]) owns the *wire protocol*: it mints the
 * call id, wraps each [CallSignal] in a NIP-59 DM envelope, and routes it over
 * the encrypted Vault channel — all of which we reach through [ComradeCore]. The
 * *media* — mic/camera capture and the `PeerConnection` — has no home in Rust
 * and lives here, in `org.webrtc`.
 *
 * This object is the bridge between the two: it drives an `org.webrtc`
 * [PeerConnection] through the exact signaling handshake the desktop webview
 * uses (see `desktop/ui/main.js`), forwarding every locally-generated SDP/ICE
 * payload to the core via [ComradeCore.sendCallSignalTyped] and feeding every
 * remote payload (delivered as [CallSignalDto] by the event pump) back into the
 * peer connection.
 *
 * ## State machine (mirrors the desktop)
 * ```
 * caller:  Idle → Ringing(out) → Connecting → Active → Ended → Idle
 * callee:  Idle → Ringing(in)  → Connecting → Active → Ended → Idle
 * ```
 * There is no explicit "accept" signal — the callee's `Answer` *is* the accept;
 * `Ringing` is purely informational. Exactly one call exists at a time; a second
 * incoming offer is auto-rejected with [CallSignal.Busy].
 *
 * ## Threading
 * `org.webrtc` invokes its [PeerConnection.Observer]/[SdpObserver] callbacks on
 * internal signaling threads. Observable state is held in [StateFlow]s (safe to
 * publish from any thread); the blocking FFI sends run on [io]. Mutating call
 * transitions are `@Synchronized` on this object so the pump thread, the WebRTC
 * threads, and the UI thread can't interleave a teardown with a fresh signal.
 */
object CallManager {

    private const val TAG = "CallManager"
    private const val STREAM_ID = "comrade-call"
    private const val LOCAL_AUDIO_ID = "comrade-audio"
    private const val LOCAL_VIDEO_ID = "comrade-video"
    private const val CAMERA_WIDTH = 1280
    private const val CAMERA_HEIGHT = 720
    private const val CAMERA_FPS = 30

    /** How long the [CallUiState.Ended] card lingers before returning to [CallUiState.Idle]. */
    private const val ENDED_LINGER_MS = 1_600L

    /**
     * Ring timeout: how long we wait for the callee to answer (caller side) or
     * for the user to accept an incoming call (callee side) before giving up with
     * "No answer". Without this a call whose offer is never answered — an offline
     * peer, an un-accepted conversation, or a dropped signaling DM — hangs on
     * "Calling…" forever.
     */
    private const val RING_TIMEOUT_MS = 45_000L

    /**
     * Connect timeout: once the answer is exchanged, how long we give ICE to
     * actually reach `CONNECTED` (through the STUN→TURN fallback) before failing
     * with "Couldn't connect".
     */
    private const val CONNECT_TIMEOUT_MS = 30_000L

    /**
     * How often [startStatsPolling] samples [PeerConnection.getStats] for the
     * connection-quality indicator. Frequent enough that the indicator feels
     * live, cheap enough that the extra wakeups don't matter.
     */
    private const val STATS_POLL_MS = 2_000L

    /**
     * Round-trip-time thresholds behind [classifyQuality] — a deliberately
     * simple heuristic, not a real quality model: at/under [RTT_GOOD_MS] (with
     * jitter also low) reads as [CallQuality.GOOD]; up to [RTT_MEDIUM_MS] is
     * [CallQuality.MEDIUM]; anything worse — or stats we can't read — is
     * [CallQuality.POOR]/[CallQuality.UNKNOWN]. Good enough to flag an
     * obviously-bad call, not meant to be precise.
     */
    private const val RTT_GOOD_MS = 150.0
    private const val RTT_MEDIUM_MS = 400.0

    /** Above this, jitter alone knocks an otherwise-GOOD RTT reading down to MEDIUM. */
    private const val JITTER_GOOD_MS = 30.0

    private val io = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    // ── Observable state for the Compose layer ───────────────────────────────
    private val _state = MutableStateFlow<CallUiState>(CallUiState.Idle)
    val state: StateFlow<CallUiState> = _state.asStateFlow()

    private val _localVideo = MutableStateFlow<VideoTrack?>(null)
    /** The local camera track (video calls only), for the self-preview renderer. */
    val localVideo: StateFlow<VideoTrack?> = _localVideo.asStateFlow()

    private val _remoteVideo = MutableStateFlow<VideoTrack?>(null)
    /** The remote camera track, published once the peer's video arrives. */
    val remoteVideo: StateFlow<VideoTrack?> = _remoteVideo.asStateFlow()

    private val _muted = MutableStateFlow(false)
    val muted: StateFlow<Boolean> = _muted.asStateFlow()

    private val _cameraOn = MutableStateFlow(true)
    /** Whether the local camera is currently capturing (video calls only). */
    val cameraOn: StateFlow<Boolean> = _cameraOn.asStateFlow()

    private val _connectionQuality = MutableStateFlow(CallQuality.UNKNOWN)
    /** A live, heuristic read on the current call's media quality — see [CallQuality]. */
    val connectionQuality: StateFlow<CallQuality> = _connectionQuality.asStateFlow()

    private val _audioRoute = MutableStateFlow(AudioRoute.EARPIECE)
    /** Where in-call audio is currently playing. */
    val audioRoute: StateFlow<AudioRoute> = _audioRoute.asStateFlow()

    private val _availableRoutes = MutableStateFlow(listOf(AudioRoute.EARPIECE, AudioRoute.SPEAKER))
    /** Which [AudioRoute]s are selectable right now — grows/shrinks as headsets connect. */
    val availableRoutes: StateFlow<List<AudioRoute>> = _availableRoutes.asStateFlow()

    private val _sasEmojis = MutableStateFlow<List<String>?>(null)

    /**
     * The 4-emoji short authentication string for the current call, once
     * connected and both sides' SDPs are known — for the two participants to
     * read aloud and compare, catching a man-in-the-middle that re-terminated
     * the DTLS-SRTP media path. `null` before that point, or if it could not
     * be derived at all (a missing fingerprint on either side): an honest
     * "can't verify", never a fabricated code. See [onConnected].
     */
    val sasEmojis: StateFlow<List<String>?> = _sasEmojis.asStateFlow()

    // ── WebRTC singletons (lazily built on the first call, never at startup) ──
    private var appContext: Context? = null
    private var eglBase: EglBase? = null
    private var factory: PeerConnectionFactory? = null

    /** The shared EGL context renderers must init against (null until a call runs). */
    val eglBaseContext: EglBase.Context? get() = eglBase?.eglBaseContext

    private var session: Session? = null

    /** Everything mutable about the one in-flight call. */
    private class Session(
        val callId: String,
        val peer: String,
        val peerLabel: String,
        val media: CallMediaKind,
        val incoming: Boolean,
    ) {
        var pc: PeerConnection? = null
        var audioSource: AudioSource? = null
        var audioTrack: AudioTrack? = null
        var videoSource: VideoSource? = null
        var videoTrack: VideoTrack? = null
        var capturer: VideoCapturer? = null
        var surfaceHelper: SurfaceTextureHelper? = null

        /** Remote description applied — gates ICE (WebRTC rejects early candidates). */
        var remoteSet = false
        val pendingIce = ArrayList<IceCandidate>()

        /** Callee only: the offer SDP, buffered from ring until Accept. */
        var offerSdp: String? = null

        /**
         * This side's negotiated local SDP — the offer for the caller, the
         * answer for the callee — captured once `setLocalDescription`
         * succeeds (see [setLocalThen]). Feeds SAS derivation in
         * [onConnected] once [remoteSdp] is known too. Overwritten, not
         * appended, on a renegotiation (ICE-restart TURN fallback, or an
         * answered re-offer), so it never goes stale.
         */
        var localSdp: String? = null

        /**
         * The peer's negotiated remote SDP — the answer for the caller, the
         * offer for the callee — captured once `setRemoteDescription`
         * succeeds (see [setRemoteThen]). Same overwrite-on-renegotiation
         * behavior as [localSdp].
         */
        var remoteSdp: String? = null

        val startedAtMs = System.currentTimeMillis()
        var connectedAtMs = 0L
        val isVideo get() = media == CallMediaKind.VIDEO

        /** Set once we've widened to STUN+TURN, so the fallback fires at most once. */
        var triedTurn = false

        /** Idempotency guard so a hangup + a remote hangup don't double-teardown. */
        var ended = false

        /** The pending ring/connect timeout, cancelled on connect or teardown. */
        var timeoutJob: Job? = null

        /** The connection-quality stats-polling loop, started on connect and cancelled on teardown. */
        var statsJob: Job? = null

        /** Caller side: the callee's device has acked with a `Ringing` signal. */
        var remoteRinging = false
    }

    // ── Public API: outgoing ─────────────────────────────────────────────────

    /**
     * Place a call to [peer]. Runs the STUN-only first attempt the core's design
     * intends: [ComradeCore.placeCallTyped] returns the minted call id and a
     * STUN-only ICE list, we build the peer connection, and send the `Offer`.
     *
     * Permissions (mic, + camera for video) must already be granted — the UI
     * gates on that before calling in.
     */
    @Synchronized
    fun startOutgoingCall(context: Context, peer: String, peerLabel: String, media: CallMediaKind) {
        if (session != null) {
            Log.w(TAG, "startOutgoingCall ignored: a call is already in progress")
            return
        }
        ensureFactory(context)
        // Optimistic ringing state so the UI opens immediately; the placeCall +
        // offer happen on IO because placeCall touches the store and the signal
        // send is a blocking DM round-trip.
        _state.value = CallUiState.Ringing(peer, peerLabel, media == CallMediaKind.VIDEO, incoming = false)
        io.launch {
            val placed = runCatching { ComradeCore.placeCallTyped(peer, media) }
                .getOrElse {
                    Log.e(TAG, "placeCall failed", it)
                    endWith(HangupReason.FAILED, "failed", sendHangup = false)
                    return@launch
                }
            synchronized(this@CallManager) {
                if (session != null) return@launch // raced with a teardown
                val s = Session(placed.callId, placed.peer, peerLabel, media, incoming = false)
                session = s
                val ice = placed.iceServers.map { it.toWebRtc() }
                if (!setupPeer(s, ice)) return@launch
                // Caller creates the offer, sets it local, then signals it. A
                // failed offer send ends the call (see sendSignalOrFail) instead
                // of hanging on "Calling…".
                s.pc?.createOffer(
                    createSdpObserver(s) { offer ->
                        setLocalThen(s, offer) { sendSignalOrFail(s, CallSignal.Offer(offer.description)) }
                    },
                    mediaConstraints(s.isVideo),
                )
                // Give up with "No answer" if the callee never picks up.
                armTimeout(s, RING_TIMEOUT_MS, HangupReason.MISSED, "missed")
            }
        }
    }

    // ── Public API: incoming (driven by the event pump) ──────────────────────

    /**
     * Route one incoming [CallSignalDto] (delivered by the MainActivity event
     * pump from [uniffi.comrade_ui.BridgeEvent.IncomingCallSignal]). Returns
     * `true` if this signal is a fresh incoming offer that should raise a
     * ringing notification, so the caller can fire [mullu.comrade.Notifier].
     */
    @Synchronized
    fun onIncomingSignal(dto: CallSignalDto): Boolean {
        val s = session
        return when (val signal = dto.signal) {
            is CallSignal.Offer -> handleRemoteOffer(dto, signal.sdp)
            is CallSignal.Answer -> {
                if (s != null && s.callId == dto.callId) applyRemoteAnswer(s, signal.sdp)
                false
            }
            is CallSignal.Ice -> {
                if (s != null && s.callId == dto.callId) addRemoteIce(s, signal)
                false
            }
            CallSignal.Ringing -> {
                // Caller side: the callee's device is ringing (pre-answer) — show
                // "Ringing…" instead of "Calling…".
                if (s != null && s.callId == dto.callId && _state.value is CallUiState.Ringing) {
                    s.remoteRinging = true
                    _state.value = CallUiState.Ringing(
                        s.peer, s.peerLabel, s.isVideo, incoming = false, remoteRinging = true,
                    )
                }
                false
            }
            CallSignal.Busy -> {
                if (s != null && s.callId == dto.callId) endWith(HangupReason.BUSY, "busy", sendHangup = false)
                false
            }
            is CallSignal.Hangup -> {
                if (s != null && s.callId == dto.callId) {
                    val connected = s.connectedAtMs > 0
                    endWith(signal.reason, outcomeForRemoteHangup(signal.reason, connected), sendHangup = false)
                }
                false
            }
        }
    }

    /** Accept the ringing incoming call: build the peer connection and answer. */
    @Synchronized
    fun accept(context: Context) {
        val s = session ?: return
        if (s.incoming.not() || _state.value !is CallUiState.Ringing) return
        val offer = s.offerSdp ?: run {
            endWith(HangupReason.FAILED, "failed", sendHangup = true)
            return
        }
        ensureFactory(context)
        _state.value = CallUiState.Connecting(s.peer, s.peerLabel, s.isVideo, incoming = true)
        io.launch {
            synchronized(this@CallManager) {
                if (session !== s || s.ended) return@launch
                // The callee starts STUN-only too; the fallback (below) widens to
                // TURN if the direct/STUN path never connects.
                val ice = runCatching { ComradeCore.callIceServersForTyped(IceStrategy.STUN_ONLY) }
                    .getOrDefault(emptyList())
                    .map { it.toWebRtc() }
                if (!setupPeer(s, ice)) return@launch
                setRemoteThen(s, SessionDescription(SessionDescription.Type.OFFER, offer)) {
                    s.pc?.createAnswer(
                        createSdpObserver(s) { answer ->
                            setLocalThen(s, answer) { sendSignalOrFail(s, CallSignal.Answer(answer.description)) }
                        },
                        MediaConstraints(), // Empty constraints for Answers
                    )
                }
                // Answered — now fail with "Couldn't connect" if ICE never completes.
                armTimeout(s, CONNECT_TIMEOUT_MS, HangupReason.FAILED, "failed")
            }
        }
    }

    /** Reject the ringing incoming call (callee declines before answering). */
    @Synchronized
    fun reject() {
        if (session == null) return
        endWith(HangupReason.DECLINED, "declined", sendHangup = true)
    }

    /** Hang up / cancel the current call from the local UI. */
    @Synchronized
    fun hangup() {
        val s = session ?: return
        val connected = s.connectedAtMs > 0
        val reason = when {
            connected -> HangupReason.NORMAL
            s.incoming -> HangupReason.DECLINED
            else -> HangupReason.CANCELLED
        }
        val outcome = when {
            connected -> "connected"
            s.incoming -> "declined"
            else -> "cancelled"
        }
        endWith(reason, outcome, sendHangup = true)
    }

    // ── Toggles ──────────────────────────────────────────────────────────────

    /** Flip local mic enablement (no renegotiation — just [AudioTrack.setEnabled]). */
    @Synchronized
    fun toggleMute() {
        val s = session ?: return
        val next = !_muted.value
        s.audioTrack?.setEnabled(!next)
        _muted.value = next
    }

    /**
     * Turn the local camera off/on mid-call (video calls only) — no
     * renegotiation, matching [toggleMute]. Turning off both disables the
     * track (so the peer, and the local self-preview, stop receiving frames)
     * and stops the capturer (releasing the physical camera, not just muting
     * it); turning back on resumes both. A no-op for audio calls.
     */
    @Synchronized
    fun toggleCamera() {
        val s = session ?: return
        if (!s.isVideo) return
        val next = !_cameraOn.value
        if (next) {
            runCatching { s.capturer?.startCapture(CAMERA_WIDTH, CAMERA_HEIGHT, CAMERA_FPS) }
                .onFailure { Log.w(TAG, "startCapture failed", it) }
            s.videoTrack?.setEnabled(true)
        } else {
            s.videoTrack?.setEnabled(false)
            runCatching { s.capturer?.stopCapture() }.onFailure { Log.w(TAG, "stopCapture failed", it) }
        }
        _cameraOn.value = next
    }

    /** Cycle to the next available [AudioRoute] (earpiece → speaker → Bluetooth/wired → …). */
    @Synchronized
    fun cycleAudioRoute() {
        val avail = _availableRoutes.value
        if (avail.isEmpty()) return
        val idx = avail.indexOf(_audioRoute.value).coerceAtLeast(0)
        setAudioRoute(avail[(idx + 1) % avail.size])
    }

    /** Explicitly route in-call audio to [route], if it's currently in [availableRoutes]. */
    @Synchronized
    fun setAudioRoute(route: AudioRoute) {
        if (route !in _availableRoutes.value) return
        val am = appContext?.getSystemService(Context.AUDIO_SERVICE) as? AudioManager ?: return
        applyAudioRoute(am, route)
        _audioRoute.value = route
    }

    /**
     * Apply [route] to the platform. API 31+ has a purpose-built API for
     * exactly this (`setCommunicationDevice`); below that, routing is the
     * older speakerphone flag plus manual Bluetooth SCO start/stop.
     */
    private fun applyAudioRoute(am: AudioManager, route: AudioRoute) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            val wantedType = when (route) {
                AudioRoute.EARPIECE -> AudioDeviceInfo.TYPE_BUILTIN_EARPIECE
                AudioRoute.SPEAKER -> AudioDeviceInfo.TYPE_BUILTIN_SPEAKER
                AudioRoute.BLUETOOTH -> AudioDeviceInfo.TYPE_BLUETOOTH_SCO
                AudioRoute.WIRED -> AudioDeviceInfo.TYPE_WIRED_HEADSET
            }
            val device = am.availableCommunicationDevices.firstOrNull {
                it.type == wantedType ||
                    (route == AudioRoute.WIRED && it.type == AudioDeviceInfo.TYPE_WIRED_HEADPHONES)
            }
            try {
                if (device != null) am.setCommunicationDevice(device) else am.clearCommunicationDevice()
            } catch (e: SecurityException) {
                // Routing to a TYPE_BLUETOOTH_SCO device needs BLUETOOTH_CONNECT
                // on API 31+; without it, fall back to the default route
                // instead of crashing the call.
                Log.w(TAG, "missing BLUETOOTH_CONNECT for setCommunicationDevice; clearing route", e)
                am.clearCommunicationDevice()
            }
        } else {
            applyAudioRouteLegacy(am, route)
        }
    }

    @Suppress("DEPRECATION") // isSpeakerphoneOn/isBluetoothScoOn/*BluetoothSco: the API 31+ path above covers modern devices
    private fun applyAudioRouteLegacy(am: AudioManager, route: AudioRoute) {
        if (route == AudioRoute.BLUETOOTH) {
            am.isSpeakerphoneOn = false
            if (!am.isBluetoothScoOn) am.startBluetoothSco()
            am.isBluetoothScoOn = true
        } else {
            if (am.isBluetoothScoOn) am.stopBluetoothSco()
            am.isBluetoothScoOn = false
            am.isSpeakerphoneOn = route == AudioRoute.SPEAKER
        }
    }

    /** Flip between the front and back cameras (video calls only). */
    @Synchronized
    fun switchCamera() {
        (session?.capturer as? CameraVideoCapturer)?.switchCamera(null)
    }

    // ── Peer connection setup ─────────────────────────────────────────────────

    /** Build the [PeerConnection] and local tracks. Returns false (and tears down) on failure. */
    private fun setupPeer(s: Session, iceServers: List<PeerConnection.IceServer>): Boolean {
        val fac = factory ?: return false
        val config = PeerConnection.RTCConfiguration(iceServers).apply {
            sdpSemantics = PeerConnection.SdpSemantics.UNIFIED_PLAN
            continualGatheringPolicy = PeerConnection.ContinualGatheringPolicy.GATHER_CONTINUALLY
            bundlePolicy = PeerConnection.BundlePolicy.MAXBUNDLE
            rtcpMuxPolicy = PeerConnection.RtcpMuxPolicy.REQUIRE
        }
        val pc = fac.createPeerConnection(config, peerObserver(s)) ?: run {
            Log.e(TAG, "createPeerConnection returned null")
            endWith(HangupReason.FAILED, "failed", sendHangup = false)
            return false
        }
        s.pc = pc

        // Local microphone — always.
        val audioSource = fac.createAudioSource(MediaConstraints())
        val audioTrack = fac.createAudioTrack(LOCAL_AUDIO_ID, audioSource).apply { setEnabled(!_muted.value) }
        s.audioSource = audioSource
        s.audioTrack = audioTrack
        pc.addTrack(audioTrack, listOf(STREAM_ID))

        // Local camera — video calls only.
        if (s.isVideo) {
            val capturer = createCameraCapturer()
            if (capturer != null) {
                val ctx = appContext!!
                val helper = SurfaceTextureHelper.create("CaptureThread", eglBase!!.eglBaseContext)
                val videoSource = fac.createVideoSource(false)
                capturer.initialize(helper, ctx, videoSource.capturerObserver)
                // The physical camera can be unavailable (in use by another
                // app, restricted by policy, hardware fault, …); an
                // uncaught failure here would otherwise abort the whole call
                // setup instead of just leaving this call without a live
                // local preview.
                runCatching { capturer.startCapture(CAMERA_WIDTH, CAMERA_HEIGHT, CAMERA_FPS) }
                    .onFailure { Log.e(TAG, "camera capture start failed; continuing without local video", it) }
                val videoTrack = fac.createVideoTrack(LOCAL_VIDEO_ID, videoSource).apply { setEnabled(true) }
                s.capturer = capturer
                s.surfaceHelper = helper
                s.videoSource = videoSource
                s.videoTrack = videoTrack
                pc.addTrack(videoTrack, listOf(STREAM_ID))
                _localVideo.value = videoTrack
            } else {
                Log.w(TAG, "no camera available; continuing audio-only")
            }
        }

        beginAudioRouting(s.isVideo)
        // Keep the process alive & visible for the rest of the call even if
        // the app is backgrounded — see CallService's doc comment.
        appContext?.let { ctx ->
            runCatching { CallService.start(ctx, s.peer, s.peerLabel, s.isVideo) }
                .onFailure { Log.w(TAG, "Failed to start CallService (foreground restrictions)", it) }
        }
        return true
    }

    private fun createCameraCapturer(): VideoCapturer? {
        val ctx = appContext ?: return null
        if (!Camera2Enumerator.isSupported(ctx)) return null
        val enumerator = Camera2Enumerator(ctx)
        val names = enumerator.deviceNames
        // Prefer the front camera for a call; fall back to any camera.
        names.firstOrNull { enumerator.isFrontFacing(it) }?.let { return enumerator.createCapturer(it, null) }
        names.firstOrNull()?.let { return enumerator.createCapturer(it, null) }
        return null
    }

    // ── Remote signal application ─────────────────────────────────────────────

    /** A fresh offer: ring (or renegotiate an existing call, or reject as busy). */
    private fun handleRemoteOffer(dto: CallSignalDto, sdp: String): Boolean {
        val existing = session
        if (existing != null) {
            // A re-offer for the current call (e.g. the caller's TURN ICE-restart)
            // is a renegotiation, not a new call — answer it on the existing pc.
            if (existing.callId == dto.callId && existing.pc != null) {
                setRemoteThen(existing, SessionDescription(SessionDescription.Type.OFFER, sdp)) {
                    existing.pc?.createAnswer(
                        createSdpObserver(existing) { answer ->
                            setLocalThen(existing, answer) {
                                sendSignal(existing, CallSignal.Answer(answer.description))
                            }
                        },
                        MediaConstraints(), // Empty constraints for Answers — see accept()
                    )
                }
                return false
            }
            // Otherwise we're already busy on another call — auto-reject.
            val busyMedia = mediaKindOf(dto.media)
            io.launch {
                runCatching {
                    ComradeCore.sendCallSignalTyped(dto.peer, dto.callId, busyMedia, CallSignal.Busy)
                    ComradeCore.logCallTyped(
                        dto.peer, dto.callId, busyMedia,
                        incoming = true, outcome = "busy", startedAt = 0, durationSecs = 0,
                    )
                }.onFailure { Log.w(TAG, "busy-reject failed", it) }
            }
            return false
        }

        val media = mediaKindOf(dto.media)
        val s = Session(dto.callId, dto.peer, mullu.comrade.ui.shortNpub(dto.peer), media, incoming = true)
        s.offerSdp = sdp
        session = s
        _state.value = CallUiState.Ringing(s.peer, s.peerLabel, s.isVideo, incoming = true)
        // Best-effort "ringing on my device" ack; failure is non-fatal.
        sendSignal(s, CallSignal.Ringing)
        // Auto-miss the call if the user never accepts.
        armTimeout(s, RING_TIMEOUT_MS, HangupReason.MISSED, "missed")
        return true
    }

    private fun applyRemoteAnswer(s: Session, sdp: String) {
        setRemoteThen(s, SessionDescription(SessionDescription.Type.ANSWER, sdp)) {
            if (_state.value is CallUiState.Ringing) {
                _state.value = CallUiState.Connecting(s.peer, s.peerLabel, s.isVideo, incoming = false)
            }
            // Answer in hand — switch from the ring timeout to the connect timeout.
            armTimeout(s, CONNECT_TIMEOUT_MS, HangupReason.FAILED, "failed")
        }
    }

    private fun addRemoteIce(s: Session, ice: CallSignal.Ice) {
        val candidate = remoteIceCandidate(ice)
        val pc = s.pc
        if (pc != null && s.remoteSet) {
            Log.i(TAG, "remote ICE candidate (${candidate.sdp.iceCandidateType()}), callId=${s.callId}")
            pc.addIceCandidate(candidate)
        } else {
            Log.i(TAG, "remote ICE candidate buffered before remote description set, callId=${s.callId}")
            s.pendingIce.add(candidate)
        }
    }

    private fun flushPendingIce(s: Session) {
        val pc = s.pc ?: return
        s.pendingIce.forEach { pc.addIceCandidate(it) }
        s.pendingIce.clear()
    }

    // ── STUN → TURN fallback (caller side) ────────────────────────────────────

    /**
     * The direct/STUN path failed to connect. If we haven't already, widen to
     * STUN+TURN ([IceStrategy.STUN_AND_TURN]) and restart ICE with a fresh
     * offer — the CGNAT case the core keeps a TURN relay for. Only the caller
     * drives this; the callee answers the re-offer as a renegotiation. With no
     * TURN configured the widened list equals the STUN-only one, so we skip
     * straight to an honest "failed".
     *
     * The callee side deliberately does *not* end the call here: a Hangup
     * sent the instant this side's ICE agent reports FAILED can reach the
     * caller before the caller's own agent has reported anything at all,
     * foreclosing the caller's rescue attempt every time a failure happens to
     * show up on the callee first (WebRTC's failure timing between the two
     * sides is not synchronized). The connect timeout already armed in
     * [accept] is the callee's backstop if no rescuing re-offer arrives.
     */
    private fun tryTurnFallbackOrFail(s: Session) {
        if (s.ended) return
        if (s.incoming) {
            Log.w(TAG, "ICE failed on the callee side (callId=${s.callId}); waiting for a possible caller re-offer")
            return
        }
        if (s.triedTurn) {
            endWith(HangupReason.FAILED, "failed", sendHangup = true)
            return
        }
        s.triedTurn = true
        io.launch {
            val stunOnly = runCatching { ComradeCore.callIceServersForTyped(IceStrategy.STUN_ONLY) }.getOrDefault(emptyList())
            val widened = runCatching { ComradeCore.callIceServersForTyped(IceStrategy.STUN_AND_TURN) }.getOrDefault(emptyList())
            synchronized(this@CallManager) {
                if (session !== s || s.ended) return@launch
                val pc = s.pc
                if (pc == null || widened.size <= stunOnly.size) {
                    // No relay to fall back to — end honestly.
                    endWith(HangupReason.FAILED, "failed", sendHangup = true)
                    return@launch
                }
                val config = PeerConnection.RTCConfiguration(widened.map { it.toWebRtc() }).apply {
                    sdpSemantics = PeerConnection.SdpSemantics.UNIFIED_PLAN
                    continualGatheringPolicy = PeerConnection.ContinualGatheringPolicy.GATHER_CONTINUALLY
                    bundlePolicy = PeerConnection.BundlePolicy.MAXBUNDLE
                    rtcpMuxPolicy = PeerConnection.RtcpMuxPolicy.REQUIRE
                }
                pc.setConfiguration(config)
                s.remoteSet = false
                pc.createOffer(
                    createSdpObserver(s) { offer ->
                        setLocalThen(s, offer) { sendSignal(s, CallSignal.Offer(offer.description)) }
                    },
                    mediaConstraints(s.isVideo).apply {
                        mandatory.add(MediaConstraints.KeyValuePair("IceRestart", "true"))
                    },
                )
            }
        }
    }

    // ── Connection lifecycle ──────────────────────────────────────────────────

    private fun onConnected(s: Session) {
        if (s.connectedAtMs == 0L) s.connectedAtMs = System.currentTimeMillis()
        s.timeoutJob?.cancel() // connected — no timeout applies any more
        _state.value = CallUiState.Active(s.peer, s.peerLabel, s.isVideo, s.incoming, s.connectedAtMs)
        startStatsPolling(s)
        maybeDeriveSas(s)
    }

    /**
     * Poll [PeerConnection.getStats] every [STATS_POLL_MS] while this call
     * stays connected, updating [_connectionQuality] via [classifyQuality].
     * `getStats`'s callback fires on an internal WebRTC thread, so — like
     * every other WebRTC-callback path in this file — the read of [session]
     * and the write to [_connectionQuality] are synchronized and re-check
     * that [s] is still the live, un-ended session before touching anything.
     */
    private fun startStatsPolling(s: Session) {
        s.statsJob?.cancel()
        s.statsJob = io.launch {
            while (true) {
                delay(STATS_POLL_MS)
                val pc = synchronized(this@CallManager) { if (session === s && !s.ended) s.pc else null } ?: break
                pc.getStats { report ->
                    val quality = classifyQuality(report)
                    synchronized(this@CallManager) {
                        if (session === s && !s.ended) _connectionQuality.value = quality
                    }
                }
            }
        }
    }

    /**
     * A deliberately simple heuristic — not a real quality model. Reads
     * `roundTripTime`/`jitter` off the RTCP-derived `"remote-inbound-rtp"`
     * stats (present once the peer's receiver reports start arriving),
     * falling back to the ICE `"candidate-pair"`'s STUN-derived
     * `currentRoundTripTime` if no RTP-stream stats are in yet (e.g. right
     * after connecting). `RTCStats.getMembers()` is a loosely-typed
     * `Map<String, Object>`, so every field is read as a nullable [Number]
     * rather than hard-cast — a missing or renamed field degrades this one
     * poll to [CallQuality.UNKNOWN] instead of crashing.
     */
    private fun classifyQuality(report: RTCStatsReport): CallQuality {
        var remoteRttSeconds: Double? = null
        var jitterSeconds: Double? = null
        var pairRttSeconds: Double? = null

        for (stat in report.statsMap.values) {
            val members = stat.members
            when (stat.type) {
                "remote-inbound-rtp" -> {
                    (members["roundTripTime"] as? Number)?.toDouble()?.let { rtt ->
                        remoteRttSeconds = maxOf(remoteRttSeconds ?: rtt, rtt)
                    }
                    (members["jitter"] as? Number)?.toDouble()?.let { jitter ->
                        jitterSeconds = maxOf(jitterSeconds ?: jitter, jitter)
                    }
                }
                "candidate-pair" -> {
                    if (members["state"] == "succeeded") {
                        (members["currentRoundTripTime"] as? Number)?.toDouble()?.let { pairRttSeconds = it }
                    }
                }
            }
        }

        val rttSeconds = remoteRttSeconds ?: pairRttSeconds ?: return CallQuality.UNKNOWN
        val rttMs = rttSeconds * 1000.0
        val jitterMs = jitterSeconds?.times(1000.0)
        return when {
            rttMs <= RTT_GOOD_MS && (jitterMs == null || jitterMs <= JITTER_GOOD_MS) -> CallQuality.GOOD
            rttMs <= RTT_MEDIUM_MS -> CallQuality.MEDIUM
            else -> CallQuality.POOR
        }
    }

    /**
     * Kick off short-authentication-string derivation once both this side's
     * and the peer's SDP are known. Runs the (blocking, native) FFI call on
     * [io] rather than inline — this fires from inside a `synchronized`
     * block ([onConnected] is called from [peerObserver]'s
     * `onConnectionChange`, itself synchronized) — and publishes the result
     * back under a fresh `synchronized` block, matching [sendSignal]'s
     * fire-and-forget shape. A missing fingerprint on either side yields
     * `null` from [mullu.comrade.ComradeCore.callSasTyped] — a valid "can't
     * verify" outcome, published as-is rather than hidden as an error.
     */
    private fun maybeDeriveSas(s: Session) {
        val local = s.localSdp
        val remote = s.remoteSdp
        if (local == null || remote == null) return
        io.launch {
            val sas = runCatching { ComradeCore.callSasTyped(local, remote) }
                .onFailure { Log.w(TAG, "callSas failed", it) }
                .getOrNull()
            synchronized(this@CallManager) {
                if (session === s && !s.ended) _sasEmojis.value = sas
            }
        }
    }

    /**
     * Terminate the call: optionally signal a hangup, log the outcome, release
     * all media/hardware, and surface the [CallUiState.Ended] card before
     * returning to [CallUiState.Idle]. Idempotent — the first caller wins.
     * `@Synchronized` (the object monitor is reentrant) so it is safe both from
     * the already-locked transitions and from the bare `placeCall` failure path.
     */
    @Synchronized
    private fun endWith(reason: HangupReason, outcome: String, sendHangup: Boolean) {
        val s = session ?: run {
            if (_state.value !is CallUiState.Idle) _state.value = CallUiState.Idle
            return
        }
        if (s.ended) return
        s.ended = true
        s.timeoutJob?.cancel()

        if (sendHangup) {
            io.launch {
                runCatching { ComradeCore.hangupCallTyped(s.peer, s.callId, s.media, reason) }
                    .onFailure { Log.w(TAG, "hangup signal failed", it) }
            }
        }
        // Log the call to the store-backed history (exercises logCallTyped; the
        // record is what callHistoryTyped later reads back).
        io.launch {
            val duration = if (s.connectedAtMs > 0) (System.currentTimeMillis() - s.connectedAtMs) / 1000 else 0L
            runCatching {
                ComradeCore.logCallTyped(
                    peer = s.peer,
                    callId = s.callId,
                    media = s.media,
                    incoming = s.incoming,
                    outcome = outcome,
                    startedAt = s.startedAtMs / 1000,
                    durationSecs = duration,
                )
            }.onFailure { Log.w(TAG, "logCall failed", it) }
        }

        teardownMedia(s)
        session = null
        _state.value = CallUiState.Ended(s.peer, s.peerLabel, s.isVideo, s.incoming, outcome)
        io.launch {
            delay(ENDED_LINGER_MS)
            synchronized(this@CallManager) {
                if (session == null && _state.value is CallUiState.Ended) _state.value = CallUiState.Idle
            }
        }
    }

    /**
     * Release every camera/microphone/PeerConnection handle. Called from
     * inside [endWith]'s `@Synchronized` block, so everything here must
     * return fast: the WebRTC `.dispose()` calls are synchronous native
     * shutdowns that block the calling thread until the internal signaling
     * threads join, and if one of those threads concurrently re-enters this
     * object's monitor (e.g. [peerObserver]'s `onConnectionChange` firing
     * mid-teardown), the two threads deadlock forever (an ANR). References
     * are captured and the session's fields cleared immediately — cheap,
     * non-blocking, so the UI/state layer updates synchronously as before —
     * and the actual disposal is offloaded onto [io], entirely outside the
     * monitor.
     */
    private fun teardownMedia(s: Session) {
        s.statsJob?.cancel()
        _localVideo.value = null
        _remoteVideo.value = null

        val capturerToDispose = s.capturer
        val videoTrackToDispose = s.videoTrack
        val videoSourceToDispose = s.videoSource
        val surfaceHelperToDispose = s.surfaceHelper
        val audioTrackToDispose = s.audioTrack
        val audioSourceToDispose = s.audioSource
        val pcToDispose = s.pc

        s.pc = null
        s.capturer = null
        s.videoTrack = null
        s.videoSource = null
        s.surfaceHelper = null
        s.audioTrack = null
        s.audioSource = null

        endAudioRouting()
        appContext?.let { CallService.stop(it) }
        _muted.value = false
        _cameraOn.value = true
        _connectionQuality.value = CallQuality.UNKNOWN
        _audioRoute.value = AudioRoute.EARPIECE
        _availableRoutes.value = listOf(AudioRoute.EARPIECE, AudioRoute.SPEAKER)
        _sasEmojis.value = null

        io.launch {
            runCatching { capturerToDispose?.stopCapture() }.onFailure { Log.w(TAG, "stopCapture failed", it) }
            runCatching { capturerToDispose?.dispose() }
            runCatching { videoTrackToDispose?.dispose() }
            runCatching { videoSourceToDispose?.dispose() }
            runCatching { surfaceHelperToDispose?.dispose() }
            runCatching { audioTrackToDispose?.dispose() }
            runCatching { audioSourceToDispose?.dispose() }
            runCatching { pcToDispose?.dispose() }
        }
    }

    // ── Audio routing ─────────────────────────────────────────────────────────

    private var audioFocus: AudioFocusRequest? = null
    private var priorAudioMode = AudioManager.MODE_NORMAL
    private var audioDeviceCallback: AudioDeviceCallback? = null

    /**
     * Enter communication audio mode, start watching for Bluetooth/wired
     * headsets connecting or disconnecting, and pick the first route: a
     * present headset wins, otherwise speaker for video / earpiece for voice.
     */
    private fun beginAudioRouting(video: Boolean) {
        val am = appContext?.getSystemService(Context.AUDIO_SERVICE) as? AudioManager ?: return
        priorAudioMode = am.mode
        val focus = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
            .setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                    .build(),
            )
            .build()
        audioFocus = focus
        am.requestAudioFocus(focus)
        am.mode = AudioManager.MODE_IN_COMMUNICATION

        val callback = object : AudioDeviceCallback() {
            override fun onAudioDevicesAdded(addedDevices: Array<out AudioDeviceInfo>) = refreshAndMaybeSwitchRoute(am)
            override fun onAudioDevicesRemoved(removedDevices: Array<out AudioDeviceInfo>) = refreshAndMaybeSwitchRoute(am)
        }
        audioDeviceCallback = callback
        am.registerAudioDeviceCallback(callback, null)

        refreshAvailableRoutes(am)
        val avail = _availableRoutes.value
        val initial = when {
            AudioRoute.BLUETOOTH in avail -> AudioRoute.BLUETOOTH
            AudioRoute.WIRED in avail -> AudioRoute.WIRED
            video -> AudioRoute.SPEAKER
            else -> AudioRoute.EARPIECE
        }
        setAudioRoute(initial)
    }

    /** Re-scan connected devices; switch route if a headset just appeared/vanished. */
    @Synchronized
    private fun refreshAndMaybeSwitchRoute(am: AudioManager) {
        if (session == null) return
        val hadBluetooth = AudioRoute.BLUETOOTH in _availableRoutes.value
        val hadWired = AudioRoute.WIRED in _availableRoutes.value
        refreshAvailableRoutes(am)
        val avail = _availableRoutes.value

        if (_audioRoute.value !in avail) {
            // The route we were on just disconnected — fall back sensibly.
            val fallback = when {
                AudioRoute.BLUETOOTH in avail -> AudioRoute.BLUETOOTH
                AudioRoute.WIRED in avail -> AudioRoute.WIRED
                else -> AudioRoute.EARPIECE
            }
            setAudioRoute(fallback)
        } else if (AudioRoute.BLUETOOTH in avail && !hadBluetooth) {
            setAudioRoute(AudioRoute.BLUETOOTH) // a headset just connected — prefer it
        } else if (AudioRoute.WIRED in avail && !hadWired && AudioRoute.BLUETOOTH !in avail) {
            setAudioRoute(AudioRoute.WIRED)
        }
    }

    /** Rebuild [availableRoutes] from the platform's current output devices. */
    private fun refreshAvailableRoutes(am: AudioManager) {
        val routes = linkedSetOf(AudioRoute.EARPIECE, AudioRoute.SPEAKER)
        val types = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            am.availableCommunicationDevices.map { it.type }
        } else {
            legacyOutputDeviceTypes(am)
        }
        for (type in types) {
            when (type) {
                AudioDeviceInfo.TYPE_BLUETOOTH_SCO, AudioDeviceInfo.TYPE_BLUETOOTH_A2DP -> routes.add(AudioRoute.BLUETOOTH)
                AudioDeviceInfo.TYPE_WIRED_HEADSET, AudioDeviceInfo.TYPE_WIRED_HEADPHONES -> routes.add(AudioRoute.WIRED)
            }
        }
        _availableRoutes.value = routes.toList()
    }

    /** `getDevices` is the pre-31 way to enumerate output devices (not deprecated — [Build.VERSION_CODES.S]'s
     * `availableCommunicationDevices` above is simply more specific, not a replacement for this general API). */
    private fun legacyOutputDeviceTypes(am: AudioManager): List<Int> =
        am.getDevices(AudioManager.GET_DEVICES_OUTPUTS).map { it.type }

    /** Restore normal audio mode, stop watching devices, and drop focus once the call is over. */
    private fun endAudioRouting() {
        val am = appContext?.getSystemService(Context.AUDIO_SERVICE) as? AudioManager ?: return
        audioDeviceCallback?.let { am.unregisterAudioDeviceCallback(it) }
        audioDeviceCallback = null
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            am.clearCommunicationDevice()
        } else {
            applyAudioRouteLegacy(am, AudioRoute.EARPIECE)
        }
        am.mode = priorAudioMode
        audioFocus?.let { am.abandonAudioFocusRequest(it) }
        audioFocus = null
    }

    // ── WebRTC bootstrap (idempotent) ─────────────────────────────────────────

    private fun ensureFactory(context: Context) {
        if (factory != null) return
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
    }

    // ── Helpers ────────────────────────────────────────────────────────────────

    private fun sendSignal(s: Session, signal: CallSignal) {
        io.launch {
            runCatching { ComradeCore.sendCallSignalTyped(s.peer, s.callId, s.media, signal) }
                .onFailure { Log.w(TAG, "sendCallSignal(${signal.kind()}) failed", it) }
        }
    }

    /**
     * Send a signal whose delivery is essential to the call (the offer/answer).
     * Unlike [sendSignal], a failure here ends the call with "Couldn't connect"
     * rather than leaving the UI waiting forever for a reply that can't come —
     * e.g. no relay connection, or a locked vault.
     */
    private fun sendSignalOrFail(s: Session, signal: CallSignal) {
        io.launch {
            val ok = runCatching { ComradeCore.sendCallSignalTyped(s.peer, s.callId, s.media, signal) }
                .onFailure { Log.w(TAG, "sendCallSignal(${signal.kind()}) failed", it) }
                .isSuccess
            if (!ok) {
                synchronized(this@CallManager) {
                    if (session === s && !s.ended) endWith(HangupReason.FAILED, "failed", sendHangup = false)
                }
            }
        }
    }

    /**
     * Arm (replacing any previous) a one-shot timeout for the current call: after
     * [ms], if this session is still current and has not connected, end it with
     * [reason]/[outcome]. Cancelled on connect and on teardown. This is what keeps
     * an unanswered or never-connecting call from hanging on "Calling…".
     */
    private fun armTimeout(s: Session, ms: Long, reason: HangupReason, outcome: String) {
        s.timeoutJob?.cancel()
        s.timeoutJob = io.launch {
            delay(ms)
            synchronized(this@CallManager) {
                if (session === s && !s.ended && s.connectedAtMs == 0L) {
                    Log.w(TAG, "${ms}ms timeout fired without connecting; ending as \"$outcome\", callId=${s.callId}")
                    endWith(reason, outcome, sendHangup = true)
                }
            }
        }
    }

    private fun mediaConstraints(video: Boolean) = MediaConstraints().apply {
        mandatory.add(MediaConstraints.KeyValuePair("OfferToReceiveAudio", "true"))
        mandatory.add(MediaConstraints.KeyValuePair("OfferToReceiveVideo", if (video) "true" else "false"))
    }

    /**
     * Builds the [IceCandidate] WebRTC expects from a remote [CallSignal.Ice]
     * payload. The wire `sdpMid` can legitimately be absent; passing a null
     * string straight into the native constructor is a fatal JNI
     * `JavaToStdString` crash, so a missing value maps to `""` instead.
     * `internal` (not `private`) so it's directly unit-testable.
     */
    internal fun remoteIceCandidate(ice: CallSignal.Ice): IceCandidate =
        IceCandidate(ice.sdpMid ?: "", ice.sdpMLineIndex?.toInt() ?: 0, ice.candidate)

    /**
     * WebRTC reports `sdpMLineIndex == -1` when a candidate isn't tied to an
     * m-line yet. [UShort] has no negative range, so a bare `.toUShort()`
     * wraps `-1` to `65535`; the remote side then rejects the line index as
     * malformed and the call sticks on "Connecting…" forever. Map negative
     * indices to `null` instead of wrapping them. `internal` (not `private`)
     * so it's directly unit-testable.
     */
    internal fun outgoingSdpMLineIndex(index: Int): UShort? = if (index >= 0) index.toUShort() else null

    /** An [SdpObserver] whose only interesting callback is create-success. */
    private fun createSdpObserver(s: Session, onCreated: (SessionDescription) -> Unit) = object : SdpObserver {
        override fun onCreateSuccess(desc: SessionDescription) = onCreated(desc)
        override fun onSetSuccess() {}
        override fun onCreateFailure(error: String?) {
            Log.e(TAG, "createOffer/Answer failed: $error")
            synchronized(this@CallManager) {
                if (session === s) endWith(HangupReason.FAILED, "failed", sendHangup = true)
            }
        }
        override fun onSetFailure(error: String?) {}
    }

    private fun setLocalThen(s: Session, desc: SessionDescription, onDone: () -> Unit) {
        s.pc?.setLocalDescription(
            object : SdpObserver {
                override fun onCreateSuccess(p0: SessionDescription?) {}
                override fun onSetSuccess() {
                    synchronized(this@CallManager) {
                        if (session !== s) return
                        // The offer for the caller, the answer for the callee — and
                        // whichever of the two on a renegotiation, overwriting the
                        // prior value so it can never go stale (see [Session.localSdp]).
                        s.localSdp = desc.description
                        onDone()
                    }
                }
                override fun onCreateFailure(p0: String?) {}
                override fun onSetFailure(error: String?) {
                    Log.e(TAG, "setLocalDescription failed: $error")
                }
            },
            desc,
        )
    }

    private fun setRemoteThen(s: Session, desc: SessionDescription, onDone: () -> Unit) {
        s.pc?.setRemoteDescription(
            object : SdpObserver {
                override fun onCreateSuccess(p0: SessionDescription?) {}
                override fun onSetSuccess() {
                    synchronized(this@CallManager) {
                        if (session !== s) return
                        s.remoteSet = true
                        // The answer for the caller, the offer for the callee — and
                        // whichever of the two on a renegotiation, overwriting the
                        // prior value so it can never go stale (see [Session.remoteSdp]).
                        s.remoteSdp = desc.description
                        flushPendingIce(s)
                        onDone()
                    }
                }
                override fun onCreateFailure(p0: String?) {}
                override fun onSetFailure(error: String?) {
                    Log.e(TAG, "setRemoteDescription failed: $error")
                    synchronized(this@CallManager) {
                        if (session === s) endWith(HangupReason.FAILED, "failed", sendHangup = true)
                    }
                }
            },
            desc,
        )
    }

    private fun outcomeForRemoteHangup(reason: HangupReason, connected: Boolean): String = when (reason) {
        HangupReason.DECLINED -> "declined"
        HangupReason.BUSY -> "busy"
        HangupReason.MISSED -> "missed"
        HangupReason.FAILED -> "failed"
        else -> if (connected) "connected" else "missed"
    }

    private fun CallSignal.kind(): String = when (this) {
        is CallSignal.Offer -> "offer"
        is CallSignal.Answer -> "answer"
        is CallSignal.Ice -> "ice"
        CallSignal.Ringing -> "ringing"
        CallSignal.Busy -> "busy"
        is CallSignal.Hangup -> "hangup"
    }

    private fun ComradeCore.IceServerInfo.toWebRtc(): PeerConnection.IceServer =
        PeerConnection.IceServer.builder(urls)
            .apply {
                username?.let { setUsername(it) }
                credential?.let { setPassword(it) }
            }
            .createIceServer()

    private fun mediaKindOf(media: String): CallMediaKind =
        if (media.equals("video", ignoreCase = true)) CallMediaKind.VIDEO else CallMediaKind.AUDIO

    // ── PeerConnection.Observer ────────────────────────────────────────────────

    private fun peerObserver(s: Session): PeerConnection.Observer = object : PeerConnection.Observer {
        override fun onIceCandidate(candidate: IceCandidate) {
            Log.i(TAG, "local ICE candidate (${candidate.sdp.iceCandidateType()}), callId=${s.callId}")
            sendSignal(
                s,
                CallSignal.Ice(
                    candidate = candidate.sdp,
                    sdpMid = candidate.sdpMid,
                    sdpMLineIndex = outgoingSdpMLineIndex(candidate.sdpMLineIndex),
                ),
            )
        }

        override fun onAddTrack(receiver: RtpReceiver, mediaStreams: Array<out MediaStream>) {
            (receiver.track() as? VideoTrack)?.let { track ->
                track.setEnabled(true)
                _remoteVideo.value = track
            }
        }

        override fun onConnectionChange(newState: PeerConnection.PeerConnectionState) {
            Log.i(TAG, "peerConnectionState → $newState, callId=${s.callId}")
            when (newState) {
                PeerConnection.PeerConnectionState.CONNECTED ->
                    synchronized(this@CallManager) { if (session === s) onConnected(s) }
                PeerConnection.PeerConnectionState.FAILED ->
                    synchronized(this@CallManager) { if (session === s) tryTurnFallbackOrFail(s) }
                else -> Unit // DISCONNECTED can be transient (ICE restart) — don't tear down.
            }
        }

        // Diagnostic only — the state machine above reacts to onConnectionChange,
        // not these; logged so a "stuck at Connecting" report is a logcat read
        // (did ICE ever leave CHECKING? did gathering find any srflx/relay
        // candidates at all?) instead of a guess.
        override fun onIceConnectionChange(newState: PeerConnection.IceConnectionState) {
            Log.i(TAG, "iceConnectionState → $newState, callId=${s.callId}")
        }
        override fun onIceGatheringChange(newState: PeerConnection.IceGatheringState) {
            Log.i(TAG, "iceGatheringState → $newState, callId=${s.callId}")
        }

        // Unused observer surface — required by the interface.
        override fun onSignalingChange(newState: PeerConnection.SignalingState) {}
        override fun onIceConnectionReceivingChange(receiving: Boolean) {}
        override fun onIceCandidatesRemoved(candidates: Array<out IceCandidate>) {}
        override fun onAddStream(stream: MediaStream) {}
        override fun onRemoveStream(stream: MediaStream) {}
        override fun onDataChannel(channel: org.webrtc.DataChannel) {}
        override fun onRenegotiationNeeded() {}
    }

    /** Best-effort `typ host|srflx|relay` extraction from a candidate's SDP line, for logging only. */
    private fun String.iceCandidateType(): String =
        Regex("""\btyp (\w+)""").find(this)?.groupValues?.get(1) ?: "unknown"
}
