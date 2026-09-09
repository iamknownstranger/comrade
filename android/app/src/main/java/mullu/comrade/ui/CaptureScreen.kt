package mullu.comrade.ui

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.graphics.SurfaceTexture
import android.net.Uri
import android.view.TextureView
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import kotlinx.coroutines.delay
import mullu.comrade.R
import mullu.comrade.capture.CameraApp
import mullu.comrade.capture.CaptureDecisions
import mullu.comrade.capture.CaptureManager
import mullu.comrade.capture.SystemCameraHandoff
import mullu.comrade.ui.theme.GlassElevation
import mullu.comrade.ui.theme.Spacing
import mullu.comrade.ui.theme.glassSurface

/**
 * The helmet cam, shaped like a real camera app (`docs/CAPTURE.md`).
 *
 * Every rule worth arguing about lives in two pure, JVM-tested files:
 * [CaptureDecisions] (the helmet-cam invariant — a `MediaRecorder` that must
 * outlive the screen lock) and [CameraApp] (photo/video modes, zoom, flash,
 * timer, and [CameraApp.handoff] — the one place a shutter press is routed to
 * Comrade's own pipeline or handed to the phone's own camera app). This file
 * turns [CaptureManager]'s state and those two decisions into a viewfinder
 * and a Pixel-shaped set of controls, and nothing more.
 *
 * **The recording is not this screen's.** [CaptureManager]/`CaptureService`
 * own it, as a foreground service, for exactly the reason the feature
 * exists: pressing the hardware lock button stops the *display*, not this
 * composition, and the file must keep growing regardless. So nothing below —
 * not `onDispose`, not `ON_STOP`, not the permission gate — ever calls
 * `CaptureManager.stop`. The only way to end a recording is the shutter the
 * rider taps on purpose, in Video mode, while one is running.
 *
 * **No early returns.** Every branch is an `if`/`when` inside the layout, for
 * the reason `.claude/rules/android.md` gives: an early `return@Column`
 * changes the composable-group count across a branch and kills the screen on
 * the *recomposition*, not the first frame — which has already happened
 * twice in this app (`TaskListScreen`, `RideScreen`). Plain functions called
 * from click handlers and `LaunchedEffect` bodies (not the composable layout
 * itself) may still `return` early — that is ordinary Kotlin, not the hazard
 * this rule is about.
 */
@Composable
fun CaptureScreen(modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current

    var hasPermissions by remember { mutableStateOf(hasCapturePermissions(context)) }
    var permissionRefused by remember { mutableStateOf(false) }

    val permissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions(),
    ) { grants ->
        val granted = CAPTURE_PERMISSIONS.all { grants[it] == true }
        hasPermissions = granted
        permissionRefused = !granted
    }

    // Cameras are enumerated once permission exists to open one — before
    // that, CaptureManager has nothing useful to attach a preview to anyway.
    LaunchedEffect(hasPermissions) {
        if (hasPermissions) CaptureManager.refreshLenses(context)
    }

    val captureState by CaptureManager.state.collectAsState()
    val lenses by CaptureManager.lenses.collectAsState()
    val selectedLensId by CaptureManager.selectedLensId.collectAsState()
    val elapsedMs by CaptureManager.elapsedMs.collectAsState()
    val lastSaved by CaptureManager.lastSaved.collectAsState()
    val mode by CaptureManager.mode.collectAsState()
    val flash by CaptureManager.flash.collectAsState()
    val hasFlashUnit by CaptureManager.hasFlashUnit.collectAsState()
    val zoomRatio by CaptureManager.zoomRatio.collectAsState()
    val zoomRange by CaptureManager.zoomRange.collectAsState()
    val capturingPhoto by CaptureManager.capturingPhoto.collectAsState()
    val focusPoint by CaptureManager.focusPoint.collectAsState()

    // This is the mechanism behind the feature, not a housekeeping detail:
    // ON_START/ON_STOP tell CaptureManager whether a preview stream is worth
    // producing at all. Locking the phone stops this screen (ON_STOP) but
    // never touches the recording — see the file doc comment above.
    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_START -> CaptureManager.setUiVisible(context, true)
                Lifecycle.Event.ON_STOP -> CaptureManager.setUiVisible(context, false)
                else -> Unit
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose {
            lifecycleOwner.lifecycle.removeObserver(observer)
            // Deliberately no CaptureManager.stop() here. Navigating away —
            // the back arrow, a rotation, the Activity being recreated — must
            // never end a recording the service is carrying; only the
            // shutter, tapped on purpose while a recording is running, may
            // do that.
        }
    }

    Column(
        modifier = modifier
            .fillMaxSize()
            .testTag("capture-screen"),
    ) {
        if (!hasPermissions) {
            CapturePermissionExplainer(
                refused = permissionRefused,
                onGrant = { permissionLauncher.launch(CAPTURE_PERMISSIONS) },
            )
        } else {
            CaptureBody(
                state = captureState,
                lenses = lenses,
                selectedLensId = selectedLensId,
                elapsedMs = elapsedMs,
                lastSaved = lastSaved,
                mode = mode,
                flash = flash,
                hasFlashUnit = hasFlashUnit,
                zoomRatio = zoomRatio,
                zoomRange = zoomRange,
                capturingPhoto = capturingPhoto,
                focusPoint = focusPoint,
                onSurfaceAvailable = { texture, width, height ->
                    CaptureManager.attachPreview(context, texture, width, height)
                },
                onSurfaceDestroyed = {
                    CaptureManager.attachPreview(context, null, 0, 0)
                },
                onSwitchLens = { CaptureManager.switchLens(context) },
                onStart = { destination -> CaptureManager.start(context, destination) },
                onStop = { CaptureManager.stop(context) },
                onSetMode = { newMode -> CaptureManager.setMode(context, newMode) },
                onSetFlash = { newFlash -> CaptureManager.setFlash(newFlash) },
                onSetZoom = { ratio -> CaptureManager.setZoom(ratio) },
                onFocusAt = { nx, ny -> CaptureManager.focusAt(nx, ny) },
                onTakePhoto = { destination -> CaptureManager.takePhoto(context, destination) },
                onPublishExternalCapture = { saved -> CaptureManager.publishExternalCapture(saved) },
            )
        }
    }
}

