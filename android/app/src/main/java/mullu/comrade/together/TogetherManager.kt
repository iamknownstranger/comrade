package mullu.comrade.together

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.net.Uri
import android.os.Build
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import mullu.comrade.ComradeCore
import mullu.comrade.Notifier
import mullu.comrade.transfer.ShareReadPolicy

/**
 * The live watch-together session on this device: one player, one peer, one
 * state flow.
 *
 * Shaped after [mullu.comrade.call.CallManager] on purpose — an object holding a
 * `StateFlow`, owning its own hardware, and starting/stopping its foreground
 * service from the same points it starts and stops playback, rather than from
 * any Compose tree. A session must not end because a screen was disposed.
 *
 * **What this does not own:** the drift arithmetic (that is
 * `comrade_core::together`, shared with desktop) and the echo/scrubber decisions
 * (those are [TogetherDecisions], pure and unit-tested). This is the wiring
 * between them and the player.
 */
object TogetherManager {

    /** How the screen reads. Mirrors `sessionStatusLabel` in the desktop module. */
    sealed interface UiState {
        data object Idle : UiState

        data class Invited(
            val peer: String,
            val peerLabel: String,
            val title: String,
            val youtube: Boolean,
            /**
             * The `TogetherContent` variant's tag, as
             * [PlaybackModeDecision.ownershipFor] takes it — `local_file` ·
             * `youtube` · `service` · `stream`.
             *
             * Carried rather than reduced to booleans because the mode decision
             * is core's shape and this screen should be asking it, not
             * re-deriving it from two flags that happen to line up today.
             */
            val contentKind: String,
        ) : UiState

        data class Live(
            val peer: String,
            val peerLabel: String,
            val title: String,
            val weLead: Boolean,
            val joined: Boolean,
            val ready: Boolean,
            val playing: Boolean,
            val positionMs: Long,
            val durationMs: Long,
            val status: Status,
            /**
             * Whether this recording turned out to have a picture, and how big.
             * Known only after the decoder reports it, so it starts as
             * [TogetherDecisions.Picture.None] and the surface appears when
             * there is something to put on it.
             *
             * An embed session is the exception and sets it up front
             * ([TogetherDecisions.EMBED_PICTURE]) — there is no decoder of ours
             * to ask, and there is always a picture.
             */
            val picture: TogetherDecisions.Picture = TogetherDecisions.Picture.None,
            /**
             * Whether the picture is drawn by a `WebView` we host rather than by
             * our own decoder.
             *
             * The screen needs it because the two draw with completely different
             * views — a `SurfaceView` the decoder writes into, against the
             * IFrame player with its own controls — and because an embed has no
             * file to hand over, so the handover affordances have nothing to
             * offer. Nothing else branches on it: the session, the ladder and
             * the readout are the same for both.
             */
            val embed: Boolean = false,
            /**
             * Whether another app entirely is holding the playback and Comrade
             * is only following it (`docs/TOGETHER.md` §13).
             *
             * The screen draws control-and-status for one of these: there is no
             * sleeve, no surface and no scrubber, because there is nothing of
             * ours to render, the picture is in somebody else's window, and a
             * `MediaSession` carries no length to scrub against.
             */
            val external: Boolean = false,
            /**
             * Whether this device is *sending* the picture and sound of what it
             * plays, rather than both sides playing their own copy
             * (`docs/TOGETHER.md` §15).
             *
             * The screen needs it for one thing the other modes have no use
             * for: a microphone control. A streamed session carries one audio
             * track, so the sender's voice and the film share it — and there is
             * nothing to switch on in a session where no audio of ours is going
             * anywhere.
             */
            val streaming: Boolean = false,
            /**
             * The decoder is waiting on bytes that have not arrived.
             *
             * Only ever true for a session whose source is a network one — a URL
             * stream or a file still being handed over. A local file cannot
             * buffer. Advisory: `MediaPlayer` may end a stall with no second
             * event, so the screen shows it and never waits on it.
             */
            val buffering: Boolean = false,
            /**
             * The last measured gap between the two playheads, signed —
             * positive means this device is ahead — with the error on it and
             * when it was taken.
             *
             * Kept raw rather than pre-rendered because whether any of it may
             * be *shown* depends on how old it is by the time the screen draws,
             * which only the screen knows. [correctedAtMs] of zero is the
             * honest starting state: nothing measured yet, which
             * [TogetherDecisions.measurement] reads as stale and shows as
             * blank rather than as a reassuring zero.
             */
            val driftMs: Long = 0,
            val qualityMs: Long = 0,
            val correctedAtMs: Long = 0,
            /**
             * What our own player was pointed at, so the screen can look for a
             * cover to draw.
             *
             * A string rather than a `Uri` for the same reason
             * [TogetherDecisions.Track] carries one: this is read by code that
             * is easier to keep honest without Android types in it, and nothing
             * here parses it — [MusicLibrary.artwork] is the only reader.
             *
             * Null for an embed and for an external session, which is not a gap
             * to fill: an embed draws its own thumbnail inside the player, and
             * another app's artwork is in another app's window.
             */
            val sourceUri: String? = null,
            /**
             * Whether there is nobody else in this session.
             *
             * The whole of listening alone, as far as this screen is concerned:
             * the player, the queue, the transport, the service and the
             * notification are the paired ones unchanged, and what a `true` here
             * removes is the half of the screen that is *about* the other person
             * — their name, the status line, the two measured readouts, and the
             * offer to send them the picture. Set once at construction from
             * [TogetherDecisions.isAlone] rather than re-derived, so the screen
             * never has to test a peer id for emptiness.
             */
            val solo: Boolean = false,
        ) : UiState
    }

    /** The honest vocabulary — never "synced", never "in sync". */
    enum class Status { WaitingForThem, OpenYourCopy, Together, CatchingUp, LostTrack, TheyPaused }

    private val _state = MutableStateFlow<UiState>(UiState.Idle)
    val state: StateFlow<UiState> = _state.asStateFlow()

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var pollJob: Job? = null

    /**
     * Whoever is holding this session's playback, behind the [SessionPlayer]
     * seam (`docs/TOGETHER.md` §14) — our own [TogetherPlayer] today, an embed
     * or an external session once those are built. Everything below drives it
     * through the interface; the two places that genuinely need the concrete
     * player ([openPlayer] and [attachSurface]) narrow it back and say why.
     */
    private var player: SessionPlayer? = null
    private val suppressor = TogetherDecisions.EchoSuppressor()
    private var scrub = TogetherDecisions.ScrubState(scrubbing = false, pendingRemoteMs = null)
    private var appContext: Context? = null

    /** What the peer named, kept so the library can be searched for it. */
    private var wanted: uniffi.comrade_core.Recording? = null
    private var wantedMs: Long = 0

    /**
     * The video an invitation named, when it was a YouTube one.
     *
     * Kept for the same reason [wanted] is: the invitation arrives before the
     * person decides, and the id is what [joinEmbed] needs when they do.
     */
    private var wantedVideoId: String? = null

    /**
     * The URL an invitation named, when it was a public media one.
     *
     * The sibling of [wantedVideoId], and kept for the same reason. Held as the
     * whole [uniffi.comrade_core.TogetherContent] rather than the bare string so
     * [joinStream] hands core back exactly the content it validated on the way
     * in, rather than re-deriving something that has to match it.
     */
    private var wantedStream: uniffi.comrade_core.TogetherContent.Stream? = null

    /** The invitation's content kind, for [PlaybackModeDecision.ownershipFor]. */
    private var invitedKind: String = ""

    private val _pairing = MutableStateFlow<TogetherDecisions.Pairing?>(null)

    /**
     * Who this device is listening with, for as long as the session lasts.
     *
     * **The unit of this feature is the pair, not the track.** Choosing someone
     * used to be part of choosing a thing to play, so every song asked again
     * and the other person was invited again — which is the right shape for
     * "watch this film with me" and the wrong one for a music player. This is
     * what survives a change of content on both sides:
     * [TogetherDecisions.startStep] reads it here, and
     * [TogetherDecisions.continuesSession] reads it on the receiving side.
     */
    val pairing: StateFlow<TogetherDecisions.Pairing?> = _pairing.asStateFlow()

    /**
     * When the last session with [_pairing] ended, or `0`.
     *
     * The receiving half of the pairing: an invitation from the same person
     * moments after a session ended is the next track rather than a new
     * request, and this is what "moments" is measured from.
     */
    private var pairingEndedAtMs: Long = 0

    /**
     * How many of *our own* `ended` events to swallow.
     *
     * `together_start` refuses while a session exists, so replacing what is
     * playing is an end followed by a start on the wire — and the end's
     * `TogetherEnded { by_peer: false }` reaches this device through the event
     * channel, which is to say **after** `together_end` returned and routinely
     * after the `together_start` that replaced it. Acting on it would tear down
     * the session that is already running.
     *
     * A count rather than a flag because it is nearly exact: one `together_end`
     * produces exactly one of these, so one increment consumes exactly one
     * event. The one case where it does not is worth stating, because it is
     * what [pendingSelfEndsAtMs] exists for — `together_end` on a session core
     * has *already* expired returns without emitting anything at all, which
     * would leave this armed and swallow the next genuine end instead.
     *
     * Nothing else is needed here, and it is worth saying why rather than
     * rediscovering it: core drops an inbound `Start` while a session exists,
     * and filters an `End` whose session id is not the one it is in
     * (`runtime.rs`, `dispatch_together`). So a stale *peer* end cannot reach
     * this at all, and only our own can.
     */
    private var pendingSelfEnds: Int = 0

    /** When [pendingSelfEnds] was last armed. */
    private var pendingSelfEndsAtMs: Long = 0

    /**
     * How long an armed [pendingSelfEnds] stays believable.
     *
     * The event it is waiting for is queued before `together_end` returns, so
     * it is a matter of milliseconds; anything past this is the case where no
     * event was emitted, and holding the latch would cost a real end.
     */
    private const val SELF_END_WINDOW_MS: Long = 5_000

    /**
     * What "next" means, when there is anything for it to mean.
     *
     * The list a track was picked out of — the library as the search field had
     * narrowed it, which is what the person was looking at when they chose.
     * Null for every source that is one thing rather than a list: a pasted
     * link, a file from the picker, a followed external session.
     */
    private val _queue = MutableStateFlow<TogetherDecisions.Queue?>(null)

    /**
     * A flow rather than a field because the next button greys out on it, and a
     * plain field would leave that button stale until something else happened
     * to recompose the transport.
     */
    val queue: StateFlow<TogetherDecisions.Queue?> = _queue.asStateFlow()

    /**
     * The video an embed session is on, so a refusal can offer a way out.
     *
     * Distinct from [wantedVideoId], which is what an *invitation* named before
     * anybody decided: this one is what is actually playing, on either side.
     */
    private var playingVideoId: String? = null

    /**
     * The public media URL this session is playing, on either side.
     *
     * Tracked for the same reason [playingVideoId] is: the status line has to
     * know what kind of thing is playing, and "open your copy to start" is
     * meaningless for a URL both devices fetch for themselves. See
     * [TogetherDecisions.needsOwnCopy].
     */
    private var playingStreamUrl: String? = null

    private val _embedFailure = MutableStateFlow<TogetherDecisions.EmbedFailure?>(null)

    /**
     * Why the embed will not play this, when it will not.
     *
     * Separate from [openFailed] because there is something to *do* about this
     * one: the common case by far is a video whose owner does not allow it
     * outside YouTube, and the useful answer is a way over there rather than a
     * sentence. Until now the error reached logcat and nothing else, so the
     * screen kept a transport under YouTube's own "This video is unavailable"
     * panel and went on saying it was waiting for the other person to open it.
     */
    val embedFailure: StateFlow<TogetherDecisions.EmbedFailure?> = _embedFailure.asStateFlow()

    /** Where to send someone the embed refused. Null unless there is one. */
    fun watchUrl(): String? = playingVideoId?.let { TogetherDecisions.watchUrl(it) }

    /**
     * The sound of what we are playing, on its way out (`docs/TOGETHER.md` §15).
     * Null in every session that is not streaming, which is all of them until
     * the stream is wired.
     */
    private var capture: PlaybackCapture? = null

    /** The picture of what we are playing, on its way out. Null unless streaming. */
    private var videoCapturer: PlayerVideoCapturer? = null
    private var videoSource: org.webrtc.VideoSource? = null
    private var surfaceHelper: org.webrtc.SurfaceTextureHelper? = null

    private val _localVideo = MutableStateFlow<org.webrtc.VideoTrack?>(null)

    /**
     * The sender's own outgoing picture, for the sender to watch.
     *
     * A `MediaPlayer` decodes into exactly one surface, and a streamed session
     * gives that surface to [PlayerVideoCapturer] — so the sender cannot also
     * point it at a `SurfaceView`. They render *this* instead, the same track
     * the other person receives, exactly as the call screen renders local camera
     * video. One picture path rather than two.
     */
    val localVideo: StateFlow<org.webrtc.VideoTrack?> = _localVideo.asStateFlow()

    private val _remoteVideo = MutableStateFlow<org.webrtc.VideoTrack?>(null)

    /**
     * The other person's picture, when they are streaming to us.
     *
     * The receiving half of [localVideo], and the receiver's whole player: there
     * is no decoder of ours in this mode and no playhead of ours to hold — the
     * frames arrive already in step, because there is only one playhead and it
     * is theirs. That is §15's claim about sync, in the one field that
     * implements it.
     */
    val remoteVideo: StateFlow<org.webrtc.VideoTrack?> = _remoteVideo.asStateFlow()

