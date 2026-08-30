package mullu.comrade.together

import mullu.comrade.together.TogetherDecisions.EchoSuppressor
import mullu.comrade.together.TogetherDecisions.Local
import mullu.comrade.together.TogetherDecisions.Op
import mullu.comrade.together.TogetherDecisions.ScrubState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The watch-together decision vectors.
 *
 * Deliberately the same cases as `desktop/ui/together_sync.test.mjs`, with the
 * same numbers, so the two frontends drifting apart is a red test rather than a
 * field bug — the remedy `docs/COMMS_ARCHITECTURE.md` ADR-2 prescribes after the
 * call state machines diverged.
 */
class TogetherDecisionsTest {

    private val playing = Local(posMs = 60_000, playing = true, prepared = true)
    private val paused = Local(posMs = 10_000, playing = false, prepared = true)

    // ── Echo suppression ────────────────────────────────────────────────────

    @Test
    fun aSeekWeAppliedDoesNotComeBackOut() {
        val s = EchoSuppressor()
        s.expect("seek", 42_000, 1_000)
        assertNull(TogetherDecisions.classifyCallback("seek", 42_000, true, s, 1_000))
        assertEquals("the entry should have been consumed", 0, s.size)
    }

    @Test
    fun sampleSnappingStillCountsAsTheSeekWeApplied() {
        val s = EchoSuppressor()
        s.expect("seek", 42_000, 1_000)
        assertNull(TogetherDecisions.classifyCallback("seek", 42_004, true, s, 1_000))
    }

    @Test
    fun aUserSeekDuringAnInFlightApplyIsStillReported() {
        val s = EchoSuppressor()
        s.expect("seek", 42_000, 1_000)
        val emit = TogetherDecisions.classifyCallback("seek", 12_000, true, s, 1_000)
        assertEquals(12_000L, emit?.posMs)
    }

    /**
     * The case that permanently wedges a boolean latch: seeking to the position
     * the player already holds may raise no callback, so nothing ever clears the
     * flag and every later user seek is swallowed.
     */
    @Test
    fun anApplyThatRaisesNoCallbackExpiresInsteadOfGoingDeaf() {
        val s = EchoSuppressor()
        s.expect("seek", 42_000, 1_000)
        val later = 1_000 + TogetherDecisions.SUPPRESS_TTL_MS + 1
        val emit = TogetherDecisions.classifyCallback("seek", 42_000, true, s, later)
        assertNotNull("a stale entry must not keep swallowing user seeks", emit)
        assertEquals(0, s.size)
    }

    @Test
    fun coalescedAppliesLeaveNoEntryThatSwallowsALaterRealSeek() {
        val s = EchoSuppressor()
        s.expect("seek", 42_000, 1_000)
        s.expect("seek", 60_000, 1_000)
        // Only the second one produced a callback.
        assertNull(TogetherDecisions.classifyCallback("seek", 60_000, true, s, 1_000))
        val later = 1_000 + TogetherDecisions.SUPPRESS_TTL_MS + 1
        assertNotNull(TogetherDecisions.classifyCallback("seek", 42_000, true, s, later))
    }

    @Test
    fun anAppliedPauseIsSilentButTheUsersNextOneIsNot() {
        val s = EchoSuppressor()
        s.expect("pause", null, 1_000)
        assertNull(TogetherDecisions.classifyCallback("pause", 10_000, false, s, 1_000))
        assertNotNull(TogetherDecisions.classifyCallback("pause", 10_000, false, s, 1_000))
    }

    @Test
    fun reachingTheEndIsAPauseNeverASeek() {
        val s = EchoSuppressor()
        val emit = TogetherDecisions.classifyCallback("completion", 7_200_000, true, s, 1_000)
        assertEquals(7_200_000L, emit?.posMs)
        assertEquals(false, emit?.playing)
    }

    @Test
    fun theLedgerStaysBoundedUnderAnApplyStorm() {
        val s = EchoSuppressor()
        repeat(100) { s.expect("seek", it.toLong(), 1_000) }
        assertTrue("unbounded ledger: ${s.size}", s.size <= 16)
    }

    // ── Corrections ─────────────────────────────────────────────────────────

    @Test
    fun holdingDoesNothingAtAll() {
        val plan = TogetherDecisions.planCorrection("hold", 0, 1f, true, playing)
        assertTrue(plan.ops.isEmpty() && plan.expect.isEmpty())
    }

    @Test
    fun aRateTrimRaisesNoCallbackSoItArmsNothing() {
        val plan = TogetherDecisions.planCorrection("nudge", 0, 1.04f, true, playing)
        assertEquals(listOf(Op.Rate(1.04f)), plan.ops)
        assertTrue(plan.expect.isEmpty())
    }

    @Test
    fun aRateTheCoreAskedForIsStillClampedToSomethingListenable() {
        val fast = TogetherDecisions.planCorrection("nudge", 0, 3f, true, playing)
        assertEquals(Op.Rate(TogetherDecisions.RATE_MAX), fast.ops.single())
        val slow = TogetherDecisions.planCorrection("nudge", 0, 0.1f, true, playing)
        assertEquals(Op.Rate(TogetherDecisions.RATE_MIN), slow.ops.single())
    }

    /**
     * `setPlaybackParams` on a paused `MediaPlayer` starts playback on several
     * Android versions. A drift correction must never do that.
     */
    @Test
    fun aRateTrimIsRefusedWhileThePlayerIsPaused() {
        val plan = TogetherDecisions.planCorrection("nudge", 0, 1.04f, true, paused)
        assertTrue("a rate trim on a paused player would start it", plan.ops.isEmpty())
    }

    @Test
    fun aCorrectionSeeksButNeverStartsPlayback() {
        val plan = TogetherDecisions.planCorrection("seek", 42_000, 1f, true, paused)
        assertEquals(listOf(Op.Seek(42_000)), plan.ops)
        assertTrue(plan.ops.none { it is Op.Play })
    }

    @Test
    fun adoptingAMissedCommandTakesTheirPlayingStateToo() {
        val plan = TogetherDecisions.planCorrection("adopt", 42_000, 1f, true, paused)
        assertEquals(listOf(Op.Seek(42_000), Op.Play), plan.ops)
    }

    @Test
    fun adoptingAStateWeAreAlreadyInDoesNothing() {
        val plan = TogetherDecisions.planCorrection("adopt", 60_000, 1f, true, playing)
        assertTrue(plan.ops.isEmpty() && plan.expect.isEmpty())
    }

    /** `seekTo` before `prepare()` throws, so nothing is planned until then. */
    @Test
    fun nothingIsPlannedBeforeThePlayerIsPrepared() {
        val unprepared = Local(posMs = 0, playing = false, prepared = false)
        assertTrue(TogetherDecisions.planCorrection("seek", 42_000, 1f, true, unprepared).ops.isEmpty())
        assertTrue(TogetherDecisions.planCommand(42_000, true, unprepared).ops.isEmpty())
    }

    /**
     * The property that makes the feedback loop unreachable rather than merely
     * unlikely: anything that can raise a callback arms a matching expectation,
     * and feeding those callbacks back in produces nothing outbound.
     */
    @Test
    fun everyOpThatCanRaiseACallbackArmsAMatchingExpectation() {
        val verdicts = listOf(
            Triple("hold", 0L, true),
            Triple("nudge", 0L, true),
            Triple("seek", 42_000L, true),
            Triple("adopt", 42_000L, true),
            Triple("adopt", 42_000L, false),
            Triple("something-new", 0L, true),
        )
        for ((kind, pos, play) in verdicts) {
            for (local in listOf(playing, paused)) {
                val plan = TogetherDecisions.planCorrection(kind, pos, 1.02f, play, local)
                val noisy = plan.ops.filterNot { it is Op.Rate }
                assertEquals(
                    "unarmed op in $kind: ${plan.ops}",
                    noisy.size,
                    plan.expect.size,
                )
                val s = EchoSuppressor()
                plan.expect.forEach { (k, p) -> s.expect(k, p, 1_000) }
                for (op in noisy) {
                    val (k, p) = when (op) {
                        is Op.Seek -> "seek" to op.posMs
                        Op.Play -> "play" to local.posMs
                        Op.Pause -> "pause" to local.posMs
                        is Op.Rate -> continue
                    }
                    assertNull(
                        "applying $kind echoed back out",
                        TogetherDecisions.classifyCallback(k, p, play, s, 1_000),
                    )
                }
            }
        }
    }

    // ── The scrubber ────────────────────────────────────────────────────────

    @Test
    fun thePollDoesNotFightAFingerOnTheSlider() {
        assertTrue(TogetherDecisions.pollMayMoveSlider(ScrubState(scrubbing = false, pendingRemoteMs = null)))
        assertTrue(!TogetherDecisions.pollMayMoveSlider(ScrubState(scrubbing = true, pendingRemoteMs = null)))
    }

    @Test
    fun aRemoteSeekMidDragIsQueuedNotApplied() {
        val dragging = ScrubState(scrubbing = true, pendingRemoteMs = null)
        val queued = TogetherDecisions.onRemoteSeek(dragging, 42_000)
        assertEquals(42_000L, queued.pendingRemoteMs)
        assertNull("nothing may be applied while the finger is down", TogetherDecisions.pendingRemoteToApply(queued))
    }