private val CAPTURE_PERMISSIONS = arrayOf(Manifest.permission.CAMERA, Manifest.permission.RECORD_AUDIO)

private fun hasCapturePermissions(context: android.content.Context): Boolean =
    CAPTURE_PERMISSIONS.all {
        context.checkSelfPermission(it) == android.content.pm.PackageManager.PERMISSION_GRANTED
    }

/** A dead viewfinder with no explanation is worse than asking plainly. */
@Composable
private fun CapturePermissionExplainer(refused: Boolean, onGrant: () -> Unit) {
    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(Spacing.space6),
        verticalArrangement = Arrangement.spacedBy(Spacing.space3, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(
            stringResource(
                if (refused) R.string.capture_permission_denied else R.string.capture_permission_explainer,
            ),
            style = MaterialTheme.typography.bodyMedium,
        )
        Button(
            onClick = onGrant,
            modifier = Modifier.testTag("capture-grant-permission"),
        ) {
            Text(stringResource(R.string.capture_grant_permission))
        }
    }
}

/**
 * Everything drawn once camera and microphone access exist: a full-bleed
 * viewfinder with every control floating over it — `docs/DESIGN_SYSTEM.md`'s
 * glass tier is for exactly this kind of floating chrome, and Android's own
 * divergence from it (no backdrop blur, `ui/theme/Glass.kt` §5) applies here
 * the same as everywhere else.
 */
