package mullu.comrade.capture

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.graphics.SurfaceTexture
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.media.CamcorderProfile
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
import java.util.TimeZone
import kotlin.math.abs

/**
 * The Android side of the helmet-cam feature: owns the camera, the recorder
 * and every decision about them, and is what starts and stops [CaptureService]
 * — the same split [mullu.comrade.together.TogetherManager] and
 * [mullu.comrade.together.TogetherService] already use. [CaptureService] holds
 * only the foreground-service contract and the notification; every decision
 * about *whether* to roll, reconfigure or stop lives here or, wherever it can,
 * in the pure [CaptureDecisions].
 *
 * ## Camera2 + MediaRecorder, not CameraX
 *
 * Two reasons. First, `CameraManager.getCameraIdList()` enumerates every
 * *physical* camera on the device — ultrawide, main, tele, front, an external
 * USB lens — which is the "switch between whatever cameras this phone has"
 * requirement; `androidx.camera.core.CameraSelector` mostly only exposes
 * front/back. Second, CameraX would be a new dependency, and in this repo
 * that means adding it to *two* Gradle files (`android/app/build.gradle.kts`
 * and `app/android/app/build.gradle.kts`, which stages every non-Compose
 * `.kt` file here into the Flutter app) or breaking the Flutter Android lane.
 * Framework classes cost neither.
 *
 * ## Threading
 *
 * `Camera2`'s device/session callbacks land on [cameraHandler], a dedicated
 * [HandlerThread] — never the main thread and never a coroutine dispatcher
 * with no `Looper`, which `CameraManager.openCamera`/`createCaptureSession`
 * both require. Every operation that mutates the live [session] (opening,
 * reconfiguring, rolling a segment, tearing down) is launched on
 * [captureLane], `Dispatchers.IO` restricted to one thread at a time — the
 * same pattern [mullu.comrade.call.CallManager.webRtcLane] uses to keep two
 * camera/recorder operations from ever interleaving, without holding a lock
 * across a suspending call. [session] itself is `@Volatile` so the ticker and
 * guard loop (running on [scope]'s default dispatcher) see a freshly
 * published reference instead of a stale one cached in a register.
 *
 * ## What "screen off" costs
 *
 * See [reconfigureOutputs]: Camera2 cannot add or remove an output from a
 * live [CameraCaptureSession], so attaching or detaching the preview is a
 * fresh `createCaptureSession` on the same open [CameraDevice]. The
 * [MediaRecorder] is never stopped across that — the file keeps growing and
 * its timestamps absorb the gap — but frames genuinely stop arriving for the
 * length of the reconfiguration. A few are lost at every screen on/off
 * transition. That is the real cost of the feature, not an incidental one,
 * and no comment here should claim otherwise.
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
    }

    data class Saved(val uri: String?, val displayName: String, val destination: CaptureDecisions.Destination)

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

    /** See [mullu.comrade.call.CallManager.disableCallServiceForTest] — same
     *  seam, same reason: skip only the [CaptureService] start/stop. Never
     *  touched by any production code path. */
    @Volatile
    var disableServiceForTest: Boolean = false

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    /** Serializes every operation that touches the live camera/recorder —
     *  see the class doc's Threading section. */
    private val captureLane = Dispatchers.IO.limitedParallelism(1)

    private val cameraThread = HandlerThread("CaptureCamera").apply { start() }
    private val cameraHandler = Handler(cameraThread.looper)

    @Volatile
    private var session: RecordingSession? = null

    @Volatile
    private var preview: PreviewTarget? = null

    /** A recording always starts with the screen on — the user just tapped
     *  Record — so this is the honest initial value rather than a guess. */
    @Volatile
    private var screenOn: Boolean = true

    @Volatile
    private var uiVisible: Boolean = true

    private data class PreviewTarget(val texture: SurfaceTexture, val width: Int, val height: Int)

    private data class Output(val surface: Surface, val isPreview: Boolean)

    private data class VideoConfig(
        val width: Int,
        val height: Int,
        val frameRate: Int,
        val bitRate: Int,
        val audioBitRate: Int,
        val audioSampleRate: Int,
        val audioChannels: Int,
    )

    private class RecordingSession(
        val appContext: Context,
        val destination: CaptureDecisions.Destination,
        val lens: CaptureDecisions.Lens,
        val baseName: String,
        val config: VideoConfig,
        val startedElapsedRealtimeMs: Long,
    ) {
        var device: CameraDevice? = null
        var captureSession: CameraCaptureSession? = null
        var recorder: MediaRecorder? = null
        var current: CaptureSink.Segment? = null
        var armedNext: CaptureSink.Segment? = null
        var segmentIndex: Int = 0
        var activeOutputs: List<Output> = emptyList()
        var wakeLock: PowerManager.WakeLock? = null
        var guardJob: Job? = null
        var tickerJob: Job? = null
    }

    // ── Lenses ───────────────────────────────────────────────────────────

    fun refreshLenses(context: Context) {
        val appContext = context.applicationContext
        scope.launch(Dispatchers.Default) {
            val discovered = mutableListOf<CaptureDecisions.Lens>()
            val manager = appContext.getSystemService(CameraManager::class.java)
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
        }
    }

    fun selectLens(cameraId: String) {
        if (_lenses.value.any { it.cameraId == cameraId }) {
            _selectedLensId.value = cameraId
        }
    }

    /**
     * Cycle to the next lens. A no-op while [state] is [UiState.Recording]:
     * Camera2 cannot move a live session to a different physical camera
     * without closing the [CameraDevice], and closing it finalises the MP4
     * mid-shot. The UI disables this control during a recording; this being a
     * silent no-op rather than an error is what makes that safe if a stray
     * tap gets through anyway.
     */
    fun switchLens(context: Context) {
        if (_state.value is UiState.Recording) return
        val next = CaptureDecisions.nextLens(_lenses.value, _selectedLensId.value) ?: return
        _selectedLensId.value = next.cameraId
    }

    // ── Preview / visibility / screen state ─────────────────────────────

    fun attachPreview(context: Context, texture: SurfaceTexture?, width: Int, height: Int) {
        preview = texture?.let { PreviewTarget(it, width, height) }
        reconfigureOutputs()
    }

    fun setUiVisible(context: Context, visible: Boolean) {
        uiVisible = visible
        reconfigureOutputs()
    }

    /** Called by [CaptureService]'s `ACTION_SCREEN_OFF`/`ACTION_SCREEN_ON`
     *  receiver — these cannot be declared in the manifest, so the service
     *  registers a dynamic one and routes the result in here. */
    fun onScreenStateChanged(screenOn: Boolean) {
        this.screenOn = screenOn
        reconfigureOutputs()
    }

    // ── Start / stop ─────────────────────────────────────────────────────

    fun start(context: Context, destination: CaptureDecisions.Destination) {
        val current = _state.value
        if (current !is UiState.Idle && current !is UiState.Failed) return
        val appContext = context.applicationContext
        if (ContextCompat.checkSelfPermission(appContext, Manifest.permission.CAMERA) !=
            PackageManager.PERMISSION_GRANTED ||
            ContextCompat.checkSelfPermission(appContext, Manifest.permission.RECORD_AUDIO) !=
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
            val failReason = runCatching { beginRecording(appContext, destination, lens) }
                .getOrElse { e ->
                    Log.e(TAG, "capture setup crashed", e)
                    Reason.RecorderFailed
                }
            if (failReason != null) _state.value = UiState.Failed(failReason)
        }
    }

    fun stop(context: Context) {
        scope.launch(captureLane) {
            if (session == null) return@launch
            finishRecording(success = true, failReason = null)
        }
    }

    private fun stopForReason(reason: CaptureDecisions.StopReason) {
        scope.launch(captureLane) {
            if (session == null) return@launch
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
     * wrapped so a mid-setup failure tears down whatever it already opened
     * (camera, recorder, segment file) rather than leaking it.
     *
     * Order matters, and matches the platform's own requirement: sources,
     * then profile/encoders, then the output target, then `prepare()`, and
     * only once that succeeds does the capture session get created with
     * `recorder.surface` as one of its outputs.
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
        val config = resolveVideoConfig(lens.cameraId)

        val device = openCameraDevice(context, lens.cameraId) ?: return Reason.CameraUnavailable

        val firstSegment = CaptureSink.open(context, destination, CaptureDecisions.segmentFileName(baseName, 0))
        if (firstSegment == null) {
            runCatching { device.close() }
            return Reason.NoStorage
        }

        val recorder = newRecorder(context)
        val prepared = runCatching {
            recorder.setAudioSource(MediaRecorder.AudioSource.MIC)
            recorder.setVideoSource(MediaRecorder.VideoSource.SURFACE)
            recorder.setOutputFormat(MediaRecorder.OutputFormat.MPEG_4)
            recorder.setVideoEncoder(MediaRecorder.VideoEncoder.H264)
            recorder.setAudioEncoder(MediaRecorder.AudioEncoder.AAC)
            recorder.setVideoSize(config.width, config.height)
            recorder.setVideoFrameRate(config.frameRate)
            recorder.setVideoEncodingBitRate(config.bitRate)
            recorder.setAudioChannels(config.audioChannels.coerceIn(1, 2))
            recorder.setAudioSamplingRate(config.audioSampleRate)
            recorder.setAudioEncodingBitRate(config.audioBitRate)
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
            runCatching { device.close() }
            return Reason.RecorderFailed
        }

        val wantsPreview = CaptureDecisions.previewAttached(screenOn, uiVisible) && preview != null
        val outputs = buildOutputs(context, recorder.surface, lens, config, if (wantsPreview) preview else null)
        val captureSession = createSession(device, outputs.map { it.surface })
        if (captureSession == null) {
            runCatching { recorder.release() }
            firstSegment.finish(success = false)
            runCatching { device.close() }
            return Reason.CameraUnavailable
        }

        val sess = RecordingSession(
            appContext = context,
            destination = destination,
            lens = lens,
            baseName = baseName,
            config = config,
            startedElapsedRealtimeMs = SystemClock.elapsedRealtime(),
        )
        sess.device = device
        sess.captureSession = captureSession
        sess.recorder = recorder
        sess.current = firstSegment
        sess.activeOutputs = outputs
        session = sess

        val requestOk = runCatching {
            val builder = device.createCaptureRequest(CameraDevice.TEMPLATE_RECORD)
            outputs.forEach { builder.addTarget(it.surface) }
            captureSession.setRepeatingRequest(builder.build(), null, cameraHandler)
        }
        if (requestOk.isFailure) {
            Log.e(TAG, "setRepeatingRequest failed", requestOk.exceptionOrNull())
            session = null
            runCatching { captureSession.close() }
            runCatching { recorder.release() }
            firstSegment.finish(success = false)
            runCatching { device.close() }
            return Reason.CameraUnavailable
        }

        recorder.setOnInfoListener { _, what, _ -> onRecorderInfo(what) }

        val started = runCatching { recorder.start() }
        if (started.isFailure) {
            Log.e(TAG, "MediaRecorder.start() failed", started.exceptionOrNull())
            session = null
            runCatching { captureSession.close() }
            runCatching { recorder.release() }
            firstSegment.finish(success = false)
            runCatching { device.close() }
            return Reason.RecorderFailed
        }

        // A foreground service does not by itself keep the CPU out of
        // suspend — this is what actually does, for the length of the
        // recording. Released in finishRecording's cleanup, always.
        val powerManager = context.getSystemService(Context.POWER_SERVICE) as? PowerManager
        sess.wakeLock = powerManager
            ?.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "comrade:capture")
            ?.apply { setReferenceCounted(false); acquire() }

        armNextSegment(sess)

        if (!disableServiceForTest) {
            runCatching { CaptureService.start(context, destination) }
        }

        _state.value = UiState.Recording(sess.startedElapsedRealtimeMs, destination, lens, segment = 0)
        startGuardLoop(sess)
        startTicker(sess)
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
    private fun armNextSegment(sess: RecordingSession) {
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
                    val sess = session ?: return@launch
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
        val sess = session ?: return
        val next = sess.armedNext ?: return
        sess.current?.finish(success = true)
        sess.current = next
        sess.armedNext = null
        sess.segmentIndex += 1
        armNextSegment(sess)
        _state.value = UiState.Recording(sess.startedElapsedRealtimeMs, sess.destination, sess.lens, sess.segmentIndex)
    }

    /**
     * The one path every ended recording goes through — user [stop], an
     * auto-stop guard trip, a lost camera, or a setup failure partway through
     * [beginRecording] that already published a [session]. Always tears down
     * in the same order and always resolves [lastSaved]/[state] exactly once.
     */
    private fun finishRecording(success: Boolean, failReason: Reason?) {
        val sess = session ?: run {
            if (failReason != null) _state.value = UiState.Failed(failReason)
            return
        }
        session = null
        sess.guardJob?.cancel()
        sess.tickerJob?.cancel()

        var stoppedCleanly = success
        if (success) {
            val stopped = runCatching { sess.recorder?.stop() }
            if (stopped.isFailure) stoppedCleanly = false
        }
        runCatching { sess.recorder?.release() }
        runCatching { sess.captureSession?.close() }
        runCatching { sess.device?.close() }
        sess.activeOutputs.filter { it.isPreview }.forEach { runCatching { it.surface.release() } }
        sess.wakeLock?.let { if (it.isHeld) runCatching { it.release() } }

        val savedUri = sess.current?.finish(success = stoppedCleanly)
        sess.armedNext?.finish(success = false) // never started writing — discard

        _elapsedMs.value = 0
        if (stoppedCleanly && sess.current != null) {
            _lastSaved.value = Saved(
                uri = savedUri?.toString(),
                displayName = CaptureDecisions.segmentFileName(sess.baseName, sess.segmentIndex),
                destination = sess.destination,
            )
        }
        _state.value = failReason?.let { UiState.Failed(it) } ?: UiState.Idle
        if (!disableServiceForTest) {
            runCatching { CaptureService.stop(sess.appContext) }
        }
    }

    // ── Preview reconfiguration ──────────────────────────────────────────

    /**
     * Rebuild the capture session's output set when whether the preview
     * should be attached has changed — see the class doc for the frame cost
     * this carries. A no-op when there is no live recording, or when the
     * desired attachment already matches what is configured.
     */
    private fun reconfigureOutputs() {
        scope.launch(captureLane) {
            val sess = session ?: return@launch
            val device = sess.device ?: return@launch
            val recorder = sess.recorder ?: return@launch
            val wantsPreview = CaptureDecisions.previewAttached(screenOn, uiVisible) && preview != null
            val hasPreview = sess.activeOutputs.any { it.isPreview }
            if (wantsPreview == hasPreview) return@launch

            runCatching { sess.captureSession?.close() }
            val outputs = buildOutputs(sess.appContext, recorder.surface, sess.lens, sess.config, if (wantsPreview) preview else null)
            val newSession = createSession(device, outputs.map { it.surface })
            if (newSession == null) {
                Log.w(TAG, "capture session reconfiguration failed; recording continues on the previous frames")
                return@launch
            }
            val requestOk = runCatching {
                val builder = device.createCaptureRequest(CameraDevice.TEMPLATE_RECORD)
                outputs.forEach { builder.addTarget(it.surface) }
                newSession.setRepeatingRequest(builder.build(), null, cameraHandler)
            }
            if (requestOk.isFailure) {
                Log.w(TAG, "setRepeatingRequest failed after reconfiguration", requestOk.exceptionOrNull())
                return@launch
            }
            sess.activeOutputs.filter { it.isPreview }.forEach { runCatching { it.surface.release() } }
            sess.captureSession = newSession
            sess.activeOutputs = outputs
        }
    }

    private fun buildOutputs(
        context: Context,
        recorderSurface: Surface,
        lens: CaptureDecisions.Lens,
        config: VideoConfig,
        previewTarget: PreviewTarget?,
    ): List<Output> {
        val outputs = mutableListOf(Output(recorderSurface, isPreview = false))
        if (previewTarget != null) {
            val built = runCatching {
                val size = choosePreviewSize(context, lens.cameraId, config.width, config.height)
                previewTarget.texture.setDefaultBufferSize(size.width, size.height)
                Output(Surface(previewTarget.texture), isPreview = true)
            }.getOrNull()
            if (built != null) outputs += built
        }
        return outputs
    }

    private fun choosePreviewSize(context: Context, cameraId: String, targetWidth: Int, targetHeight: Int): Size {
        val fallback = Size(targetWidth, targetHeight)
        val manager = context.getSystemService(CameraManager::class.java) ?: return fallback
        val chars = runCatching { manager.getCameraCharacteristics(cameraId) }.getOrNull() ?: return fallback
        val map = chars.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP) ?: return fallback
        val sizes = map.getOutputSizes(SurfaceTexture::class.java)
        if (sizes.isNullOrEmpty()) return fallback
        val targetRatio = targetWidth.toDouble() / targetHeight
        return sizes.minByOrNull { size -> abs(size.width.toDouble() / size.height - targetRatio) } ?: fallback
    }

    // ── Camera2 plumbing ─────────────────────────────────────────────────

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
            val sess = session ?: return@launch
            if (sess.device !== lostDevice) return@launch
            finishRecording(success = true, failReason = Reason.CameraUnavailable)
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

    // ── Guards: battery, storage, thermal ────────────────────────────────

    private const val GUARD_INTERVAL_MS = 30_000L
    private const val TICKER_INTERVAL_MS = 500L

    /** 30s: frequent enough that a genuinely critical state gets caught
     *  promptly, infrequent enough that the guard itself is not a battery
     *  cost the feature has to answer for. */
    private fun startGuardLoop(sess: RecordingSession) {
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

    private fun startTicker(sess: RecordingSession) {
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
