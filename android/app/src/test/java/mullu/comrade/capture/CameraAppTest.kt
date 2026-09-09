package mullu.comrade.capture

import mullu.comrade.capture.CameraApp.Fallback
import mullu.comrade.capture.CameraApp.Flash
import mullu.comrade.capture.CameraApp.Mode
import mullu.comrade.capture.CameraApp.Route
import mullu.comrade.capture.CameraApp.Timer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The full-camera-app decision vectors, run without an emulator — the JVM
 * lane this file was written so it could see before CI.
 */
class CameraAppTest {

    // ── Handoff: the ride invariant ─────────────────────────────────────────

    @Test
    fun aRideRecordingNeverGoesToTheSystemCameraEvenWhenPreferredAndInstalled() {
        val handoff = CameraApp.handoff(
            mode = Mode.Video,
            keepRecordingWhenLocked = true,
            destinationIsPrivate = false,
            preferSystemCamera = true,
            systemCameraInstalled = true,
        )
        assertEquals(Route.InApp, handoff.route)
        assertEquals(Fallback.RideMustStayInApp, handoff.fallback)
    }

    @Test
    fun rideMustStayInAppWinsOverEveryOtherReason() {
        // Even when the system camera isn't installed either, the ride reason
        // is the one reported, not the coincidental other one.
        val handoff = CameraApp.handoff(
            mode = Mode.Video,
            keepRecordingWhenLocked = true,
            destinationIsPrivate = false,
            preferSystemCamera = false,
            systemCameraInstalled = false,
        )
        assertEquals(Fallback.RideMustStayInApp, handoff.fallback)
    }

    @Test
    fun aPrivateRideRecordingStillReportsRideMustStayInAppNotPrivateStaysInApp() {
        // The ride rule outranks the private-destination rule, and the reason
        // shown to the rider should name the one that actually decided it.
        val handoff = CameraApp.handoff(
            mode = Mode.Video,
            keepRecordingWhenLocked = true,
            destinationIsPrivate = true,
            preferSystemCamera = true,
            systemCameraInstalled = true,
        )
        assertEquals(Route.InApp, handoff.route)
        assertEquals(Fallback.RideMustStayInApp, handoff.fallback)
    }

    @Test
    fun aPrivatePhotoTheUserAskedToHandOffStaysInAppBecauseThereIsNoUriToShare() {
        val handoff = CameraApp.handoff(
            mode = Mode.Photo,
            keepRecordingWhenLocked = false,
            destinationIsPrivate = true,
            preferSystemCamera = true,
            systemCameraInstalled = true,
        )
        assertEquals(Route.InApp, handoff.route)
        assertEquals(Fallback.PrivateStaysInApp, handoff.fallback)
    }

    @Test
    fun aPhotoGoesToTheSystemCameraWhenPreferredAndInstalled() {
        val handoff = CameraApp.handoff(
            mode = Mode.Photo,
            keepRecordingWhenLocked = false,
            destinationIsPrivate = false,
            preferSystemCamera = true,
            systemCameraInstalled = true,
        )
        assertEquals(Route.SystemCamera(CameraApp.ACTION_IMAGE_CAPTURE), handoff.route)
        assertEquals(Fallback.None, handoff.fallback)
    }

    @Test
    fun aPlainVideoGoesToTheSystemCameraWhenPreferredAndInstalled() {
        // keepRecordingWhenLocked is off, so this is an ordinary clip, not a ride.
        val handoff = CameraApp.handoff(
            mode = Mode.Video,
            keepRecordingWhenLocked = false,
            destinationIsPrivate = false,
            preferSystemCamera = true,
            systemCameraInstalled = true,
        )
        assertEquals(Route.SystemCamera(CameraApp.ACTION_VIDEO_CAPTURE), handoff.route)
        assertEquals(Fallback.None, handoff.fallback)
    }

    @Test
    fun noSystemCameraInstalledFallsBackToInAppRegardlessOfPreference() {
        val handoff = CameraApp.handoff(
            mode = Mode.Photo,
            keepRecordingWhenLocked = false,
            destinationIsPrivate = false,
            preferSystemCamera = true,
            systemCameraInstalled = false,
        )
        assertEquals(Route.InApp, handoff.route)
        assertEquals(Fallback.NoSystemCameraInstalled, handoff.fallback)
    }

    @Test
    fun userChoosingInAppIsRespectedWhenTheSystemCameraIsAvailable() {
        val handoff = CameraApp.handoff(
            mode = Mode.Photo,
            keepRecordingWhenLocked = false,
            destinationIsPrivate = false,
            preferSystemCamera = false,
            systemCameraInstalled = true,
        )
        assertEquals(Route.InApp, handoff.route)
        assertEquals(Fallback.UserChoseInApp, handoff.fallback)
    }

    // ── Flash ────────────────────────────────────────────────────────────

    @Test
    fun photoFlashCyclesThroughOffAutoOn() {
        assertEquals(Flash.Auto, CameraApp.nextFlash(Flash.Off, Mode.Photo))
        assertEquals(Flash.On, CameraApp.nextFlash(Flash.Auto, Mode.Photo))
        assertEquals(Flash.Off, CameraApp.nextFlash(Flash.On, Mode.Photo))
    }

