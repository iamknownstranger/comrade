package mullu.comrade.ui

import android.content.Intent
import android.graphics.Bitmap
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import mullu.comrade.ComradeCore

/**
 * A chat bubble for one NIP-94/96 attachment — renders images, audio, and
 * video inline, and offers a generic "open externally" action for anything
 * else (e.g. PDFs), matching standard messaging-app UX.
 *
 * A tap on a photo or a video opens it full screen ([MediaViewerDialog]); a
 * long press anywhere on the bubble starts a reply aimed at it, the same
 * gesture a text bubble answers to. [onReply] is null where the caller offers
 * no reply, and then there is no long press to miss rather than one that does
 * nothing.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun MediaAttachmentBubble(
    info: ComradeCore.MediaMessageInfo,
    modifier: Modifier = Modifier,
    onReply: (() -> Unit)? = null,
) {
    var viewerOpen by remember(info.eventId) { mutableStateOf(false) }
    if (viewerOpen) {
        MediaViewerDialog(info, onDismiss = { viewerOpen = false })
    }
    // The long press has to be attached to each *leaf* as well as the bubble:
    // a child with its own pointer input (the photo's tap-to-open, the audio
    // play button) consumes the gesture before an ancestor sees it, so a bubble
    // that only wrapped itself would be repliable everywhere except on the
    // attachment itself — which is the part people press.
    val openFullScreen: (() -> Unit)? =
        if (opensFullScreen(info.mimeType)) ({ viewerOpen = true }) else null
    // A bubble with neither action gets no gesture at all, rather than a
    // ripple that leads nowhere — the settings screen's "no fake switches"
    // rule, applied to a tap target.
    val bubbleModifier = modifier
        .widthIn(max = 280.dp)
        .let { base ->
            if (openFullScreen == null && onReply == null) {
                base
            } else {
                base.combinedClickable(
                    onClick = { openFullScreen?.invoke() },
                    onLongClick = onReply,
                )
            }
        }
    Surface(
        shape = RoundedCornerShape(
            topStart = 18.dp,
            topEnd = 18.dp,
            bottomStart = if (info.outgoing) 18.dp else 6.dp,
            bottomEnd = if (info.outgoing) 6.dp else 18.dp,
        ),
        color = if (info.outgoing) {
            MaterialTheme.colorScheme.primaryContainer
        } else {
            MaterialTheme.colorScheme.surfaceVariant
        },
        tonalElevation = 1.dp,
        modifier = bubbleModifier,
    ) {
        Column(Modifier.padding(10.dp)) {
            if (info.caption.isNotBlank()) {
                Text(
                    info.caption,
                    style = MaterialTheme.typography.bodySmall,
                    modifier = Modifier.padding(bottom = 6.dp),
                )
            }
            when {
                info.mimeType.startsWith("image/") ->
                    InlineImage(info, onOpen = openFullScreen, onReply = onReply)
                info.mimeType.startsWith("audio/") -> InlineAudio(info)
                info.mimeType.startsWith("video/") ->
                    VideoPoster(onOpen = openFullScreen, onReply = onReply)
                else -> GenericFile(info)
            }
            Text(
                relativeTime(info.createdAt),
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.outline,
                modifier = Modifier.padding(top = 4.dp),
            )
        }
    }
}

/**
 * Images auto-load, like any standard messenger — no extra tap needed. The tap
 * is spent on opening the photo full screen instead, which is what a tap on a
 * photo means everywhere else.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun InlineImage(
    info: ComradeCore.MediaMessageInfo,
    onOpen: (() -> Unit)?,
    onReply: (() -> Unit)?,
) {
    var bitmap by remember(info.eventId) { mutableStateOf<Bitmap?>(null) }
    var error by remember(info.eventId) { mutableStateOf<String?>(null) }

    LaunchedEffect(info.eventId) {
        runCatching { MediaCache.decodeImage(info) }
            .onSuccess { bitmap = it }
            .onFailure { error = it.message ?: "Could not load image" }
    }

    Box(
        modifier = Modifier
            .widthIn(max = 240.dp)
            .heightIn(max = 240.dp)
            .clip(RoundedCornerShape(10.dp))
            .background(MaterialTheme.colorScheme.surface.copy(alpha = 0.4f))
            .combinedClickable(
                // A failed image keeps the retry it always had: the tap that
                // would open a picture there is no picture for is better spent
                // fetching one.
                onClick = { if (error != null) error = null else onOpen?.invoke() },
                onLongClick = onReply,
            )
            .testTag("media-open-fullscreen"),
        contentAlignment = Alignment.Center,
    ) {
        val shown = bitmap
        when {
            shown != null -> Image(
                bitmap = shown.asImageBitmap(),
                contentDescription = info.caption.ifBlank { "Photo attachment" },
            )
            error != null -> Text(
                "⚠ $error · tap to retry",
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.padding(16.dp),
            )
            else -> CircularProgressIndicator(Modifier.padding(24.dp).size(28.dp))
        }
    }
}

/**
 * Voice notes and audio clips, played by the same bar the journal's voice
 * entries use ([AudioPlayerBar]).
 *
 * This bubble used to be a play button and the words "Voice message" — no
 * length, no progress, no way back over the sentence you missed. Sharing the
 * player with the journal is what fixed that, and is why the fix cannot drift
 * back apart: there is one implementation to improve.
 *
 * The decrypt still happens on the tap, not on the scroll — [MediaCache.resolveFile]
 * is the resolver, so a thread of voice notes costs nothing until one is played.
 * No duration is passed: a chat note is an AAC/ADTS stream with no duration
 * header, so the bar falls back to whatever `MediaPlayer` reports. See
 * `journal/JournalRecordings.kt` for why the journal's own recordings use a
 * container that does carry one.
 */