    @Test
    fun theUsersOwnPositionWinsOnRelease() {
        val queued = ScrubState(scrubbing = true, pendingRemoteMs = 42_000)
        val released = TogetherDecisions.onScrubRelease(queued)
        assertNull(
            "a remote seek from mid-drag must not land after the user named a position",
            TogetherDecisions.pendingRemoteToApply(released),
        )
    }

    @Test
    fun aRemoteSeekWithNoFingerDownAppliesStraightAway() {
        val idle = ScrubState(scrubbing = false, pendingRemoteMs = null)
        val next = TogetherDecisions.onRemoteSeek(idle, 42_000)
        assertNull(next.pendingRemoteMs)
    }

    // ── Picture ─────────────────────────────────────────────────────────────
    //
    // The regression these pin: the session had no video surface at all, so a
    // film opened in watch-together played as audio. Nothing above could catch
    // that, because nothing above knew whether there was a picture.

    @Test
    fun aVideoTrackAsksForASurface() {
        val picture = TogetherDecisions.pictureOf(1920, 1080)
        assertEquals(TogetherDecisions.Picture.Video(1920, 1080), picture)
        assertEquals(16f / 9f, TogetherDecisions.aspectRatioOf(picture)!!, 0.001f)
    }

    @Test
    fun anAudioOnlyRecordingAsksForNoSurface() {
        // What MediaPlayer reports for a track with no video: zero, not null.
        val picture = TogetherDecisions.pictureOf(0, 0)
        assertEquals(TogetherDecisions.Picture.None, picture)
        assertNull("audio gets no black rectangle", TogetherDecisions.aspectRatioOf(picture))
    }

    @Test
    fun dimensionsThatArriveBeforeTheFirstFrameReadAsNoPictureYet() {
        // Deliberately the same answer as audio-only: the surface appears when
        // there is something to put on it.
        assertEquals(TogetherDecisions.Picture.None, TogetherDecisions.pictureOf(1920, 0))
        assertEquals(TogetherDecisions.Picture.None, TogetherDecisions.pictureOf(0, 1080))
    }

    /**
     * Compose's `aspectRatio` throws on a non-positive ratio, and the dimensions
     * come from a file the other person chose. A broken or hostile header must
     * not be able to take the screen down.
     */
    @Test
    fun aBrokenHeaderCannotProduceAnUndrawableShape() {
        for ((w, h) in listOf(1 to 1_000_000, 1_000_000 to 1, 3 to 2, 9 to 16)) {
            val ratio = TogetherDecisions.aspectRatioOf(TogetherDecisions.pictureOf(w, h))
            assertNotNull("${w}x$h produced no ratio", ratio)
            assertTrue("${w}x$h escaped the clamp", ratio!! >= TogetherDecisions.MIN_ASPECT)
            assertTrue("${w}x$h escaped the clamp", ratio <= TogetherDecisions.MAX_ASPECT)
        }
    }

    @Test
    fun negativeDimensionsAreAudio() {
        assertEquals(TogetherDecisions.Picture.None, TogetherDecisions.pictureOf(-1920, -1080))
    }

    @Test
    fun onlyPlayingVideoHoldsTheScreenAwake() {
        val video = TogetherDecisions.pictureOf(1920, 1080)
        val audio = TogetherDecisions.pictureOf(0, 0)
        assertTrue(TogetherDecisions.keepScreenOn(video, playing = true))
        assertFalse("a paused film does not need the screen", TogetherDecisions.keepScreenOn(video, playing = false))
        // The case that matters: two hours of music must not burn the battery
        // holding up a screen with nothing on it.
        assertFalse("audio must never hold the screen", TogetherDecisions.keepScreenOn(audio, playing = true))
    }

    // ── What the two measured numbers are allowed to say ────────────────────
    //
    // The same vectors as `desktop/ui/player_view.test.mjs`. These are the
    // rules the whole design rests on — never claim a precision we cannot
    // measure — so two frontends disagreeing about them is worse than neither
    // showing the numbers at all.

    private fun measure(driftMs: Long, qualityMs: Long, ageMs: Long = 1_000) =
        TogetherDecisions.measurement(driftMs, qualityMs, ageMs)

    @Test
    fun aGapSmallerThanOurOwnErrorIsNotReported() {
        // Printing "0.4 s apart" while the error is 0.8 s is invention.
        assertEquals(TogetherDecisions.Drift.Silent, measure(400, 800).drift)
        assertEquals(TogetherDecisions.Drift.Silent, measure(-400, 800).drift)
        // Bigger than the error, and worth saying.
        assertEquals(
            TogetherDecisions.Drift.Gap(1_200, weAreAhead = true),
            measure(1_200, 300).drift,
        )
        assertEquals(
            TogetherDecisions.Drift.Gap(1_200, weAreAhead = false),
            measure(-1_200, 300).drift,
        )
    }

    @Test
    fun aPerfectClockDoesNotLicenceNarratingATinyGap() {
        // A measured error of zero is what a mesh hop approaches; it must not
        // become permission to report a 50 ms gap nobody can hear.
        assertEquals(TogetherDecisions.Drift.Silent, measure(50, 0).drift)
        assertEquals(TogetherDecisions.Drift.Silent, measure(TogetherDecisions.TOGETHER_MS, 0).drift)
    }

    @Test
    fun thePathIsNamedBecauseItDecidesWhatTheGapIsWorth() {
        assertTrue((measure(0, 30).quality as TogetherDecisions.Quality.Known).direct)
        assertTrue((measure(0, 120).quality as TogetherDecisions.Quality.Known).direct)
        assertFalse((measure(0, 600).quality as TogetherDecisions.Quality.Known).direct)
    }

    @Test
    fun anUnmeasuredPathClaimsNothing() {
        for (bad in listOf(0L, -1L)) {
            assertEquals("quality $bad", TogetherDecisions.Quality.Unknown, measure(0, bad).quality)
        }
    }

    @Test
    fun aDirectPathNeverClaimsToBeTighterThanEitherPlayerCanReport() {
        // Desktop prints "±0.05s" for anything at or under the floor rather
        // than the raw figure, and this is where that floor lives.
        val tiny = measure(0, 5).quality as TogetherDecisions.Quality.Known
        assertEquals(TogetherDecisions.QUALITY_FLOOR_MS, tiny.ms)
        assertEquals(2, tiny.decimals)
        // A relayed figure gets one place: the second digit is not real.
        assertEquals(1, (measure(0, 620).quality as TogetherDecisions.Quality.Known).decimals)
    }

    @Test
    fun aReadingOlderThanTwoHeartbeatsIsNotShownAtAll() {
        // Corrections cross the bridge only when the verdict is not "hold", so
        // a screen that keeps the last pair shows a gap closed minutes ago,
        // underneath the word "together".
        val stale = measure(3_200, 100, ageMs = TogetherDecisions.READING_STALE_MS)
        assertEquals(TogetherDecisions.Drift.Silent, stale.drift)
        assertEquals(TogetherDecisions.Quality.Unknown, stale.quality)
    }

    @Test
    fun theErrorLineAgesOutWithTheGapNeverAfterIt() {
        // "±0.05s" left on screen after the gap it qualified has gone would
        // claim we are still measuring, which inverts the point of showing it.
        val old = measure(9_000, 40, ageMs = 60_000)
        assertEquals(TogetherDecisions.Drift.Silent, old.drift)
        assertEquals(TogetherDecisions.Quality.Unknown, old.quality)
    }

    @Test
    fun aSessionWithNoCorrectionYetShowsBlanksNotZeroes() {
        // What the screen computes before the first correction:
        // `System.currentTimeMillis() - 0`.
        val never = TogetherDecisions.measurement(0, 0, ageMs = System.currentTimeMillis())
        assertEquals(TogetherDecisions.Drift.Silent, never.drift)
        assertEquals(TogetherDecisions.Quality.Unknown, never.quality)
    }

    @Test
    fun aFreshReadingInsideTheDeadbandStillNamesThePath() {
        // The gap is not worth mentioning; how well we can see it still is.
        val m = measure(50, 40)
        assertEquals(TogetherDecisions.Drift.Silent, m.drift)
        assertTrue(m.quality is TogetherDecisions.Quality.Known)
    }

    @Test
    fun aClockThatWentBackwardsShowsNothingRatherThanAFutureReading() {
        val m = TogetherDecisions.measurement(3_200, 100, ageMs = -5_000)
        assertEquals(TogetherDecisions.Drift.Silent, m.drift)
        assertEquals(TogetherDecisions.Quality.Unknown, m.quality)
    }

    // ── A player that reports where it is once a second ─────────────────────

    private fun playhead() = TogetherDecisions.CoarsePlayhead()

    @Test
    fun aPlayheadBetweenTicksIsInterpolatedRatherThanStale() {
        // The whole reason this exists. `onCurrentSecond` fires ~1 Hz and the
        // poll reports 4x a second; handing the ladder a reading a second old
        // is how a session invents drift that is not there.
        val p = playhead()
        p.onPlaying(true, 1_000)
        p.onTick(30_000, 1_000)
        assertEquals(30_000, p.estimateMs(1_000))
        assertEquals(30_250, p.estimateMs(1_250))
        assertEquals(30_900, p.estimateMs(1_900))
    }

    @Test
    fun aPausedPlayheadDoesNotAdvance() {
        val p = playhead()
        p.onPlaying(false, 1_000)
        p.onTick(30_000, 1_000)
        assertEquals(30_000, p.estimateMs(9_000))
    }

