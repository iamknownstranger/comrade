package mullu.comrade.capture

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.graphics.ImageFormat
import android.graphics.Rect
import android.graphics.SurfaceTexture
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CaptureFailure
import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.TotalCaptureResult
import android.hardware.camera2.params.MeteringRectangle
import android.media.CamcorderProfile
import android.media.ImageReader
import android.media.MediaRecorder
import android.os.BatteryManager
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.PowerManager
import android.os.StatFs
import android.os.SystemClock
import android.util.Log
import android.util.Size
import android.view.Surface
import android.view.WindowManager
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import java.util.TimeZone
import kotlin.math.abs
import kotlin.math.roundToInt

/**
 * The Android side of Capture: owns the camera, the recorder and every
 * decision about them, and is what starts and stops [CaptureService] — the
 * same split [mullu.comrade.together.TogetherManager] and
 * [mullu.comrade.together.TogetherService] already use. [CaptureService]
 * holds only the foreground-service contract and the notification; every
 * decision about *whether* to roll, reconfigure or stop lives here or,
 * wherever it can, in the pure [CaptureDecisions]/[CameraApp].
 *
 * ## The open camera is not the recording
 *
 * The original helmet-cam build opened the camera only when [start] was
 * called and closed it in [stop] — a black rectangle until you pressed
 * record. That is split into two objects now: [Camera] is an open
 * `CameraDevice` with a live `CameraCaptureSession`, which exists whenever a
 * preview surface, permission and visible UI ask for one, whether or not
 * anything is recording; [Recording] is the `MediaRecorder` half, which
 * exists only between [start] and whatever ends it. A recording always
 * reuses whatever [Camera] the preview already opened on the selected lens
 * rather than closing and reopening the device — see [beginRecording].
 *
 * The helmet-cam invariant does not move: [CaptureDecisions.previewAttached]
 * still decides only whether the *preview* output is attached to whatever
 * session is live, a recording's [Camera] and [Recording] never close because
 * the UI went away, and [reconcileLocked] is the one place that decision
 * turns into an actual `createCaptureSession` call — mirroring the one this
 * file used to make only while recording, now made for a live viewfinder too.
 *
 * ## Threading
 *
 * `Camera2`'s device/session callbacks land on [cameraHandler], a dedicated
 * [HandlerThread] — never the main thread and never a coroutine dispatcher
 * with no `Looper`, which `CameraManager.openCamera`/`createCaptureSession`
 * both require. Every operation that mutates [camera] or [recording] is
 * launched on [captureLane], `Dispatchers.IO` restricted to one thread at a
 * time — the same pattern [mullu.comrade.call.CallManager.webRtcLane] uses to
 * keep two camera/recorder operations from ever interleaving, without holding
 * a lock across a suspending call. Both fields are `@Volatile` so the ticker
 * and guard loop (running on [scope]'s default dispatcher) see a freshly
 * published reference instead of a stale one cached in a register.
 *
 * A suspending step inside a `captureLane`-launched coroutine (opening the
 * device, configuring a session) still yields the thread to another queued
 * `captureLane` coroutine at its suspension point — `limitedParallelism(1)`
 * bounds how many run *at once*, not the order two separately launched
 * coroutines interleave in. [captureStill] calls that out at the one place it
 * matters in practice.
 *
 * ## What "screen off" costs
 *
 * See [reconcileLocked]: Camera2 cannot add or remove an output from a live
 * [CameraCaptureSession], so attaching or detaching the preview — or the
 * still-photo `ImageReader`, or the recorder surface — is a fresh
 * `createCaptureSession` on the same open [CameraDevice]. A running
 * [Recording]'s [MediaRecorder] is never stopped across that — the file keeps
 * growing and its timestamps absorb the gap — but frames genuinely stop
 * arriving for the length of the reconfiguration. A few are lost at every
 * screen on/off transition, every mode switch, and every lens switch that now
 * happens while only the preview (not a recording) is live. That is the real
 * cost of the design, not an incidental one, and no comment here should claim
 * otherwise.
 *
 * A reconfiguration can also fail outright rather than merely cost frames.
 * While a [Recording] is live that has a defined answer instead of a log
 * line: drop the preview and retry encoder-only, and if even that will not
 * configure, end the recording and publish it under
 * [Reason.StoppedCameraLost] — a helmet cam whose session quietly died is
 * indistinguishable, while riding, from one that is working. Outside a
 * recording there is nothing to protect this way: a failed preview
 * reconfiguration just leaves the viewfinder waiting for the next state
 * change to try again.
 *
 * ## Photo, zoom, flash, focus
 *
 * [Camera] carries the request parameters a fresh repeating request must
 * always reapply after *any* `createCaptureSession` — zoom ratio, flash mode,
 * the tap-to-focus metering rectangle (see [applyLiveParams]) — because a
 * reconfiguration silently dropping them (a zoom level reset by the screen
 * turning back on, say) is exactly the class of bug this file's comments
 * exist to prevent, not merely describe after the fact.
 *
 * A still photo needs its own `ImageReader` output, added only in
 * [CameraApp.Mode.Photo] and only while nothing is recording — see
 * [takePhoto]'s doc comment for why a still is refused, rather than attempted
 * through a live-session reconfiguration, while a [Recording] is running.
 */
object CaptureManager {

    private const val TAG = "CaptureManager"

    sealed interface UiState {
        data object Idle : UiState
        data object Preparing : UiState
        data class Recording(
            val startedElapsedRealtimeMs: Long,
            val destination: CaptureDecisions.Destination,
            val lens: CaptureDecisions.Lens,
            val segment: Int,
        ) : UiState
        data class Failed(val reason: Reason) : UiState
    }

    enum class Reason {
        CameraUnavailable,
        RecorderFailed,
        NoStorage,
        PermissionDenied,
        StoppedBattery,
        StoppedStorage,
        StoppedThermal,

        /**
         * The camera stopped being able to deliver frames mid-recording and
         * [reconcileLocked] could not get a session back, not even the
         * encoder-only one. Grouped with the other `Stopped*` reasons rather
         * than with [CameraUnavailable] because it is the same shape of
         * outcome: the ride's footage up to this point is finished and
         * published, and the user is told. The alternative — the behaviour
         * this replaced — was to log a line and keep a recording alive that
         * was writing no video at all.
         */
        StoppedCameraLost,

        /**
         * [takePhoto] could not get a JPEG — no still output was configured
         * (most often: a video [Recording] is running, and a still during one
         * is deliberately refused, see that function's doc comment), the
         * capture request failed, or nothing wrote successfully to
         * [CaptureSink]. Reuses the same [UiState.Failed] shape the recording
         * failures use — see [takePhoto] for why it is only ever published
         * while [state] is [UiState.Idle] (or already [UiState.Failed]),
         * never over a live [UiState.Recording]/[UiState.Preparing].
         */
        PhotoFailed,
    }

    enum class Kind { Photo, Video }

    data class Saved(
        val uri: String?,
        val displayName: String,
        val destination: CaptureDecisions.Destination,
        val kind: Kind,
    )

    /** A tap-to-focus point, in normalised viewfinder coordinates
     *  (`(0,0)` top-left, `(1,1)` bottom-right) — the same coordinate space
     *  [CameraApp.meteringRect] takes. [atElapsedRealtimeMs] is when it was
     *  set, so the UI can fade the reticle out on its own rather than this
     *  object owning a UI-timing concern. */
    data class FocusPoint(val nx: Float, val ny: Float, val atElapsedRealtimeMs: Long)

    private val _state = MutableStateFlow<UiState>(UiState.Idle)
    val state: StateFlow<UiState> = _state.asStateFlow()

    private val _lenses = MutableStateFlow<List<CaptureDecisions.Lens>>(emptyList())
    val lenses: StateFlow<List<CaptureDecisions.Lens>> = _lenses.asStateFlow()

    private val _selectedLensId = MutableStateFlow<String?>(null)
    val selectedLensId: StateFlow<String?> = _selectedLensId.asStateFlow()

    private val _elapsedMs = MutableStateFlow(0L)
    val elapsedMs: StateFlow<Long> = _elapsedMs.asStateFlow()

    private val _lastSaved = MutableStateFlow<Saved?>(null)
    val lastSaved: StateFlow<Saved?> = _lastSaved.asStateFlow()

