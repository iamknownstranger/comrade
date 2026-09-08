package mullu.comrade.capture

import mullu.comrade.capture.CaptureDecisions.Facing
import mullu.comrade.capture.CaptureDecisions.Lens
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The helmet-cam decision vectors — every case named in the spec, run without
 * an emulator: this is the half of the feature the JVM lane can see before CI.
 */
class CaptureDecisionsTest {

    private fun lens(
        id: String,
        facing: Facing,
        sensorOrientation: Int = 90,
        focalLengthMm: Float? = null,
    ) = Lens(cameraId = id, facing = facing, sensorOrientation = sensorOrientation, focalLengthMm = focalLengthMm)

    // ── Lens ordering ────────────────────────────────────────────────────

    @Test
    fun backCamerasComeBeforeFrontAndExternal() {
        val ordered = CaptureDecisions.orderLenses(
            listOf(
                lens("2", Facing.External),
                lens("1", Facing.Front),
                lens("0", Facing.Back),
            ),
        )
        assertEquals(listOf(Facing.Back, Facing.Front, Facing.External), ordered.map { it.facing })
    }

    @Test
    fun withinAFacingTheUltrawideComesBeforeTheMainAndTheTeleLast() {
        val ordered = CaptureDecisions.orderLenses(
            listOf(
                lens("main", Facing.Back, focalLengthMm = 4.25f),
                lens("tele", Facing.Back, focalLengthMm = 6.9f),
                lens("ultrawide", Facing.Back, focalLengthMm = 1.7f),
            ),
        )
        assertEquals(listOf("ultrawide", "main", "tele"), ordered.map { it.cameraId })
    }

    @Test
    fun aLensWithNoReportedFocalLengthSortsAfterOnesThatReportOne() {
        val ordered = CaptureDecisions.orderLenses(
            listOf(
                lens("unknown", Facing.Back, focalLengthMm = null),
                lens("known", Facing.Back, focalLengthMm = 4.25f),
            ),
        )
        assertEquals(listOf("known", "unknown"), ordered.map { it.cameraId })
    }

    @Test
    fun tiesFallBackToCameraIdNumericallyWhenBothParseAsNumbers() {
        val ordered = CaptureDecisions.orderLenses(
            listOf(
                lens("10", Facing.Back, focalLengthMm = 4f),
                lens("2", Facing.Back, focalLengthMm = 4f),
            ),
        )
        // Numeric order, not the lexicographic order that would put "10" first.
        assertEquals(listOf("2", "10"), ordered.map { it.cameraId })
    }

    @Test
    fun tiesFallBackToCameraIdLexicographicallyWhenEitherIsNotANumber() {
        val ordered = CaptureDecisions.orderLenses(
            listOf(
                lens("b-lens", Facing.Back, focalLengthMm = 4f),
                lens("a-lens", Facing.Back, focalLengthMm = 4f),
            ),
        )
        assertEquals(listOf("a-lens", "b-lens"), ordered.map { it.cameraId })
    }

    @Test
    fun orderingAnEmptyListNeverThrows() {
        assertTrue(CaptureDecisions.orderLenses(emptyList()).isEmpty())
    }

    @Test
    fun orderingIsDeterministicAcrossCalls() {
        val lenses = listOf(
            lens("front", Facing.Front, focalLengthMm = 2.2f),
            lens("back-tele", Facing.Back, focalLengthMm = 6.9f),
            lens("back-main", Facing.Back, focalLengthMm = 4.25f),
        )
        val a = CaptureDecisions.orderLenses(lenses).map { it.cameraId }
        val b = CaptureDecisions.orderLenses(lenses.shuffled(java.util.Random(1))).map { it.cameraId }
        assertEquals(a, b)
    }

    // ── Default and next lens ───────────────────────────────────────────────

    @Test
    fun theDefaultLensIsTheFirstBackCamera() {
        val ordered = CaptureDecisions.orderLenses(
            listOf(lens("front", Facing.Front), lens("back", Facing.Back)),
        )
        assertEquals("back", CaptureDecisions.defaultLens(ordered)?.cameraId)
    }

    @Test
    fun withNoBackCameraTheDefaultIsWhicheverOrderingPutFirst() {
        val ordered = CaptureDecisions.orderLenses(listOf(lens("front", Facing.Front)))
        assertEquals("front", CaptureDecisions.defaultLens(ordered)?.cameraId)
    }

    @Test
    fun theDefaultLensOfAnEmptyListIsNull() {
        assertNull(CaptureDecisions.defaultLens(emptyList()))
    }

