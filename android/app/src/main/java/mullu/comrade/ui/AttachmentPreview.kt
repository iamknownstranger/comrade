package mullu.comrade.ui

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.provider.OpenableColumns
import androidx.compose.foundation.Image
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import mullu.comrade.handoff.HandoffDecisions
import mullu.comrade.ui.theme.ComradeRadii
import mullu.comrade.ui.theme.GlassElevation
import mullu.comrade.ui.theme.glassSurface

/**
 * One attachment that has been obtained but not yet sent, waiting on the
 * sender's confirmation.
 *
 * A plain class, not a `data class`: it holds a [ByteArray], and the generated
 * `equals` would compare those by identity anyway while implying it compares
 * them by content.
 */
class PendingAttachment(
    /** The device-chosen filename, or empty for a capture that has none. */
    val name: String,
    val mime: String,
    /**
     * The bytes, for a file taking the hosted road.
     *
     * Null for one going straight to the other device: a 400 MB attachment is
     * read a chunk at a time out of the provider's descriptor while it is being
     * sent, and holding it here would be an out-of-memory kill before the sender
     * had even pressed Send.
     */
    val bytes: ByteArray?,
    /** The provider URI, for a file going straight to the other device. */
    val uri: Uri?,
    /** The size: [bytes]' length when there are bytes, the provider's otherwise. */
    val sizeBytes: Long,
    /** What the caption box opens with — the composer's draft, or empty while a reply is pending. */
    val seedCaption: String,
    /** Whether [seedCaption] came from the composer, so the box must be cleared on success. */
    val consumesDraft: Boolean,
    /**
     * Whether this file goes straight to the other device rather than through a
     * host. Answered by `attachment_route_for_bytes`, never by comparing against
     * a local 10 MB — the threshold belongs to the core that enforces it.
     */
    val peerToPeer: Boolean,
)