    @Test
    fun pausingBanksTheTimeSinceTheLastTickInsteadOfDiscardingIt() {
        // Pause 900ms after a tick and that 900ms of real playback is real.
        // Throwing it away reports a position the video has already passed.
        val p = playhead()
        p.onPlaying(true, 1_000)
        p.onTick(30_000, 1_000)
        p.onPlaying(false, 1_900)
        assertEquals(30_900, p.estimateMs(5_000))
    }

    @Test
    fun ourOwnSeekMovesTheEstimateAtOnce() {
        // The bug this prevents is the sticky-trim sawtooth in a different
        // costume: without it, the second after a correction still reports the
        // old position, so the ladder corrects again for a gap it just closed.
        val p = playhead()
        p.onPlaying(true, 1_000)
        p.onTick(30_000, 1_000)
        p.onSeek(120_000, 1_100)
        assertEquals(120_000, p.estimateMs(1_100))
        assertEquals(120_400, p.estimateMs(1_500))
    }

    @Test
    fun aPlayerThatStoppedTickingStopsBeingGuessedAt() {
        // A stalled video, or one the system froze on backgrounding, sends no
        // ticks. An uncapped estimate would advance a standing-still playhead
        // forever and hide the stall from the drift verdict that should see it.
        val p = playhead()
        p.onPlaying(true, 1_000)
        p.onTick(30_000, 1_000)
        val capped = 30_000 + TogetherDecisions.COARSE_EXTRAPOLATE_MAX_MS
        assertEquals(capped, p.estimateMs(1_000 + TogetherDecisions.COARSE_EXTRAPOLATE_MAX_MS))
        assertEquals(capped, p.estimateMs(60_000))
    }

    @Test
    fun aClockThatWentBackwardsDoesNotRewindThePlayhead() {
        val p = playhead()
        p.onPlaying(true, 10_000)
        p.onTick(30_000, 10_000)
        assertEquals(30_000, p.estimateMs(9_000))
    }

    @Test
    fun aResetPlayheadClaimsNothing() {
        val p = playhead()
        p.onPlaying(true, 1_000)
        p.onTick(30_000, 1_000)
        p.reset()
        assertEquals(0, p.estimateMs(99_000))
    }

    // ── What an embed's state means ─────────────────────────────────────────

    @Test
    fun bufferingIsNeverReportedAsAPause() {
        // docs/TOGETHER.md §10. Telling the peer "they paused" because our own
        // video stalled is the worst ping-pong available: they pause, which
        // makes us re-evaluate, which makes them re-evaluate.
        val stalled = TogetherDecisions.embedState("buffering")
        assertEquals(TogetherDecisions.EmbedState.Stalled, stalled)
        assertFalse(TogetherDecisions.embedStateIsWorthSending(stalled))
    }

    @Test
    fun playingAndPausingAreBothWorthSending() {
        assertEquals(
            TogetherDecisions.EmbedState.Live(playing = true),
            TogetherDecisions.embedState("playing"),
        )
        assertEquals(
            TogetherDecisions.EmbedState.Live(playing = false),
            TogetherDecisions.embedState("paused"),
        )
        for (s in listOf("playing", "paused", "ended")) {
            assertTrue(s, TogetherDecisions.embedStateIsWorthSending(TogetherDecisions.embedState(s)))
        }
    }

    @Test
    fun aPlayerThatHasNotStartedHasNoPositionWorthSending() {
        for (s in listOf("unstarted", "video_cued", "unknown", "", "nonsense")) {
            assertEquals(s, TogetherDecisions.EmbedState.NotReady, TogetherDecisions.embedState(s))
            assertFalse(s, TogetherDecisions.embedStateIsWorthSending(TogetherDecisions.embedState(s)))
        }
    }

    // ── A playhead that stands still while the player claims to play ────────

    private fun watch() = TogetherDecisions.StallWatch()

    @Test
    fun aPlayheadAdvancingNormallyIsNeverAStall() {
        val w = watch()
        var t = 1_000L
        for (second in 0 until 30) {
            assertFalse(
                "second $second",
                w.onSample(posMs = second * 1_000L, playing = true, nowMs = t),
            )
            t += 1_000
        }
    }

    @Test
    fun aPlayerStuckOnOneSecondStopsBeingCalledPlaying() {
        // The ad-break signature, docs/TOGETHER.md §9a: the embed says it is
        // playing, and what it is playing is not our video.
        val w = watch()
        assertFalse(w.onSample(12_000, playing = true, nowMs = 1_000))
        assertFalse(w.onSample(12_000, playing = true, nowMs = 2_000))
        assertFalse(
            "two ticks is a late tick, not a stall",
            w.onSample(12_000, playing = true, nowMs = 3_999),
        )
        assertTrue(w.onSample(12_000, playing = true, nowMs = 4_000))
        assertTrue(w.isStalled)
    }

    @Test
    fun theBreakEndingClearsItOnTheFirstTickThatMoves() {
        // The whole value is in the recovery being immediate: one advancing tick
        // and this device is claiming to play again, so the peer's next verdict
        // closes the gap the break opened instead of the session staying held.
        val w = watch()
        w.onSample(12_000, playing = true, nowMs = 1_000)
        assertTrue(w.onSample(12_000, playing = true, nowMs = 5_000))
        assertFalse(w.onSample(13_000, playing = true, nowMs = 6_000))
        assertFalse(w.isStalled)
    }

    @Test
    fun aPausedPlayerIsNotAStall() {
        // It is a pause, and the peer is being told about that through the
        // ordinary path. Calling it a stall as well would hold a session the
        // user deliberately stopped.
        val w = watch()
        for (t in listOf(1_000L, 5_000L, 60_000L)) {
            assertFalse(w.onSample(12_000, playing = false, nowMs = t))
        }
        assertFalse(w.isStalled)
    }

    @Test
    fun aPauseInTheMiddleOfAStallEndsIt() {
        val w = watch()
        w.onSample(12_000, playing = true, nowMs = 1_000)
        assertTrue(w.onSample(12_000, playing = true, nowMs = 5_000))
        assertFalse(w.onSample(12_000, playing = false, nowMs = 5_500))
        // And the pause must not leave the clock armed: resuming starts over.
        assertFalse(w.onSample(12_000, playing = true, nowMs = 6_000))
    }

    @Test
    fun jitterUnderTheEpsilonIsStillAStall() {
        // A coarse player re-reporting the same second as 12_000 then 12_100 has
        // not moved; treating that as movement would make an ad break invisible.
        val w = watch()
        w.onSample(12_000, playing = true, nowMs = 1_000)
        w.onSample(12_100, playing = true, nowMs = 2_000)
        assertTrue(w.onSample(12_050, playing = true, nowMs = 5_000))
    }

    @Test
    fun aClockThatSteppedBackwardsDoesNotInstantlyStall() {
        val w = watch()
        w.onSample(12_000, playing = true, nowMs = 10_000)
        assertFalse(w.onSample(12_000, playing = true, nowMs = 4_000))
    }

    @Test
    fun theFirstSampleIsNeverAStallHoweverLateItIs() {
        // Zero is a real position on a fresh video, so "no sample yet" cannot be
        // spelled as a position — the first tick would otherwise look like a
        // playhead that had been sitting at 0 since the epoch.
        val w = watch()
        assertFalse(w.onSample(0, playing = true, nowMs = 1_700_000_000_000))
    }

    @Test
    fun resetForgetsAStallInProgress() {
        val w = watch()
        w.onSample(12_000, playing = true, nowMs = 1_000)
        assertTrue(w.onSample(12_000, playing = true, nowMs = 5_000))
        w.reset()
        assertFalse(w.isStalled)
        assertFalse(w.onSample(12_000, playing = true, nowMs = 5_100))
    }

    @Test
    fun theStallWindowIsWiderThanTheExtrapolationCap() {
        // Not a coincidence to preserve by luck: the estimate must have gone
        // flat, and been seen to go flat, before anything is called a stall.
        assertTrue(TogetherDecisions.STALL_AFTER_MS > TogetherDecisions.COARSE_EXTRAPOLATE_MAX_MS)
    }

    // ── The clock under the scrubber ────────────────────────────────────────

    @Test
    fun theClockEarnsItsHourFieldRatherThanAlwaysShowingIt() {
        assertEquals("0:00", TogetherDecisions.clock(0))
        assertEquals("0:07", TogetherDecisions.clock(7_400))
        assertEquals("3:07", TogetherDecisions.clock(187_000))
        assertEquals("59:59", TogetherDecisions.clock(3_599_999))
        // The one boundary worth pinning: the minute field pads only once there
        // is an hour in front of it.
        assertEquals("1:00:00", TogetherDecisions.clock(3_600_000))
        assertEquals("1:02:33", TogetherDecisions.clock(3_753_000))
    }

    @Test
    fun theClockTruncatesRatherThanRounding() {
        // Never show a second the playhead has not reached — 2.9s is still 0:02.
        assertEquals("0:02", TogetherDecisions.clock(2_900))
    }

    @Test
    fun abrokenPlayerReadingIsUnremarkableRatherThanNegative() {
        assertEquals("0:00", TogetherDecisions.clock(-5_000))
    }

