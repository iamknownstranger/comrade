package mullu.comrade.capture

/**
 * The helmet-cam decisions that must not live in a `Camera2`/`MediaRecorder`
 * callback — pure, with no Android imports, so the JVM lane can pin them.
 *
 * The feature is one sentence: mount the phone, press record, press the
 * hardware lock button, keep riding. The screen is what heats the phone and
 * drains the battery on a mount with no airflow, so [previewAttached] is the
 * whole point of this file — everything else here exists to make the file it
 * writes land in the right place, survive a long ride, and stop itself before
 * the phone does.
 *
 * **The gallery is not incidental.** Footage lands in [GALLERY_RELATIVE_PATH]
 * — `DCIM/Camera`, the stock camera app's own folder — so it shows up next to
 * every other clip and Google Photos' default backup picks it up without the
 * rider doing anything extra. [Destination.Private] is the opt-out for anyone
 * who does not want a ride on their public timeline; it changes only where the
 * file is written; naming and rollover are the same either way.
 *
 * **The naming is deliberately old-fashioned.** [baseName] reproduces the
 * stock camera app's `VID_yyyyMMdd_HHmmss` convention using nothing but
 * integer arithmetic on the shifted epoch — no `java.util.Calendar`, no
 * `java.time` — because that arithmetic is the one part of this feature this
 * sandbox can actually run before CI, and a `TimeZone` lookup would not
 * survive that trip.
 */
object CaptureDecisions {

    // ── Cameras ───────────────────────────────────────────────────────────

    enum class Facing { Back, Front, External }

    /** Where a finished recording is published. */
    enum class Destination { Gallery, Private }

    /** One physical camera, as Camera2's `CameraCharacteristics` describe it. */
    data class Lens(
        val cameraId: String,
        val facing: Facing,
        val sensorOrientation: Int,
        val focalLengthMm: Float?,
    )

    /**
     * What the UI puts on the lens button: a facing plus, when a device has
     * several of that facing, which one this is (`0` == the primary).
     */
    data class LensBadge(val facing: Facing, val ordinal: Int)

    /**
     * Stable, deterministic ordering so the lens button cycles the same way
     * every launch — a helmet cam points forward, so Back comes first, Front
     * second, External (a USB action-cam lens, or a dock) last.
     *
     * Within a facing, ascending focal length: an ultrawide sorts before the
     * main sensor and a tele last, which is also the order a rider would reach
     * for them (widest field of view first). A lens that does not report a
     * focal length sorts after every lens that does — an unknown number is not
     * evidence it is the widest — and ties (including two unknowns) fall back
     * to `cameraId`, compared as numbers when both parse as one and as text
     * otherwise, so the order never depends on `HashMap` iteration or on which
     * lens Camera2 happened to enumerate first.
     */
    fun orderLenses(lenses: List<Lens>): List<Lens> = lenses.sortedWith(LENS_ORDER)

    private val LENS_ORDER = Comparator<Lens> { a, b ->
        val facing = facingRank(a.facing) - facingRank(b.facing)
        if (facing != 0) return@Comparator facing
        val focal = compareFocal(a.focalLengthMm, b.focalLengthMm)
        if (focal != 0) return@Comparator focal
        compareCameraId(a.cameraId, b.cameraId)
    }

    private fun facingRank(facing: Facing): Int = when (facing) {
        Facing.Back -> 0
        Facing.Front -> 1
        Facing.External -> 2
    }

    private fun compareFocal(a: Float?, b: Float?): Int = when {
        a == null && b == null -> 0
        a == null -> 1 // an unknown focal length is not evidence it is the widest
        b == null -> -1
        else -> a.compareTo(b)
    }

    private fun compareCameraId(a: String, b: String): Int {
        val an = a.toLongOrNull()
        val bn = b.toLongOrNull()
        return if (an != null && bn != null) an.compareTo(bn) else a.compareTo(b)
    }

    /** The first Back lens, if any — a helmet cam points forward — else
     *  whichever [orderLenses] put first. */
    fun defaultLens(ordered: List<Lens>): Lens? =
        ordered.firstOrNull { it.facing == Facing.Back } ?: ordered.firstOrNull()

    /**
     * Cycle to the next lens in [ordered], wrapping. A `currentId` this build
     * cannot find — an unplugged USB camera, a stale selection from before a
     * relaunch — returns the first entry rather than throwing or standing
     * still, because the button must always do something. `null` only when
     * there is nothing to select at all.
     */
    fun nextLens(ordered: List<Lens>, currentId: String?): Lens? {
        if (ordered.isEmpty()) return null
        val idx = ordered.indexOfFirst { it.cameraId == currentId }
        return if (idx < 0) ordered.first() else ordered[(idx + 1) % ordered.size]
    }