    private var streamAudioSource: org.webrtc.AudioSource? = null
    private var streamAudioTrack: org.webrtc.AudioTrack? = null

    /**
     * What arrives when the other side streams to us.
     *
     * Installed for the length of the app rather than per session: a stream
     * offer can reach this device before it knows one is coming, which is the
     * whole point of "the SDP is the intent".
     */
    private val streamSink = object : StreamTransfer.Sink {
        override fun onRemoteVideo(track: org.webrtc.VideoTrack?) {
            _remoteVideo.value = track
            // Only on arrival: a null means the stream ended, and the session
            // may be about to end with it.
            if (track != null) refreshLive(streaming = true)
        }

        override fun onRemoteAudio(track: org.webrtc.AudioTrack?) {
            // Nothing to hold: WebRTC plays a received audio track through the
            // device's own output the moment it is added. Named rather than
            // absent so the next reader does not go looking for the sink that
            // must be missing.
            runCatching { track?.setEnabled(true) }
        }

        override fun onLost() {
            _remoteVideo.value = null
            refreshLive(status = Status.LostTrack)
        }
    }

    /**
     * Installed once, for the life of the process, and that is the point.
     *
     * A stream offer can reach this device before it has any idea one is
     * coming — "the SDP is the intent" means the *first* thing that says a
     * stream exists is the offer itself. Registering per session would mean the
     * receiver had to have guessed first, which is exactly what the design
     * avoids. Placed after [streamSink] because an object's initialisers run in
     * declaration order.
     */
    init {
        StreamTransfer.setSink(streamSink)
        drainOutbound()
    }

    // ── Everything this device says to the other one ────────────────────────

    /**
     * One outbound session command, queued.
     *
     * @param what a short label, for the log line when it fails. Never the
     *   content: these end up in logcat.
     */
    private data class Outbound(val what: String, val run: () -> Unit)

    /**
     * Commands on their way out, in the order they were asked for.
     *
     * **This exists because every `together_*` call blocks the thread it is made
     * on, and they were being made on the main one.** `ComradeCore` bridges the
     * async FFI with `runBlocking`, and the last rung of `send_together` is
     * `vault.send_dm(...).await` — a relay round trip. So a tap on play ran the
     * network send inside the click handler, and with no relay reachable the
     * main thread sat in it until Android put up *"Comrade isn't responding"*.
     * Offline is exactly where that is worst and exactly where this feature is
     * supposed to be at its best.
     *
     * A channel rather than `launch(Dispatchers.IO)` per call, and the reason is
     * ordering: `together_set_state` takes the session's next Lamport sequence
     * number **inside** the call, so two commands racing on different IO threads
     * can be numbered in the opposite order to the taps that made them — pause
     * then play arriving as play then pause, which is a session that ends up
     * playing when the person asked for silence. One consumer, one at a time,
     * FIFO.
     *
     * `UNLIMITED` because the producer is a person pressing buttons and the
     * consumer is a network send: a bounded queue could only choose between
     * dropping a command and suspending the caller, and the caller is the UI
     * thread this whole mechanism exists to keep free.
     */
    private val outbound = Channel<Outbound>(Channel.UNLIMITED)

    private fun drainOutbound() = scope.launch(Dispatchers.IO) {
        for (command in outbound) {
            // Cleared by the next thing that *does* go, so the sentence on
            // screen tracks whether we can reach them now rather than whether we
            // ever failed to. A flag that only ever latches true would be a
            // permanent accusation against a session that recovered.
            runCatching { command.run() }.onSuccess { _sendFailed.value = false }.onFailure {
                // Logged and dropped rather than retried. A session command is
                // worthless once stale — the next heartbeat carries the truth,
                // and the drift ladder is what closes the gap a lost command
                // left. Retrying would deliver a play the person has since
                // undone.
                Log.w(TAG, "could not send ${command.what}", it)
                _sendFailed.value = true
            }
        }
    }

    /**
     * Say something to the other device — off this thread, in order, and **not
     * at all when there is nobody to say it to**.
     *
     * The solo gate is here rather than at each call site because that is what
     * makes listening alone the same code path as listening together
     * ([TogetherDecisions.ALONE]). Every transport control, the queue, the
     * service and the notification behave identically; the sends simply stop at
     * this line.
     */
    private fun sendOut(what: String, run: () -> Unit) {
        if (alone) return
        outbound.trySend(Outbound(what, run))
    }

    /** Whether the session being set up, or running, has nobody else in it. */
    private val alone: Boolean get() = TogetherDecisions.isAlone(_pairing.value)

    /**
     * A file-handover or stream-negotiation signal, on the session envelope.
     *
     * Exposed so [ShareTransfer] and [StreamTransfer] reach the same queue as
     * every other outbound command, which they need for a reason of their own:
     * both call from **WebRTC observer callbacks** — `onIceCandidate`,
     * `onCreateSuccess` — and a relay round trip inside one of those blocks the
     * peer connection's signalling thread. That is the shape of the two
     * callback deadlocks `.claude/rules/rust.md` records, arriving from the
     * Kotlin side instead.
     *
     * Ordering matters here too, and more than for a transport command: an
     * answer that overtakes its offer is a negotiation that cannot complete.
     */
    fun sendShareSignal(signal: uniffi.comrade_core.ShareSignal) {
        sendOut("share signal") { ComradeCore.togetherShareTyped(signal) }
    }

    private val _sendFailed = MutableStateFlow(false)

    /**
     * Whether the last thing we tried to tell them did not go.
     *
     * Surfaced because the queue made the failure silent: it used to throw out
     * of the click handler and the screen said so. Now the player starts
     * regardless — which is the point, since your own music has no business
     * waiting on a relay — and this is what admits that the other person has not
     * heard about it.
     */
    val sendFailed: StateFlow<Boolean> = _sendFailed.asStateFlow()

    private val _micEnabled = MutableStateFlow(false)

    /**
     * Whether this device's microphone is going out to the other person.
     *
     * A flow rather than a plain flag because the control is a toggle on screen
     * and has to redraw when it changes — the same shape `CallManager.muted`
     * uses for the in-call microphone, deliberately, so the two controls behave
     * alike.
     *
     * **Off by default, in every mode.** A session that opened with a live
     * microphone would have decided something about a room it cannot see.
     */
    val micEnabled: StateFlow<Boolean> = _micEnabled.asStateFlow()

    /**
     * Turn the microphone on or off.
     *
     * Listening to something together and being unable to say anything about it
     * is not the feature, so this is available in **every** mode rather than
     * only in a streamed one — a shared album with no way to say "this bit" is
     * a worse version of listening alone. What differs is only what the voice
     * rides on:
     *
     * - **streaming** (`docs/TOGETHER.md` §15): the outgoing track already
     *   carries the film's own sound, and [PlaybackCapture.micEnabled] decides
     *   whether the voice is *summed* into it. The track itself stays live.
     * - **everything else**: there is no outgoing audio at all until now, so
     *   turning the microphone on opens a voice-only connection
     *   ([startTalking]) and muting is that one track going quiet.
     *
     * @return `false` when the microphone is not ours to switch on yet —
     *   `RECORD_AUDIO` has not been granted, or the channel could not open. The
     *   screen asks for the permission and calls [startTalking]; a control that
     *   silently did nothing is the one outcome worth avoiding here.
     */
    fun toggleMic(context: Context): Boolean {
        appContext = context.applicationContext
        if (_micEnabled.value) {
            applyMic(false)
            return true
        }
        if (streamAudioTrack == null && !startTalking(context)) return false
        applyMic(true)
        return true
    }

    /** Push the microphone decision at whatever is actually carrying it. */
    private fun applyMic(on: Boolean) {
        _micEnabled.value = on
        capture?.micEnabled = on
        // A streamed session's track carries the film whatever the microphone
        // is doing, so it stays enabled; a voice-only one *is* the microphone,
        // so muting is the track going quiet.
        runCatching { streamAudioTrack?.setEnabled(capture != null || on) }
    }

    /**
     * Open a voice channel to the person we are listening with.
     *
     * The same `PeerConnection` a streamed session uses, with no video on it —
     * "the SDP is the intent" (see [StreamTransfer]) covers this without a new
     * wire type, because an offer arriving with no armed transfer is already
     * known not to be a file, and its m-lines say the rest.
     *
     * **The track is attached at negotiation time and never afterwards**, on
     * both sides: [StreamTransfer.localAudio] is what the answering side adds
     * before it answers, so once a connection exists both microphones are on it
     * and muting is `setEnabled`. Nothing here renegotiates, and this is why it
     * does not have to.
     *
     * Called by the screen after `RECORD_AUDIO` comes back granted, and by
     * [toggleMic] when it was already granted.
     */
    fun startTalking(context: Context): Boolean {
        appContext = context.applicationContext
        val ctx = appContext ?: return false
        if (!micGranted(ctx)) return false
        if (_state.value !is UiState.Live) return false
        val factory = mullu.comrade.call.CallManager.sharedFactory(ctx) ?: return false
        if (streamAudioTrack == null) {
            val source = runCatching { factory.createAudioSource(org.webrtc.MediaConstraints()) }
                .onFailure { Log.w(TAG, "could not open the microphone", it) }
                .getOrNull() ?: return false
            streamAudioSource = source
            streamAudioTrack = factory.createAudioTrack(STREAM_AUDIO_ID, source)
        }
        StreamTransfer.localAudio = streamAudioTrack
        // Already negotiated with our voice on it: nothing to do but let
        // [applyMic] unmute the track.
        if (StreamTransfer.active && StreamTransfer.sendingAudio) return true
        // Connected, but our track was not on the wire when it was negotiated —
        // the permission arrived afterwards. Renegotiating means tearing this
        // connection down and offering a new one, which is fine for a voice
        // channel and **not** fine while a picture is arriving on it: ending it
        // would take away the film to add a microphone. That case waits.
        if (_remoteVideo.value != null) return false
        return StreamTransfer.offer(ctx, _localVideo.value, streamAudioTrack)
    }

    /** Whether the microphone has been granted, asked exactly where it is used. */
    private fun micGranted(ctx: Context): Boolean =
        ctx.checkSelfPermission(android.Manifest.permission.RECORD_AUDIO) ==
            android.content.pm.PackageManager.PERMISSION_GRANTED

    /**
     * Which of the two ways [toggleMic] can say no this is.
     *
     * They need different answers and the screen cannot tell them apart from a
     * `false`: a missing permission gets the system dialog, and the other case —
     * a picture already arriving on the only connection there is, which
     * renegotiating would take away — gets a sentence, because there is nothing
     * to tap.
     */
    fun micNeedsPermission(context: Context): Boolean = !micGranted(context.applicationContext)

    /**
     * Put the voice channel back after the session it rode on was replaced.
     *
     * Signalling rides the session envelope, so replacing what is playing takes
     * the connection with it — see [beginSession]. Without this, talking would
     * end every time somebody pressed next, which is the opposite of what a
     * pairing is for.
     */
    private fun reopenVoice() {
        val ctx = appContext ?: return
        if (!_micEnabled.value) return
        if (StreamTransfer.active && StreamTransfer.sendingAudio) return
        if (!startTalking(ctx)) return
        applyMic(true)
    }

    private val _openFailed = MutableStateFlow(false)

    /**
     * Whether the thing we opened refused to play.
     *
     * Added with the pasted-link source, and the reason is that source: a URL
     * that is a web page rather than a file fails several seconds *after* the
     * session opens, in a decoder callback, with nothing on screen changing. The
     * player error was logged and nothing else, so the session sat at 0:00
     * looking like the feature was broken.
     *
     * A flag rather than a message because the sentence is the screen's, and
     * because there is only one useful thing to say — `MediaPlayer`'s
     * `what/extra` pair does not distinguish "not media" from "server hung up"
     * in any way worth putting in front of somebody.
     */
    val openFailed: StateFlow<Boolean> = _openFailed.asStateFlow()
    private var focusRequest: AudioFocusRequest? = null

    /**
     * The file this device actually has open, if it is one we can read back —
     * which is what makes us able to *send* it. A content:// URI we were handed
     * by a picker is not, so this stays null for those and the handover simply
     * does not offer, rather than failing halfway through.
     */
    private var openedPath: String? = null
    private var openedDurationMs: Long = 0

    /**
     * What our own player was pointed at, whether or not we can read it back.
     *
     * Distinct from [openedPath], which is the narrower question "can we *send*
     * this" and is therefore null for everything a picker handed us. This one is
     * only for drawing a cover, which a `content://` URI answers perfectly well.
     */
    private var openedUri: String? = null

    /**
     * Set by tests that must not touch the foreground-service contract, exactly
     * as `CallManager.disableCallServiceForTest` does. Keep it: a test that
     * genuinely exercises promotion should be its own test rather than flipping
     * this one.
     */
    @Volatile
    var disableServiceForTest: Boolean = false

    // ── Incoming, from the bridge ───────────────────────────────────────────