@Composable
private fun CaptureBody(
    state: CaptureManager.UiState,
    lenses: List<CaptureDecisions.Lens>,
    selectedLensId: String?,
    elapsedMs: Long,
    lastSaved: CaptureManager.Saved?,
    mode: CameraApp.Mode,
    flash: CameraApp.Flash,
    hasFlashUnit: Boolean,
    zoomRatio: Float,
    zoomRange: ClosedFloatingPointRange<Float>,
    capturingPhoto: Boolean,
    focusPoint: CaptureManager.FocusPoint?,
    onSurfaceAvailable: (SurfaceTexture, Int, Int) -> Unit,
    onSurfaceDestroyed: () -> Unit,
    onSwitchLens: () -> Unit,
    onStart: (CaptureDecisions.Destination) -> Unit,
    onStop: () -> Unit,
    onSetMode: (CameraApp.Mode) -> Unit,
    onSetFlash: (CameraApp.Flash) -> Unit,
    onSetZoom: (Float) -> Unit,
    onFocusAt: (Float, Float) -> Unit,
    onTakePhoto: (CaptureDecisions.Destination) -> Unit,
    onPublishExternalCapture: (CaptureManager.Saved) -> Unit,
) {
    val context = LocalContext.current

    // The destination is chosen before recording starts and fixed for its
    // duration (docs/CAPTURE.md §5) — this is that choice, live only while
    // idle. Once `state` is `Recording`, its own `destination` is the one
    // that is actually in effect and is what the toggle below reflects.
    var pendingPrivate by rememberSaveable { mutableStateOf(false) }

    // The helmet-cam switch: on by default, because keeping a video recording
    // alive through the screen lock is the entire reason this feature exists
    // (docs/CAPTURE.md §1) — a rider who never touches this control still
    // gets it.
    var keepRecordingWhenLocked by rememberSaveable { mutableStateOf(true) }

    // Staying inside Comrade is the default until a rider asks otherwise —
    // most people never need this, since CameraApp.handoff already refuses
    // it for every case (a locked ride, a private destination) where it
    // would cost something. See CameraApp's own doc comment.
    var preferSystemCamera by rememberSaveable { mutableStateOf(false) }

    var showGrid by rememberSaveable { mutableStateOf(false) }
    var timer by rememberSaveable { mutableStateOf(CameraApp.Timer.Off) }
    var countdownRemaining by rememberSaveable { mutableStateOf<Int?>(null) }
    var countdownGeneration by rememberSaveable { mutableStateOf(0) }

    // Not saveable: SystemCameraHandoff.Prepared holds a live CaptureSink.Still
    // (a ContentResolver + a Uri), not primitives, so it cannot survive the
    // process being killed while the system camera app is in front. See
    // SystemCameraHandoff's class doc for what that costs.
    var pendingExternal by remember { mutableStateOf<SystemCameraHandoff.Prepared?>(null) }

    var systemCameraInstalled by remember { mutableStateOf(false) }

    val recording = state as? CaptureManager.UiState.Recording
    val destination = recording?.destination
        ?: if (pendingPrivate) CaptureDecisions.Destination.Private else CaptureDecisions.Destination.Gallery

    val action = if (mode == CameraApp.Mode.Photo) CameraApp.ACTION_IMAGE_CAPTURE else CameraApp.ACTION_VIDEO_CAPTURE
    LaunchedEffect(action) {
        systemCameraInstalled = SystemCameraHandoff.installedFor(context, action)
    }

    val handoff = remember(mode, keepRecordingWhenLocked, destination, preferSystemCamera, systemCameraInstalled) {
        CameraApp.handoff(
            mode = mode,
            keepRecordingWhenLocked = keepRecordingWhenLocked,
            destinationIsPrivate = destination == CaptureDecisions.Destination.Private,
            preferSystemCamera = preferSystemCamera,
            systemCameraInstalled = systemCameraInstalled,
        )
    }

    val systemCameraLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.StartActivityForResult(),
    ) { result ->
        val prepared = pendingExternal
        if (prepared != null) {
            val saved = SystemCameraHandoff.finish(prepared, ok = result.resultCode == Activity.RESULT_OK)
            if (saved != null) onPublishExternalCapture(saved)
        }
        pendingExternal = null
    }

    fun launchSystemCamera(systemAction: String, isVideo: Boolean) {
        val prepared = SystemCameraHandoff.prepare(context, systemAction, isVideo)
        if (prepared != null) {
            pendingExternal = prepared
            systemCameraLauncher.launch(prepared.intent)
        }
        // prepared == null: the gallery target itself could not be opened
        // (full disk, a refused MediaStore insert). Nothing to launch, and
        // the same underlying cause already has its own message in the
        // record/photo failure paths, so this stays silent rather than
        // inventing a second way to say "not enough room".
    }

    fun beginPhotoNow() {
        val route = handoff.route
        if (route is CameraApp.Route.SystemCamera) {
            launchSystemCamera(route.action, isVideo = false)
        } else {
            onTakePhoto(destination)
        }
    }

    LaunchedEffect(countdownGeneration) {
        if (countdownGeneration > 0) {
            var remaining = countdownRemaining ?: 0
            while (remaining > 0) {
                delay(1_000)
                remaining -= 1
                countdownRemaining = remaining
            }
            countdownRemaining = null
            beginPhotoNow()
        }
    }

    fun onShutterClick() {
        when (mode) {
            CameraApp.Mode.Photo -> {
                val route = handoff.route
                val seconds = CameraApp.timerSeconds(timer)
                if (seconds <= 0 || route is CameraApp.Route.SystemCamera) {
                    // The self-timer is Comrade's own countdown UI; a shot
                    // handed to another app takes that app's capture UI along
                    // with it, timer included, so there is nothing of ours
                    // left to count down.
                    beginPhotoNow()
                } else {
                    countdownRemaining = seconds
                    countdownGeneration += 1
                }
            }
            CameraApp.Mode.Video -> {
                val route = handoff.route
                if (recording != null) {
                    onStop()
                } else if (route is CameraApp.Route.SystemCamera) {
                    launchSystemCamera(route.action, isVideo = true)
                } else {
                    onStart(destination)
                }
            }
        }
    }

    fun openLastShot() {
        val uri = lastSaved?.uri
        if (uri != null) {
            runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(uri))) }
        }
    }

    val fallbackTextRes = if (preferSystemCamera) fallbackMessage(handoff.fallback) else null

    Box(
        modifier = Modifier
            .fillMaxSize()
            .testTag("capture-viewfinder"),
    ) {
        CaptureViewfinderArea(
            onSurfaceAvailable = onSurfaceAvailable,
            onSurfaceDestroyed = onSurfaceDestroyed,
            showGrid = showGrid,
            focusPoint = focusPoint,
            zoomRatio = zoomRatio,
            zoomRange = zoomRange,
            onFocusTap = onFocusAt,
            onZoom = onSetZoom,
            modifier = Modifier.fillMaxSize(),
        )

        if (recording != null) {
            RecordingBadge(
                elapsedMs = elapsedMs,
                modifier = Modifier
                    .padding(Spacing.space4)
                    .align(Alignment.TopStart),
            )
        }

        if (countdownRemaining != null) {
            CountdownOverlay(
                remaining = countdownRemaining ?: 0,
                modifier = Modifier.align(Alignment.Center),
            )
        }

        TopControlStrip(
            flash = flash,
            hasFlashUnit = hasFlashUnit,
            timer = timer,
            showGrid = showGrid,
            isPrivate = destination == CaptureDecisions.Destination.Private,
            privateEnabled = recording == null,
            preferSystemCamera = preferSystemCamera,
            onCycleFlash = { onSetFlash(CameraApp.nextFlash(flash, mode)) },
            onCycleTimer = { timer = CameraApp.nextTimer(timer) },
            onToggleGrid = { showGrid = !showGrid },
            onTogglePrivate = { pendingPrivate = !pendingPrivate },
            onTogglePreferSystemCamera = { preferSystemCamera = !preferSystemCamera },
            modifier = Modifier
                .align(Alignment.TopCenter)
                .fillMaxWidth()
                .padding(Spacing.space2),
        )

        // Status banners: the handoff explanation, the saved/failed lines and
        // the lock hint. Stacked under the top strip rather than pushing the
        // viewfinder down — the whole point of this screen is that the
        // camera fills it.
        Column(
            modifier = Modifier
                .align(Alignment.TopCenter)
                .padding(top = Spacing.space12)
                .padding(horizontal = Spacing.space4),
            verticalArrangement = Arrangement.spacedBy(Spacing.space2),
        ) {
            if (fallbackTextRes != null) {
                StatusBanner(
                    stringResource(fallbackTextRes),
                    modifier = Modifier.testTag("capture-handoff-note"),
                )
            }
            lastSaved?.let { saved ->
                StatusBanner(
                    stringResource(
                        if (saved.destination == CaptureDecisions.Destination.Gallery) {
                            R.string.capture_saved_gallery
                        } else {
                            R.string.capture_saved_private
                        },
                        saved.displayName,
                    ),
                    modifier = Modifier.testTag("capture-saved"),
                )
            }
            if (state is CaptureManager.UiState.Failed) {
                StatusBanner(
                    stringResource(failureMessage(state.reason)),
                    color = MaterialTheme.colorScheme.error,
                    modifier = Modifier.testTag("capture-failed"),
                )
            }
            if (recording != null) {
                StatusBanner(
                    stringResource(R.string.capture_lock_hint),
                    modifier = Modifier.testTag("capture-lock-hint"),
                )
            }
        }

        Column(
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .padding(Spacing.space4),
            verticalArrangement = Arrangement.spacedBy(Spacing.space3),
        ) {
            ZoomChipsRow(
                zoomRatio = zoomRatio,
                zoomRange = zoomRange,
                onSelect = onSetZoom,
                modifier = Modifier.fillMaxWidth(),
            )
            ModeSwitcher(
                mode = mode,
                // Switching mode mid-recording makes no sense — there is
                // exactly one thing a live recording is doing — so this is
                // dead the same way the lens button below is.
                enabled = recording == null,
                onSetMode = onSetMode,
                modifier = Modifier.fillMaxWidth(),
            )
            if (mode == CameraApp.Mode.Video) {
                HelmetCamSwitchRow(
                    checked = keepRecordingWhenLocked,
                    onCheckedChange = { keepRecordingWhenLocked = it },
                    modifier = Modifier.fillMaxWidth(),
                )
            }
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                LastShotThumbnail(
                    saved = lastSaved,
                    onClick = { openLastShot() },
                )
                ShutterButton(
                    state = state,
                    mode = mode,
                    capturingPhoto = capturingPhoto,
                    countdownActive = countdownRemaining != null,
                    onClick = { onShutterClick() },
                )
                LensButton(
                    lenses = lenses,
                    selectedLensId = selectedLensId,
                    // Camera2 cannot switch the physical camera without
                    // closing the file it is writing to, so the button is
                    // dead the moment a recording is running.
                    enabled = recording == null,
                    onClick = onSwitchLens,
                )
            }
        }
    }
}

