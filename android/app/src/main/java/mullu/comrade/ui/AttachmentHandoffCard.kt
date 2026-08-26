package mullu.comrade.ui

import android.content.Intent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.core.content.FileProvider
import java.io.File
import mullu.comrade.handoff.AttachmentHandoffManager
import mullu.comrade.ui.theme.ComradeRadii
import mullu.comrade.ui.theme.GlassElevation
import mullu.comrade.ui.theme.glassSurface

/**
 * The large-attachment handoff, in the thread it belongs to.
 *
 * Renders [AttachmentHandoffManager]'s state and nothing of its own: the transfer
 * is owned by the manager, so this can be disposed, recomposed or navigated away
 * from without a 400 MB transfer noticing. Nothing at all is drawn when there is
 * no transfer, or when the live one belongs to a different conversation.
 *
 * Two rules it follows deliberately:
 *
 *  * **No fake affordances.** Accept is drawn only when the offer can actually be
 *    taken; an offer this device has no room for, or whose hash is not a hash,
 *    shows the reason and only Decline.
 *  * **Decline is not Refuse.** A person saying no sends
 *    `HandoffSignal.Decline`; the network saying no is what `Refuse` is for, and
 *    the manager is the only thing that sends it.
 */
@Composable
fun AttachmentHandoffPanel(peer: String, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val state by AttachmentHandoffManager.state.collectAsState()
    val relayQuestion by AttachmentHandoffManager.consentQuestion.collectAsState()
    var openError by remember { mutableStateOf<String?>(null) }

    if (state is AttachmentHandoffManager.UiState.Idle || state.peer != peer) return

    // The policy wants a person to agree to *this* transfer going through a
    // relay. Unanswered it stalls in silence, which is the failure the whole
    // consent path exists to remove.
    relayQuestion?.let { question ->
        val dialogShape = RoundedCornerShape(ComradeRadii.xl)
        AlertDialog(
            onDismissRequest = { AttachmentHandoffManager.refuseRelayConsent() },
            modifier = Modifier.testTag("handoff-relay-consent").glassSurface(GlassElevation.Sheet, shape = dialogShape),
            shape = dialogShape,
            containerColor = Color.Transparent,
            title = { Text("Send it anyway?") },
            text = { Text(question) },
            confirmButton = {
                TextButton(onClick = { AttachmentHandoffManager.grantRelayConsent() }) {
                    Text("Send it")
                }
            },
            dismissButton = {
                TextButton(onClick = { AttachmentHandoffManager.refuseRelayConsent() }) {
                    Text("Don't")
                }
            },
        )
    }

    Surface(
        shape = RoundedCornerShape(12.dp),
        color = MaterialTheme.colorScheme.surfaceVariant,
        modifier = modifier
            .fillMaxWidth()
            .padding(horizontal = 12.dp, vertical = 4.dp)
            .testTag("attachment-handoff"),
    ) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 10.dp)) {
            when (val s = state) {
                is AttachmentHandoffManager.UiState.Idle -> Unit

                is AttachmentHandoffManager.UiState.Preparing -> {
                    Line("Getting ${s.fileName} ready to send…")
                    // No progress: hashing reports nothing to divide, and a bar
                    // that moved by guesswork would be a lie about a real wait.
                    Detail(formatAttachmentSize(s.sizeBytes))
                }

                is AttachmentHandoffManager.UiState.Offered -> {
                    Line("Waiting for them to accept ${s.fileName}.")
                    Detail(
                        "${formatAttachmentSize(s.sizeBytes)} · they have to be online for this " +
                            "to start.",
                    )
                    Actions {
                        TextButton(
                            onClick = { AttachmentHandoffManager.cancel(context) },
                            modifier = Modifier.testTag("handoff-cancel"),
                        ) {
                            Text("Take it back")
                        }
                    }
                }

                is AttachmentHandoffManager.UiState.Incoming -> {
                    Line(s.headline)
                    if (s.caption.isNotEmpty()) Detail(s.caption)
                    Detail(
                        "Sent straight from their device, so it only arrives while you are " +
                            "both here.",
                    )
                    s.refusal?.let {
                        Text(
                            it,
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.error,
                            modifier = Modifier
                                .padding(top = 4.dp)
                                .testTag("handoff-offer-refusal"),
                        )
                    }
                    Actions {
                        TextButton(
                            onClick = { AttachmentHandoffManager.decline() },
                            modifier = Modifier.testTag("handoff-decline"),
                        ) {
                            Text("No thanks")
                        }
                        // Only when it can actually be taken.
                        if (s.refusal == null) {
                            Button(
                                onClick = { AttachmentHandoffManager.accept(context) },
                                modifier = Modifier
                                    .padding(start = 8.dp)
                                    .testTag("handoff-accept"),
                            ) {
                                Text("Receive")
                            }
                        }
                    }
                }

                is AttachmentHandoffManager.UiState.Moving -> {
                    Line(if (s.sending) "Sending ${s.fileName}" else "Receiving ${s.fileName}")
                    s.status?.let { Detail(it) }
                    val fraction = s.fraction
                    // Indeterminate while a route is still being worked out:
                    // there is genuinely nothing to measure yet.
                    if (fraction == null) {
                        LinearProgressIndicator(
                            modifier = Modifier
                                .fillMaxWidth()
                                .padding(top = 8.dp)
                                .testTag("handoff-progress"),
                        )
                    } else {
                        LinearProgressIndicator(
                            progress = { fraction },
                            modifier = Modifier
                                .fillMaxWidth()
                                .padding(top = 8.dp)
                                .testTag("handoff-progress"),
                        )
                    }
                    Actions {
                        TextButton(
                            onClick = { AttachmentHandoffManager.cancel(context) },
                            modifier = Modifier.testTag("handoff-cancel"),
                        ) {
                            Text("Stop")
                        }
                    }
                }

                is AttachmentHandoffManager.UiState.Ended -> {
                    Line(s.message)
                    s.advice?.let { Detail(it) }
                    openError?.let {
                        Text(
                            it,
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.error,
                            modifier = Modifier.padding(top = 4.dp),
                        )
                    }
                    Actions {
                        TextButton(
                            onClick = {
                                openError = null
                                AttachmentHandoffManager.dismiss(context)
                            },
                            modifier = Modifier.testTag("handoff-dismiss"),
                        ) {
                            // Naming the consequence: dismissing drops the
                            // plaintext this device is holding (AUDIT S-4).
                            Text(if (s.receivedPath == null) "OK" else "Delete it")
                        }
                        s.receivedPath?.let { path ->
                            Button(
                                onClick = {
                                    openError = runCatching {
                                        val uri = FileProvider.getUriForFile(
                                            context,
                                            "${context.packageName}.fileprovider",
                                            File(path),
                                        )
                                        context.startActivity(
                                            Intent(Intent.ACTION_VIEW).apply {
                                                setDataAndType(
                                                    uri,
                                                    s.receivedMime.ifBlank {
                                                        "application/octet-stream"
                                                    },
                                                )
                                                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                                            },
                                        )
                                    }.exceptionOrNull()?.let {
                                        "No app on this device opens that kind of file."
                                    }
                                },
                                modifier = Modifier
                                    .padding(start = 8.dp)
                                    .testTag("handoff-open"),
                            ) {
                                Text("Open")
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun Line(text: String) {
    Text(text, style = MaterialTheme.typography.bodyMedium)
}

@Composable
private fun Detail(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(top = 2.dp),
    )
}

@Composable
private fun Actions(content: @Composable () -> Unit) {
    Row(
        Modifier.fillMaxWidth().padding(top = 6.dp),
        horizontalArrangement = Arrangement.End,
    ) {
        content()
    }
}
