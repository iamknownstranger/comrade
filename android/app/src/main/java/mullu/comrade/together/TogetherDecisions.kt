package mullu.comrade.together

/**
 * The watch-together decisions that must not live in a listener — pure, with no
 * Android imports, so the JVM lane can pin them.
 *
 * The drift arithmetic is **not** here: it lives in `comrade_core::together`, so
 * this frontend and the desktop one inherit one answer instead of two that drift
 * apart (the mistake `docs/COMMS_ARCHITECTURE.md` ADR-2 exists to stop
 * repeating). What is genuinely local is everything about *this* player's event
 * semantics, and there are three traps in it.
 *
 * **Echo.** Applying a remote position makes our own player fire the callbacks
 * we listen to, and re-broadcasting those is a feedback loop between two
 * devices. A boolean guard cannot close it: `seekTo` completes asynchronously,
 * so the flag is already clear by the time `onSeekComplete` runs, and seeking to
 * the position the player already holds may fire nothing at all — which leaves a
 * latch set forever and the session deaf to the user. So an apply records what
 * it expects to see, matched by value and bounded by a deadline, and anything
 * unexplained is the user.
 *
 * **The scrubber.** A drag emits continuously. Only the release is a command,
 * the position poll must not fight the thumb, and a remote seek arriving
 * mid-drag must not yank it out of someone's finger.
 *
 * **The `MediaPlayer` footguns**, encoded here rather than trusted to a comment:
 * `setPlaybackParams` on a *paused* player starts playback on several Android
 * versions, and `seekTo` before `prepare()` throws.
 */
object TogetherDecisions {

    /** Positions this close are the same position — `MediaPlayer` reports in ms
     *  but seeks land on sample boundaries. */
    const val EPSILON_MS: Long = 250

    /** How long an apply may wait for the callback it expects before we assume
     *  it produced none. */
    const val SUPPRESS_TTL_MS: Long = 1_500

    /** How often the UI reads the playhead while playing. Local only — it is not
     *  the wire heartbeat, which is ten seconds and lives in Rust. */
    const val POLL_MS: Long = 250

    /** A drift correction must never produce chipmunk audio, whatever the core
     *  asks for. Mirrors `TOGETHER_MAX_TRIM` on the Rust side and
     *  `RATE_MIN`/`RATE_MAX` in `desktop/ui/together_sync.mjs`. */
    const val RATE_MIN: Float = 0.9f
    const val RATE_MAX: Float = 1.1f

    /** What the player is being asked to do. */
    sealed interface Op {
        data class Seek(val posMs: Long) : Op
        data class Rate(val value: Float) : Op
        data object Play : Op
        data object Pause : Op
    }

    /** What a player callback should tell the other person, if anything. */
    data class Emit(val posMs: Long, val playing: Boolean)

    /** One thing an apply expects the player to report back. */
    data class Expectation(val kind: String, val posMs: Long?, val deadlineMs: Long)

    /**
     * A ledger of callbacks an apply is about to cause.
     *
     * Not thread-safe on purpose: it belongs to the player thread that both
     * applies and observes. Anything else would be a lock in a callback.
     */
    class EchoSuppressor(private val ttlMs: Long = SUPPRESS_TTL_MS) {
        private val pending = ArrayDeque<Expectation>()

        fun expect(kind: String, posMs: Long?, nowMs: Long) {
            reap(nowMs)
            pending.addLast(Expectation(kind, posMs, nowMs + ttlMs))
            // Bounded: an apply storm must not grow this without limit.
            while (pending.size > 16) pending.removeFirst()
        }

        /**
         * Is this observed callback explained by something we did? Consumes the
         * entry if so — one apply explains exactly one callback.
         */
        fun explains(kind: String, posMs: Long?, nowMs: Long): Boolean {
            reap(nowMs)
            val idx = pending.indexOfFirst {
                it.kind == kind &&
                    (it.posMs == null || posMs == null || kotlin.math.abs(it.posMs - posMs) <= EPSILON_MS)
            }
            if (idx < 0) return false
            pending.removeAt(idx)
            return true
        }

        fun clear() = pending.clear()

        val size: Int get() = pending.size

        /**
         * Drop entries whose apply never produced a callback — reaped by
         * deadline rather than popped in order, because two quick seeks can
         * coalesce into one callback and strand the other, and a stranded entry
         * must expire rather than swallow a later genuine seek.
         */
        private fun reap(nowMs: Long) {
            while (pending.isNotEmpty() && pending.first().deadlineMs <= nowMs) {
                pending.removeFirst()
            }
            // The stranded entry may not be the oldest, so sweep too.
            pending.removeAll { it.deadlineMs <= nowMs }
        }
    }

    /** The player state a plan is computed against. */
    data class Local(val posMs: Long, val playing: Boolean, val prepared: Boolean)

    /** Operations to run, and the expectations to arm alongside them. */
    data class Plan(val ops: List<Op>, val expect: List<Pair<String, Long?>>)

    private val NOTHING = Plan(emptyList(), emptyList())

    /**
     * Turn a correction from `comrade_core::together::sync_verdict` into player
     * operations **and the ledger entries that must be armed with them**.
     *
     * Returning a value rather than acting inline is the point: an operation
     * that can raise a callback without a matching expectation *is* the feedback
     * loop, and a test can assert the pairing without a player.
     *
     * @param kind the verdict's `kind` string as it crosses the bridge
     */
    fun planCorrection(kind: String, posMs: Long, rate: Float, playing: Boolean, local: Local): Plan {
        if (!local.prepared) return NOTHING // seekTo before prepare() throws
        return when (kind) {
            "hold" -> NOTHING

            // A rate trim raises no seek callback at all, so it arms nothing —
            // the common case is silent by construction rather than suppressed.
            // Guarded because setPlaybackParams on a paused player starts
            // playback on several Android versions.
            "nudge" ->
                if (local.playing) {
                    Plan(listOf(Op.Rate(rate.coerceIn(RATE_MIN, RATE_MAX))), emptyList())
                } else {
                    NOTHING
                }

            // Deliberately no Play: a drift correction must never start a player
            // the user paused.
            "seek" -> Plan(listOf(Op.Seek(posMs)), listOf("seek" to posMs))

            // A command we missed — take their state wholesale, playing included.
            "adopt" -> {
                val ops = mutableListOf<Op>()
                val expect = mutableListOf<Pair<String, Long?>>()
                if (kotlin.math.abs(posMs - local.posMs) > EPSILON_MS) {
                    ops += Op.Seek(posMs)
                    expect += "seek" to posMs
                }
                if (playing != local.playing) {
                    if (playing) {
                        ops += Op.Play
                        expect += "play" to null
                    } else {
                        ops += Op.Pause
                        expect += "pause" to null
                    }
                }
                Plan(ops, expect)
            }

            else -> NOTHING // an unrecognised verdict is ignored, not guessed at
        }
    }

    /**
     * A command from the other person: where to be, and whether to wait first.
     *
     * `applyInMs` comes from the Rust side, which has already carried the
     * position forward through the message's flight time — so this applies
     * `posMs` as given and must not compensate again.
     */
    fun planCommand(posMs: Long, playing: Boolean, local: Local): Plan {
        if (!local.prepared) return NOTHING
        val ops = mutableListOf<Op>()
        val expect = mutableListOf<Pair<String, Long?>>()
        if (kotlin.math.abs(posMs - local.posMs) > EPSILON_MS) {
            ops += Op.Seek(posMs)
            expect += "seek" to posMs
        }
        if (playing != local.playing) {
            if (playing) {
                ops += Op.Play
                expect += "play" to null
            } else {
                ops += Op.Pause
                expect += "pause" to null
            }
        }
        return Plan(ops, expect)
    }

    /**
     * What a local player callback should tell the other person.
     *
     * `ratechange` has no Android callback, so unlike the desktop module there
     * is nothing to filter here — the rate is set directly and never observed.
     */
    fun classifyCallback(
        kind: String,
        posMs: Long,
        playing: Boolean,
        suppressor: EchoSuppressor,
        nowMs: Long,
    ): Emit? {
        // Reaching the end is a pause, never a seek: the playhead did not move
        // anywhere the other person needs to follow.
        if (kind == "completion") return Emit(posMs, false)
        if (kind != "seek" && kind != "play" && kind != "pause") return null
        if (suppressor.explains(kind, if (kind == "seek") posMs else null, nowMs)) return null
        return Emit(posMs, if (kind == "pause") false else if (kind == "play") true else playing)
    }

    /** What the scrubber and the position poll are allowed to do to each other. */
    data class ScrubState(val scrubbing: Boolean, val pendingRemoteMs: Long?)

    /**
     * Whether the periodic poll may move the slider. It may not while a finger
     * is on it — otherwise the thumb fights the user every 250 ms.
     */
    fun pollMayMoveSlider(state: ScrubState): Boolean = !state.scrubbing

    /**
     * A remote seek that arrives mid-drag is queued, not applied: yanking the
     * playhead out from under someone's finger is worse than being briefly out
     * of step, and they are about to name a position anyway.
     */
    fun onRemoteSeek(state: ScrubState, posMs: Long): ScrubState =
        if (state.scrubbing) state.copy(pendingRemoteMs = posMs) else state.copy(pendingRemoteMs = null)

