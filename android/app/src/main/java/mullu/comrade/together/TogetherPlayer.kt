package mullu.comrade.together

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioManager
import android.media.MediaPlayer
import android.media.PlaybackParams
import android.net.Uri
import android.util.Log
import android.view.Surface

/**
 * The player half of watch-together, behind a small interface so swapping the
 * implementation later is one file.
 *
 * **`MediaPlayer`, not ExoPlayer/Media3**, and the deciding detail is
 * [MediaPlayer.SEEK_CLOSEST]: the plain `seekTo(int)` seeks to the nearest sync
 * frame, so on a video with 5-10 s keyframe spacing "seek to 42.0" lands at 38.2
 * — a sync failure that looks like a working feature. The precise overload needs
 * API 26 and `minSdk` is 26, so it is simply available. Media3 would add ~2 MB
 * for adaptive streaming this feature does not use, in a repo that inlines icons
 * rather than take `material-icons-extended`.
 *
 * ## Output latency, stated honestly
 * What a listener hears is the decoder position minus this device's audio output
 * latency — 20-100 ms on Android, and different between handsets. Two players
 * agreeing perfectly on decoder position can still be a tenth of a second apart
 * in the room, which is the error no browser-based watch-party can even see.
 *
 * `AudioTrack.getTimestamp()` measures it properly, but `MediaPlayer` does not
 * hand out its `AudioTrack`, so [outputLatencyMs] here is an **estimate** from
 * the device's own low-latency buffer properties, not a measurement, and it is
 * documented as such rather than dressed up. Two things make it useful anyway:
 * it is a real per-device number rather than a constant, and what the sync
 * arithmetic actually uses is the *difference* between the two sides, which is
 * more accurate than either absolute figure. A true measurement needs an
 * `AudioTrack` we own — i.e. a Media3 migration — and is the honest follow-up.
 */
class TogetherPlayer(private val context: Context) : SessionPlayer {

    interface Listener {
        fun onPrepared(durationMs: Long)
        fun onSeekComplete(posMs: Long)
        fun onCompletion(posMs: Long)
        fun onError(message: String)

        /**
         * The decoder's picture size, `0x0` for a recording with no video track.
         * Fires before the first frame and again if the track changes size.
         */
        fun onVideoSize(width: Int, height: Int)

        /**
         * The decoder ran out of buffered bytes, or got some again.
         *
         * **Nothing listened to this before**, so a stream that stalled looked
         * exactly like one that had broken: the transport stayed on "playing", the
         * playhead stopped, and the only visible consequence was the other side
         * pulling ahead until the drift ladder started issuing corrections
         * nothing could apply. A default implementation, because a session on a
         * local file cannot buffer and has no reason to care.
         */
        fun onBuffering(buffering: Boolean) {}
    }

    private var player: MediaPlayer? = null
    private var listener: Listener? = null

    /**
     * The audio session this player outputs through, when a player exists.
     *
     * [PlayerEffects] attaches equalization here — one effect instance per
     * session id, re-attached on every `open`, because `MediaPlayer()` mints a
     * fresh session each time. Cleared on release like everything else the
     * decoder owned.
     */
    var audioSessionId: Int? = null
        private set

    /**
     * The window to draw into, held here because it and the player have
     * independent lifetimes: the surface is destroyed and recreated on every
     * rotation and every trip through the background, while the player must
     * survive both — losing the film at a rotation is not a fix for losing the
     * picture. Whichever arrives second re-attaches to the first.
     */
    private var surface: Surface? = null

    /** `seekTo` before `prepare()` throws — every caller checks this first. */
    override var prepared: Boolean = false
        private set

    fun setListener(listener: Listener?) {
        this.listener = listener
    }

    /**
     * Give the decoder somewhere to put the picture, or take it away again.
     *
     * **Without this call `MediaPlayer` decodes video to nowhere and the film
     * plays as sound only** — which is exactly what shipped, because nothing
     * ever handed it a surface. It is safe before `prepare()` and safe while
     * playing.
     *
     * Passing `null` is not optional tidiness: a `Surface` that has been
     * destroyed while the decoder still holds it is a use-after-free in the
     * media server, so the view detaching *must* clear it.
     */
    fun attachSurface(surface: Surface?) {
        this.surface = surface
        runCatching { player?.setSurface(surface) }
            .onFailure { Log.w(TAG, "could not attach surface", it) }
    }

    override val positionMs: Long
        get() = if (prepared) (player?.currentPosition ?: 0).toLong() else 0L

    override val durationMs: Long
        get() = if (prepared) (player?.duration ?: 0).toLong() else 0L

    override val isPlaying: Boolean
        get() = runCatching { player?.isPlaying == true }.getOrDefault(false)

    /**
     * Open a file that is **still arriving**, so a session can start on the head
     * of it rather than waiting for the whole thing.
     *
     * `docs/TOGETHER.md` §12. Everything about the player is identical to
     * [open]; the only difference is where the bytes come from, and
     * `setDataSource(MediaDataSource)` is genuinely the one-line change §12
     * predicted it would be. What is *not* one line is the source itself —
     * `PartialFileDataSource` blocks the decoder's thread on bytes that have not
     * landed, and the session holds the playhead alongside it.
     *
     * API 23+, and `minSdk` is 26, so the overload is simply available.
     */
    fun open(source: android.media.MediaDataSource) = openWith { it.setDataSource(source) }