    @Test
    fun nextLensCyclesAndWraps() {
        val ordered = CaptureDecisions.orderLenses(
            listOf(lens("back", Facing.Back), lens("front", Facing.Front), lens("ext", Facing.External)),
        )
        assertEquals("front", CaptureDecisions.nextLens(ordered, "back")?.cameraId)
        assertEquals("ext", CaptureDecisions.nextLens(ordered, "front")?.cameraId)
        assertEquals("back", CaptureDecisions.nextLens(ordered, "ext")?.cameraId)
    }

    @Test
    fun aVanishedCurrentLensReturnsTheFirstEntryRatherThanStandingStill() {
        val ordered = CaptureDecisions.orderLenses(listOf(lens("back", Facing.Back), lens("front", Facing.Front)))
        // An unplugged USB camera, or a stale selection from before a relaunch.
        assertEquals("back", CaptureDecisions.nextLens(ordered, "usb-that-left")?.cameraId)
        assertEquals("back", CaptureDecisions.nextLens(ordered, null)?.cameraId)
    }

    @Test
    fun nextLensOfAnEmptyListIsNull() {
        assertNull(CaptureDecisions.nextLens(emptyList(), "anything"))
    }

    // ── Badges ───────────────────────────────────────────────────────────

    @Test
    fun theBadgeOrdinalCountsWithinTheSameFacingOnly() {
        val ordered = CaptureDecisions.orderLenses(
            listOf(
                lens("wide", Facing.Back, focalLengthMm = 1.7f),
                lens("main", Facing.Back, focalLengthMm = 4.25f),
                lens("tele", Facing.Back, focalLengthMm = 6.9f),
                lens("selfie", Facing.Front),
            ),
        )
        assertEquals(CaptureDecisions.LensBadge(Facing.Back, 0), CaptureDecisions.badge(ordered, "wide"))
        assertEquals(CaptureDecisions.LensBadge(Facing.Back, 1), CaptureDecisions.badge(ordered, "main"))
        assertEquals(CaptureDecisions.LensBadge(Facing.Back, 2), CaptureDecisions.badge(ordered, "tele"))
        assertEquals(CaptureDecisions.LensBadge(Facing.Front, 0), CaptureDecisions.badge(ordered, "selfie"))
    }

    @Test
    fun theBadgeOfAnUnknownIdIsNull() {
        val ordered = CaptureDecisions.orderLenses(listOf(lens("back", Facing.Back)))
        assertNull(CaptureDecisions.badge(ordered, "nope"))
    }

    // ── File naming ──────────────────────────────────────────────────────

    // 2024-01-15T10:30:45Z, verified against the epoch millis independently.
    private val jan15_1030_45_utc = 1_705_314_645_000L

    @Test
    fun theBaseNameMatchesTheStockCameraAppsConventionInUtc() {
        assertEquals("VID_20240115_103045", CaptureDecisions.baseName(jan15_1030_45_utc, utcOffsetMinutes = 0))
    }

    @Test
    fun aPositiveOffsetShiftsTheClockForwardWithoutChangingTheDate() {
        // IST, UTC+5:30 — same calendar day, later local clock.
        assertEquals(
            "VID_20240115_160045",
            CaptureDecisions.baseName(jan15_1030_45_utc, utcOffsetMinutes = 5 * 60 + 30),
        )
    }

    @Test
    fun aNegativeOffsetCanRollTheDateBackward() {
        // 2024-01-15T00:30:00Z at UTC-1 is 2024-01-14T23:30:00 local.
        val midnightHalfHourUtc = 1_705_278_600_000L
        assertEquals(
            "VID_20240114_233000",
            CaptureDecisions.baseName(midnightHalfHourUtc, utcOffsetMinutes = -60),
        )
    }

    @Test
    fun aPreEpochStartTimeIsHandledRatherThanProducingGarbage() {
        // One hour before the epoch: 1969-12-31T23:00:00Z.
        assertEquals(
            "VID_19691231_230000",
            CaptureDecisions.baseName(-3_600_000L, utcOffsetMinutes = 0),
        )
    }

    @Test
    fun aNegativeOffsetOnAnEpochStartTimeCrossesIntoNineteenSixtyNine() {
        // Epoch instant itself, shifted an hour into the previous local day.
        assertEquals(
            "VID_19691231_230000",
            CaptureDecisions.baseName(0L, utcOffsetMinutes = -60),
        )
    }

    @Test
    fun segmentZeroHasThePlainName() {
        assertEquals("VID_20240115_103045.mp4", CaptureDecisions.segmentFileName("VID_20240115_103045", 0))
    }