    /**
     * On release the *user's* position wins — they were the one holding it — and
     * any queued remote seek is dropped rather than applied after the fact.
     */
    fun onScrubRelease(state: ScrubState): ScrubState = ScrubState(scrubbing = false, pendingRemoteMs = null)

    /** Whether a queued remote seek should now be applied (drag ended, nothing newer). */
    fun pendingRemoteToApply(state: ScrubState): Long? = if (state.scrubbing) null else state.pendingRemoteMs

    // ── What the two measured numbers are allowed to say ────────────────────
    //
    // Mirrors `desktop/ui/player_view.mjs` — `TOGETHER_MS`, `READING_STALE_MS`,
    // `driftLabel`, `qualityLabel`, `measurementLines` — against the same
    // vectors, because the rule they encode is the one the whole design rests
    // on and two frontends disagreeing about it is worse than neither showing
    // it. What is *not* mirrored is the wording: these return decisions, and
    // `ui/TogetherScreen.kt` turns them into localised strings, the same
    // division `Status` already uses.

    /** A gap smaller than this is not worth narrating even when we can see it. */
    const val TOGETHER_MS: Long = 400

    /**
     * How long a measurement is worth showing after it was taken.
     *
     * Corrections cross the bridge **only** when the verdict is not `hold`, so
     * the steady state emits nothing — and a screen that keeps the last pair
     * therefore prints a gap that was closed minutes ago, underneath the word
     * "Together". Two heartbeats, matching `READING_STALE_MS` on desktop and
     * `together::TOGETHER_DIRECT_SILENCE_MS` in core, and derived the same way:
     * two heartbeats of silence means at least one verdict said the gap was
     * inside the deadband.
     */
    const val READING_STALE_MS: Long = 20_000

    /** The floor on what a direct path may claim about itself, in ms. */
    const val QUALITY_FLOOR_MS: Long = 50

    /** Above this the path is a relay, and says so. */
    const val QUALITY_DIRECT_MAX_MS: Long = 150

    /**
     * The gap, when it is bigger than our own ability to measure it.
     *
     * [Silent] is not "they are together" — it is "we have nothing honest to
     * say", which covers both being genuinely in step and not being able to
     * tell. [Quality] carries the difference; this must not try to.
     */
    sealed interface Drift {
        data object Silent : Drift

        /** Rendered to one decimal place, as `%.1f`. */
        data class Gap(val ms: Long, val weAreAhead: Boolean) : Drift
    }

    /** How well we can measure, which is what makes a [Drift.Gap] mean anything. */
    sealed interface Quality {
        data object Unknown : Quality

        /**
         * @param ms floored at [QUALITY_FLOOR_MS] — under that the figure is
         *   below what either player can report, so claiming it would be the
         *   same invention the deadband exists to prevent.
         * @param direct a peer channel or a mesh hop, rather than a relay.
         * @param decimals 2 for a direct path, 1 for a relayed one — a relayed
         *   `±0.62s` implies a precision the second digit does not have.
         */
        data class Known(val ms: Long, val direct: Boolean, val decimals: Int) : Quality
    }

    /** Both lines, or neither. */
    data class Measurement(val drift: Drift, val quality: Quality)

    /**
     * What the readout may say, given a correction that arrived [ageMs] ago.
     *
     * One call rather than two because it is one measurement: a drift figure
     * without its error is unreadable, and an error left on screen after the
     * drift aged out would claim we are still measuring after we stopped
     * hearing verdicts. So they age out as a pair.
     *
     * A session with no correction yet passes a large [ageMs] and gets blanks —
     * the same answer as a stale reading, which is right: both mean there is no
     * current number.
     *
     * @param driftMs signed; positive means this device is ahead.
     */
    fun measurement(driftMs: Long, qualityMs: Long, ageMs: Long): Measurement {
        if (ageMs < 0 || ageMs >= READING_STALE_MS) {
            return Measurement(Drift.Silent, Quality.Unknown)
        }
        return Measurement(driftOf(driftMs, qualityMs), qualityOf(qualityMs))
    }

    /** Reported only above our own error — see the desktop module's long note. */
    private fun driftOf(driftMs: Long, qualityMs: Long): Drift {
        val gap = kotlin.math.abs(driftMs)
        val floor = maxOf(kotlin.math.abs(qualityMs), TOGETHER_MS)
        return if (gap <= floor) Drift.Silent else Drift.Gap(gap, weAreAhead = driftMs > 0)
    }

    private fun qualityOf(qualityMs: Long): Quality = when {
        qualityMs <= 0 -> Quality.Unknown
        qualityMs <= QUALITY_FLOOR_MS -> Quality.Known(QUALITY_FLOOR_MS, direct = true, decimals = 2)
        qualityMs <= QUALITY_DIRECT_MAX_MS -> Quality.Known(qualityMs, direct = true, decimals = 2)
        else -> Quality.Known(qualityMs, direct = false, decimals = 1)
    }

    // ── Driving a player that only reports where it is once a second ────────
    //
    // A YouTube embed tells us its position through `onCurrentSecond`, which
    // fires about once a second. `together_report_position` is called by the
    // poll four times a second, and the drift ladder compares whatever it was
    // last given against the peer. Handing it a reading up to a second old is
    // how a session invents drift that is not there. Everything below is about
    // that gap, and none of it imports anything — the JVM lane is the only lane
    // that will check it before CI.

    /**
     * How far past its last tick a coarse playhead may be extrapolated.
     *
     * Beyond this we stop guessing and report the last thing we actually knew.
     * The difference matters: a video that stalled, or a player the system
     * froze when the app went to the background, sends no ticks at all — and an
     * uncapped estimate would keep advancing a playhead that is standing still,
     * confidently, forever. Reporting a stale-but-true position lets the next
     * drift verdict see a real gap; reporting an invented one hides it.
     *
     * Two ticks, so a single late one costs nothing.
     */
    const val COARSE_EXTRAPOLATE_MAX_MS: Long = 2_000

    /**
     * Where a once-a-second player is *now*, between its ticks.
     *
     * Not thread-safe, like [EchoSuppressor] and for the same reason: it
     * belongs to the thread that both observes the player and reports for it.
     */
    class CoarsePlayhead(private val maxExtrapolateMs: Long = COARSE_EXTRAPOLATE_MAX_MS) {
        private var knownMs: Long = 0
        private var knownAtMs: Long = 0
        private var running: Boolean = false

        /** The player said where it is. */
        fun onTick(posMs: Long, nowMs: Long) {
            knownMs = posMs.coerceAtLeast(0)
            knownAtMs = nowMs
        }

        /**
         * *We* moved it, and the estimate must move with it immediately.
         *
         * This is the one that would otherwise be a real bug rather than a
         * rounding error. Waiting for the next tick means up to a second of
         * reporting the position we just left — so the ladder sees the same gap
         * it has already closed and corrects a second time. That is the
         * sawtooth `AUDIT.md` already records for the sticky rate trim, in a
         * different costume.
         */
        fun onSeek(posMs: Long, nowMs: Long) = onTick(posMs, nowMs)

        /** Play or pause. A paused playhead does not advance. */
        fun onPlaying(playing: Boolean, nowMs: Long) {
            // Bank the elapsed time before the state changes, or a pause after
            // 900 ms of un-ticked playback throws that 900 ms away.
            knownMs = estimateMs(nowMs)
            knownAtMs = nowMs
            running = playing
        }

        /** Start again from nothing — a new video, or a released player. */
        fun reset() {
            knownMs = 0
            knownAtMs = 0
            running = false
        }

        /**
         * Best estimate of the playhead at `nowMs`.
         *
         * A clock that steps backwards reads as "no time has passed" rather
         * than as a negative advance, the same saturating choice
         * `direct_path_live` makes in core.
         */
        fun estimateMs(nowMs: Long): Long {
            if (!running) return knownMs
            val elapsed = (nowMs - knownAtMs).coerceIn(0, maxExtrapolateMs)
            return knownMs + elapsed
        }
    }

    /**
     * What an embedded player's state means to a session.
     *
     * A `String` rather than the library's enum on purpose: this file has no
     * Android and no third-party imports, which is what lets the JVM lane run
     * it — the same reason [planCorrection] takes a verdict `kind` as text.
     */
    sealed interface EmbedState {
        /** Loaded but never started, or cued and waiting. Nothing to report. */
        data object NotReady : EmbedState

        data class Live(val playing: Boolean) : EmbedState

        /** Reached the end, which is a pause the peer should hear about. */
        data object Ended : EmbedState

        /**
         * Buffering. Deliberately **not** [Live] with `playing = false`.
         *
         * `docs/TOGETHER.md` §10 rules out reporting buffering to the peer, and
         * this is where that rule is actually enforced: telling them "they
         * paused" because our own video stalled is the worst ping-pong
         * available here — they pause, which makes us re-evaluate, which makes
         * them re-evaluate. A stall is ridden out locally and the next drift
         * verdict closes the gap.
         */
        data object Stalled : EmbedState
    }