    /** Video is the helmet-cam default: pressing Record works without
     *  touching this at all, matching the feature's original behaviour. */
    private val _mode = MutableStateFlow(CameraApp.Mode.Video)
    val mode: StateFlow<CameraApp.Mode> = _mode.asStateFlow()

    private val _flash = MutableStateFlow(CameraApp.Flash.Off)
    val flash: StateFlow<CameraApp.Flash> = _flash.asStateFlow()

    private val _hasFlashUnit = MutableStateFlow(false)
    val hasFlashUnit: StateFlow<Boolean> = _hasFlashUnit.asStateFlow()

    private val _zoomRatio = MutableStateFlow(1f)
    val zoomRatio: StateFlow<Float> = _zoomRatio.asStateFlow()

    private val _zoomRange = MutableStateFlow(1f..1f)
    val zoomRange: StateFlow<ClosedFloatingPointRange<Float>> = _zoomRange.asStateFlow()

    private val _capturingPhoto = MutableStateFlow(false)
    val capturingPhoto: StateFlow<Boolean> = _capturingPhoto.asStateFlow()

    private val _focusPoint = MutableStateFlow<FocusPoint?>(null)
    val focusPoint: StateFlow<FocusPoint?> = _focusPoint.asStateFlow()

    /** See [mullu.comrade.call.CallManager.disableCallServiceForTest] — same
     *  seam, same reason: skip only the [CaptureService] start/stop. Never
     *  touched by any production code path. */
    @Volatile
    var disableServiceForTest: Boolean = false

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    /** Serializes every operation that touches [camera]/[recording] — see the
     *  class doc's Threading section. */
    private val captureLane = Dispatchers.IO.limitedParallelism(1)

    private val cameraThread = HandlerThread("CaptureCamera").apply { start() }
    private val cameraHandler = Handler(cameraThread.looper)

    @Volatile
    private var camera: Camera? = null

    @Volatile
    private var recording: Recording? = null

    @Volatile
    private var preview: PreviewTarget? = null

    /** A recording always starts with the screen on — the user just tapped
     *  Record — so this is the honest initial value rather than a guess. */
    @Volatile
    private var screenOn: Boolean = true

    @Volatile
    private var uiVisible: Boolean = true

    /**
     * [onScreenStateChanged], [setZoom], [setFlash], [focusAt] and
     * [selectLens] act on an already-open [camera] (or, for [selectLens],
     * need only enough of a [Context] to reopen the device) and have no
     * [Context] parameter of their own — the UI agent's contract for this
     * object fixes those signatures. Every other entry point does receive a
     * [Context]; this is where the most recent one is kept for the ones that
     * do not. Always the application context, never an `Activity`.
     */
    @Volatile
    private var appContext: Context? = null

    private fun rememberContext(context: Context) {
        appContext = context.applicationContext
    }

    private data class PreviewTarget(val texture: SurfaceTexture, val width: Int, val height: Int)

    private enum class OutputKind { Preview, Recorder, Still }

    private data class Output(val surface: Surface, val kind: OutputKind)

    private data class VideoConfig(
        val width: Int,
        val height: Int,
        val frameRate: Int,
        val bitRate: Int,
        val audioBitRate: Int,
        val audioSampleRate: Int,
        val audioChannels: Int,
    )

    /**
     * One open `CameraDevice` plus everything currently attached to it: the
     * live `CameraCaptureSession`, its output list, the still-photo
     * `ImageReader` (kept across reconfigurations rather than rebuilt every
     * time — see [ensureStillReader]), and the request parameters a fresh
     * repeating request must always carry (class doc: "Photo, zoom, flash,
     * focus").
     *
     * Opened independently of whether anything is recording, and closed only
     * when nothing wants it open at all — see [reconcileLocked].
     */
    private class Camera(
        val lens: CaptureDecisions.Lens,
        val chars: CameraCharacteristics,
        val config: VideoConfig,
    ) {
        var device: CameraDevice? = null
        var captureSession: CameraCaptureSession? = null
        var activeOutputs: List<Output> = emptyList()
        var imageReader: ImageReader? = null

        val hasFlashUnit: Boolean = chars.get(CameraCharacteristics.FLASH_INFO_AVAILABLE) == true
        val zoomRange: ClosedFloatingPointRange<Float> = zoomRangeFor(chars)

        // Live request parameters, reapplied to every repeating (and one-off)
        // request built after this point — see applyLiveParams.
        var zoomRatio: Float = 1f
        var flash: CameraApp.Flash = CameraApp.Flash.Off
        var meteringRect: CameraApp.MeteringRect? = null
    }

    /**
     * The `MediaRecorder` half of a running capture, split out from [Camera]
     * so the camera can be open — with a live preview, in Photo mode,
     * whatever — with no recording underneath it at all.
     */
    private class Recording(
        val appContext: Context,
        val destination: CaptureDecisions.Destination,
        val baseName: String,
        val config: VideoConfig,
        val startedElapsedRealtimeMs: Long,
    ) {
        var recorder: MediaRecorder? = null
        var current: CaptureSink.Segment? = null
        var armedNext: CaptureSink.Segment? = null
        var segmentIndex: Int = 0
        var wakeLock: PowerManager.WakeLock? = null
        var guardJob: Job? = null
        var tickerJob: Job? = null
    }

    private data class DesiredOutputs(val preview: Boolean, val stillReader: Boolean)

    // ── Lenses ───────────────────────────────────────────────────────────

    fun refreshLenses(context: Context) {
        rememberContext(context)
        val appCtx = context.applicationContext
        scope.launch(Dispatchers.Default) {
            val discovered = mutableListOf<CaptureDecisions.Lens>()
            val manager = appCtx.getSystemService(CameraManager::class.java)
            if (manager != null) {
                for (id in runCatching { manager.cameraIdList }.getOrDefault(emptyArray())) {
                    val chars = runCatching { manager.getCameraCharacteristics(id) }.getOrNull() ?: continue
                    val facing = when (chars.get(CameraCharacteristics.LENS_FACING)) {
                        CameraCharacteristics.LENS_FACING_FRONT -> CaptureDecisions.Facing.Front
                        CameraCharacteristics.LENS_FACING_EXTERNAL -> CaptureDecisions.Facing.External
                        else -> CaptureDecisions.Facing.Back
                    }
                    val sensorOrientation = chars.get(CameraCharacteristics.SENSOR_ORIENTATION) ?: 0
                    val focal = chars.get(CameraCharacteristics.LENS_INFO_AVAILABLE_FOCAL_LENGTHS)?.minOrNull()
                    discovered += CaptureDecisions.Lens(id, facing, sensorOrientation, focal)
                }
            }
            val ordered = CaptureDecisions.orderLenses(discovered)
            _lenses.value = ordered
            if (ordered.none { it.cameraId == _selectedLensId.value }) {
                _selectedLensId.value = CaptureDecisions.defaultLens(ordered)?.cameraId
            }
            scope.launch(captureLane) {
                // The selection above may have moved out from under an
                // already-open preview (a lens vanished, e.g. a USB camera
                // unplugged) — close it so reconcileLocked reopens fresh on
                // whatever is selected now, instead of leaving the old lens
                // live because nothing else told it to change.
                val cam = camera
                if (cam != null && recording == null && cam.lens.cameraId != _selectedLensId.value) {
                    closeCameraLocked(cam)
                }
                reconcileLocked()
            }
        }
    }

    fun selectLens(cameraId: String) {
        if (_lenses.value.none { it.cameraId == cameraId }) return
        if (_state.value is UiState.Recording) {
            // Recorded for the *next* recording, same as the old behaviour —
            // Camera2 cannot move a live session between physical cameras
            // without closing the file it is writing.
            _selectedLensId.value = cameraId
            return
        }
        scope.launch(captureLane) {
            val lens = _lenses.value.find { it.cameraId == cameraId } ?: return@launch
            changeLensLocked(lens)
        }
    }