    /**
     * They invited us.
     *
     * Before asking the listener to go and find a file, look for it: if the
     * invitation named a recording and this device's own library holds a
     * confident match, the session can just start. That is the whole point of
     * carrying a recording identity rather than a bare duration — the Antra idea
     * (`docs/TOGETHER.md` §2), minus the acquiring.
     */
    fun onInvited(
        context: Context,
        peer: String,
        peerLabel: String,
        recording: uniffi.comrade_core.Recording?,
        durationMs: Long,
        contentKind: String,
        videoId: String?,
        stream: uniffi.comrade_core.TogetherContent.Stream? = null,
    ) {
        appContext = context.applicationContext
        wanted = recording
        wantedMs = durationMs
        wantedVideoId = videoId
        wantedStream = stream
        invitedKind = contentKind
        val youtube = videoId != null
        // A stream's own recording when the inviter named one, and otherwise the
        // host it comes from — which is what the URL discloses and the one thing
        // about it worth reading before agreeing to fetch it. Never the whole
        // URL: a 2 kB string does not belong in a sentence.
        val streamTitle = stream?.let { s ->
            s.recording?.let { titleOf(it) } ?: hostOf(s.url)
        }
        val title = streamTitle
            ?: recording?.let { titleOf(it) }.orEmpty()
        _state.value = UiState.Invited(peer, peerLabel, title, youtube, contentKind)

        // The next thing from the person we were *just* listening with is the
        // next track, not a new request — so it is answered rather than asked
        // about (`TogetherDecisions.continuesSession`, which is where the rule
        // and its one exclusion live). Being asked once per song is the thing
        // that made this tab unusable as a music player.
        //
        // Below the `youtube` and `stream` returns on purpose: those two are
        // deliberately never auto-joined *cold*, and this is the warm case.
        if (
            TogetherDecisions.continuesSession(
                pairing = _pairing.value,
                fromNpub = peer,
                contentKind = contentKind,
                endedAtMs = pairingEndedAtMs,
                nowMs = System.currentTimeMillis(),
            )
        ) {
            if (youtube) {
                joinEmbed(context)
                return
            }
            // A copy we already hold is better than one we pull over the wire,
            // so the library is still asked first — it is only the *question*
            // that is skipped, never the lookup.
            val here = recording
                ?.let { runCatching { LibraryResolver.resolve(context, it, durationMs) }.getOrNull() }
            if (here != null) join(context, here.uri) else askForTheirCopy(context)
            return
        }

        // A YouTube invitation is deliberately **not** auto-joined, even though
        // this device could certainly play it and no library lookup is needed.
        // Opening a video and starting to report a playhead is agreeing to watch
        // something with someone; the file path only ever does that when this
        // phone already held the recording, which is a much smaller claim.
        //
        // A stream invitation is not auto-joined either, and the reason is
        // stronger: joining one makes a request to a host the *other* person
        // named. That is a decision about this device's network, and it is not
        // ours to take on someone's behalf however confidently core validated the
        // URL. It also must not fall through to the lookup below — a stream that
        // happens to name a recording this phone owns would otherwise open the
        // local file and report a playhead for something else entirely.
        if (stream != null || recording == null || youtube) return
        val found = runCatching { LibraryResolver.resolve(context, recording, durationMs) }.getOrNull()
        if (found != null) join(context, found.uri)
    }

    /** Remember who this session is with, from the invitation we are answering. */
    private fun pairWith(invited: UiState.Invited) {
        _pairing.value = TogetherDecisions.Pairing(invited.peer, invited.peerLabel)
        pairingEndedAtMs = 0
    }

    /** `Artist — Title`, or just the title when the file named no artist. */
    private fun titleOf(recording: uniffi.comrade_core.Recording): String =
        if (recording.artist.isBlank()) {
            recording.title
        } else {
            "${recording.artist} — ${recording.title}"
        }

    /**
     * The host a stream URL comes from, as something to read.
     *
     * Not the path and not the query: the host is the disclosure that matters
     * before agreeing to fetch something, and the rest is length without
     * meaning. `www.` is dropped because it is never the useful part of a name.
     * Falls back to the empty string rather than to the raw URL — a screen with
     * no title is better than one with 2 kB of query string in it.
     */
    private fun hostOf(url: String): String =
        runCatching { Uri.parse(url).host.orEmpty().removePrefix("www.") }.getOrDefault("")

    /**
     * Look again for the invitation's recording, now that we may read the
     * library, and join if it is here.
     *
     * [onInvited] runs this once on arrival, but at that moment the answer is
     * whatever the permission happened to be — and the invitation is exactly the
     * moment someone is most willing to grant it. Returns whether a copy was
     * found, so the screen can say "not on this phone" rather than leaving a tap
     * that appears to do nothing.
     *
     * A no-op unless we are still holding an invitation: by the time a grant
     * comes back the session may already be live or abandoned, and re-opening a
     * file under a running player is the one thing this must not do.
     */
    fun lookAgain(context: Context): Boolean {
        if (_state.value !is UiState.Invited) return false
        val recording = wanted ?: return false
        val found = runCatching { LibraryResolver.resolve(context, recording, wantedMs) }.getOrNull()
            ?: return false
        join(context, found.uri)
        return true
    }

    /**
     * They joined. What the status line then says depends on whether there is a
     * copy for anybody to open.
     *
     * **This said `OpenYourCopy` unconditionally, and that shipped.** A YouTube
     * session showed "open your copy to start" underneath YouTube's own "This
     * video is unavailable" panel — there is no copy of an embed, and none of a
     * URL stream either, since both devices fetch the same address. The
     * distinction is [TogetherDecisions.needsOwnCopy]'s, so the JVM lane pins it.
     */
    fun onJoined() {
        (_state.value as? UiState.Live)?.let {
            val needsCopy = TogetherDecisions.needsOwnCopy(
                embed = it.embed,
                external = it.external,
                stream = playingStreamUrl != null,
            )
            _state.value = it.copy(
                joined = true,
                status = if (needsCopy) Status.OpenYourCopy else Status.Together,
            )
        }
    }

    /**
     * They played, paused or seeked.
     *
     * `posMs` has already been carried forward through the message's flight time
     * by `comrade_core::together` — this applies it as given and must **not**
     * compensate again. `applyInMs` is non-zero only when the sender was on a
     * transport fast enough to schedule ahead, in which case both players change
     * state on the same instant instead of one chasing the other.
     */
    fun onCommand(posMs: Long, playing: Boolean, applyInMs: Long) {
        val p = player ?: return
        if (applyInMs > 0) {
            scope.launch {
                delay(applyInMs)
                applyCommand(p, posMs, playing)
            }
        } else {
            applyCommand(p, posMs, playing)
        }
    }

    private fun applyCommand(p: SessionPlayer, posMs: Long, playing: Boolean) {
        // A remote seek must not yank the thumb out of a finger mid-drag.
        scrub = TogetherDecisions.onRemoteSeek(scrub, posMs)
        if (scrub.scrubbing) return
        val plan = TogetherDecisions.planCommand(
            posMs,
            playing,
            TogetherDecisions.Local(p.positionMs, p.isPlaying, p.prepared),
        )
        run(p, plan)
        refreshLive(playing = playing, status = if (playing) Status.Together else Status.TheyPaused)
    }

    /**
     * A drift correction. Emitted only when the verdict is not "hold".
     *
     * `driftMs` and `qualityMs` are the two measured figures the correction
     * carries, and they are recorded even though nothing here renders them: the
     * screen decides what they are worth saying, and the pair is worthless
     * without knowing when it was taken.
     */
    fun onCorrection(
        kind: String,
        posMs: Long,
        rate: Float,
        playing: Boolean,
        driftMs: Long = 0,
        qualityMs: Long = 0,
    ) {
        val p = player ?: return
        val plan = TogetherDecisions.planCorrection(
            kind,
            posMs,
            rate,
            playing,
            TogetherDecisions.Local(p.positionMs, p.isPlaying, p.prepared),
        )
        run(p, plan)
        refreshLive(
            status = Status.CatchingUp,
            driftMs = driftMs,
            qualityMs = qualityMs,
            correctedAtMs = System.currentTimeMillis(),
        )
    }

    fun onEnded(byPeer: Boolean) {
        // Our own end, sent to make room for the next thing with the same
        // person. It reaches us through the event channel and therefore lands
        // *after* the `together_start` that replaced it, so acting on it would
        // tear down the session that is already running. See [pendingSelfEnds].
        if (!byPeer && pendingSelfEnds > 0) {
            if (System.currentTimeMillis() - pendingSelfEndsAtMs <= SELF_END_WINDOW_MS) {
                pendingSelfEnds--
                return
            }
            // Armed and never spent: `together_end` found nothing to end,
            // because the session had already expired. Dropped rather than
            // held, or this would swallow a real end later.
            pendingSelfEnds = 0
        }
        ShareTransfer.end()
        stopPlayback()
        if (byPeer) {
            // Held for a minute rather than dropped, because from this side a
            // session ending and the next track being put on look identical —
            // an `End` followed by a `Start`. Keeping the pairing is what makes
            // the second one continue the evening instead of asking again, and
            // the cost of being wrong is small: a fresh invitation from the
            // same person inside a minute is answered rather than offered.
            val at = System.currentTimeMillis()
            pairingEndedAtMs = at
            // And dropped when the window closes, which is not decoration. The
            // pairing is also what [TogetherDecisions.startStep] reads to skip
            // the who-with sheet, so a pairing left behind by somebody who
            // genuinely walked away would mean a track picked ten minutes later
            // silently invited them.
            scope.launch {
                delay(TogetherDecisions.PAIRING_GRACE_MS)
                if (_state.value is UiState.Idle && pairingEndedAtMs == at) forgetPairing()
            }
        } else {
            forgetPairing()
        }
        _state.value = UiState.Idle
    }

    // ── Handing the file over ───────────────────────────────────────────────

    /**
     * Where our own player is, for the receiver's next request. Requests are
     * anchored at the playhead, so a seek costs one request rather than a
     * re-download — which only works if the transfer can ask.
     */
    fun currentPositionMs(): Long = player?.positionMs ?: 0

    /**
     * The live file player, when the session's sound comes from our own
     * decoder — the only case an equalizer can reach ([PlayerEffects]).
     * Returns the [SessionPlayer] so callers narrow with `as?` rather than
     * this growing a second accessor per implementation.
     */
    fun filePlayer(): SessionPlayer? = player

    /**
     * "I don't have this — send me yours."
     *
     * Joining first is deliberate and not just ordering: the handover rides the
     * session envelope, so there has to *be* a session before a byte can be
     * negotiated. That is what stops this from being a way to open a
     * peer-to-peer connection to someone who never agreed to watch anything.
     */
    fun askForTheirCopy(context: Context) {
        appContext = context.applicationContext
        val invited = _state.value as? UiState.Invited ?: return
        pairWith(invited)
        _queue.value = null
        sendOut("join") { ComradeCore.togetherJoinTyped() }
        _state.value = UiState.Live(
            peer = invited.peer,
            peerLabel = invited.peerLabel,
            title = invited.title,
            weLead = false,
            joined = true,
            // Not ready: we have nothing to play yet. The status line says so
            // rather than showing a player that cannot start.
            ready = false,
            playing = false,
            positionMs = 0,
            durationMs = 0,
            status = Status.OpenYourCopy,
        )
        ShareTransfer.ask()
        // **This route did not do this, and [startService]'s own comment claimed
        // every route out of `Invited` did.** Survivable while it was the
        // secondary "I don't have it" answer; not survivable now that it is what
        // Join does for a local file (`TogetherDecisions.joinAction`) — without
        // it the invitation stays in the shade after being answered, and the
        // transfer and the playback that follows run with no foreground service
        // to keep them alive when the app goes to the background.
        startService()
    }

    /** What the handover is doing, for the screen. Null when nothing is. */
    fun shareStatus(): String? = ShareTransfer.status

    /** One step of the handover arrived on the session channel. */
    fun onShareSignal(context: Context, signal: uniffi.comrade_core.ShareSignal) {
        appContext = context.applicationContext
        ShareTransfer.onSignal(context, signal, localPath = openedPath, durationMs = openedDurationMs)
    }

    /**
     * The file finished arriving and its hash checked out. Open it and carry on
     * from where the session already is — the point of the handover is that the
     * session does not restart, it simply stops being one-sided.
     */
    fun onSharedFileReady(path: String) {
        // On the session's own scope, like [onSharedFileStreaming] and for the
        // same reason: the caller is the transfer, finishing on
        // `Dispatchers.IO`, and [player] is otherwise only touched from here.
        // That was survivable while this ran once at the very end of a transfer
        // and nothing else held the player; it is not, now that a partial-file
        // player is live at exactly this moment.
        scope.launch { openFinishedFile(path) }
    }

    private fun openFinishedFile(path: String) {
        if (appContext == null) return
        val live = _state.value as? UiState.Live
        // Where the partial file had got to, if it was already playing
        // (`docs/TOGETHER.md` §12). Reopening resets a `MediaPlayer` to zero, so
        // without this the transfer *finishing* would throw the listener back to
        // the start of the track they were already halfway through — a
        // regression that only appears once early playback works, which is
        // exactly the kind that ships.
        val resumeAtMs = player?.positionMs ?: 0
        val wasPlaying = player?.isPlaying ?: false
        openPlayer(Uri.fromFile(java.io.File(path))) { durationMs ->
            openedPath = path
            openedDurationMs = durationMs
            if (live != null) _state.value = live.copy(ready = true, durationMs = durationMs)
            if (resumeAtMs > TogetherDecisions.EPSILON_MS) {
                // Armed, not silent: reopening produces a real `onSeekComplete`,
                // and an unexplained one is re-broadcast as the user having
                // seeked — which would move the other person to a position they
                // are already at, for no reason anyone could explain.
                suppressor.expect("seek", resumeAtMs, System.currentTimeMillis())
                player?.seekTo(resumeAtMs)
            }
            if (wasPlaying) player?.play()
        }
        // Whatever they are doing now is what we should be doing. No command is
        // sent: the next heartbeat's drift verdict closes the gap, and a
        // command from the side that just arrived would move *them*.
    }

