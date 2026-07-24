package mullu.comrade.call

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.media.AudioAttributes
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.os.Build
import android.util.Log
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import mullu.comrade.ComradeCore
import mullu.comrade.voice.MicHolder
import mullu.comrade.voice.WakeWordService
import org.webrtc.AudioSource
import org.webrtc.AudioTrack
import org.webrtc.Camera2Enumerator
import org.webrtc.CameraVideoCapturer
import org.webrtc.DataChannel
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
 *
 * One hard rule inside that scheme: a callback delivered *on* a WebRTC thread
 * must never take this object's monitor inline — it hops to [webRtcLane]
 * first. See that field for the deadlock this prevents.
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
     * Recovery window for a call that was already [CallUiState.Active] and
     * then lost its media path (ICE `FAILED`, or `DISCONNECTED` for longer
     * than [DISCONNECT_GRACE_MS]): if it hasn't reported `CONNECTED` again
     * within this long, end it honestly instead of leaving it "Active" with
     * dead audio/video forever — the callee in particular has no TURN-retry
     * mechanism of its own (only the caller re-offers), so without this it
     * waits forever. See [armRecoveryTimeout].
     */
    private const val RECOVERY_TIMEOUT_MS = 20_000L

    /**
     * How long a mid-call `DISCONNECTED` is tolerated as transient (e.g. a
     * brief network blip, or the caller's own ICE restart in progress) before
     * [armRecoveryTimeout] is armed for it, same as a `FAILED`.
     */
    private const val DISCONNECT_GRACE_MS = 15_000L

    /**
     * Bound on [endedCallIds] — comfortably above how many calls could
     * plausibly overlap the 2-day relay backfill window in practice.
     */
    private const val ENDED_CALL_IDS_CAP = 32

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

    /**
     * Ordered, single-lane dispatcher for WebRTC callbacks that need this
     * object's monitor.
     *
     * `org.webrtc` delivers [PeerConnection.Observer]/[SdpObserver]/stats
     * callbacks on its internal signaling thread — the same thread every
     * blocking `PeerConnection` proxy method (`addIceCandidate`,
     * `setConfiguration`, `signalingState`, `addTrack`, …) synchronously
     * waits on. If a callback `synchronized`s on this object *inline* while
     * another thread already holds the monitor and is inside one of those
     * blocking proxy calls (the event pump does exactly that:
     * [onIncomingSignal] is `@Synchronized` and [addRemoteIce] calls
     * `addIceCandidate`), the two park on each other forever: the signaling
     * thread waits for the monitor, the monitor holder waits for the
     * signaling thread. From that moment *every* `synchronized` transition —
     * [hangup], [reject], the armed ring/connect timeouts — blocks for good:
     * the "stuck on Connecting… and End call does nothing" freeze, hitting
     * the callee hardest because its pump applies the caller's trickled ICE
     * candidates right while its own peer connection is firing
     * CONNECTING/FAILED state changes.
     *
     * The invariant that breaks the cycle: a WebRTC-thread callback never
     * takes the monitor inline — it hops here instead, so the signaling
     * thread is always free and every blocking proxy call made under the
     * monitor stays bounded. A single lane (not the whole [io] pool) so the
     * hopped work preserves the signaling thread's delivery order — an SDP
     * `onSetSuccess` still lands before the `onConnectionChange(CONNECTED)`
     * that follows it.
     */
    @OptIn(ExperimentalCoroutinesApi::class)
    private val webRtcLane = Dispatchers.IO.limitedParallelism(1)

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
    // @Volatile: written under [factoryLock] (see ensureFactory) but read from
    // other threads without it — the [eglBaseContext] getter on the UI thread,
    // and [factory]/[appContext] on the call-setup coroutine — so their
    // initialised values must be visible across threads.
    @Volatile private var appContext: Context? = null
    @Volatile private var eglBase: EglBase? = null
    @Volatile private var factory: PeerConnectionFactory? = null

    /**
     * Serializes [ensureFactory]'s one-time native init on a lock that is
     * **not** this object's own monitor. The init is a synchronous, multi-second
     * `PeerConnectionFactory.initialize` + `EglBase.create()` +
     * `createPeerConnectionFactory()` build; every user-facing call control
     * ([accept], [hangup], [reject], [toggleMute], …) is `@Synchronized` on
     * `this`. When init held the shared monitor, a hang-up tapped on a still
     * "Connecting…" screen blocked the **main thread** for the entire init →
     * "Comrade isn't responding" (ANR). A dedicated lock preserves the
     * double-initialisation guard while leaving the main-thread monitor free.
     */
    private val factoryLock = Any()

    /** The shared EGL context renderers must init against (null until a call runs). */
    val eglBaseContext: EglBase.Context? get() = eglBase?.eglBaseContext

    private var session: Session? = null

    /**
     * Bounded (cap [ENDED_CALL_IDS_CAP]) memory of recently-ended call ids —
     * lets [onIncomingSignal] silently drop a redelivered `Offer` for a call
     * we already tore down (relay at-least-once delivery, or the 2-day inbox
     * backfill re-scanning on every launch) instead of ringing again.
     * Recorded in [endWith]; oldest evicted first once at capacity.
     */
    private val endedCallIds = ArrayDeque<String>()

    /** Record `callId` as ended — see [endedCallIds]. A blank/provisional id (never placed) is not worth remembering. */
    private fun rememberEnded(callId: String) {
        if (callId.isEmpty()) return
        endedCallIds.addLast(callId)
        while (endedCallIds.size > ENDED_CALL_IDS_CAP) endedCallIds.removeFirst()
    }

    /** Everything mutable about the one in-flight call. */
    private class Session(
        /**
         * Empty until [ComradeCore.placeCallTyped] mints the real id (caller
         * side, provisional phase only — see [startOutgoingCall]); `var`,
         * not `val`, so the same [Session] object can be filled in in place
         * rather than replaced once the id is known. An incoming session
         * always constructs with the real id already known.
         */
        var callId: String,
        val peer: String,
        /** Display title; starts as the short key and is upgraded to the alias/@handle off the monitor (see [upgradePeerLabel]). */
        var peerLabel: String,
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

        /**
         * Caller side only: the coroutine running [ComradeCore.placeCallTyped]
         * and (once it resolves) building the peer connection and sending the
         * offer. Cancelled by [hangup] so a cancel-before-placed never lets a
         * late continuation send an offer after the UI has already gone back
         * to [CallUiState.Idle] (AUDIT.md COMMS-05).
         */
        var placingJob: Job? = null

        /** The connection-quality stats-polling loop, started on connect and cancelled on teardown. */
        var statsJob: Job? = null

        /** Caller side: the callee's device has acked with a `Ringing` signal. */
        var remoteRinging = false

        /**
         * Whether a post-connect recovery countdown is currently armed — see
         * [armRecoveryTimeout]. Checked (not just `recoveryJob`'s cancellation)
         * inside that countdown's own body, mirroring how [timeoutJob] guards
         * itself with `connectedAtMs == 0L`: a reconnect and the countdown
         * firing can race to acquire this object's monitor, and this flag is
         * what's still correct by the time the loser gets it.
         */
        var recovering = false

        /** The pending post-connect recovery timeout, cancelled on reconnect or teardown. */
        var recoveryJob: Job? = null
    }

    // ── Public API: outgoing ─────────────────────────────────────────────────

    /**
     * Place a call to [peer]. Runs the STUN-only first attempt the core's design
     * intends: [ComradeCore.placeCallTyped] returns the minted call id and a
     * STUN-only ICE list, we build the peer connection, and send the `Offer`.
     *
     * Permissions (mic, + camera for video) must already be granted — the UI
     * gates on that before calling in.
     *
     * ## Provisional session (AUDIT.md COMMS-05)
     * The [Session] is created **synchronously, here** — before the async
     * [ComradeCore.placeCallTyped] request even begins — rather than only
     * once it resolves. That used to be a real bug: [session] stayed `null`
     * for the whole `placeCallTyped` round-trip, so [hangup] called during
     * that window found nothing to act on (`session ?: return`) and silently
     * no-opped, leaving the UI stuck on "Calling…" — and the *later*
     * continuation, seeing `session == null` (because the no-op hangup never
     * set anything), proceeded to build the peer connection and send the
     * offer anyway, after the user had already tried to cancel. Creating the
     * provisional session up front means [hangup] always finds a session to
     * mark [Session.ended] on and a [Session.placingJob] to cancel, and the
     * continuation below re-checks both before doing anything observable.
     *
     * A ring/connect timeout is armed immediately too (not only once the
     * offer is sent), so a slow or hanging `placeCallTyped` call can't leave
     * the UI on "Calling…" with no timeout backstop either.
     *
     * ## Incoming offer during the provisional window
     * The provisional session already occupies [session], so
     * [handleRemoteOffer] treats a fresh incoming offer exactly like it would
     * during an established call: not the same `callId` and no live `pc` yet
     * ⇒ "already busy", auto-rejected with [CallSignal.Busy]. This is the
     * documented policy — an outgoing call in flight always wins over a
     * simultaneous incoming one, the same as after it connects.
     */
    @Synchronized
    fun startOutgoingCall(context: Context, peer: String, peerLabel: String, media: CallMediaKind) {
        if (session != null) {
            Log.w(TAG, "startOutgoingCall ignored: a call is already in progress")
            return
        }
        val appCtx = context.applicationContext
        val s = Session(callId = "", peer = peer, peerLabel = peerLabel, media = media, incoming = false)
        session = s
        // Optimistic ringing state so the UI opens immediately; the factory
        // init, placeCall, and offer all happen on IO — ensureFactory is a
        // synchronous native init (PeerConnectionFactory.initialize +
        // EglBase.create()) that has no business running on the caller's
        // (Compose click) thread, and placeCall touches the store and the
        // signal send is a blocking DM round-trip.
        _state.value = CallUiState.Ringing(peer, peerLabel, media == CallMediaKind.VIDEO, incoming = false)
        // Give up with "No answer" if the callee never picks up — armed now,
        // not after the offer sends, so a slow placeCall is covered too.
        armTimeout(s, RING_TIMEOUT_MS, HangupReason.MISSED, "missed")
        s.placingJob = io.launch {
            ensureFactory(appCtx)
            val placed = runCatching { ComradeCore.placeCallTyped(peer, media) }
                .getOrElse {
                    Log.e(TAG, "placeCall failed", it)
                    synchronized(this@CallManager) {
                        if (session === s && !s.ended) endWith(HangupReason.FAILED, "failed", sendHangup = false)
                    }
                    return@launch
                }
            synchronized(this@CallManager) {
                // Cancelled (hangup) while placeCallTyped was in flight — never
                // build a peer connection or send an offer after that.
                if (session !== s || s.ended) return@launch
                s.callId = placed.callId
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
        if (dto.signal is CallSignal.Offer && isOfferForEndedCall(dto.callId, endedCallIds)) {
            // The peer already received a terminal Hangup/Busy for this call
            // id — a redelivered Offer (at-least-once relay delivery, or the
            // 2-day backfill re-scanning on every launch) must not ring again.
            Log.i(TAG, "dropping offer for already-ended callId=${dto.callId}")
            return false
        }
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
        val appCtx = context.applicationContext
        _state.value = CallUiState.Connecting(s.peer, s.peerLabel, s.isVideo, incoming = true)
        io.launch {
            // Synchronous native init (PeerConnectionFactory.initialize +
            // EglBase.create()) — off the caller's (Compose click) thread,
            // same as startOutgoingCall. The optimistic Connecting state
            // above is already set, so the UI doesn't wait on this either.
            ensureFactory(appCtx)
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

    /**
     * Reject the ringing incoming call (callee declines before answering).
     *
     * Dispatched to [io] instead of taking the monitor on the caller's
     * (main/UI) thread: `setupPeer`/`ensureFactory` hold the monitor across
     * blocking native work, and a control tapped on the main thread while that
     * runs would block the UI thread for its whole duration — the "Comrade
     * isn't responding" ANR. Running the transition on [io] means the main
     * thread only ever *launches* it and returns immediately.
     */
    fun reject() {
        io.launch {
            synchronized(this@CallManager) {
                if (session == null) return@launch
                endWith(HangupReason.DECLINED, "declined", sendHangup = true)
            }
        }
    }

    /**
     * Hang up / cancel the current call from the local UI.
     *
     * A call cancelled before [ComradeCore.placeCallTyped] has even returned
     * (`!s.incoming && s.pc == null` — the provisional window [startOutgoingCall]
     * documents) has no offer on the wire yet to hang up: there is nothing for
     * a [CallSignal.Hangup] to reach, so this ends the call locally only and
     * cancels the in-flight [Session.placingJob] instead, which is what
     * actually stops a late offer from being sent (see [endWith]).
     *
     * Dispatched to [io] (see [reject] for why) so the main thread never
     * blocks on the monitor a running `setupPeer` may be holding.
     */
    fun hangup() {
        io.launch {
            synchronized(this@CallManager) {
                val s = session ?: return@launch
                val connected = s.connectedAtMs > 0
                val stillPlacing = !s.incoming && s.pc == null
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
                endWith(reason, outcome, sendHangup = !stillPlacing)
            }
        }
    }

    // ── Toggles ──────────────────────────────────────────────────────────────

    /** Flip local mic enablement (no renegotiation — just [AudioTrack.setEnabled]). Dispatched to [io], see [reject]. */
    fun toggleMute() {
        io.launch {
            synchronized(this@CallManager) {
                val s = session ?: return@launch
                val next = !_muted.value
                s.audioTrack?.setEnabled(!next)
                _muted.value = next
            }
        }
    }

    /**
     * Turn the local camera off/on mid-call (video calls only) — no
     * renegotiation, matching [toggleMute]. Turning off both disables the
     * track (so the peer, and the local self-preview, stop receiving frames)
     * and stops the capturer (releasing the physical camera, not just muting
     * it); turning back on resumes both. A no-op for audio calls.
     */
    fun toggleCamera() {
        // Dispatched to [io] (see [reject]); startCapture/stopCapture are
        // blocking native calls that must not run on the main thread either.
        io.launch {
            synchronized(this@CallManager) {
                val s = session ?: return@launch
                if (!s.isVideo) return@launch
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
        }
    }

    /** Cycle to the next available [AudioRoute] (earpiece → speaker → Bluetooth/wired → …). */
    @Synchronized
    fun cycleAudioRoute() {
        val avail = _availableRoutes.value
        if (avail.isEmpty()) return
        val idx = avail.indexOf(_audioRoute.value).coerceAtLeast(0)
        setAudioRoute(avail[(idx + 1) % avail.size])
    }

    /**
     * Explicitly route in-call audio to [route], if it's currently in
     * [availableRoutes]. [_audioRoute] is only updated when the platform
     * actually accepted the switch — a route that failed to apply (device
     * vanished mid-tap, or Bluetooth without `BLUETOOTH_CONNECT`) must not
     * leave the UI claiming audio is somewhere it isn't.
     */
    @Synchronized
    fun setAudioRoute(route: AudioRoute) {
        if (route !in _availableRoutes.value) return
        val am = appContext?.getSystemService(Context.AUDIO_SERVICE) as? AudioManager ?: return
        if (applyAudioRoute(am, route)) {
            _audioRoute.value = route
        }
    }

    /**
     * Every [AudioDeviceInfo] type that counts as [route] — shared by
     * availability scanning ([refreshAvailableRoutes]) and actual routing
     * ([applyAudioRoute]) so they can never disagree. Notably WIRED includes
     * the USB types (a USB-C headset is what "wired earphones" *is* on a
     * jack-less phone) and BLUETOOTH includes the API 31+ LE Audio types —
     * before this, a plugged USB-C headset was never offered and call setup
     * actively routed audio away from it to the earpiece.
     */
    private fun routeDeviceTypes(route: AudioRoute): Set<Int> = when (route) {
        AudioRoute.EARPIECE -> setOf(AudioDeviceInfo.TYPE_BUILTIN_EARPIECE)
        AudioRoute.SPEAKER -> setOf(AudioDeviceInfo.TYPE_BUILTIN_SPEAKER)
        AudioRoute.BLUETOOTH -> buildSet {
            add(AudioDeviceInfo.TYPE_BLUETOOTH_SCO)
            add(AudioDeviceInfo.TYPE_BLUETOOTH_A2DP)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                add(AudioDeviceInfo.TYPE_BLE_HEADSET)
                add(AudioDeviceInfo.TYPE_BLE_SPEAKER)
            }
        }
        AudioRoute.WIRED -> setOf(
            AudioDeviceInfo.TYPE_WIRED_HEADSET,
            AudioDeviceInfo.TYPE_WIRED_HEADPHONES,
            AudioDeviceInfo.TYPE_USB_HEADSET,
            AudioDeviceInfo.TYPE_USB_DEVICE,
        )
    }

    /**
     * Routing a call to Bluetooth needs `BLUETOOTH_CONNECT` on API 31+. The
     * in-call UI requests it when the user *taps* the Bluetooth route; the
     * automatic picks (call start, headset-connected callback) must instead
     * check first — silently failing `setCommunicationDevice` used to leave
     * the state claiming Bluetooth while audio played nowhere the user could
     * hear it.
     */
    private fun canRouteBluetooth(): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return true
        val ctx = appContext ?: return false
        return ContextCompat.checkSelfPermission(ctx, Manifest.permission.BLUETOOTH_CONNECT) ==
            PackageManager.PERMISSION_GRANTED
    }

    /**
     * Apply [route] to the platform; returns whether it actually took. API
     * 31+ has a purpose-built API for exactly this (`setCommunicationDevice`);
     * below that, routing is the older speakerphone flag plus manual
     * Bluetooth SCO start/stop.
     */
    private fun applyAudioRoute(am: AudioManager, route: AudioRoute): Boolean {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            val wantedTypes = routeDeviceTypes(route)
            val device = am.availableCommunicationDevices.firstOrNull { it.type in wantedTypes }
            if (device == null) {
                // No matching communication device: only the earpiece request
                // legitimately maps to "the platform default".
                am.clearCommunicationDevice()
                return route == AudioRoute.EARPIECE
            }
            return try {
                am.setCommunicationDevice(device)
            } catch (e: SecurityException) {
                // Routing to a Bluetooth device needs BLUETOOTH_CONNECT on
                // API 31+; without it, fall back to the default route
                // instead of crashing the call.
                Log.w(TAG, "missing BLUETOOTH_CONNECT for setCommunicationDevice; clearing route", e)
                am.clearCommunicationDevice()
                false
            }
        }
        applyAudioRouteLegacy(am, route)
        return true
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

    /** Flip between the front and back cameras (video calls only). Dispatched to [io], see [reject]. */
    fun switchCamera() {
        io.launch {
            synchronized(this@CallManager) {
                (session?.capturer as? CameraVideoCapturer)?.switchCamera(null)
            }
        }
    }

    // ── TURN connectivity diagnostic (AUDIT.md COMMS-02) ─────────────────────

    /** An honest, support-screen-ready read on TURN relay reachability. */
    enum class TurnDiagnostic {
        /** No TURN server is configured at all — nothing to test. */
        NO_SERVER_CONFIGURED,

        /** A `relay`-typed local ICE candidate was gathered — the relay is reachable. */
        RELAY_AVAILABLE,

        /** A server is configured but no relay candidate came back in time. */
        RELAY_UNAVAILABLE,
    }

    /**
     * Best-effort "is the configured TURN relay actually reachable" check for
     * a settings/support screen — gathers ICE candidates against *only* the
     * configured TURN server (`iceTransportsType = RELAY`, no STUN, no
     * signaling, no remote peer, never touches [session]) and reports
     * whether a `relay`-typed local candidate came back within [timeoutMs].
     * Safe to call at any time, including mid-call, since it never touches
     * the live session — it allocates its own throwaway [PeerConnection] and
     * always disposes it before returning.
     */
    suspend fun testTurnConnectivity(context: Context, timeoutMs: Long = 8_000L): TurnDiagnostic {
        val turnConfigured = runCatching { ComradeCore.turnServerStatusTyped().configured }.getOrDefault(false)
        if (!turnConfigured) return TurnDiagnostic.NO_SERVER_CONFIGURED

        val iceServers = runCatching { ComradeCore.callIceServersForTyped(IceStrategy.STUN_AND_TURN) }
            .getOrDefault(emptyList())
            .filter { it.username != null } // the TURN entry only — nothing to "test" about public STUN
            .map { it.toWebRtc() }
        if (iceServers.isEmpty()) return TurnDiagnostic.NO_SERVER_CONFIGURED

        ensureFactory(context)
        val fac = factory ?: return TurnDiagnostic.RELAY_UNAVAILABLE
        val relayFound = CompletableDeferred<Boolean>()
        val config = rtcConfig(iceServers).apply { iceTransportsType = PeerConnection.IceTransportsType.RELAY }
        val observer = object : PeerConnection.Observer {
            override fun onIceCandidate(candidate: IceCandidate) {
                if (candidate.sdp.iceCandidateType() == "relay") relayFound.complete(true)
            }
            override fun onIceGatheringChange(newState: PeerConnection.IceGatheringState) {
                if (newState == PeerConnection.IceGatheringState.COMPLETE) relayFound.complete(false)
            }
            // Unused observer surface — required by the interface (mirrors peerObserver's).
            override fun onSignalingChange(newState: PeerConnection.SignalingState) {}
            override fun onIceConnectionChange(newState: PeerConnection.IceConnectionState) {}
            override fun onIceConnectionReceivingChange(receiving: Boolean) {}
            override fun onIceCandidatesRemoved(candidates: Array<out IceCandidate>) {}
            override fun onAddStream(stream: MediaStream) {}
            override fun onRemoveStream(stream: MediaStream) {}
            override fun onDataChannel(channel: DataChannel) {}
            override fun onRenegotiationNeeded() {}
            override fun onAddTrack(receiver: RtpReceiver, mediaStreams: Array<out MediaStream>) {}
            override fun onConnectionChange(newState: PeerConnection.PeerConnectionState) {}
        }
        val pc = fac.createPeerConnection(config, observer)
        if (pc == null) {
            return TurnDiagnostic.RELAY_UNAVAILABLE
        }
        // ICE gathering needs at least one m-line to negotiate; a bare data
        // channel is enough and never touches the microphone/camera.
        pc.createDataChannel("turn-connectivity-test", DataChannel.Init())
        pc.createOffer(
            object : SdpObserver {
                override fun onCreateSuccess(desc: SessionDescription) {
                    pc.setLocalDescription(NoopSdpObserver, desc)
                }
                override fun onSetSuccess() {}
                override fun onCreateFailure(error: String?) {
                    Log.w(TAG, "testTurnConnectivity: createOffer failed: $error")
                    relayFound.complete(false)
                }
                override fun onSetFailure(error: String?) {}
            },
            MediaConstraints(),
        )
        val result = withTimeoutOrNull(timeoutMs) { relayFound.await() } ?: false
        withContext(Dispatchers.IO) {
            runCatching { pc.close() }
            runCatching { pc.dispose() }
        }
        return if (result) TurnDiagnostic.RELAY_AVAILABLE else TurnDiagnostic.RELAY_UNAVAILABLE
    }

    /** An [SdpObserver] that ignores every callback — for a `setLocalDescription` this diagnostic doesn't need to react to. */
    private object NoopSdpObserver : SdpObserver {
        override fun onCreateSuccess(desc: SessionDescription?) {}
        override fun onSetSuccess() {}
        override fun onCreateFailure(error: String?) {}
        override fun onSetFailure(error: String?) {}
    }

    // ── Peer connection setup ─────────────────────────────────────────────────

    /**
     * Test-only override: when set, every [rtcConfig] this object builds from
     * now on forces [PeerConnection.IceTransportsType.RELAY] — the standard
     * WebRTC mechanism for "only use relay candidates", regardless of what
     * direct/STUN paths actually exist. This is the fixture AUDIT.md
     * COMMS-02/03 asks for to test TURN fallback without needing to actually
     * firewall UDP on a device: an instrumented test (or a hidden developer
     * setting) flips this before placing/accepting a call. Never touched by
     * any production code path.
     */
    @Volatile
    var forceRelayOnly: Boolean = false

    /**
     * Test-only: instrumented state-machine tests (`CallManagerLifecycleTest`)
     * place real provisional calls and cancel them immediately; depending on
     * scheduling the placing continuation can still win the race, reach
     * [setupPeer], and start [CallService] — a real foreground service those
     * tests neither exercise nor can safely host. On a loaded emulator the
     * queued `startForegroundService` → `stopService` pair can trip Android's
     * "did not call startForeground" process kill even though the service
     * goes foreground first thing in its `onCreate`: the *creation itself* is
     * what never gets scheduled inside the contract window. Setting this
     * skips only the CallService start/stop; every other part of call
     * setup/teardown runs unchanged. Never touched by any production code
     * path.
     */
    @Volatile
    var disableCallServiceForTest: Boolean = false

    /**
     * Test-only: a [PeerConnection.Observer] wired to a throwaway, never-current
     * [Session] — lets an instrumented test deliver observer callbacks from its
     * own stand-in "signaling thread" and assert they return promptly even
     * while another thread holds this object's monitor (the deadlock
     * [webRtcLane] exists to prevent). The throwaway session is never installed
     * in [session], so every hopped `session === s` guard no-ops — nothing
     * about live call state is reachable through it.
     */
    internal fun peerConnectionObserverForTest(): PeerConnection.Observer =
        peerObserver(
            Session(
                callId = "monitor-hazard-test",
                peer = "monitor-hazard-test-peer",
                peerLabel = "monitor-hazard-test-peer",
                media = CallMediaKind.AUDIO,
                incoming = true,
            ),
        )

    /** The [PeerConnection.RTCConfiguration] every new/renegotiated peer connection uses. */
    private fun rtcConfig(iceServers: List<PeerConnection.IceServer>) =
        PeerConnection.RTCConfiguration(iceServers).apply {
            sdpSemantics = PeerConnection.SdpSemantics.UNIFIED_PLAN
            continualGatheringPolicy = PeerConnection.ContinualGatheringPolicy.GATHER_CONTINUALLY
            bundlePolicy = PeerConnection.BundlePolicy.MAXBUNDLE
            rtcpMuxPolicy = PeerConnection.RtcpMuxPolicy.REQUIRE
            if (forceRelayOnly) iceTransportsType = PeerConnection.IceTransportsType.RELAY
        }

    /** Build the [PeerConnection] and local tracks. Returns false (and tears down) on failure. */
    private fun setupPeer(s: Session, iceServers: List<PeerConnection.IceServer>): Boolean {
        // Release the wake-word recogniser's hold on the mic first — a call
        // and the always-listening "Hey Comrade" recogniser both wanting
        // AudioSource.MIC is exactly the contention that must not happen.
        // Resumed once the call ends, in endWith.
        WakeWordService.pause(MicHolder.CALL)
        val fac = factory ?: return false
        val config = rtcConfig(iceServers)
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
        if (!disableCallServiceForTest) {
            appContext?.let { ctx ->
                runCatching { CallService.start(ctx, s.peer, s.peerLabel, s.isVideo) }
                    .onFailure { Log.w(TAG, "Failed to start CallService (foreground restrictions)", it) }
            }
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

    /**
     * A fresh offer: renegotiate an existing call, no-op a duplicate, resolve
     * glare (mutual simultaneous calls), ring, or reject as busy — see
     * [decideOfferForExistingSession] for the pure decision this dispatches on.
     */
    private fun handleRemoteOffer(dto: CallSignalDto, sdp: String): Boolean {
        val existing = session
        if (existing == null) return ringFreshIncoming(dto, sdp)

        when (decideOfferForExistingSession(dto.callId, existing.callId, existing.pc != null)) {
            OfferDecision.RENEGOTIATE -> {
                // A re-offer for the current call (e.g. the caller's TURN
                // ICE-restart) is a renegotiation, not a new call — answer it
                // on the existing pc.
                //
                // Pre-connect, this is the caller's rescue attempt after its
                // STUN-only ICE failed: give it a fresh, full connect window
                // instead of whatever remains of the timeout armed at accept
                // (which may be moments from firing by the time ICE has
                // failed and the widened re-offer has travelled here).
                if (existing.connectedAtMs == 0L) {
                    armTimeout(existing, CONNECT_TIMEOUT_MS, HangupReason.FAILED, "failed")
                }
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
            OfferDecision.DUPLICATE_NOOP -> {
                // The same offer redelivered while we're still ringing on it
                // (pre-accept, no pc yet) — at-least-once relay delivery or a
                // backfill re-scan. Drop it silently: re-ringing would only
                // restart the ring timeout for no reason.
                return false
            }
            OfferDecision.BUSY -> {
                if (isGlareCandidate(existing, dto.peer)) {
                    val ourNpub = runCatching { ComradeCore.currentIdentityTyped() }.getOrNull()
                    if (ourNpub != null && decideGlare(ourNpub, dto.peer) == GlareDecision.WE_LOSE_TAKE_INCOMING) {
                        // Mutual simultaneous calls to each other. Deterministic
                        // tiebreak: yield our own outgoing attempt (silently —
                        // no Busy, they already know about this call, they
                        // placed it) and ring their incoming offer instead.
                        Log.i(TAG, "glare with ${dto.peer}: yielding our outgoing call (lower npub wins)")
                        existing.placingJob?.cancel()
                        teardownMedia(existing)
                        session = null
                        return ringFreshIncoming(dto, sdp)
                    }
                    // We win the tiebreak (or couldn't resolve our own
                    // identity) — keep our outgoing call, ignore theirs.
                    return false
                }
                // Otherwise we're already busy on an unrelated call — auto-reject.
                val busyMedia = mediaKindOf(dto.media)
                io.launch {
                    runCatching {
                        ComradeCore.sendCallSignalTyped(dto.peer, dto.callId, busyMedia, CallSignal.Busy)
                        ComradeCore.logCallTyped(
                            dto.peer, dto.callId, busyMedia,
                            incoming = true, outcome = "busy",
                            startedAt = nowEpochSecs(), durationSecs = 0,
                        )
                    }.onFailure { Log.w(TAG, "busy-reject failed", it) }
                }
                return false
            }
        }
    }

    /** Glare candidate: an outgoing, not-yet-connected call of ours to the exact peer who just offered us one. */
    private fun isGlareCandidate(existing: Session, remotePeer: String): Boolean =
        !existing.incoming && existing.connectedAtMs == 0L && existing.peer == remotePeer

    /** Start ringing a brand-new incoming offer — the fresh-call path, also reused by the glare loser. */
    private fun ringFreshIncoming(dto: CallSignalDto, sdp: String): Boolean {
        val media = mediaKindOf(dto.media)
        // Seed the label with the cheap, non-blocking short key. This runs
        // inside the @Synchronized onIncomingSignal monitor, so it must NOT
        // call the blocking JNI store read resolvePeerLabel used to do — that
        // held the monitor across a full-history decrypt scan, blocking any
        // main-thread call control (accept/hangup/…) that arrived meanwhile.
        // The alias/@handle is resolved off the monitor in upgradePeerLabel.
        val s = Session(dto.callId, dto.peer, mullu.comrade.ui.shortNpub(dto.peer), media, incoming = true)
        s.offerSdp = sdp
        session = s
        _state.value = CallUiState.Ringing(s.peer, s.peerLabel, s.isVideo, incoming = true)
        // Best-effort "ringing on my device" ack; failure is non-fatal.
        sendSignal(s, CallSignal.Ringing)
        // Auto-miss the call if the user never accepts.
        armTimeout(s, RING_TIMEOUT_MS, HangupReason.MISSED, "missed")
        // Upgrade the short key to the contact's alias/@handle without holding
        // the monitor across the blocking store read.
        upgradePeerLabel(s)
        return true
    }

    /**
     * Resolve [Session.peerLabel] from the short key to the contact's
     * alias/published @handle off the signal-handling monitor (the lookup is a
     * blocking JNI store read), then republish the ringing state so the screen
     * and any already-shown notification title agree once it lands. A lookup
     * failure (e.g. store locked) simply leaves the short key in place.
     */
    private fun upgradePeerLabel(s: Session) {
        io.launch {
            val resolved = resolvePeerLabel(s.peer)
            synchronized(this@CallManager) {
                if (session !== s || s.ended || resolved == s.peerLabel) return@launch
                s.peerLabel = resolved
                val current = _state.value
                if (current is CallUiState.Ringing && current.incoming) {
                    _state.value = CallUiState.Ringing(s.peer, resolved, s.isVideo, incoming = true)
                }
            }
        }
    }

    /**
     * Display name for an incoming call's peer: the same alias/published-name
     * precedence [mullu.comrade.ui.peerTitle] applies across the rest of the
     * app (chat list, call history), so the ringing screen and the call
     * notification ([mullu.comrade.ChatEventRouter] reads it back off
     * [CallUiState.Ringing.peerLabel]) show the same name. Falls back to the
     * shortened key on any lookup failure (e.g. store locked) — never blocks
     * or throws on the signal-handling path.
     */
    private fun resolvePeerLabel(peer: String): String {
        val convo = runCatching { ComradeCore.conversations() }.getOrDefault(emptyList())
            .find { it.peer == peer }
        return mullu.comrade.ui.peerTitle(peer, convo?.alias, convo?.peerName)
    }

    private fun applyRemoteAnswer(s: Session, sdp: String) {
        val signalingState = s.pc?.signalingState()
        if (decideAnswer(signalingState) != AnswerDecision.APPLY) {
            // A duplicate/out-of-order Answer (relay redelivery, or one that
            // arrived after we'd already moved on) — WebRTC rejects
            // setRemoteDescription outside HAVE_LOCAL_OFFER, tearing down the
            // live call; dropping it here is what keeps that call alive.
            Log.i(TAG, "ignoring answer: signalingState=$signalingState (not HAVE_LOCAL_OFFER), callId=${s.callId}")
            return
        }
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
     *
     * Only ever reached pre-connect now (`peerObserver`'s `onConnectionChange`
     * routes a *post*-connect FAILED to [armRecoveryTimeout] instead, which
     * does apply to the callee — see that function).
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
                pc.setConfiguration(rtcConfig(widened.map { it.toWebRtc() }))
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
        cancelRecoveryTimeout(s) // back to CONNECTED cancels any pending recovery countdown
        _state.value = CallUiState.Active(s.peer, s.peerLabel, s.isVideo, s.incoming, s.connectedAtMs)
        startStatsPolling(s)
        maybeDeriveSas(s)
    }

    /**
     * A previously-CONNECTED call's media path just failed, or has been
     * DISCONNECTED for longer than [DISCONNECT_GRACE_MS] — arm a bounded
     * recovery window: if ICE hasn't reported CONNECTED again within [ms],
     * end the call honestly rather than leaving it "Active" with dead media
     * forever. A no-op if already counting down.
     */
    private fun armRecoveryTimeout(s: Session, ms: Long) {
        if (s.recovering) return
        s.recovering = true
        s.recoveryJob?.cancel()
        s.recoveryJob = io.launch {
            delay(ms)
            synchronized(this@CallManager) {
                if (session === s && !s.ended && s.recovering) {
                    Log.w(TAG, "connection did not recover within ${ms}ms; ending, callId=${s.callId}")
                    endWith(HangupReason.FAILED, "connection lost", sendHangup = true)
                }
            }
        }
    }

    private fun cancelRecoveryTimeout(s: Session) {
        s.recovering = false
        s.recoveryJob?.cancel()
        s.recoveryJob = null
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
                    // classifyQuality is pure — safe on the WebRTC thread — but
                    // the monitor is not (see [webRtcLane]).
                    val quality = classifyQuality(report)
                    io.launch(webRtcLane) {
                        synchronized(this@CallManager) {
                            if (session === s && !s.ended) _connectionQuality.value = quality
                        }
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
        s.recovering = false
        s.recoveryJob?.cancel()
        // Belt-and-suspenders alongside the `session !== s || s.ended` guards
        // in startOutgoingCall's continuation and setLocalThen/setRemoteThen:
        // whatever path reached endWith, a still-pending placeCall/offer
        // coroutine must never proceed afterward (AUDIT.md COMMS-05).
        s.placingJob?.cancel()
        // Remember this callId so a redelivered terminal Offer (relay
        // at-least-once delivery, or the 2-day backfill re-scan) doesn't ring
        // again — see onIncomingSignal/endedCallIds.
        rememberEnded(s.callId)

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
        // Give the mic back to the wake-word recogniser now that the call's
        // own hold on it (see setupPeer) is gone — a no-op while a voice-note
        // recording still holds it (see MicHolderSet).
        WakeWordService.resume(MicHolder.CALL)
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
        if (!disableCallServiceForTest) appContext?.let { CallService.stop(it) }
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
        // Each call gets a fresh chance to route to Bluetooth — a denial on a
        // past call must not permanently hide the option (the user may have
        // since granted it via system settings, or just wants to be asked
        // again on the next call).
        bluetoothPermissionDenied = false
        priorAudioMode = am.mode
        val focus = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
            .setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                    .build(),
            )
            // Delivered on the MAIN thread. onAudioFocusChange is @Synchronized,
            // and beginAudioRouting (running under the CallManager monitor on io)
            // holds that monitor across slow native audio calls — so doing the
            // work inline here would block the UI thread on the lock. Hop to io
            // so the main thread never waits on the monitor from an audio callback.
            .setOnAudioFocusChangeListener { change -> io.launch { onAudioFocusChange(change) } }
            .build()
        audioFocus = focus
        am.requestAudioFocus(focus)
        am.mode = AudioManager.MODE_IN_COMMUNICATION

        val callback = object : AudioDeviceCallback() {
            // registerAudioDeviceCallback(…, null) delivers these on the MAIN
            // thread; refreshAndMaybeSwitchRoute is @Synchronized and the monitor
            // is held throughout beginAudioRouting's slow native audio calls, so
            // running it inline blocked the UI thread on the lock long enough to
            // starve CallService.startForeground() → the "did not call
            // startForeground()" ANR. Hop to io so the main thread never blocks.
            override fun onAudioDevicesAdded(addedDevices: Array<out AudioDeviceInfo>) {
                io.launch { refreshAndMaybeSwitchRoute(am) }
            }
            override fun onAudioDevicesRemoved(removedDevices: Array<out AudioDeviceInfo>) {
                io.launch { refreshAndMaybeSwitchRoute(am) }
            }
        }
        audioDeviceCallback = callback
        am.registerAudioDeviceCallback(callback, null)

        refreshAvailableRoutes(am)
        val avail = _availableRoutes.value
        val initial = when {
            // Bluetooth is only auto-picked when it can actually be applied
            // (see canRouteBluetooth) — otherwise it stays selectable in the
            // route menu, where tapping it runs the permission prompt.
            AudioRoute.BLUETOOTH in avail && canRouteBluetooth() -> AudioRoute.BLUETOOTH
            AudioRoute.WIRED in avail -> AudioRoute.WIRED
            video -> AudioRoute.SPEAKER
            else -> AudioRoute.EARPIECE
        }
        setAudioRoute(initial)
    }

    /**
     * React to a system audio-focus change mid-call — e.g. another app
     * briefly grabs focus for a notification sound, or a higher-priority
     * call app takes over. Mutes our outgoing audio while we don't hold
     * focus, restores it on regain. Deliberately doesn't touch [_muted] (the
     * user's own mute toggle): this is a transient, focus-driven mute, so
     * regaining focus must restore whatever the user's own mute state
     * actually was, not force-unmute them.
     */
    @Synchronized
    private fun onAudioFocusChange(focusChange: Int) {
        val s = session ?: return
        when (focusChange) {
            AudioManager.AUDIOFOCUS_LOSS, AudioManager.AUDIOFOCUS_LOSS_TRANSIENT ->
                s.audioTrack?.setEnabled(false)
            AudioManager.AUDIOFOCUS_GAIN ->
                s.audioTrack?.setEnabled(!_muted.value)
        }
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
                AudioRoute.BLUETOOTH in avail && canRouteBluetooth() -> AudioRoute.BLUETOOTH
                AudioRoute.WIRED in avail -> AudioRoute.WIRED
                else -> AudioRoute.EARPIECE
            }
            setAudioRoute(fallback)
        } else if (AudioRoute.BLUETOOTH in avail && !hadBluetooth && canRouteBluetooth()) {
            setAudioRoute(AudioRoute.BLUETOOTH) // a headset just connected — prefer it
        } else if (AudioRoute.WIRED in avail && !hadWired && AudioRoute.BLUETOOTH !in avail) {
            setAudioRoute(AudioRoute.WIRED)
        }
    }

    /**
     * Set once the user explicitly selected the Bluetooth route but denied
     * the `BLUETOOTH_CONNECT` runtime prompt (API 31+, requested by the UI —
     * see `AudioRouteButton` in CallScreen.kt) — [refreshAvailableRoutes] then
     * excludes Bluetooth from [availableRoutes] for the rest of *this* call
     * rather than re-offering (and re-denying) it on every device-callback
     * re-scan. Reset per call in [beginAudioRouting].
     */
    @Volatile
    private var bluetoothPermissionDenied = false

    /** Called by the UI once the user denies the Bluetooth permission prompt after selecting that route. */
    @Synchronized
    fun onBluetoothPermissionDenied() {
        bluetoothPermissionDenied = true
        if (_audioRoute.value == AudioRoute.BLUETOOTH) setAudioRoute(AudioRoute.EARPIECE)
        val am = appContext?.getSystemService(Context.AUDIO_SERVICE) as? AudioManager ?: return
        refreshAvailableRoutes(am)
    }

    /** Rebuild [availableRoutes] from the platform's current output devices. */
    private fun refreshAvailableRoutes(am: AudioManager) {
        val routes = linkedSetOf(AudioRoute.EARPIECE, AudioRoute.SPEAKER)
        val types = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            am.availableCommunicationDevices.map { it.type }
        } else {
            legacyOutputDeviceTypes(am)
        }
        val bluetoothTypes = routeDeviceTypes(AudioRoute.BLUETOOTH)
        val wiredTypes = routeDeviceTypes(AudioRoute.WIRED)
        for (type in types) {
            when (type) {
                in bluetoothTypes -> if (!bluetoothPermissionDenied) routes.add(AudioRoute.BLUETOOTH)
                in wiredTypes -> routes.add(AudioRoute.WIRED)
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

    // Called from startOutgoingCall/accept's io.launch before either takes the
    // CallManager monitor (T3.4: this is a synchronous native init with no
    // business running on the caller's Compose-click thread). It guards the
    // check-then-act with [factoryLock] — deliberately NOT this object's own
    // monitor — so two overlapping call-setup attempts (e.g. a glare loser
    // whose non-suspend native call is still finishing when a new session
    // starts its own) can't double-initialise, while a main-thread call
    // control (`@Synchronized` on `this`) is never blocked behind the
    // multi-second native build. See [factoryLock].
    private fun ensureFactory(context: Context) = synchronized(factoryLock) {
        if (factory != null) return@synchronized
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

    // ── Pure signal/state decisions (unit-testable without WebRTC) ───────────
    //
    // Each of these mirrors `remoteIceCandidate`/`outgoingSdpMLineIndex` above:
    // `internal`, side-effect-free, and taking only plain values or WebRTC
    // *enum* types (safe to reference in a JVM unit test — see
    // `CallManagerTest`'s existing use of `IceCandidate`/`CallSignal`; unlike
    // `PeerConnectionFactory` these carry no native object, just a value) so
    // the actual call-handling functions above can stay thin dispatchers.

    /** Whether an incoming `Offer` for `callId` must be dropped because that call already ended — see [endedCallIds]. */
    internal fun isOfferForEndedCall(callId: String, endedCallIds: Collection<String>): Boolean =
        callId in endedCallIds

    /**
     * Wall-clock now, in unix seconds — used for the busy-reject call-history
     * entry, which used to hard-code `startedAt = 0` (rendering as the 1970
     * epoch in [mullu.comrade.ui.CallHistoryScreen]). `internal` so a test can
     * pin "never zero" without waiting on a real busy-reject round-trip.
     */
    internal fun nowEpochSecs(): Long = System.currentTimeMillis() / 1000

    internal enum class AnswerDecision { APPLY, IGNORE }

    /**
     * An `Answer` is only meaningful while we're the caller still waiting on
     * our own outstanding offer (`HAVE_LOCAL_OFFER`). Applying one outside
     * that state — a redelivered duplicate, or one that arrives late — makes
     * WebRTC's `setRemoteDescription` fail and tears down the live call.
     */
    internal fun decideAnswer(signalingState: PeerConnection.SignalingState?): AnswerDecision =
        if (signalingState == PeerConnection.SignalingState.HAVE_LOCAL_OFFER) {
            AnswerDecision.APPLY
        } else {
            AnswerDecision.IGNORE
        }

    internal enum class OfferDecision { RENEGOTIATE, DUPLICATE_NOOP, BUSY }

    /**
     * How to handle an incoming `Offer` while a [Session] already exists.
     * `BUSY` covers both "genuinely busy on another call" and "glare with
     * this same peer" — [handleRemoteOffer] tells those apart with
     * [isGlareCandidate]/[decideGlare] before actually sending a `Busy` signal.
     */
    internal fun decideOfferForExistingSession(
        incomingCallId: String,
        existingCallId: String,
        existingHasPc: Boolean,
    ): OfferDecision = when {
        incomingCallId == existingCallId && existingHasPc -> OfferDecision.RENEGOTIATE
        incomingCallId == existingCallId -> OfferDecision.DUPLICATE_NOOP
        else -> OfferDecision.BUSY
    }

    internal enum class GlareDecision { WE_WIN_KEEP_OUTGOING, WE_LOSE_TAKE_INCOMING }

    /**
     * Deterministic tiebreak for "glare" — both peers dialled each other at
     * about the same moment. Whoever has the lexicographically smaller npub
     * wins as caller; the loser silently cancels its own outgoing ring (no
     * `Busy` — the winner already knows about this call, they placed it) and
     * answers the incoming offer instead. Both sides reach the same outcome
     * independently: this is symmetric under swapping the two npubs.
     */
    internal fun decideGlare(ourNpub: String, remoteNpub: String): GlareDecision =
        if (ourNpub < remoteNpub) GlareDecision.WE_WIN_KEEP_OUTGOING else GlareDecision.WE_LOSE_TAKE_INCOMING

    internal enum class ConnectionStateAction { NONE, RECOVER_NOW, RECOVER_AFTER_GRACE, TRY_TURN_FALLBACK }

    /**
     * What a [PeerConnection.PeerConnectionState] change should do, given
     * whether this call had already reached `CONNECTED` at least once.
     * `CONNECTED` itself isn't decided here — [peerObserver] handles it
     * directly via [onConnected], since that path does real state-machine
     * work, not just a decision.
     */
    internal fun decideConnectionStateAction(
        newState: PeerConnection.PeerConnectionState,
        hasConnectedBefore: Boolean,
    ): ConnectionStateAction = when (newState) {
        PeerConnection.PeerConnectionState.FAILED ->
            if (hasConnectedBefore) ConnectionStateAction.RECOVER_NOW else ConnectionStateAction.TRY_TURN_FALLBACK
        PeerConnection.PeerConnectionState.DISCONNECTED ->
            if (hasConnectedBefore) ConnectionStateAction.RECOVER_AFTER_GRACE else ConnectionStateAction.NONE
        else -> ConnectionStateAction.NONE
    }

    /** An [SdpObserver] whose only interesting callback is create-success. */
    private fun createSdpObserver(s: Session, onCreated: (SessionDescription) -> Unit) = object : SdpObserver {
        override fun onCreateSuccess(desc: SessionDescription) = onCreated(desc)
        override fun onSetSuccess() {}
        override fun onCreateFailure(error: String?) {
            Log.e(TAG, "createOffer/Answer failed: $error")
            // WebRTC-thread callback — hop before taking the monitor (see [webRtcLane]).
            io.launch(webRtcLane) {
                synchronized(this@CallManager) {
                    if (session === s) endWith(HangupReason.FAILED, "failed", sendHangup = true)
                }
            }
        }
        override fun onSetFailure(error: String?) {}
    }

    private fun setLocalThen(s: Session, desc: SessionDescription, onDone: () -> Unit) {
        s.pc?.setLocalDescription(
            object : SdpObserver {
                override fun onCreateSuccess(p0: SessionDescription?) {}
                override fun onSetSuccess() {
                    // WebRTC-thread callback — hop before taking the monitor (see [webRtcLane]).
                    io.launch(webRtcLane) {
                        synchronized(this@CallManager) {
                            if (session !== s || s.ended) return@launch
                            // The offer for the caller, the answer for the callee — and
                            // whichever of the two on a renegotiation, overwriting the
                            // prior value so it can never go stale (see [Session.localSdp]).
                            s.localSdp = desc.description
                            onDone()
                        }
                    }
                }
                override fun onCreateFailure(p0: String?) {}
                override fun onSetFailure(error: String?) {
                    Log.e(TAG, "setLocalDescription failed: $error")
                    io.launch(webRtcLane) {
                        synchronized(this@CallManager) {
                            if (session === s) endWith(HangupReason.FAILED, "failed", sendHangup = true)
                        }
                    }
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
                    // WebRTC-thread callback — hop before taking the monitor (see [webRtcLane]).
                    io.launch(webRtcLane) {
                        synchronized(this@CallManager) {
                            if (session !== s || s.ended) return@launch
                            s.remoteSet = true
                            // The answer for the caller, the offer for the callee — and
                            // whichever of the two on a renegotiation, overwriting the
                            // prior value so it can never go stale (see [Session.remoteSdp]).
                            s.remoteSdp = desc.description
                            flushPendingIce(s)
                            onDone()
                        }
                    }
                }
                override fun onCreateFailure(p0: String?) {}
                override fun onSetFailure(error: String?) {
                    Log.e(TAG, "setRemoteDescription failed: $error")
                    io.launch(webRtcLane) {
                        synchronized(this@CallManager) {
                            if (session === s) endWith(HangupReason.FAILED, "failed", sendHangup = true)
                        }
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
            // Delivered on the WebRTC signaling thread — taking the monitor
            // inline here deadlocks against any monitor holder inside a
            // blocking PeerConnection proxy call (see [webRtcLane]).
            io.launch(webRtcLane) {
                if (newState == PeerConnection.PeerConnectionState.CONNECTED) {
                    synchronized(this@CallManager) { if (session === s) onConnected(s) }
                    return@launch
                }
                val hasConnectedBefore = synchronized(this@CallManager) { s.connectedAtMs > 0 }
                when (decideConnectionStateAction(newState, hasConnectedBefore)) {
                    // A call that was already Active lost its media path — arm a
                    // bounded recovery window rather than the pre-connect TURN
                    // retry (which only the caller drives) or leaving it hanging
                    // forever (the callee's prior behavior).
                    ConnectionStateAction.RECOVER_NOW ->
                        synchronized(this@CallManager) { if (session === s) armRecoveryTimeout(s, RECOVERY_TIMEOUT_MS) }
                    // DISCONNECTED can be transient (a brief blip, or an ICE
                    // restart in progress) — tolerate it for a grace period
                    // before starting the same recovery countdown.
                    ConnectionStateAction.RECOVER_AFTER_GRACE ->
                        synchronized(this@CallManager) {
                            if (session === s) armRecoveryTimeout(s, DISCONNECT_GRACE_MS + RECOVERY_TIMEOUT_MS)
                        }
                    // Pre-connect FAILED: the caller's existing STUN→TURN fallback.
                    ConnectionStateAction.TRY_TURN_FALLBACK ->
                        synchronized(this@CallManager) { if (session === s) tryTurnFallbackOrFail(s) }
                    ConnectionStateAction.NONE -> Unit
                }
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