    /** Map the embed's reported state. Anything unrecognised is "not ready". */
    fun embedState(state: String): EmbedState = when (state) {
        "playing" -> EmbedState.Live(playing = true)
        "paused" -> EmbedState.Live(playing = false)
        "buffering" -> EmbedState.Stalled
        "ended" -> EmbedState.Ended
        else -> EmbedState.NotReady
    }

    /**
     * Whether this state change is ours to tell the other person about.
     *
     * [EmbedState.Stalled] never is (see above), and [EmbedState.NotReady]
     * never is — a player that has not started has no position worth sending.
     */
    fun embedStateIsWorthSending(state: EmbedState): Boolean =
        state is EmbedState.Live || state is EmbedState.Ended

    // ── When the player says it is playing and the video is not ─────────────
    //
    // `docs/TOGETHER.md` §9a: an ad break is per-viewer. One side gets a
    // pre-roll, the other does not, and the session comes apart by exactly the
    // length of the break through no fault of the clock. Nothing in the drift
    // ladder can see it — from core's side the stalled device simply reports a
    // playhead that is not where it should be, and the correction it gets back
    // is a seek *forward past the ad*, which the embed refuses, which produces
    // the next correction. A screen saying "catching up…" while nothing catches
    // up is the visible symptom.

    /**
     * How long a claimed-playing playhead may stand still before we stop
     * claiming.
     *
     * Three ticks of a once-a-second player. Deliberately one tick past
     * [COARSE_EXTRAPOLATE_MAX_MS], so the estimate has already gone flat — and
     * been *seen* to go flat — before anything is called a stall: a single late
     * tick then costs nothing, which is the same allowance the extrapolation cap
     * makes for the same reason.
     */
    const val STALL_AFTER_MS: Long = 3_000

    /**
     * Whether this device's own playhead is actually moving.
     *
     * **What this can and cannot know, stated plainly, because the useful case
     * is an inference.** It observes one fact: the player reports itself
     * playing and the position it reports is not advancing. On an embed, one
     * cause of that is an ad break — the ad is what the player is playing, and
     * it is not the video. Another is a stall the player did not label
     * `buffering`. This does not tell them apart and must not claim to; what it
     * asserts is exactly the fact it measured.
     *
     * That is enough, because **both causes want the same answer**: stop telling
     * the other device we are playing. A hold reported as position rather than
     * as a command is what keeps them from running away — the same instruction
     * §12 gives a byte-starved reader, and for the same reason.
     *
     * Not thread-safe, like [CoarsePlayhead] and [EchoSuppressor]: it belongs to
     * the thread that observes the player.
     */
    class StallWatch(private val afterMs: Long = STALL_AFTER_MS) {
        private var lastPosMs: Long = UNSET
        private var movedAtMs: Long = 0
        private var stalled: Boolean = false

        /** Whether the playhead is standing still as of the last sample. */
        val isStalled: Boolean get() = stalled

        /**
         * Feed one observation. Returns whether the playhead is stalled.
         *
         * `posMs` must be the player's **raw** reported position, not
         * [CoarsePlayhead.estimateMs] — an estimate advances between ticks by
         * construction, so watching it would mean watching our own arithmetic
         * rather than the player, and nothing would ever look stalled until the
         * extrapolation cap hit.
         */
        fun onSample(posMs: Long, playing: Boolean, nowMs: Long): Boolean {
            // A paused player standing still is not a stall, it is a pause, and
            // the peer is being told about that through the ordinary path.
            if (!playing) {
                lastPosMs = posMs
                movedAtMs = nowMs
                stalled = false
                return false
            }
            if (lastPosMs == UNSET || kotlin.math.abs(posMs - lastPosMs) > EPSILON_MS) {
                lastPosMs = posMs
                movedAtMs = nowMs
                stalled = false
                return false
            }
            // A clock that stepped backwards reads as no elapsed time rather
            // than as an instant stall — the same saturating choice
            // `direct_path_live` makes in core.
            if (nowMs - movedAtMs >= afterMs) stalled = true
            return stalled
        }

        /** A new video, a seek we caused, or a released player. */
        fun reset() {
            lastPosMs = UNSET
            movedAtMs = 0
            stalled = false
        }

        private companion object {
            /** No sample yet. `0` would be a real position on a fresh video. */
            const val UNSET = Long.MIN_VALUE
        }
    }

    // ── Picture ─────────────────────────────────────────────────────────────

    /**
     * What this recording turned out to be, decided from the only thing
     * `MediaPlayer` will tell us: the video track's dimensions, which are `0`
     * when there is no video track.
     *
     * This exists because the screen has to answer "video or not" *after*
     * opening the file rather than before. The picked MIME type is not a usable
     * answer — an `.mkv` of an album is `video/x-matroska` with no video track,
     * and a `.mp4` podcast is `video/mp4` — so trusting it shows a permanent
     * black rectangle to someone listening to music.
     */
    sealed interface Picture {
        /** A video track, with the dimensions the decoder reported. */
        data class Video(val width: Int, val height: Int) : Picture

        /** Audio only, or not yet known — both mean "draw no surface". */
        data object None : Picture
    }

    /**
     * Classify from reported dimensions.
     *
     * Non-positive is the audio-only signal, and it is also what `MediaPlayer`
     * reports *before* the first frame is decoded, so this deliberately makes
     * "not yet" and "never" the same answer: the surface appears when there is
     * something to put on it, which is the transition [Picture.Video] describes.
     */
    fun pictureOf(width: Int, height: Int): Picture =
        if (width > 0 && height > 0) Picture.Video(width, height) else Picture.None

    /**
     * The picture an embed session has, known before anything loads.
     *
     * Unlike a file, this is not a decoder report and does not need to be: the
     * IFrame player draws into a frame the library fixes at 16:9
     * (`SixteenByNineFrameLayout`), and there is always a picture — nobody
     * invites you to a YouTube video with no video track. Stating it as a
     * constant is what lets [keepScreenOn] and [aspectRatioOf] treat both
     * sources with one rule instead of growing a second argument that means
     * "unless it is an embed".
     */
    val EMBED_PICTURE: Picture.Video = Picture.Video(16, 9)

    /**
     * The shape to give the video surface, or `null` when there is no picture.
     *
     * Compose's `aspectRatio` modifier throws on a non-positive ratio, so the
     * clamp is not decoration — a corrupt header reporting `1x1000000` would
     * otherwise take the whole screen down. The bounds are wider than any real
     * recording (32:9 ultrawide is 3.56, a vertical phone clip is 0.56) and
     * exist only to keep a hostile or broken file inside something drawable.
     */
    fun aspectRatioOf(picture: Picture): Float? = when (picture) {
        is Picture.None -> null
        is Picture.Video -> (picture.width.toFloat() / picture.height.toFloat())
            .takeIf { it.isFinite() && it > 0f }
            ?.coerceIn(MIN_ASPECT, MAX_ASPECT)
    }

    const val MIN_ASPECT: Float = 0.25f
    const val MAX_ASPECT: Float = 4.0f

    /**
     * Whether to hold the screen awake.
     *
     * Only for video that is actually playing. An audio session must *not* hold
     * it: "listen together" over a two-hour album is the case where a burnt-out
     * battery is the whole failure, and there is nothing to look at anyway.
     */
    fun keepScreenOn(picture: Picture, playing: Boolean): Boolean =
        playing && picture is Picture.Video

    // ── Choosing something to play, and someone to play it with ─────────────
    //
    // The Together tab is the one place a session starts from, which means this
    // is where "pick music" and "pick a person" have to be decided. All of it
    // is here rather than in the composable for the reason the file header
    // gives: `ui/TogetherScreen.kt` imports Android and Compose, so the JVM
    // lane cannot see a single line of it, and CI is several minutes away.

    /** How the clock is written under the scrubber. */
    private const val MS_PER_SECOND = 1_000L
    private const val SECONDS_PER_MINUTE = 60L
    private const val MINUTES_PER_HOUR = 60L

    /**
     * A playhead as a clock — `3:07`, or `1:02:33` once it earns the hours.
     *
     * The hour field appears only when there is one, because a two-minute song
     * showing `0:02:07` is the shape a transport control gets wrong and then
     * nobody trusts. Truncated rather than rounded, so the label never shows a
     * second the playhead has not reached.
     *
     * A negative reading is `0:00`: a player that reports one is broken, and the
     * clock's job at that moment is to be unremarkable rather than to display a
     * minus sign.
     */
    fun clock(ms: Long): String {
        val total = (ms.coerceAtLeast(0)) / MS_PER_SECOND
        val seconds = total % SECONDS_PER_MINUTE
        val minutes = (total / SECONDS_PER_MINUTE) % MINUTES_PER_HOUR
        val hours = total / (SECONDS_PER_MINUTE * MINUTES_PER_HOUR)
        return if (hours > 0) {
            "$hours:${pad(minutes)}:${pad(seconds)}"
        } else {
            "$minutes:${pad(seconds)}"
        }
    }

    private fun pad(value: Long): String = if (value < 10) "0$value" else value.toString()