    @Test
    fun aLengthOfZeroIsNotALengthSoNothingIsClaimedAboutWhatIsLeft() {
        // An embed before it loads, and an external session always. `0:00` here
        // would be a claim that the track has ended.
        assertNull(TogetherDecisions.remainingClock(positionMs = 0, durationMs = 0))
        assertNull(TogetherDecisions.remainingClock(positionMs = 30_000, durationMs = -1))
    }

    @Test
    fun whatIsLeftCountsDownAndSaysSo() {
        assertEquals("-2:00", TogetherDecisions.remainingClock(60_000, 180_000))
        // A playhead past the end (a peer's clamp, a longer local copy) reads as
        // the end rather than as time owed.
        assertEquals("-0:00", TogetherDecisions.remainingClock(200_000, 180_000))
    }

    // ── The library list ────────────────────────────────────────────────────

    private fun track(
        title: String,
        artist: String = "",
        album: String? = null,
        durationMs: Long = 187_000,
        albumId: Long? = 1L,
    ) = TogetherDecisions.Track(
        uri = "content://media/external/audio/media/$title",
        title = title,
        artist = artist,
        album = album,
        durationMs = durationMs,
        albumId = albumId,
    )

    @Test
    fun aTrackSubtitleLeavesOutWhatTheFileNeverSaid() {
        assertEquals(
            "A. R. Rahman · Rockstar · 3:07",
            TogetherDecisions.trackSubtitle(track("Kun Faya Kun", "A. R. Rahman", "Rockstar")),
        )
        // No tags at all: the length is the only true thing left, and there is
        // no orphaned separator in front of it.
        assertEquals("3:07", TogetherDecisions.trackSubtitle(track("track01")))
        assertEquals(
            "",
            TogetherDecisions.trackSubtitle(track("track01", durationMs = 0)),
        )
    }

    @Test
    fun mediaStoresPlaceholderIsNotSomethingToShowSomeone() {
        // `<unknown>` is a detail of the provider, not a fact about the file.
        val row = track("track01", artist = TogetherDecisions.MEDIASTORE_UNKNOWN, album = "<unknown>")
        assertEquals("3:07", TogetherDecisions.trackSubtitle(row))
    }

    @Test
    fun everyWordHasToLandSomewhereButNotAllInTheSameField() {
        // The behaviour worth having over MediaStore's own LIKE: this is how
        // someone searches for music they already own.
        val library = listOf(
            track("Kun Faya Kun", "A. R. Rahman", "Rockstar"),
            track("Nadaan Parindey", "A. R. Rahman", "Rockstar"),
            track("Solaris", "Photek"),
        )
        assertEquals(
            listOf("Kun Faya Kun"),
            TogetherDecisions.filterTracks(library, "rahman kun").map { it.title },
        )
        assertEquals(
            listOf("Kun Faya Kun", "Nadaan Parindey"),
            TogetherDecisions.filterTracks(library, "rockstar").map { it.title },
        )
        assertTrue(TogetherDecisions.filterTracks(library, "rahman photek").isEmpty())
    }

    @Test
    fun anEmptySearchChangesNeitherTheContentsNorTheOrder() {
        val library = listOf(track("Zoo"), track("Apple"))
        for (query in listOf("", "   ")) {
            assertEquals(
                "a filter that re-ranked would make the list jump under the finger",
                listOf("Zoo", "Apple"),
                TogetherDecisions.filterTracks(library, query).map { it.title },
            )
        }
    }

    @Test
    fun searchingIgnoresCase() {
        val library = listOf(track("Kun Faya Kun", "A. R. Rahman"))
        assertEquals(1, TogetherDecisions.filterTracks(library, "RAHMAN").size)
    }

    // ── The library as albums ───────────────────────────────────────────────

    /**
     * Title-ordered, the way `MusicLibrary.page` hands it over — and
     * deliberately *not* in album order, so a grouping that kept the arrival
     * order fails the test below instead of passing it by luck.
     */
    private val collection = listOf(
        track("Aadat", "Atif Aslam", "Jal Pari", albumId = 20L),
        track("Kun Faya Kun", "A. R. Rahman", "Rockstar", albumId = 10L),
        track("Nadaan Parindey", "A. R. Rahman", "Rockstar", albumId = 10L),
        track("Sadda Haq", "Mohit Chauhan", "Rockstar", albumId = 10L),
        track("Tera Mera Rishta", "Mustafa Zahid", "Awarapan", albumId = 30L),
    )

    @Test
    fun aLibraryBrowsesAsRecordsAndNotAsOneRecordsThirdSong() {
        val albums = TogetherDecisions.albumsOf(collection)
        // Alphabetical by album. Arrival order is Jal Pari, Rockstar, Awarapan —
        // because the provider sorts by *track* title, so grouping alone orders
        // records by whichever of their songs happens to come first.
        assertEquals(listOf("Awarapan", "Jal Pari", "Rockstar"), albums.map { it.title })
        val rockstar = albums.last()
        // Inside a record, the order it arrived in is kept.
        assertEquals(
            listOf("Kun Faya Kun", "Nadaan Parindey", "Sadda Haq"),
            rockstar.tracks.map { it.title },
        )
        // The tile's artwork is the first track's, which is the only one a
        // grouping function can name without doing IO.
        assertEquals("Kun Faya Kun", rockstar.cover.title)
    }

    @Test
    fun anAlbumIsByOnePersonOrBySeveralOrByNobody() {
        val albums = TogetherDecisions.albumsOf(collection).associateBy { it.title }
        assertEquals(
            TogetherDecisions.AlbumArtist.One("Atif Aslam"),
            albums.getValue("Jal Pari").artist,
        )
        // A record with a guest on one track is not "by A. R. Rahman", and
        // saying so would be contradicted by its own third row.
        assertEquals(TogetherDecisions.AlbumArtist.Various, albums.getValue("Rockstar").artist)
        // Nobody said, which is not the same claim as "various".
        val untagged = TogetherDecisions.albumsOf(
            listOf(
                track("track01", artist = TogetherDecisions.MEDIASTORE_UNKNOWN, album = "Rip"),
                track("track02", artist = "", album = "Rip"),
            ),
        )
        assertEquals(TogetherDecisions.AlbumArtist.Unknown, untagged.single().artist)
    }

    @Test
    fun twoRecordsThatShareANameStayApartWhenMediaStoreKnowsTheyAreTwo() {
        // The reason the album id is the key and the title only the fallback.
        val albums = TogetherDecisions.albumsOf(
            listOf(
                track("Song A", "One Band", "Greatest Hits", albumId = 1L),
                track("Song B", "Another Band", "Greatest Hits", albumId = 2L),
            ),
        )
        assertEquals(2, albums.size)
        assertEquals(listOf("Greatest Hits", "Greatest Hits"), albums.map { it.title })
        assertNotEquals(albums[0].key, albums[1].key)
    }

    @Test
    fun withNoAlbumIdTheNameIsTheKey() {
        // A stated limit rather than a hidden one: two untagged records that
        // share a name merge, and splitting them by artist would scatter an
        // untagged compilation into one tile per guest instead.
        val albums = TogetherDecisions.albumsOf(
            listOf(
                track("Song A", "One Band", "Greatest Hits", albumId = null),
                track("Song B", "Another Band", "greatest hits", albumId = null),
            ),
        )
        assertEquals(1, albums.size)
        assertEquals(2, albums.single().tracks.size)
    }

    @Test
    fun theTracksThatNameNoAlbumAreKeptAndComeLast() {
        val albums = TogetherDecisions.albumsOf(
            listOf(
                track("Loose One", albumId = null),
                // `<unknown>` is the provider's placeholder, not an album called
                // that — the same rule the subtitle follows.
                track("Loose Two", album = TogetherDecisions.MEDIASTORE_UNKNOWN, albumId = null),
                track("Zebra", album = "Zoology", albumId = 5L),
            ),
        )
        assertEquals(listOf("Zoology", null), albums.map { it.title })
        val leftovers = albums.last()
        assertEquals(TogetherDecisions.NO_ALBUM_KEY, leftovers.key)
        assertEquals(listOf("Loose One", "Loose Two"), leftovers.tracks.map { it.title })
    }

    @Test
    fun everyTrackLandsInExactlyOneAlbum() {
        // The property worth pinning: a grouping that quietly dropped what it
        // could not classify would make part of someone's music unreachable, and
        // the only symptom would be a library that looks smaller than it is.
        val awkward = collection + listOf(
            track("No Album", albumId = null),
            track("Blank Album", album = "   ", albumId = null),
            track("Same Name", album = "Rockstar", albumId = 99L),
        )
        val grouped = TogetherDecisions.albumsOf(awkward).flatMap { it.tracks }
        assertEquals(awkward.size, grouped.size)
        assertEquals(awkward.map { it.uri }.toSet(), grouped.map { it.uri }.toSet())
    }

    @Test
    fun anAlbumWithNoTracksIsNotAnAlbum() {
        // `cover` reads first(); the invariant is refused at construction rather
        // than pushed to the screen as a nullability it cannot draw.
        assertThrows(IllegalArgumentException::class.java) {
            TogetherDecisions.Album(
                key = "id:1",
                title = "Rockstar",
                artist = TogetherDecisions.AlbumArtist.Unknown,
                tracks = emptyList(),
            )
        }
    }