    @Test
    fun laterSegmentsGetAZeroPaddedSuffixSoARollingRideStillSortsInOrder() {
        assertEquals("VID_20240115_103045_001.mp4", CaptureDecisions.segmentFileName("VID_20240115_103045", 1))
        assertEquals("VID_20240115_103045_007.mp4", CaptureDecisions.segmentFileName("VID_20240115_103045", 7))
        assertEquals("VID_20240115_103045_123.mp4", CaptureDecisions.segmentFileName("VID_20240115_103045", 123))
    }

    // ── Orientation ──────────────────────────────────────────────────────

    @Test
    fun aBackCameraSubtractsTheDeviceRotation() {
        val sensor = 90
        assertEquals(90, CaptureDecisions.orientationHint(sensor, 0, Facing.Back))
        assertEquals(0, CaptureDecisions.orientationHint(sensor, 90, Facing.Back))
        assertEquals(270, CaptureDecisions.orientationHint(sensor, 180, Facing.Back))
        assertEquals(180, CaptureDecisions.orientationHint(sensor, 270, Facing.Back))
    }

    @Test
    fun anExternalCameraIsTreatedLikeABackCamera() {
        val sensor = 90
        assertEquals(90, CaptureDecisions.orientationHint(sensor, 0, Facing.External))
        assertEquals(0, CaptureDecisions.orientationHint(sensor, 90, Facing.External))
    }

    @Test
    fun aFrontCameraAddsTheDeviceRotation() {
        val sensor = 270
        assertEquals(270, CaptureDecisions.orientationHint(sensor, 0, Facing.Front))
        assertEquals(0, CaptureDecisions.orientationHint(sensor, 90, Facing.Front))
        assertEquals(90, CaptureDecisions.orientationHint(sensor, 180, Facing.Front))
        assertEquals(180, CaptureDecisions.orientationHint(sensor, 270, Facing.Front))
    }

    @Test
    fun orientationHintIsAlwaysInRange() {
        for (facing in Facing.entries) {
            for (sensor in listOf(0, 90, 180, 270)) {
                for (rotation in listOf(0, 90, 180, 270)) {
                    val hint = CaptureDecisions.orientationHint(sensor, rotation, facing)
                    assertTrue("$facing/$sensor/$rotation -> $hint", hint in 0..359)
                }
            }
        }
    }

    // ── Long rides ───────────────────────────────────────────────────────

    @Test
    fun aSegmentRollsWhenItGetsTooBig() {
        assertTrue(CaptureDecisions.shouldRoll(CaptureDecisions.MAX_SEGMENT_BYTES, elapsedMs = 0))
        assertTrue(CaptureDecisions.shouldRoll(CaptureDecisions.MAX_SEGMENT_BYTES + 1, elapsedMs = 0))
        assertFalse(CaptureDecisions.shouldRoll(CaptureDecisions.MAX_SEGMENT_BYTES - 1, elapsedMs = 0))
    }

    @Test
    fun aSegmentRollsWhenItRunsTooLongEvenIfItIsSmall() {
        assertTrue(CaptureDecisions.shouldRoll(bytesWritten = 0, CaptureDecisions.MAX_SEGMENT_MS))
        assertFalse(CaptureDecisions.shouldRoll(bytesWritten = 0, CaptureDecisions.MAX_SEGMENT_MS - 1))
    }

    @Test
    fun theSegmentCapIsComfortablyUnderTheFourGibibyteCeiling() {
        assertTrue(CaptureDecisions.MAX_SEGMENT_BYTES < 4L * 1024 * 1024 * 1024)
    }

    // ── Stopping before the phone does ───────────────────────────────────

    @Test
    fun lowBatteryStopsRecordingOnlyWhenNotCharging() {
        assertEquals(
            CaptureDecisions.StopReason.BatteryCritical,
            CaptureDecisions.autoStop(
                batteryPercent = CaptureDecisions.MIN_BATTERY_PERCENT - 1,
                charging = false,
                freeBytes = CaptureDecisions.MIN_FREE_BYTES + 1,
                thermalStatus = 0,
            ),
        )
        // A helmet cam is often fed off the bike's own USB supply.
        assertNull(
            CaptureDecisions.autoStop(
                batteryPercent = 1,
                charging = true,
                freeBytes = CaptureDecisions.MIN_FREE_BYTES + 1,
                thermalStatus = 0,
            ),
        )
    }