    /**
     * Cycle to the next lens. A no-op while [state] is [UiState.Recording]:
     * Camera2 cannot move a live session to a different physical camera
     * without closing the [CameraDevice], and closing it finalises the MP4
     * mid-shot. While only the *preview* is running this now really does
     * switch it live — reopening the device on the new lens and rebuilding
     * the session, at the same frame-loss cost documented for every other
     * reconfiguration in the class doc.
     */
    fun switchLens(context: Context) {
        rememberContext(context)
        if (_state.value is UiState.Recording) return
        scope.launch(captureLane) {
            val next = CaptureDecisions.nextLens(_lenses.value, _selectedLensId.value) ?: return@launch
            changeLensLocked(next)
        }
    }

    private suspend fun changeLensLocked(lens: CaptureDecisions.Lens) {
        _selectedLensId.value = lens.cameraId
        if (recording != null) return // callers above already refuse this; kept as a backstop
        camera?.let { closeCameraLocked(it) }
        reconcileLocked()
    }

    // ── Preview / visibility / screen state ─────────────────────────────

    fun attachPreview(context: Context, texture: SurfaceTexture?, width: Int, height: Int) {
        rememberContext(context)
        preview = texture?.let { PreviewTarget(it, width, height) }
        scope.launch(captureLane) { reconcileLocked() }
    }

    fun setUiVisible(context: Context, visible: Boolean) {
        rememberContext(context)
        uiVisible = visible
        scope.launch(captureLane) { reconcileLocked() }
    }

    /** Called by [CaptureService]'s `ACTION_SCREEN_OFF`/`ACTION_SCREEN_ON`
     *  receiver — these cannot be declared in the manifest, so the service
     *  registers a dynamic one and routes the result in here. Only ever
     *  delivered while [CaptureService] is running, i.e. only during a
     *  recording — so this never needs to *open* a camera, only reconfigure
     *  one that [beginRecording] already has open. */
    fun onScreenStateChanged(screenOn: Boolean) {
        this.screenOn = screenOn
        scope.launch(captureLane) { reconcileLocked() }
    }

    // ── Mode / flash / zoom / focus ──────────────────────────────────────

    /**
     * Switch between [CameraApp.Mode.Photo] and [CameraApp.Mode.Video] — adds
     * or drops the still-photo `ImageReader` output on whatever is currently
     * open. A no-op while [state] is [UiState.Recording]: adding an
     * `ImageReader` to a session that is also, right now, writing the file
     * that matters is exactly the risk [takePhoto] refuses for the same
     * reason (see its doc comment); the UI is expected to disable the mode
     * toggle while recording, and this is the backstop if a stray tap gets
     * through anyway.
     */
    fun setMode(context: Context, mode: CameraApp.Mode) {
        rememberContext(context)
        if (_state.value is UiState.Recording) return
        if (_mode.value == mode) return
        _mode.value = mode
        scope.launch(captureLane) { reconcileLocked() }
    }

    fun setFlash(flash: CameraApp.Flash) {
        scope.launch(captureLane) {
            val cam = camera
            val normalized = when {
                cam == null -> flash // nothing open yet to check a flash unit against — trust the caller
                cam.hasFlashUnit -> flash
                else -> CameraApp.Flash.Off // this lens reports no flash unit; ignore the request rather than pretend
            }
            _flash.value = normalized
            if (cam == null) return@launch
            cam.flash = normalized
            reapplyRepeatingRequest(cam)
        }
    }

    fun setZoom(ratio: Float) {
        scope.launch(captureLane) {
            val cam = camera
            if (cam == null) {
                // Clamp against the last-known range and remember it for
                // whichever lens opens next — openCameraLocked reseeds from
                // this value.
                _zoomRatio.value = CameraApp.clampZoom(ratio, _zoomRange.value.start, _zoomRange.value.endInclusive)
                return@launch
            }
            cam.zoomRatio = CameraApp.clampZoom(ratio, cam.zoomRange.start, cam.zoomRange.endInclusive)
            _zoomRatio.value = cam.zoomRatio
            reapplyRepeatingRequest(cam)
        }
    }

    /**
     * Tap-to-focus at normalised viewfinder point ([nx], [ny]). Builds the
     * metering rectangle through [CameraApp.meteringRect] — the sensor-space
     * math is not repeated here — applies it as `CONTROL_AF_REGIONS`/
     * `CONTROL_AE_REGIONS` with a one-shot `CONTROL_AF_TRIGGER_START`, then
     * returns to a plain repeating request that still carries the new
     * metering rectangle (via [applyLiveParams]) without re-triggering AF on
     * every subsequent frame. A lens reporting zero AF *and* AE regions
     * (`CONTROL_MAX_REGIONS_AF`/`_AE` both `0`) makes this a no-op, not a
     * crash — some lenses genuinely have neither.
     */
    fun focusAt(nx: Float, ny: Float) {
        _focusPoint.value = FocusPoint(nx, ny, SystemClock.elapsedRealtime())
        scope.launch(captureLane) {
            val cam = camera ?: return@launch
            val device = cam.device ?: return@launch
            val session = cam.captureSession ?: return@launch
            val maxAf = cam.chars.get(CameraCharacteristics.CONTROL_MAX_REGIONS_AF) ?: 0
            val maxAe = cam.chars.get(CameraCharacteristics.CONTROL_MAX_REGIONS_AE) ?: 0
            if (maxAf <= 0 && maxAe <= 0) return@launch
            val active = cam.chars.get(CameraCharacteristics.SENSOR_INFO_ACTIVE_ARRAY_SIZE) ?: return@launch
            val outputs = cam.activeOutputs
            if (outputs.isEmpty()) return@launch

            cam.meteringRect = CameraApp.meteringRect(
                nx,
                ny,
                active.width(),
                active.height(),
                cam.lens.sensorOrientation,
                mirrored = cam.lens.facing == CaptureDecisions.Facing.Front,
            )

            val template = if (recording != null) CameraDevice.TEMPLATE_RECORD else CameraDevice.TEMPLATE_PREVIEW
            val trigger = runCatching { device.createCaptureRequest(template) }.getOrNull() ?: return@launch
            outputs.forEach { trigger.addTarget(it.surface) }
            applyLiveParams(trigger, cam)
            trigger.set(CaptureRequest.CONTROL_AF_TRIGGER, CaptureRequest.CONTROL_AF_TRIGGER_START)
            runCatching { session.capture(trigger.build(), null, cameraHandler) }

            reapplyRepeatingRequest(cam)
        }
    }

    // ── Photo capture ─────────────────────────────────────────────────────

    /**
     * Take a still. Requires the live session to already have a still-photo
     * `ImageReader` output configured — which [reconcileLocked] only ever
     * adds while [CameraApp.Mode.Photo] is selected and nothing is recording.
     *
     * **A still while a video [Recording] is running is refused, not
     * attempted.** A Pixel supports this; this build does not. Camera2 has no
     * way to add an output to a *live* session — every reconfiguration in
     * this file is a fresh `createCaptureSession`, and [reconcileLocked]
     * already treats that as a real, documented cost (frame loss on success,
     * a defined but limited fallback on failure) when it happens for a screen
     * on/off transition, which is rare. Doing the same thing on every shutter
     * tap during a recording — the one time this app's entire purpose is "the
     * recording must not stop" — trades a rare, well-understood risk for a
     * frequent one with no automatic recovery path back to the recorder-only
     * set if it goes wrong mid-attempt. Refusing is the stated compromise;
     * see `docs/CAPTURE.md` §7 for the same shape of trade-off already made
     * for lens switching.
     *
     * The refusal (and any other capture failure) only reaches [state] while
     * [state] is [UiState.Idle] or already [UiState.Failed] — never over a
     * live [UiState.Recording] or [UiState.Preparing], which this must not
     * disturb.
     */
    fun takePhoto(context: Context, destination: CaptureDecisions.Destination) {
        rememberContext(context)
        if (_capturingPhoto.value) return
        scope.launch(captureLane) {
            val cam = camera
            val hasReader = cam?.activeOutputs?.any { it.kind == OutputKind.Still } == true
            if (cam == null || !hasReader) {
                Log.w(TAG, "no still output configured; refusing capture")
                failPhotoIfIdle()
                return@launch
            }
            _capturingPhoto.value = true
            val ok = runCatching { captureStill(context, destination, cam) }
                .getOrElse { e ->
                    Log.e(TAG, "still capture crashed", e)
                    false
                }
            _capturingPhoto.value = false
            if (!ok) failPhotoIfIdle()
        }
    }

