package mullu.comrade.ui

/**
 * The downsample arithmetic every `BitmapFactory` caller in this package
 * needs, lifted out of `AttachmentPreview.decodePreview` (the first place it
 * was written) so `MediaCache.decodeImage` and `ProfileScreen.decodeAvatar`
 * share it rather than each growing a slightly different copy.
 *
 * Pure Kotlin stdlib, no `android.graphics` import — so this is the checkable
 * half CLAUDE.md describes: [BitmapBudgetTest] compiles and runs on the plain
 * `kotlinc` lane, before CI ever sees this file.
 */
object BitmapBudget {

    /**
     * The smallest power-of-two `inSampleSize` that decodes a [sourceWidth] ×
     * [sourceHeight] image to at most [maxPixels].
     *
     * `BitmapFactory.Options.inSampleSize` only honours powers of two — an
     * odd factor is rounded *down* to the nearest one below it before it does
     * anything, so computing anything else here would silently decode larger
     * than the budget rather than erroring.
     *
     * A non-positive dimension (a bounds probe that failed to read the
     * source) or a non-positive budget both return 1 — the same "no
     * downsampling" answer `BitmapFactory` itself gives an invalid probe,
     * rather than looping forever or dividing by nothing.
     */
    fun sampleSizeFor(sourceWidth: Int, sourceHeight: Int, maxPixels: Long): Int {
        if (sourceWidth <= 0 || sourceHeight <= 0 || maxPixels <= 0) return 1
        var sample = 1
        while ((sourceWidth / sample).toLong() * (sourceHeight / sample) > maxPixels) {
            sample *= 2
        }
        return sample
    }
}
