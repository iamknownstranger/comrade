package mullu.comrade.ui

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Pins [BitmapBudget.sampleSizeFor] — the arithmetic behind every downsampled
 * decode in this package (chat photos, the outgoing preview, avatars). A
 * regression here silently reinflates one of them back towards the ~680 MB
 * Pixel 9 report this class was written to fix.
 */
class BitmapBudgetTest {

    @Test
    fun exactlyAtBudgetNeedsNoDownsampling() {
        assertEquals(1, BitmapBudget.sampleSizeFor(1000, 2000, 2_000_000L))
    }

    @Test
    fun justOverBudgetStepsToTheNextPowerOfTwo() {
        // 1000x2001 is one pixel row over 2,000,000; halving both dimensions
        // is the only move `inSampleSize` has, so it lands well under budget
        // rather than exactly at it.
        assertEquals(2, BitmapBudget.sampleSizeFor(1000, 2001, 2_000_000L))
    }

    @Test
    fun aCameraPhotoStepsDownOnce() {
        // 4032x3024 (12.2 MP) against a 4 MP budget: /2 -> 2016x1512 = 3.05 MP,
        // already under, so one step is enough.
        assertEquals(2, BitmapBudget.sampleSizeFor(4032, 3024, 4_000_000L))
    }

    @Test
    fun aVeryLargeSourceNeedsSeveralSteps() {
        // A 40 MP source against the same 4 MP budget: /2 -> 10 MP, still
        // over; /4 -> 2.55 MP, under. Two steps.
        assertEquals(4, BitmapBudget.sampleSizeFor(7216, 5412, 4_000_000L))
    }

    @Test
    fun aSourceAlreadySmallerThanBudgetIsUntouched() {
        assertEquals(1, BitmapBudget.sampleSizeFor(64, 64, 4_000_000L))
    }

    @Test
    fun aFailedBoundsProbeDoesNotDownsample() {
        // `BitmapFactory` reports -1x-1 for a stream it could not read at all;
        // there is nothing to sample against, so this must not loop or crash.
        assertEquals(1, BitmapBudget.sampleSizeFor(-1, -1, 4_000_000L))
        assertEquals(1, BitmapBudget.sampleSizeFor(0, 0, 4_000_000L))
    }

    @Test
    fun aNonPositiveBudgetIsTreatedAsUnbounded() {
        assertEquals(1, BitmapBudget.sampleSizeFor(4032, 3024, 0L))
        assertEquals(1, BitmapBudget.sampleSizeFor(4032, 3024, -1L))
    }
}