    /** Apply a plan, arming its expectations first so nothing echoes back out. */
    private fun run(p: SessionPlayer, plan: TogetherDecisions.Plan) {
        val now = System.currentTimeMillis()
        plan.expect.forEach { (kind, pos) -> suppressor.expect(kind, pos, now) }
        for (op in plan.ops) {
            when (op) {
                is TogetherDecisions.Op.Seek -> p.seekTo(op.posMs)
                is TogetherDecisions.Op.Rate -> p.setRate(op.value)
                TogetherDecisions.Op.Play -> p.play()
                TogetherDecisions.Op.Pause -> p.pause()
            }
        }
    }

    // ── Outgoing, from this device ──────────────────────────────────────────

    /**
     * Make room for something new without letting go of the person.
     *
     * Every route that opens a session goes through here, and it is the whole
     * of "pick a person once". `together_start` refuses while a session exists,
     * so putting a second track on genuinely is an end and a start on the wire —
     * what makes it one evening rather than two is that the pairing survives it
     * here, and that the other side recognises the invitation that follows
     * ([TogetherDecisions.continuesSession]).
     *
     * The microphone survives too, which is why [stopPlayback] is told this is a
     * replacement: a voice channel rides the session envelope and therefore dies
     * with it, and losing the ability to talk every time somebody presses next
     * would be the opposite of what a pairing is for. [reopenVoice] puts it
     * back once the new session is live.
     */
    private fun beginSession(context: Context, peer: String, peerLabel: String) {
        appContext = context.applicationContext
        if (_state.value is UiState.Live) {
            pendingSelfEnds++
            pendingSelfEndsAtMs = System.currentTimeMillis()
            sendOut("leave") { ComradeCore.togetherEndTyped() }
            ShareTransfer.end()
            stopPlayback(replacing = true)
        }
        _pairing.value = TogetherDecisions.Pairing(peer, peerLabel)
        pairingEndedAtMs = 0
    }

    /**
     * @param queue the list this track came out of, so prev and next mean
     *   something. Null for every source that is one thing rather than a list,
     *   which is the honest answer for a pasted link or a file from the picker.
     */
    fun start(
        context: Context,
        peer: String,
        peerLabel: String,
        uri: Uri,
        recording: uniffi.comrade_core.Recording?,
        queue: TogetherDecisions.Queue? = null,
        resumeAtMs: Long = 0L,
    ) {
        beginSession(context, peer, peerLabel)
        _queue.value = queue
        val title = recording?.title.orEmpty()
        rememberPlayedLocal(uri, recording)
        openPlayer(uri) { durationMs ->
            // Queued, not awaited. The player is already open by this point and
            // there is no reason for it to wait on a relay — offline, that wait
            // was the ANR. A session the peer never hears about simply never
            // gets joined, which the status line already says.
            sendOut("invitation") {
                ComradeCore.togetherStartTyped(
                    peer,
                    uniffi.comrade_core.TogetherContent.LocalFile(durationMs.toULong(), recording),
                )
            }
            val startingAt = resumeAtMs.coerceIn(0L, durationMs)
            _state.value = UiState.Live(
                peer = peer,
                peerLabel = peerLabel,
                title = title,
                weLead = true,
                joined = false,
                ready = true,
                playing = false,
                // Resuming a saved queue starts where it was left, not at 0 —
                // the whole point of having saved it.
                positionMs = startingAt,
                durationMs = durationMs,
                status = Status.WaitingForThem,
                sourceUri = openedUri,
                solo = alone,
            )
            if (startingAt > 0L) {
                (player as? TogetherPlayer)?.let { p ->
                    runCatching { p.seekTo(startingAt) }
                        .onFailure { Log.w(TAG, "resume seek failed", it) }
                }
            }
            applySpeedIfAlone()
            startService()
        }
    }

    /**
     * Start a session on a YouTube video — the one route to "play something
     * neither of us has" that needs no account on either side.
     *
     * A sibling of [start] rather than a branch inside it, the same shape §14
     * asks of [openEmbed] and [openPlayer], because almost nothing is shared:
     * there is no file to remember, no length to report (the player discovers
     * that), and no handover to offer.
     */
    fun startEmbed(context: Context, peer: String, peerLabel: String, videoId: String) {
        beginSession(context, peer, peerLabel)
        _queue.value = null
        openEmbed(videoId)
        sendOut("video invitation") {
            ComradeCore.togetherStartTyped(
                peer,
                uniffi.comrade_core.TogetherContent.Youtube(videoId),
            )
        }
        _state.value = UiState.Live(
            peer = peer,
            peerLabel = peerLabel,
            // The video's own title is the embed's to know and it does not tell
            // us, so the screen shows the peer and the player shows the title —
            // rather than this inventing one from an eleven-character id.
            title = "",
            weLead = true,
            joined = false,
            ready = true,
            playing = false,
            positionMs = 0,
            // Not the inviter's to claim: `TogetherContent::duration_ms` returns
            // `None` for a video, and the scrubber grows when the player says.
            durationMs = 0,
            status = Status.WaitingForThem,
            picture = TogetherDecisions.EMBED_PICTURE,
            embed = true,
            solo = alone,
        )
        startService()
    }

    /**
     * Start a session on one public HTTPS media URL — a podcast episode off its
     * feed, an Internet Archive item (`docs/TOGETHER.md` §11a).
     *
     * A sibling of [start] and [startEmbed] rather than a branch in either, the
     * shape §14 asks for. It plays in the same `MediaPlayer` a local file does,
     * so it inherits the fine deadband and the whole correction ladder — but
     * nothing else about it is the file path: there is no copy to hand over, no
     * length to report, and the source is a URL a peer will fetch rather than
     * bytes either of us holds.
     *
     * **The order of the two steps is the decision, and it matches
     * `desktop/ui/main.js`'s `startStreamSession`: core sees the URL before the
     * media player does.** `together_start` runs `TogetherContent::admissible`,
     * which for a `Stream` is `valid_stream_url` — so a URL naming this phone's
     * own LAN, a literal address or a credential pair is refused before any
     * request leaves the device. Opening the player first to learn its length
     * would make that request ahead of the check that exists to prevent it, and
     * would buy a `duration_ms` a source both sides fetch from the same place
     * does not need.
     *
     * Throws whatever `together_start` throws, so the screen can say why. That
     * is deliberate: a refused URL that silently did nothing is the failure this
     * ordering exists to make visible.
     */
    fun startStream(
        context: Context,
        peer: String,
        peerLabel: String,
        content: uniffi.comrade_core.TogetherContent.Stream,
    ) {
        beginSession(context, peer, peerLabel)
        _queue.value = null
        sendOut("stream invitation") { ComradeCore.togetherStartTyped(peer, content) }
        _state.value = UiState.Live(
            peer = peer,
            peerLabel = peerLabel,
            // The host, not the URL — see [onInvited]. A feed that named a
            // recording gets its name instead.
            title = content.recording?.let { titleOf(it) } ?: hostOf(content.url),
            weLead = true,
            joined = false,
            ready = true,
            playing = false,
            positionMs = 0,
            // The player's to discover, like an embed's: `TogetherContent::Stream`
            // carries `duration_ms: None` when nobody loaded the URL first, which
            // is exactly the case the ordering above creates.
            durationMs = 0,
            status = Status.WaitingForThem,
            solo = alone,
        )
        startService()
        rememberPlayedStream(content.url, content.recording)
        openStreamPlayer(content.url)
    }

    /**
     * Accept a stream invitation — fetch the same URL they are fetching.
     *
     * Nothing is transferred between the two devices: the handover path (§9a) is
     * for a file one of us holds, and neither of us holds this. What makes it
     * safe to point a player at a peer-supplied string is that core already ran
     * `TogetherContent::admissible` on the way *in*, before this session existed;
     * [wantedStream] is that validated content, kept rather than re-derived.
     */
    fun joinStream(context: Context) {
        appContext = context.applicationContext
        val invited = _state.value as? UiState.Invited ?: return
        val content = wantedStream ?: return
        pairWith(invited)
        sendOut("join") { ComradeCore.togetherJoinTyped() }
        _state.value = UiState.Live(
            peer = invited.peer,
            peerLabel = invited.peerLabel,
            title = invited.title,
            weLead = false,
            joined = true,
            ready = true,
            playing = false,
            positionMs = 0,
            durationMs = 0,
            status = Status.Together,
        )
        startService()
        openStreamPlayer(content.url)
    }

    /**
     * Point the file player at a URL, and take its length when it arrives.
     *
     * Shared by both ends of a stream session because both do exactly this. The
     * length goes through [refreshLive] rather than into the [UiState.Live] that
     * was just built, for the same reason an embed's does: it is not known until
     * the player says, which is after the session opened.
     */
    private fun openStreamPlayer(url: String) {
        playingStreamUrl = url
        openPlayer(Uri.parse(url)) { durationMs -> refreshLive(durationMs = durationMs) }
    }

    /** Accept a YouTube invitation. Nothing to look for and nothing to ask for. */
    fun joinEmbed(context: Context) {
        appContext = context.applicationContext
        val invited = _state.value as? UiState.Invited ?: return
        val videoId = wantedVideoId ?: return
        pairWith(invited)
        openEmbed(videoId)
        sendOut("join") { ComradeCore.togetherJoinTyped() }
        _state.value = UiState.Live(
            peer = invited.peer,
            peerLabel = invited.peerLabel,
            title = invited.title,
            weLead = false,
            joined = true,
            ready = true,
            playing = false,
            positionMs = 0,
            durationMs = 0,
            status = Status.Together,
            picture = TogetherDecisions.EMBED_PICTURE,
            embed = true,
        )
        startService()
    }

    /**
     * Accept an invitation by following whatever this phone is **already**
     * playing — Spotify, a podcast app, VLC, whatever is installed.
     *
     * `docs/TOGETHER.md` §13. Comrade holds no player at all in this mode: it
     * reads the other app's published `MediaSession` and drives its transport,
     * which is what a car head unit does. That reaches every service at once
     * instead of one integration per vendor, and it needs no client id, no OAuth
     * and no vendored SDK.
     *
     * Returns why it could not start, or `null` when a session opened — the
     * screen needs to tell those apart, because "turn the permission on" and
     * "press play in your music app first" are different next steps and offering
     * the wrong one is worse than offering none.
     *
     * **The mode is [PlaybackModeDecision]'s to decide, not this function's.**
     * A file we hold is always ours even with an external session running, and a
     * file we do *not* hold is nothing yet rather than external — claiming an
     * external player there would start a session against whatever happened to
     * be on the phone, which is not what was invited.
     */
    fun followExternal(context: Context): FollowRefusal? {
        appContext = context.applicationContext
        val invited = _state.value as? UiState.Invited ?: return FollowRefusal.NoInvitation
        if (!MediaSessionAccess.hasAccess(context)) return FollowRefusal.NeedsAccess
        val ownership = PlaybackModeDecision.ownershipFor(
            contentKind = invitedKind,
            // This branch exists precisely because we do not hold it. If we did,
            // the file path would already have started and this would be the
            // wrong answer — see the rule above.
            haveOurCopy = false,
            externalSessionAvailable = MediaSessionAccess.anySession(
                context,
                android.os.SystemClock.elapsedRealtime(),
            ),
        )
        if (ownership != PlaybackOwnership.EXTERNAL) return FollowRefusal.NothingPlaying

        val p = ExternalSessionPlayer(context)
        p.setListener(externalListener)
        if (!p.bind()) return FollowRefusal.NothingPlaying
        pairWith(invited)
        player?.release()
        player = p
        _queue.value = null
        openedPath = null
        openedUri = null
        openedDurationMs = 0
        sendOut("join") { ComradeCore.togetherJoinTyped() }
        _state.value = UiState.Live(
            peer = invited.peer,
            peerLabel = invited.peerLabel,
            title = invited.title,
            weLead = false,
            joined = true,
            ready = true,
            playing = p.isPlaying,
            positionMs = p.positionMs,
            // A `MediaSession` carries no length we can trust, so the screen
            // shows a position without a bound rather than a bound we invented.
            durationMs = 0,
            status = Status.Together,
            // Nothing of ours to draw: the picture, if any, is in another app's
            // window. §14 calls this the price of reaching every service at once.
            external = true,
        )
        startService()
        return null
    }

    /** Why following what is playing could not start. Each needs a different sentence. */
    enum class FollowRefusal {
        /** Not holding an invitation any more. */
        NoInvitation,

        /** Notification-listener access has not been granted. Send them to settings. */
        NeedsAccess,

        /** Granted, and nothing is playing to follow. Tell them to press play. */
        NothingPlaying,
    }

    private val externalListener = object : ExternalSessionPlayer.Listener {
        override fun onStateChanged(posMs: Long, playing: Boolean) {
            sendOut("state") { ComradeCore.togetherSetStateTyped(posMs, playing, 0) }
            refreshLive(playing = playing, positionMs = posMs)
        }

        /**
         * They skipped, or the app moved on. The claim ends rather than
         * following into a different song and calling it "together".
         */
        override fun onTrackChanged(trackKey: String) {
            refreshLive(status = Status.LostTrack)
        }

        /**
         * Nothing to be done about it, so the screen says so and the session
         * stops claiming — a seek cannot hold two different playback speeds
         * together, because the gap reopens as fast as it closes.
         */
        override fun onSpeedsDisagree(ours: Float, theirs: Float) {
            Log.i(TAG, "speeds disagree: ours $ours, theirs $theirs")
            refreshLive(status = Status.LostTrack)
        }

        override fun onLost() {
            refreshLive(status = Status.LostTrack)
        }
    }