/**
 * The last look before an attachment leaves the device.
 *
 * Telegram's shape: pick a file and you get the file itself, a caption box, and
 * two ways out. This app had none of it — a pick went straight to encrypt and
 * upload, so the first time the sender saw what they had chosen was in their own
 * thread, already delivered. Three things follow:
 *
 *  * **A wrong pick is recoverable.** A thumbnail plus the file's own name and
 *    size is what tells you the picker handed back the wrong something.
 *  * **The caption is composed against the picture**, not typed beforehand into a
 *    box that then empties. It is still *seeded* from the composer, so the habit
 *    of typing-then-attaching keeps working (see [captionForAttachment]).
 *  * **Backing out costs nothing.** Nothing is encrypted or uploaded until Send.
 *
 * Ports: `app/lib/src/widgets/attachment_preview.dart`, `openAttachmentPreview`
 * in `desktop/ui/main.js`.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AttachmentPreviewSheet(
    pending: PendingAttachment,
    onDismiss: () -> Unit,
    onSend: (String) -> Unit,
) {
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    var caption by remember(pending) { mutableStateOf(TextFieldValue(pending.seedCaption)) }

    val sheetShape = RoundedCornerShape(topStart = ComradeRadii.xl, topEnd = ComradeRadii.xl)
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
        modifier = Modifier.testTag("attachment-preview").glassSurface(GlassElevation.Sheet, shape = sheetShape),
        shape = sheetShape,
        containerColor = Color.Transparent,
    ) {
        // Scrollable: header + a 280 dp photo + the caption box + the buttons is
        // taller than a phone in landscape, and the two controls the sheet exists
        // for are the ones at the bottom.
        Column(
            Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 16.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(
                        mediaQuoteLabel(pending.mime, ""),
                        style = MaterialTheme.typography.titleMedium,
                    )
                    Text(
                        // The one place the device-chosen filename belongs: it
                        // answers "is this the file I meant", and it is
                        // deliberately not what gets sent as the caption.
                        attachmentPreviewDetail(pending.name, pending.sizeBytes),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        modifier = Modifier.testTag("attachment-preview-detail"),
                    )
                }
                // "✕" as text, the same way `MediaViewerDialog` closes: there is no
                // close glyph in `AppIcons.kt` and one drawn for this alone would
                // be the only vector in the file with a single caller.
                TextButton(
                    onClick = onDismiss,
                    modifier = Modifier.testTag("attachment-preview-close"),
                ) {
                    Text("✕", style = MaterialTheme.typography.titleMedium)
                }
            }

            // Which road this file takes, and what that costs — stated before
            // Send rather than discovered afterwards as a progress bar that never
            // moves.
            Text(
                HandoffDecisions.routeNote(pending.peerToPeer),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier
                    .padding(top = 4.dp)
                    .testTag("attachment-preview-route"),
            )

            AttachmentPreviewBody(pending)

            OutlinedTextField(
                value = caption,
                onValueChange = {
                    // Capped at the input rather than silently on send, so the
                    // sender is never shown words the recipient will not get.
                    caption = if (it.text.length <= MAX_CAPTION_LENGTH) {
                        it
                    } else {
                        TextFieldValue(it.text.take(MAX_CAPTION_LENGTH))
                    }
                },
                placeholder = { Text("Add a caption…") },
                shape = RoundedCornerShape(12.dp),
                maxLines = 4,
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(top = 12.dp)
                    .testTag("attachment-preview-caption"),
            )

            Row(
                Modifier
                    .fillMaxWidth()
                    .padding(top = 8.dp, bottom = 20.dp),
                horizontalArrangement = Arrangement.End,
            ) {
                TextButton(
                    onClick = onDismiss,
                    modifier = Modifier.testTag("attachment-preview-cancel"),
                ) {
                    Text("Cancel")
                }
                Button(
                    onClick = { onSend(normalizeCaption(caption.text)) },
                    modifier = Modifier
                        .padding(start = 8.dp)
                        .testTag("attachment-preview-send"),
                ) {
                    Text("Send")
                }
            }
        }
    }
}

@Composable
private fun AttachmentPreviewBody(pending: PendingAttachment) {
    val bytes = pending.bytes
    if (bytes == null) {
        // Nothing to draw: the file was never read into memory, and decoding a
        // 400 MB pick to show a thumbnail is the allocation that keeping it out of
        // memory avoided. The name, the size and the road are the whole story for
        // a file this size.
        AttachmentPreviewCard(pending, note = null)
        return
    }
    when (attachmentPreviewKind(pending.mime)) {
        AttachmentPreviewKind.Image -> InlinePreviewImage(pending, bytes)
        // No still frame and no player: feeding `VideoView` means a file, and an
        // unsent attachment must not put plaintext on disk (AUDIT S-4).
        AttachmentPreviewKind.Video,
        AttachmentPreviewKind.Audio,
        AttachmentPreviewKind.File,
        -> AttachmentPreviewCard(pending, note = null)
    }
}

@Composable
private fun InlinePreviewImage(pending: PendingAttachment, bytes: ByteArray) {
    var bitmap by remember(pending) { mutableStateOf<Bitmap?>(null) }
    var failed by remember(pending) { mutableStateOf(false) }

    LaunchedEffect(pending) {
        val decoded = withContext(Dispatchers.IO) { decodePreview(bytes) }
        if (decoded == null) failed = true else bitmap = decoded
    }

    val decoded = bitmap
    when {
        // A picker can hand back something labelled an image that is not one. Say
        // so rather than showing a broken glyph — and still allow the send: the
        // bytes are the sender's to send and another device may well decode them.
        failed -> AttachmentPreviewCard(
            pending,
            note = "This device cannot display it, but it can still be sent.",
        )
        // Reserve the room the picture will take, so the caption box and the
        // buttons do not jump the moment it decodes.
        decoded == null -> Box(
            Modifier
                .fillMaxWidth()
                .height(
                    attachmentPreviewMediaHeight(
                        LocalConfiguration.current.screenHeightDp.toFloat(),
                    ).dp,
                ),
        )
        else -> Image(
            bitmap = decoded.asImageBitmap(),
            contentDescription = mediaKindLabel(pending.mime),
            contentScale = ContentScale.Fit,
            modifier = Modifier
                .fillMaxWidth()
                // Bounded by the shared rule, so a portrait photo cannot occupy
                // the whole sheet and leave the caption box below the fold.
                .heightIn(
                    max = attachmentPreviewMediaHeight(
                        LocalConfiguration.current.screenHeightDp.toFloat(),
                    ).dp,
                )
                .testTag("attachment-preview-image"),
        )
    }
}

/**
 * Decode at most ~2 megapixels.
 *
 * A 10 MB JPEG can be 50 megapixels, which is 200 MB of ARGB_8888 and an OOM on
 * a mid-range phone — for a thumbnail that will be drawn 280 dp tall. The
 * original bytes are what gets sent; this is only what gets looked at.
 *
 * The bounds-then-sample shape, and the power-of-two arithmetic itself, live in
 * [BitmapBudget] — shared with [MediaCache.decodeImage] and
 * `ProfileScreen.decodeAvatar` rather than re-derived here a third time.
 */