    private fun failPhotoIfIdle() {
        if (_state.value !is UiState.Recording && _state.value !is UiState.Preparing) {
            _state.value = UiState.Failed(Reason.PhotoFailed)
        }
    }

    /**
     * Runs on [captureLane]. A capture request's own suspension points
     * (waiting on the `ImageReader` callback, the precapture trigger) yield
     * the lane to whatever else is queued on it — `limitedParallelism(1)`
     * bounds concurrent execution, not the order two separately launched
     * coroutines interleave in. If [start] lands in that window and
     * reconfigures [cam] out from under this capture (dropping the still
     * reader because a recording now needs the encoder surface instead), the
     * `ImageReader` this already submitted a request to may never fire its
     * callback again. The `withTimeoutOrNull` below is what turns that into a
     * clean [Reason.PhotoFailed] instead of a hang; it does not prevent the
     * interleaving itself, which would need a real mutex around the whole
     * capture rather than the coroutine-per-hop pattern the rest of this file
     * uses. A shutter tap and a record-button tap landing in the same tens of
     * milliseconds is judged rare enough that this contained failure mode is
     * an acceptable trade for not rewriting the file's concurrency model.
     */
    private suspend fun captureStill(
        context: Context,
        destination: CaptureDecisions.Destination,
        cam: Camera,
    ): Boolean {
        val device = cam.device ?: return false
        val session = cam.captureSession ?: return false
        val reader = cam.imageReader ?: return false

        runPrecaptureIfNeeded(cam)

        val builder = runCatching { device.createCaptureRequest(CameraDevice.TEMPLATE_STILL_CAPTURE) }
            .getOrNull() ?: return false
        builder.addTarget(reader.surface)
        applyLiveParams(builder, cam)
        val rotationDeg = currentRotationDeg(context)
        builder.set(
            CaptureRequest.JPEG_ORIENTATION,
            CaptureDecisions.orientationHint(cam.lens.sensorOrientation, rotationDeg, cam.lens.facing),
        )

        val imageDeferred = CompletableDeferred<ByteArray?>()
        reader.setOnImageAvailableListener(
            { r ->
                val image = r.acquireLatestImage()
                if (image == null) {
                    imageDeferred.complete(null)
                } else {
                    val buffer = image.planes[0].buffer
                    val bytes = ByteArray(buffer.remaining())
                    buffer.get(bytes)
                    image.close()
                    imageDeferred.complete(bytes)
                }
            },
            cameraHandler,
        )

        val submitted = runCatching { session.capture(builder.build(), null, cameraHandler) }.isSuccess
        if (!submitted) return false
        val bytes = withTimeoutOrNull(5_000L) { imageDeferred.await() }
        if (bytes == null || bytes.isEmpty()) return false

        val startedAtEpochMs = System.currentTimeMillis()
        val utcOffsetMinutes = TimeZone.getDefault().getOffset(startedAtEpochMs) / 60_000
        val displayName = CaptureDecisions.photoFileName(CaptureDecisions.photoBaseName(startedAtEpochMs, utcOffsetMinutes))
        val still = CaptureSink.openStill(context, destination, displayName) ?: return false
        val wrote = still.write(bytes)
        val uri = still.finish(success = wrote)
        if (!wrote) return false
        _lastSaved.value = Saved(uri?.toString(), displayName, destination, Kind.Photo)
        return true
    }

    /**
     * A best-effort AE precapture trigger before a flash-lit still: fire one
     * request with `CONTROL_AE_PRECAPTURE_TRIGGER_START` over whatever the
     * live (non-still) outputs are, and wait — up to a fixed timeout, not a
     * real "keep watching `CONTROL_AE_STATE` on subsequent frames until it
     * converges" handshake — for that one capture to complete. Good enough to
     * fire the flash at roughly the right moment; not a guarantee of a
     * correctly metered exposure the way a full precapture state machine
     * would be. A no-op when [Camera.flash] is [CameraApp.Flash.Off] or there
     * is nothing but the still reader to target it at.
     */
    private suspend fun runPrecaptureIfNeeded(cam: Camera) {
        if (cam.flash == CameraApp.Flash.Off) return
        val device = cam.device ?: return
        val session = cam.captureSession ?: return
        val targets = cam.activeOutputs.filter { it.kind != OutputKind.Still }
        if (targets.isEmpty()) return
        val builder = runCatching { device.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW) }.getOrNull() ?: return
        targets.forEach { builder.addTarget(it.surface) }
        applyLiveParams(builder, cam)
        builder.set(CaptureRequest.CONTROL_AE_PRECAPTURE_TRIGGER, CaptureRequest.CONTROL_AE_PRECAPTURE_TRIGGER_START)