    @Test
    fun theAlbumIdWinsOverTheName() {
        assertEquals("id:7", TogetherDecisions.albumKeyOf(track("x", album = "Rockstar", albumId = 7L)))
        assertEquals(
            "name:rockstar",
            TogetherDecisions.albumKeyOf(track("x", album = "Rockstar", albumId = null)),
        )
        assertEquals(
            TogetherDecisions.NO_ALBUM_KEY,
            TogetherDecisions.albumKeyOf(track("x", album = null, albumId = null)),
        )
        // Grouped with its siblings, because whether that group turns out to be
        // an album is `albumsOf`'s question and not this one's.
        assertEquals("id:7", TogetherDecisions.albumKeyOf(track("x", album = null, albumId = 7L)))
    }

    @Test
    fun aRecordSurvivesOneFileLosingItsAlbumTag() {
        // Ordinary: a re-encode, an edit by another app. Deciding leftovers per
        // *row* would take this track out of its own record — a tile reading
        // "2 tracks", a queue that never reaches the third, and the third alone
        // at the bottom of the library.
        val albums = TogetherDecisions.albumsOf(
            listOf(
                track("Kun Faya Kun", "A. R. Rahman", "Rockstar", albumId = 10L),
                track("Nadaan Parindey", "A. R. Rahman", album = null, albumId = 10L),
                track("Sadda Haq", "Mohit Chauhan", "Rockstar", albumId = 10L),
            ),
        )
        assertEquals(1, albums.size)
        assertEquals("Rockstar", albums.single().title)
        assertEquals(3, albums.single().tracks.size)
    }

    @Test
    fun onlyOneGroupIsEverTheOneWithNoAlbum() {
        // What `Album.title`'s "the one group that is not an album" claims. Two
        // untitled tiles would be two rows named the same thing, which is not a
        // state to explain to somebody.
        val albums = TogetherDecisions.albumsOf(
            listOf(
                track("Untagged A", albumId = 3L),
                track("Untagged B", albumId = 4L),
                track("Untagged C", album = "", albumId = null),
                track("Tagged", album = "Zoology", albumId = 5L),
            ),
        )
        assertEquals(1, albums.count { it.title == null })
        assertEquals(3, albums.single { it.title == null }.tracks.size)
    }

    @Test
    fun typingSwitchesFromCoversToTracks() {
        // A search is for a song at least as often as for a record, and a grid of
        // covers cannot show which track matched.
        for (nothingTyped in listOf("", "   ")) {
            val view = TogetherDecisions.browse(collection, nothingTyped)
            assertTrue("$nothingTyped should browse covers", view is TogetherDecisions.Browse.Albums)
        }
        val searched = TogetherDecisions.browse(collection, "rahman")
        assertEquals(
            TogetherDecisions.Browse.Tracks(TogetherDecisions.filterTracks(collection, "rahman")),
            searched,
        )
        assertEquals(
            listOf("Kun Faya Kun", "Nadaan Parindey"),
            (searched as TogetherDecisions.Browse.Tracks).tracks.map { it.title },
        )
    }

    // ── What is on offer ────────────────────────────────────────────────────

    @Test
    fun everySourceIsOfferedWhateverThePermissionSaysAndOnlyTheStepChanges() {
        // A source that vanished on a refusal would leave no way back to it.
        for (granted in listOf(true, false)) {
            val sources = TogetherDecisions.sources(libraryGranted = granted)
            assertEquals(6, sources.size)
            assertEquals(
                TogetherDecisions.Source.OnThisPhone(needsPermission = !granted),
                sources.first(),
            )
            assertTrue(sources.contains(TogetherDecisions.Source.PickAFile))
            assertTrue(sources.contains(TogetherDecisions.Source.FromALink))
            assertTrue(sources.contains(TogetherDecisions.Source.SearchByName))
            assertTrue(sources.contains(TogetherDecisions.Source.YourServer))
            assertTrue(sources.contains(TogetherDecisions.Source.PublicCollections))
        }
    }

    // ── A pasted link ───────────────────────────────────────────────────────

    @Test
    fun aVideoLinkBeatsTheStreamCheckThatWouldAlsoAcceptIt() {
        // The trap: `https://youtu.be/…` is a valid HTTPS URL too, so a stream
        // check running first would point a MediaPlayer at a web page.
        assertEquals(
            TogetherDecisions.Link.Video("dQw4w9WgXcQ"),
            TogetherDecisions.classifyLink(
                videoId = "dQw4w9WgXcQ",
                streamUrl = "https://youtu.be/dQw4w9WgXcQ",
            ),
        )
    }

    @Test
    fun aMediaUrlIsAStreamAndEverythingElseIsSomethingToLookFor() {
        assertEquals(
            TogetherDecisions.Link.Stream("https://feeds.example.com/ep/42.mp3"),
            TogetherDecisions.classifyLink(null, "https://feeds.example.com/ep/42.mp3"),
        )
        // Core said no to both. Words and a page link get the same answer,
        // because they have the same next step.
        assertEquals(TogetherDecisions.Link.NotPlayable, TogetherDecisions.classifyLink(null, null))
        assertEquals(TogetherDecisions.Link.NotPlayable, TogetherDecisions.classifyLink("", ""))
    }

    // ── Who to ask ──────────────────────────────────────────────────────────

    private fun listener(label: String, comrade: Boolean = false, online: Boolean = false) =
        TogetherDecisions.Listener(npub = "npub_$label", label = label, comrade = comrade, online = online)

    @Test
    fun theListStartsWithTheComradeWhoIsActuallyThere() {
        // Starting a session is asking for someone's attention now.
        val all = listOf(
            listener("Zara"),
            listener("Bo", comrade = true),
            listener("Ana", comrade = true, online = true),
            listener("Yara", online = true),
        )
        assertEquals(
            listOf("Ana", "Bo", "Yara", "Zara"),
            TogetherDecisions.listenersFor(all, "").map { it.label },
        )
    }

    @Test
    fun theOrderIsStableBetweenOpeningsWithinEachGroup() {
        // Two comrades, both offline, same standing: alphabetical, so the wrong
        // person cannot end up under the finger from one opening to the next.
        val all = listOf(listener("Bo", comrade = true), listener("Ana", comrade = true))
        assertEquals(
            listOf("Ana", "Bo"),
            TogetherDecisions.listenersFor(all, "").map { it.label },
        )
    }

    @Test
    fun twoPeopleWithTheSameNameKeepAStableOrderToo() {
        // The label is not unique — an alias can repeat — so the key falls
        // through to the npub rather than to whatever order the store returned.
        val first = TogetherDecisions.Listener("npub_a", "Ana", comrade = true, online = false)
        val second = TogetherDecisions.Listener("npub_b", "Ana", comrade = true, online = false)
        assertEquals(
            listOf("npub_a", "npub_b"),
            TogetherDecisions.listenersFor(listOf(second, first), "").map { it.npub },
        )
    }

    @Test
    fun searchingForAPersonWorksTheSameWayAsSearchingForATrack() {
        val all = listOf(listener("Ana Iyer"), listener("Bo"))
        assertEquals(
            listOf("Ana Iyer"),
            TogetherDecisions.listenersFor(all, "iyer ana").map { it.label },
        )
        assertTrue(TogetherDecisions.listenersFor(all, "zz").isEmpty())
    }

    // ── The scrubber's own precondition ─────────────────────────────────────

    @Test
    fun thereIsNothingToDragWhenNothingKnowsHowLongItIs() {
        assertTrue(TogetherDecisions.scrubbable(durationMs = 180_000, external = false))
        // An embed before it loads.
        assertFalse(TogetherDecisions.scrubbable(durationMs = 0, external = false))
        // Another app's session: a MediaSession carries no duration we can
        // trust, so a thumb would land nowhere.
        assertFalse(TogetherDecisions.scrubbable(durationMs = 180_000, external = true))
    }

    // ── Pairing ─────────────────────────────────────────────────────────────

    private val ana = TogetherDecisions.Pairing("npub_ana", "Ana")

    @Test
    fun theFirstThingPlayedAsksWhoWithAndTheSecondDoesNot() {
        assertEquals(
            TogetherDecisions.StartStep.AskWho,
            TogetherDecisions.startStep(pairing = null, sessionLive = false, weLead = false),
        )
        // The whole point: paired, so choosing a second track plays it.
        assertEquals(
            TogetherDecisions.StartStep.PlayNow(ana),
            TogetherDecisions.startStep(pairing = ana, sessionLive = false, weLead = true),
        )
    }

    @Test
    fun skippingYourOwnTrackIsNotAQuestion() {
        // A dialog on every press of next is what would make a queue unusable.
        assertEquals(
            TogetherDecisions.StartStep.PlayNow(ana),
            TogetherDecisions.startStep(pairing = ana, sessionLive = true, weLead = true),
        )
    }

    @Test
    fun interruptingWhatTheOtherPersonPutOnIsAskedFirst() {
        assertEquals(
            TogetherDecisions.StartStep.ConfirmTakeover(ana),
            TogetherDecisions.startStep(pairing = ana, sessionLive = true, weLead = false),
        )
    }

    @Test
    fun tappingASongInYourOwnLibraryPlaysIt() {
        // §18's argument, one screen earlier: a music player that stops to ask
        // who with before a single note plays is unusable exactly when it is most
        // useful. The other routes — a pasted link, a picked file — still ask,
        // because those gestures are aimed at somebody.
        assertEquals(
            TogetherDecisions.StartStep.PlayNow(TogetherDecisions.ALONE),
            TogetherDecisions.startStepInLibrary(
                pairing = null,
                choosingPerson = false,
                sessionLive = false,
                weLead = false,
            ),
        )
        // And the sheet is still there for whoever asks for it.
        assertEquals(
            TogetherDecisions.StartStep.AskWho,
            TogetherDecisions.startStepInLibrary(
                pairing = null,
                choosingPerson = true,
                sessionLive = false,
                weLead = false,
            ),
        )
    }

