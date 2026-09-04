package mullu.comrade.together

import android.app.Notification
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.graphics.Bitmap
import android.media.AudioManager
import android.media.MediaMetadata
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.Build
import android.os.IBinder
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import mullu.comrade.MainActivity
import mullu.comrade.Notifier
import mullu.comrade.R

/**
 * Keeps playback alive and visible while a watch-together session is running,
 * so putting the phone down or switching apps does not stop the film for both
 * people.
 *
 * Holds only the foreground-service contract — [TogetherManager] still owns the
 * player and every sync decision, and is also what starts and stops this, from
 * the same points it opens and releases the player rather than from any Compose
 * tree. A session must not end because a screen was disposed.
 *
 * ## Why there is a `MediaSession` and why it is the framework one
 * On Android 14 a `mediaPlayback` foreground service is expected to look like
 * media playback, and the platform routes hardware media keys through a session.
 * `androidx.media3.session` would do this too and bring ~2 MB of adaptive
 * streaming this feature does not use — so this is `android.media.session`, in
 * the framework since API 21, costing no dependency at all. The repo declines
 * dependencies for considerably less.
 *
 * The notification is built with the **framework** [Notification.Builder] and
 * [Notification.MediaStyle] rather than `NotificationCompat`, for the same
 * reason and one more: `MediaStyle` lives in `androidx.media:media`, which this
 * app does not depend on, and adding it would mean adding it to *two* Gradle
 * files (`app/` compiles `android/`'s Kotlin — see CLAUDE.md's traps). The
 * framework style needs API 21 and `minSdk` is 26, so it is simply available.
 * What `MediaStyle` buys is not decoration: it is what makes the platform draw
 * this as media — the artwork, the compact-view transport, and the scrubber
 * Android 13+ renders from the session's own metadata and playback state.
 *
 * ## What the session must not advertise
 * [PlaybackState] actions and the [MediaSession.Callback] are one contract in
 * two halves. A state advertising `ACTION_SEEK_TO` that the callback ignores
 * gives a car head unit a scrubber that does nothing, so both halves are
 * derived from the same [TogetherDecisions] answers rather than written twice.
 *
 * ## The promotion contract
 * `startForeground` happens in [onCreate], before any intent is examined. That
 * is not defensive style, it is the rule `.claude/rules/android.md` records in
 * blood: the obligation arms per *call* to `startForegroundService`, a refused
 * promotion does not cancel the pending kill, and `runCatching` at the call site
 * cannot catch a throw that happens later on this service's own looper. The only
 * safe move is to make promotion succeed immediately and refine the notification
 * in place afterwards.
 */
class TogetherService : Service() {

    /**
     * Whether this service has been re-announced as carrying a projection.
     *
     * A field rather than a parameter because [promote] is called again on every
     * start command and from [onCreate], and each of those has to announce the
     * *same* set of types — dropping `mediaProjection` from a later promotion
     * while a capture is running is how a projection dies mid-session.
     */
    private var projecting = false

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var stateJob: Job? = null
    private var session: MediaSession? = null

    /**
     * The last shade actually posted, so an unchanged one is not posted again.
     *
     * [TogetherManager.state] re-emits on every poll tick. Rebuilding a
     * notification at that rate is a battery and jank bug that buys nothing:
     * the playhead travels in the [PlaybackState], which the platform
     * extrapolates from its own timestamp without a re-post.
     */
    private var posted: TogetherDecisions.Shade? = null

    /** Cover art for [posted]'s `artKey`, or null. Dropped with the session. */
    private var art: Bitmap? = null
    private var artKey: String? = null