private fun decodePreview(bytes: ByteArray): Bitmap? {
    val probe = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, probe)
    if (probe.outWidth <= 0 || probe.outHeight <= 0) return null
    val options = BitmapFactory.Options().apply {
        inSampleSize = BitmapBudget.sampleSizeFor(probe.outWidth, probe.outHeight, PREVIEW_MAX_PIXELS)
    }
    return BitmapFactory.decodeByteArray(bytes, 0, bytes.size, options)
}

private const val PREVIEW_MAX_PIXELS = 2_000_000L

@Composable
private fun AttachmentPreviewCard(pending: PendingAttachment, note: String?) {
    Surface(
        color = MaterialTheme.colorScheme.surfaceVariant,
        shape = RoundedCornerShape(12.dp),
        modifier = Modifier
            .fillMaxWidth()
            .padding(top = 8.dp)
            .testTag("attachment-preview-card"),
    ) {
        Column(
            Modifier
                .fillMaxWidth()
                .padding(vertical = 24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(mediaKindGlyph(pending.mime), style = MaterialTheme.typography.headlineLarge)
            Text(mediaKindLabel(pending.mime), style = MaterialTheme.typography.bodyMedium)
            if (note != null) {
                Text(
                    note,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(top = 4.dp, start = 16.dp, end = 16.dp),
                )
            }
        }
    }
}

/**
 * The length of a picked file when the provider reported none in
 * [describePickedFile].
 *
 * `OpenableColumns.SIZE` is optional in the SAF contract, and a missing size used
 * to mean reading the whole file to measure it — which for a 400 MB pick is the
 * out-of-memory kill that not holding it in memory exists to avoid. The
 * descriptor's `statSize` answers the same question for nothing, and reports -1
 * for a stream that genuinely has no length, which reads as "still unknown".
 */
fun pickedFileLength(context: Context, uri: Uri): Long? = runCatching {
    context.contentResolver.openFileDescriptor(uri, "r")?.use { it.statSize }
}.getOrNull()?.takeIf { it >= 0 }

/** What the content provider says about a picked file, before it is read. */
class PickedFileInfo(
    val name: String,
    val mime: String,
    /** Null where the provider does not report one. */
    val size: Long?,
)

/**
 * Ask the content provider for a picked file's display name, MIME type and size.
 *
 * The name is what makes a wrong pick recognisable in the preview sheet; the size
 * is what lets an oversize file be refused *without* reading it, so choosing a
 * 1 GB video does not mean loading a 1 GB video into memory to learn that it is
 * one. Both columns are optional in the SAF contract, so both degrade: a blank
 * name shows just the size, and a missing size falls back to measuring the bytes.
 */
fun describePickedFile(context: Context, uri: Uri): PickedFileInfo {
    val mime = context.contentResolver.getType(uri) ?: "application/octet-stream"
    var name = ""
    var size: Long? = null
    // A provider is free to throw on an unsupported query; the file is still
    // readable, so fall back to knowing nothing about it rather than failing the
    // pick.
    runCatching {
        context.contentResolver.query(
            uri,
            arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE),
            null,
            null,
            null,
        )?.use { cursor ->
            if (cursor.moveToFirst()) {
                val nameColumn = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                if (nameColumn >= 0 && !cursor.isNull(nameColumn)) {
                    name = cursor.getString(nameColumn) ?: ""
                }
                val sizeColumn = cursor.getColumnIndex(OpenableColumns.SIZE)
                if (sizeColumn >= 0 && !cursor.isNull(sizeColumn)) {
                    size = cursor.getLong(sizeColumn)
                }
            }
        }
    }
    return PickedFileInfo(name, mime, size)
}