/**
 * The `TextureView` bridge into Camera2 plus every gesture the viewfinder
 * itself answers to: tap to focus and pinch to zoom. Ownership of what is
 * actually flowing into the `TextureView` — or whether anything is — belongs
 * entirely to `CaptureManager.attachPreview`; this composable only forwards
 * the surface, its size, and the gestures drawn over it.
 *
 * Both gesture detectors are keyed on `Unit` deliberately, not on the values
 * they read: a pinch changes [zoomRatio] continuously while it is in
 * progress, and keying on it would restart (and so cancel) the very gesture
 * in flight. [rememberUpdatedState] is what keeps each detector reading the
 * current value without needing to restart for it.
 */
@Composable
private fun CaptureViewfinderArea(
    onSurfaceAvailable: (SurfaceTexture, Int, Int) -> Unit,
    onSurfaceDestroyed: () -> Unit,
    showGrid: Boolean,
    focusPoint: CaptureManager.FocusPoint?,
    zoomRatio: Float,
    zoomRange: ClosedFloatingPointRange<Float>,
    onFocusTap: (Float, Float) -> Unit,
    onZoom: (Float) -> Unit,
    modifier: Modifier = Modifier,
) {
    val currentZoomRatio = rememberUpdatedState(zoomRatio)
    val currentZoomRange = rememberUpdatedState(zoomRange)
    val currentOnZoom = rememberUpdatedState(onZoom)
    val currentOnFocusTap = rememberUpdatedState(onFocusTap)

    Box(
        modifier = modifier
            .pointerInput(Unit) {
                detectTapGestures { offset ->
                    val nx = (offset.x / size.width.toFloat()).coerceIn(0f, 1f)
                    val ny = (offset.y / size.height.toFloat()).coerceIn(0f, 1f)
                    currentOnFocusTap.value(nx, ny)
                }
            }
            .pointerInput(Unit) {
                detectTransformGestures { _, _, zoomChange, _ ->
                    val range = currentZoomRange.value
                    val next = CameraApp.clampZoom(currentZoomRatio.value * zoomChange, range.start, range.endInclusive)
                    currentOnZoom.value(next)
                }
            },
    ) {
        CaptureViewfinder(onSurfaceAvailable, onSurfaceDestroyed, Modifier.fillMaxSize())
        if (showGrid) {
            RuleOfThirdsGrid(Modifier.fillMaxSize())
        }
        if (focusPoint != null) {
            FocusRing(focusPoint, Modifier.fillMaxSize())
        }
    }
}