    /** Accept an invitation, once the user has opened their own copy. */
    fun join(context: Context, uri: Uri) {
        appContext = context.applicationContext
        val invited = _state.value as? UiState.Invited ?: return
        pairWith(invited)
        openPlayer(uri) { durationMs ->
            sendOut("join") { ComradeCore.togetherJoinTyped() }
            _state.value = UiState.Live(
                peer = invited.peer,
                peerLabel = invited.peerLabel,
                title = invited.title,
                weLead = false,
                joined = true,
                ready = true,
                playing = false,
                positionMs = 0,
                durationMs = durationMs,
                status = Status.Together,
                sourceUri = openedUri,
            )
            startService()
        }
    }

    /**
     * A local play/pause/seek the user asked for.
     *
     * Deferred by [SCHEDULE_AHEAD_MS] on both devices rather than applied here
     * and chased there: a few tens of milliseconds is imperceptible on a button
     * press, and it is what makes both playheads change on the *same* instant.
     */
    fun setState(posMs: Long, playing: Boolean) {
        val p = player ?: return
        sendOut("state") { ComradeCore.togetherSetStateTyped(posMs, playing, SCHEDULE_AHEAD_MS) }
        scope.launch {
            delay(SCHEDULE_AHEAD_MS)
            val now = System.currentTimeMillis()
            if (kotlin.math.abs(posMs - p.positionMs) > TogetherDecisions.EPSILON_MS) {
                suppressor.expect("seek", posMs, now)
                p.seekTo(posMs)
            }
            if (playing != p.isPlaying) {
                suppressor.expect(if (playing) "play" else "pause", null, now)
                if (playing) p.play() else {
                    p.pause()
                    saveQueueSnapshot()
                }
            }
            refreshLive(playing = playing)
        }
    }

    fun onScrubStart() {
        scrub = scrub.copy(scrubbing = true)
    }

    fun onScrubRelease(posMs: Long) {
        scrub = TogetherDecisions.onScrubRelease(scrub)
        setState(posMs, (_state.value as? UiState.Live)?.playing ?: false)
    }

    fun leave() {
        // Cleared first: this end is the user's, so the next `TogetherEnded`
        // must be acted on rather than swallowed as a replacement's.
        pendingSelfEnds = 0
        sendOut("leave") { ComradeCore.togetherEndTyped() }
        saveQueueSnapshot()
        stopPlayback()
        forgetPairing()
        _state.value = UiState.Idle
    }

    /** The pairing is over — not paused between tracks, over. */
    private fun forgetPairing() {
        _sendFailed.value = false
        _pairing.value = null
        pairingEndedAtMs = 0
        _queue.value = null
        cancelSleepTimer()
        extrasLoaded = false
        _extras.value = PlayerExtras(speed = _extras.value.speed, sleepEndsAtMs = null)
    }

    // ── The queue ───────────────────────────────────────────────────────────

    /**
     * Play a track from this phone's library to whoever we are paired with.
     *
     * The queue is the list it was picked out of, so prev and next mean the
     * list the person was looking at rather than the whole library.
     */
    fun playTrack(
        context: Context,
        pairing: TogetherDecisions.Pairing,
        track: TogetherDecisions.Track,
        queue: List<TogetherDecisions.Track>,
    ): Boolean = runCatching {
        start(
            context = context,
            peer = pairing.npub,
            peerLabel = pairing.label,
            uri = Uri.parse(track.uri),
            recording = MusicLibrary.recordingOf(track),
            queue = TogetherDecisions.queueFrom(queue, track.uri),
        )
    }.onFailure { Log.w(TAG, "could not play that track", it) }.isSuccess

    /** The next track in the queue, to the same person. A no-op at the end of it.
     *
     * Under shuffle "next" is the order's neighbour — [extras]'s `order` — not
     * the file's; repeat-one deliberately does NOT trap a manual skip
     * ([TogetherDecisions.manualNextIndex]). */
    fun skipForward(context: Context) {
        val pairing = _pairing.value ?: return
        val at = _queue.value ?: return
        val to = TogetherDecisions.manualNextIndex(extras.value.order, at.index, at.tracks.size) ?: return
        val moved = TogetherDecisions.movedTo(at, to) ?: return
        val track = moved.current ?: return
        playTrack(context, pairing, track, moved.tracks)
    }

    /**
     * Back — which is two different things, and
     * [TogetherDecisions.backStep] is what tells them apart.
     *
     * Restarting goes through [setState] like every other transport command, so
     * the other person follows it; moving to the previous track replaces the
     * content, exactly as [skipForward] does.
     */
    fun skipBack(context: Context) {
        val live = _state.value as? UiState.Live ?: return
        when (val step = TogetherDecisions.backStep(_queue.value, live.positionMs)) {
            TogetherDecisions.Back.Restart -> setState(0, live.playing)
            is TogetherDecisions.Back.Previous -> {
                val pairing = _pairing.value ?: return
                val at = _queue.value ?: return
                // Under shuffle the previous track is the order's previous;
                // the restart-within-seconds rule above still wins first.
                val to = if (extras.value.order != null && extras.value.order!!.size == at.tracks.size) {
                    TogetherDecisions.previousIndexWith(extras.value.order, at.index, at.tracks.size)
                } else {
                    step.index
                } ?: return
                val moved = TogetherDecisions.movedTo(at, to) ?: return
                val track = moved.current ?: return
                playTrack(context, pairing, track, moved.tracks)
            }
        }
    }

    // ── Player extras: shuffle, repeat, speed, sleep timer ──────────────────

    /**
     * The remembered-and-live shape of the player's own conveniences.
     *
     * One [StateFlow] rather than five scattered ones because every control
     * that reads one reads the button states of the others too — and because
     * prefs are the source of truth on disk while this flow is the source in
     * memory, they are loaded together at first use and written through
     * together on each toggle.
     */
    data class PlayerExtras(
        val shuffle: Boolean = false,
        val repeat: TogetherDecisions.RepeatMode = TogetherDecisions.RepeatMode.OFF,
        /** Live only when [shuffle] is on AND a queue exists; `null` otherwise. */
        val order: List<Int>? = null,
        val speed: Float = 1.0f,
        val sleepEndsAtMs: Long? = null,
        /**
         * Stop at the end of the track rather than at [sleepEndsAtMs] exactly.
         *
         * The complaint every duration-only sleep timer earns: thirty minutes
         * lands mid-song more often than not. In this mode the instant is a
         * floor rather than a deadline — see
         * [TogetherDecisions.sleepTimerDone].
         */
        val sleepEndOfTrack: Boolean = false,
    )

    private val _extras = kotlinx.coroutines.flow.MutableStateFlow(PlayerExtras())
    val extras: kotlinx.coroutines.flow.StateFlow<PlayerExtras> = _extras

    private var extrasLoaded = false

    /**
     * Read what was remembered. Called from the screen entering the player —
     * idempotent by construction, cheap by design.
     */
    fun loadExtras(context: Context) {
        if (extrasLoaded) return
        val shuffleOn = PlayerPrefs.shuffle(context)
        val order = rebuildOrder(shuffleOn)
        _extras.value = PlayerExtras(
            shuffle = shuffleOn,
            repeat = PlayerPrefs.repeat(context),
            order = order,
            speed = PlayerPrefs.speed(context),
            sleepEndsAtMs = _extras.value.sleepEndsAtMs,
        )
        extrasLoaded = true
        applySpeedIfAlone()
    }

    /** A fresh shuffle order over the live queue, keeping the current track first. */
    private fun rebuildOrder(shuffleOn: Boolean): List<Int>? {
        val at = _queue.value ?: return null
        if (!shuffleOn) return null
        return TogetherDecisions.shuffledOrder(at.tracks.size, at.index, kotlin.random.Random(System.nanoTime()))
    }

    fun toggleShuffle(context: Context) {
        val on = !_extras.value.shuffle
        PlayerPrefs.setShuffle(context, on)
        val order = rebuildOrder(on)
        _extras.value = _extras.value.copy(shuffle = on, order = order)
    }

    /** OFF → ALL → ONE → OFF, persisted per press. */
    fun cycleRepeat(context: Context) {
        val next = when (_extras.value.repeat) {
            TogetherDecisions.RepeatMode.OFF -> TogetherDecisions.RepeatMode.ALL
            TogetherDecisions.RepeatMode.ALL -> TogetherDecisions.RepeatMode.ONE
            TogetherDecisions.RepeatMode.ONE -> TogetherDecisions.RepeatMode.OFF
        }
        PlayerPrefs.setRepeat(context, next)
        _extras.value = _extras.value.copy(repeat = next)
    }

    /**
     * User-facing playback rate. **Solo only** — see
     * [TogetherDecisions.speedAllowed]; in a session the correction ladder owns
     * this dial, and a listener turning it would fight their own sync.
     */
    fun setSpeed(context: Context, rate: Float) {
        val clamped = TogetherDecisions.clampSpeed(rate)
        PlayerPrefs.setSpeed(context, clamped)
        _extras.value = _extras.value.copy(speed = clamped)
        applySpeedIfAlone()
    }

    private fun applySpeedIfAlone() {
        if (!alone) return
        val rate = _extras.value.speed
        if (rate != 1.0f) player?.setRate(rate)
    }

    /**
     * Pause when the timer runs out — a command to the session, so a peer's
     * device pauses too, which is what "sleep" means when two people fell
     * asleep watching.
     */
    /**
     * Set the timer, in either mode.
     *
     * **Polled rather than scheduled**, which is a change from the `delay` this
     * used to be and not merely a refactor: end-of-track cannot be scheduled at
     * all, because the instant it fires is not known when it is set — it
     * depends on where the playhead gets to. The wall-clock mode rides the same
     * poll for free, and loses nothing by it: the poll already runs while
     * anything is playing, and a timer that fires up to one tick late is a
     * timer nobody can perceive being late.
     *
     * [minutes] `0` with [endOfTrack] is "stop when this track ends", which is
     * the floor case [TogetherDecisions.sleepTimerDone] documents.
     */
    fun startSleepTimer(minutes: Int, endOfTrack: Boolean = false) {
        val endsAt = System.currentTimeMillis() + minutes * 60_000L
        _extras.value = _extras.value.copy(sleepEndsAtMs = endsAt, sleepEndOfTrack = endOfTrack)
    }

    fun cancelSleepTimer() {
        if (_extras.value.sleepEndsAtMs != null) {
            _extras.value = _extras.value.copy(sleepEndsAtMs = null, sleepEndOfTrack = false)
        }
    }

    /**
     * Ask the timer, once per poll, whether this is the moment.
     *
     * Pauses through [setState] so both devices stop together — which is what
     * "sleep" means when two people fell asleep watching, and unchanged from
     * the scheduled version.
     */
    private fun checkSleepTimer(p: SessionPlayer) {
        val endsAt = _extras.value.sleepEndsAtMs ?: return
        val done = TogetherDecisions.sleepTimerDone(
            endsAtMs = endsAt,
            nowMs = System.currentTimeMillis(),
            positionMs = p.positionMs,
            durationMs = p.durationMs,
            endOfTrack = _extras.value.sleepEndOfTrack,
        )
        if (!done) return
        cancelSleepTimer()
        setState(p.positionMs, false)
    }

    // ── The queue as something you can change ───────────────────────────────

    /**
     * Replace the live queue, keeping any shuffle order pointing at the same
     * songs it pointed at before.
     *
     * The carry-over is the whole reason this is one function rather than two
     * lines at each call site. [rebuildOrder] draws a *fresh* permutation, which
     * is correct when the person asks for shuffle and wrong after every
     * mutation: it would reshuffle what they are looking at each time they drag
     * a row. [TogetherDecisions.carryOverOrder] walks the old order by track
     * identity instead, so the up-next list they can see stays the up-next list
     * that plays.
     *
     * A count-changing mutation would in fact be caught anyway — `manualNextIndex`
     * and its siblings refuse an order whose size does not match the queue — but
     * a **reorder** preserves the count, so a stale same-size order there
     * resolves to a plausible, wrong index. That case has its own test next door
     * (`aReorderWithoutCarryingTheOrderCanNameTheWrongNextSong`), and this is
     * the call that keeps it from happening.
     */
    private fun applyQueue(next: TogetherDecisions.Queue?) {
        val before = _queue.value
        val order = _extras.value.order
        _queue.value = next
        if (order == null || before == null || next == null) return
        _extras.value = _extras.value.copy(
            order = TogetherDecisions.carryOverOrder(order, before.tracks, next.tracks),
        )
    }

    /** Put a track straight after the one playing. */
    fun playNext(track: TogetherDecisions.Track) {
        val at = _queue.value ?: return
        applyQueue(TogetherDecisions.playNext(at, track))
    }

    /** Put a track at the end of what is already lined up. */
    fun addToQueue(track: TogetherDecisions.Track) {
        val at = _queue.value ?: return
        applyQueue(TogetherDecisions.addToQueue(at, track))
    }