    /**
     * What goes on the right of the scrubber, or `null` when nothing honest can.
     *
     * A length of zero is not a length — it is what an embed reports before it
     * loads and what an external session reports always — so this returns
     * nothing rather than `0:00`, which would be a claim that the track has
     * ended. That is the same rule the slider itself follows in
     * `ui/TogetherScreen.kt`, stated once so the two cannot disagree.
     *
     * Counts **down**, and is written with a leading `-`. A remaining figure is
     * what someone deciding whether to start the next thing actually wants, and
     * the sign is what stops it reading as the total.
     */
    fun remainingClock(positionMs: Long, durationMs: Long): String? {
        if (durationMs <= 0) return null
        val left = (durationMs - positionMs).coerceIn(0, durationMs)
        return "-${clock(left)}"
    }

    /**
     * One row in the phone's own music library.
     *
     * No `Uri` and no `Context`: this is the shape the pure half reasons about,
     * and `MusicLibrary` is what turns `MediaStore` rows into it. `uri` is
     * carried as the string form for the same reason — a `android.net.Uri` here
     * would put an Android import in the one file that must not have one.
     */
    data class Track(
        val uri: String,
        val title: String,
        val artist: String,
        val album: String?,
        val durationMs: Long,
        /** For artwork. `null` on a row `MediaStore` gave no album for. */
        val albumId: Long?,
    )

    /**
     * The line under a track's title — artist, album and length, minus whatever
     * the file did not say.
     *
     * Joined with the separator rather than a template so a track with no artist
     * does not read " · Unknown · 3:07". `MediaStore` writes the literal string
     * `<unknown>` into the artist column for a file with no tag, which is a
     * detail of the provider rather than a thing to show someone — so it is
     * treated as absent here, where every caller inherits the answer.
     */
    fun trackSubtitle(track: Track): String = listOfNotNull(
        named(track.artist),
        named(track.album),
        track.durationMs.takeIf { it > 0 }?.let { clock(it) },
    ).joinToString(SUBTITLE_SEPARATOR)

    /**
     * A tag as something to show someone, or `null` when the file did not
     * really say.
     *
     * One rule in one place, because three callers now need it and a fourth
     * that spelled it differently would put the literal string `<unknown>` on
     * screen for one of them and not the others.
     */
    fun named(tag: String?): String? =
        tag?.takeIf { it.isNotBlank() && it != MEDIASTORE_UNKNOWN }

    /** `MediaStore`'s placeholder for a tag the file does not carry. */
    const val MEDIASTORE_UNKNOWN = "<unknown>"

    const val SUBTITLE_SEPARATOR = " · "

    /**
     * The library, filtered by what has been typed into the search field.
     *
     * Every word has to appear *somewhere* in the row — title, artist or album —
     * rather than the whole phrase appearing in one field. That is what makes
     * "rahman kun" find "Kun Faya Kun" by A. R. Rahman, which is how people
     * actually search for music they already own, and it is the one behaviour
     * worth having over `MediaStore`'s own `LIKE`.
     *
     * Order is preserved: the caller sorted by title in the query, and a filter
     * that also re-ranked would make the list jump under the finger as
     * characters arrive.
     */
    fun filterTracks(tracks: List<Track>, query: String): List<Track> {
        val words = query.trim().lowercase().split(' ').filter { it.isNotBlank() }
        if (words.isEmpty()) return tracks
        return tracks.filter { track ->
            val haystack = buildString {
                append(track.title.lowercase())
                append(' ')
                append(track.artist.lowercase())
                append(' ')
                append(track.album?.lowercase().orEmpty())
            }
            words.all { haystack.contains(it) }
        }
    }

    // ── The library as albums, because that is how a collection is browsed ───
    //
    // A flat title-ordered list of two thousand tracks is a search result, not a
    // library: it puts one record's third song next to a different record's
    // third song, and the only way to find an album is to remember the name of
    // something on it. A collection is browsed by cover — which is also the one
    // piece of metadata a phone reliably has, since `MediaStore` gives artwork
    // per album and nothing per track.
    //
    // So albums are what the browser shows and the flat list is what a *query*
    // produces. [browse] is the single place that chooses between them, which is
    // what keeps the screen from having a fourth opinion about it.

    /** Who an album is by, which is three different answers and not two. */
    sealed interface AlbumArtist {
        /** Every track that said anything said the same thing. */
        data class One(val name: String) : AlbumArtist

        /**
         * They disagree — a compilation, or a record with a guest credited per
         * track. Naming the first one would be a claim about the album that one
         * of its own tracks contradicts.
         */
        data object Various : AlbumArtist

        /**
         * Nobody said.
         *
         * Deliberately not folded into [Various]: an untagged rip is not a
         * compilation, and a tile reading "Various artists" over four songs from
         * one gig would be inventing the one fact the files withheld.
         */
        data object Unknown : AlbumArtist
    }

    /**
     * One album's worth of the library, ready to draw as a tile.
     *
     * [title] is `null` for the one group that is not an album: tracks no row of
     * which named one. They are kept rather than dropped — a phone full of
     * untagged rips would otherwise browse as an empty library — and the screen
     * names that group, because this file has no strings in it. Exactly one such
     * group exists whatever the library looks like; [albumsOf] is where that is
     * arranged and a test is what holds it.
     */
    data class Album(
        val key: String,
        val title: String?,
        val artist: AlbumArtist,
        val tracks: List<Track>,
    ) {
        init {
            // Only [albumsOf] builds these and it builds them out of a grouping,
            // so an empty one cannot arise there. Refused rather than tolerated
            // because [cover] reads `first()`, and a lenient `firstOrNull` would
            // push a nullability the screen has nothing useful to do with.
            require(tracks.isNotEmpty()) { "an album with no tracks is not an album" }
        }

        /**
         * The track whose artwork stands for the album.
         *
         * The first, not a search for one that has a cover: whether a file has
         * artwork is not knowable without asking `MediaStore`, which is IO, and
         * a grouping function that did IO would stop being testable — the whole
         * reason this file is worth having.
         */
        val cover: Track get() = tracks.first()
    }

    /** The group tracks land in when their file names no album at all. */
    const val NO_ALBUM_KEY = "none"

    /**
     * The library, grouped into albums, in the order they are offered.
     *
     * Sorted by album title rather than left in the order the tracks arrived.
     * That is the opposite of [filterTracks]' rule and for the same underlying
     * reason: this list is not re-computed under a moving finger — a query
     * switches to [Browse.Tracks] instead of re-ranking these — so alphabetical
     * is a help here where it would be a jumping list there. The provider's
     * track order is title-ordered, so grouping alone would sort records by
     * whichever of their songs happens to come first in the alphabet.
     *
     * **Every track lands in exactly one album**, which is pinned by a test
     * rather than left to reading: a grouping that silently dropped the rows it
     * could not classify would make part of someone's music unreachable, and the
     * only symptom would be a library that looks smaller than it is.
     */
    fun albumsOf(tracks: List<Track>): List<Album> {
        val groups = LinkedHashMap<String, MutableList<Track>>()
        for (track in tracks) groups.getOrPut(albumKeyOf(track)) { mutableListOf() } += track

        // **A group is leftovers when no row in it names an album — not when a
        // row lacks a tag**, and the difference is a whole record.
        //
        // One file in a rip losing its `ALBUM` tag is ordinary: a re-encode, an
        // edit by another app. Deciding per row would take that one track out of
        // its own record — a tile reading "11 tracks", a queue that can never
        // reach the twelfth, and the twelfth sitting alone at the bottom of the
        // library under "not in an album". `MediaStore` still groups it by
        // `ALBUM_ID`, so it stays there and the *group* is asked for the name.
        //
        // Folding every still-untitled group into one is then what lets
        // [Album.title] promise `null` for exactly one of them — an id is a
        // number in a column, not evidence that anything was tagged, so an
        // id-keyed group where nothing was named is not an album either, and two
        // untitled tiles would be two rows headed the same thing.
        val albums = mutableListOf<Album>()
        val leftovers = mutableListOf<Track>()
        for ((key, rows) in groups) {
            val title = rows.firstNotNullOfOrNull { named(it.album) }
            if (title == null) {
                leftovers += rows
            } else {
                albums += Album(
                    key = key,
                    title = title,
                    artist = albumArtistOf(rows),
                    // Within an album, the provider's order is kept — which is
                    // title order, not track order, because
                    // `MediaStore.Audio.Media.TRACK` is not in the projection. A
                    // stated gap: it is the one thing that would make an album
                    // read the way the record does.
                    tracks = rows.toList(),
                )
            }
        }
        if (leftovers.isNotEmpty()) {
            albums += Album(
                key = NO_ALBUM_KEY,
                title = null,
                artist = albumArtistOf(leftovers),
                tracks = leftovers.toList(),
            )
        }
        return albums.sortedWith(
            compareBy(
                // The no-album group last, wherever its name would have put it:
                // it is the leftovers, and leftovers do not belong between two
                // records.
                { it.title == null },
                { it.title?.lowercase() ?: "" },
            ),
        )
    }

