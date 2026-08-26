package mullu.comrade.ui

import android.text.format.Formatter
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import mullu.comrade.R
import mullu.comrade.model.ModelCatalog
import mullu.comrade.model.ModelDownloadService
import mullu.comrade.model.ModelDownloadState as State
import mullu.comrade.model.ModelDownloads
import mullu.comrade.ui.theme.ComradeRadii
import mullu.comrade.ui.theme.GlassElevation
import mullu.comrade.ui.theme.glassSurface
import mullu.comrade.voice.VoskModel

/**
 * The "download the on-device speech model?" prompt — Comrade's equivalent of
 * Google's offline speech-model dialog. Voice entry points show it when
 * [mullu.comrade.voice.VoskModel.isAvailable] is false: it explains the
 * one-time ~40 MB download, then tracks the shared, process-wide
 * [ModelDownloads] state (progress → verify/install → ready or
 * failed-with-retry). When the model lands, [onReady] fires so the tap that
 * opened the dialog can finally do its job.
 *
 * Dismissing while a download runs does NOT abort it — the transfer belongs to
 * [ModelDownloadService], so it keeps going (with progress in the notification
 * bar) even if the app is backgrounded, and any voice button picks it back up;
 * only the explicit cancel button stops it.
 */
@Composable
fun VoiceModelDownloadDialog(onReady: () -> Unit, onDismiss: () -> Unit) {
    val context = LocalContext.current
    val spec = ModelCatalog.SPEECH
    val state by ModelDownloads.stateOf(spec.id).collectAsState()

    LaunchedEffect(state) {
        if (state is State.Ready) {
            // Trust Ready only while the model is really still there — a
            // stale in-memory Ready (files cleared mid-process) re-arms the
            // offer instead of firing onReady into a load that can't succeed.
            if (VoskModel.isAvailable(context)) onReady() else ModelDownloads.reofferIfGone(context, spec)
        }
    }

    val dialogShape = RoundedCornerShape(ComradeRadii.xl)
    AlertDialog(
        onDismissRequest = onDismiss,
        modifier = Modifier.glassSurface(GlassElevation.Sheet, shape = dialogShape),
        shape = dialogShape,
        containerColor = Color.Transparent,
        title = { Text(stringResource(R.string.voice_model_prompt_title)) },
        text = {
            when (val current = state) {
                is State.Downloading -> Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(stringResource(R.string.voice_model_downloading))
                    LinearProgressIndicator(
                        progress = {
                            (current.bytesRead.toFloat() / current.totalBytes).coerceIn(0f, 1f)
                        },
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Text(
                        stringResource(
                            R.string.voice_model_progress,
                            Formatter.formatShortFileSize(context, current.bytesRead),
                            Formatter.formatShortFileSize(context, current.totalBytes),
                        ),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                is State.Installing -> Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(stringResource(R.string.voice_model_verifying))
                    LinearProgressIndicator(Modifier.fillMaxWidth())
                }
                is State.Failed -> Text(stringResource(R.string.voice_model_failed, current.message))
                // Idle (and the momentary Ready before onReady closes us):
                // the offer itself.
                else -> Text(
                    stringResource(
                        R.string.voice_model_prompt_body,
                        Formatter.formatShortFileSize(context, spec.downloadBytes),
                    ),
                )
            }
        },
        confirmButton = {
            when (state) {
                is State.Idle -> TextButton(onClick = { ModelDownloadService.start(context, spec.id) }) {
                    Text(stringResource(R.string.voice_model_download))
                }
                is State.Failed -> TextButton(onClick = { ModelDownloadService.start(context, spec.id) }) {
                    Text(stringResource(R.string.voice_model_retry))
                }
                else -> Unit
            }
        },
        dismissButton = {
            when (state) {
                is State.Downloading, State.Installing -> TextButton(
                    onClick = {
                        ModelDownloadService.cancel(spec.id)
                        onDismiss()
                    },
                ) { Text(stringResource(R.string.voice_model_cancel_download)) }
                else -> TextButton(onClick = onDismiss) {
                    Text(stringResource(R.string.voice_model_not_now))
                }
            }
        },
    )
}
