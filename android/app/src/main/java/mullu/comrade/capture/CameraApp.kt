package mullu.comrade.capture

import kotlin.math.roundToInt

/**
 * The Pixel-Camera-shaped half of Capture — photo/video modes, zoom stops,
 * flash, tap-to-focus, self-timer — pure, with no Android imports, so the JVM
 * lane can pin it the same way [CaptureDecisions] pins the helmet cam.
 *
 * **Why this is a separate file from [CaptureDecisions] rather than an
 * extension of it:** [CaptureDecisions] is the helmet-cam invariant — a
 * `MediaRecorder` that must outlive the screen lock — and [handoff] is the
 * seam where that invariant meets the new feature. Everything else here
 * (flash, zoom, timer, tap-to-focus) is ordinary still/video-camera UI state
 * that has nothing to do with the lock button, and keeping it in its own file
 * keeps [CaptureDecisions] readable as exactly the one decision it was written
 * to pin.
 *
 * **Why a one-off shot can go to the system camera app at all:** most riders
 * never touch the lock-and-ride flow — they want a quick photo or clip the
 * same way they always have, and Google Camera's HDR, portrait mode and shutter
 * lag are years ahead of anything this app will ever reimplement. [handoff] is
 * the only place that decision is made, and it exists so the ride invariant
 * cannot be bypassed by accident: a caller cannot construct
 * `Route.SystemCamera` for a locked ride without going through this function
 * and getting `Route.InApp` back instead.
 */
object CameraApp {

    enum class Mode { Photo, Video }

    /** Photo has a real Auto; a video torch is only on or off. */
    enum class Flash { Off, Auto, On }

    enum class Timer { Off, Three, Ten }

    /** Where a shutter press is actually serviced. */
    sealed interface Route {
        data object InApp : Route
        data class SystemCamera(val action: String) : Route
    }

    /** Why the system camera was not used, so the UI can say so instead of
     *  silently doing something else than the switch implies. */
    enum class Fallback { None, RideMustStayInApp, PrivateStaysInApp, NoSystemCameraInstalled, UserChoseInApp }

    data class Handoff(val route: Route, val fallback: Fallback)

    const val ACTION_IMAGE_CAPTURE: String = "android.media.action.IMAGE_CAPTURE"
    const val ACTION_VIDEO_CAPTURE: String = "android.media.action.VIDEO_CAPTURE"

    /**
     * `keepRecordingWhenLocked` is the helmet-cam switch. When it is on and the
     * mode is Video, the answer is always InApp with Fallback.RideMustStayInApp
     * — regardless of `destinationIsPrivate`, `preferSystemCamera` or
     * `systemCameraInstalled`, because no stock camera app keeps a
     * `MediaRecorder` alive through the lock button.
     *
     * `destinationIsPrivate` comes next, and it is not a preference the way
     * `preferSystemCamera` is: `Destination.Private` (see `CaptureSink`'s class
     * doc and `docs/CAPTURE.md` §5) is a file in this app's own external
     * storage that is never inserted into `MediaStore`, so there is no
     * shareable `Uri` to hand another app as `EXTRA_OUTPUT`. A handoff for a
     * private shot is not a thing we chose not to build, it is not a thing
     * that can be built, and `Fallback.PrivateStaysInApp` says that rather
     * than reporting `UserChoseInApp` for a switch the user never touched.
     *
     * Precedence after that: no system camera installed, then the user's own
     * preference, then SystemCamera with the action for the mode.
     */
    fun handoff(
        mode: Mode,
        keepRecordingWhenLocked: Boolean,
        destinationIsPrivate: Boolean,
        preferSystemCamera: Boolean,
        systemCameraInstalled: Boolean,
    ): Handoff = when {
        mode == Mode.Video && keepRecordingWhenLocked ->
            Handoff(Route.InApp, Fallback.RideMustStayInApp)
        destinationIsPrivate ->
            Handoff(Route.InApp, Fallback.PrivateStaysInApp)
        !systemCameraInstalled ->
            Handoff(Route.InApp, Fallback.NoSystemCameraInstalled)
        !preferSystemCamera ->
            Handoff(Route.InApp, Fallback.UserChoseInApp)
        else -> {
            val action = when (mode) {
                Mode.Photo -> ACTION_IMAGE_CAPTURE
                Mode.Video -> ACTION_VIDEO_CAPTURE
            }
            Handoff(Route.SystemCamera(action), Fallback.None)
        }
    }

    /** Photo: Off→Auto→On→Off. Video: Off→On→Off — Auto is not a real video
     *  state (a video light is only on or off), so it normalises straight to
     *  On rather than being cycled through. */
    fun nextFlash(current: Flash, mode: Mode): Flash = when (mode) {
        Mode.Photo -> when (current) {
            Flash.Off -> Flash.Auto
            Flash.Auto -> Flash.On
            Flash.On -> Flash.Off
        }
        Mode.Video -> when (current) {
            Flash.Off -> Flash.On
            Flash.On -> Flash.Off
            Flash.Auto -> Flash.On
        }
    }

    fun nextTimer(current: Timer): Timer = when (current) {
        Timer.Off -> Timer.Three
        Timer.Three -> Timer.Ten
        Timer.Ten -> Timer.Off
    }

    fun timerSeconds(timer: Timer): Int = when (timer) {
        Timer.Off -> 0
        Timer.Three -> 3
        Timer.Ten -> 10
    }