    /**
     * Which group a track belongs to — **not** whether that group is an album.
     * [albumsOf] decides the second thing, by asking the assembled group whether
     * anything in it named a record; a row with an id and no `ALBUM` tag belongs
     * with its siblings, and only a group where *nobody* named one is leftovers.
     *
     * The album id first, because two records can share a name and `MediaStore`
     * is the only thing that knows they are different. The title is the fallback
     * and it is deliberately the *whole* key: a name collision between two
     * id-less records merges them into one tile, which is accepted rather than
     * fixed by adding the artist — this path is the untagged remainder, and
     * splitting *that* by artist would scatter a compilation nobody tagged into
     * one tile per guest, which is the worse of the two wrong answers.
     */
    fun albumKeyOf(track: Track): String {
        track.albumId?.let { return "id:$it" }
        val title = named(track.album) ?: return NO_ALBUM_KEY
        return "name:${title.lowercase()}"
    }

    private fun albumArtistOf(tracks: List<Track>): AlbumArtist {
        // Case-insensitively distinct, keeping the first spelling: two rips of
        // one record that disagree about capitalisation are not a compilation.
        val names = tracks.mapNotNull { named(it.artist) }.distinctBy { it.lowercase() }
        return when {
            names.isEmpty() -> AlbumArtist.Unknown
            names.size == 1 -> AlbumArtist.One(names.first())
            else -> AlbumArtist.Various
        }
    }

    /** What the library browser is showing. */
    sealed interface Browse {
        /** Nothing typed: the collection, by cover. */
        data class Albums(val albums: List<Album>) : Browse

        /** A query: the matching tracks, in the provider's order. */
        data class Tracks(val tracks: List<Track>) : Browse
    }

    /**
     * Covers to browse, or tracks to pick from — decided by whether anything has
     * been typed.
     *
     * A search is for a song at least as often as for a record, and an album
     * grid cannot show which of an album's tracks matched. So typing switches to
     * the list, which is also why the grid never has to re-rank: it is only ever
     * the whole library.
     */
    fun browse(tracks: List<Track>, query: String): Browse =
        if (query.isBlank()) {
            Browse.Albums(albumsOf(tracks))
        } else {
            Browse.Tracks(filterTracks(tracks, query))
        }

    /**
     * What the Together tab can offer, given what it is allowed to do.
     *
     * Three sources, and the screen shows all three of them whatever the
     * permission state is — what changes is the *step* each one takes. A source
     * that vanished when a permission was refused would leave someone with no
     * way back to it, which is the failure the invitation screen's
     * `MediaLibraryAccess.next` already avoids for the same permission.
     */
    sealed interface Source {
        /**
         * The phone's own music. [needsPermission] when the library has not
         * been granted — the browser then shows the ask rather than an empty
         * list, because "no music on this phone" and "not allowed to look" are
         * different sentences.
         */
        data class OnThisPhone(val needsPermission: Boolean) : Source

        /** A file picked by hand. Needs no permission — SAF grants per file. */
        data object PickAFile : Source

        /** A pasted link: a video, or a public media URL. */
        data object FromALink : Source

        /**
         * Search a public catalogue by name — the only source that names a
         * recording rather than pointing at a file.
         *
         * It is the one source that tells a third party anything, which is why
         * it sits last and why [SearchOutcome] insists on naming which catalogue
         * answered. What leaves the phone is the query text and nothing else.
         */
        data object SearchByName : Source

        /**
         * Search a streaming server you own — Subsonic/Navidrome and kin, the
         * online-streaming source this app sanctions (`docs/TOGETHER.md` §23).
         *
         * It reaches **your** server with your credentials, which makes it a
         * different sentence from [SearchByName]: the catalogue asks a third
         * party about a recording, this one asks your own machine for a file
         * it already serves you. Offered after the catalogue because it needs
         * setup the others do not — a server, a user, a password — and a card
         * that leads to settings on first touch must sit below the cards that
         * just work.
         */
        data object YourServer : Source

        /**
         * Keyless public collections — the Internet Archive's audio and any
         * podcast feed. The online sources that need no account and no server
         * of one's own; offered last because two keyboard screens in a row is
         * already a lot of typing for a tab whose first card needs none.
         */
        data object PublicCollections : Source
    }

    /**
     * The sources, in the order they are offered.
     *
     * On-device first, deliberately: it is the one that needs no typing, no
     * network and no account, and it is what "listen to music together" means
     * most of the time. The keyboard ones come last, and search comes after the
     * link field because it is the only source that reaches a third party at
     * all. Your own server comes after even that: it reaches only where you
     * already have an account, but it needs setting up first, and the cards
     * that just work sit above the one that asks for credentials.
     */
    fun sources(libraryGranted: Boolean): List<Source> = listOf(
        Source.OnThisPhone(needsPermission = !libraryGranted),
        Source.PickAFile,
        Source.FromALink,
        Source.SearchByName,
        Source.YourServer,
        Source.PublicCollections,
    )

    // ── Searching a catalogue by name ────────────────────────────────────────

    /**
     * One catalogue answer, ready to draw.
     *
     * [catalogue] is not decoration and must not be dropped to save a line:
     * `CatalogueResolver::name`'s contract is that *a guess presented without
     * its source is worse than no guess*. A row saying "Kun Faya Kun — A. R.
     * Rahman" with no indication that MusicBrainz said so is a claim this app
     * cannot stand behind.
     */
    data class Candidate(
        val title: String,
        val artist: String,
        val album: String?,
        val durationMs: Long,
        val catalogue: String,
        /** `true` when [durationMs] came from the catalogue rather than a guess. */
        val durationKnown: Boolean,
    )

    /**
     * What a search came back with — **five outcomes, not a list and a spinner.**
     *
     * The distinction the shapes exist for is [NoCatalogue] versus [NothingFound].
     * They are one pixel apart on screen and opposite in meaning: one says this
     * build cannot search, the other says the catalogue has no such recording.
     * Collapsing them tells somebody their song does not exist because a Cargo
     * feature is off, which is a wrong answer delivered confidently — see
     * `UiError::CatalogueUnavailable`.
     */
    sealed interface SearchOutcome {
        /** Nothing typed yet, or the field was cleared. */
        data object Idle : SearchOutcome

        /** A lookup is in flight. */
        data object Searching : SearchOutcome

        /** Answers, most likely first, as the catalogue ordered them. */
        data class Found(val candidates: List<Candidate>) : SearchOutcome

        /** The catalogue was reached and has no such recording. */
        data object NothingFound : SearchOutcome

        /**
         * This build has no catalogue at all (`catalogue-http` off). Not a
         * failure of the query and must never be rendered as one.
         */
        data object NoCatalogue : SearchOutcome

        /** The lookup itself failed — offline, timed out, rate-limited. */
        data class Failed(val reason: String) : SearchOutcome

        /**
         * The catalogue knew the song and this phone has no copy of it.
         *
         * **A sixth outcome rather than a [Failed] with a different sentence,
         * because the search did not fail — it succeeded.** Reusing [Failed] here
         * is a bug that shipped: the screen renders that state as
         * "Couldn't search: …", so a search that worked perfectly reported itself
         * as a failed search, and replacing the outcome also wiped the result list
         * so there was no second candidate left to try.
         *
         * That is the same mistake this type was created to prevent one variant
         * up — collapsing two states whose only difference is meaning. So the
         * candidates are carried through: the list stays on screen, and [wanted]
         * names which row was tapped so the message can be about that song rather
         * than about the search.
         */
        data class NotOnThisPhone(
            val candidates: List<Candidate>,
            val wanted: Candidate,
        ) : SearchOutcome
    }

    /**
     * Turn a completed lookup into what the screen shows.
     *
     * @param unavailable what `UiError::CatalogueUnavailable` came back as — the
     *   caller's single job is to keep this distinct from an error string, and
     *   it is checked first for exactly that reason.
     * @param error any other failure's message, or `null`
     * @param candidates what arrived, possibly empty
     */
    fun searchOutcome(
        unavailable: Boolean,
        error: String?,
        candidates: List<Candidate>,
    ): SearchOutcome = when {
        unavailable -> SearchOutcome.NoCatalogue
        error != null -> SearchOutcome.Failed(error)
        candidates.isEmpty() -> SearchOutcome.NothingFound
        else -> SearchOutcome.Found(candidates)
    }

    /**
     * The outcome after tapping a result that turns out to have no local copy.
     *
     * Its own function so the screen cannot reach for [SearchOutcome.Failed]
     * again: that is what shipped, and it rendered a working search as
     * "Couldn't search: …" while throwing away the list. Keeping [candidates]
     * means the other rows are still there to try, which is the whole reason
     * somebody searched a catalogue that returns several.
     */
    fun notOnThisPhone(candidates: List<Candidate>, wanted: Candidate): SearchOutcome =
        SearchOutcome.NotOnThisPhone(candidates = candidates, wanted = wanted)

    // ── What the status line says when they join ─────────────────────────────

