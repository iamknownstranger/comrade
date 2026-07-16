package mullu.comrade.ui

import android.text.format.Formatter
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import mullu.comrade.R
import mullu.comrade.voice.VoiceModelDownloader
import mullu.comrade.voice.VoiceModelDownloader.State
import mullu.comrade.voice.VoskModel

/**
 * The "download the on-device speech model?" prompt — Comrade's equivalent of
 * Google's offline speech-model dialog. Voice entry points show it when
 * [mullu.comrade.voice.VoskModel.isAvailable] is false: it explains the
 * one-time ~40 MB download, then tracks [VoiceModelDownloader]'s process-wide
 * state (progress → verify/install → ready or failed-with-retry). When the
 * model lands, [onReady] fires so the tap that opened the dialog can finally
 * do its job.
 *
 * Dismissing while a download runs does NOT abort it — like the platform
 * flow, it finishes in the background and any voice button picks it back up;
 * only the explicit cancel button stops it.
 */
@Composable
fun VoiceModelDownloadDialog(onReady: () -> Unit, onDismiss: () -> Unit) {
    val context = LocalContext.current
    val state by VoiceModelDownloader.state.collectAsState()

    LaunchedEffect(state) {
        if (state is State.Ready) {
            // Trust Ready only while the model is really still there — a
            // stale in-memory Ready (files cleared mid-process) re-arms the
            // offer instead of firing onReady into a load that can't succeed.
            if (VoskModel.isAvailable(context)) onReady() else VoiceModelDownloader.reofferIfGone(context)
        }
    }

    AlertDialog(
        onDismissRequest = onDismiss,
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
                        Formatter.formatShortFileSize(context, VoiceModelDownloader.MODEL_ZIP_BYTES),
                    ),
                )
            }
        },
        confirmButton = {
            when (state) {
                is State.Idle -> TextButton(onClick = { VoiceModelDownloader.start(context) }) {
                    Text(stringResource(R.string.voice_model_download))
                }
                is State.Failed -> TextButton(onClick = { VoiceModelDownloader.start(context) }) {
                    Text(stringResource(R.string.voice_model_retry))
                }
                else -> Unit
            }
        },
        dismissButton = {
            when (state) {
                is State.Downloading, State.Installing -> TextButton(
                    onClick = {
                        VoiceModelDownloader.cancel()
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