    /** The badge for one lens: its facing, and how many lenses of that same
     *  facing precede it in [ordered]. `null` for an id [ordered] does not
     *  contain. */
    fun badge(ordered: List<Lens>, cameraId: String): LensBadge? {
        val lens = ordered.find { it.cameraId == cameraId } ?: return null
        var ordinal = 0
        for (l in ordered) {
            if (l.cameraId == cameraId) break
            if (l.facing == lens.facing) ordinal++
        }
        return LensBadge(lens.facing, ordinal)
    }

    // ── Where a file goes and what it is called ─────────────────────────────

    /** The stock camera app's own folder, so Photos' default backup and the
     *  gallery pick this footage up as if the stock app had shot it. */
    const val GALLERY_RELATIVE_PATH: String = "DCIM/Camera"

    /** Where a [Destination.Private] recording is written instead — off the
     *  gallery path entirely, not merely hidden from it with a `.nomedia`. */
    const val PRIVATE_SUBDIR: String = "private"

    const val MIME_TYPE: String = "video/mp4"

    private const val DAY_MS = 86_400_000L

    /**
     * `VID_yyyyMMdd_HHmmss`, in local time — the stock camera app's own naming
     * convention, so this footage sorts and groups with camera footage in the
     * gallery instead of standing out as something else.
     *
     * Computed as integer arithmetic on `startedAtEpochMs` shifted by
     * [utcOffsetMinutes], deliberately without `java.util.Calendar` or
     * `java.time`: neither is worth trusting to be deterministic under test,
     * and only pure arithmetic runs in the kotlinc lane this file exists for.
     * The days-since-epoch to calendar-date step is Howard Hinnant's
     * `civil_from_days`, which is exact for the proleptic Gregorian calendar
     * on both sides of 1970 — so a negative [utcOffsetMinutes] and a
     * pre-epoch [startedAtEpochMs] fall out of the same arithmetic rather than
     * needing a separate clamp.
     */
    fun baseName(startedAtEpochMs: Long, utcOffsetMinutes: Int): String {
        val shifted = startedAtEpochMs + utcOffsetMinutes * 60_000L
        val days = floorDiv(shifted, DAY_MS)
        val msOfDay = floorMod(shifted, DAY_MS)
        val (year, month, day) = civilFromDays(days)
        val hours = msOfDay / 3_600_000L
        val minutes = (msOfDay / 60_000L) % 60L
        val seconds = (msOfDay / 1_000L) % 60L
        return buildString {
            append("VID_")
            append(year.toString().padStart(4, '0'))
            append(month.toString().padStart(2, '0'))
            append(day.toString().padStart(2, '0'))
            append('_')
            append(hours.toString().padStart(2, '0'))
            append(minutes.toString().padStart(2, '0'))
            append(seconds.toString().padStart(2, '0'))
        }
    }

    /** Division that rounds toward negative infinity, unlike `/`'s
     *  round-toward-zero — needed once [startedAtEpochMs] or the shift can be
     *  negative, or a day boundary would land on the wrong side of midnight. */
    private fun floorDiv(a: Long, b: Long): Long {
        val q = a / b
        return if (a % b != 0L && (a < 0) != (b < 0)) q - 1 else q
    }

    private fun floorMod(a: Long, b: Long): Long = a - floorDiv(a, b) * b

    /**
     * Howard Hinnant's `civil_from_days`: the calendar date for `z` days since
     * 1970-01-01, exact for the proleptic Gregorian calendar for any `z`,
     * positive or negative. See
     * http://howardhinnant.github.io/date_algorithms.html.
     */
    private fun civilFromDays(z: Long): Triple<Long, Int, Int> {
        val zz = z + 719_468L
        val era = if (zz >= 0) zz / 146_097L else (zz - 146_096L) / 146_097L
        val doe = zz - era * 146_097L // [0, 146096]
        val yoe = (doe - doe / 1_460L + doe / 36_524L - doe / 146_096L) / 365L // [0, 399]
        val y = yoe + era * 400L
        val doy = doe - (365L * yoe + yoe / 4L - yoe / 100L) // [0, 365]
        val mp = (5L * doy + 2L) / 153L // [0, 11]
        val d = (doy - (153L * mp + 2L) / 5L + 1L).toInt() // [1, 31]
        val m = (if (mp < 10L) mp + 3L else mp - 9L).toInt() // [1, 12]
        val year = if (m <= 2) y + 1L else y
        return Triple(year, m, d)
    }

    /**
     * Segment `0` is the plain name; every later one gets a zero-padded
     * suffix, so a long ride's files still sort in recording order next to
     * the un-suffixed first one.
     */
    fun segmentFileName(baseName: String, segment: Int): String =
        if (segment <= 0) "$baseName.mp4" else "${baseName}_${segment.toString().padStart(3, '0')}.mp4"