    /**
     * Whether "open your copy to start" is a sentence that means anything for
     * what is playing.
     *
     * **It is not, for an embed or a stream, and saying it anyway is a bug that
     * shipped.** A YouTube session showed "with them · open your copy to start"
     * under YouTube's own "This video is unavailable" panel: there is no copy of
     * an embed to open, and there is none of a stream either — both sides fetch
     * the same URL. Only a `LocalFile` session is waiting on somebody to open a
     * file, because that is the one kind where the invitation carries a length and
     * each side supplies its own bytes.
     *
     * @param embed the session is a YouTube embed
     * @param external we are following another app's `MediaSession` (§13), which
     *   also has no copy of ours to open
     * @param stream the session is a public media URL both sides fetch
     */
    fun needsOwnCopy(embed: Boolean, external: Boolean, stream: Boolean): Boolean =
        !embed && !external && !stream

    /**
     * What tapping a search result does, given the tier core chose.
     *
     * A thin naming of `AudioPlan` rather than a second ladder: the order is
     * `comrade_core::catalogue`'s and re-deciding it here is the drift this file
     * exists to avoid. What is genuinely local is the *sentence* — a screen has
     * to say something different for each, and only the frontend knows its own
     * words.
     */
    sealed interface CandidateAction {
        /** This phone has it. Start a session on the local copy. */
        data class PlayOwnCopy(val confidence: Double) : CandidateAction

        /** The other person has it and can send it over. */
        data object AskThePeer : CandidateAction

        /** An archive whose licence permits a copy — fetch it. */
        data class FetchOpenly(val url: String) : CandidateAction

        /**
         * Nobody can hand over the bytes.
         *
         * The honest floor, and the reason it is not a dead end: an embed still
         * plays, and a name still tells somebody where to open it. A screen that
         * offered a download here would be offering something this app will not
         * do — see `catalogue.rs`'s header.
         */
        data object EmbedOrNameIt : CandidateAction
    }

    /**
     * Name the tier `choose_audio_plan` picked.
     *
     * @param tier the plan's discriminant as the FFI reports it —
     *   `library` · `peer` · `open_licence` · `embed_only`
     * @param confidence the match score, for `library`
     * @param url the fetchable URL, for `open_licence`
     *
     * An unrecognised tier is [CandidateAction.EmbedOrNameIt] rather than a
     * throw: a core that grows a fifth tier this build has never heard of should
     * degrade to the floor, not crash a music player. It is also the only
     * fallback that promises nothing.
     */
    fun candidateAction(tier: String, confidence: Double, url: String?): CandidateAction =
        when (tier) {
            "library" -> CandidateAction.PlayOwnCopy(confidence)
            "peer" -> CandidateAction.AskThePeer
            "open_licence" ->
                if (url.isNullOrBlank()) {
                    CandidateAction.EmbedOrNameIt
                } else {
                    CandidateAction.FetchOpenly(url)
                }
            else -> CandidateAction.EmbedOrNameIt
        }

    /**
     * What a pasted link turns out to be.
     *
     * **The classification is core's, and this only orders the two answers it
     * gives.** `play_query` knows the service hosts and `TogetherContent::stream`
     * knows what a media URL is; a third opinion in Kotlin is exactly the drift
     * `docs/CHAT_ACTIONS.md` §7 records for `/pay`. So the caller passes what
     * core said and this decides which of them wins.
     */
    sealed interface Link {
        /** A YouTube video, playable in the embed. */
        data class Video(val videoId: String) : Link

        /** One public HTTPS media URL both devices will fetch for themselves. */
        data class Stream(val url: String) : Link

        /**
         * Neither — words, or a link to a page rather than to media.
         *
         * Deliberately one answer for both: the next step is the same, which is
         * to look for it in the library, and a screen that told them apart would
         * be claiming to know which it was.
         */
        data object NotPlayable : Link
    }

    /**
     * Order the two answers core gave for the same text.
     *
     * A YouTube link wins, and the reason is the trap: `https://youtu.be/…` is a
     * perfectly valid HTTPS URL, so a stream check that ran first would point a
     * `MediaPlayer` at a web page and produce a blank screen instead of a video.
     * `desktop/ui/stream_link.mjs` makes the same ordering for the same reason.
     *
     * @param videoId what `play_query` resolved a YouTube link to, or `null`
     * @param streamUrl what `TogetherContent::stream` accepted, or `null`
     */
    fun classifyLink(videoId: String?, streamUrl: String?): Link = when {
        !videoId.isNullOrBlank() -> Link.Video(videoId)
        !streamUrl.isNullOrBlank() -> Link.Stream(streamUrl)
        else -> Link.NotPlayable
    }

    /** Someone a session can be started with. */
    data class Listener(val npub: String, val label: String, val comrade: Boolean, val online: Boolean)

    /**
     * Who to offer, and in what order.
     *
     * Comrades before everyone else and online before offline, because starting
     * a session is asking for someone's attention *now* — the person who is
     * there is the one it makes sense to ask. Alphabetical within each group so
     * the list is stable between openings; a list that reorders itself is one
     * where the wrong person gets tapped.
     *
     * The search is the same word-wise match [filterTracks] uses, over the one
     * field a person has.
     */
    fun listenersFor(all: List<Listener>, query: String): List<Listener> {
        val words = query.trim().lowercase().split(' ').filter { it.isNotBlank() }
        return all
            .filter { listener ->
                words.isEmpty() || words.all { listener.label.lowercase().contains(it) }
            }
            .sortedWith(
                compareByDescending<Listener> { it.comrade }
                    .thenByDescending { it.online }
                    .thenBy { it.label.lowercase() }
                    .thenBy { it.npub },
            )
    }

    /**
     * Whether the scrubber may be dragged.
     *
     * Not the same question as whether to *draw* one. An external session
     * reports no duration we can trust, so there is no distance for a thumb to
     * express — and a scrubber that moves but lands nowhere is worse than none,
     * which is the same judgement `ui/TogetherScreen.kt` already made for it.
     */
    fun scrubbable(durationMs: Long, external: Boolean): Boolean = !external && durationMs > 0

    // ── The `TogetherContent` variants, as they cross the bridge ────────────
    //
    // Strings rather than the typed variants for the reason the file header
    // gives: nothing here may import uniffi, or the JVM lane loses the only half
    // of this feature it can check. `RelayConnectionService` flattens the enum
    // to exactly these, so this is the vocabulary, not a parallel one.

    const val YOUTUBE_KIND = "youtube"
    const val STREAM_KIND = "stream"
    const val LOCAL_FILE_KIND = "local_file"

    // ── Staying paired, so a session is with a person and not with a file ───
    //
    // Until 2026-08-08 the unit of this feature was *one thing, played with one
    // person*: every track asked again who to play it with, and the other side
    // got a fresh invitation for each one. That is the right shape for "watch
    // this film with me" and the wrong one for a music player, which is what
    // this tab mostly is — nobody picks a friend once per song.
    //
    // So the **pairing** is the session and the content is what passes through
    // it. None of that is a protocol change: `together_start` refuses while a
    // session exists, so replacing what is playing is still an end and a start
    // on the wire. What makes the pair survive it is [continuesSession], which
    // is the receiving side agreeing that the next invitation from the same
    // person, moments later, is the same evening rather than a new request.

    /** The person this device is playing with, for as long as the session lasts. */
    data class Pairing(val npub: String, val label: String)

    /**
     * Listening on your own — the pairing with nobody in it.
     *
     * **This is what makes the tab a music player rather than only a way to
     * share one**, which is what it was asked to be, and modelling it as a
     * pairing rather than as a second kind of session is the whole trick: the
     * player, the queue, prev/next, the foreground service and the notification
     * are identical, and the only difference is that nothing is sent. One gate
     * (`TogetherManager.sendOut`) reads [isAlone] and returns, instead of a
     * `solo` branch in every one of those places.
     *
     * The empty string is a safe sentinel because a real peer is an `npub1…`
     * bech32 string and can never be blank — pinned by a test rather than left
     * as an assumption. It also fails in the right direction: a bug that let
     * this reach `parse_pubkey` gets a refusal from core, not a message to the
     * wrong person.
     */
    val ALONE: Pairing = Pairing(npub = "", label = "")

    /** Whether this session has nobody else in it. */
    fun isAlone(pairing: Pairing?): Boolean = pairing != null && pairing.npub.isBlank()

    /** What has to happen before something new can start playing. */
    sealed interface StartStep {
        /** Nobody chosen yet — the "and who with?" sheet. */
        data object AskWho : StartStep

        /** Already paired, and nothing of anyone else's is interrupted. */
        data class PlayNow(val pairing: Pairing) : StartStep

        /**
         * Already paired, and **they** are the one playing.
         *
         * The follower may start something as freely as the leader — that is
         * the point of a pair rather than a broadcast — but it stops what the
         * other person put on, on both devices, so it is asked first and their
         * name is in the question. Nothing here decides the wording; the screen
         * does, from [Pairing.label].
         */
        data class ConfirmTakeover(val pairing: Pairing) : StartStep
    }