    /** Drag a row to a new place in the up-next list. */
    fun moveInQueue(from: Int, to: Int) {
        val at = _queue.value ?: return
        applyQueue(TogetherDecisions.moveInQueue(at, from, to))
    }

    /** Everything after the current track goes; what is playing keeps playing. */
    fun clearUpNext() {
        val at = _queue.value ?: return
        applyQueue(TogetherDecisions.clearUpNext(at))
    }

    /**
     * Take one row out of the queue.
     *
     * **Removing the row that is playing is a content change, not a list edit**,
     * and it is handled as one: the slot inherits whatever now sits at the same
     * position, and that track is started exactly as pressing next would start
     * it. Leaving the removed track playing under a queue that no longer
     * contains it is the alternative, and it is the one that reads as broken —
     * the up-next list would disagree with the sleeve above it.
     *
     * Emptying the queue entirely leaves the current track playing with no list
     * behind it, which is [TogetherDecisions.removeAt]'s `null` and the same
     * state a pasted link has always been in.
     */
    fun removeFromQueue(context: Context, at: Int) {
        val queue = _queue.value ?: return
        val wasCurrent = at == queue.index
        val next = TogetherDecisions.removeAt(queue, at)
        applyQueue(next)
        if (!wasCurrent || next == null) return
        val pairing = _pairing.value ?: return
        val track = next.current ?: return
        playTrack(context, pairing, track, next.tracks)
    }

    /** A tap on a row of the up-next list. */
    fun jumpTo(context: Context, at: Int) {
        val queue = _queue.value ?: return
        val pairing = _pairing.value ?: return
        val moved = TogetherDecisions.jumpTo(queue, at) ?: return
        val track = moved.current ?: return
        playTrack(context, pairing, track, moved.tracks)
    }

    // ── Loving what is on ───────────────────────────────────────────────────

    private val _loved = kotlinx.coroutines.flow.MutableStateFlow(false)

    /**
     * Whether the track playing right now is a favourite.
     *
     * A flow rather than a per-row lookup because the heart sits on the
     * now-playing sleeve, which is drawn continuously — and because until this
     * existed there was **no way to favourite anything at all**, while
     * `library_empty_favourites` had been telling people to "tap the heart on a
     * row in your library" since favourites shipped. The list could only ever
     * be empty.
     */
    val loved: kotlinx.coroutines.flow.StateFlow<Boolean> = _loved.asStateFlow()

    /**
     * What the vault calls the track on now, or `null` when there is nothing
     * nameable playing.
     *
     * The key convention is [rememberPlayed]'s, and deliberately the same one:
     * a track loved here and the same track in the history have to be one row,
     * or "loved" and "played" would disagree about what a track is.
     */
    private fun nowPlayingDto(): uniffi.comrade_ui.PlayerTrackDto? {
        _queue.value?.current?.let { t ->
            return uniffi.comrade_ui.PlayerTrackDto(
                key = "local:${t.uri}",
                title = t.title,
                artist = t.artist,
                album = t.album,
                durationMs = t.durationMs.toULong(),
                url = null,
                kind = uniffi.comrade_ui.PlayerTrackKind.LOCAL,
            )
        }
        val url = playingStreamUrl ?: return null
        val live = _state.value as? UiState.Live ?: return null
        return uniffi.comrade_ui.PlayerTrackDto(
            key = "stream:$url",
            title = live.title.ifBlank { hostOf(url) },
            artist = "",
            album = null,
            durationMs = live.durationMs.toULong(),
            url = url,
            kind = uniffi.comrade_ui.PlayerTrackKind.STREAM,
        )
    }

    /**
     * Ask the vault whether what is playing is loved, and say so.
     *
     * Best-effort exactly as [rememberPlayed] is: a locked vault throws here,
     * and an unanswerable heart draws as not-loved rather than failing the
     * screen around it. The one thing it must not do is *lie the other way* —
     * a false "loved" would make the toggle below remove something.
     */
    fun refreshLoved() {
        val dto = nowPlayingDto()
        if (dto == null) {
            _loved.value = false
            return
        }
        scope.launch(Dispatchers.IO) {
            _loved.value = runCatching { ComradeCore.favouriteIs(dto.key) }.getOrDefault(false)
        }
    }

    /** Toggle it, and render from what the vault says it now is. */
    fun toggleLoved() {
        val dto = nowPlayingDto() ?: return
        scope.launch(Dispatchers.IO) {
            runCatching { ComradeCore.favouriteToggle(dto) }
                .onSuccess { _loved.value = it }
                .onFailure { Log.w(TAG, "could not love that track", it) }
        }
    }

    /** Add what is playing to a named playlist. */
    fun addNowPlayingToPlaylist(playlistId: String) {
        val dto = nowPlayingDto() ?: return
        scope.launch(Dispatchers.IO) {
            runCatching { ComradeCore.playlistAddTrack(playlistId, dto) }
                .onFailure { Log.w(TAG, "could not add to that playlist", it) }
        }
    }

    /**
     * What a finished track does next **when nobody else is listening**.
     *
     * Auto-advance is a solo-player behaviour by definition here: in a session
     * the leader's completion is a playhead fact for the follower to follow,
     * not a decision, and both devices deciding independently would race. So
     * `alone` gates the whole function. Repeat-one with no queue at all still
     * restarts — a single track is its own queue of one.
     */
    private fun maybeAutoAdvance() {
        if (!alone || appContext == null) return
        val repeatMode = _extras.value.repeat
        val at = _queue.value
        val ctx = appContext ?: return
        if (at == null) {
            if (repeatMode == TogetherDecisions.RepeatMode.ONE && player?.prepared == true) {
                setState(0, true)
            }
            return
        }
        val next = TogetherDecisions.nextIndexOnEnd(repeatMode, _extras.value.order, at.index, at.tracks.size)
            ?: return  // end of listen; the session stays, stopped, as it always has.
        val pairing = _pairing.value ?: return
        val track = at.tracks.getOrNull(next) ?: return
        playTrack(ctx, pairing, track, at.tracks)
    }

    /**
     * Rebuild a session from the queue snapshot the vault kept.
     *
     * **Solo only**, and only from tracks that are still resolvable — a saved
     * row whose MediaStore entry was deleted since is skipped rather than
     * offered as a broken button. Streams are deliberately not resumed: their
     * URLs may have expired tokens or moved servers, and silently refetching
     * one is how a resume turns into an error page. Returns whether anything
     * actually started, so the screen can fall back to its ordinary sentence
     * when everything it remembered has since been deleted.
     */
    suspend fun resumeSavedQueue(context: Context): Boolean {
        if (!TogetherDecisions.mayChoosePerson(_pairing.value)) return false
        val saved = runCatching { ComradeCore.queueLoad() }.getOrNull() ?: return false
        if (saved.tracks.isEmpty()) return false

        val resolver = context.contentResolver
        val tracks = saved.tracks.mapNotNull { dto ->
            if (dto.kind != uniffi.comrade_ui.PlayerTrackKind.LOCAL) return@mapNotNull null
            val uri = dto.key.removePrefix("local:")
            val uriObj = android.net.Uri.parse(uri)
            val exists = when (uriObj.scheme) {
                null, "file" -> uriObj.path?.let { java.io.File(it).exists() } == true
                "content" -> runCatching {
                    resolver.query(uriObj, arrayOf(android.provider.MediaStore.Audio.Media._ID), null, null, null)
                        ?.use { it.count > 0 } == true
                }.getOrDefault(false)
                else -> false
            }
            if (!exists) return@mapNotNull null
            TogetherDecisions.Track(
                uri = uri,
                title = dto.title,
                artist = dto.artist,
                album = dto.album,
                durationMs = dto.durationMs.toLong(),
                albumId = null,
            )
        }
        if (tracks.isEmpty()) return false

        val index = saved.index.toInt().coerceIn(0, tracks.lastIndex)
        beginSession(context, TogetherDecisions.ALONE.npub, TogetherDecisions.ALONE.label)
        _queue.value = TogetherDecisions.Queue(tracks, index)
        _extras.value = _extras.value.copy(order = rebuildOrder(_extras.value.shuffle))
        rememberPlayedLocal(
            android.net.Uri.parse(tracks[index].uri),
            MusicLibrary.recordingOf(tracks[index]),
        )
        start(
            context = context,
            peer = TogetherDecisions.ALONE.npub,
            peerLabel = TogetherDecisions.ALONE.label,
            uri = android.net.Uri.parse(tracks[index].uri),
            recording = MusicLibrary.recordingOf(tracks[index]),
            queue = TogetherDecisions.Queue(tracks, index),
            resumeAtMs = saved.positionMs.toLong(),
        )
        return true
    }

    /**
     * The app went to the background.
     *
     * Playback deliberately **continues** — that is what the foreground service
     * is for — so this does nothing but exist as the documented answer to "does
     * leaving the app pause it?". The answer is no; the notification is how you
     * get back.
     */
    fun onAppBackgrounded() = Unit

    // ── Player plumbing ─────────────────────────────────────────────────────

    /**
     * Open the file **as it arrives**, so the session starts on the head of it
     * instead of waiting for the whole thing (`docs/TOGETHER.md` §12).
     *
     * Called when the transfer is armed rather than when it finishes. The player
     * is the ordinary one; only the bytes are different, and the two things that
     * make it work are elsewhere — `PartialFileDataSource` blocks the decoder on
     * chunks that have not landed, and [applyShareVerdict] below holds the
     * playhead when it should not be moving.
     *
     * A no-op when nothing is being received, which is every session where both
     * sides already have the file.
     */
    fun onSharedFileStreaming() {
        // Hopped onto the session's own scope rather than run inline: the caller
        // is the transfer, which arms a receive on `Dispatchers.IO`, and [player]
        // is otherwise only ever touched from here — by the poll, by every
        // incoming command and by every correction. One more thread writing it
        // would make a field that is currently single-threaded by construction
        // into one that merely looks that way.
        scope.launch { openStreamingPlayer() }
    }

    private fun openStreamingPlayer() {
        val ctx = appContext ?: return
        val source = ShareTransfer.streamingSource() ?: return
        val live = _state.value as? UiState.Live
        // The path stays null: a partial file in the cache is not something this
        // device can turn round and offer to somebody else, and `openedPath` is
        // exactly the flag that decides whether it tries.
        openedPath = null
        openPlayerWith(
            ctx,
            onReady = { durationMs ->
                if (live != null) _state.value = live.copy(ready = true, durationMs = durationMs)
            },
            feed = { it.open(source) },
        )
    }

    private fun openPlayer(uri: Uri, onReady: (Long) -> Unit) {
        val ctx = appContext ?: return
        // Remember the readable path, if there is one: it is the difference
        // between being able to hand this file over and only being able to
        // receive one.
        openedPath = if (uri.scheme == null || uri.scheme == "file") uri.path else null
        openedUri = uri.toString()
        openPlayerWith(ctx, onReady) { it.open(uri) }
    }

    // ── The player's own library: what this device just played ──────────────

    /**
     * Write one recently-played row — fire-and-forget, off the main thread,
     * and never allowed to fail a session over: the diary is the least
     * important thing happening while music starts.
     *
     * Duration is deliberately not passed even when known; history rows are
     * for finding something again, and `0` reads as "unknown" everywhere that
     * renders them. Embeds are skipped entirely: an eleven-character id with
     * no title is not a row anybody can recognise.
     */
    private fun rememberPlayed(
        key: String,
        kind: uniffi.comrade_ui.PlayerTrackKind,
        title: String,
        artist: String,
        album: String?,
        url: String?,
    ) {
        scope.launch(Dispatchers.IO) {
            runCatching {
                ComradeCore.historyRecord(
                    uniffi.comrade_ui.PlayerTrackDto(
                        key = key,
                        title = title,
                        artist = artist,
                        album = album,
                        durationMs = 0uL,
                        url = url,
                        kind = kind,
                    ),
                    System.currentTimeMillis(),
                )
            }.onFailure { Log.w(TAG, "history write skipped", it) }
        }
        // The heart belongs to the track, so it is re-asked exactly where the
        // diary is written: this is the one call every route to a new track
        // passes through, local and stream alike.
        refreshLoved()
    }

    private fun rememberPlayedLocal(uri: Uri, recording: uniffi.comrade_core.Recording?) {
        val rec = recording ?: return
        rememberPlayed(
            key = "local:$uri",
            kind = uniffi.comrade_ui.PlayerTrackKind.LOCAL,
            title = rec.title,
            artist = rec.artist,
            album = rec.album,
            url = null,
        )
    }

    private fun rememberPlayedStream(url: String, recording: uniffi.comrade_core.Recording?) {
        rememberPlayed(
            key = "stream:$url",
            kind = uniffi.comrade_ui.PlayerTrackKind.STREAM,
            title = recording?.title ?: hostOf(url),
            artist = recording?.artist.orEmpty(),
            album = recording?.album,
            url = url,
        )
    }