    @Test
    fun thePersonButtonIsTheWayOutOfListeningAlone() {
        // The case `startStep` cannot answer: already paired with nobody, so it
        // says PlayNow and a solo session would have no route to a shared one
        // short of ending it.
        assertEquals(
            TogetherDecisions.StartStep.PlayNow(TogetherDecisions.ALONE),
            TogetherDecisions.startStep(
                pairing = TogetherDecisions.ALONE,
                sessionLive = true,
                weLead = true,
            ),
        )
        assertTrue(TogetherDecisions.mayChoosePerson(TogetherDecisions.ALONE))
        assertEquals(
            TogetherDecisions.StartStep.AskWho,
            TogetherDecisions.startStepInLibrary(
                pairing = TogetherDecisions.ALONE,
                choosingPerson = true,
                sessionLive = true,
                weLead = true,
            ),
        )
        // And the row that fires on every tap *after* the first one in a solo
        // session: not armed, so it plays. Asserted through the library's own
        // function rather than inferred from `startStep` above, since that is
        // what the screen calls.
        assertEquals(
            TogetherDecisions.StartStep.PlayNow(TogetherDecisions.ALONE),
            TogetherDecisions.startStepInLibrary(
                pairing = TogetherDecisions.ALONE,
                choosingPerson = false,
                sessionLive = true,
                weLead = true,
            ),
        )
    }

    @Test
    fun thereIsNobodyToOfferOnceThereIsSomebody() {
        // §16's rule: the person is chosen once per session. So the button is not
        // on screen, and an armed flag surviving from before cannot smuggle the
        // sheet back in and swap a peer mid-session.
        assertTrue(TogetherDecisions.mayChoosePerson(null))
        assertFalse(TogetherDecisions.mayChoosePerson(ana))
        for (choosing in listOf(true, false)) {
            assertEquals(
                "an armed person button must not change a paired session",
                TogetherDecisions.StartStep.ConfirmTakeover(ana),
                TogetherDecisions.startStepInLibrary(
                    pairing = ana,
                    choosingPerson = choosing,
                    sessionLive = true,
                    weLead = false,
                ),
            )
            assertEquals(
                TogetherDecisions.StartStep.PlayNow(ana),
                TogetherDecisions.startStepInLibrary(
                    pairing = ana,
                    choosingPerson = choosing,
                    sessionLive = true,
                    weLead = true,
                ),
            )
        }
    }

    @Test
    fun theNextTrackFromThePersonWeArePairedWithIsNotANewInvitation() {
        assertTrue(
            TogetherDecisions.continuesSession(
                pairing = ana,
                fromNpub = "npub_ana",
                contentKind = TogetherDecisions.LOCAL_FILE_KIND,
                endedAtMs = 1_000,
                nowMs = 1_400,
            ),
        )
    }

    @Test
    fun somebodyElseEntirelyStillGetsToAsk() {
        assertFalse(
            TogetherDecisions.continuesSession(
                pairing = ana,
                fromNpub = "npub_bo",
                contentKind = TogetherDecisions.LOCAL_FILE_KIND,
                endedAtMs = 1_000,
                nowMs = 1_400,
            ),
        )
        assertFalse(
            TogetherDecisions.continuesSession(
                pairing = null,
                fromNpub = "npub_ana",
                contentKind = TogetherDecisions.LOCAL_FILE_KIND,
                endedAtMs = 1_000,
                nowMs = 1_400,
            ),
        )
    }

    @Test
    fun aPairingGoesStaleAndSoDoesOneThatNeverEnded() {
        val late = 1_000 + TogetherDecisions.PAIRING_GRACE_MS + 1
        assertFalse(
            TogetherDecisions.continuesSession(
                ana, "npub_ana", TogetherDecisions.LOCAL_FILE_KIND, endedAtMs = 1_000, nowMs = late,
            ),
        )
        // Zero is "no previous session", which must not read as "just now".
        assertFalse(
            TogetherDecisions.continuesSession(
                ana, "npub_ana", TogetherDecisions.LOCAL_FILE_KIND, endedAtMs = 0, nowMs = 1_400,
            ),
        )
        // A clock that stepped backwards is not evidence of anything.
        assertFalse(
            TogetherDecisions.continuesSession(
                ana, "npub_ana", TogetherDecisions.LOCAL_FILE_KIND, endedAtMs = 5_000, nowMs = 1_400,
            ),
        )
    }

    @Test
    fun beingPairedIsNotAgreementToFetchFromWhereverTheyPointNext() {
        // The one kind that is always asked about: joining a stream makes a
        // request to a host they named, which is a decision about this device's
        // network rather than about listening together.
        assertFalse(
            TogetherDecisions.continuesSession(
                ana, "npub_ana", TogetherDecisions.STREAM_KIND, endedAtMs = 1_000, nowMs = 1_400,
            ),
        )
        assertTrue(
            TogetherDecisions.continuesSession(
                ana, "npub_ana", TogetherDecisions.YOUTUBE_KIND, endedAtMs = 1_000, nowMs = 1_400,
            ),
        )
    }

    // ── The queue ───────────────────────────────────────────────────────────

    private val threeTracks = listOf(track("One"), track("Two"), track("Three"))

    @Test
    fun theQueueIsTheListTheTrackWasPickedOutOf() {
        val queue = TogetherDecisions.queueFrom(threeTracks, threeTracks[1].uri)
        assertNotNull(queue)
        assertEquals("Two", queue?.current?.title)
        assertEquals("Three", TogetherDecisions.nextTrack(queue)?.title)
    }

    @Test
    fun aTrackThatIsNotInTheListGetsNoQueue() {
        // A pasted link, a file from the picker: one thing, no list.
        assertNull(TogetherDecisions.queueFrom(threeTracks, "content://elsewhere"))
        assertNull(TogetherDecisions.nextTrack(null))
    }

    @Test
    fun thereIsNoNextTrackAtTheEnd() {
        val queue = TogetherDecisions.queueFrom(threeTracks, threeTracks[2].uri)
        assertNull(TogetherDecisions.nextTrack(queue))
    }

    @Test
    fun backMeansTheStartOfThisOneOrTheLastOneDependingOnWhen() {
        val queue = TogetherDecisions.queueFrom(threeTracks, threeTracks[1].uri)
        // Straight away: they meant the previous track.
        assertEquals(
            TogetherDecisions.Back.Previous(0),
            TogetherDecisions.backStep(queue, positionMs = 900),
        )
        // Well into it: they missed the start of this one.
        assertEquals(
            TogetherDecisions.Back.Restart,
            TogetherDecisions.backStep(queue, positionMs = TogetherDecisions.RESTART_WITHIN_MS + 1),
        )
    }

    @Test
    fun backAlwaysDoesSomethingEvenWithNothingBehindIt() {
        // A back button that does nothing reads as broken, so the first track
        // and the no-queue case both restart.
        val first = TogetherDecisions.queueFrom(threeTracks, threeTracks[0].uri)
        assertEquals(TogetherDecisions.Back.Restart, TogetherDecisions.backStep(first, positionMs = 0))
        assertEquals(TogetherDecisions.Back.Restart, TogetherDecisions.backStep(null, positionMs = 0))
    }

    @Test
    fun theQueueOnlyMovesToATrackThatExists() {
        val queue = TogetherDecisions.Queue(threeTracks, 1)
        assertEquals(2, TogetherDecisions.movedTo(queue, 2)?.index)
        assertNull(TogetherDecisions.movedTo(queue, 3))
        assertNull(TogetherDecisions.movedTo(queue, -1))
    }

    @Test
    fun aDragReorderMovesOneRowAndClampsTheRest() {
        // The dragged row lands *at* `to`; every other row closes the gap.
        assertEquals(listOf(1, 2, 3, 0), TogetherDecisions.reorderedOrder(4, 0, 3))
        assertEquals(listOf(3, 0, 1, 2), TogetherDecisions.reorderedOrder(4, 3, 0))
        assertEquals(listOf(0, 2, 1, 3), TogetherDecisions.reorderedOrder(4, 1, 2))
        // Out of range is a drag to the nearest end, matching core's clamp.
        assertEquals(listOf(1, 2, 3, 0), TogetherDecisions.reorderedOrder(4, 0, 99))
        assertEquals(listOf(3, 0, 1, 2), TogetherDecisions.reorderedOrder(4, 99, 0))
        assertEquals(listOf(1, 2, 3, 0), TogetherDecisions.reorderedOrder(4, -5, 3))
        // No-ops round-trip untouched.
        assertEquals(listOf(0, 1, 2, 3), TogetherDecisions.reorderedOrder(4, 2, 2))
        assertEquals(listOf(0), TogetherDecisions.reorderedOrder(1, 0, 0))
        assertTrue(TogetherDecisions.reorderedOrder(0, 0, 1).isEmpty())
    }

    // ── Answering an invitation ─────────────────────────────────────────────