    @Test
    fun videoFlashOnlyHasOffAndOn() {
        assertEquals(Flash.On, CameraApp.nextFlash(Flash.Off, Mode.Video))
        assertEquals(Flash.Off, CameraApp.nextFlash(Flash.On, Mode.Video))
    }

    @Test
    fun videoFlashNormalisesAutoToOnRatherThanCyclingThroughIt() {
        assertEquals(Flash.On, CameraApp.nextFlash(Flash.Auto, Mode.Video))
    }

    // ── Timer ────────────────────────────────────────────────────────────

    @Test
    fun timerCyclesThroughOffThreeTen() {
        assertEquals(Timer.Three, CameraApp.nextTimer(Timer.Off))
        assertEquals(Timer.Ten, CameraApp.nextTimer(Timer.Three))
        assertEquals(Timer.Off, CameraApp.nextTimer(Timer.Ten))
    }

    @Test
    fun timerSecondsMatchEachSetting() {
        assertEquals(0, CameraApp.timerSeconds(Timer.Off))
        assertEquals(3, CameraApp.timerSeconds(Timer.Three))
        assertEquals(10, CameraApp.timerSeconds(Timer.Ten))
    }

    // ── Zoom stops ───────────────────────────────────────────────────────

    @Test
    fun zoomStopsOnAPixelIshRangeIncludeTheCanonicalStopsThatFit() {
        val stops = CameraApp.zoomStops(0.5f, 30f)
        assertEquals(listOf(0.5f, 1f, 2f, 5f, 10f, 20f, 30f), stops)
    }

    @Test
    fun zoomStopsOnANarrowerRangeOmitStopsOutsideIt() {
        val stops = CameraApp.zoomStops(1f, 8f)
        assertEquals(listOf(1f, 2f, 5f), stops)
    }

    @Test
    fun zoomStopsAlwaysIncludeTheMinimumEvenWhenItIsNotACanonicalStop() {
        val stops = CameraApp.zoomStops(0.6f, 4f)
        assertEquals(listOf(0.6f, 1f, 2f), stops)
    }

    @Test
    fun aDegenerateZoomRangeYieldsExactlyOneStop() {
        assertEquals(listOf(1f), CameraApp.zoomStops(2f, 1f))
        assertEquals(listOf(1f), CameraApp.zoomStops(1f, 1f))
        assertEquals(listOf(1f), CameraApp.zoomStops(Float.NaN, 5f))
        assertEquals(listOf(1f), CameraApp.zoomStops(1f, Float.POSITIVE_INFINITY))
    }

    @Test
    fun zoomClampsIntoRange() {
        assertEquals(1f, CameraApp.clampZoom(0.2f, 1f, 10f))
        assertEquals(10f, CameraApp.clampZoom(50f, 1f, 10f))
        assertEquals(4f, CameraApp.clampZoom(4f, 1f, 10f))
    }

    @Test
    fun zoomCyclesToTheNextStopAndWrapsAround() {
        val stops = listOf(0.5f, 1f, 2f, 5f, 10f)
        assertEquals(1f, CameraApp.nextZoomStop(0.5f, stops))
        assertEquals(2f, CameraApp.nextZoomStop(1f, stops))
        assertEquals(0.5f, CameraApp.nextZoomStop(10f, stops))
    }

    @Test
    fun zoomLabelsShowAtMostOneDecimalAndNeverATrailingZero() {
        assertEquals("0.6×", CameraApp.zoomLabel(0.6f))
        assertEquals("1×", CameraApp.zoomLabel(1.0f))
        assertEquals("2.5×", CameraApp.zoomLabel(2.5f))
        assertEquals("10×", CameraApp.zoomLabel(10.0f))
    }

    // ── Tap-to-focus metering rect ───────────────────────────────────────

    private val activeWidth = 4000
    private val activeHeight = 3000

    @Test
    fun aCenterTapMetersTheCenterOfTheSensorAtAnyOrientation() {
        for (orientation in listOf(0, 90, 180, 270)) {
            val rect = CameraApp.meteringRect(
                nx = 0.5f,
                ny = 0.5f,
                activeWidth = activeWidth,
                activeHeight = activeHeight,
                sensorOrientation = orientation,
                mirrored = false,
            )
            val cx = (rect.left + rect.right) / 2
            val cy = (rect.top + rect.bottom) / 2
            assertNear("orientation $orientation", activeWidth / 2, cx, 1)
            assertNear("orientation $orientation", activeHeight / 2, cy, 1)
        }
    }

    @Test
    fun anOffCenterTapRotatesWithSensorOrientationZero() {
        // Left-middle of the viewfinder, unrotated sensor: left-middle of the sensor.
        val rect = CameraApp.meteringRect(
            nx = 0.25f,
            ny = 0.5f,
            activeWidth = activeWidth,
            activeHeight = activeHeight,
            sensorOrientation = 0,
            mirrored = false,
        )
        val cx = (rect.left + rect.right) / 2
        val cy = (rect.top + rect.bottom) / 2
        assertNear(1000, cx, 1)
        assertNear(1500, cy, 1)
    }