    /**
     * Whether choosing something to play needs a person, a confirmation, or
     * neither.
     *
     * Deliberately silent about the leader interrupting themselves: replacing
     * your own track is what a next button does, and a dialog on every skip is
     * the thing that would make the queue unusable.
     */
    fun startStep(pairing: Pairing?, sessionLive: Boolean, weLead: Boolean): StartStep = when {
        pairing == null -> StartStep.AskWho
        // Nobody to interrupt and nobody to ask about. Checked before the
        // takeover arm rather than left to `weLead` happening to be true: a
        // solo session that ever reached that arm would put up "stop what  is
        // playing?" — a question about nobody, with a blank where the name goes.
        isAlone(pairing) -> StartStep.PlayNow(pairing)
        sessionLive && !weLead -> StartStep.ConfirmTakeover(pairing)
        else -> StartStep.PlayNow(pairing)
    }

    /**
     * Whether there is anyone to offer to listen with.
     *
     * `false` once a real person is in the session, which is §16's rule — the
     * person is chosen once per session, not once per track — so the button that
     * offers it must go away rather than sit there implying a peer can be
     * swapped mid-session. Listening **alone** is not that case: nobody is being
     * dropped, so the offer stands.
     *
     * Both the button's visibility and [startStepInLibrary] read this, so they
     * cannot disagree about when it applies — the same reason
     * `PlaybackModeDecision.ownershipFor` is asked twice rather than inlined
     * once.
     */
    fun mayChoosePerson(pairing: Pairing?): Boolean = pairing == null || isAlone(pairing)

    /**
     * [startStep], for a tap in the phone's own library — where a tap plays.
     *
     * **This is the library's rule and not the tab's**, and the difference is
     * the point. Pasting a link or picking a file is a gesture aimed at somebody
     * ("watch this with me"), so those routes still ask through [startStep] and
     * are unchanged. Tapping a song in your own collection is what a music
     * player is for, and §18's argument — that a music player insisting on a
     * second person is unusable exactly when it is most useful — applies to the
     * *first* tap just as much as to the session it starts. Asking "and who
     * with?" before a single note plays is that same insistence, one screen
     * earlier.
     *
     * So the sheet becomes something asked for rather than something imposed:
     * [choosingPerson] is the person button having been tapped, and it is the
     * only route from listening alone to listening with someone — which is why
     * it is honoured even when [pairing] is already [ALONE], where [startStep]
     * would answer `PlayNow` and there would be no way out of a solo session
     * short of ending it.
     */
    fun startStepInLibrary(
        pairing: Pairing?,
        choosingPerson: Boolean,
        sessionLive: Boolean,
        weLead: Boolean,
    ): StartStep = when {
        choosingPerson && mayChoosePerson(pairing) -> StartStep.AskWho
        // Nobody chosen and nobody asked for: play it. The pairing with nobody
        // in it, so this is the same call every other route makes and not a
        // second kind of session — see [ALONE].
        pairing == null -> StartStep.PlayNow(ALONE)
        else -> startStep(pairing, sessionLive, weLead)
    }

    /**
     * How long after a session ends its pairing is still the current one.
     *
     * Generous, because it only has to cover an end and a start crossing the
     * same relay back to back, and stingy enough that an invitation arriving
     * minutes later is asked about rather than assumed. A minute is well past
     * any round trip this feature tolerates elsewhere — the session TTL itself
     * is shorter.
     */
    const val PAIRING_GRACE_MS: Long = 60_000

    /**
     * Whether an arriving invitation continues the session we were just in.
     *
     * `true` means the screen must **not** ask again: the person already agreed
     * to listen with them, and being asked once per track is the failure this
     * whole section exists to fix.
     *
     * **[STREAM_KIND] is excluded on purpose, and it is the one interesting
     * line here.** Joining a stream makes a request to a host the *other*
     * person named, which `TogetherManager.onInvited` already refuses to do
     * unasked however confidently core validated the URL. A pairing is
     * agreement to listen together; it is not agreement to fetch from wherever
     * they point next, and quietly widening it here would undo that rule
     * somewhere it is easy to miss.
     *
     * @param endedAtMs when the previous session ended, or `0` for "never" —
     *   which is not the same as "just now" and must not read as inside the
     *   window.
     */
    fun continuesSession(
        pairing: Pairing?,
        fromNpub: String,
        contentKind: String,
        endedAtMs: Long,
        nowMs: Long,
    ): Boolean {
        val paired = pairing ?: return false
        if (paired.npub != fromNpub) return false
        if (endedAtMs <= 0) return false
        val age = nowMs - endedAtMs
        // A clock that stepped backwards is not evidence of anything, the same
        // saturating refusal `StallWatch` makes rather than reading it as zero.
        if (age < 0 || age > PAIRING_GRACE_MS) return false
        return contentKind != STREAM_KIND
    }

    // ── A queue, because prev and next are what a music player is ───────────
    //
    // The queue is whatever list the track was picked out of — the library, or
    // the library as the search field had narrowed it. That is the list the
    // person was looking at when they chose, so it is the one "next" should
    // mean, and it costs nothing to carry.
    //
    // Sources that are not a list get no queue and say so: a pasted link, a
    // file from the picker and a followed external session are each one thing.
    // The buttons stay on screen and go quiet, rather than appearing and
    // disappearing under the thumb.

    data class Queue(val tracks: List<Track>, val index: Int) {
        val current: Track? get() = tracks.getOrNull(index)
    }

    /** The list, positioned at the track that was tapped. `null` if it is not in it. */
    fun queueFrom(tracks: List<Track>, uri: String): Queue? {
        val at = tracks.indexOfFirst { it.uri == uri }
        return if (at < 0) null else Queue(tracks, at)
    }

    /** The next track, or `null` at the end of the queue and with no queue at all. */
    fun nextTrack(queue: Queue?): Track? =
        queue?.tracks?.getOrNull(queue.index + 1)

    /**
     * How long into a track the back button still means "the one before".
     *
     * The convention every music player shares, and it is worth stating why
     * rather than copying: back is pressed for two different reasons — *I
     * missed the start of this* and *I want the last one* — and time-into-track
     * is the only signal that tells them apart.
     */
    const val RESTART_WITHIN_MS: Long = 3_000

    /** What the back button does, which is not always the same thing. */
    sealed interface Back {
        /** Put this one back to the beginning. Always available. */
        data object Restart : Back

        data class Previous(val index: Int) : Back
    }

    /**
     * Back, resolved.
     *
     * Restart is the answer whenever there is nothing before this — including
     * with no queue at all — because a back button that does nothing is the one
     * outcome guaranteed to read as broken, and restarting is both honest and
     * what was probably wanted.
     */
    fun backStep(queue: Queue?, positionMs: Long): Back =
        if (positionMs > RESTART_WITHIN_MS || queue == null || queue.index <= 0) {
            Back.Restart
        } else {
            Back.Previous(queue.index - 1)
        }

    /** The queue moved to [to], or `null` when that is not a track. */
    fun movedTo(queue: Queue, to: Int): Queue? =
        if (to in queue.tracks.indices) queue.copy(index = to) else null

    /**
     * The index permutation a drag-reorder produces: every position once, the
     * item at [from] moved to sit at [to].
     *
     * Mirrors core's `reorder_tracks` (`comrade_ui::runtime`) exactly — both
     * indices clamp into range, and `from == to`, a one-item list and an empty
     * list are no-ops — so the row a finger drags and the row the vault writes
     * end up in the same place. Returned as a permutation of `0 until count`
     * rather than a reordered list so the caller keeps ownership of whatever
     * it is actually reordering (a `LazyColumn`'s items, the persisted tracks)
     * and this stays a pure arithmetic answer.
     */
    fun reorderedOrder(count: Int, from: Int, to: Int): List<Int> {
        if (count <= 0) return emptyList()
        val f = from.coerceIn(0, count - 1)
        val t = to.coerceIn(0, count - 1)
        val order = (0 until count).toMutableList()
        if (f == t) return order
        order.add(t, order.removeAt(f))
        return order
    }

    // ── Player extras: shuffle, repeat, speed, sleep ─────────────────────────

    /**
     * Repeat, as three answers to "what happens when this track ends".
     *
     * [OFF][RepeatMode.OFF] stops, [ALL][RepeatMode.ALL] wraps the queue,
     * [ONE][RepeatMode.ONE] restarts the track that just finished.
     */
    enum class RepeatMode { OFF, ALL, ONE }

    /**
     * A play order for shuffle: every index once, current track kept first.
     *
     * Kept deterministic on an injected [Random] rather than calling
     * `Random.Default`, so a test can pin an order and — more importantly — so
     * the *property* is testable whatever the seed: current first, nothing
     * dropped, nothing twice. Fisher–Yates, the boring correct way.
     */
    fun shuffledOrder(count: Int, keepFirst: Int, random: kotlin.random.Random): List<Int> {
        if (count <= 0) return emptyList()
        val start = keepFirst.coerceIn(0, count - 1)
        val rest = (0 until count).filter { it != start }.toMutableList()
        for (i in rest.size - 1 downTo 1) {
            val j = random.nextInt(i + 1)
            val tmp = rest[i]
            rest[i] = rest[j]
            rest[j] = tmp
        }
        return listOf(start) + rest
    }