/**
 * The `TextureView` bridge into Camera2. Ownership of what is actually
 * flowing into it — or whether anything is — belongs entirely to
 * [CaptureManager.attachPreview]; this composable only forwards the surface
 * and its size.
 */
@Composable
private fun CaptureViewfinder(
    onSurfaceAvailable: (SurfaceTexture, Int, Int) -> Unit,
    onSurfaceDestroyed: () -> Unit,
    modifier: Modifier = Modifier,
) {
    AndroidView(
        modifier = modifier,
        factory = { ctx ->
            TextureView(ctx).apply {
                surfaceTextureListener = object : TextureView.SurfaceTextureListener {
                    override fun onSurfaceTextureAvailable(
                        surface: SurfaceTexture,
                        width: Int,
                        height: Int,
                    ) {
                        onSurfaceAvailable(surface, width, height)
                    }

                    override fun onSurfaceTextureSizeChanged(
                        surface: SurfaceTexture,
                        width: Int,
                        height: Int,
                    ) {
                        onSurfaceAvailable(surface, width, height)
                    }

                    override fun onSurfaceTextureDestroyed(surface: SurfaceTexture): Boolean {
                        onSurfaceDestroyed()
                        return true
                    }

                    override fun onSurfaceTextureUpdated(surface: SurfaceTexture) = Unit
                }
            }
        },
    )
}

/** The rule-of-thirds grid overlay — pure decoration, drawn over whatever
 *  frame the viewfinder is currently showing (or the black box under it,
 *  before the first one arrives). */
@Composable
private fun RuleOfThirdsGrid(modifier: Modifier = Modifier) {
    Canvas(modifier = modifier) {
        val color = Color.White.copy(alpha = 0.5f)
        val strokePx = 1.dp.toPx()
        val thirdW = size.width / 3f
        val thirdH = size.height / 3f
        drawLine(color, Offset(thirdW, 0f), Offset(thirdW, size.height), strokeWidth = strokePx)
        drawLine(color, Offset(thirdW * 2f, 0f), Offset(thirdW * 2f, size.height), strokeWidth = strokePx)
        drawLine(color, Offset(0f, thirdH), Offset(size.width, thirdH), strokeWidth = strokePx)
        drawLine(color, Offset(0f, thirdH * 2f), Offset(size.width, thirdH * 2f), strokeWidth = strokePx)
    }
}

private const val FOCUS_RING_VISIBLE_MS = 900L

/** A ring at the tapped point, visible for [FOCUS_RING_VISIBLE_MS] after
 *  [focusPoint] changes and then gone — not tied to whether the camera is
 *  actually still focusing there, since Camera2 gives no such callback this
 *  file could wait on. */
@Composable
private fun FocusRing(focusPoint: CaptureManager.FocusPoint, modifier: Modifier = Modifier) {
    var visible by remember(focusPoint) { mutableStateOf(true) }
    LaunchedEffect(focusPoint) {
        delay(FOCUS_RING_VISIBLE_MS)
        visible = false
    }
    if (visible) {
        BoxWithConstraints(modifier = modifier) {
            val ringSize = Spacing.space12
            val x = maxWidth * focusPoint.nx - ringSize / 2
            val y = maxHeight * focusPoint.ny - ringSize / 2
            Box(
                modifier = Modifier
                    .offset(x, y)
                    .size(ringSize)
                    .border(BorderStroke(2.dp, Color.White), CircleShape)
                    .testTag("capture-focus-ring"),
            )
        }
    }
}

