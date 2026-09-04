package mullu.comrade.together

/**
 * What a together session needs from a player, whoever owns it.
 *
 * Three things end up behind this, and the reason it exists is that all three
 * were separately blocked on `TogetherManager` being concrete on
 * [TogetherPlayer]:
 *
 * - **[TogetherPlayer]** — our own `MediaPlayer`, for a file on this device or
 *   one handed over. Comrade owns the picture and draws it.
 * - **A YouTube embed** (`docs/TOGETHER.md` §11b) — a `WebView` we own but do
 *   not decode in, reporting position about once a second.
 * - **Someone else's app** (§13) — Spotify, a podcast player, whatever is
 *   installed, driven through its published `MediaSession`. Comrade owns no
 *   player at all and follows.
 *
 * **Both halves ship, and that is the point of the abstraction.** Comrade
 * keeps its own player for the file cases — the sleeve, the video surface, the
 * scrubber all stay — and gains the ability to follow an external one for
 * everything it cannot play itself. A session picks a mode at the start
 * ([PlaybackOwnership]) and the manager drives whichever it got, so neither
 * capability is built at the other's expense.
 *
 * **No Android imports here on purpose.** The interface is the seam, and
 * keeping it framework-free means the mode decision below is checkable with
 * `kotlinc` in this sandbox (`CLAUDE.md`) rather than only in CI — which
 * matters, because `android/` is the priority frontend and the one that cannot
 * otherwise be built here. Anything needing a `Uri`, a `Surface` or a
 * `Context` belongs on the implementation, not on this.
 */
interface SessionPlayer {

    /** Where the playhead is. `0` before there is anything to report. */
    val positionMs: Long

    /** How long the content runs, or `0` when the player cannot say yet. */
    val durationMs: Long

    val isPlaying: Boolean

    /**
     * Whether the player can be *acted on* at all.
     *
     * Named for the `MediaPlayer` case, where `seekTo` before `prepare()`
     * throws — but every implementation has some version of "not yet". An
     * external session has it too: there may simply be no session running.
     */
    val prepared: Boolean

    fun play()

    fun pause()

    fun seekTo(posMs: Long)

    /**
     * Ask for a playback rate.
     *
     * Implementations that cannot express an arbitrary rate should do nothing
     * rather than approximate. The ladder already knows not to ask — a source
     * whose `SyncTuning.can_rate_trim` is false never produces a `Nudge` — so a
     * silent no-op here is a second line of defence and not the mechanism.
     */
    fun setRate(rate: Float)

    /**
     * Set output level, `0f`..`1f`, for ducking under a transient focus loss.
     *
     * A no-op by default and deliberately so: an embed and a followed external
     * session both play through somebody else's output, which this app has no
     * handle on and no business turning down. Ducking is therefore a **best**
     * effort — [TogetherDecisions.FocusOutcome.Duck] says what should happen,
     * and a player that cannot do it leaves the sound where it is rather than
     * pausing as a substitute. Losing a navigation prompt over a film is a
     * smaller failure than a film that stops for one.
     */
    fun setVolume(level: Float) = Unit

    fun release()

    /**
     * The session's poll is about to read this player. Nothing, for a player
     * that always knows where it is.
     *
     * It exists for the implementations that are *told* where they are on
     * somebody else's schedule — an embed reporting once a second, an external
     * app reporting on state changes. Those need a clock that keeps running when
     * the reports stop, because "the reports stopped" is itself the thing worth
     * noticing: a frozen player that is never sampled goes on claiming to play
     * against the last position it happened to mention.
     *
     * A default rather than an abstract member deliberately — [TogetherPlayer]
     * reads a real decoder and has nothing to do here, and making it write an
     * empty override would say otherwise.
     */
    fun onPoll(nowMs: Long) = Unit

    /**
     * How far behind [positionMs] the sound actually leaves this device, in ms.
     *
     * Zero is honest and means "unmeasured", which is what every implementation
     * that does not own an `AudioTrack` must return. Guessing here would put a
     * fabricated offset into the drift arithmetic, which is worse than the
     * accuracy it pretends to add.
     */
    val outputLatencyMs: Long
        get() = 0L
}

/**
 * Who is holding the player for this session.
 *
 * A session decides once, at the start, and the screen follows: [Ours] draws a
 * sleeve, a video surface and a scrubber, [External] draws control-and-status
 * because there is nothing of ours to render and the picture — if any — is in
 * another app's window.
 */
enum class PlaybackOwnership {
    /** Comrade opened it and can draw it. A file, or an embed we host. */
    OURS,

    /** Another app is playing; we follow its session and drive its transport. */
    EXTERNAL,
}

/**
 * Which mode a session should run in, decided from what the content is and what
 * this device can actually do about it.
 *
 * Pure so the JVM lane can pin it, and separate from the players themselves so
 * that "can we play this?" is answered once rather than by whichever
 * implementation happens to be tried first.
 */
object PlaybackModeDecision {

    /**
     * @param contentKind the `TogetherContent` variant's tag as it crosses the
     *   bridge — `local_file` · `youtube` · `service` · `stream` · `now_playing`.
     * @param haveOurCopy this device has the file, or the URL, in hand.
     * @param externalSessionAvailable an external media session is running that
     *   we could follow (`MediaSessionDecisions.pick` returned something).
     */
    fun ownershipFor(
        contentKind: String,
        haveOurCopy: Boolean,
        externalSessionAvailable: Boolean,
    ): PlaybackOwnership? = when (contentKind) {
        // Ours whenever we actually hold it. Following an external app for a
        // file we could open ourselves would trade the sleeve, the surface and
        // an accurate playhead for nothing.
        "local_file" -> if (haveOurCopy) PlaybackOwnership.OURS else null

        // Always ours, and it stopped sharing the arm above on 2026-08-08 when
        // `TogetherManager.joinStream` landed. `haveOurCopy` is the wrong
        // question for a stream: nobody *holds* one, both devices fetch the same
        // public URL, and the URL travelled with the invitation — so there is
        // never a moment where this is "not yet anything" the way a file we have
        // not been handed is. Grouping it with `local_file` made every stream
        // invitation look unplayable, which was true only for as long as Android
        // had no way to open one.
        "stream" -> PlaybackOwnership.OURS

        // We host the WebView, so the embed is ours even though we do not
        // decode it — the picture lands in our window and the scrubber is ours
        // to draw.
        "youtube" -> PlaybackOwnership.OURS

        // A streaming service is only ever somebody else's player. There is no
        // version of this where Comrade holds the audio.
        "service", "now_playing" ->
            if (externalSessionAvailable) PlaybackOwnership.EXTERNAL else null

        // An unrecognised kind is a content variant this build has not learned.
        // Refusing beats guessing at a player for it — the same call
        // `play_flow.mjs` makes for an unknown route.
        else -> null
    }

    /**
     * Whether a mode change mid-session is allowed.
     *
     * It is not, and the reason is worth stating: swapping the player under a
     * live session means tearing down one playhead and standing another up in
     * its place, and the gap between those is a session that is neither
     * following nor playing. A handover *finishing* is the one thing that
     * legitimately changes what this device can do, and it does not change the
     * mode — [PlaybackOwnership.OURS] was already the answer, the file simply
     * arrived (`TogetherManager.onSharedFileReady`).
     */
    fun mayChangeMidSession(): Boolean = false
}