    /**
     * Where playback goes when a track ends, or `null` to stop.
     *
     * `order` is [shuffledOrder]'s output when shuffle is on, `null` when off;
     * navigation differs only in which neighbour "next" means. Repeat-one
     * deliberately wins over the queue: the button someone tapped last is the
     * intent in force at the moment the track ends. Wrapping under [ALL]
     * follows the same order shuffle produced, so a shuffled queue repeats
     * shuffled rather than snapping back to file order mid-listen.
     */
    fun nextIndexOnEnd(
        repeat: RepeatMode,
        order: List<Int>?,
        currentIndex: Int,
        count: Int,
    ): Int? {
        if (count <= 0 || currentIndex !in 0 until count) return null
        if (repeat == RepeatMode.ONE) return currentIndex
        if (order != null && order.size == count) {
            val at = order.indexOf(currentIndex)
            if (at >= 0 && at + 1 < order.size) return order[at + 1]
            // End of the order: wrap only when repeat says so.
            return if (repeat == RepeatMode.ALL) order.firstOrNull() else null
        }
        if (currentIndex + 1 < count) return currentIndex + 1
        return if (repeat == RepeatMode.ALL) 0 else null
    }

    /** The index skip-back lands on, honouring a shuffle order when one is live. */
    fun previousIndexWith(order: List<Int>?, currentIndex: Int, count: Int): Int? {
        if (count <= 0 || currentIndex !in 0 until count) return null
        if (order != null && order.size == count) {
            val at = order.indexOf(currentIndex)
            if (at > 0) return order[at - 1]
            return null
        }
        return if (currentIndex - 1 >= 0) currentIndex - 1 else null
    }

    /**
     * Where a *manual* skip-forward lands, or `null` at the end.
     *
     * Deliberately not [nextIndexOnEnd]: a person pressing next while repeat
     * is ONE means "play something else", not "restart this one" — repeat-one
     * answers only what happens when a track ends by itself. Shuffle still
     * governs which "something else", for the same reason it governs the end
     * of a track: under shuffle, file order is not the listen's order.
     */
    fun manualNextIndex(order: List<Int>?, currentIndex: Int, count: Int): Int? {
        if (count <= 0 || currentIndex !in 0 until count) return null
        if (order != null && order.size == count) {
            val at = order.indexOf(currentIndex)
            return order.getOrNull(at + 1)
        }
        return (currentIndex + 1).takeIf { it < count }
    }

    /**
     * Whether a playback-speed control makes sense right now.
     *
     * **Only solo.** In a together session the rate is not yours to set — the
     * correction ladder trims it continuously to hold two playheads in step,
     * and a user multiplier underneath that would fight the very mechanism
     * keeping the two devices together. Listening alone has nobody to stay
     * in step with, so there the control is honest.
     */
    fun speedAllowed(solo: Boolean): Boolean = solo

    /** Speed snapped to the slider's steps and clamped to sane bounds. */
    fun clampSpeed(value: Float): Float {
        val stepped = (value * 20).toInt() / 20f // 0.05 steps
        return stepped.coerceIn(0.5f, 2.0f)
    }

    /** Milliseconds left on a sleep timer, or `null` when none is running. */
    fun sleepRemainingMs(startedAtMs: Long?, durationMs: Long, nowMs: Long): Long? {
        val started = startedAtMs ?: return null
        val remaining = durationMs - (nowMs - started)
        return remaining.takeIf { it > 0 }
    }

    // ── Lyrics ───────────────────────────────────────────────────────────────

    /** One lyric line: when it is sung, and what is sung. */
    data class LyricLine(val atMs: Long, val text: String)

    /**
     * Parse an LRC document into timed lines.
     *
     * Handles the shapes feeds actually contain: several timestamps on one
     * line (`[00:12.00][01:40.50]chorus`), metadata tags (`[ar:]`, `[ti:]`,
     * `[by:]`) which are dropped rather than sung, fractional parts of two or
     * three digits, and lines out of order — sorted here so the consumer can
     * binary-search. Untimed text (plain lyrics) parses to an empty list,
     * which the caller must render as "no synced lyrics" rather than nothing.
     */
    fun parseLrc(text: String): List<LyricLine> {
        val tag = Regex("""\[(\d{1,2}):(\d{1,2})(?:[.:](\d{1,3}))?]""")
        val out = mutableListOf<LyricLine>()
        text.lineSequence().forEach { raw ->
            val stamps = tag.findAll(raw).toList()
            if (stamps.isEmpty()) return@forEach
            val body = raw.substring(stamps.last().range.last + 1).trim()
            if (body.isEmpty()) return@forEach
            stamps.forEach { m ->
                val (mm, ss) = m.groupValues[0].let { _ ->
                    m.groupValues[1].toLong() to m.groupValues[2].toLong()
                }
                val fracRaw = m.groupValues[3]
                val frac = when (fracRaw.length) {
                    0 -> 0L
                    1 -> fracRaw.toLong() * 100
                    2 -> fracRaw.toLong() * 10
                    else -> fracRaw.take(3).toLong()
                }
                out.add(LyricLine(mm * 60_000 + ss * 1_000 + frac, body))
            }
        }
        return out.sortedBy { it.atMs }
    }

    /** Which line is being sung at `posMs`: the last one that started, or `-1`. */
    fun lyricIndexAt(lines: List<LyricLine>, posMs: Long): Int {
        var best = -1
        for (i in lines.indices) {
            if (lines[i].atMs <= posMs) best = i else break
        }
        return best
    }

    // ── Answering an invitation ─────────────────────────────────────────────

    /**
     * What "Join" does, per kind of thing we were invited to.
     *
     * **This is the fix for a Join button that opened a file picker.** A
     * follower who taps join wants what the leader is playing; being handed the
     * document picker and asked to find their own copy of it is a question with
     * no good answer, since the usual reason to be invited is that they do not
     * have the file. So a local file joins by taking *their* copy over the
     * session's own connection (`docs/TOGETHER.md` §9a and §12, which already
     * plays it as it arrives) and the picker becomes the second answer, for the
     * person who does have it and would rather not spend the bytes.
     */
    sealed interface JoinAction {
        /** The embed. Nothing to find and nothing to fetch. */
        data object WatchVideo : JoinAction

        /** Both devices fetch the URL the inviter named. */
        data object FetchTheStream : JoinAction

        /** Pull their copy down the session, and play it as it lands. */
        data object TakeTheirCopy : JoinAction
    }

    fun joinAction(youtube: Boolean, contentKind: String): JoinAction = when {
        youtube || contentKind == YOUTUBE_KIND -> JoinAction.WatchVideo
        contentKind == STREAM_KIND -> JoinAction.FetchTheStream
        else -> JoinAction.TakeTheirCopy
    }

    // ── When the embed refuses ──────────────────────────────────────────────
    //
    // The IFrame player draws its own "This video is unavailable" panel inside
    // the WebView and reports the same thing to us, and until now this side did
    // nothing with it: the error went to logcat, the screen kept its transport,
    // and the session sat there claiming to be waiting for the other person to
    // open something that was never going to open. The panel is YouTube's and
    // cannot be replaced — that is the term of use §11a records — but the
    // *session's* answer to it is ours, and it should be a sentence and a way
    // out rather than silence.

    /** Why the embed will not play this, in the vocabulary the screen needs. */
    sealed interface EmbedFailure {
        /**
         * The owner does not allow it to play outside YouTube.
         *
         * By far the most common one, and the only one with a genuinely useful
         * next step: the video is fine, it just has to be watched over there.
         */
        data object NotEmbeddable : EmbedFailure

        /** The id names nothing — a private, deleted or mistyped video. */
        data object NotFound : EmbedFailure

        /** Something else. Named rather than guessed at. */
        data object Unknown : EmbedFailure
    }

    /**
     * Classify the player's error.
     *
     * Takes text for the same reason [embedState] does — the library's enum is
     * a third-party import this file must not have — and
     * `YoutubeSessionPlayer.errorName` is the single place that maps it.
     */
    fun embedFailure(error: String): EmbedFailure = when (error) {
        // 101 and 150 are the same refusal reported two ways, which is why the
        // library folds them into one constant and so does this.
        "not_embeddable" -> EmbedFailure.NotEmbeddable
        "video_not_found" -> EmbedFailure.NotFound
        else -> EmbedFailure.Unknown
    }

    /**
     * Where to send someone whose video will not play here.
     *
     * `null` for anything that is not an id, rather than a URL with the input
     * pasted into it: this string goes into an `Intent`, and the one rule for
     * building a URL out of something that arrived over the wire is not to.
     * Core's `valid_youtube_id` is the real gate on the way in; this is the
     * same character set, restated where the URL is actually assembled.
     */
    fun watchUrl(videoId: String): String? {
        if (videoId.isBlank() || videoId.length > MAX_VIDEO_ID) return null
        val safe = videoId.all { it.isDigit() || it in 'a'..'z' || it in 'A'..'Z' || it == '-' || it == '_' }
        return if (safe) "https://www.youtube.com/watch?v=$videoId" else null
    }

    /** Ids are eleven characters; the cap is only to bound a hostile string. */
    private const val MAX_VIDEO_ID = 32
}