    /**
     * Headphones out, or the Bluetooth speaker gone.
     *
     * Registered at runtime and never in the manifest: `ACTION_AUDIO_BECOMING_NOISY`
     * is one of the implicit broadcasts Android stopped delivering to manifest
     * receivers, so a manifest entry would be a control that silently never
     * fires. Scoped to this service because that is exactly the lifetime it
     * should have — there is nothing to pause when nothing is playing.
     */
    private val becomingNoisy = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
            if (intent?.action == AudioManager.ACTION_AUDIO_BECOMING_NOISY) {
                TogetherManager.onBecomingNoisy()
            }
        }
    }
    private var noisyRegistered = false

    override fun onCreate() {
        super.onCreate()
        Notifier.ensureChannels(this)
        session = MediaSession(this, "comrade-together").apply {
            setCallback(object : MediaSession.Callback() {
                override fun onPlay() = togglePlayback(play = true)
                override fun onPause() = togglePlayback(play = false)
                override fun onStop() = TogetherManager.leave()

                // The three a headset, a car and the lock screen actually call,
                // and which did not exist before: without them a Bluetooth
                // next-track button reached a session that had not said it
                // could do that, and nothing happened.
                override fun onSkipToNext() {
                    TogetherManager.skipForward(this@TogetherService)
                }

                override fun onSkipToPrevious() {
                    TogetherManager.skipBack(this@TogetherService)
                }

                override fun onSeekTo(pos: Long) {
                    val live = TogetherManager.state.value as? TogetherManager.UiState.Live ?: return
                    // Guarded by the same answer the state advertises with, so
                    // a seek that arrives anyway on a session that cannot take
                    // one is dropped rather than applied to a length nobody knows.
                    if (!TogetherDecisions.mediaSeekAllowed(live.durationMs, live.external)) return
                    TogetherManager.setState(pos.coerceIn(0L, live.durationMs), live.playing)
                }
            })
            isActive = true
        }
        registerNoisy()
        // Honour the contract immediately with a generic notification; the
        // collector below replaces it in place (same id) as the session's real
        // state arrives. Nothing on this path may throw or block — see the
        // promotion contract above — so it takes no metadata and no artwork.
        promote(build(getString(R.string.together_notification_title), "", null))
        stateJob = scope.launch {
            TogetherManager.state.collect { state ->
                when (state) {
                    is TogetherManager.UiState.Live -> onLive(state)
                    // The session ended while we were still up; TogetherManager
                    // stops us, but do not keep claiming to be playing meanwhile.
                    else -> {
                        posted = null
                        publishPlaybackState(playing = false, positionMs = 0, live = null)
                    }
                }
            }
        }
    }

    /**
     * One emission of the live session.
     *
     * The playback state is republished every time — it is cheap, it carries
     * the playhead, and a stale one is what makes a lock-screen scrubber crawl.
     * The *notification* is rebuilt only when [TogetherDecisions.Shade] says a
     * person would see a difference.
     */
    private fun onLive(state: TogetherManager.UiState.Live) {
        publishPlaybackState(state.playing, state.positionMs, state)
        val queue = TogetherManager.queue.value
        val track = queue?.current
        val shade = TogetherDecisions.Shade(
            title = track?.title ?: state.title,
            artist = track?.artist.orEmpty(),
            album = track?.album,
            durationMs = state.durationMs,
            playing = state.playing,
            actions = TogetherDecisions.notificationActions(
                playing = state.playing,
                hasNext = TogetherDecisions.nextTrack(queue) != null,
                hasPrevious = queue != null,
                external = state.external,
            ),
            artKey = track?.albumId?.toString() ?: track?.uri,
        )
        if (shade == posted) return
        posted = shade
        // Art is IO and must not touch the promotion path, so the shade is
        // posted now with whatever art is already in hand and re-posted when a
        // new cover arrives. `MusicLibrary.artwork` is a byte-budgeted cache
        // (§22), so this is usually a hit and the second post never happens.
        if (shade.artKey != artKey && track != null) {
            scope.launch {
                val loaded = withContext(Dispatchers.IO) {
                    runCatching { MusicLibrary.artwork(this@TogetherService, track, ART_PX) }
                        .getOrNull()
                }
                // Re-check: the track may have moved on while this was loading,
                // and posting its cover over the next one is worse than none.
                if (posted?.artKey != shade.artKey) return@launch
                art = loaded
                artKey = shade.artKey
                publishMetadata(shade)
                promote(build(shade))
            }
        } else if (shade.artKey != artKey) {
            art = null
            artKey = shade.artKey
        }
        publishMetadata(shade)
        promote(build(shade))
    }

    private fun registerNoisy() {
        if (noisyRegistered) return
        val filter = IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY)
        if (Build.VERSION.SDK_INT >= 34) {
            registerReceiver(becomingNoisy, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            registerReceiver(becomingNoisy, filter)
        }
        noisyRegistered = true
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // Re-promote first, always: the obligation arms per *call* to
        // startForegroundService, so a redelivered intent that bailed early
        // would strand this instance and kill the process.
        // Read **before** promoting: on Android 14 a MediaProjection may only
        // begin while a foreground service already declaring `mediaProjection`
        // is running, so the re-announce that adds the type has to land in the
        // promotion below rather than after it. Sticky, because a later plain
        // start must not silently drop the type from under a live capture.
        if (intent?.getBooleanExtra(EXTRA_PROJECTION, false) == true) projecting = true
        // Re-promote with the shade already posted rather than the generic one:
        // rebuilding the plain notification here would blank the transport for
        // the moment between this and the collector's next emission, and a
        // button that vanishes under the thumb is the complaint `Transport`
        // already answers on screen.
        promote(
            posted?.let(::build)
                ?: build(getString(R.string.together_notification_title), "", null),
        )
        when (intent?.action) {
            ACTION_LEAVE -> TogetherManager.leave()
            ACTION_PLAY -> togglePlayback(play = true)
            ACTION_PAUSE -> togglePlayback(play = false)
            ACTION_NEXT -> TogetherManager.skipForward(this)
            ACTION_PREVIOUS -> TogetherManager.skipBack(this)
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        stateJob?.cancel()
        scope.cancel()
        if (noisyRegistered) {
            runCatching { unregisterReceiver(becomingNoisy) }
            noisyRegistered = false
        }
        session?.isActive = false
        session?.release()
        session = null
        // The cover outlives nothing: it is a cache hit away and holding a
        // sleeve-sized bitmap for a session that has ended is the shape of
        // waste `ui/MediaCache.kt` was fixed for.
        art = null
        artKey = null
        posted = null
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun togglePlayback(play: Boolean) {
        val live = TogetherManager.state.value as? TogetherManager.UiState.Live ?: return
        TogetherManager.setState(live.positionMs, play)
    }

    /**
     * What this session can do, said once for both halves of the contract.
     *
     * Play, pause and stop are always true. Skip and seek are exactly what the
     * callback above will honour, which is the point: an advertised action the
     * callback ignores is a dead button on somebody's steering wheel.
     */
    private fun publishPlaybackState(
        playing: Boolean,
        positionMs: Long,
        live: TogetherManager.UiState.Live?,
    ) {
        var actions = PlaybackState.ACTION_PLAY or
            PlaybackState.ACTION_PAUSE or
            PlaybackState.ACTION_PLAY_PAUSE or
            PlaybackState.ACTION_STOP
        if (live != null) {
            val queue = TogetherManager.queue.value
            // Previous is offered whenever there is a queue at all, because
            // `backStep` always does something — it restarts the current track
            // when there is nothing behind it. Next genuinely can have nothing
            // to do, and is left off rather than drawn dead.
            if (queue != null) actions = actions or PlaybackState.ACTION_SKIP_TO_PREVIOUS
            if (TogetherDecisions.nextTrack(queue) != null) {
                actions = actions or PlaybackState.ACTION_SKIP_TO_NEXT
            }
            if (TogetherDecisions.mediaSeekAllowed(live.durationMs, live.external)) {
                actions = actions or PlaybackState.ACTION_SEEK_TO
            }
        }
        val state = PlaybackState.Builder()
            .setActions(actions)
            .setState(
                if (playing) PlaybackState.STATE_PLAYING else PlaybackState.STATE_PAUSED,
                positionMs,
                // The rate the *platform* extrapolates the playhead with. Zero
                // while paused, or a lock-screen scrubber keeps travelling
                // through a track nobody is playing.
                if (playing) 1.0f else 0f,
            )
            .build()
        runCatching { session?.setPlaybackState(state) }
    }

    /**
     * What the lock screen and the Android 13+ media panel draw.
     *
     * `METADATA_KEY_DURATION` is not decoration either: it is what makes the
     * scrubber appear at all. A session that cannot honour a seek publishes no
     * duration, so no scrubber is drawn rather than one that does nothing —
     * the same answer [publishPlaybackState] gives about `ACTION_SEEK_TO`, from
     * the same function, so the two cannot drift apart.
     */
    private fun publishMetadata(shade: TogetherDecisions.Shade) {
        val live = TogetherManager.state.value as? TogetherManager.UiState.Live
        val seekable = live != null &&
            TogetherDecisions.mediaSeekAllowed(live.durationMs, live.external)
        val meta = MediaMetadata.Builder()
            .putString(MediaMetadata.METADATA_KEY_TITLE, shade.title)
            .putString(MediaMetadata.METADATA_KEY_ARTIST, shade.artist)
            .putString(MediaMetadata.METADATA_KEY_ALBUM, shade.album.orEmpty())
            .putLong(
                MediaMetadata.METADATA_KEY_DURATION,
                if (seekable) shade.durationMs else 0L,
            )
            .apply { art?.let { putBitmap(MediaMetadata.METADATA_KEY_ALBUM_ART, it) } }
            .build()
        runCatching { session?.setMetadata(meta) }
    }

    private fun promote(notification: Notification) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(
                NOTIFICATION_ID,
                notification,
                if (projecting) {
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK or
                        ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION
                } else {
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK
                },
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    private fun build(shade: TogetherDecisions.Shade): Notification =
        build(
            title = shade.title.ifBlank { getString(R.string.together_notification_title) },
            text = shade.artist,
            shade = shade,
        )

    /**
     * The notification, with as much of a media notification as it has.
     *
     * [shade] `null` is the promotion path: no metadata, no transport, no art,
     * nothing that can fail — because a throw here is not a missing button, it
     * is `ForegroundServiceDidNotStartInTimeException` and the process. Every
     * richer version is a re-post over the same id afterwards.
     */
    private fun build(title: String, text: String, shade: TogetherDecisions.Shade?): Notification {
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val builder = Notification.Builder(this, Notifier.CHANNEL_CONNECTION)
            .setSmallIcon(android.R.drawable.ic_media_play)
            .setContentTitle(title)
            .setContentText(text)
            .setContentIntent(open)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setCategory(Notification.CATEGORY_TRANSPORT)
        shade?.let { art?.let(builder::setLargeIcon) }

        // The transport, in the order the decision returned — which is empty
        // for a followed external session, where the app being followed already
        // publishes its own and two notifications over one stream is not help.
        val slots = shade?.actions.orEmpty()
        slots.forEach { builder.addAction(actionFor(it)) }
        builder.addAction(
            Notification.Action.Builder(
                null,
                getString(R.string.together_leave),
                service(REQ_LEAVE, ACTION_LEAVE),
            ).build(),
        )

        val style = Notification.MediaStyle().setMediaSession(session?.sessionToken)
        // The compact view holds three at most, and the platform silently drops
        // extras rather than erroring — so the indices are computed from what
        // was actually added instead of assumed.
        if (slots.isNotEmpty()) {
            style.setShowActionsInCompactView(*slots.indices.take(3).toList().toIntArray())
        }
        builder.setStyle(style)
        return builder.build()
    }

    private fun actionFor(slot: TogetherDecisions.NotificationAction): Notification.Action = when (slot) {
        TogetherDecisions.NotificationAction.SKIP_PREVIOUS -> Notification.Action.Builder(
            null,
            getString(R.string.media_previous),
            service(REQ_PREVIOUS, ACTION_PREVIOUS),
        ).build()

        TogetherDecisions.NotificationAction.PLAY -> Notification.Action.Builder(
            null,
            getString(R.string.together_play),
            service(REQ_PLAY, ACTION_PLAY),
        ).build()

        TogetherDecisions.NotificationAction.PAUSE -> Notification.Action.Builder(
            null,
            getString(R.string.together_pause),
            service(REQ_PAUSE, ACTION_PAUSE),
        ).build()

        TogetherDecisions.NotificationAction.SKIP_NEXT -> Notification.Action.Builder(
            null,
            getString(R.string.media_next),
            service(REQ_NEXT, ACTION_NEXT),
        ).build()
    }

    /**
     * A `PendingIntent` back into this service.
     *
     * Distinct request codes per action, which is the whole reason this is a
     * function: `FLAG_UPDATE_CURRENT` matches on request code and *not* on the
     * action, so reusing one code would have every button rewrite the others
     * and the shade would end up with four intents that all did the same thing.
     */
    private fun service(requestCode: Int, action: String): PendingIntent =
        PendingIntent.getService(
            this,
            requestCode,
            Intent(this, TogetherService::class.java).setAction(action),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )

    companion object {
        private const val NOTIFICATION_ID = 4102
        const val ACTION_LEAVE = "mullu.comrade.together.LEAVE"
        const val ACTION_PLAY = "mullu.comrade.together.PLAY"
        const val ACTION_PAUSE = "mullu.comrade.together.PAUSE"
        const val ACTION_NEXT = "mullu.comrade.together.NEXT"
        const val ACTION_PREVIOUS = "mullu.comrade.together.PREVIOUS"
        const val EXTRA_PROJECTION = "projection"

        // One request code per action; see `service`. Sharing one would make
        // every button overwrite the others under FLAG_UPDATE_CURRENT.
        private const val REQ_LEAVE = 1
        private const val REQ_PLAY = 2
        private const val REQ_PAUSE = 3
        private const val REQ_NEXT = 4
        private const val REQ_PREVIOUS = 5

        /**
         * The cover's edge, in pixels, asked of `MusicLibrary.artwork`.
         *
         * A fixed number rather than a density-scaled one because this art is
         * for the shade and the lock screen, which are not laid out in this
         * app's `dp` — and because the size is part of that cache's key (§22),
         * so a figure that moved with the device would mint a second entry for
         * every album already in there.
         */
        private const val ART_PX = 320

        fun start(context: Context) {
            context.startForegroundService(Intent(context, TogetherService::class.java))
        }

        /**
         * Re-announce with `mediaProjection` **before** a projection starts.
         *
         * From Android 14 a projection may only begin while such a service is
         * already running, and getting that order wrong throws at the point of
         * capture rather than here — the same sequencing `CallManager.startScreenShare`
         * depends on. This is a second `startForegroundService` to a live
         * instance, which arms a fresh promotion deadline; `onStartCommand`
         * promotes immediately, which is what keeps that safe.
         */
        fun startWithProjection(context: Context) {
            context.startForegroundService(
                Intent(context, TogetherService::class.java)
                    .putExtra(EXTRA_PROJECTION, true),
            )
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, TogetherService::class.java))
        }
    }
}