    @Test
    fun joiningALocalFileTakesTheirCopyRatherThanOpeningAPicker() {
        // The bug this replaced: tapping Join handed the follower the document
        // picker and asked them to find a file the invitation exists because
        // they do not have.
        assertEquals(
            TogetherDecisions.JoinAction.TakeTheirCopy,
            TogetherDecisions.joinAction(youtube = false, contentKind = TogetherDecisions.LOCAL_FILE_KIND),
        )
        assertEquals(
            TogetherDecisions.JoinAction.WatchVideo,
            TogetherDecisions.joinAction(youtube = true, contentKind = TogetherDecisions.YOUTUBE_KIND),
        )
        assertEquals(
            TogetherDecisions.JoinAction.FetchTheStream,
            TogetherDecisions.joinAction(youtube = false, contentKind = TogetherDecisions.STREAM_KIND),
        )
    }

    // ── When the embed refuses ──────────────────────────────────────────────

    @Test
    fun theEmbedsRefusalIsToldApartBecauseTheNextStepDiffers() {
        assertEquals(
            TogetherDecisions.EmbedFailure.NotEmbeddable,
            TogetherDecisions.embedFailure("not_embeddable"),
        )
        assertEquals(
            TogetherDecisions.EmbedFailure.NotFound,
            TogetherDecisions.embedFailure("video_not_found"),
        )
        assertEquals(TogetherDecisions.EmbedFailure.Unknown, TogetherDecisions.embedFailure("html_5_player"))
    }

    @Test
    fun theWayOutIsBuiltOnlyFromSomethingThatIsActuallyAnId() {
        assertEquals(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            TogetherDecisions.watchUrl("dQw4w9WgXcQ"),
        )
        // This string goes into an Intent. Anything that is not an id gets no
        // URL rather than a URL with it pasted in.
        assertNull(TogetherDecisions.watchUrl(""))
        assertNull(TogetherDecisions.watchUrl("abc&sub=1"))
        assertNull(TogetherDecisions.watchUrl("../../evil"))
        assertNull(TogetherDecisions.watchUrl("a".repeat(64)))
    }
    // ── Listening alone ─────────────────────────────────────────────────────

    @Test
    fun aPairingWithNobodyInItIsTheOneThatMeansAlone() {
        assertTrue(TogetherDecisions.isAlone(TogetherDecisions.ALONE))
        assertFalse(TogetherDecisions.isAlone(TogetherDecisions.Pairing("npub1abc", "Ana")))
        // Not chosen yet is not the same as chosen to be alone — `startStep`
        // asks who in the first case and must never confuse it with the second.
        assertFalse(TogetherDecisions.isAlone(null))
    }

    @Test
    fun theSoloSentinelCannotCollideWithARealPeer() {
        // The whole safety of an empty npub as the sentinel: a real one is a
        // bech32 string and can never be blank. Pinned rather than assumed,
        // because the consequence of a collision is a send to the wrong person.
        for (real in listOf("npub1abc", "  npub1abc  ", "0", "-")) {
            assertFalse(
                "a real peer id read as alone: $real",
                TogetherDecisions.isAlone(TogetherDecisions.Pairing(real, "x")),
            )
        }
        // Whitespace is not a peer either — it is the same nobody.
        assertTrue(TogetherDecisions.isAlone(TogetherDecisions.Pairing(" ", "")))
    }

    @Test
    fun beingAloneIsStillAChosenAnswerSoNothingAsksAgain() {
        // `startStep` must treat it as settled: the sheet has been answered, and
        // re-opening it on the next track is the failure the pairing exists to
        // fix — for one listener as much as for two.
        val step = TogetherDecisions.startStep(
            pairing = TogetherDecisions.ALONE,
            sessionLive = true,
            weLead = true,
        )
        assertEquals(
            TogetherDecisions.StartStep.PlayNow(TogetherDecisions.ALONE),
            step,
        )
    }

    @Test
    fun aloneNeverAsksToTakeOverFromNobody() {
        // The takeover arm reads `weLead`, and a solo session has no leader
        // because it has no second person. Whichever way that flag happens to
        // fall, the question "stop what … is playing?" has a blank where the
        // name goes — so alone is answered before the arm is reached.
        for (weLead in listOf(true, false)) {
            assertEquals(
                "asked about nobody with weLead=$weLead",
                TogetherDecisions.StartStep.PlayNow(TogetherDecisions.ALONE),
                TogetherDecisions.startStep(
                    pairing = TogetherDecisions.ALONE,
                    sessionLive = true,
                    weLead = weLead,
                ),
            )
        }
    }

    // ── Searching a catalogue by name ────────────────────────────────────────

    @Test
    fun searchIsOfferedLastAmongTheNoSetupSourcesBecauseItTellsAThirdPartyThings() {
        val offered = TogetherDecisions.sources(libraryGranted = true)
        // Your own server needs credentials before it can do anything at all,
        // so it sits below the cards that just work — but it does not outrank
        // the catalogue's disclosure either, so the catalogue keeps its place
        // ahead of every networked-but-unconfigured option.
        assertEquals(
            "the collections card must be offered last",
            TogetherDecisions.Source.PublicCollections,
            offered.last(),
        )
        assertTrue(
            "the server card must sit below the catalogue too — it also needs setup",
            offered.indexOf(TogetherDecisions.Source.SearchByName) <
                offered.indexOf(TogetherDecisions.Source.YourServer),
        )
        assertEquals(
            "on-device music must still be first",
            TogetherDecisions.Source.OnThisPhone(needsPermission = false),
            offered.first(),
        )
        assertTrue(
            "the catalogue must sit before the server card",
            offered.indexOf(TogetherDecisions.Source.SearchByName) <
                offered.indexOf(TogetherDecisions.Source.YourServer),
        )
        assertTrue(
            "the catalogue must sit after the sources that need nothing but the phone",
            offered.indexOf(TogetherDecisions.Source.FromALink) <
                offered.indexOf(TogetherDecisions.Source.SearchByName),
        )
    }

    /**
     * The distinction the whole outcome type exists for. If these two ever
     * collapse, the app tells somebody their song does not exist because a Cargo
     * feature is off.
     */
    @Test
    fun aBuildWithNoCatalogueIsNotTheSameAsARecordingThatDoesNotExist() {
        assertEquals(
            TogetherDecisions.SearchOutcome.NoCatalogue,
            TogetherDecisions.searchOutcome(unavailable = true, error = null, candidates = emptyList()),
        )
        assertEquals(
            TogetherDecisions.SearchOutcome.NothingFound,
            TogetherDecisions.searchOutcome(unavailable = false, error = null, candidates = emptyList()),
        )
        assertNotEquals(
            TogetherDecisions.searchOutcome(unavailable = true, error = null, candidates = emptyList()),
            TogetherDecisions.searchOutcome(unavailable = false, error = null, candidates = emptyList()),
        )
    }

    @Test
    fun unavailableWinsOverAnErrorStringSoTheReasonIsNeverMisreported() {
        // Both can be set by a caller that maps errors sloppily. "This build
        // cannot search" is the more specific and more useful answer, so it is
        // checked first rather than being shadowed by a generic failure.
        assertEquals(
            TogetherDecisions.SearchOutcome.NoCatalogue,
            TogetherDecisions.searchOutcome(
                unavailable = true,
                error = "this build has no catalogue support",
                candidates = emptyList(),
            ),
        )
    }

    @Test
    fun aFailedLookupKeepsItsReasonRatherThanBecomingAnEmptyList() {
        assertEquals(
            TogetherDecisions.SearchOutcome.Failed("musicbrainz timed out"),
            TogetherDecisions.searchOutcome(
                unavailable = false,
                error = "musicbrainz timed out",
                candidates = emptyList(),
            ),
        )
    }

    @Test
    fun resultsKeepTheCatalogueOrderTheyArrivedIn() {
        // "Most likely first" is the catalogue's judgement, and re-sorting it
        // here would be substituting ours for one we have no basis for.
        val one = candidate("Kun Faya Kun")
        val two = candidate("Kun Faya Kun (Remix)")
        val outcome = TogetherDecisions.searchOutcome(false, null, listOf(one, two))
        assertEquals(
            TogetherDecisions.SearchOutcome.Found(listOf(one, two)),
            outcome,
        )
    }

    @Test
    fun eachTierNamesADifferentAction() {
        assertEquals(
            TogetherDecisions.CandidateAction.PlayOwnCopy(0.95),
            TogetherDecisions.candidateAction("library", 0.95, null),
        )
        assertEquals(
            TogetherDecisions.CandidateAction.AskThePeer,
            TogetherDecisions.candidateAction("peer", 0.0, null),
        )
        assertEquals(
            TogetherDecisions.CandidateAction.FetchOpenly("https://archive.example/a.flac"),
            TogetherDecisions.candidateAction("open_licence", 0.0, "https://archive.example/a.flac"),
        )
        assertEquals(
            TogetherDecisions.CandidateAction.EmbedOrNameIt,
            TogetherDecisions.candidateAction("embed_only", 0.0, null),
        )
    }

    @Test
    fun anOpenLicenceTierWithNoUrlFallsToTheFloorRatherThanOfferingAFetch() {
        // A button that says "download" and has nothing to download is worse
        // than the honest floor.
        assertEquals(
            TogetherDecisions.CandidateAction.EmbedOrNameIt,
            TogetherDecisions.candidateAction("open_licence", 0.0, null),
        )
        assertEquals(
            TogetherDecisions.CandidateAction.EmbedOrNameIt,
            TogetherDecisions.candidateAction("open_licence", 0.0, "  "),
        )
    }