    @Test
    fun batteryExactlyAtTheFloorIsNotYetCritical() {
        assertNull(
            CaptureDecisions.autoStop(
                batteryPercent = CaptureDecisions.MIN_BATTERY_PERCENT,
                charging = false,
                freeBytes = CaptureDecisions.MIN_FREE_BYTES + 1,
                thermalStatus = 0,
            ),
        )
    }

    @Test
    fun lowStorageStopsRecordingRegardlessOfBatteryOrCharging() {
        assertEquals(
            CaptureDecisions.StopReason.StorageLow,
            CaptureDecisions.autoStop(
                batteryPercent = 100,
                charging = true,
                freeBytes = CaptureDecisions.MIN_FREE_BYTES - 1,
                thermalStatus = 0,
            ),
        )
    }

    @Test
    fun freeBytesExactlyAtTheFloorIsNotYetLow() {
        assertNull(
            CaptureDecisions.autoStop(
                batteryPercent = 100,
                charging = true,
                freeBytes = CaptureDecisions.MIN_FREE_BYTES,
                thermalStatus = 0,
            ),
        )
    }

    @Test
    fun onlyThermalEmergencyStopsRecordingNotTheEarlierThrottleStates() {
        // SEVERE (4) and below are states a phone recording video legitimately
        // reaches on a warm day; stopping there would kill the feature every
        // summer afternoon.
        for (status in 0..(CaptureDecisions.THERMAL_STOP_STATUS - 1)) {
            assertNull(
                "status $status stopped recording",
                CaptureDecisions.autoStop(
                    batteryPercent = 100,
                    charging = true,
                    freeBytes = CaptureDecisions.MIN_FREE_BYTES + 1,
                    thermalStatus = status,
                ),
            )
        }
        assertEquals(
            CaptureDecisions.StopReason.ThermalCritical,
            CaptureDecisions.autoStop(
                batteryPercent = 100,
                charging = true,
                freeBytes = CaptureDecisions.MIN_FREE_BYTES + 1,
                thermalStatus = CaptureDecisions.THERMAL_STOP_STATUS,
            ),
        )
    }

    @Test
    fun whenEverythingTripsAtOnceThermalWinsThenStorageThenBattery() {
        val everythingBad = CaptureDecisions.autoStop(
            batteryPercent = 0,
            charging = false,
            freeBytes = 0,
            thermalStatus = CaptureDecisions.THERMAL_STOP_STATUS,
        )
        assertEquals(CaptureDecisions.StopReason.ThermalCritical, everythingBad)

        val storageAndBattery = CaptureDecisions.autoStop(
            batteryPercent = 0,
            charging = false,
            freeBytes = 0,
            thermalStatus = 0,
        )
        assertEquals(CaptureDecisions.StopReason.StorageLow, storageAndBattery)
    }

    @Test
    fun nothingTrippingMeansKeepGoing() {
        assertNull(
            CaptureDecisions.autoStop(
                batteryPercent = 100,
                charging = false,
                freeBytes = CaptureDecisions.MIN_FREE_BYTES + 1,
                thermalStatus = 0,
            ),
        )
    }

    // ── The feature itself ───────────────────────────────────────────────

    @Test
    fun thePreviewIsAttachedOnlyWhenTheScreenIsOnAndTheAppIsVisible() {
        assertTrue(CaptureDecisions.previewAttached(screenOn = true, uiVisible = true))
        assertFalse(CaptureDecisions.previewAttached(screenOn = false, uiVisible = true))
        assertFalse(CaptureDecisions.previewAttached(screenOn = true, uiVisible = false))
        assertFalse(CaptureDecisions.previewAttached(screenOn = false, uiVisible = false))
    }

    // ── The elapsed-time readout ─────────────────────────────────────────

    @Test
    fun theElapsedLabelStaysMinutesAndSecondsUnderAnHour() {
        assertEquals("00:00", CaptureDecisions.elapsedLabel(0))
        assertEquals("01:05", CaptureDecisions.elapsedLabel(65_000))
        assertEquals("59:59", CaptureDecisions.elapsedLabel(3_599_999))
    }

    @Test
    fun theElapsedLabelEarnsAnUnpaddedHourFieldAtOneHour() {
        assertEquals("1:00:00", CaptureDecisions.elapsedLabel(3_600_000))
        assertEquals("1:02:03", CaptureDecisions.elapsedLabel(3_723_000))
    }

    @Test
    fun aNegativeElapsedTimeClampsToZeroRatherThanShowingASign() {
        assertEquals("00:00", CaptureDecisions.elapsedLabel(-5_000))
    }
}
