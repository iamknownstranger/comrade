package mullu.comrade.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pins the chat-thread UX rules: when a day separator is due, when fresh
 * messages may auto-scroll the reader, and how bubble times render.
 */
class ChatThreadTest {

    private val utc = java.time.ZoneId.of("UTC")
    private val noon = 1_752_321_600L // 2025-07-12T12:00:00Z

    // ── Day separators ──────────────────────────────────────────────────────

    @Test
    fun firstMessageAlwaysOpensADay() {
        assertTrue(startsNewDay(null, noon, utc))
    }

    @Test
    fun sameDayDoesNotRepeatTheHeader() {
        assertFalse(startsNewDay(noon, noon + 3600, utc))
    }

    @Test
    fun crossingMidnightOpensANewDay() {
        // 23:30 → 00:30 the next day: only a one-hour gap, but a new date.
        val lateEvening = noon + 11 * 3600 + 1800
        assertTrue(startsNewDay(lateEvening, lateEvening + 3600, utc))
    }

    @Test
    fun dayBoundaryFollowsTheZoneNotUtc() {
        // 23:30 UTC and 00:30 UTC straddle midnight in UTC, but both are
        // afternoon of the same day in UTC+10.
        val lateEvening = noon + 11 * 3600 + 1800
        assertFalse(startsNewDay(lateEvening, lateEvening + 3600, java.time.ZoneId.of("+10:00")))
    }

    // ── Auto-scroll on new messages ─────────────────────────────────────────

    @Test
    fun readerAtTheNewestMessageAutoScrolls() {
        assertTrue(isNearBottom(lastVisibleIndex = 9, totalCount = 10))
    }

    @Test
    fun readerWithinSlackOfTheBottomAutoScrolls() {
        assertTrue(isNearBottom(lastVisibleIndex = 7, totalCount = 10, slack = 2))
    }

    @Test
    fun readerScrolledUpInHistoryIsNotYanked() {
        assertFalse(isNearBottom(lastVisibleIndex = 6, totalCount = 10, slack = 2))
        assertFalse(isNearBottom(lastVisibleIndex = 0, totalCount = 100))
    }

    @Test
    fun emptyOrUnmeasuredListCountsAsBottom() {
        assertTrue(isNearBottom(lastVisibleIndex = -1, totalCount = 0))
    }

    // ── Bubble timestamps ───────────────────────────────────────────────────

    @Test
    fun clockTimeRendersWallClockInTheGivenZone() {
        assertEquals("12:00", clockTime(noon, utc))
        assertEquals("12:05", clockTime(noon + 300, utc))
        assertEquals("22:00", clockTime(noon, java.time.ZoneId.of("+10:00")))
        // 24-hour clock, zero-padded: 00:xx just after midnight.
        assertEquals("00:30", clockTime(noon + 12 * 3600 + 1800, utc))
    }
}