    @Test
    fun aTierThisBuildHasNeverHeardOfDegradesToTheFloorRatherThanThrowing() {
        // A core that grows a fifth tier must not crash an older music player,
        // and the floor is the only fallback that promises nothing.
        assertEquals(
            TogetherDecisions.CandidateAction.EmbedOrNameIt,
            TogetherDecisions.candidateAction("some_future_tier", 0.0, "https://x.example/a.mp3"),
        )
    }

    private fun candidate(title: String) = TogetherDecisions.Candidate(
        title = title,
        artist = "A. R. Rahman",
        album = "Rockstar",
        durationMs = 470_000,
        catalogue = "MusicBrainz",
        durationKnown = true,
    )

    /**
     * The regression test for a bug that shipped: tapping a search result with no
     * local copy reported "Couldn't search: …" and wiped the result list.
     */
    @Test
    fun aResultWithNoLocalCopyIsNotAFailedSearchAndKeepsTheOtherResults() {
        val one = candidate("Kun Faya Kun")
        val two = candidate("Kun Faya Kun (live)")
        val outcome = TogetherDecisions.notOnThisPhone(candidates = listOf(one, two), wanted = one)

        assertTrue(
            "a search that found the song must not be a Failed search — that renders " +
                "as \"Couldn't search\", which is the bug this pins",
            outcome !is TogetherDecisions.SearchOutcome.Failed,
        )
        val miss = outcome as TogetherDecisions.SearchOutcome.NotOnThisPhone
        assertEquals(
            "the other candidates must survive, or there is nothing left to try",
            listOf(one, two),
            miss.candidates,
        )
        assertEquals(one, miss.wanted)
    }

    @Test
    fun onlyAFileSessionIsWaitingForSomebodyToOpenTheirOwnCopy() {
        // An embed has no copy; a stream has none either, because both devices
        // fetch the same URL; an external session's copy belongs to another app.
        assertFalse(
            "a YouTube embed has no copy to open — this is what shipped saying it did",
            TogetherDecisions.needsOwnCopy(embed = true, external = false, stream = false),
        )
        assertFalse(
            TogetherDecisions.needsOwnCopy(embed = false, external = false, stream = true),
        )
        assertFalse(
            TogetherDecisions.needsOwnCopy(embed = false, external = true, stream = false),
        )
        assertTrue(
            "a local-file session is the one kind where each side supplies its own bytes",
            TogetherDecisions.needsOwnCopy(embed = false, external = false, stream = false),
        )
    }

    // ── Player extras ────────────────────────────────────────────────────────

    @Test
    fun shuffleKeepsTheCurrentTrackFirstAndLosesNothing() {
        for (seed in 1..50) {
            val order = TogetherDecisions.shuffledOrder(8, 3, kotlin.random.Random(seed))
            assertEquals("current track leads", 3, order.first())
            assertEquals(
                "every index exactly once",
                (0 until 8).toSet(),
                order.toSet(),
            )
        }
        assertEquals("a one-track queue shuffles to itself", listOf(0), TogetherDecisions.shuffledOrder(1, 0, kotlin.random.Random(1)))
        assertTrue("an empty queue shuffles to nothing", TogetherDecisions.shuffledOrder(0, 0, kotlin.random.Random(1)).isEmpty())
    }

    @Test
    fun repeatOneRestartsWhateverEndedEvenUnderShuffle() {
        val order = TogetherDecisions.shuffledOrder(5, 2, kotlin.random.Random(7))
        assertEquals(4, TogetherDecisions.nextIndexOnEnd(TogetherDecisions.RepeatMode.ONE, null, 4, 5))
        assertEquals(2, TogetherDecisions.nextIndexOnEnd(TogetherDecisions.RepeatMode.ONE, order, 2, 5))
    }

    @Test
    fun endOfQueueStopsWithoutRepeatAndWrapsWithIt() {
        // Straight order, last track.
        assertEquals(null, TogetherDecisions.nextIndexOnEnd(TogetherDecisions.RepeatMode.OFF, null, 4, 5))
        assertEquals(0, TogetherDecisions.nextIndexOnEnd(TogetherDecisions.RepeatMode.ALL, null, 4, 5))
        // Mid-queue advances either way.
        assertEquals(3, TogetherDecisions.nextIndexOnEnd(TogetherDecisions.RepeatMode.OFF, null, 2, 5))
    }

    @Test
    fun shuffleAdvancesAlongItsOwnOrderAndWrapsShuffled() {
        val order = TogetherDecisions.shuffledOrder(6, 1, kotlin.random.Random(42))
        val at = order.indexOf(1)
        // The next track is the next element of the ORDER, not of the files.
        if (at + 1 < order.size) {
            assertEquals(order[at + 1], TogetherDecisions.nextIndexOnEnd(TogetherDecisions.RepeatMode.OFF, order, 1, 6))
        }
        // At the order's end: stop without repeat…
        assertEquals(null, TogetherDecisions.nextIndexOnEnd(TogetherDecisions.RepeatMode.OFF, order, order.last(), 6))
        // …and wrap to the order's head with it — shuffled stays shuffled.
        assertEquals(order.first(), TogetherDecisions.nextIndexOnEnd(TogetherDecisions.RepeatMode.ALL, order, order.last(), 6))
    }

    @Test
    fun skipBackFollowsTheOrderToo() {
        val order = TogetherDecisions.shuffledOrder(5, 2, kotlin.random.Random(9))
        val at = order.indexOf(2)
        if (at > 0) {
            assertEquals(order[at - 1], TogetherDecisions.previousIndexWith(order, 2, 5))
        } else {
            assertEquals(null, TogetherDecisions.previousIndexWith(order, 2, 5))
        }
        assertEquals(null, TogetherDecisions.previousIndexWith(null, 0, 5))
        assertEquals(1, TogetherDecisions.previousIndexWith(null, 2, 5))
    }

    @Test
    fun manualSkipForwardIgnoresRepeatOneButNotShuffle() {
        val order = TogetherDecisions.shuffledOrder(4, 0, kotlin.random.Random(3))
        // Repeat-one does not trap a manual next…
        assertEquals(1, TogetherDecisions.manualNextIndex(null, 0, 4))
        assertEquals(null, TogetherDecisions.manualNextIndex(null, 3, 4))
        // …but shuffle still chooses which track comes next.
        val at = order.indexOf(0)
        assertEquals(order.getOrNull(at + 1), TogetherDecisions.manualNextIndex(order, 0, 4))
    }

    @Test
    fun speedIsASoloLuxuryAndSnapsToItsSteps() {
        assertTrue(TogetherDecisions.speedAllowed(solo = true))
        assertFalse("in a session the correction ladder owns the rate", TogetherDecisions.speedAllowed(solo = false))
        assertEquals(1.0f, TogetherDecisions.clampSpeed(1.02f))
        assertEquals(1.15f, TogetherDecisions.clampSpeed(1.17f))
        assertEquals(0.5f, TogetherDecisions.clampSpeed(0.1f))
        assertEquals(2.0f, TogetherDecisions.clampSpeed(9f))
    }

    @Test
    fun sleepTimerCountsDownOnlyWhileItLives() {
        assertEquals(null, TogetherDecisions.sleepRemainingMs(null, 600_000, 999))
        assertEquals(401_000L, TogetherDecisions.sleepRemainingMs(1_000, 601_000, 201_000))
        // JUnit4 puts the message first; getting that backwards asserts the
        // message equals the value, which is how this line once passed a null
        // expectation against the string it was named with.
        assertNull("expired is none", TogetherDecisions.sleepRemainingMs(1_000, 600_000, 601_000))
    }

    @Test
    fun lrcParsesTimestampsSortsAndDropsMetadata() {
        val doc = "[ar:A. R. Rahman]\n" +
            "[00:30.50]second line\n" +
            "[01:05][00:10]chorus appears twice\n" +
            "no tags at all\n" +
            "[00:20.250]fraction of three digits\n"
        val lines = TogetherDecisions.parseLrc(doc)
        assertEquals(listOf(10_000L, 20_250L, 30_500L, 65_000L), lines.map { it.atMs })
        assertEquals("chorus appears twice", lines[0].text)
        assertEquals("chorus appears twice", lines[3].text)
        assertEquals("second line", lines[2].text)
    }

    @Test
    fun plainUntimedTextIsNoSyncedLyricsRatherThanAnEmptyDocument() {
        assertTrue(TogetherDecisions.parseLrc("just words\nmore words").isEmpty())
        assertEquals("metadata-only is empty too", 0, TogetherDecisions.parseLrc("[ti:title]").size)
    }

    @Test
    fun theLyricHighlightIsTheLastLineThatStarted() {
        val lines = listOf(
            TogetherDecisions.LyricLine(1_000, "a"),
            TogetherDecisions.LyricLine(5_000, "b"),
            TogetherDecisions.LyricLine(9_000, "c"),
        )
        assertEquals(-1, TogetherDecisions.lyricIndexAt(lines, 500))
        assertEquals(0, TogetherDecisions.lyricIndexAt(lines, 1_000))
        assertEquals(1, TogetherDecisions.lyricIndexAt(lines, 5_500))
        assertEquals(2, TogetherDecisions.lyricIndexAt(lines, 99_000))
    }

}