    fun open(uri: Uri) = openWith { it.setDataSource(context, uri) }

    private fun openWith(feed: (MediaPlayer) -> Unit) {
        release()
        prepared = false
        val mp = MediaPlayer().apply {
            setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MOVIE)
                    .build()
            )
            setOnPreparedListener {
                this@TogetherPlayer.prepared = true
                listener?.onPrepared(it.duration.toLong())
            }
            setOnSeekCompleteListener { listener?.onSeekComplete(it.currentPosition.toLong()) }
            // How the screen learns there is a picture at all. `0x0` here is
            // the audio-only answer, and the screen draws no surface for it
            // rather than a black rectangle over someone's album.
            setOnVideoSizeChangedListener { _, width, height ->
                listener?.onVideoSize(width, height)
            }
            setOnCompletionListener { listener?.onCompletion(it.currentPosition.toLong()) }
            setOnErrorListener { _, what, extra ->
                listener?.onError("player error $what/$extra")
                true
            }
            // The only signal Android gives for a stall. `MEDIA_INFO_BUFFERING_*`
            // arrive on the player's own thread and are advisory — a stall may
            // end with `BUFFERING_END`, or simply by playback resuming with no
            // second event at all, which is why the screen must treat this as a
            // hint and never as a state it waits on.
            setOnInfoListener { _, what, _ ->
                when (what) {
                    MediaPlayer.MEDIA_INFO_BUFFERING_START -> listener?.onBuffering(true)
                    MediaPlayer.MEDIA_INFO_BUFFERING_END -> listener?.onBuffering(false)
                }
                // False: these are informational, and claiming to have handled
                // them suppresses nothing useful.
                false
            }
        }
        player = mp
        audioSessionId = runCatching { mp.audioSessionId }.getOrNull()
        // Re-attach whatever the view already gave us: `open` is also how a
        // handed-over copy replaces the file mid-session, and the surface from
        // before that swap is still on screen.
        runCatching { mp.setSurface(surface) }
            .onFailure { Log.w(TAG, "could not attach surface", it) }
        runCatching {
            feed(mp)
            // Asynchronous: prepare() blocks, and a large local file on slow
            // storage would block whatever thread opened it. For a partial file
            // it is not merely slow but *unbounded* — preparing reads the header
            // and may wait for chunks that have not arrived — so this is the
            // difference between a session that starts late and a frozen UI.
            mp.prepareAsync()
        }.onFailure {
            Log.w(TAG, "could not open media", it)
            listener?.onError(it.message ?: "could not open this file")
        }
    }

    override fun play() {
        if (!prepared) return
        runCatching { player?.start() }.onFailure { Log.w(TAG, "start failed", it) }
    }

    override fun pause() {
        if (!prepared) return
        runCatching { player?.pause() }.onFailure { Log.w(TAG, "pause failed", it) }
    }

    /**
     * Seek to an exact position, not to the nearest keyframe — see the class
     * comment. This is the single call that decides whether the feature works.
     */
    override fun seekTo(posMs: Long) {
        if (!prepared) return
        runCatching {
            player?.seekTo(posMs, MediaPlayer.SEEK_CLOSEST)
        }.onFailure { Log.w(TAG, "seek failed", it) }
    }

    /**
     * Trim the playback rate to close a small gap.
     *
     * Only ever called while playing: `setPlaybackParams` on a paused player
     * *starts* it on several Android versions, which would turn a drift
     * correction into an unasked-for resume. The guard is in
     * [TogetherDecisions.planCorrection] and tested there; this is the second
     * line of it.
     */
    override fun setRate(rate: Float) {
        if (!prepared || !isPlaying) return
        runCatching {
            val mp = player ?: return
            mp.playbackParams = PlaybackParams().setSpeed(rate.coerceIn(TogetherDecisions.RATE_MIN, TogetherDecisions.RATE_MAX))
        }.onFailure { Log.w(TAG, "rate trim failed", it) }
    }

    override fun release() {
        // Drop the surface before the player goes: the view may outlive this,
        // and a released player still holding it is the crash above.
        runCatching { player?.setSurface(null) }
        runCatching { player?.release() }
        player = null
        prepared = false
        audioSessionId = null
    }

    /**
     * An **estimate** of how far behind [positionMs] the sound actually leaves
     * the speaker — see the class comment for why this is not a measurement.
     *
     * Two output buffers' worth is the usual rule of thumb for the mixer path;
     * a device that reports nothing gets 0, which reads as "unmeasured" on the
     * wire and costs only the accuracy it cannot supply. It never guesses a
     * constant, because a wrong number applied confidently is worse than an
     * absent one the arithmetic knows to ignore.
     */
    override val outputLatencyMs: Long
        get() {
            val am = context.getSystemService(Context.AUDIO_SERVICE) as? AudioManager ?: return 0
            val frames = am.getProperty(AudioManager.PROPERTY_OUTPUT_FRAMES_PER_BUFFER)?.toIntOrNull() ?: return 0
            val rate = am.getProperty(AudioManager.PROPERTY_OUTPUT_SAMPLE_RATE)?.toIntOrNull() ?: return 0
            if (frames <= 0 || rate <= 0) return 0
            return (2L * frames * 1000L) / rate
        }

    private companion object {
        const val TAG = "TogetherPlayer"
    }
}