    /**
     * The value for `MediaRecorder.setOrientationHint`. Standard formula: a
     * front sensor's rotation adds to the device's, a back or external one
     * subtracts it — getting the sign wrong here is the classic "video is
     * sideways in the gallery" bug, because the front sensor is mounted
     * mirrored relative to the back one. Always returns a value in `0..359`.
     */
    fun orientationHint(sensorOrientation: Int, deviceRotationDeg: Int, facing: Facing): Int {
        val raw = when (facing) {
            Facing.Front -> sensorOrientation + deviceRotationDeg
            Facing.Back, Facing.External -> sensorOrientation - deviceRotationDeg + 360
        }
        return ((raw % 360) + 360) % 360
    }

    // ── Long rides ────────────────────────────────────────────────────────

    /**
     * Comfortably under the 4 GiB (2^32 − 1 byte) ceiling that both the MP4
     * `stco` box's 32-bit offsets and FAT32 share — 3.5 GB leaves headroom for
     * the last write before a check, rather than rolling exactly at the wire.
     */
    const val MAX_SEGMENT_BYTES: Long = 3_500_000_000L

    /** 30 minutes, so a crash or a battery pull mid-ride costs at most one
     *  segment of footage rather than the whole recording. */
    const val MAX_SEGMENT_MS: Long = 30 * 60 * 1_000L

    /** True once a segment has grown too big or run too long — either is
     *  reason enough to close this file and start the next. */
    fun shouldRoll(bytesWritten: Long, elapsedMs: Long): Boolean =
        bytesWritten >= MAX_SEGMENT_BYTES || elapsedMs >= MAX_SEGMENT_MS

    // ── Stopping before the phone does ──────────────────────────────────────

    /** Below this, and not charging, recording stops rather than following the
     *  phone into an uncontrolled shutdown mid-file. */
    const val MIN_BATTERY_PERCENT: Int = 15

    /** Free space a running recording must keep in hand — enough for the
     *  writer to finish its current buffer and close the file cleanly instead
     *  of the filesystem refusing the next write outright. */
    const val MIN_FREE_BYTES: Long = 200_000_000L

    /**
     * `PowerManager.THERMAL_STATUS_EMERGENCY` (`5`), spelled out as a number
     * because this file may not import Android. Deliberately EMERGENCY and
     * not the earlier SEVERE or CRITICAL: those are states a phone recording
     * video on a warm day *legitimately* reaches, and stopping there would
     * kill the feature every summer afternoon. EMERGENCY is the one step
     * before the OS itself intervenes.
     */
    const val THERMAL_STOP_STATUS: Int = 5

    enum class StopReason { BatteryCritical, StorageLow, ThermalCritical }

    /**
     * The reason recording must end now, or `null` to keep going.
     *
     * A fixed precedence for the case where several trip at once: thermal
     * first, because it is the state right before the OS itself throttles or
     * kills things out from under this app; storage second, because a full
     * disk corrupts the segment being written *right now*; battery last,
     * because it is the most survivable of the three — plug in and the same
     * check passes on the next sample, and a battery that ran out regardless
     * still leaves a complete, playable file behind.
     *
     * Battery is checked with `!charging`: a helmet cam is often fed off a
     * bike's own USB supply, and stopping a phone that is actively charging
     * would be exactly wrong.
     */
    fun autoStop(batteryPercent: Int, charging: Boolean, freeBytes: Long, thermalStatus: Int): StopReason? = when {
        thermalStatus >= THERMAL_STOP_STATUS -> StopReason.ThermalCritical
        freeBytes < MIN_FREE_BYTES -> StopReason.StorageLow
        batteryPercent < MIN_BATTERY_PERCENT && !charging -> StopReason.BatteryCritical
        else -> null
    }

    // ── The feature ──────────────────────────────────────────────────────

    /**
     * Whether the camera session should have a preview surface attached at
     * all.
     *
     * This one function is the whole feature. When it returns `false` the
     * capture session is reconfigured down to the encoder surface alone — no
     * preview stream is produced, so the display and the GPU that would
     * otherwise composite it do no work. That is what lets recording survive
     * the hardware lock button: the screen going off is not a pause, it is a
     * cheaper capture session that happens to write the same file.
     */
    fun previewAttached(screenOn: Boolean, uiVisible: Boolean): Boolean = screenOn && uiVisible

    /**
     * The running-time readout: `"MM:SS"` under an hour, `"H:MM:SS"` at or
     * over one — the hour field is unpadded because nothing above it needs
     * lining up, unlike the minutes and seconds either side of a colon.
     * Negative input clamps to zero rather than showing a sign.
     */
    fun elapsedLabel(elapsedMs: Long): String {
        val totalSeconds = elapsedMs.coerceAtLeast(0L) / 1_000L
        val seconds = totalSeconds % 60L
        val minutes = (totalSeconds / 60L) % 60L
        val hours = totalSeconds / 3_600L
        return if (hours > 0L) {
            "$hours:${pad2(minutes)}:${pad2(seconds)}"
        } else {
            "${pad2(minutes)}:${pad2(seconds)}"
        }
    }

    private fun pad2(value: Long): String = value.toString().padStart(2, '0')
}