@Composable
private fun InlineAudio(info: ComradeCore.MediaMessageInfo) {
    val context = LocalContext.current
    AudioPlayerBar(
        resolve = { MediaCache.resolveFile(context, info) },
        key = info.eventId,
        label = "Voice message",
    )
}

/**
 * Video: a poster that opens the full-screen viewer. The decrypt happens there,
 * on the tap — still bandwidth-conscious, and now played on a whole screen
 * rather than in a 280 dp bubble.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun VideoPoster(onOpen: (() -> Unit)?, onReply: (() -> Unit)?) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(10.dp))
            .background(MaterialTheme.colorScheme.surface.copy(alpha = 0.4f))
            .combinedClickable(onClick = { onOpen?.invoke() }, onLongClick = onReply)
            .testTag("media-open-fullscreen")
            .padding(20.dp),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text("▶", style = MaterialTheme.typography.titleMedium)
        Text("Tap to play video", style = MaterialTheme.typography.bodyMedium)
    }
}

/** Anything else (PDFs, etc.): decrypt then hand off to whatever app the user has for it. */
@Composable
private fun GenericFile(info: ComradeCore.MediaMessageInfo) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var loading by remember(info.eventId) { mutableStateOf(false) }
    var error by remember(info.eventId) { mutableStateOf<String?>(null) }

    fun openExternally() {
        loading = true
        error = null
        scope.launch {
            val opened = runCatching {
                val file = MediaCache.resolveFile(context, info)
                val uri = MediaCache.uriFor(context, file)
                Intent(Intent.ACTION_VIEW).apply {
                    setDataAndType(uri, info.mimeType.ifBlank { "application/octet-stream" })
                    addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                }
            }.mapCatching { intent ->
                context.startActivity(intent)
            }
            loading = false
            opened.onFailure { error = "Could not open this file" }
        }
    }

    OutlinedButton(onClick = ::openExternally, enabled = !loading) {
        if (loading) {
            CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
        } else {
            val ext = info.mimeType.substringAfterLast('/', "file")
            Text(error ?: "⬇ Open $ext")
        }
    }
}