/** Elapsed time plus the recording dot — `docs/DESIGN_SYSTEM.md`'s error
 *  token used exactly as the chat composer's own recording banner does. */
@Composable
private fun RecordingBadge(elapsedMs: Long, modifier: Modifier = Modifier) {
    Row(
        modifier = modifier
            .background(Color.Black.copy(alpha = 0.5f))
            .padding(horizontal = Spacing.space3, vertical = Spacing.space2),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Spacing.space2),
    ) {
        Box(
            Modifier
                .size(10.dp)
                .clip(CircleShape)
                .background(MaterialTheme.colorScheme.error),
        )
        Text(
            CaptureDecisions.elapsedLabel(elapsedMs),
            color = Color.White,
            style = MaterialTheme.typography.labelMedium,
            modifier = Modifier.testTag("capture-elapsed"),
        )
    }
}

/** The self-timer's on-screen countdown, big enough to read at arm's length. */
@Composable
private fun CountdownOverlay(remaining: Int, modifier: Modifier = Modifier) {
    Box(
        modifier = modifier
            .size(Spacing.space16)
            .clip(CircleShape)
            .background(Color.Black.copy(alpha = 0.5f))
            .testTag("capture-countdown"),
        contentAlignment = Alignment.Center,
    ) {
        Text(remaining.toString(), color = Color.White, style = MaterialTheme.typography.headlineLarge)
    }
}

/**
 * Flash, self-timer, grid, Private and "use my camera app" — the row a
 * Pixel's own camera app puts across the top. A glass chip strip
 * (`docs/DESIGN_SYSTEM.md`'s floating-chrome tier), scrollable so a phone
 * narrow enough not to fit all five still reaches every one of them.
 */
@Composable
private fun TopControlStrip(
    flash: CameraApp.Flash,
    hasFlashUnit: Boolean,
    timer: CameraApp.Timer,
    showGrid: Boolean,
    isPrivate: Boolean,
    privateEnabled: Boolean,
    preferSystemCamera: Boolean,
    onCycleFlash: () -> Unit,
    onCycleTimer: () -> Unit,
    onToggleGrid: () -> Unit,
    onTogglePrivate: () -> Unit,
    onTogglePreferSystemCamera: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Row(
        modifier = modifier
            .glassSurface(GlassElevation.Chrome, shape = RoundedCornerShape(Spacing.space6))
            .horizontalScroll(rememberScrollState())
            .padding(horizontal = Spacing.space3, vertical = Spacing.space2),
        horizontalArrangement = Arrangement.spacedBy(Spacing.space2),
    ) {
        if (hasFlashUnit) {
            FilterChip(
                selected = flash != CameraApp.Flash.Off,
                onClick = onCycleFlash,
                label = {
                    Text(
                        stringResource(
                            when (flash) {
                                CameraApp.Flash.Off -> R.string.capture_flash_off
                                CameraApp.Flash.Auto -> R.string.capture_flash_auto
                                CameraApp.Flash.On -> R.string.capture_flash_on
                            },
                        ),
                    )
                },
                modifier = Modifier.testTag("capture-flash"),
            )
        }
        FilterChip(
            selected = timer != CameraApp.Timer.Off,
            onClick = onCycleTimer,
            label = {
                Text(
                    stringResource(
                        when (timer) {
                            CameraApp.Timer.Off -> R.string.capture_timer_off
                            CameraApp.Timer.Three -> R.string.capture_timer_3
                            CameraApp.Timer.Ten -> R.string.capture_timer_10
                        },
                    ),
                )
            },
            modifier = Modifier.testTag("capture-timer"),
        )
        FilterChip(
            selected = showGrid,
            onClick = onToggleGrid,
            label = { Text(stringResource(R.string.capture_grid_toggle)) },
            modifier = Modifier.testTag("capture-grid-toggle"),
        )
        FilterChip(
            selected = isPrivate,
            enabled = privateEnabled,
            onClick = onTogglePrivate,
            label = { Text(stringResource(R.string.capture_private_toggle)) },
            modifier = Modifier.testTag("capture-private-toggle"),
        )
        FilterChip(
            selected = preferSystemCamera,
            onClick = onTogglePreferSystemCamera,
            label = { Text(stringResource(R.string.capture_use_system_camera_chip)) },
            modifier = Modifier.testTag("capture-use-system-camera-toggle"),
        )
    }
}

/** The row of zoom stops a Pixel shows — `0.6× 1× 2×` — built from whatever
 *  [zoomRange] this lens actually covers ([CameraApp.zoomStops]). */