    /**
     * The zoom stops a Pixel's own camera app offers, ordered from the
     * ultrawide crop up through the far end of digital zoom. Not every device
     * has all of these — [zoomStops] is what filters this list down to what a
     * given lens combination actually covers.
     */
    private val CANONICAL_STOPS = listOf(0.5f, 1f, 2f, 5f, 10f, 20f, 30f)

    /**
     * The zoom chips a Pixel shows: every stop in [CANONICAL_STOPS] the
     * device's own [minRatio]..[maxRatio] actually covers, always including
     * [minRatio] and `1.0` when in range, de-duplicated and ascending. A
     * device that reports a degenerate range (max <= min, or a non-finite
     * bound) yields exactly `listOf(1f)` rather than an empty row of chips.
     */
    fun zoomStops(minRatio: Float, maxRatio: Float): List<Float> {
        if (!minRatio.isFinite() || !maxRatio.isFinite() || maxRatio <= minRatio) {
            return listOf(1f)
        }
        val stops = sortedSetOf<Float>()
        stops += minRatio
        if (1f in minRatio..maxRatio) stops += 1f
        for (stop in CANONICAL_STOPS) {
            if (stop in minRatio..maxRatio) stops += stop
        }
        return stops.toList()
    }

    fun clampZoom(requested: Float, minRatio: Float, maxRatio: Float): Float {
        if (!minRatio.isFinite() || !maxRatio.isFinite() || maxRatio < minRatio) return minRatio
        return requested.coerceIn(minRatio, maxRatio)
    }

    /** Cycle to the next stop above the current ratio, wrapping to the first. */
    fun nextZoomStop(current: Float, stops: List<Float>): Float {
        if (stops.isEmpty()) return current
        return stops.firstOrNull { it > current } ?: stops.first()
    }

    /**
     * "0.6×", "1×", "2.5×", "10×" — a trailing ".0" is never shown, and one
     * decimal is the most that ever is. Rounded with integer arithmetic on
     * tenths rather than `String.format`, whose decimal separator is locale
     * dependent and therefore not deterministic under test.
     */
    fun zoomLabel(ratio: Float): String {
        val tenths = Math.round(ratio * 10f)
        val whole = tenths / 10
        val frac = tenths % 10
        val text = if (frac == 0) whole.toString() else "$whole.$frac"
        return "${text}×"
    }

    /** A square metering box, in sensor-array coordinates, for a tap at the
     *  normalised viewfinder point (nx, ny) — (0,0) top-left, (1,1) bottom-right.
     *  Rotates by [sensorOrientation] (0/90/180/270) and mirrors horizontally
     *  for a front lens, because the viewfinder is mirrored and an unmirrored
     *  metering rect focuses on the opposite side of the frame — the classic
     *  tap-to-focus bug. The box is [sizeFraction] of the shorter sensor edge,
     *  clamped so it never leaves the active array. Out-of-range taps clamp
     *  rather than throw.
     */
    data class MeteringRect(val left: Int, val top: Int, val right: Int, val bottom: Int)

    fun meteringRect(
        nx: Float,
        ny: Float,
        activeWidth: Int,
        activeHeight: Int,
        sensorOrientation: Int,
        mirrored: Boolean,
        sizeFraction: Float = 0.12f,
    ): MeteringRect {
        val cx = nx.coerceIn(0f, 1f)
        val cy = ny.coerceIn(0f, 1f)
        val mx = if (mirrored) 1f - cx else cx
        val (u, v) = rotateNormalized(mx, cy, sensorOrientation)

        val sensorX = u * activeWidth
        val sensorY = v * activeHeight
        val side = (sizeFraction * minOf(activeWidth, activeHeight)).coerceAtLeast(1f)
        val half = side / 2f

        var left = sensorX - half
        var top = sensorY - half
        var right = sensorX + half
        var bottom = sensorY + half

        // Shift the box back in bounds without shrinking it, so a corner tap
        // still meters over the same box size the centre of the frame would
        // get — only its position moves.
        if (right > activeWidth) {
            val shift = right - activeWidth
            left -= shift
            right -= shift
        }
        if (left < 0f) {
            val shift = -left
            left += shift
            right += shift
        }
        if (bottom > activeHeight) {
            val shift = bottom - activeHeight
            top -= shift
            bottom -= shift
        }
        if (top < 0f) {
            val shift = -top
            top += shift
            bottom += shift
        }

        // Only reached when the active array itself is smaller than the box.
        left = left.coerceIn(0f, activeWidth.toFloat())
        right = right.coerceIn(0f, activeWidth.toFloat())
        top = top.coerceIn(0f, activeHeight.toFloat())
        bottom = bottom.coerceIn(0f, activeHeight.toFloat())

        return MeteringRect(left.roundToInt(), top.roundToInt(), right.roundToInt(), bottom.roundToInt())
    }

    /**
     * Maps a normalised point from viewfinder space into sensor-array space
     * for a sensor mounted [sensorOrientation] degrees from the viewfinder's
     * own orientation. Values other than 0/90/180/270 fall back to the
     * identity mapping rather than throwing.
     */
    private fun rotateNormalized(x: Float, y: Float, sensorOrientation: Int): Pair<Float, Float> {
        val normalized = ((sensorOrientation % 360) + 360) % 360
        return when (normalized) {
            90 -> y to (1f - x)
            180 -> (1f - x) to (1f - y)
            270 -> (1f - y) to x
            else -> x to y
        }
    }
}