        val done = CompletableDeferred<Unit>()
        val callback = object : CameraCaptureSession.CaptureCallback() {
            override fun onCaptureCompleted(session: CameraCaptureSession, request: CaptureRequest, result: TotalCaptureResult) {
                done.complete(Unit)
            }

            override fun onCaptureFailed(session: CameraCaptureSession, request: CaptureRequest, failure: CaptureFailure) {
                done.complete(Unit)
            }
        }
        val submitted = runCatching { session.capture(builder.build(), callback, cameraHandler) }.isSuccess
        if (!submitted) return
        withTimeoutOrNull(1_500L) { done.await() }
    }

    /** Publish a shot the *system* camera app took on our behalf — see
     *  [CameraApp.handoff] — so the tab's "last shot" row and thumbnail are
     *  the same whichever camera took it. Never touches [state]: an external
     *  capture has no [UiState.Recording]/[UiState.Preparing] of its own to
     *  become, so leaving whatever this screen's own state already was alone
     *  is the correct answer regardless of what it is. */
    fun publishExternalCapture(saved: Saved) {
        _lastSaved.value = saved
    }

    // ── Start / stop ─────────────────────────────────────────────────────

    fun start(context: Context, destination: CaptureDecisions.Destination) {
        rememberContext(context)
        val current = _state.value
        if (current !is UiState.Idle && current !is UiState.Failed) return
        val appCtx = context.applicationContext
        if (ContextCompat.checkSelfPermission(appCtx, Manifest.permission.CAMERA) !=
            PackageManager.PERMISSION_GRANTED ||
            ContextCompat.checkSelfPermission(appCtx, Manifest.permission.RECORD_AUDIO) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            _state.value = UiState.Failed(Reason.PermissionDenied)
            return
        }
        val lens = _lenses.value.find { it.cameraId == _selectedLensId.value }
            ?: CaptureDecisions.defaultLens(_lenses.value)
        if (lens == null) {
            _state.value = UiState.Failed(Reason.CameraUnavailable)
            return
        }
        _state.value = UiState.Preparing
        scope.launch(captureLane) {
            val failReason = runCatching { beginRecording(appCtx, destination, lens) }
                .getOrElse { e ->
                    Log.e(TAG, "capture setup crashed", e)
                    Reason.RecorderFailed
                }
            if (failReason != null) _state.value = UiState.Failed(failReason)
        }
    }

    fun stop(context: Context) {
        scope.launch(captureLane) {
            if (recording == null) return@launch
            finishRecording(success = true, failReason = null)
        }
    }

    private fun stopForReason(reason: CaptureDecisions.StopReason) {
        scope.launch(captureLane) {
            if (recording == null) return@launch
            finishRecording(success = true, failReason = mapStopReason(reason))
        }
    }

    private fun mapStopReason(reason: CaptureDecisions.StopReason): Reason = when (reason) {
        CaptureDecisions.StopReason.BatteryCritical -> Reason.StoppedBattery
        CaptureDecisions.StopReason.StorageLow -> Reason.StoppedStorage
        CaptureDecisions.StopReason.ThermalCritical -> Reason.StoppedThermal
    }

    // ── Recording setup ──────────────────────────────────────────────────

    /**
     * Runs on [captureLane]. Returns `null` on success, or the [Reason] the
     * caller should publish as [UiState.Failed] — never throws, every step is
     * wrapped so a mid-setup failure tears down whatever it opened for this
     * attempt (the recorder, the segment file) rather than leaking it. A
     * [Camera] already open for a live preview on this exact lens is reused
     * rather than closed and reopened, and every failure path below leaves it
     * exactly as it was — a live viewfinder must survive a failed "start
     * recording" attempt just as much as it must survive the recording that
     * did start.
     */
    private suspend fun beginRecording(
        context: Context,
        destination: CaptureDecisions.Destination,
        lens: CaptureDecisions.Lens,
    ): Reason? {
        val rotationDeg = currentRotationDeg(context)
        val startedAtEpochMs = System.currentTimeMillis()
        val utcOffsetMinutes = TimeZone.getDefault().getOffset(startedAtEpochMs) / 60_000
        val baseName = CaptureDecisions.baseName(startedAtEpochMs, utcOffsetMinutes)

        var cam = camera
        if (cam == null || cam.lens.cameraId != lens.cameraId) {
            cam?.let { closeCameraLocked(it) }
            cam = openCameraLocked(context, lens) ?: return Reason.CameraUnavailable
            camera = cam
        }
        val device = cam.device ?: return Reason.CameraUnavailable

        val firstSegment = CaptureSink.open(context, destination, CaptureDecisions.segmentFileName(baseName, 0))
        if (firstSegment == null) {
            reconcileLocked() // the camera we just opened/reused is still wanted (or not) independent of this failure
            return Reason.NoStorage
        }

        val recorder = newRecorder(context)
        val prepared = runCatching {
            recorder.setAudioSource(MediaRecorder.AudioSource.MIC)
            recorder.setVideoSource(MediaRecorder.VideoSource.SURFACE)
            recorder.setOutputFormat(MediaRecorder.OutputFormat.MPEG_4)
            recorder.setVideoEncoder(MediaRecorder.VideoEncoder.H264)
            recorder.setAudioEncoder(MediaRecorder.AudioEncoder.AAC)
            recorder.setVideoSize(cam.config.width, cam.config.height)
            recorder.setVideoFrameRate(cam.config.frameRate)
            recorder.setVideoEncodingBitRate(cam.config.bitRate)
            recorder.setAudioChannels(cam.config.audioChannels.coerceIn(1, 2))
            recorder.setAudioSamplingRate(cam.config.audioSampleRate)
            recorder.setAudioEncodingBitRate(cam.config.audioBitRate)
            recorder.setOrientationHint(CaptureDecisions.orientationHint(lens.sensorOrientation, rotationDeg, lens.facing))
            recorder.setMaxFileSize(CaptureDecisions.MAX_SEGMENT_BYTES)
            recorder.setMaxDuration(CaptureDecisions.MAX_SEGMENT_MS.toInt())
            firstSegment.setAsOutput(recorder)
            recorder.prepare()
        }
        if (prepared.isFailure) {
            Log.e(TAG, "MediaRecorder.prepare() failed", prepared.exceptionOrNull())
            runCatching { recorder.release() }
            firstSegment.finish(success = false)
            reconcileLocked()
            return Reason.RecorderFailed
        }

        val sessRecording = Recording(
            appContext = context,
            destination = destination,
            baseName = baseName,
            config = cam.config,
            startedElapsedRealtimeMs = SystemClock.elapsedRealtime(),
        )
        sessRecording.recorder = recorder
        sessRecording.current = firstSegment
        recording = sessRecording

        val desired = desiredOutputs()
        val configured = applyOutputs(cam, sessRecording, desired) ||
            (desired.preview && applyOutputs(cam, sessRecording, desired.copy(preview = false)))
        if (!configured) {
            recording = null
            runCatching { recorder.release() }
            firstSegment.finish(success = false)
            reconcileLocked()
            return Reason.CameraUnavailable
        }

        recorder.setOnInfoListener { _, what, _ -> onRecorderInfo(what) }

        val started = runCatching { recorder.start() }
        if (started.isFailure) {
            Log.e(TAG, "MediaRecorder.start() failed", started.exceptionOrNull())
            recording = null
            runCatching { recorder.release() }
            firstSegment.finish(success = false)
            // The session above is now configured with a recorder surface
            // that never actually started — drop back to a preview-only (or
            // closed) session rather than leaving it that way.
            reconcileLocked()
            return Reason.RecorderFailed
        }

        // A foreground service does not by itself keep the CPU out of
        // suspend — this is what actually does, for the length of the
        // recording. Released in finishRecording's cleanup, always.
        // Wrapped like every other step in this function, and for the same
        // reason: by this point the camera is open and the recorder is
        // running, so an exception escaping here would unwind past all of the
        // teardown below and strand a live CameraDevice with no service, no
        // wake lock and no way for the user to stop it. A recording that runs
        // without the lock is merely worse than one that runs with it; a
        // leaked camera is worse than both, and blocks every other app until
        // the phone reboots.
        sessRecording.wakeLock = runCatching {
            (context.getSystemService(Context.POWER_SERVICE) as? PowerManager)
                ?.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "comrade:capture")
                ?.apply { setReferenceCounted(false); acquire() }
        }.onFailure { Log.w(TAG, "could not hold a wake lock; recording anyway", it) }.getOrNull()

        armNextSegment(sessRecording)

        if (!disableServiceForTest) {
            runCatching { CaptureService.start(context, destination) }
        }

        _state.value = UiState.Recording(sessRecording.startedElapsedRealtimeMs, destination, lens, segment = 0)
        startGuardLoop(sessRecording)
        startTicker(sessRecording)
        return null
    }

    /**
     * Pre-open and arm the *next* segment's file immediately after recording
     * starts, and again immediately after each rollover, rather than waiting
     * for a `MAX_FILESIZE_APPROACHING` callback to open it reactively.
     * `setNextOutputFile` has to be called before the current file's limit is
     * actually hit or `MediaRecorder` simply stops instead of rolling over —
     * pre-arming removes that race entirely rather than trying to win it.
     */
    private fun armNextSegment(sess: Recording) {
        val recorder = sess.recorder ?: return
        val nextIndex = sess.segmentIndex + 1
        val name = CaptureDecisions.segmentFileName(sess.baseName, nextIndex)
        val next = CaptureSink.open(sess.appContext, sess.destination, name)
        if (next == null) {
            // Could not open a target for the next segment (storage, a
            // refused MediaStore insert). Not fatal on its own: the guard
            // loop's free-space check is what actually ends the ride if this
            // is a genuine "disk is full" — this just means the *current*
            // file keeps growing until MediaRecorder itself stops on the
            // size/duration ceiling with nowhere to roll to.
            Log.w(TAG, "could not arm the next capture segment; continuing on the current one")
            return
        }
        val armed = runCatching { next.setAsNextOutput(recorder) }
        if (armed.isFailure) {
            Log.w(TAG, "setNextOutputFile failed", armed.exceptionOrNull())
            next.finish(success = false)
            return
        }
        sess.armedNext = next
    }

    private fun onRecorderInfo(what: Int) {
        when (what) {
            MediaRecorder.MEDIA_RECORDER_INFO_NEXT_OUTPUT_FILE_STARTED -> {
                scope.launch(captureLane) { promoteSegment() }
            }
            MediaRecorder.MEDIA_RECORDER_INFO_MAX_FILESIZE_REACHED,
            MediaRecorder.MEDIA_RECORDER_INFO_MAX_DURATION_REACHED -> {
                scope.launch(captureLane) {
                    val sess = recording ?: return@launch
                    if (sess.armedNext == null) {
                        // setNextOutputFile was never armed — MediaRecorder
                        // has now stopped writing on its own with nowhere to
                        // roll to. Save what is there rather than lose it.
                        finishRecording(success = true, failReason = Reason.NoStorage)
                    }
                }
            }
            else -> {}
        }
    }

    private fun promoteSegment() {
        val sess = recording ?: return
        val lens = camera?.lens ?: return
        val next = sess.armedNext ?: return
        sess.current?.finish(success = true)
        sess.current = next
        sess.armedNext = null
        sess.segmentIndex += 1
        armNextSegment(sess)
        _state.value = UiState.Recording(sess.startedElapsedRealtimeMs, sess.destination, lens, sess.segmentIndex)
    }

    /**
     * The one path every ended recording goes through — user [stop], an
     * auto-stop guard trip, a lost camera, or a setup failure partway through
     * [beginRecording] that already published a [recording]. Always tears
     * down in the same order and always resolves [lastSaved]/[state] exactly
     * once.
     *
     * The order matters in a way it did not when this always closed the
     * whole [CameraCaptureSession]/[CameraDevice] outright: [reconcileLocked]
     * runs — dropping the recorder surface from the live session, or closing
     * the camera entirely if nothing else wants it — *before* the
     * [MediaRecorder] is released, not after. Releasing first would leave the
     * still-live session's repeating request pointed at a `Surface`
     * `MediaRecorder` had already invalidated for however long the
     * reconfiguration took. The forced [closeCameraLocked] afterwards is the
     * backstop for the rare case [reconcileLocked] could not rebuild a
     * recorder-free session at all — never leave a live session referencing a
     * `Surface` about to be released regardless of why the reconfiguration
     * failed.
     */
    private suspend fun finishRecording(success: Boolean, failReason: Reason?) {
        val sess = recording ?: run {
            if (failReason != null) _state.value = UiState.Failed(failReason)
            return
        }
        recording = null
        sess.guardJob?.cancel()
        sess.tickerJob?.cancel()

        var stoppedCleanly = success
        if (success) {
            val stopped = runCatching { sess.recorder?.stop() }
            if (stopped.isFailure) stoppedCleanly = false
        }

        reconcileLocked()
        camera?.let { cam ->
            if (cam.activeOutputs.any { it.kind == OutputKind.Recorder }) {
                Log.w(TAG, "could not drop the recorder surface from the live session; closing the camera instead")
                closeCameraLocked(cam)
            }
        }

        runCatching { sess.recorder?.release() }
        sess.wakeLock?.let { if (it.isHeld) runCatching { it.release() } }

        val savedUri = sess.current?.finish(success = stoppedCleanly)
        sess.armedNext?.finish(success = false) // never started writing — discard

        _elapsedMs.value = 0
        if (stoppedCleanly && sess.current != null) {
            _lastSaved.value = Saved(
                uri = savedUri?.toString(),
                displayName = CaptureDecisions.segmentFileName(sess.baseName, sess.segmentIndex),
                destination = sess.destination,
                kind = Kind.Video,
            )
        }
        _state.value = failReason?.let { UiState.Failed(it) } ?: UiState.Idle
        if (!disableServiceForTest) {
            runCatching { CaptureService.stop(sess.appContext) }
        }
    }

    // ── Reconfiguration ──────────────────────────────────────────────────

    private fun desiredOutputs(): DesiredOutputs {
        val wantsPreview = CaptureDecisions.previewAttached(screenOn, uiVisible) && preview != null
        // A still reader is only ever added while nothing is recording — see
        // takePhoto's doc comment for why a mid-recording still is refused
        // rather than reconfigured for.
        val wantsReader = recording == null && _mode.value == CameraApp.Mode.Photo
        return DesiredOutputs(wantsPreview, wantsReader)
    }

    private fun outputsSatisfy(cam: Camera, desired: DesiredOutputs, sess: Recording?): Boolean {
        val hasPreview = cam.activeOutputs.any { it.kind == OutputKind.Preview }
        val hasReader = cam.activeOutputs.any { it.kind == OutputKind.Still }
        val hasRecorder = cam.activeOutputs.any { it.kind == OutputKind.Recorder }
        return hasPreview == desired.preview && hasReader == desired.stillReader && hasRecorder == (sess != null)
    }

    /**
     * The single place that decides whether [camera] should be open at all,
     * on which lens, and with which outputs — called after every state
     * change that could move any of those answers (preview attach/detach, UI
     * visibility, screen on/off, mode switch, a lens change already closed
     * the old device for). A no-op when everything already matches.
     */
    private suspend fun reconcileLocked() {
        val sess = recording
        val shouldBeOpen = sess != null || (uiVisible && preview != null)
        var cam = camera

        if (!shouldBeOpen) {
            if (cam != null && sess == null) closeCameraLocked(cam)
            return
        }

        if (cam == null) {
            if (sess != null) return // beginRecording always opens its own camera before publishing `recording`
            val ctx = appContext ?: return
            val lens = _lenses.value.find { it.cameraId == _selectedLensId.value }
                ?: CaptureDecisions.defaultLens(_lenses.value)
                ?: return
            cam = openCameraLocked(ctx, lens) ?: return
            camera = cam
        }

        val current = cam
        val desired = desiredOutputs()
        if (outputsSatisfy(current, desired, sess)) return

        if (sess != null) {
            // The old session is deliberately NOT closed up front. Camera2
            // replaces the previous session when a new one is created, so
            // closing first buys nothing — and it used to cost everything:
            // if the new session then failed to configure, this returned with
            // `captureSession` still pointing at the session it had just
            // closed, so no frames reached the encoder for the rest of the
            // ride while the timer and the notification carried on as if they
            // did. On a helmet mount nobody finds out until they get home.
            if (applyOutputs(current, sess, desired)) return
            if (desired.preview && applyOutputs(current, sess, desired.copy(preview = false))) {
                Log.w(TAG, "preview could not be reattached; recording continues without a viewfinder")
                return
            }
            Log.w(TAG, "capture session lost and could not be rebuilt; ending the recording")
            finishRecording(success = true, failReason = Reason.StoppedCameraLost)
        } else {
            if (applyOutputs(current, null, desired)) return
            if (desired.stillReader && applyOutputs(current, null, desired.copy(stillReader = false))) {
                Log.w(TAG, "still output could not be configured; the viewfinder stays live without photo capture")
                return
            }
            if (desired.preview && applyOutputs(current, null, desired.copy(preview = false, stillReader = false))) {
                Log.w(TAG, "preview could not be configured")
                return
            }
            Log.w(TAG, "camera session could not be configured; leaving it for the next attempt")
        }
    }

    /**
     * Swap the live capture session for one carrying [desired]'s output set
     * (plus [sess]'s recorder surface, if any), and answer whether that
     * actually worked. Every failure path leaves [cam] untouched and disposes
     * whatever it built, so a caller may retry a smaller output set
     * immediately afterwards. A previously-attached preview surface is
     * released only once the replacement is configured and repeating —
     * releasing it earlier is releasing a surface the still-current session
     * is drawing into.
     */
    private suspend fun applyOutputs(cam: Camera, sess: Recording?, desired: DesiredOutputs): Boolean {
        val device = cam.device ?: return false
        val built = buildOutputs(cam, sess, desired)
        if (desired.preview && built.none { it.kind == OutputKind.Preview }) {
            releasePreviewSurfaces(built)
            return false
        }
        if (desired.stillReader && built.none { it.kind == OutputKind.Still }) {
            releasePreviewSurfaces(built)
            return false
        }
        val newSession = createSession(device, built.map { it.surface })
        if (newSession == null) {
            releasePreviewSurfaces(built)
            return false
        }
        val template = if (sess != null) CameraDevice.TEMPLATE_RECORD else CameraDevice.TEMPLATE_PREVIEW
        val requestOk = runCatching {
            val builder = device.createCaptureRequest(template)
            built.forEach { builder.addTarget(it.surface) }
            applyLiveParams(builder, cam)
            newSession.setRepeatingRequest(builder.build(), null, cameraHandler)
        }
        if (requestOk.isFailure) {
            Log.w(TAG, "setRepeatingRequest failed after reconfiguration", requestOk.exceptionOrNull())
            runCatching { newSession.close() }
            releasePreviewSurfaces(built)
            return false
        }
        val previous = cam.activeOutputs
        cam.captureSession = newSession
        cam.activeOutputs = built
        releasePreviewSurfaces(previous)
        return true
    }

    private fun releasePreviewSurfaces(outputs: List<Output>) {
        outputs.filter { it.kind == OutputKind.Preview }.forEach { runCatching { it.surface.release() } }
    }

    private fun buildOutputs(cam: Camera, sess: Recording?, desired: DesiredOutputs): List<Output> {
        val outputs = mutableListOf<Output>()
        sess?.recorder?.surface?.let { outputs += Output(it, OutputKind.Recorder) }
        if (desired.preview) {
            val target = preview
            if (target != null) {
                val built = runCatching {
                    val size = choosePreviewSize(cam.chars, cam.config.width, cam.config.height)
                    target.texture.setDefaultBufferSize(size.width, size.height)
                    Output(Surface(target.texture), OutputKind.Preview)
                }.getOrNull()
                if (built != null) outputs += built
            }
        }
        if (desired.stillReader) {
            val reader = ensureStillReader(cam)
            if (reader != null) outputs += Output(reader.surface, OutputKind.Still)
        }
        return outputs
    }

    /** Created once per [Camera] and kept for its whole life — see the class
     *  doc — rather than rebuilt on every reconfiguration; only whether it is
     *  part of the *current* output set changes. */
    private fun ensureStillReader(cam: Camera): ImageReader? {
        cam.imageReader?.let { return it }
        val size = chooseStillSize(cam.chars, cam.config.width, cam.config.height) ?: return null
        val reader = runCatching { ImageReader.newInstance(size.width, size.height, ImageFormat.JPEG, 2) }.getOrNull()
            ?: return null
        cam.imageReader = reader
        return reader
    }

    private fun choosePreviewSize(chars: CameraCharacteristics, targetWidth: Int, targetHeight: Int): Size {
        val fallback = Size(targetWidth, targetHeight)
        val map = chars.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP) ?: return fallback
        val sizes = map.getOutputSizes(SurfaceTexture::class.java)
        if (sizes.isNullOrEmpty()) return fallback
        val targetRatio = targetWidth.toDouble() / targetHeight
        return sizes.minByOrNull { size -> abs(size.width.toDouble() / size.height - targetRatio) } ?: fallback
    }

    /** The largest JPEG size at (or closest to) the video config's aspect
     *  ratio — "the selected aspect ratio" a photo should match, so a still
     *  taken next to a video is never a visibly different shape. */
    private fun chooseStillSize(chars: CameraCharacteristics, targetWidth: Int, targetHeight: Int): Size? {
        val map = chars.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP) ?: return null
        val sizes = map.getOutputSizes(ImageFormat.JPEG)
        if (sizes.isNullOrEmpty()) return null
        val targetRatio = targetWidth.toDouble() / targetHeight
        val matching = sizes.filter { abs(it.width.toDouble() / it.height - targetRatio) < 0.02 }
        val pool = matching.ifEmpty { sizes.toList() }
        return pool.maxByOrNull { it.width.toLong() * it.height }
    }

    // ── Live request parameters (zoom / flash / metering) ────────────────

    private fun applyLiveParams(builder: CaptureRequest.Builder, cam: Camera) {
        applyZoom(builder, cam)
        applyFlash(builder, cam)
        applyMetering(builder, cam)
    }

    private fun applyZoom(builder: CaptureRequest.Builder, cam: Camera) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            builder.set(CaptureRequest.CONTROL_ZOOM_RATIO, cam.zoomRatio)
        } else {
            val active = cam.chars.get(CameraCharacteristics.SENSOR_INFO_ACTIVE_ARRAY_SIZE) ?: return
            builder.set(CaptureRequest.SCALER_CROP_REGION, cropRegionForZoom(active, cam.zoomRatio))
        }
    }

    private fun cropRegionForZoom(active: Rect, zoomRatio: Float): Rect {
        val zoom = zoomRatio.coerceAtLeast(1f)
        val croppedWidth = (active.width() / zoom).roundToInt().coerceIn(1, active.width())
        val croppedHeight = (active.height() / zoom).roundToInt().coerceIn(1, active.height())
        val left = active.left + (active.width() - croppedWidth) / 2
        val top = active.top + (active.height() - croppedHeight) / 2
        return Rect(left, top, left + croppedWidth, top + croppedHeight)
    }

    private fun applyFlash(builder: CaptureRequest.Builder, cam: Camera) {
        if (!cam.hasFlashUnit) return
        when (_mode.value) {
            CameraApp.Mode.Video -> builder.set(
                CaptureRequest.FLASH_MODE,
                if (cam.flash == CameraApp.Flash.On) CaptureRequest.FLASH_MODE_TORCH else CaptureRequest.FLASH_MODE_OFF,
            )
            CameraApp.Mode.Photo -> builder.set(
                CaptureRequest.CONTROL_AE_MODE,
                when (cam.flash) {
                    CameraApp.Flash.Off -> CaptureRequest.CONTROL_AE_MODE_ON
                    CameraApp.Flash.Auto -> CaptureRequest.CONTROL_AE_MODE_ON_AUTO_FLASH
                    CameraApp.Flash.On -> CaptureRequest.CONTROL_AE_MODE_ON_ALWAYS_FLASH
                },
            )
        }
    }

    private fun applyMetering(builder: CaptureRequest.Builder, cam: Camera) {
        val rect = cam.meteringRect ?: return
        val maxAf = cam.chars.get(CameraCharacteristics.CONTROL_MAX_REGIONS_AF) ?: 0
        val maxAe = cam.chars.get(CameraCharacteristics.CONTROL_MAX_REGIONS_AE) ?: 0
        if (maxAf <= 0 && maxAe <= 0) return
        val region = MeteringRectangle(
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            MeteringRectangle.METERING_WEIGHT_MAX,
        )
        if (maxAf > 0) builder.set(CaptureRequest.CONTROL_AF_REGIONS, arrayOf(region))
        if (maxAe > 0) builder.set(CaptureRequest.CONTROL_AE_REGIONS, arrayOf(region))
    }

    /** Rebuild and resubmit the repeating request against [Camera.activeOutputs]
     *  as they already are — used by [setZoom]/[setFlash]/[focusAt], none of
     *  which change *which* outputs are live, only the parameters on the
     *  request already targeting them. */
    private fun reapplyRepeatingRequest(cam: Camera) {
        val device = cam.device ?: return
        val session = cam.captureSession ?: return
        val outputs = cam.activeOutputs
        if (outputs.isEmpty()) return
        val template = if (recording != null) CameraDevice.TEMPLATE_RECORD else CameraDevice.TEMPLATE_PREVIEW
        val builder = runCatching { device.createCaptureRequest(template) }.getOrNull() ?: return
        outputs.forEach { builder.addTarget(it.surface) }
        applyLiveParams(builder, cam)
        runCatching { session.setRepeatingRequest(builder.build(), null, cameraHandler) }
    }

    // ── Camera2 plumbing ─────────────────────────────────────────────────

    /**
     * Opens [lens] and seeds its live parameters from the current
     * zoom/flash flows (clamped to what this lens actually reports), so
     * switching lenses does not silently reset them back to a default.
     */
    private suspend fun openCameraLocked(context: Context, lens: CaptureDecisions.Lens): Camera? {
        val manager = context.getSystemService(CameraManager::class.java) ?: return null
        val chars = runCatching { manager.getCameraCharacteristics(lens.cameraId) }.getOrNull() ?: return null
        val config = resolveVideoConfig(lens.cameraId)
        val device = openCameraDevice(context, lens.cameraId) ?: return null
        val cam = Camera(lens, chars, config)
        cam.device = device
        cam.zoomRatio = CameraApp.clampZoom(_zoomRatio.value, cam.zoomRange.start, cam.zoomRange.endInclusive)
        cam.flash = if (cam.hasFlashUnit) _flash.value else CameraApp.Flash.Off
        _zoomRange.value = cam.zoomRange
        _zoomRatio.value = cam.zoomRatio
        _hasFlashUnit.value = cam.hasFlashUnit
        _flash.value = cam.flash
        return cam
    }

    private fun closeCameraLocked(cam: Camera) {
        runCatching { cam.captureSession?.close() }
        runCatching { cam.device?.close() }
        releasePreviewSurfaces(cam.activeOutputs)
        cam.imageReader?.let { runCatching { it.close() } }
        cam.activeOutputs = emptyList()
        cam.captureSession = null
        cam.device = null
        cam.imageReader = null
        if (camera === cam) camera = null
    }

    /**
     * Opens [cameraId] and awaits the result off [cameraHandler]'s callback.
     * The same [CameraDevice.StateCallback] instance keeps watching the
     * device for the rest of its life — a later `onDisconnected`/`onError`
     * (another app takes the camera, a USB lens unplugged) routes into
     * [handleCameraLost] rather than needing a second registration.
     */
    private suspend fun openCameraDevice(context: Context, cameraId: String): CameraDevice? {
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            return null
        }
        val manager = context.getSystemService(CameraManager::class.java) ?: return null
        val result = CompletableDeferred<CameraDevice?>()
        val callback = object : CameraDevice.StateCallback() {
            override fun onOpened(device: CameraDevice) {
                result.complete(device)
            }

            override fun onDisconnected(device: CameraDevice) {
                device.close()
                result.complete(null)
                handleCameraLost(device)
            }

            override fun onError(device: CameraDevice, error: Int) {
                device.close()
                result.complete(null)
                handleCameraLost(device)
            }
        }
        val opened = runCatching { manager.openCamera(cameraId, callback, cameraHandler) }
        if (opened.isFailure) return null
        return result.await()
    }

    private fun handleCameraLost(lostDevice: CameraDevice) {
        scope.launch(captureLane) {
            val cam = camera ?: return@launch
            if (cam.device !== lostDevice) return@launch
            val sess = recording
            cam.captureSession = null
            cam.activeOutputs = emptyList()
            cam.device = null
            cam.imageReader?.let { runCatching { it.close() } }
            cam.imageReader = null
            camera = null
            if (sess != null) {
                finishRecording(success = true, failReason = Reason.CameraUnavailable)
            } else {
                // Nothing was writing to disk, so there is nothing to
                // publish — just try to get the preview back if anything
                // still wants it. No retry loop beyond this one attempt: if
                // whatever took the lens still holds it, this leaves the
                // camera closed until the next attachPreview/setUiVisible/
                // mode change asks for it again.
                reconcileLocked()
            }
        }
    }

    private suspend fun createSession(device: CameraDevice, surfaces: List<Surface>): CameraCaptureSession? {
        val result = CompletableDeferred<CameraCaptureSession?>()
        val callback = object : CameraCaptureSession.StateCallback() {
            override fun onConfigured(session: CameraCaptureSession) {
                result.complete(session)
            }

            override fun onConfigureFailed(session: CameraCaptureSession) {
                result.complete(null)
            }
        }
        // The `SessionConfiguration` overload is API 28+; this one has been
        // available since Camera2 shipped (API 21) and covers this app's
        // minSdk without a version branch.
        @Suppress("DEPRECATION")
        val attempted = runCatching { device.createCaptureSession(surfaces, callback, cameraHandler) }
        if (attempted.isFailure) return null
        return result.await()
    }

    private fun newRecorder(context: Context): MediaRecorder =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            MediaRecorder(context)
        } else {
            @Suppress("DEPRECATION")
            MediaRecorder()
        }

    /**
     * The highest quality [CamcorderProfile]/`EncoderProfiles` reports for
     * [cameraId], H.264 + AAC. `EncoderProfiles.getAll` (API 31+) takes the
     * Camera2 string id directly; below that, the legacy `CamcorderProfile`
     * API only understands an integer id, so it is tried only when [cameraId]
     * actually parses as one — an external/USB lens's id may not.
     * [FALLBACK_CONFIG] is the answer when nothing above resolves: a
     * `CamcorderProfile` is not guaranteed to exist for every camera id, only
     * for id `"0"`.
     */
    private fun resolveVideoConfig(cameraId: String): VideoConfig {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            val profiles = runCatching { CamcorderProfile.getAll(cameraId, CamcorderProfile.QUALITY_HIGH) }.getOrNull()
            val video = profiles?.videoProfiles?.firstOrNull()
            if (video != null) {
                val audio = profiles.audioProfiles.firstOrNull()
                return VideoConfig(
                    width = video.width,
                    height = video.height,
                    frameRate = video.frameRate,
                    bitRate = video.bitrate,
                    audioBitRate = audio?.bitrate ?: FALLBACK_CONFIG.audioBitRate,
                    audioSampleRate = audio?.sampleRate ?: FALLBACK_CONFIG.audioSampleRate,
                    audioChannels = audio?.channels ?: FALLBACK_CONFIG.audioChannels,
                )
            }
        }
        val legacyId = cameraId.toIntOrNull()
        if (legacyId != null) {
            for (quality in LEGACY_QUALITY_FALLBACK) {
                val has = runCatching { CamcorderProfile.hasProfile(legacyId, quality) }.getOrDefault(false)
                if (has) {
                    val p = CamcorderProfile.get(legacyId, quality)
                    return VideoConfig(
                        width = p.videoFrameWidth,
                        height = p.videoFrameHeight,
                        frameRate = p.videoFrameRate,
                        bitRate = p.videoBitRate,
                        audioBitRate = p.audioBitRate,
                        audioSampleRate = p.audioSampleRate,
                        audioChannels = p.audioChannels,
                    )
                }
            }
        }
        return FALLBACK_CONFIG
    }

    private val LEGACY_QUALITY_FALLBACK = listOf(
        CamcorderProfile.QUALITY_HIGH,
        CamcorderProfile.QUALITY_1080P,
        CamcorderProfile.QUALITY_720P,
        CamcorderProfile.QUALITY_480P,
    )

    private val FALLBACK_CONFIG = VideoConfig(
        width = 1280,
        height = 720,
        frameRate = 30,
        bitRate = 8_000_000,
        audioBitRate = 128_000,
        audioSampleRate = 44_100,
        audioChannels = 1,
    )

    private fun currentRotationDeg(context: Context): Int {
        val rotation = runCatching {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                context.display?.rotation
            } else {
                @Suppress("DEPRECATION")
                (context.getSystemService(Context.WINDOW_SERVICE) as? WindowManager)?.defaultDisplay?.rotation
            }
        }.getOrNull() ?: Surface.ROTATION_0
        return when (rotation) {
            Surface.ROTATION_90 -> 90
            Surface.ROTATION_180 -> 180
            Surface.ROTATION_270 -> 270
            else -> 0
        }
    }

    private fun zoomRangeFor(chars: CameraCharacteristics): ClosedFloatingPointRange<Float> {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            val range = chars.get(CameraCharacteristics.CONTROL_ZOOM_RATIO_RANGE)
            if (range != null) return range.lower..range.upper
        }
        val maxDigital = chars.get(CameraCharacteristics.SCALER_AVAILABLE_MAX_DIGITAL_ZOOM) ?: 1f
        return 1f..maxDigital.coerceAtLeast(1f)
    }

    // ── Guards: battery, storage, thermal ────────────────────────────────

    private const val GUARD_INTERVAL_MS = 30_000L
    private const val TICKER_INTERVAL_MS = 500L

    /** 30s: frequent enough that a genuinely critical state gets caught
     *  promptly, infrequent enough that the guard itself is not a battery
     *  cost the feature has to answer for. */
    private fun startGuardLoop(sess: Recording) {
        sess.guardJob = scope.launch {
            while (isActive) {
                delay(GUARD_INTERVAL_MS)
                val context = sess.appContext
                val reason = CaptureDecisions.autoStop(
                    batteryPercent(context),
                    isCharging(context),
                    freeBytes(context),
                    thermalStatus(context),
                )
                if (reason != null) {
                    stopForReason(reason)
                    break
                }
            }
        }
    }

    private fun startTicker(sess: Recording) {
        sess.tickerJob = scope.launch {
            while (isActive) {
                _elapsedMs.value = (SystemClock.elapsedRealtime() - sess.startedElapsedRealtimeMs).coerceAtLeast(0L)
                delay(TICKER_INTERVAL_MS)
            }
        }
    }

    /** A read failure defaults open (as if charged, plugged in, thermally
     *  fine) rather than tripping an auto-stop on a transient API hiccup —
     *  the guard loop tries again in [GUARD_INTERVAL_MS] regardless. */
    private fun batteryPercent(context: Context): Int {
        val bm = context.getSystemService(BatteryManager::class.java) ?: return 100
        return runCatching { bm.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY) }.getOrDefault(100)
    }

    private fun isCharging(context: Context): Boolean {
        val bm = context.getSystemService(BatteryManager::class.java) ?: return true
        return runCatching { bm.isCharging }.getOrDefault(true)
    }

    /** Free space on the volume this recording is actually writing to,
     *  read via the app's own external-files directory — no storage
     *  permission needed, and it shares a partition with `DCIM/Camera` on
     *  every device this matters for. */
    private fun freeBytes(context: Context): Long {
        val dir = context.getExternalFilesDir(null) ?: context.filesDir
        return runCatching { StatFs(dir.path).availableBytes }.getOrDefault(Long.MAX_VALUE)
    }

    private fun thermalStatus(context: Context): Int {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return 0
        val pm = context.getSystemService(PowerManager::class.java) ?: return 0
        return runCatching { pm.currentThermalStatus }.getOrDefault(0)
    }
}