@Composable
private fun ZoomChipsRow(
    zoomRatio: Float,
    zoomRange: ClosedFloatingPointRange<Float>,
    onSelect: (Float) -> Unit,
    modifier: Modifier = Modifier,
) {
    val stops = remember(zoomRange) { CameraApp.zoomStops(zoomRange.start, zoomRange.endInclusive) }
    Row(
        modifier = modifier,
        horizontalArrangement = Arrangement.spacedBy(Spacing.space2, Alignment.CenterHorizontally),
    ) {
        stops.forEach { stop ->
            val label = CameraApp.zoomLabel(stop)
            FilterChip(
                selected = kotlin.math.abs(stop - zoomRatio) < 0.05f,
                onClick = { onSelect(CameraApp.clampZoom(stop, zoomRange.start, zoomRange.endInclusive)) },
                label = { Text(label) },
                modifier = Modifier.testTag("capture-zoom-$label"),
            )
        }
    }
}

/** Photo | Video, the switch every real camera app puts above its shutter. */
@Composable
private fun ModeSwitcher(
    mode: CameraApp.Mode,
    enabled: Boolean,
    onSetMode: (CameraApp.Mode) -> Unit,
    modifier: Modifier = Modifier,
) {
    Row(
        modifier = modifier,
        horizontalArrangement = Arrangement.spacedBy(Spacing.space2, Alignment.CenterHorizontally),
    ) {
        FilterChip(
            selected = mode == CameraApp.Mode.Photo,
            enabled = enabled,
            onClick = { onSetMode(CameraApp.Mode.Photo) },
            label = { Text(stringResource(R.string.capture_mode_photo)) },
            modifier = Modifier.testTag("capture-mode-photo"),
        )
        FilterChip(
            selected = mode == CameraApp.Mode.Video,
            enabled = enabled,
            onClick = { onSetMode(CameraApp.Mode.Video) },
            label = { Text(stringResource(R.string.capture_mode_video)) },
            modifier = Modifier.testTag("capture-mode-video"),
        )
    }
}

/** The helmet-cam switch, shown only in Video mode — see the feature's own
 *  reason for existing, `docs/CAPTURE.md` §1. */
@Composable
private fun HelmetCamSwitchRow(
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
    modifier: Modifier = Modifier,
) {
    Row(
        modifier = modifier
            .glassSurface(GlassElevation.Chrome, shape = RoundedCornerShape(Spacing.space3))
            .padding(horizontal = Spacing.space4, vertical = Spacing.space2),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(modifier = Modifier.weight(1f)) {
            Text(
                stringResource(R.string.capture_lock_toggle),
                style = MaterialTheme.typography.titleSmall,
                color = Color.White,
            )
            Text(
                stringResource(R.string.capture_lock_toggle_note),
                style = MaterialTheme.typography.labelSmall,
                color = Color.White.copy(alpha = 0.8f),
            )
        }
        Switch(
            checked = checked,
            onCheckedChange = onCheckedChange,
            modifier = Modifier.testTag("capture-lock-toggle"),
        )
    }
}

@Composable
private fun LensButton(
    lenses: List<CaptureDecisions.Lens>,
    selectedLensId: String?,
    enabled: Boolean,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val ordered = CaptureDecisions.orderLenses(lenses)
    val badge = selectedLensId?.let { CaptureDecisions.badge(ordered, it) }
    OutlinedButton(
        onClick = onClick,
        enabled = enabled && ordered.isNotEmpty(),
        modifier = modifier
            .height(Spacing.space16)
            .testTag("capture-switch-lens"),
    ) {
        Icon(FlipCameraIcon, contentDescription = null)
        Spacer(Modifier.size(Spacing.space2))
        Text(if (badge != null) lensBadgeLabel(badge) else stringResource(R.string.capture_no_lens))
    }
}

@Composable
private fun lensBadgeLabel(badge: CaptureDecisions.LensBadge): String {
    val facingName = stringResource(
        when (badge.facing) {
            CaptureDecisions.Facing.Back -> R.string.capture_lens_back
            CaptureDecisions.Facing.Front -> R.string.capture_lens_front
            CaptureDecisions.Facing.External -> R.string.capture_lens_external
        },
    )
    return if (badge.ordinal <= 0) {
        facingName
    } else {
        stringResource(R.string.capture_lens_ordinal, facingName, badge.ordinal + 1)
    }
}

/**
 * The shutter: a plain white circle in Photo mode, a red circle in Video
 * mode that becomes a red square while [state] is [CaptureManager.UiState.Recording],
 * and a spinner whenever it is not safe to tap — preparing, mid-photo, or
 * counting down the self-timer.
 */