    /**
     * Snapshot the live queue into the vault, if there is one to snapshot.
     *
     * Called when playback pauses and when a session ends — the two moments a
     * "come back later" decision is actually made. Local tracks carry their
     * MediaStore identity so a future restore can find them again; stream rows
     * carry their URL, which is re-fetchable exactly as long as the server is.
     * Best-effort like [rememberPlayed]: persistence must never cost playback.
     */
    fun saveQueueSnapshot() {
        val at = _queue.value ?: return
        val tracks = at.tracks.map { t ->
            uniffi.comrade_ui.PlayerTrackDto(
                key = "local:${t.uri}",
                title = t.title,
                artist = t.artist,
                album = t.album,
                durationMs = t.durationMs.toULong(),
                url = null,
                kind = uniffi.comrade_ui.PlayerTrackKind.LOCAL,
            )
        }
        scope.launch(Dispatchers.IO) {
            runCatching {
                ComradeCore.queueSave(
                    uniffi.comrade_ui.SavedQueueDto(
                        tracks = tracks,
                        index = at.index.toUInt(),
                        positionMs = currentPositionMs().toULong(),
                        savedAtMs = System.currentTimeMillis().toULong(),
                    ),
                )
            }.onFailure { Log.w(TAG, "queue snapshot skipped", it) }
        }
    }

    /**
     * The file path's player, wherever its bytes come from.
     *
     * `feed` is the only difference between a file on this device and one still
     * arriving over the wire (`docs/TOGETHER.md` §12) — everything else, the
     * listener included, is `MediaPlayer` semantics that both share.
     */
    private fun openPlayerWith(
        ctx: Context,
        onReady: (Long) -> Unit,
        feed: (TogetherPlayer) -> Unit,
    ) {
        // This is the **file** path's construction, deliberately concrete: the
        // listener callbacks below are `MediaPlayer` semantics and do not
        // survive being made abstract, so a new mode gets a sibling of this
        // function rather than a generalisation of it (`docs/TOGETHER.md` §14).
        // The reuse arm therefore narrows: today [player] is only ever a
        // [TogetherPlayer], so `as?` never fails and this is the same reuse it
        // has always been. When a second implementation can occupy that field,
        // this arm needs to release what it is replacing before it overwrites
        // it — mode is fixed for a session
        // ([PlaybackModeDecision.mayChangeMidSession]), so that only happens if
        // that rule is broken.
        val p = (player as? TogetherPlayer) ?: TogetherPlayer(ctx).also { player = it }
        p.setListener(object : TogetherPlayer.Listener {
            override fun onPrepared(durationMs: Long) {
                requestAudioFocus()
                openedDurationMs = durationMs
                // Cleared here rather than where the player is constructed: a
                // handover replaces the source mid-session, and the failure of
                // the copy being replaced must not outlive the one that worked.
                _openFailed.value = false
                onReady(durationMs)
                startPolling()
            }

            override fun onSeekComplete(posMs: Long) {
                emitIfUserCaused("seek", posMs)
            }

            override fun onCompletion(posMs: Long) {
                emitIfUserCaused("completion", posMs)
                maybeAutoAdvance()
            }

            override fun onError(message: String) {
                Log.w(TAG, "player: $message")
                // Said on screen, not only in logcat. The session is left
                // running: the other person may still be playing their own copy,
                // and ending it from here would take the session away from them
                // because *our* source failed.
                _openFailed.value = true
            }

            override fun onBuffering(buffering: Boolean) {
                // Straight through: what it means for the session is the
                // screen's to say, and the drift ladder already handles a
                // playhead that has stopped moving. Nothing here pauses or
                // corrects — a stall that fixes itself in 300ms must not become
                // a command the other side has to apply.
                refreshLive(buffering = buffering)
            }

            override fun onVideoSize(width: Int, height: Int) {
                refreshLive(picture = TogetherDecisions.pictureOf(width, height))
                // The capture starts at a guess, because `MediaPlayer` only
                // knows its dimensions once the file is open. This is the
                // correction, and without it the outgoing picture is scaled to
                // whatever the guess was for the whole session.
                if (width > 0 && height > 0) {
                    videoCapturer?.changeCaptureFormat(width, height, STREAM_FPS)
                }
            }
        })
        feed(p)
        // The session id is minted inside `feed`'s `open`, so it exists only
        // now — one Equalizer instance per open, never per frame.
        PlayerEffects.attach(p.audioSessionId)
    }

    /**
     * The **embed** path's construction, and a sibling of [openPlayer] rather
     * than a generalisation of it (`docs/TOGETHER.md` §14).
     *
     * Nothing here is shared with the file path: there is no `Uri`, no surface,
     * no readable path to offer in a handover, and the callbacks are IFrame
     * semantics rather than `MediaPlayer` ones. Trying to fold the two together
     * would mean an abstraction over two sets of events that do not correspond.
     *
     * Whatever was playing is released first. The mode never changes mid-session
     * ([PlaybackModeDecision.mayChangeMidSession]), so in practice this only
     * ever replaces a *previous* session's player — but leaving the old one to
     * be garbage-collected would leave its `WebView` running audio in a window
     * nothing draws.
     */
    private fun openEmbed(videoId: String) {
        player?.release()
        openedPath = null
        openedUri = null
        openedDurationMs = 0
        playingVideoId = videoId
        _embedFailure.value = null
        val p = YoutubeSessionPlayer(videoId)
        p.setListener(object : YoutubeSessionPlayer.Listener {
            override fun onPrepared(durationMs: Long) {
                requestAudioFocus()
                refreshLive(durationMs = durationMs)
                startPolling()
            }

            /**
             * Straight out, with no echo suppression — and that is not an
             * oversight.
             *
             * The suppressor exists because a `MediaPlayer` reports back the
             * seeks and pauses *we* asked it for, so an apply would otherwise
             * re-broadcast itself between two devices. The IFrame player does
             * the same for state, which is why an apply arms nothing here: this
             * fires on the embed's own transitions, and [YoutubeSessionPlayer]
             * has already dropped the ones that are not worth sending
             * (`buffering`, `unstarted`) before it calls.
             */
            override fun onStateChanged(posMs: Long, playing: Boolean) {
                sendOut("state") { ComradeCore.togetherSetStateTyped(posMs, playing, 0) }
                refreshLive(playing = playing, positionMs = posMs)
            }

            /**
             * Said on screen, with a way out.
             *
             * It used to go to logcat and nowhere else, which left the session
             * sitting under YouTube's own "This video is unavailable" panel
             * still claiming to be waiting for the other person to open
             * something that was never going to open. The panel is theirs and
             * §11a is why we may not replace it — but this side's answer to it
             * is ours, and the common case (a video its owner does not allow
             * outside YouTube) has a genuinely useful next step.
             */
            override fun onError(message: String) {
                Log.w(TAG, "embed: $message")
                _embedFailure.value = TogetherDecisions.embedFailure(message)
            }
        })
        player = p
        // Nothing starts until the screen hands over a view: the session exists
        // before there is anywhere to draw it, exactly as a file session exists
        // before the surface arrives.
        startPolling()
    }

    /**
     * Start sending the picture and the sound of what this device is playing.
     *
     * `docs/TOGETHER.md` §15. The third answer to §9a's question — after *find
     * your own copy* and *take mine* — and the one that works when the other
     * person will never hold the file.
     *
     * Two things are set up and they are independent. The **picture**:
     * [PlayerVideoCapturer] takes the surface the player was decoding into, its
     * frames become a `VideoTrack`, and the sender watches that track rather
     * than the raw surface (see [localVideo]). The **sound**: [PlaybackCapture]
     * records this app's own playback and [AudioInjection] routes it into
     * WebRTC's record buffer in place of, or mixed with, the microphone.
     *
     * @param projection consent from `MediaProjectionManager.createScreenCaptureIntent`,
     *   needed for the audio half — capturing even *our own* playback requires
     *   one. Null starts the picture without the sound, which is a real state
     *   rather than a failure: someone who declines the dialog should still be
     *   able to show what they are watching.
     * @return whether the picture started. The sound is best-effort and
     *   [PlaybackCapture.start] says so on its own.
     */
    /**
     * Begin streaming from the system's capture-consent result.
     *
     * The **ordering** is the reason this exists rather than the screen doing
     * it: from Android 14 a `MediaProjection` may only begin while a foreground
     * service already declaring `mediaProjection` is running, so the service is
     * re-announced *before* the projection is fetched. Getting that round the
     * wrong way throws at capture, far from the code that caused it — the same
     * sequencing `CallManager.startScreenShare` depends on, kept in one place
     * here so no caller can get it wrong.
     *
     * A refused dialog is not a failure: `resultCode` other than `RESULT_OK`
     * streams the picture with no sound, which is a real thing to offer someone
     * who does not want to grant a recording consent for a film they are only
     * showing.
     */
    fun startStreamingFromConsent(context: Context, resultCode: Int, data: android.content.Intent?): Boolean {
        appContext = context.applicationContext
        val ctx = appContext ?: return false
        val projection = if (resultCode == android.app.Activity.RESULT_OK && data != null) {
            // Re-announce first. See above.
            if (!disableServiceForTest) {
                runCatching { TogetherService.startWithProjection(ctx) }
                    .onFailure { Log.w(TAG, "could not re-announce for projection", it) }
            }
            val manager = ctx.getSystemService(android.media.projection.MediaProjectionManager::class.java)
            runCatching { manager?.getMediaProjection(resultCode, data) }
                .onFailure { Log.w(TAG, "projection refused by the system", it) }
                .getOrNull()
        } else {
            null
        }
        return startStreaming(context, projection)
    }

    fun startStreaming(context: Context, projection: android.media.projection.MediaProjection?): Boolean {
        appContext = context.applicationContext
        val ctx = appContext ?: return false
        val player = this.player as? TogetherPlayer ?: return false
        val factory = mullu.comrade.call.CallManager.sharedFactory(ctx) ?: return false
        val egl = mullu.comrade.call.CallManager.eglBaseContext ?: return false
        if (videoCapturer != null) return true

        val helper = org.webrtc.SurfaceTextureHelper.create("TogetherCapture", egl)
        // `isScreencast = false` on the source as well as the capturer: this is
        // motion video, so it must degrade resolution and hold the frame rate
        // rather than the other way round.
        val source = factory.createVideoSource(false)
        val capturer = PlayerVideoCapturer()
        capturer.initialize(helper, ctx, source.capturerObserver)
        // A guess, corrected by `onVideoSize` as soon as the decoder reports —
        // which is why the listener forwards it.
        capturer.startCapture(1280, 720, STREAM_FPS)
        val surface = capturer.outputSurface
        if (surface == null) {
            runCatching { capturer.dispose() }
            runCatching { helper.dispose() }
            runCatching { source.dispose() }
            return false
        }
        // The player stops drawing to the screen and starts drawing to the
        // encoder. The sender sees the same frames back through [localVideo].
        player.attachSurface(surface)

        surfaceHelper = helper
        videoSource = source
        videoCapturer = capturer
        _localVideo.value = factory.createVideoTrack(STREAM_VIDEO_ID, source).apply { setEnabled(true) }

        if (projection != null) {
            val pc = PlaybackCapture()
            pc.micEnabled = _micEnabled.value
            if (pc.start(projection)) {
                pc.injecting = true
                capture = pc
                AudioInjection.install(pc)
            }
        }

        // The audio track exists whether or not the capture started: it is what
        // carries the sender's voice, and the microphone is worth sending even
        // when the film's sound is not going anywhere.
        //
        // Reused rather than rebuilt when a voice channel already opened one —
        // a second `AudioSource` over the same microphone is a leak of the
        // first, and there is exactly one outgoing audio track in every mode.
        val audioTrack = streamAudioTrack ?: run {
            val audioSource = factory.createAudioSource(org.webrtc.MediaConstraints())
            streamAudioSource = audioSource
            factory.createAudioTrack(STREAM_AUDIO_ID, audioSource).also { streamAudioTrack = it }
        }
        // Enabled unconditionally here, unlike the voice-only case: this track
        // carries the film's own sound, and `micEnabled` decides only whether
        // the voice is summed into it.
        runCatching { audioTrack.setEnabled(true) }
        StreamTransfer.localAudio = audioTrack

        if (!StreamTransfer.offer(ctx, _localVideo.value, audioTrack)) {
            Log.w(TAG, "could not offer the stream")
        }
        refreshLive(streaming = true)
        return true
    }

    /** Tear the outgoing picture down. Safe to call twice, and from any thread. */
    private fun stopVideoCapture() {
        StreamTransfer.end()
        // Before the track is disposed: a connection negotiated later must not
        // be handed one that has been released.
        StreamTransfer.localAudio = null
        _remoteVideo.value = null
        streamAudioTrack?.let { runCatching { it.dispose() } }
        streamAudioTrack = null
        streamAudioSource?.let { runCatching { it.dispose() } }
        streamAudioSource = null
        // The player first: a decoder still drawing into a surface whose
        // SurfaceTexture has gone is the use-after-free in the media server
        // that `attachSurface(null)` exists to prevent.
        (player as? TogetherPlayer)?.attachSurface(null)
        _localVideo.value?.let { runCatching { it.dispose() } }
        _localVideo.value = null
        videoCapturer?.let { runCatching { it.dispose() } }
        videoCapturer = null
        videoSource?.let { runCatching { it.dispose() } }
        videoSource = null
        surfaceHelper?.let { runCatching { it.dispose() } }
        surfaceHelper = null
    }

    /**
     * Hand the embed the view the screen built, or take it away.
     *
     * The twin of [attachSurface], narrowed for the same reason and with the
     * same lifetime: the view is destroyed and rebuilt on every rotation while
     * the session must survive both. For a file session this is correctly
     * nothing at all.
     */
    fun attachEmbedView(
        view: com.pierfrancescosoffritti.androidyoutubeplayer.core.player.views.YouTubePlayerView?,
    ) {
        (player as? YoutubeSessionPlayer)?.attach(view)
    }