    @Test
    fun anOffCenterTapRotatesWithSensorOrientationNinety() {
        val rect = CameraApp.meteringRect(
            nx = 0.25f,
            ny = 0.5f,
            activeWidth = activeWidth,
            activeHeight = activeHeight,
            sensorOrientation = 90,
            mirrored = false,
        )
        val cx = (rect.left + rect.right) / 2
        val cy = (rect.top + rect.bottom) / 2
        // rotate(0.25, 0.5, 90) = (0.5, 0.75) -> bottom-middle of the sensor.
        assertNear(2000, cx, 1)
        assertNear(2250, cy, 1)
    }

    @Test
    fun anOffCenterTapRotatesWithSensorOrientationOneEighty() {
        val rect = CameraApp.meteringRect(
            nx = 0.25f,
            ny = 0.5f,
            activeWidth = activeWidth,
            activeHeight = activeHeight,
            sensorOrientation = 180,
            mirrored = false,
        )
        val cx = (rect.left + rect.right) / 2
        val cy = (rect.top + rect.bottom) / 2
        // rotate(0.25, 0.5, 180) = (0.75, 0.5) -> right-middle of the sensor.
        assertNear(3000, cx, 1)
        assertNear(1500, cy, 1)
    }

    @Test
    fun anOffCenterTapRotatesWithSensorOrientationTwoSeventy() {
        val rect = CameraApp.meteringRect(
            nx = 0.25f,
            ny = 0.5f,
            activeWidth = activeWidth,
            activeHeight = activeHeight,
            sensorOrientation = 270,
            mirrored = false,
        )
        val cx = (rect.left + rect.right) / 2
        val cy = (rect.top + rect.bottom) / 2
        // rotate(0.25, 0.5, 270) = (0.5, 0.25) -> top-middle of the sensor.
        assertNear(2000, cx, 1)
        assertNear(750, cy, 1)
    }

    @Test
    fun aFrontLensMirrorsHorizontallySoTheTapDoesNotLandOnTheOppositeSide() {
        // Same tap as the sensorOrientation=0 case above, but mirrored: the
        // classic tap-to-focus bug is focusing the opposite side of the frame,
        // so the mirrored result must land on the opposite side of the
        // unmirrored one.
        val unmirrored = CameraApp.meteringRect(
            nx = 0.25f,
            ny = 0.5f,
            activeWidth = activeWidth,
            activeHeight = activeHeight,
            sensorOrientation = 0,
            mirrored = false,
        )
        val mirrored = CameraApp.meteringRect(
            nx = 0.25f,
            ny = 0.5f,
            activeWidth = activeWidth,
            activeHeight = activeHeight,
            sensorOrientation = 0,
            mirrored = true,
        )
        val unmirroredCx = (unmirrored.left + unmirrored.right) / 2
        val mirroredCx = (mirrored.left + mirrored.right) / 2
        assertNear(1000, unmirroredCx, 1)
        assertNear(3000, mirroredCx, 1)
    }

    @Test
    fun aCornerTapKeepsTheFullSizedBoxByShiftingItBackInsideTheArray() {
        val rect = CameraApp.meteringRect(
            nx = 0f,
            ny = 0f,
            activeWidth = activeWidth,
            activeHeight = activeHeight,
            sensorOrientation = 0,
            mirrored = false,
            sizeFraction = 0.12f,
        )
        val expectedSide = (0.12f * activeHeight).toInt()
        assertEquals(0, rect.left)
        assertEquals(0, rect.top)
        assertNear(expectedSide, rect.right, 1)
        assertNear(expectedSide, rect.bottom, 1)
        assertTrue(rect.left in 0..activeWidth)
        assertTrue(rect.right in 0..activeWidth)
        assertTrue(rect.top in 0..activeHeight)
        assertTrue(rect.bottom in 0..activeHeight)
    }

    @Test
    fun anOutOfRangeTapClampsToTheNearestCornerRatherThanThrowing() {
        val clamped = CameraApp.meteringRect(
            nx = -0.5f,
            ny = 1.5f,
            activeWidth = activeWidth,
            activeHeight = activeHeight,
            sensorOrientation = 0,
            mirrored = false,
        )
        val corner = CameraApp.meteringRect(
            nx = 0f,
            ny = 1f,
            activeWidth = activeWidth,
            activeHeight = activeHeight,
            sensorOrientation = 0,
            mirrored = false,
        )
        assertEquals(corner, clamped)
    }

    private fun assertNear(message: String, expected: Int, actual: Int, tolerance: Int) {
        assertTrue("$message: expected $expected within $tolerance of $actual", Math.abs(expected - actual) <= tolerance)
    }

    private fun assertNear(expected: Int, actual: Int, tolerance: Int) {
        assertTrue("expected $expected within $tolerance of $actual", Math.abs(expected - actual) <= tolerance)
    }
}