@Composable
private fun ShutterButton(
    state: CaptureManager.UiState,
    mode: CameraApp.Mode,
    capturingPhoto: Boolean,
    countdownActive: Boolean,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val recording = state is CaptureManager.UiState.Recording
    val busy = state is CaptureManager.UiState.Preparing || capturingPhoto || countdownActive
    val innerColor = if (mode == CameraApp.Mode.Video) MaterialTheme.colorScheme.error else Color.White
    val contentDescription = stringResource(
        when {
            recording -> R.string.capture_stop
            mode == CameraApp.Mode.Photo -> R.string.capture_take_photo
            else -> R.string.capture_record
        },
    )
    Box(
        modifier = modifier
            .size(Spacing.space16)
            .clip(CircleShape)
            .border(BorderStroke(4.dp, Color.White), CircleShape)
            .clickable(enabled = !busy, onClickLabel = contentDescription, onClick = onClick)
            .testTag("capture-record-button"),
        contentAlignment = Alignment.Center,
    ) {
        if (busy) {
            CircularProgressIndicator(
                modifier = Modifier.size(Spacing.space6),
                color = innerColor,
                strokeWidth = 3.dp,
            )
        } else if (recording) {
            Box(
                Modifier
                    .size(Spacing.space6)
                    .clip(RoundedCornerShape(4.dp))
                    .background(innerColor),
            )
        } else {
            Box(
                Modifier
                    .size(Spacing.space10)
                    .clip(CircleShape)
                    .background(innerColor),
            )
        }
    }
}

/**
 * The last thing shot this session — [saved]'s `Uri`, opened in the system
 * viewer. Inert (and says so) for a Private save, which has no `Uri` by
 * construction, and for the empty slot before anything has been shot yet.
 */
@Composable
private fun LastShotThumbnail(
    saved: CaptureManager.Saved?,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val hasUri = saved?.uri != null
    Box(
        modifier = modifier
            .size(Spacing.space12)
            .clip(RoundedCornerShape(Spacing.space2))
            .background(Color.Black.copy(alpha = 0.5f))
            .let { base -> if (hasUri) base.clickable(onClickLabel = stringResource(R.string.capture_last_shot), onClick = onClick) else base }
            .testTag("capture-last-shot"),
        contentAlignment = Alignment.Center,
    ) {
        if (saved == null) {
            // Nothing shot yet this session — an empty slot, not a broken one.
        } else if (!hasUri) {
            Text(
                stringResource(R.string.capture_last_shot_private),
                style = MaterialTheme.typography.labelSmall,
                color = Color.White,
                textAlign = TextAlign.Center,
                modifier = Modifier.padding(Spacing.space1),
            )
        } else {
            val icon = if (saved.kind == CaptureManager.Kind.Video) VideocamIcon else PhotoCameraIcon
            Icon(icon, contentDescription = stringResource(R.string.capture_last_shot), tint = Color.White)
        }
    }
}

/** A short status line over the viewfinder, glass-chip styled to match the
 *  top control strip. */
@Composable
private fun StatusBanner(text: String, modifier: Modifier = Modifier, color: Color = Color.White) {
    Text(
        text,
        color = color,
        style = MaterialTheme.typography.bodySmall,
        modifier = modifier
            .glassSurface(GlassElevation.Chrome, shape = RoundedCornerShape(Spacing.space2))
            .padding(horizontal = Spacing.space3, vertical = Spacing.space2),
    )
}

private fun failureMessage(reason: CaptureManager.Reason): Int = when (reason) {
    CaptureManager.Reason.CameraUnavailable -> R.string.capture_error_camera_unavailable
    CaptureManager.Reason.RecorderFailed -> R.string.capture_error_recorder_failed
    CaptureManager.Reason.NoStorage -> R.string.capture_error_no_storage
    CaptureManager.Reason.PermissionDenied -> R.string.capture_error_permission_denied
    CaptureManager.Reason.StoppedBattery -> R.string.capture_stopped_battery
    CaptureManager.Reason.StoppedStorage -> R.string.capture_stopped_storage
    CaptureManager.Reason.StoppedThermal -> R.string.capture_stopped_thermal
    CaptureManager.Reason.StoppedCameraLost -> R.string.capture_stopped_camera_lost
    CaptureManager.Reason.PhotoFailed -> R.string.capture_error_photo_failed
}

/**
 * Why a shot stayed in Comrade even though "use my camera app" is on —
 * `null` for [CameraApp.Fallback.None] (the handoff actually happened, so
 * there is nothing to explain) and for [CameraApp.Fallback.UserChoseInApp]
 * (the switch being off already says this; no one flipped it and needs it
 * explained back to them).
 */
private fun fallbackMessage(fallback: CameraApp.Fallback): Int? = when (fallback) {
    CameraApp.Fallback.RideMustStayInApp -> R.string.capture_handoff_ride_must_stay_in_app
    CameraApp.Fallback.PrivateStaysInApp -> R.string.capture_handoff_private_stays_in_app
    CameraApp.Fallback.NoSystemCameraInstalled -> R.string.capture_handoff_no_system_camera_installed
    CameraApp.Fallback.UserChoseInApp -> null
    CameraApp.Fallback.None -> null
}
