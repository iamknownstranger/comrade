package mullu.comrade.ui

import android.Manifest
import android.graphics.SurfaceTexture
import android.view.TextureView
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
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
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import mullu.comrade.R
import mullu.comrade.capture.CaptureDecisions
import mullu.comrade.capture.CaptureManager
import mullu.comrade.ui.theme.Spacing

/**
 * The helmet cam (`docs/CAPTURE.md`). Deliberately thin, like `RideScreen` and
 * `TravelScreen`: every rule worth arguing about — lens ordering, the file
 * name, when a segment rolls, when the phone must stop itself — lives in
 * [CaptureDecisions], which is pure and JVM-tested. This file turns
 * [CaptureManager]'s state into a viewfinder and a few glove-sized buttons and
 * nothing more.
 *
 * **The recording is not this screen's.** [CaptureManager]/`CaptureService`
 * own it, as a foreground service, for exactly the reason the feature exists:
 * pressing the hardware lock button stops the *display*, not this
 * composition, and the file must keep growing regardless. So nothing below —
 * not `onDispose`, not `ON_STOP`, not the permission gate — ever calls
 * `CaptureManager.stop`. The only way to end a recording is the button the
 * rider taps on purpose.
 *
 * **No early returns.** Every branch is an `if`/`when` inside the layout, for
 * the reason `.claude/rules/android.md` gives: an early `return@Column`
 * changes the composable-group count across a branch and kills the screen on
 * the *recomposition*, not the first frame — which has already happened twice
 * in this app (`TaskListScreen`, `RideScreen`).
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
            // never end a recording the service is carrying; only the stop
            // button below may do that.
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
                onSurfaceAvailable = { texture, width, height ->
                    CaptureManager.attachPreview(context, texture, width, height)
                },
                onSurfaceDestroyed = {
                    CaptureManager.attachPreview(context, null, 0, 0)
                },
                onSwitchLens = { CaptureManager.switchLens(context) },
                onStart = { destination -> CaptureManager.start(context, destination) },
                onStop = { CaptureManager.stop(context) },
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

/** Everything drawn once camera and microphone access exist. */
@Composable
private fun CaptureBody(
    state: CaptureManager.UiState,
    lenses: List<CaptureDecisions.Lens>,
    selectedLensId: String?,
    elapsedMs: Long,
    lastSaved: CaptureManager.Saved?,
    onSurfaceAvailable: (SurfaceTexture, Int, Int) -> Unit,
    onSurfaceDestroyed: () -> Unit,
    onSwitchLens: () -> Unit,
    onStart: (CaptureDecisions.Destination) -> Unit,
    onStop: () -> Unit,
) {
    // The destination is chosen before recording starts and fixed for its
    // duration (docs/CAPTURE.md §5) — this is that choice, live only while
    // idle. Once `state` is `Recording`, its own `destination` is the one
    // that is actually in effect and is what the toggle below reflects.
    var pendingPrivate by rememberSaveable { mutableStateOf(false) }
    val recording = state as? CaptureManager.UiState.Recording
    val destination = recording?.destination
        ?: if (pendingPrivate) CaptureDecisions.Destination.Private else CaptureDecisions.Destination.Gallery

    Column(modifier = Modifier.fillMaxSize()) {
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .aspectRatio(3f / 4f)
                .background(Color.Black)
                .testTag("capture-viewfinder"),
        ) {
            CaptureViewfinder(
                onSurfaceAvailable = onSurfaceAvailable,
                onSurfaceDestroyed = onSurfaceDestroyed,
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
        }

        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(Spacing.space5),
            verticalArrangement = Arrangement.spacedBy(Spacing.space4),
        ) {
            lastSaved?.let { saved ->
                Text(
                    stringResource(
                        if (saved.destination == CaptureDecisions.Destination.Gallery) {
                            R.string.capture_saved_gallery
                        } else {
                            R.string.capture_saved_private
                        },
                        saved.displayName,
                    ),
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.testTag("capture-saved"),
                )
            }

            if (state is CaptureManager.UiState.Failed) {
                Text(
                    stringResource(failureMessage(state.reason)),
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.testTag("capture-failed"),
                )
            }

            if (recording != null) {
                Text(
                    stringResource(R.string.capture_lock_hint),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.testTag("capture-lock-hint"),
                )
            }

            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(Spacing.space3),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                LensButton(
                    lenses = lenses,
                    selectedLensId = selectedLensId,
                    // Camera2 cannot switch the physical camera without
                    // closing the file it is writing to, so the button is
                    // dead the moment a recording is running.
                    enabled = recording == null,
                    onClick = onSwitchLens,
                    modifier = Modifier.weight(1f),
                )
                RecordButton(
                    state = state,
                    onStart = { onStart(destination) },
                    onStop = onStop,
                    modifier = Modifier.weight(1f),
                )
            }

            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column {
                    Text(
                        stringResource(R.string.capture_private_toggle),
                        style = MaterialTheme.typography.titleSmall,
                    )
                    Text(
                        stringResource(R.string.capture_private_note),
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Switch(
                    checked = destination == CaptureDecisions.Destination.Private,
                    // The destination is fixed the moment a recording starts
                    // (docs/CAPTURE.md §5), so this is dead while one is
                    // running — the checked state above still reflects it.
                    enabled = recording == null,
                    onCheckedChange = { pendingPrivate = it },
                    modifier = Modifier.testTag("capture-private-toggle"),
                )
            }
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

/** Elapsed time plus the recording dot — `docs/DESIGN_SYSTEM.md`'s error
 *  token used exactly as the chat composer's own recording banner does. */
@Composable
private fun RecordingBadge(elapsedMs: Long, modifier: Modifier = Modifier) {
    Row(
        modifier = modifier
            .background(Color.Black.copy(alpha = 0.5f))
            .padding(horizontal = Spacing.space3, vertical = Spacing.space2)
            .testTag("capture-elapsed"),
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

@Composable
private fun RecordButton(
    state: CaptureManager.UiState,
    onStart: () -> Unit,
    onStop: () -> Unit,
    modifier: Modifier = Modifier,
) {
    // `if/else`, not an early return — `state` flips the moment the manager's
    // flow emits, which is a recomposition. See the file doc comment.
    if (state is CaptureManager.UiState.Recording) {
        Button(
            onClick = onStop,
            colors = ButtonDefaults.buttonColors(
                containerColor = MaterialTheme.colorScheme.error,
            ),
            modifier = modifier
                .height(Spacing.space16)
                .testTag("capture-record-button"),
        ) {
            Text(stringResource(R.string.capture_stop))
        }
    } else if (state is CaptureManager.UiState.Preparing) {
        Button(
            onClick = {},
            enabled = false,
            modifier = modifier
                .height(Spacing.space16)
                .testTag("capture-record-button"),
        ) {
            CircularProgressIndicator(
                modifier = Modifier.size(Spacing.space5),
                color = MaterialTheme.colorScheme.onPrimary,
            )
            Spacer(Modifier.size(Spacing.space2))
            Text(stringResource(R.string.capture_preparing))
        }
    } else {
        // Idle, or Failed — either way tapping this starts a fresh attempt.
        // A Failed reason is not a dead end (docs/CAPTURE.md §2): even the
        // three "stopped itself" reasons already saved a complete file, and
        // there is nothing else useful this button could do with them.
        Button(
            onClick = onStart,
            modifier = modifier
                .height(Spacing.space16)
                .testTag("capture-record-button"),
        ) {
            Text(
                stringResource(
                    if (state is CaptureManager.UiState.Failed) R.string.capture_retry else R.string.capture_record,
                ),
            )
        }
    }
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
}