    /**
     * Core asking us to carry a signal over the direct peer channel.
     *
     * **Nothing arrives here yet, by construction.** Core only emits an outbound
     * signal after a frontend has reported a live channel via
     * `togetherDirectReady(true)`, and this frontend never does — a session-long
     * peer connection is not built here yet, only the file-handover one that
     * lives for the length of a transfer. So this drops, and says so, rather
     * than looking like a wired path that silently loses signals.
     *
     * When the connection lands this becomes a `send` on it, and the only other
     * thing it must do is report `togetherDirectReady(false)` the moment the
     * channel closes — there is no timeout behind that flag, so a stale `true`
     * would send every signal into a socket nobody reads and let the session die
     * on its TTL instead of falling back to the relay.
     */
    fun onOutbound(json: String) {
        Log.d(TAG, "dropping a direct together signal: no session channel on this frontend (${json.length}B)")
    }

    /**
     * Hand the player the window to draw into, or take it away.
     *
     * Called by the screen as its surface is created and destroyed — which
     * happens on every rotation, independently of the session. The player holds
     * the last value, so the order the two arrive in does not matter.
     *
     * Narrowed rather than lifted onto [SessionPlayer]: a surface is meaningful
     * only to the player that decodes into one. An embed draws into a `WebView`
     * we host and an external session draws in another app's window, so for
     * those this is correctly nothing at all rather than an override that has to
     * pretend (`docs/TOGETHER.md` §14).
     */
    fun attachSurface(surface: android.view.Surface?) {
        (player as? TogetherPlayer)?.attachSurface(surface)
    }

    /** The one place a player callback becomes an outbound signal, or does not. */
    private fun emitIfUserCaused(kind: String, posMs: Long) {
        val p = player ?: return
        val emit = TogetherDecisions.classifyCallback(
            kind,
            posMs,
            p.isPlaying,
            suppressor,
            System.currentTimeMillis(),
        ) ?: return
        sendOut("state") { ComradeCore.togetherSetStateTyped(emit.posMs, emit.playing, 0) }
    }

    /**
     * Feed the core our playhead and our output-latency estimate.
     *
     * This sends nothing — the ten-second wire heartbeat is the Rust side's job.
     * It only keeps the next drift verdict comparing against something true.
     */
    private fun startPolling() {
        pollJob?.cancel()
        pollJob = scope.launch {
            while (true) {
                val p = player ?: break
                // Before the read, not after: a player that is told where it is
                // on somebody else's schedule needs a clock that keeps running
                // when the reports stop, and this poll is it. See
                // [SessionPlayer.onPoll].
                p.onPoll(System.currentTimeMillis())
                applyShareVerdict(p)
                checkSleepTimer(p)
                ComradeCore.togetherReportPosition(p.positionMs, p.isPlaying, p.outputLatencyMs)
                if (TogetherDecisions.pollMayMoveSlider(scrub)) {
                    refreshLive(positionMs = p.positionMs)
                }
                delay(TogetherDecisions.POLL_MS)
            }
        }
    }

    /**
     * Hold the playhead when the bytes ran out, and let it go when they arrive.
     *
     * `docs/TOGETHER.md` §12. A no-op in every session where this device is not
     * *receiving* a file — [ShareTransfer.readVerdictAt] answers `null` and
     * nothing here runs.
     *
     * **The whole trap is in what a hold is expressed as.** It pauses the local
     * player and nothing else: the very next line of the poll reports
     * `together_report_position(pos, playing = false, latency)`, a heartbeat,
     * which the peer's next `sync_verdict` reads as "they are not playing" and
     * answers by holding rather than correcting. It is **not**
     * `together_set_state(.., playing: false, ..)` — that is a command, it takes
     * the next sequence number, and it would pause the other person because our
     * download fell behind. That is precisely the ping-pong §10 rules out, and
     * it is the single easiest thing to get wrong here.
     *
     * Resuming is deliberately conditional on the *session* wanting to play.
     * `Start` is permission, not an instruction: a player the person paused, or
     * one a `pause` command from the peer paused, must stay paused however many
     * bytes arrive.
     */
    private fun applyShareVerdict(p: SessionPlayer) {
        val verdict = ShareTransfer.readVerdictAt(p.positionMs, p.isPlaying) ?: return
        val wantsToPlay = (_state.value as? UiState.Live)?.playing ?: false
        when (verdict) {
            ShareReadPolicy.Verdict.HOLD -> if (p.isPlaying) p.pause()
            ShareReadPolicy.Verdict.START -> if (wantsToPlay && !p.isPlaying) p.play()
            // Already running with enough in hand. Nothing to do — and nothing
            // to say, which is the point of there being no fourth arm.
            ShareReadPolicy.Verdict.CONTINUE -> Unit
        }
    }

    private fun refreshLive(
        playing: Boolean? = null,
        streaming: Boolean? = null,
        buffering: Boolean? = null,
        positionMs: Long? = null,
        /**
         * Only an embed passes this.
         *
         * A file session knows its length the moment the decoder prepares, and
         * sets it when the [UiState.Live] is built — so this parameter did not
         * exist until a player turned up whose length arrives *after* the
         * session opens. A YouTube duration is the player's to report and
         * `TogetherContent::duration_ms` returns `None` for one, so the scrubber
         * starts at zero and grows when the embed says.
         */
        durationMs: Long? = null,
        status: Status? = null,
        picture: TogetherDecisions.Picture? = null,
        driftMs: Long? = null,
        qualityMs: Long? = null,
        correctedAtMs: Long? = null,
    ) {
        val live = _state.value as? UiState.Live ?: return
        _state.value = live.copy(
            playing = playing ?: live.playing,
            streaming = streaming ?: live.streaming,
            buffering = buffering ?: live.buffering,
            positionMs = positionMs ?: live.positionMs,
            durationMs = durationMs ?: live.durationMs,
            status = status ?: live.status,
            picture = picture ?: live.picture,
            driftMs = driftMs ?: live.driftMs,
            qualityMs = qualityMs ?: live.qualityMs,
            correctedAtMs = correctedAtMs ?: live.correctedAtMs,
        )
    }

    /**
     * @param replacing whether the session is being swapped for another with the
     *   same person, rather than genuinely ending. The difference is the
     *   microphone: it is the user's standing choice about the room they are
     *   in, not a property of whichever track happens to be on, so pressing next
     *   must not silently mute them.
     */
    private fun stopPlayback(replacing: Boolean = false) {
        pollJob?.cancel()
        pollJob = null
        PlayerEffects.detach()
        player?.release()
        player = null
        suppressor.clear()
        // The capture holds an `AudioRecord` and a thread; a session ending
        // without releasing it leaves both running against a projection the
        // user thinks they have finished with.
        //
        // Uninstalled from the process-wide module **first**: a stale capture
        // left routed there would go on mixing a released player's audio into
        // whatever call came next.
        AudioInjection.install(null)
        capture?.stop()
        capture = null
        if (!replacing) _micEnabled.value = false
        stopVideoCapture()
        wanted = null
        wantedMs = 0
        wantedVideoId = null
        wantedStream = null
        playingVideoId = null
        playingStreamUrl = null
        invitedKind = ""
        _openFailed.value = false
        _embedFailure.value = null
        scrub = TogetherDecisions.ScrubState(scrubbing = false, pendingRemoteMs = null)
        _loved.value = false
        abandonAudioFocus()
        stopService()
    }

    // ── Audio focus ─────────────────────────────────────────────────────────

    /**
     * Required even though playback is foreground-service backed: without it an
     * incoming phone call plays over the film for one person and not the other,
     * and the two silently diverge. Losing focus pauses **and tells the peer**,
     * so what they see is "they paused" rather than an unexplained drift.
     */
    /**
     * What a focus change means, remembered across changes.
     *
     * Rebuilt with the request rather than held for the app's life: a new
     * session has not paused anything, and carrying an armed "we paused this"
     * across sessions is a resume firing on the wrong film.
     */
    private var focusWatch = TogetherDecisions.FocusWatch()

    private fun requestAudioFocus() {
        val ctx = appContext ?: return
        val am = ctx.getSystemService(Context.AUDIO_SERVICE) as? AudioManager ?: return
        focusWatch = TogetherDecisions.FocusWatch()
        val listener = AudioManager.OnAudioFocusChangeListener { change ->
            onFocusChange(change)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val req = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
                .setAudioAttributes(
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_MEDIA)
                        .setContentType(AudioAttributes.CONTENT_TYPE_MOVIE)
                        .build()
                )
                .setOnAudioFocusChangeListener(listener)
                .build()
            focusRequest = req
            am.requestAudioFocus(req)
        }
    }

    /**
     * One focus change, decided by [TogetherDecisions.FocusWatch] and applied
     * here.
     *
     * The decision is next door rather than inline because the bit that matters
     * cannot be recomputed from anything visible at this moment: **a gain may
     * resume only playback that a transient loss paused, never playback the
     * person paused deliberately.** The old listener here could not tell those
     * apart — it treated `LOSS` and `LOSS_TRANSIENT` identically and never
     * resumed at all, so a phone call stopped the music permanently and a
     * navigation prompt did too.
     *
     * Pausing and resuming go through [setState], so the other person follows
     * them, exactly as the previous version did and for the reason its comment
     * gave: what they see is "they paused" rather than an unexplained drift.
     * **Ducking deliberately does not** — it changes this device's output level
     * and nothing about the session, because a car's navigation prompt is not
     * an event in someone else's living room.
     */
    private fun onFocusChange(change: Int) {
        val p = player ?: return
        val kind = when (change) {
            AudioManager.AUDIOFOCUS_GAIN -> TogetherDecisions.FocusChange.GAIN
            AudioManager.AUDIOFOCUS_LOSS -> TogetherDecisions.FocusChange.LOSS
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> TogetherDecisions.FocusChange.LOSS_TRANSIENT
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK ->
                TogetherDecisions.FocusChange.LOSS_TRANSIENT_CAN_DUCK
            else -> return
        }
        when (val outcome = focusWatch.focusAction(kind, p.isPlaying)) {
            is TogetherDecisions.FocusOutcome.None -> Unit
            is TogetherDecisions.FocusOutcome.PauseAndRemember -> setState(p.positionMs, false)
            is TogetherDecisions.FocusOutcome.PauseForever -> setState(p.positionMs, false)
            is TogetherDecisions.FocusOutcome.Duck -> p.setVolume(outcome.volume)
            is TogetherDecisions.FocusOutcome.Unduck -> p.setVolume(1f)
            is TogetherDecisions.FocusOutcome.Resume -> {
                p.setVolume(1f)
                setState(p.positionMs, true)
            }
        }
    }

    /**
     * The headphones came out, or the Bluetooth speaker dropped.
     *
     * Registered by [TogetherService], which owns the receiver because it owns
     * the lifetime this has to match. Pauses through [setState] like everything
     * else, which in a paired session stops the other person too —
     * [TogetherDecisions.becomingNoisyAction] carries the argument for why that
     * is the right answer here and not merely the convenient one.
     */
    fun onBecomingNoisy() {
        val p = player ?: return
        if (!TogetherDecisions.becomingNoisyAction(p.isPlaying, alone)) return
        setState(p.positionMs, false)
    }

    private fun abandonAudioFocus() {
        val ctx = appContext ?: return
        val am = ctx.getSystemService(Context.AUDIO_SERVICE) as? AudioManager ?: return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            focusRequest?.let { am.abandonAudioFocusRequest(it) }
        }
        focusRequest = null
    }

    // ── The foreground service ──────────────────────────────────────────────

    private fun startService() {
        // Before the `disableServiceForTest` bail, and outside it: an answered
        // invitation must leave the shade whether or not this build runs the
        // service, and every route out of `Invited` passes through here.
        appContext?.let { Notifier.clearTogetherInvite(it) }
        // A voice channel rides the session envelope, so replacing what is
        // playing takes it with it. Put back here, the one point every live
        // route passes through, so talking survives pressing next. A no-op
        // whenever the microphone is off, which is its default.
        reopenVoice()
        if (disableServiceForTest) return
        val ctx = appContext ?: return
        TogetherService.start(ctx)
    }

    private fun stopService() {
        // An invitation nobody answered goes with the session it belonged to —
        // a "wants to listen with you" left in the shade after they gave up is
        // an invitation to join something that is no longer there.
        appContext?.let { Notifier.clearTogetherInvite(it) }
        if (disableServiceForTest) return
        val ctx = appContext ?: return
        TogetherService.stop(ctx)
    }

    /**
     * How far ahead a local command is scheduled.
     *
     * Imperceptible on a button press, and comfortably more than a local-network
     * round trip — so on the mesh both devices genuinely change state on the same
     * instant. Over a relay the other side will usually receive it late and
     * project instead, which is still correct, just not simultaneous.
     */
    const val SCHEDULE_AHEAD_MS: Long = 80

    private const val TAG = "TogetherManager"

    /** Track id for the outgoing picture of a streamed session. */
    private const val STREAM_VIDEO_ID = "comrade_together_video"

    /** Track id for its sound — the film, the sender's voice, or both. */
    private const val STREAM_AUDIO_ID = "comrade_together_audio"

    /**
     * The frame rate the capture is *configured* at, not one it enforces.
     *
     * Nothing here polls: frames arrive when the decoder draws them, so a
     * 24 fps film produces 24 fps whatever this says. It is the hint WebRTC's
     * encoder budgets against, and 30 covers the common cases without asking
     * for headroom a phone would spend.
     */
    private const val STREAM_FPS = 30
}
