package mullu.comrade.ui

/**
 * Pure chat-thread rules — day grouping and auto-scroll — kept free of
 * Compose/Android imports so plain JVM unit tests (`ChatThreadTest`) can
 * pin them.
 */

/**
 * Whether a message at [epochSecs] opens a new calendar day relative to the
 * one before it at [prevEpochSecs] — i.e. whether the thread should render a
 * day separator above it. The first message of a thread (null prev) always
 * does.
 */
fun startsNewDay(
    prevEpochSecs: Long?,
    epochSecs: Long,
    zone: java.time.ZoneId = java.time.ZoneId.systemDefault(),
): Boolean {
    if (prevEpochSecs == null) return true
    fun day(secs: Long) = java.time.Instant.ofEpochSecond(secs).atZone(zone).toLocalDate()
    return day(epochSecs) != day(prevEpochSecs)
}

/**
 * Whether the reader is close enough to the newest message that fresh
 * arrivals should auto-scroll into view. Someone scrolled up reading
 * history (further than [slack] items from the end) must NOT be yanked
 * down — they get a "new messages" affordance instead. An empty or
 * not-yet-laid-out list counts as at the bottom.
 */
fun isNearBottom(lastVisibleIndex: Int, totalCount: Int, slack: Int = 2): Boolean =
    totalCount <= 0 || lastVisibleIndex >= totalCount - 1 - slack
