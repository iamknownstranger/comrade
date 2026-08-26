package mullu.comrade.ui

import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.animateColorAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SmallFloatingActionButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.core.content.FileProvider
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import mullu.comrade.R
import mullu.comrade.ComradeCore
import mullu.comrade.Notifier
import mullu.comrade.PresenceMonitor
import mullu.comrade.handoff.AttachmentHandoffManager
import mullu.comrade.media.VoiceRecorder
import mullu.comrade.together.LibraryResolver
import mullu.comrade.together.MediaLibraryAccess
import mullu.comrade.together.TogetherManager
import mullu.comrade.ui.theme.AvatarPalette
import mullu.comrade.ui.theme.ComradeRadii
import mullu.comrade.ui.theme.GlassElevation
import mullu.comrade.ui.theme.glassSurface
import uniffi.comrade_ui.PlayRoute

@Composable
fun PeerAvatar(
    title: String,
    modifier: Modifier = Modifier,
    seed: String = title,
    size: Dp = 46.dp,
) {
    val base = AvatarPalette[avatarColorIndex(seed, AvatarPalette.size)]
    Box(
        modifier = modifier
            .size(size)
            .clip(CircleShape)
            .background(
                Brush.verticalGradient(listOf(base.copy(alpha = 0.82f), base)),
            ),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = title.trimStart('@').take(1).uppercase().ifEmpty { "?" },
            style = MaterialTheme.typography.titleMedium,
            fontWeight = FontWeight.Bold,
            color = Color.White,
        )
    }
}

// ── Chat list ────────────────────────────────────────────────────────────────

/** Tappable banner atop the chat list linking to the message-requests inbox. */
@Composable
private fun RequestsBanner(count: Int, onClick: () -> Unit) {
    if (count <= 0) return
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable { onClick() }
            .background(MaterialTheme.colorScheme.secondaryContainer)
            .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("✉", style = MaterialTheme.typography.titleMedium)
        Text(
            "Message requests ($count)",
            style = MaterialTheme.typography.titleSmall,
            modifier = Modifier.weight(1f),
        )
        Text("›", style = MaterialTheme.typography.titleMedium)
    }
}

@Composable
fun ChatsScreen(
    chatTick: Int,
    requestTick: Int,
    onOpen: (peer: String, alias: String?, username: String?) -> Unit,
    onNewChat: () -> Unit,
    onOpenRequests: () -> Unit,
    modifier: Modifier = Modifier,
) {
    var conversations by remember { mutableStateOf<List<ComradeCore.ConversationInfo>?>(null) }
    var requestCount by remember { mutableStateOf(0) }
    val presenceNow by PresenceMonitor.presence.collectAsState()

    LaunchedEffect(chatTick) {
        conversations = withContext(Dispatchers.IO) {
            runCatching { ComradeCore.conversations() }.getOrDefault(emptyList())
        }
    }
    LaunchedEffect(chatTick, requestTick) {
        requestCount = withContext(Dispatchers.IO) {
            runCatching { ComradeCore.messageRequestsTyped().size }.getOrDefault(0)
        }
    }

    val list = conversations
    when {
        list == null -> Column(modifier.fillMaxSize()) {
            // §7.3: the real row geometry, pulsing, rather than a spinner over
            // a blank screen — the list's shape is known before it arrives.
            repeat(ComradeSkeletonRowCount) { ChatRowSkeleton() }
        }
        list.isEmpty() -> Column(
            modifier = modifier.fillMaxSize(),
        ) {
            RequestsBanner(requestCount, onOpenRequests)
            EmptyChats(onNewChat)
        }
        else -> LazyColumn(modifier = modifier.fillMaxSize()) {
            item { RequestsBanner(requestCount, onOpenRequests) }
            items(list, key = { it.peer }) { convo ->
                val title = peerTitle(convo.peer, convo.alias, convo.peerName)
                // A comrade's dot follows the live flow, so a beacon arriving
                // while the list is on screen moves it without a reload.
                val online = convo.comrade && (presenceNow[convo.peer]?.online ?: convo.online)
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable { onOpen(convo.peer, convo.alias, convo.peerName) }
                        .padding(horizontal = Spacing.space4, vertical = Spacing.space3),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(Spacing.space4),
                ) {
                    Box(contentAlignment = Alignment.BottomEnd) {
                        PeerAvatar(title, seed = convo.peer)
                        if (convo.comrade) PresenceDot(online, size = 12.dp)
                    }
                    Column(Modifier.weight(1f)) {
                        Text(
                            title,
                            style = MaterialTheme.typography.titleMedium,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        Text(
                            text = (if (convo.lastOutgoing) "You: " else "") + convo.lastMessage,
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                    Text(
                        relativeTime(convo.lastAt),
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.outline,
                    )
                }
                HorizontalDivider(
                    modifier = Modifier.padding(start = 76.dp),
                    color = MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.4f),
                )
            }
        }
    }
}

/**
 * Empty-state prompt for the chat list — §7.4: one muted line, one button.
 * This is first-run-empty specifically ("nothing here yet"), not a filtered-
 * to-nothing state — the chat list has no filter to distinguish it from.
 */
@Composable
private fun EmptyChats(onNewChat: () -> Unit) {
    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(Spacing.space8),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Text(
            "No chats yet — find someone by username, or share your key so they can find you.",
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
            modifier = Modifier.padding(bottom = Spacing.space4),
        )
        Button(onClick = onNewChat) { Text("Start a chat") }
    }
}

/** §7.3's row geometry for a chat-list entry, painted with [comradeSkeleton] instead of real content. */
@Composable
private fun ChatRowSkeleton() {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = Spacing.space4, vertical = Spacing.space3),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Spacing.space4),
    ) {
        Box(
            Modifier
                .size(46.dp)
                .comradeSkeleton(CircleShape),
        )
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Spacing.space1)) {
            Box(Modifier.fillMaxWidth(0.45f).height(16.dp).comradeSkeleton(RoundedCornerShape(ComradeRadii.sm)))
            Box(Modifier.fillMaxWidth(0.7f).height(12.dp).comradeSkeleton(RoundedCornerShape(ComradeRadii.sm)))
        }
        Box(Modifier.width(32.dp).height(12.dp).comradeSkeleton(RoundedCornerShape(ComradeRadii.sm)))
    }
}

/** §7.3's row geometry for a message-request row: avatar, two text lines, no trailing time. */
@Composable
private fun RequestRowSkeleton() {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = Spacing.space4, vertical = Spacing.space3),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Spacing.space4),
    ) {
        Box(Modifier.size(46.dp).comradeSkeleton(CircleShape))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Spacing.space1)) {
            Box(Modifier.fillMaxWidth(0.4f).height(16.dp).comradeSkeleton(RoundedCornerShape(ComradeRadii.sm)))
            Box(Modifier.fillMaxWidth(0.8f).height(12.dp).comradeSkeleton(RoundedCornerShape(ComradeRadii.sm)))
        }
    }
}

// ── New chat (find people) ───────────────────────────────────────────────────

@Composable
fun NewChatScreen(
    onOpen: (peer: String, alias: String?, username: String?) -> Unit,
    modifier: Modifier = Modifier,
) {
    var query by remember { mutableStateOf("") }
    var searching by remember { mutableStateOf(false) }
    var searched by remember { mutableStateOf(false) }
    var results by remember { mutableStateOf<List<ComradeCore.FoundProfile>>(emptyList()) }
    var contacts by remember { mutableStateOf<List<ComradeCore.ContactInfo>>(emptyList()) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()

    LaunchedEffect(Unit) {
        contacts = withContext(Dispatchers.IO) {
            runCatching { ComradeCore.contacts() }.getOrDefault(emptyList())
        }
    }

    val trimmed = query.trim()
    val isKey = trimmed.startsWith("npub1") && trimmed.length > 20

    fun search() {
        if (trimmed.isEmpty() || isKey) return
        searching = true
        error = null
        scope.launch {
            runCatching {
                withContext(Dispatchers.IO) { ComradeCore.searchProfilesTyped(trimmed) }
            }.onSuccess {
                results = it
                searched = true
                searching = false
            }.onFailure {
                error = it.message
                searching = false
            }
        }
    }

    fun startChat(npub: String, username: String?) {
        scope.launch {
            // Pin the key only (trust-on-first-use). The published @handle is
            // cached by the search itself; an alias stays the user's to set.
            val saved = withContext(Dispatchers.IO) {
                runCatching { ComradeCore.addContactTyped(npub, "") }.getOrNull()
            }
            if (saved == null) {
                error = "That doesn't look like a valid key."
            } else {
                onOpen(saved.npub, saved.alias.ifBlank { null }, username ?: saved.name)
            }
        }
    }

    LazyColumn(
        modifier = modifier
            .fillMaxSize()
            .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(Spacing.space3),
    ) {
        item {
            OutlinedTextField(
                value = query,
                onValueChange = { query = it; searched = false },
                label = { Text("@username or npub1… key") },
                singleLine = true,
                modifier = Modifier
                    .fillMaxWidth()
                    .testTag("newchat-query"),
            )
        }
        item {
            if (isKey) {
                Button(
                    onClick = { startChat(trimmed, null) },
                    modifier = Modifier.fillMaxWidth(),
                ) { Text("Start chat with ${shortNpub(trimmed)}") }
            } else {
                OutlinedButton(
                    onClick = { search() },
                    enabled = trimmed.isNotEmpty() && !searching,
                    modifier = Modifier.fillMaxWidth(),
                ) { Text(if (searching) "Searching…" else "Search") }
            }
        }
        item {
            Text(
                text = "Search asks public directory relays, so it only finds people " +
                    "who published their username. Names are not unique — always " +
                    "glance at the key. The safest way to connect is swapping " +
                    "npub keys directly.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        error?.let { msg ->
            item { Text(msg, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }
        }
        if (searched && results.isEmpty()) {
            item {
                Text(
                    "No one found under that name. They may not have published it — " +
                        "ask them for their npub key instead.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        items(results, key = { it.npub }) { found ->
            val title = found.name?.let { "@$it" } ?: shortNpub(found.npub)
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable { startChat(found.npub, found.name) }
                    .padding(vertical = Spacing.space2),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                PeerAvatar(title, seed = found.npub)
                Column(Modifier.weight(1f)) {
                    Text(title, style = MaterialTheme.typography.titleSmall)
                    Text(
                        shortNpub(found.npub) + (found.about?.let { " · $it" } ?: ""),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        fontFamily = FontFamily.Monospace,
                    )
                }
            }
        }
        if (contacts.isNotEmpty()) {
            item {
                Text(
                    "Contacts",
                    style = MaterialTheme.typography.titleSmall,
                    color = MaterialTheme.colorScheme.primary,
                    modifier = Modifier.padding(top = 8.dp),
                )
            }
            items(contacts, key = { "c:" + it.npub }) { contact ->
                val title = peerTitle(contact.npub, contact.alias, contact.name)
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable {
                            onOpen(contact.npub, contact.alias.ifBlank { null }, contact.name)
                        }
                        .padding(vertical = Spacing.space2),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    PeerAvatar(title, seed = contact.npub)
                    Column {
                        Text(title, style = MaterialTheme.typography.titleSmall)
                        Text(
                            shortNpub(contact.npub),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            fontFamily = FontFamily.Monospace,
                        )
                    }
                }
            }
        }
    }
}

// ── Conversation ─────────────────────────────────────────────────────────────

/** Delivery-status glyph shown on outgoing bubbles: ✓ sent, ✓✓ delivered/read. */
private fun statusGlyph(status: String?): String = when (status) {
    "read", "delivered" -> "✓✓"
    else -> "✓"
}

/** A conversation is a time-ordered merge of text messages and media attachments. */
private sealed interface ChatItem {
    val createdAt: Long

    /** Whether *this device* sent it — your own messages are never "unread". */
    val outgoing: Boolean

    /**
     * Stable list key. Media ids are namespaced so a media event id can never
     * collide with a message id. Also used to remember the unread boundary by
     * identity rather than by index, so a backfill of older history inserted
     * above it cannot slide the divider onto the wrong message.
     */
    val key: String

    /**
     * The nostr event id a reply points at.
     *
     * **Not** [key]: the namespacing that keeps the list keys apart would make
     * a reply address `media:abc…`, which is not an event and would never
     * resolve on the other side. A message and an attachment are both ordinary
     * events, which is why replying to media needs no core change —
     * `send_dm_reply` tags whatever id it is given.
     */
    val eventId: String

    /** One line naming this item, for a reply chip or a quoted preview. */
    val preview: String

    data class TextItem(val msg: ComradeCore.MessageInfo) : ChatItem {
        override val createdAt get() = msg.createdAt
        override val outgoing get() = msg.outgoing
        override val key get() = msg.id
        override val eventId get() = msg.id
        override val preview get() = msg.content
    }

    data class MediaItem(val info: ComradeCore.MediaMessageInfo) : ChatItem {
        override val createdAt get() = info.createdAt
        override val outgoing get() = info.outgoing
        override val key get() = "media:${info.eventId}"
        override val eventId get() = info.eventId
        override val preview get() = mediaQuoteLabel(info.mimeType, info.caption)
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
fun ConversationScreen(
    peer: String,
    chatTick: Int,
    /**
     * Bumped when the *peer* reorganised the conversation — see
     * `ChatEventRouter.topicTick`. Separate from [chatTick] because rebuilding
     * the topic counts is a full history scan and a chat tick fires on every
     * message and receipt.
     */
    topicTick: Int = 0,
    /**
     * Open the file picker for a `/play` that knows what it wants and could not
     * find it on this phone.
     *
     * Hoisted to the activity because that is where the picker and its
     * persistable-permission handling already live — the same launcher the
     * Together button in the header uses, so a file arriving by either route
     * starts a session identically. Without it this screen could only *name* a
     * place to go, which is one gesture too many for "listen to this with me".
     */
    onPickTogetherFile: (peer: String, label: String) -> Unit = { _, _ -> },
    modifier: Modifier = Modifier,
) {
    var messages by remember { mutableStateOf<List<ComradeCore.MessageInfo>>(emptyList()) }
    var mediaItems by remember { mutableStateOf<List<ComradeCore.MediaMessageInfo>>(emptyList()) }
    // A TextFieldValue rather than a String: the emoji picker inserts at the
    // caret, which needs the selection.
    var draft by remember { mutableStateOf(TextFieldValue()) }

    /**
     * The one place the draft changes by hand, so typing and an emoji
     * insertion tell the core the same thing. It needs to know a composer
     * holds unsent text before it can ever tell a comrade one was given up on
     * (`comrade_core::nudge`) — and nothing about the text itself goes with it.
     *
     * Whitespace-only counts as empty, because [send] would not send it either.
     */
    fun editDraft(next: TextFieldValue) {
        draft = next
        if (next.text.isBlank()) ComradeCore.abandonDraft(peer) else ComradeCore.noteDraft(peer)
    }

    // ── In-chat commands (grammar in `comrade_core::command`) ────────────────
    //
    // The `/` picker's rows, and whether the composer currently holds a private
    // aside. Both are derived from the draft rather than stored, so they cannot
    // fall out of step with what is actually in the box.
    val commandCatalog = remember { runCatching { ComradeCore.chatCommandCatalog() }.getOrDefault(emptyList()) }
    val pickerRows = remember(draft.text, commandCatalog) {
        ChatCommands.pickerRows(draft.text, commandCatalog)
    }
    // Decided from the raw text, so it is right the moment `@tara ` is typed —
    // before there is anything to parse. A private thing that looks like a
    // message is how somebody sends one by accident, and now the reverse is
    // possible too: `@tara` reaches the other person and `/tara` does not.
    val taraAudience = remember(draft.text) { ChatCommands.taraDraft(draft.text) }
    var commandNote by remember { mutableStateOf<String?>(null) }
    // Which contact a user picked for an ambiguous `@handle`, by handle. Kept for
    // the life of the conversation rather than the draft: someone who has just
    // said which "ana" they meant should not be asked again on the next command.
    // `ChatCommands.withChoices` refuses a pin that no longer names a candidate,
    // so a stale one cannot retarget a message.
    var mentionChoices by remember(peer) { mutableStateOf<Map<String, String>>(emptyMap()) }
    // The question the composer is currently asking about a handle, if any. Keyed
    // on `peer` like the rest of the per-thread state: a question raised in one
    // conversation means nothing in the next.
    var choosing by remember(peer) { mutableStateOf<ComposerPlan.Choose?>(null) }
    // A question about an ambiguous handle belongs to the text that raised it, so
    // editing the box withdraws it rather than leaving a chooser hanging over a
    // draft it no longer describes. Keyed on the text, so it does not fire on the
    // recomposition that *shows* the chooser — only when the draft actually moves.
    LaunchedEffect(draft.text) { choosing = null }
    var emojiOpen by remember { mutableStateOf(false) }
    var sending by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    // A reply target is any chat item, text or attachment: the `e` tag does not
    // care which kind of event it names, so "reply to that photo" needs nothing
    // from the core that "reply to that message" did not already have.
    var replyingTo by remember { mutableStateOf<ChatItem?>(null) }
    // Reactions on this thread, and which message a long press is acting on.
    // Reloaded by the same effect as the messages, and re-read after every
    // toggle so a chip reflects what the store actually holds rather than what
    // the UI hoped for.
    var reactions by remember(peer) { mutableStateOf<List<ComradeCore.ReactionInfo>>(emptyList()) }
    var actingOn by remember { mutableStateOf<ChatItem?>(null) }
    // The full picker, opened from the reaction row's "+" for anything outside
    // the quick six. Held separately from `actingOn` because the action sheet
    // closes when it opens, and the target has to outlive that.
    var pickingFor by remember { mutableStateOf<ChatItem?>(null) }
    // ── Threads and topics (see docs/CHAT_THREADS.md) ────────────────────────
    //
    // Three pieces of state, and they are separate because they are three
    // questions: which thread is open, whether the topic list is open, and
    // whether the topic list is currently a *destination* for a filing rather
    // than a place to read. Folding the third into the second would make the
    // sheet's every row mean two things at once.
    var threadOpenFor by remember(peer) { mutableStateOf<String?>(null) }
    var topicsOpen by remember(peer) { mutableStateOf(false) }
    var filingMessage by remember(peer) { mutableStateOf<String?>(null) }
    // Bumped by this device's own filings, which raise no BridgeEvent — that
    // variant reports the peer's changes. Added to `topicTick` so both sources
    // reload the same sheets.
    var localTopicTick by remember(peer) { mutableStateOf(0) }
    // Just enough to decide whether the conversation has any threads worth
    // offering a way into. Deliberately not the full list: the sheets read
    // their own, and a bar that held one would keep a second copy in step.
    var hasThreads by remember(peer) { mutableStateOf(false) }
    var unreadThreads by remember(peer) { mutableStateOf(0) }
    var attaching by remember { mutableStateOf(false) }
    // Voice notes: tap the action icon to record, tap again to send.
    var recording by remember { mutableStateOf(false) }
    var voiceSending by remember { mutableStateOf(false) }
    // Keyed on `peer` so switching conversations resets the scroll bookkeeping.
    var loadedOnce by remember(peer) { mutableStateOf(false) }
    var newMessagesBelow by remember(peer) { mutableStateOf(false) }
    // The unread boundary for this visit, held by item key rather than index so
    // a backfill of older history above it can't slide the divider onto the
    // wrong message. Captured once on open and deliberately *not* recomputed as
    // the watermark advances — Telegram leaves the line where you found it for
    // the rest of the visit, which is what makes it useful to read down to.
    var unreadBoundaryKey by remember(peer) { mutableStateOf<String?>(null) }
    // The item flashing because its quote was tapped to reach it. Keyed on the
    // item key, not the index, for the same reason as the unread boundary above.
    var highlightKey by remember(peer) { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val listState = rememberLazyListState()
    val context = LocalContext.current
    val clipboard = LocalClipboardManager.current
    val recorder = remember { VoiceRecorder(context) }
    // What this device can actually capture, probed once. Capability-gated so a
    // tablet with no camera gets a mic and nothing to swap to, rather than a
    // control that fails on tap.
    val availableModes = remember {
        availableCaptureModes(
            canRecordAudio = context.packageManager.hasSystemFeature(PackageManager.FEATURE_MICROPHONE),
            canTakePhoto = context.packageManager.hasSystemFeature(PackageManager.FEATURE_CAMERA_ANY),
            canRecordVideo = context.packageManager.hasSystemFeature(PackageManager.FEATURE_CAMERA_ANY),
        )
    }
    var captureMode by remember { mutableStateOf(availableModes.firstOrNull()) }
    // MediaRecorder holds the mic while active; a composition that leaves the
    // conversation mid-record (back-navigation) must not leak it.
    DisposableEffect(Unit) { onDispose { recorder.cancel() } }
    val requestMicPermission = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { /* Granted state is re-checked on the next press; nothing to do here. */ }

    // Text and media interleaved in one time-ordered thread, like a real chat.
    // Named `chatItems`, not `items`, to avoid shadowing the LazyListScope
    // `items(...)` DSL function called with it below.
    val chatItems = remember(messages, mediaItems) {
        (messages.map(ChatItem::TextItem) + mediaItems.map(ChatItem::MediaItem))
            .sortedBy { it.createdAt }
    }
    // Quick lookup so a bubble carrying reply_to can show a quoted preview.
    // Keyed over the merged thread, not just the messages: a reply to an
    // attachment resolves to that attachment, and quoting it says what it was.
    val byId = remember(chatItems) { chatItems.associateBy { it.eventId } }
    // Chips per message, grouped once per reaction change rather than per bubble
    // per recomposition. `outgoing` is the "yours" marker rather than the local
    // npub, which this screen has no reason to hold: the core already decided
    // which rows this device sent.
    val chipsByTarget = remember(reactions) {
        reactions
            .groupBy { it.targetId }
            .mapValues { (_, rows) ->
                summariseReactions(
                    rows.map {
                        ReactionRow(
                            targetId = it.targetId,
                            // Every outgoing row is collapsed to one synthetic
                            // reactor id so `mine` is exactly "this device sent
                            // one", with no npub lookup.
                            reactor = if (it.outgoing) MINE_REACTOR else it.reactor,
                            emoji = it.emoji,
                        )
                    },
                    mineReactor = MINE_REACTOR,
                )
            }
    }
    // The emoji this device has on a given message, for the sheet's highlight.
    val myReactionByTarget = remember(reactions) {
        reactions.filter { it.outgoing }.associate { it.targetId to it.emoji }
    }

    /**
     * Toggle a reaction and re-read the thread's reactions from the store.
     *
     * Re-reading rather than patching the list in place: the core owns whether a
     * tap added, replaced or withdrew (see `ComradeRuntime::toggle_reaction`), and
     * a local guess at which of the three happened is a second implementation of
     * the rule that exists precisely so there is only one.
     */
    fun toggleReaction(item: ChatItem, emoji: String) {
        scope.launch {
            val (failure, fresh) = withContext(Dispatchers.IO) {
                val failure = runCatching {
                    ComradeCore.toggleReactionTyped(peer, item.eventId, emoji)
                }.exceptionOrNull()
                failure to runCatching { ComradeCore.reactions(peer) }.getOrNull()
            }
            if (failure != null) error = failure.message ?: "Could not react."
            fresh?.let { reactions = it }
        }
    }

    /**
     * Go to the message a reply is quoting, and flash it on arrival.
     *
     * The flash is not decoration. The scroll lands the target somewhere in a
     * screenful of other messages and says nothing about which one it was; without
     * the highlight the jump reads as the thread having lost your place. The
     * jump-to-latest button appears on the way, which is the way back.
     *
     * Does nothing when the original is not in the loaded thread — see
     * [indexOfEventId]. Staying put is the honest outcome there.
     */
    fun goToQuoted(targetId: String?) {
        val index = indexOfEventId(chatItems.map { it.eventId }, targetId) ?: return
        val key = chatItems[index].key
        highlightKey = key
        scope.launch {
            listState.animateScrollToItem(index)
            delay(QUOTE_HIGHLIGHT_MS)
            // Only the newest tap gets to end the flash, so tapping through a
            // chain of replies keeps highlighting where you actually are.
            if (highlightKey == key) highlightKey = null
        }
    }

    // `chatTick` is a GLOBAL event tick — it fires for activity in any
    // conversation, and repeatedly while this one is open. So a reload must
    // not yank a reader who scrolled up in history back to the bottom:
    // auto-scroll only on first load or when they were already near it,
    // otherwise light up the jump-to-latest button instead.
    // Whether there is anything to open a thread sheet *for*, and how much of it
    // is unread. Rides `chatTick` as well as the topic ticks, because a reply
    // arriving turns a message into a thread and that is a chat event.
    LaunchedEffect(peer, chatTick, topicTick, localTopicTick) {
        val rows = withContext(Dispatchers.IO) {
            runCatching { ComradeCore.threads(peer) }.getOrDefault(emptyList())
        }
        val visible = mullu.comrade.topic.TopicDecisions.threadsFor(rows)
        hasThreads = visible.isNotEmpty() ||
            withContext(Dispatchers.IO) {
                runCatching { ComradeCore.topics(peer).isNotEmpty() }.getOrDefault(false)
            }
        unreadThreads = mullu.comrade.topic.TopicDecisions.unreadThreadCount(visible)
    }

    LaunchedEffect(peer, chatTick) {
        val loaded = withContext(Dispatchers.IO) {
            Triple(
                runCatching { ComradeCore.messages(peer) }.getOrDefault(emptyList()),
                runCatching { ComradeCore.media(peer) }.getOrDefault(emptyList()),
                // Reactions ride the same tick: an IncomingReaction bumps
                // chatTick, and reading them here rather than in their own
                // effect keeps a bubble and its chips from being drawn a frame
                // apart.
                runCatching { ComradeCore.reactions(peer) }.getOrDefault(emptyList()),
            )
        }
        val (msgs, media) = loaded.first to loaded.second
        reactions = loaded.third
        val grew = msgs.size + media.size > messages.size + mediaItems.size
        val wasNearBottom = isNearBottom(
            lastVisibleIndex = listState.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: -1,
            totalCount = listState.layoutInfo.totalItemsCount,
        )
        messages = msgs
        mediaItems = media
        // The merged list this effect is about to scroll — recomputed here
        // rather than read from `chatItems`, which is derived from the state
        // set two lines up and so still holds the previous composition's value.
        val merged = (msgs.map(ChatItem::TextItem) + media.map(ChatItem::MediaItem))
            .sortedBy { it.createdAt }
        val last = merged.size - 1
        when {
            last < 0 -> {}
            !loadedOnce -> {
                // One call both records that the thread is now read and reports
                // where the reader had got to, so there is no window where the
                // answer has already been overwritten.
                val previous = withContext(Dispatchers.IO) {
                    runCatching { ComradeCore.markConversationReadTyped(peer) }.getOrDefault(0L)
                }
                val firstUnread = firstUnreadIndex(
                    createdAt = merged.map { it.createdAt },
                    outgoing = merged.map { it.outgoing },
                    lastReadAt = previous,
                )
                unreadBoundaryKey = firstUnread?.let { merged[it].key }
                // Open where they left off, not at the newest message.
                listState.scrollToItem(firstUnread ?: last)
                loadedOnce = true
            }
            grew && wasNearBottom -> {
                listState.scrollToItem(last)
                // They can see these, so they are read — receipt and watermark.
                withContext(Dispatchers.IO) {
                    runCatching { ComradeCore.markConversationReadTyped(peer) }
                }
            }
            // Scrolled up in history: deliberately *not* marked read. They have
            // not seen these, so neither the peer's receipt nor the watermark
            // should claim otherwise — the jump-to-latest button is the signal,
            // and marking read here would cost them the divider next visit.
            grew -> newMessagesBelow = true
        }
    }

    // Clear any pending notification for this peer. The read receipt itself
    // rides along with the load effect above, which needs its return value.
    LaunchedEffect(peer) { Notifier.clearForPeer(context, peer) }

    // Walking away from a half-written message is the other way to abandon it,
    // and the one the person is least likely to come back from. Keyed on `peer`
    // so switching conversations reports the thread being left, not the one
    // arriving; an empty composer makes this a no-op.
    DisposableEffect(peer) { onDispose { ComradeCore.abandonDraft(peer) } }

    // The library permission and the command that needs it are two halves of a
    // cycle: `/play` finds it missing, and the answer has to run `/play` again.
    // Broken by going through state rather than by calling across it — the
    // command only records what it wanted, the effect below puts the question,
    // and the launcher's result runs the command once more. Keyed on `peer` so a
    // question raised in one conversation cannot answer into the next.
    var pendingLibraryAsk by remember(peer) { mutableStateOf<String?>(null) }

    /**
     * Act on an in-chat command, or return false to let [send] deliver the text.
     *
     * The grammar is Rust ([ComradeCore.parseChatCommand]) and the decision is
     * [ChatCommands.planFor]; this only carries it out. Everything that touches
     * a relay runs on IO, and the composer is cleared only once the call has
     * actually succeeded — a command that failed must leave the text where the
     * user can try again.
     */
    fun runCommand(text: String): Boolean {
        val command = runCatching { ComradeCore.parseChatCommand(text) }.getOrNull()
        if (command == null) {
            // **Fail closed.** Returning false here would hand the text to
            // [send], and for `/tara i can't stand my brother` that means
            // sending somebody their own private thought. So anything that
            // *looks* like a command is refused rather than delivered when the
            // grammar is unreachable — `taraDraft` is pure Kotlin and cannot
            // itself fail, which is why it can be trusted at this point.
            if (ChatCommands.taraDraft(text) != null || text.startsWith("/")) {
                commandNote = "Couldn't read that command — nothing was sent."
                return true
            }
            return false
        }
        if (command is uniffi.comrade_core.ChatCommand.Plain ||
            command is uniffi.comrade_core.ChatCommand.Pay
        ) {
            return false
        }
        val mentions = ChatCommands.withChoices(
            runCatching { ComradeCore.resolveMentions(text) }.getOrDefault(emptyList()),
            mentionChoices,
        )
        // The reply target is what `/assign` files. Any message in the thread
        // will do — core walks up to the root — so "whatever you are replying
        // to" is the honest answer to "which thread did you mean", and it is
        // the only one the composer actually has.
        val plan = ChatCommands.planFor(command, mentions, replyingTo?.eventId)

        // Empties the composer. Deliberately does **not** touch [commandNote]:
        // it used to, and every `/task` confirmation was erased on the line
        // after it was set, so the command appeared to do nothing at all.
        fun clearDraft() {
            editDraft(TextFieldValue())
        }

        // Set and cleared in one place, so a chooser cannot outlive the draft
        // that raised it: any other plan means the question no longer applies.
        choosing = plan as? ComposerPlan.Choose

        when (plan) {
            is ComposerPlan.Send -> return false

            // Say why and keep the text: a command the user has to retype is a
            // command they stop using.
            is ComposerPlan.Explain -> commandNote = plan.message

            // Nothing to do here — the chooser is rendered from [choosing] above,
            // and picking a row re-runs this whole function with the choice
            // applied. The text stays in the box; it is still what the user meant.
            is ComposerPlan.Choose -> Unit

            is ComposerPlan.Help -> commandNote = commandCatalog.joinToString("\n") {
                "/${it.name} ${it.argument} — ${it.help}"
            }

            is ComposerPlan.Aside -> {
                sending = true
                scope.launch {
                    runCatching { withContext(Dispatchers.IO) { ComradeCore.taraAsideTyped(plan.text) } }
                        .onSuccess { reply ->
                            // Shown in the composer's own note, not as a chat
                            // bubble: this never went anywhere, and putting it in
                            // the thread would make it look as though it had.
                            //
                            // The helplines were missing here until now: an aside
                            // that tripped the distress detector showed the reply
                            // alone, so `AUDIT.md` §8's gate held on the Tara tab
                            // and not in the composer that can reach the same
                            // engine. The reply text asks the reader to call
                            // somebody; leaving out *who* was the whole failure.
                            val helplines = if (reply.crisis) {
                                ChatCommands.crisisLines(
                                    withContext(Dispatchers.IO) {
                                        runCatching { ComradeCore.taraCrisisResources() }
                                            .getOrDefault(emptyList())
                                    },
                                )
                            } else {
                                ""
                            }
                            commandNote = listOf(reply.text, helplines)
                                .filter { it.isNotEmpty() }
                                .joinToString("\n\n")
                            editDraft(TextFieldValue())
                            sending = false
                        }
                        .onFailure {
                            commandNote = it.message ?: "Tara could not answer."
                            sending = false
                        }
                }
            }

            is ComposerPlan.TaraHere -> {
                sending = true
                scope.launch {
                    runCatching {
                        withContext(Dispatchers.IO) {
                            ComradeCore.taraInChatTyped(peer, plan.text)
                        }
                    }
                        .onSuccess { turn ->
                            if (turn.keptPrivate) {
                                // Core refused to publish this one, and the note
                                // has to say so: the user asked in the open and
                                // is owed the fact that nothing was sent, or they
                                // will assume the other person read it. The
                                // helplines come with it — `AUDIT.md` §8's gate
                                // holds wherever a distress reply is shown.
                                val helplines = if (turn.crisis) {
                                    ChatCommands.crisisLines(
                                        withContext(Dispatchers.IO) {
                                            runCatching { ComradeCore.taraCrisisResources() }
                                                .getOrDefault(emptyList())
                                        },
                                    )
                                } else {
                                    ""
                                }
                                commandNote = listOf(
                                    turn.reply,
                                    "(Kept between us — this one wasn't sent.)",
                                    helplines,
                                ).filter { it.isNotEmpty() }.joinToString("\n\n")
                            } else {
                                // Both messages are already stored; appending
                                // them beats a reload, which would race the relay
                                // and could show the answer before the question.
                                messages = messages +
                                    listOfNotNull(turn.asked, turn.answered)
                                commandNote = null
                                scope.launch {
                                    listState.scrollToItem(
                                        messages.size + mediaItems.size - 1,
                                    )
                                }
                            }
                            clearDraft()
                            sending = false
                        }
                        .onFailure {
                            commandNote = it.message ?: "Tara could not answer."
                            sending = false
                        }
                }
            }

            is ComposerPlan.OpenTopics -> {
                topicsOpen = true
                filingMessage = null
                clearDraft()
            }

            is ComposerPlan.AssignTopic -> {
                sending = true
                scope.launch {
                    runCatching {
                        withContext(Dispatchers.IO) {
                            ComradeCore.assignThreadTyped(peer, plan.messageId, plan.slug)
                        }
                    }
                        .onSuccess { filed ->
                            // Filing produces no bubble on either side, so this
                            // note is the only evidence the command did
                            // anything — see `comrade_core::topic`.
                            commandNote = filed.topicSlug
                                ?.let { context.getString(R.string.topics_filed, it) }
                                ?: context.getString(R.string.topics_unfiled_note)
                            localTopicTick += 1
                            // The reply chip goes with it: the message was
                            // selected in order to file it, and leaving it armed
                            // would make the next thing typed a reply nobody
                            // asked for.
                            replyingTo = null
                            clearDraft()
                            sending = false
                        }
                        .onFailure {
                            commandNote = it.message ?: "Could not file that."
                            sending = false
                        }
                }
            }

            is ComposerPlan.Task -> {
                sending = true
                scope.launch {
                    runCatching {
                        withContext(Dispatchers.IO) {
                            ComradeCore.assignTaskTyped(plan.peer, plan.text)
                        }
                    }
                        .onSuccess {
                            // Names where the list is: this screen owns no nav
                            // state, so it cannot open Tasks itself, and a
                            // confirmation about a list you cannot find is the
                            // gap this screen used to have.
                            commandNote = if (plan.peer != null) {
                                "Asked them — it's under Tasks in the menu."
                            } else {
                                "Added to your list — it's under Tasks in the menu."
                            }
                            clearDraft()
                            sending = false
                        }
                        .onFailure {
                            commandNote = it.message ?: "Could not add that."
                            sending = false
                        }
                }
            }

            is ComposerPlan.Offer -> {
                sending = true
                scope.launch {
                    runCatching {
                        withContext(Dispatchers.IO) {
                            ComradeCore.offerActionTyped(plan.action, plan.peers)
                        }
                    }
                        .onSuccess { outcome ->
                            // A deliberate command that silently did nothing
                            // reads as a bug, so say which of the three reasons
                            // applied rather than guessing at the friendliest.
                            commandNote = when {
                                outcome.sent.isNotEmpty() -> null
                                outcome.notComrades.isNotEmpty() ->
                                    "Mark them a comrade first — this only goes to comrades."
                                outcome.onCooldown.isNotEmpty() ->
                                    "They were told recently — leaving them be for now."
                                else -> "Couldn't reach them just now."
                            }
                            if (outcome.sent.isNotEmpty()) clearDraft()
                            sending = false
                        }
                        .onFailure {
                            commandNote = it.message ?: "Could not send that."
                            sending = false
                        }
                }
            }

            is ComposerPlan.Open -> {
                // Navigating out of a conversation from its own composer needs a
                // host that owns the nav state; until `ConversationScreen` takes
                // one, say where it is rather than doing nothing.
                commandNote = "${ChatCommands.labelFor(plan.action)} is on the ${
                    if (plan.action == uniffi.comrade_core.AppAction.BREATHE ||
                        plan.action == uniffi.comrade_core.AppAction.FOCUS ||
                        plan.action == uniffi.comrade_core.AppAction.READ
                    ) {
                        "Focus"
                    } else {
                        plan.action.name.lowercase().replaceFirstChar { c -> c.uppercase() }
                    }
                } tab."
            }

            is ComposerPlan.Play -> {
                sending = true
                scope.launch {
                    // Three steps, and only the middle one is this frontend's:
                    // core says what the query names, `MediaStore` says whether
                    // a copy is here, and core turns those two answers into a
                    // route. Nothing about *when* a session may open is decided
                    // in this file.
                    val target = withContext(Dispatchers.IO) {
                        runCatching { ComradeCore.playQuery(plan.query, plan.service) }.getOrNull()
                    }
                    if (target == null) {
                        // Fail closed, like the grammar above: a query nobody
                        // resolved must not fall through to a file picker.
                        commandNote = "Couldn't work out what to play — nothing was sent."
                        sending = false
                        return@launch
                    }
                    // Whether a copy on this phone would change the answer at
                    // all, asked of core rather than decided here: for a Spotify
                    // link the route is "open it there" either way, and putting
                    // a permission dialog in front of someone to reach the same
                    // sentence is the kind of prompt that teaches people to
                    // refuse. Only a route that actually differs earns the ask.
                    val libraryWouldMatter = target.recording != null &&
                        withContext(Dispatchers.IO) {
                            val withoutCopy =
                                ComradeCore.playRoute(target.plan, false, target.link)
                            val withCopy =
                                ComradeCore.playRoute(target.plan, true, target.link)
                            withoutCopy != null && withCopy != null && withoutCopy != withCopy
                        }
                    // "Not allowed to look" and "looked, not there" are different
                    // problems with different next steps, so they are told apart
                    // before either sentence is chosen. `askedBefore` defaults to
                    // true if the preference cannot be read: an unreadable answer
                    // must not turn into an endless prompt.
                    val step = MediaLibraryAccess.next(
                        granted = runCatching { LibraryResolver.mayRead(context) }
                            .getOrDefault(false),
                        askedBefore = runCatching { MediaLibraryAccess.asked(context) }
                            .getOrDefault(true),
                    )
                    if (libraryWouldMatter && step == MediaLibraryAccess.Step.Ask) {
                        // No sentence: the system dialog is about to say why, and
                        // the whole command re-runs on the answer. The draft is
                        // deliberately left alone so a refusal loses nothing.
                        pendingLibraryAsk = text
                        sending = false
                        return@launch
                    }
                    val mayLook = step == MediaLibraryAccess.Step.Look
                    val found = withContext(Dispatchers.IO) {
                        target.recording?.takeIf { mayLook }?.let { want ->
                            // Duration is unknown for words the user typed, and
                            // `match_score` treats an unknown length as neutral
                            // rather than as a mismatch — that is what lets
                            // "Kun Faya Kun" match at all.
                            runCatching { LibraryResolver.resolve(context, want, 0L) }.getOrNull()
                        }
                    }
                    val route = withContext(Dispatchers.IO) {
                        ComradeCore.playRoute(target.plan, found != null, target.link)
                    }
                    if (route == null) {
                        commandNote = "Couldn't work out what to play — nothing was sent."
                        sending = false
                        return@launch
                    }
                    // Held locally rather than written straight to [commandNote]:
                    // a note is only cleared when the user taps it, so a
                    // conditional write would let the previous command's
                    // sentence stand in for this one's.
                    var failed: String? = null
                    // A label, not an npub: an invitation that says "npub1…"
                    // wants to listen with you is unreadable. Needed by both
                    // routes that reach a session, so it is read once.
                    val label = withContext(Dispatchers.IO) {
                        runCatching { ComradeCore.contacts() }.getOrNull()
                            ?.firstOrNull { it.npub == peer }
                            ?.let { peerTitle(it.npub, it.alias, it.name) }
                            ?: shortNpub(peer)
                    }
                    if (route == PlayRoute.START_TOGETHER && found != null) {
                        runCatching {
                            TogetherManager.start(context, peer, label, found.uri, found.recording)
                        }
                            .onSuccess { clearDraft() }
                            .onFailure { failed = it.message ?: "Could not start that." }
                    }
                    // The other half of the same gesture. We know what they
                    // asked for and this phone cannot hand it over, so the
                    // picker *is* the next step — naming a screen to go and
                    // open was asking them to say the same thing twice.
                    //
                    // Opened whether or not the library could be read: "no copy
                    // here" and "not allowed to look" are different sentences
                    // (below) but the same next action, and the file picker
                    // needs no permission either way.
                    if (route == PlayRoute.ASK_FOR_FILE) {
                        clearDraft()
                        onPickTogetherFile(peer, label)
                    }
                    // The third route that reaches a session, and the only one
                    // that needs nothing from either device — no file, no
                    // account, no library permission. The id comes from core's
                    // own parse (`PlayPlan::OpenNow` sets `content`), never from
                    // re-reading the link here: one parser, and it is the one
                    // that already decided this was a video.
                    if (route == PlayRoute.PLAY_EMBED) {
                        val videoId =
                            (target.content as? uniffi.comrade_core.TogetherContent.Youtube)?.videoId
                        if (videoId == null) {
                            // Core said embed and carried no id. Refuse rather
                            // than open a player with nothing in it — the same
                            // fail-closed call the grammar above makes.
                            failed = "Couldn't work out which video that is — nothing was sent."
                        } else {
                            runCatching { TogetherManager.startEmbed(context, peer, label, videoId) }
                                .onSuccess { clearDraft() }
                                .onFailure { failed = it.message ?: "Could not start that." }
                        }
                    }
                    commandNote = when {
                        failed != null -> failed
                        route == PlayRoute.ASK_FOR_FILE && !mayLook -> ChatCommands.LIBRARY_UNSEEN
                        else -> ChatCommands.playNote(route, target.service, plan.query)
                    }
                    sending = false
                }
            }
        }
        return true
    }

    // The other half of the cycle described above [runCommand]. Declared here
    // rather than beside the other launchers because its result calls
    // [runCommand], which has to exist by this point.
    val askToReadLibrary = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) {
        val retry = pendingLibraryAsk
        pendingLibraryAsk = null
        // Recorded whatever the answer was: what makes a second ask pointless is
        // that the question was already put. Android stops showing the dialog
        // after a refusal, so asking again would be a button that does nothing.
        runCatching { MediaLibraryAccess.rememberAsked(context) }
        // Granted or refused, the command runs again and takes the route its
        // answer allows — and cannot ask a second time, because `asked` is now
        // true, so this terminates.
        if (retry != null) runCommand(retry)
    }
    LaunchedEffect(pendingLibraryAsk) {
        if (pendingLibraryAsk == null) return@LaunchedEffect
        askToReadLibrary.launch(MediaLibraryAccess.permissionFor(Build.VERSION.SDK_INT))
    }

    fun send() {
        val text = draft.text.trim()
        if (text.isEmpty() || sending) return
        // A command is intercepted before anything reaches the DM path — an
        // aside in particular must never be able to reach `sendDmReplyTyped`.
        if (runCommand(text)) return
        sending = true
        error = null
        val replyId = replyingTo?.eventId
        scope.launch {
            runCatching {
                withContext(Dispatchers.IO) { ComradeCore.sendDmReplyTyped(peer, text, replyId) }
            }.onSuccess { sent ->
                draft = TextFieldValue()
                replyingTo = null
                sending = false
                messages = messages + sent
                scope.launch { listState.scrollToItem(messages.size + mediaItems.size - 1) }
            }.onFailure {
                sending = false
                error = it.message ?: "Could not send."
            }
        }
    }

    // ── Attachments (NIP-94 over the DM channel) ────────────────────────────
    //
    // Three steps, and the split between them is the point: obtain the bytes,
    // let the sender confirm them, then encrypt and upload. It used to be one
    // step — a pick went straight to the uploader — so the first sight of what
    // had been chosen was in one's own thread, already delivered.

    // The pick waiting on that confirmation; null whenever the sheet is closed.
    var pendingAttachment by remember { mutableStateOf<PendingAttachment?>(null) }

    // Encrypt + upload a confirmed attachment.
    //
    // The box is emptied only once the send has actually succeeded and only if
    // its text went along: a failed upload must leave what the person typed
    // where they typed it.
    fun sendAttachment(pending: PendingAttachment, caption: String) {
        if (attaching) return
        pendingAttachment = null
        // Past the hosted ceiling the file does not go through a host at all: it
        // is handed to the other device over a data channel, which the manager
        // owns so that leaving this screen does not kill a 400 MB transfer. It
        // deliberately produces no chat bubble — there is no NIP-94 event to make
        // one from — so the handoff panel is what reports it.
        if (pending.peerToPeer) {
            val uri = pending.uri
            if (uri == null) {
                error = "Lost track of that file — pick it again."
                return
            }
            val refusal = AttachmentHandoffManager.offer(
                context = context,
                peer = peer,
                uri = uri,
                fileName = pending.name,
                mimeType = pending.mime,
                caption = caption,
                sizeBytes = pending.sizeBytes,
            )
            if (refusal != null) {
                error = refusal
                return
            }
            error = null
            if (pending.consumesDraft) editDraft(TextFieldValue())
            return
        }
        // Non-null for every hosted attachment: the road was chosen from the size
        // and the bytes were read for exactly this call.
        val bytes = pending.bytes ?: run {
            error = "Lost track of that file — pick it again."
            return
        }
        attaching = true
        error = null
        scope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    ComradeCore.sendMediaBytesTyped(peer, pending.mime, caption, bytes)
                }
            }.onSuccess {
                attaching = false
                if (pending.consumesDraft) editDraft(TextFieldValue())
                // Render the real attachment inline — not a synthetic text line.
                mediaItems = mediaItems + it
                scope.launch { listState.scrollToItem(messages.size + mediaItems.size - 1) }
            }.onFailure {
                attaching = false
                error = it.message ?: "Could not send the attachment."
            }
        }
    }

    // Hold a freshly obtained attachment for confirmation — or refuse it here,
    // before the sheet, which is the whole reason the cap moved forward: it used
    // to surface after the upload had begun, discarding the caption the sender
    // had just written.
    //
    // The caption the sheet opens with is whatever is in the composer, per
    // [captionForAttachment] — Telegram's rule, minus the case where those words
    // are a half-written reply.
    fun offerAttachment(name: String, mime: String, bytes: ByteArray) {
        val refusal = attachmentRejection(name, bytes.size.toLong())
        if (refusal != null) {
            error = refusal
            return
        }
        error = null
        val replyPending = replyingTo != null
        pendingAttachment = PendingAttachment(
            name = name,
            mime = mime,
            bytes = bytes,
            uri = null,
            sizeBytes = bytes.size.toLong(),
            seedCaption = captionForAttachment(draft.text, replyPending),
            consumesDraft = captionConsumesDraft(draft.text, replyPending),
            // Anything held in memory took the hosted road to get here: the cap
            // is what made reading it safe.
            peerToPeer = false,
        )
    }

    // A pick too large for the hosted road. The bytes stay where they are — the
    // URI is what gets sent from, a chunk at a time — so nothing about a 400 MB
    // file is ever in memory on this side.
    fun offerLargeAttachment(name: String, mime: String, uri: Uri, sizeBytes: Long) {
        val refusal = peerToPeerAttachmentRejection(name, sizeBytes)
        if (refusal != null) {
            error = refusal
            return
        }
        error = null
        val replyPending = replyingTo != null
        pendingAttachment = PendingAttachment(
            name = name,
            mime = mime,
            bytes = null,
            uri = uri,
            sizeBytes = sizeBytes,
            seedCaption = captionForAttachment(draft.text, replyPending),
            consumesDraft = captionConsumesDraft(draft.text, replyPending),
            peerToPeer = true,
        )
    }

    val pickMedia = rememberLauncherForActivityResult(
        ActivityResultContracts.GetContent(),
    ) { uri ->
        if (uri == null || attaching || pendingAttachment != null) {
            return@rememberLauncherForActivityResult
        }
        error = null
        scope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    val described = describePickedFile(context, uri)
                    // The size decides the road, so it is worth two cheap
                    // questions before the expensive one: the provider's own
                    // column, then the descriptor's `statSize`. Reading a 1 GB
                    // video into memory to learn that it is a 1 GB video is what
                    // both of them avoid.
                    val declared = described.size ?: pickedFileLength(context, uri)
                    val peerToPeer = declared != null &&
                        ComradeCore.attachmentRouteForBytes(declared) ==
                        uniffi.comrade_core.AttachmentRoute.PEER_TO_PEER
                    if (peerToPeer) {
                        peerToPeerAttachmentRejection(described.name, declared ?: 0)
                            ?.let { throw IllegalStateException(it) }
                        // No bytes: the transfer reads them from the provider as
                        // the receiver asks for them.
                        return@withContext PickedFile(described.name, described.mime, declared, null)
                    }
                    if (declared != null) {
                        attachmentRejection(described.name, declared)
                            ?.let { throw IllegalStateException(it) }
                    }
                    val bytes = context.contentResolver.openInputStream(uri)?.use { it.readBytes() }
                        ?: throw IllegalStateException("Could not read the file.")
                    PickedFile(described.name, described.mime, declared, bytes)
                }
            }.onSuccess { picked ->
                val bytes = picked.bytes
                if (bytes != null) {
                    offerAttachment(picked.name, picked.mime, bytes)
                } else {
                    offerLargeAttachment(picked.name, picked.mime, uri, picked.size ?: 0)
                }
            }.onFailure {
                error = it.message ?: "Could not read the file."
            }
        }
    }

    // ── Camera capture (the composer's photo/video modes) ───────────────────
    //
    // Written into `cache/capture/`, which `file_paths.xml` exposes to the
    // camera app through the existing FileProvider. A separate subdirectory
    // from `cache/media/` on purpose: that one holds decrypted attachments and
    // is swept by `purgeDecryptedMedia`, which must not race a capture that is
    // still being written.
    fun newCaptureFile(extension: String): File {
        val dir = File(context.cacheDir, "capture").apply { mkdirs() }
        return File(dir, "cap-${System.nanoTime()}.$extension")
    }

    fun captureUri(file: File): Uri = FileProvider.getUriForFile(
        context,
        "${context.packageName}.fileprovider",
        file,
    )

    // Read the captured file, offer it for confirmation, and delete the
    // plaintext immediately — the same rule voice notes follow (AUDIT S-4): a
    // decrypted clip must not linger in the cache. Deleting at *read* time
    // rather than after the send means backing out of the preview drops it too,
    // which the old send-immediately path never had to think about.
    //
    // The bytes then live only in the pending attachment, in memory, until the
    // sheet resolves one way or the other.
    fun offerCapturedFile(file: File, mime: String) {
        if (attaching || pendingAttachment != null) return
        error = null
        scope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    try {
                        file.readBytes()
                    } finally {
                        file.delete()
                    }
                }
            }.onSuccess { bytes ->
                // No name: the camera's temp file is called `cap-<nanos>.jpg`,
                // which tells the sender nothing they did not already know.
                offerAttachment("", mime, bytes)
            }.onFailure {
                error = it.message ?: "Could not read the capture."
            }
        }
    }

    // The pending capture target: the contracts hand back only success/failure,
    // not the file, so the launcher and the result have to agree out of band.
    var pendingCapture by remember { mutableStateOf<Pair<File, String>?>(null) }

    val takePhoto = rememberLauncherForActivityResult(
        ActivityResultContracts.TakePicture(),
    ) { ok ->
        val pending = pendingCapture
        pendingCapture = null
        if (pending == null) return@rememberLauncherForActivityResult
        // Cancelled: drop the empty placeholder rather than sending 0 bytes.
        if (!ok) { pending.first.delete(); return@rememberLauncherForActivityResult }
        offerCapturedFile(pending.first, pending.second)
    }

    val recordVideo = rememberLauncherForActivityResult(
        ActivityResultContracts.CaptureVideo(),
    ) { ok ->
        val pending = pendingCapture
        pendingCapture = null
        if (pending == null) return@rememberLauncherForActivityResult
        if (!ok) { pending.first.delete(); return@rememberLauncherForActivityResult }
        offerCapturedFile(pending.first, pending.second)
    }

    val requestCameraPermission = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { /* Re-checked on the next press; nothing to do here. */ }

    fun hasCameraPermission(): Boolean =
        ContextCompat.checkSelfPermission(context, android.Manifest.permission.CAMERA) ==
            PackageManager.PERMISSION_GRANTED

    fun launchCapture(mode: CaptureMode) {
        if (attaching) return
        if (!hasCameraPermission()) {
            requestCameraPermission.launch(android.Manifest.permission.CAMERA)
            return
        }
        when (mode) {
            CaptureMode.Photo -> {
                val file = newCaptureFile("jpg")
                pendingCapture = file to "image/jpeg"
                takePhoto.launch(captureUri(file))
            }
            CaptureMode.Video -> {
                val file = newCaptureFile("mp4")
                pendingCapture = file to "video/mp4"
                recordVideo.launch(captureUri(file))
            }
            // Voice never routes here — it has its own press-and-hold button.
            CaptureMode.Voice -> Unit
        }
    }

    fun hasMicPermission(): Boolean =
        ContextCompat.checkSelfPermission(context, android.Manifest.permission.RECORD_AUDIO) ==
            PackageManager.PERMISSION_GRANTED

    // Encrypt + send a recorded voice note, exactly like any other media
    // attachment (audio/aac over the DM channel), then wipe the plaintext clip.
    fun sendVoiceNote(file: File) {
        if (voiceSending) return
        voiceSending = true
        error = null
        scope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    val bytes = file.readBytes()
                    // Delete the temp recording the moment the send resolves —
                    // whether it succeeds or throws (AUDIT S-4): the decrypted
                    // clip must not linger on disk after the FFI call returns.
                    try {
                        ComradeCore.sendMediaBytesTyped(peer, VoiceRecorder.MIME_TYPE, "", bytes)
                    } finally {
                        file.delete()
                    }
                }
            }.onSuccess { info ->
                voiceSending = false
                mediaItems = mediaItems + info
                scope.launch { listState.scrollToItem(messages.size + mediaItems.size - 1) }
            }.onFailure {
                voiceSending = false
                error = it.message ?: "Could not send the voice note."
            }
        }
    }

    // Tap-to-start, tap-to-send. Returns silently if the mic is unavailable or
    // the permission was refused — the press is then simply a no-op rather than
    // leaving the icon stuck in a recording pose.
    fun toggleRecording() {
        if (recording) {
            recording = false
            recorder.stop()?.let { sendVoiceNote(it) }
            return
        }
        if (!hasMicPermission()) {
            requestMicPermission.launch(android.Manifest.permission.RECORD_AUDIO)
            return
        }
        if (recorder.start()) recording = true
    }


    // Whether the newest message is (nearly) on screen right now — drives
    // both the jump-to-latest button and clearing its "new" highlight.
    val atBottom by remember {
        derivedStateOf {
            isNearBottom(
                lastVisibleIndex = listState.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: -1,
                totalCount = listState.layoutInfo.totalItemsCount,
            )
        }
    }
    LaunchedEffect(atBottom) { if (atBottom) newMessagesBelow = false }
    // Reaching the bottom is the moment anything left unread has actually been
    // seen — the load effect deliberately doesn't claim that while scrolled up.
    LaunchedEffect(atBottom, chatItems.size) {
        if (loadedOnce && atBottom && chatItems.isNotEmpty()) {
            withContext(Dispatchers.IO) {
                runCatching { ComradeCore.markConversationReadTyped(peer) }
            }
        }
    }
    // Day headers compare against "now" once per data change; good enough —
    // a stale "Today" flips on the next message either way.
    val nowSecs = remember(chatItems) { System.currentTimeMillis() / 1000 }

    Column(modifier = modifier.fillMaxSize().imePadding()) {
        // The way in, and it only appears once there is something to go in to.
        //
        // The app bar belongs to `MainActivity` and is shared by every tab, so a
        // per-conversation action would have to be threaded up through it and
        // back down. A strip that is *absent* until the conversation has a
        // thread or a topic costs a chat that has neither exactly nothing — and
        // is more discoverable than a menu item, which is what this feature
        // most needs.
        if (hasThreads) {
            Surface(
                color = MaterialTheme.colorScheme.surfaceVariant,
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable {
                        filingMessage = null
                        topicsOpen = true
                    }
                    .testTag("threads-bar"),
            ) {
                Row(
                    modifier = Modifier.padding(horizontal = Spacing.space4, vertical = Spacing.space2),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(Spacing.space3),
                ) {
                    Icon(
                        TagIcon,
                        contentDescription = null,
                        tint = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.size(18.dp),
                    )
                    Text(
                        stringResource(R.string.threads_open),
                        style = MaterialTheme.typography.labelLarge,
                        modifier = Modifier.weight(1f),
                    )
                    if (unreadThreads > 0) {
                        Surface(
                            shape = CircleShape,
                            color = MaterialTheme.colorScheme.primary,
                        ) {
                            Text(
                                unreadThreads.toString(),
                                style = MaterialTheme.typography.labelSmall,
                                color = MaterialTheme.colorScheme.onPrimary,
                                modifier = Modifier.padding(horizontal = Spacing.space2, vertical = Spacing.space1),
                            )
                        }
                    }
                }
            }
        }
        Box(
            Modifier
                .weight(1f)
                .fillMaxWidth(),
        ) {
            LazyColumn(
                state = listState,
                modifier = Modifier.fillMaxSize(),
                contentPadding = PaddingValues(Spacing.space3),
                verticalArrangement = Arrangement.spacedBy(Spacing.space2),
            ) {
                if (chatItems.isEmpty()) {
                    item {
                        Text(
                            "Messages are end-to-end encrypted with your keys. Say hi!",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.fillMaxWidth().padding(top = 24.dp),
                            textAlign = TextAlign.Center,
                        )
                    }
                }
                itemsIndexed(
                    chatItems,
                    key = { _, item -> item.key },
                ) { index, item ->
                    // The separators live inside the message item (not as their
                    // own list items), so item indices keep matching `chatItems`
                    // and the scroll arithmetic above stays honest.
                    val prevAt = chatItems.getOrNull(index - 1)?.createdAt
                    // Tinted while this item is the target of a quote tap, and
                    // animated so the flash fades rather than blinking. Around the
                    // bubble only, not the separators above it: a target that
                    // happens to open a new day must not tint that day's header.
                    val highlight by animateColorAsState(
                        targetValue = if (item.key == highlightKey) {
                            MaterialTheme.colorScheme.primary.copy(alpha = 0.14f)
                        } else {
                            Color.Transparent
                        },
                        label = "quote-target-highlight",
                    )
                    Column(Modifier.fillMaxWidth()) {
                        if (startsNewDay(prevAt, item.createdAt)) {
                            DaySeparator(dayLabel(item.createdAt, nowSecs))
                        }
                        if (item.key == unreadBoundaryKey) {
                            UnreadSeparator(stringResource(R.string.unread_messages))
                        }
                        // Reply is a *swipe* now and react is the long press —
                        // Telegram's split. Long-press used to be reply, which
                        // left reactions with no gesture to live on and made the
                        // one discoverable interaction on a message the less
                        // common of the two.
                        //
                        // The accessibility action is not a nicety here: a drag
                        // and a long press are both invisible to a screen reader,
                        // so without it reply and react would be gesture-only.
                        val replyLabel = stringResource(R.string.message_action_reply)
                        val reactLabel = stringResource(R.string.message_action_react)
                        val gestureModifier = Modifier
                            .fillMaxWidth()
                            .semantics {
                                customActions = listOf(
                                    CustomAccessibilityAction(replyLabel) {
                                        replyingTo = item
                                        true
                                    },
                                    CustomAccessibilityAction(reactLabel) {
                                        actingOn = item
                                        true
                                    },
                                )
                            }
                            .combinedClickable(
                                onClick = {},
                                onLongClick = { actingOn = item },
                            )
                        val chips = chipsByTarget[item.eventId].orEmpty()
                        Column(
                            Modifier
                                .fillMaxWidth()
                                .background(highlight, RoundedCornerShape(18.dp)),
                        ) {
                            when (item) {
                                is ChatItem.MediaItem -> SwipeToReply(
                                    onReply = { replyingTo = item },
                                    modifier = gestureModifier,
                                ) {
                                    Column(
                                        Modifier.fillMaxWidth(),
                                        horizontalAlignment = if (item.info.outgoing) {
                                            Alignment.End
                                        } else {
                                            Alignment.Start
                                        },
                                    ) {
                                        MediaAttachmentBubble(
                                            item.info,
                                            onReply = { replyingTo = item },
                                        )
                                        ReactionChips(chips) { toggleReaction(item, it) }
                                    }
                                }
                                is ChatItem.TextItem -> {
                                    val msg = item.msg
                                    val quoted = msg.replyTo?.let { byId[it] }
                                    // Tara sits on the left for *both* people,
                                    // so this is not simply `msg.outgoing`: her
                                    // answer is carried by whichever device
                                    // asked, and aligning by who sent it would
                                    // put one line on opposite sides of the two
                                    // phones. It also drops the ticks from her
                                    // bubble — true that this device sent it,
                                    // but the question right above carries the
                                    // same receipt, and a tick on a third
                                    // party's line reads as a claim about her.
                                    // Mirrored in `message_bubble.dart` and
                                    // `main.js`.
                                    val hers = msg.fromTara
                                    val mine = msg.outgoing && !hers
                                    SwipeToReply(
                                        onReply = { replyingTo = item },
                                        modifier = gestureModifier,
                                    ) {
                                        Column(
                                            Modifier.fillMaxWidth(),
                                            horizontalAlignment = if (mine) Alignment.End else Alignment.Start,
                                        ) {
                                            Surface(
                                                shape = RoundedCornerShape(
                                                    topStart = 18.dp,
                                                    topEnd = 18.dp,
                                                    bottomStart = if (mine) 18.dp else 6.dp,
                                                    bottomEnd = if (mine) 6.dp else 18.dp,
                                                ),
                                                color = when {
                                                    hers -> MaterialTheme.colorScheme.tertiaryContainer
                                                    mine -> MaterialTheme.colorScheme.primaryContainer
                                                    else -> MaterialTheme.colorScheme.surfaceVariant
                                                },
                                                tonalElevation = 1.dp,
                                                modifier = Modifier.widthIn(max = 300.dp),
                                            ) {
                                                Column(Modifier.padding(horizontal = Spacing.space3, vertical = Spacing.space2)) {
                                                    if (hers) {
                                                        Text(
                                                            stringResource(R.string.tara_author_label),
                                                            style = MaterialTheme.typography.labelSmall,
                                                            fontWeight = FontWeight.SemiBold,
                                                            color = MaterialTheme.colorScheme.onTertiaryContainer,
                                                            modifier = Modifier.testTag("tara-author"),
                                                        )
                                                    }
                                                    if (quoted != null) {
                                                        QuotedPreview(quoted.preview) {
                                                            goToQuoted(msg.replyTo)
                                                        }
                                                    }
                                                    // A shared journal note keeps the
                                                    // bubble it arrived in and gains a
                                                    // header saying where it was
                                                    // written; see SharedNoteBody.
                                                    val note = msg.sharedNote
                                                    if (note != null) {
                                                        SharedNoteBody(note, outgoing = msg.outgoing)
                                                    } else {
                                                        Text(msg.content, style = MaterialTheme.typography.bodyLarge)
                                                    }
                                                    Row(
                                                        modifier = Modifier
                                                            .align(Alignment.End)
                                                            .padding(top = Spacing.space1),
                                                        verticalAlignment = Alignment.CenterVertically,
                                                        horizontalArrangement = Arrangement.spacedBy(4.dp),
                                                    ) {
                                                        Text(
                                                            clockTime(msg.createdAt),
                                                            style = MaterialTheme.typography.labelSmall,
                                                            color = MaterialTheme.colorScheme.outline,
                                                        )
                                                        if (mine) {
                                                            Text(
                                                                statusGlyph(msg.status),
                                                                style = MaterialTheme.typography.labelSmall,
                                                                color = if (msg.status == "read") {
                                                                    MaterialTheme.colorScheme.primary
                                                                } else {
                                                                    MaterialTheme.colorScheme.outline
                                                                },
                                                            )
                                                        }
                                                    }
                                                }
                                            }
                                            ReactionChips(chips) { toggleReaction(item, it) }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if (!atBottom && chatItems.isNotEmpty()) {
                SmallFloatingActionButton(
                    onClick = {
                        newMessagesBelow = false
                        scope.launch { listState.scrollToItem(chatItems.size - 1) }
                    },
                    containerColor = if (newMessagesBelow) {
                        MaterialTheme.colorScheme.primary
                    } else {
                        MaterialTheme.colorScheme.surfaceVariant
                    },
                    contentColor = if (newMessagesBelow) {
                        MaterialTheme.colorScheme.onPrimary
                    } else {
                        MaterialTheme.colorScheme.onSurfaceVariant
                    },
                    modifier = Modifier
                        .align(Alignment.BottomEnd)
                        .padding(12.dp)
                        .testTag("dm-jump-latest"),
                ) {
                    Icon(
                        Icons.Filled.KeyboardArrowDown,
                        contentDescription = if (newMessagesBelow) {
                            "New messages — jump to latest"
                        } else {
                            "Jump to latest"
                        },
                    )
                }
            }
        }

        // A large attachment being handed straight to this peer, if there is
        // one. Rendered from the manager's flow, so it survives this screen.
        AttachmentHandoffPanel(peer)

        error?.let {
            Text(
                it,
                color = MaterialTheme.colorScheme.error,
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.padding(horizontal = 16.dp),
            )
        }

        // "Replying to…" chip above the composer.
        replyingTo?.let { r ->
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Surface(
                    shape = RoundedCornerShape(10.dp),
                    color = MaterialTheme.colorScheme.surfaceVariant,
                    modifier = Modifier.weight(1f),
                ) {
                    Text(
                        "↩ " + r.preview,
                        style = MaterialTheme.typography.bodySmall,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.padding(horizontal = Spacing.space3, vertical = Spacing.space2),
                    )
                }
                TextButton(onClick = { replyingTo = null }) { Text("✕") }
            }
        }

        // While recording, a live "● Recording…" banner so it is unmistakable
        // that the microphone is hot.
        if (recording) {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 16.dp, vertical = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Box(
                    Modifier
                        .size(10.dp)
                        .clip(CircleShape)
                        .background(MaterialTheme.colorScheme.error),
                )
                Text(
                    stringResource(R.string.composer_recording),
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.error,
                )
            }
        }

        // Composer, Telegram's layout:
        //
        //   ╭────────────────────────────────╮      ╭────╮
        //   │ 🙂  Message…               📎  │  ⧉   │ 🎤 │
        //   ╰────────────────────────────────╯      ╰────╯
        //     emoji     text field     attach  swap  capture → send
        //
        // Emoji and the paper clip sit *inside* the field, which is what makes
        // the pill read as one control instead of a row of loose buttons. Only
        // the round button (and its swap) live outside, because they are the
        // ones whose meaning changes.
        // The `/` picker sits above the field so choosing a row does not shift
        // the composer under the thumb mid-tap.
        //
        // **It scrolls, and it is capped.** A bare `/` matches the whole
        // catalogue — every command in `comrade_core::command::catalog` — and each
        // row is two lines, so an uncapped `Column` grew taller than the screen:
        // the rows past the fold were unreachable and the thread was pushed out
        // of sight. `heightIn` bounds it to roughly four rows, which is what
        // leaves the conversation visible while you type.
        if (pickerRows.isNotEmpty()) {
            LazyColumn(
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(max = PICKER_MAX_HEIGHT)
                    .padding(horizontal = 12.dp)
                    .testTag("dm-command-picker"),
            ) {
                // Keyed by name, per `.claude/rules/android.md`: the list is
                // re-filtered on every keystroke, and without a key the row state
                // reattaches to whichever command now sits at that index.
                items(pickerRows, key = { it.name }) { spec ->
                    Text(
                        text = "/${spec.name}  ${spec.argument}\n${spec.help}",
                        style = MaterialTheme.typography.bodySmall,
                        modifier = Modifier
                            .fillMaxWidth()
                            .clickable {
                                editDraft(
                                    TextFieldValue(ChatCommands.completionFor(spec)).let {
                                        it.copy(selection = TextRange(it.text.length))
                                    },
                                )
                            }
                            .padding(vertical = Spacing.space2, horizontal = Spacing.space1),
                    )
                }
            }
        }

        // Two contacts answer to one handle, so ask which — rather than the dead
        // end this used to be, which said "pick which one" and gave nothing to
        // pick. The key is on every row: the whole reason the handle is ambiguous
        // is often that both people also chose the same name.
        choosing?.let { question ->
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(max = PICKER_MAX_HEIGHT)
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = 16.dp, vertical = 4.dp)
                    .testTag("dm-mention-chooser"),
            ) {
                Text(
                    text = stringResource(R.string.mention_which_one, question.handle),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.primary,
                )
                for (candidate in question.candidates) {
                    Text(
                        text = ChatCommands.candidateLabel(
                            peerTitle(candidate.npub, candidate.alias, candidate.name),
                            candidate.npub,
                        ),
                        style = MaterialTheme.typography.bodyMedium,
                        modifier = Modifier
                            .fillMaxWidth()
                            .clickable {
                                mentionChoices = mentionChoices +
                                    (question.handle to candidate.npub)
                                // Re-run the same draft, now that the handle
                                // resolves. The text never left the box, so this
                                // is the command the user already meant.
                                runCommand(draft.text.trim())
                            }
                            .padding(vertical = 8.dp),
                    )
                }
            }
        }

        // Tara addressed from a chat must not look like a message — and now must
        // not look like the *wrong audience* either. Said in words as well as in
        // the field's own styling, because the words are what a first-time user
        // needs and the styling is what a returning one reads at a glance.
        taraAudience?.let { audience ->
            Text(
                text = stringResource(
                    when (audience) {
                        ChatCommands.TaraAudience.Private -> R.string.aside_only_you
                        ChatCommands.TaraAudience.Shared -> R.string.tara_here_both_see
                    },
                ),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.primary,
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 16.dp)
                    .testTag("dm-aside-note"),
            )
        }

        commandNote?.let { note ->
            Text(
                text = note,
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable { commandNote = null }
                    .padding(horizontal = 16.dp, vertical = 4.dp)
                    .testTag("dm-command-note"),
            )
        }

        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 12.dp, vertical = 8.dp),
            verticalAlignment = Alignment.Bottom,
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            OutlinedTextField(
                value = draft,
                onValueChange = { editDraft(it) },
                placeholder = {
                    Text(
                        when (taraAudience) {
                            ChatCommands.TaraAudience.Private ->
                                stringResource(R.string.aside_placeholder)
                            ChatCommands.TaraAudience.Shared ->
                                stringResource(R.string.tara_here_placeholder)
                            null -> "Message"
                        },
                    )
                },
                shape = RoundedCornerShape(26.dp),
                modifier = Modifier
                    .weight(1f)
                    .testTag("dm-input"),
                maxLines = 4,
                leadingIcon = {
                    IconButton(
                        onClick = { emojiOpen = true },
                        modifier = Modifier.testTag("dm-emoji"),
                    ) {
                        Icon(
                            EmojiIcon,
                            contentDescription = stringResource(R.string.composer_emoji),
                            tint = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                },
                trailingIcon = {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        IconButton(
                            onClick = { if (!attaching) pickMedia.launch("*/*") },
                            enabled = !attaching,
                            modifier = Modifier.testTag("dm-attach"),
                        ) {
                            if (attaching) {
                                CircularProgressIndicator(
                                    modifier = Modifier.size(20.dp),
                                    strokeWidth = 2.dp,
                                )
                            } else {
                                Icon(
                                    AttachFileIcon,
                                    contentDescription = stringResource(R.string.composer_attach),
                                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                            }
                        }
                        // The one action control, inside the field. Send while
                        // there is text; otherwise the current capture mode,
                        // which a right-swipe on this same icon cycles.
                        ComposerActionIcon(
                            hasText = draft.text.isNotBlank(),
                            mode = effectiveCaptureMode(captureMode, availableModes),
                            recording = recording,
                            busy = sending || voiceSending || attaching,
                            onSend = { send() },
                            onCycle = { next -> captureMode = next },
                            nextMode = { nextCaptureMode(captureMode, availableModes) },
                            onCapture = { mode ->
                                when (mode) {
                                    CaptureMode.Voice -> toggleRecording()
                                    else -> launchCapture(mode)
                                }
                            },
                        )
                    }
                },
            )
        }
    }

    if (emojiOpen) {
        EmojiPickerSheet(
            onPick = { editDraft(insertEmoji(draft, it)) },
            onDismiss = { emojiOpen = false },
        )
    }

    pendingAttachment?.let { pending ->
        AttachmentPreviewSheet(
            pending = pending,
            // Dismissing sends nothing and leaves the draft alone. The bytes go
            // with the state, and for a capture the file behind them is already
            // deleted.
            onDismiss = { pendingAttachment = null },
            onSend = { caption -> sendAttachment(pending, caption) },
        )
    }

    // Long press on a message: react, or reach the actions.
    actingOn?.let { item ->
        MessageActionSheet(
            myReaction = myReactionByTarget[item.eventId],
            onReact = {
                toggleReaction(item, it)
                actingOn = null
            },
            onMoreEmoji = {
                // Hand the target over before closing, or the picker opens with
                // nothing to react to.
                pickingFor = item
                actingOn = null
            },
            onReply = {
                replyingTo = item
                actingOn = null
            },
            onCopy = {
                clipboard.setText(AnnotatedString(item.preview))
                actingOn = null
            },
            // Both take the *tapped* message's event id, not a thread root:
            // core walks up the reply chain, so long-pressing a reply opens and
            // files the thread it belongs to rather than starting a second one.
            onOpenThread = {
                threadOpenFor = item.eventId
                actingOn = null
            },
            onFile = {
                filingMessage = item.eventId
                topicsOpen = true
                actingOn = null
            },
            onDismiss = { actingOn = null },
        )
    }

    threadOpenFor?.let { root ->
        ThreadSheet(
            peer = peer,
            rootId = root,
            tick = chatTick + topicTick + localTopicTick,
            onDismiss = { threadOpenFor = null },
            onFile = {
                filingMessage = root
                topicsOpen = true
                threadOpenFor = null
            },
        )
    }

    if (topicsOpen) {
        TopicSheet(
            peer = peer,
            tick = topicTick + localTopicTick,
            pendingFile = filingMessage,
            onDismiss = {
                topicsOpen = false
                filingMessage = null
            },
            onOpenThread = {
                topicsOpen = false
                threadOpenFor = it
            },
            onFiled = { slug ->
                // Said out loud, because filing produces no bubble on either
                // side (`comrade_core::topic`'s module header says why) — so
                // without this the one deliberate action in the sheet would
                // leave no trace at all.
                commandNote = slug?.let { context.getString(R.string.topics_filed, it) }
                    ?: context.getString(R.string.topics_unfiled_note)
                localTopicTick += 1
                topicsOpen = false
                filingMessage = null
            },
        )
    }

    // "+" from the reaction row: the same picker the composer uses, pointed at a
    // message instead of the draft. Closes on a pick — unlike the composer's,
    // where choosing several in a row is the common case; one message takes one
    // reaction, so staying open would only invite a second toggle that undoes it.
    pickingFor?.let { item ->
        EmojiPickerSheet(
            onPick = {
                toggleReaction(item, it)
                pickingFor = null
            },
            onDismiss = { pickingFor = null },
        )
    }
}

/** The glyph for a capture mode. */
private fun captureIcon(mode: CaptureMode) = when (mode) {
    CaptureMode.Voice -> MicIcon
    CaptureMode.Photo -> PhotoCameraIcon
    CaptureMode.Video -> VideocamIcon
}

/** The string resource naming what a capture mode does. */
private fun captureLabel(mode: CaptureMode): Int = when (mode) {
    CaptureMode.Voice -> R.string.composer_record_voice
    CaptureMode.Photo -> R.string.composer_take_photo
    CaptureMode.Video -> R.string.composer_record_video
}

/**
 * The composer's single action control, inside the text field.
 *
 * One icon, three jobs:
 *  * **Send**, whenever there is text — that outranks everything.
 *  * **Tap** otherwise performs the current capture mode: start/stop a voice
 *    note, or open the camera for a photo or a video.
 *  * **Swipe right** cycles the mode (voice → photo → video → voice).
 *
 * Recording is tap-to-start / tap-to-send rather than press-and-hold, because a
 * hold and a swipe cannot share one target: holding to record would have to win
 * the gesture before a swipe could be recognised, so one of the two would
 * always lose. Tap and drag compose cleanly, and it matches what the Flutter
 * composer already chose for the same reason a mouse cannot press-and-hold
 * meaningfully.
 *
 * Swiping is not discoverable on its own, and it is unavailable to anyone using
 * a screen reader or a switch device — so the mode is also exposed as a
 * semantics custom action, which is what assistive tech offers instead.
 */
@Composable
private fun ComposerActionIcon(
    hasText: Boolean,
    mode: CaptureMode?,
    recording: Boolean,
    busy: Boolean,
    onSend: () -> Unit,
    onCycle: (CaptureMode) -> Unit,
    nextMode: () -> CaptureMode?,
    onCapture: (CaptureMode) -> Unit,
) {
    // Accumulated horizontal travel of the current drag. Reset on each stop, so
    // a left-then-right wobble does not add up into a mode change.
    var dragX by remember { mutableStateOf(0f) }
    val swapTo = nextMode()
    val cycleLabel = swapTo?.let {
        stringResource(R.string.composer_swap_capture, stringResource(captureLabel(it)))
    }

    val icon = when {
        hasText -> Icons.AutoMirrored.Filled.Send
        recording -> StopIcon
        mode != null -> captureIcon(mode)
        // Nothing to capture on this device: a Send that is simply inert until
        // there is text, rather than a control that fails on tap.
        else -> Icons.AutoMirrored.Filled.Send
    }
    val label = when {
        hasText -> stringResource(R.string.composer_send)
        recording -> stringResource(R.string.composer_stop_recording)
        mode != null -> stringResource(captureLabel(mode))
        else -> stringResource(R.string.composer_send)
    }
    val tint = when {
        recording -> MaterialTheme.colorScheme.error
        hasText -> MaterialTheme.colorScheme.primary
        else -> MaterialTheme.colorScheme.onSurfaceVariant
    }

    val enabled = !busy || recording
    Box(
        modifier = Modifier
            .size(48.dp)
            .testTag("dm-action")
            .clip(CircleShape)
            .semantics {
                // The swipe, offered to assistive tech as a real action.
                if (!hasText && swapTo != null && cycleLabel != null) {
                    customActions = listOf(
                        CustomAccessibilityAction(cycleLabel) { onCycle(swapTo); true },
                    )
                }
            }
            .clickable(enabled = enabled) {
                when {
                    hasText -> onSend()
                    mode != null -> onCapture(mode)
                    else -> Unit
                }
            }
            .draggable(
                orientation = Orientation.Horizontal,
                enabled = !hasText && swapTo != null && !recording,
                state = rememberDraggableState { delta -> dragX += delta },
                onDragStopped = {
                    // Rightwards only, and far enough to be deliberate rather
                    // than a slip while reaching for the icon.
                    if (dragX > SWIPE_THRESHOLD_PX) nextMode()?.let(onCycle)
                    dragX = 0f
                },
            ),
        contentAlignment = Alignment.Center,
    ) {
        if (busy && !recording) {
            CircularProgressIndicator(modifier = Modifier.size(20.dp), strokeWidth = 2.dp)
        } else {
            Icon(icon, contentDescription = label, tint = tint)
        }
    }
}

/**
 * How far right the finger has to travel to count as a mode swipe.
 *
 * In pixels rather than dp because [androidx.compose.foundation.gestures.draggable]
 * reports raw deltas; ~64px is comfortably past touch slop on every density the
 * app targets without demanding a full swipe across the field.
 */
private const val SWIPE_THRESHOLD_PX = 64f

/**
 * How tall the `/` picker and the mention chooser may grow before they scroll.
 *
 * Both sit between the thread and the composer, so their height is taken from
 * the conversation. A bare `/` lists the entire catalogue and two contacts can
 * answer to one handle — neither list has a small upper bound, and before this
 * cap the rows past the fold could not be reached at all.
 */
private val PICKER_MAX_HEIGHT = 200.dp

/** Centred "Today" / "Yesterday" / "12 Jul 2026" pill between days. */
@Composable
private fun DaySeparator(label: String) {
    Box(
        Modifier
            .fillMaxWidth()
            .padding(vertical = Spacing.space2),
        contentAlignment = Alignment.Center,
    ) {
        Surface(
            shape = RoundedCornerShape(12.dp),
            color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.7f),
        ) {
            Text(
                label,
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = Spacing.space2, vertical = Spacing.space1),
            )
        }
    }
}

/**
 * "Unread messages" line marking where the reader left off.
 *
 * A full-width rule rather than a day-style pill: it is a boundary through the
 * thread, not a label on one message, and it has to be findable at a glance
 * after scrolling away from it.
 */
@Composable
private fun UnreadSeparator(label: String) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(vertical = Spacing.space2),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        HorizontalDivider(
            modifier = Modifier.weight(1f),
            color = MaterialTheme.colorScheme.primary.copy(alpha = 0.5f),
        )
        Text(
            label,
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.primary,
        )
        HorizontalDivider(
            modifier = Modifier.weight(1f),
            color = MaterialTheme.colorScheme.primary.copy(alpha = 0.5f),
        )
    }
}

/**
 * A small quoted line rendered above a reply's own text.
 *
 * Tapping it goes to the message being quoted. [onTap] is null when there is
 * nowhere to go — the original is outside the loaded history — and then the quote
 * renders with no tap target and no accent bar, so a tap that could not work is
 * never offered. Mirrors `message_bubble.dart`'s `QuotedPreview`.
 */
@Composable
private fun QuotedPreview(text: String, onTap: (() -> Unit)? = null) {
    val label = stringResource(R.string.message_goto_quoted)
    Surface(
        shape = RoundedCornerShape(8.dp),
        color = MaterialTheme.colorScheme.surface.copy(alpha = 0.6f),
        modifier = Modifier
            .fillMaxWidth()
            .padding(bottom = 4.dp)
            .then(
                if (onTap == null) {
                    Modifier
                } else {
                    Modifier.clickable(onClickLabel = label, onClick = onTap)
                },
            ),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            // The accent bar is the affordance: it says "this points at something
            // else", which is what makes the tap discoverable at all.
            if (onTap != null) {
                Box(
                    Modifier
                        .width(3.dp)
                        .height(28.dp)
                        .background(MaterialTheme.colorScheme.primary),
                )
            }
            Text(
                text,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(horizontal = 8.dp, vertical = 4.dp),
            )
        }
    }
}

// ── Reactions and the gestures that reach them ────────────────────────────────

/**
 * The reaction chips under one bubble. Tapping a chip toggles *your* reaction in
 * it — adding yours if you were not in it, taking yours back if you were — which
 * is why [ReactionChip.mine] has to be drawn: without the highlight, tapping is
 * a coin flip between the two.
 *
 * Renders nothing at all for an empty list, so a message with no reactions costs
 * no vertical space.
 */
@Composable
private fun ReactionChips(chips: List<ReactionChip>, onToggle: (String) -> Unit) {
    if (chips.isEmpty()) return
    Row(
        modifier = Modifier.padding(top = Spacing.space1).testTag("dm-reactions"),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        for (chip in chips) {
            Surface(
                shape = RoundedCornerShape(12.dp),
                color = if (chip.mine) {
                    MaterialTheme.colorScheme.primary.copy(alpha = 0.18f)
                } else {
                    MaterialTheme.colorScheme.surfaceVariant
                },
                modifier = Modifier.clickable { onToggle(chip.emoji) },
            ) {
                Row(
                    modifier = Modifier.padding(horizontal = Spacing.space2, vertical = Spacing.space1),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(Spacing.space1),
                ) {
                    Text(chip.emoji, style = MaterialTheme.typography.labelLarge)
                    // The count is only informative once more than one person is
                    // in it; "🔥 1" is noise next to "🔥".
                    if (chip.count > 1) {
                        Text(
                            chip.count.toString(),
                            style = MaterialTheme.typography.labelSmall,
                            color = if (chip.mine) {
                                MaterialTheme.colorScheme.primary
                            } else {
                                MaterialTheme.colorScheme.onSurfaceVariant
                            },
                        )
                    }
                }
            }
        }
    }
}

/**
 * What a long press on a message opens: the quick reaction row, then the actions.
 *
 * A bottom sheet rather than the floating row Telegram anchors over the bubble.
 * The reachability argument decides it: an anchored popup has to be positioned
 * against a bubble that may be at the very top or bottom of the list, and the
 * failure mode is a row half off screen. A sheet is always in the same place and
 * always within a thumb's reach.
 *
 * [myReaction] is the emoji this device has already sent on this message, if any
 * — highlighted, so it is visible that tapping it again removes it.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun MessageActionSheet(
    myReaction: String?,
    onReact: (String) -> Unit,
    onMoreEmoji: () -> Unit,
    onReply: () -> Unit,
    onCopy: () -> Unit,
    onOpenThread: () -> Unit,
    onFile: () -> Unit,
    onDismiss: () -> Unit,
) {
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val sheetShape = RoundedCornerShape(topStart = ComradeRadii.xl, topEnd = ComradeRadii.xl)
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
        modifier = Modifier.testTag("message-actions").glassSurface(GlassElevation.Sheet, shape = sheetShape),
        shape = sheetShape,
        containerColor = Color.Transparent,
    ) {
        Column(Modifier.fillMaxWidth().padding(bottom = 8.dp)) {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceEvenly,
            ) {
                for (emoji in QUICK_REACTIONS) {
                    val mine = emoji == myReaction
                    Surface(
                        shape = CircleShape,
                        color = if (mine) {
                            MaterialTheme.colorScheme.primary.copy(alpha = 0.18f)
                        } else {
                            Color.Transparent
                        },
                        modifier = Modifier
                            .clip(CircleShape)
                            .clickable { onReact(emoji) }
                            .testTag("react-$emoji"),
                    ) {
                        Text(
                            emoji,
                            style = MaterialTheme.typography.headlineSmall,
                            modifier = Modifier.padding(8.dp),
                        )
                    }
                }
                // Anything outside the six. Same sheet the composer's emoji
                // button opens, so there is one picker in the app.
                IconButton(onClick = onMoreEmoji, modifier = Modifier.testTag("react-more")) {
                    Icon(Icons.Filled.Add, contentDescription = stringResource(R.string.react_more))
                }
            }
            HorizontalDivider(Modifier.padding(vertical = 8.dp))
            SheetAction(
                icon = ReplyIcon,
                label = stringResource(R.string.message_action_reply),
                onClick = onReply,
                testTag = "action-reply",
            )
            SheetAction(
                icon = ChatBubbleIcon,
                label = stringResource(R.string.message_action_open_thread),
                onClick = onOpenThread,
                testTag = "action-open-thread",
            )
            SheetAction(
                icon = TagIcon,
                label = stringResource(R.string.message_action_assign_topic),
                onClick = onFile,
                testTag = "action-assign-topic",
            )
            SheetAction(
                icon = CopyIcon,
                label = stringResource(R.string.message_action_copy),
                onClick = onCopy,
                testTag = "action-copy",
            )
        }
    }
}

/** One full-width row of [MessageActionSheet]. */
@Composable
private fun SheetAction(
    icon: androidx.compose.ui.graphics.vector.ImageVector,
    label: String,
    onClick: () -> Unit,
    testTag: String,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(horizontal = 20.dp, vertical = 14.dp)
            .testTag(testTag),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(18.dp),
    ) {
        Icon(icon, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(label, style = MaterialTheme.typography.bodyLarge)
    }
}

/**
 * Wraps a bubble so dragging it rightward and letting go replies to it.
 *
 * The arrow fades in behind the bubble as the drag approaches the trigger, so the
 * gesture teaches itself — there is no other way to discover it. On release the
 * offset returns to zero whether or not it fired, because the bubble is not a
 * pane that stays open; the reply chip above the composer is the result.
 *
 * `onReply` is *also* exposed as a [CustomAccessibilityAction] by the caller: a
 * drag is invisible to a screen reader, and reply must not be a gesture-only
 * feature. The numbers live in [replySwipeOffsetDp] / [replySwipeTriggered] so a
 * JVM test can pin them and the Flutter side can match them.
 */
@Composable
private fun SwipeToReply(
    onReply: () -> Unit,
    modifier: Modifier = Modifier,
    content: @Composable () -> Unit,
) {
    var drag by remember { mutableStateOf(0f) }
    val density = LocalDensity.current
    val offsetDp = replySwipeOffsetDp(drag)
    val armed = replySwipeTriggered(drag)
    Box(modifier) {
        if (offsetDp > 1f) {
            Icon(
                ReplyIcon,
                contentDescription = null,
                tint = if (armed) {
                    MaterialTheme.colorScheme.primary
                } else {
                    MaterialTheme.colorScheme.outline
                },
                modifier = Modifier
                    .align(Alignment.CenterStart)
                    .padding(start = 4.dp)
                    .size(20.dp)
                    .alpha((offsetDp / REPLY_SWIPE_TRIGGER_DP).coerceIn(0f, 1f)),
            )
        }
        Box(
            Modifier
                .offset(x = offsetDp.dp)
                .draggable(
                    orientation = Orientation.Horizontal,
                    state = rememberDraggableState { delta ->
                        // Accumulated in dp, not pixels, so the threshold means
                        // the same distance on every screen density.
                        drag += with(density) { delta.toDp().value }
                    },
                    onDragStopped = {
                        if (replySwipeTriggered(drag)) onReply()
                        drag = 0f
                    },
                ),
        ) {
            content()
        }
    }
}

// ── Message requests (gate strangers until accepted) ──────────────────────────

/**
 * The message-requests inbox: strangers' first DMs, gated out of the chat list.
 * Accepting shares your @handle with them and moves the thread into Chats;
 * blocking drops their future messages.
 */
@Composable
fun RequestsScreen(
    chatTick: Int,
    onOpen: (peer: String, alias: String?, username: String?) -> Unit,
    modifier: Modifier = Modifier,
) {
    var requests by remember { mutableStateOf<List<ComradeCore.MessageRequestInfo>?>(null) }
    var reloadTick by remember { mutableStateOf(0) }
    val scope = rememberCoroutineScope()

    LaunchedEffect(chatTick, reloadTick) {
        requests = withContext(Dispatchers.IO) {
            runCatching { ComradeCore.messageRequestsTyped() }.getOrDefault(emptyList())
        }
    }

    val list = requests
    when {
        list == null -> Column(modifier.fillMaxSize()) {
            // §7.3: skeletons, not a spinner — same reasoning as the chat list above.
            repeat(ComradeSkeletonRowCount) { RequestRowSkeleton() }
        }
        list.isEmpty() -> Box(
            modifier.fillMaxSize().padding(Spacing.space8),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                "No message requests.",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        else -> LazyColumn(modifier.fillMaxSize()) {
            items(list, key = { it.peer }) { req ->
                Column(
                    Modifier
                        .fillMaxWidth()
                        .padding(horizontal = Spacing.space4, vertical = Spacing.space3),
                ) {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(Spacing.space4),
                    ) {
                        PeerAvatar(shortNpub(req.peer), seed = req.peer)
                        Column(Modifier.weight(1f)) {
                            Text(
                                shortNpub(req.peer),
                                style = MaterialTheme.typography.titleSmall,
                                fontFamily = FontFamily.Monospace,
                            )
                            Text(
                                req.lastMessage,
                                style = MaterialTheme.typography.bodyMedium,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                maxLines = 2,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                    }
                    Row(
                        Modifier.fillMaxWidth().padding(top = Spacing.space2),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        OutlinedButton(
                            onClick = {
                                scope.launch {
                                    withContext(Dispatchers.IO) {
                                        runCatching { ComradeCore.blockConversationTyped(req.peer) }
                                    }
                                    reloadTick++
                                }
                            },
                        ) { Text("Block") }
                        Button(
                            onClick = {
                                scope.launch {
                                    val ok = withContext(Dispatchers.IO) {
                                        runCatching { ComradeCore.acceptRequestTyped(req.peer) }
                                            .isSuccess
                                    }
                                    reloadTick++
                                    if (ok) onOpen(req.peer, null, null)
                                }
                            },
                        ) { Text("Accept") }
                    }
                    HorizontalDivider(
                        color = MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.4f),
                        modifier = Modifier.padding(top = 8.dp),
                    )
                }
            }
        }
    }
}

/**
 * What a pick turned out to be, once the provider has been asked.
 *
 * [bytes] is null for a file taking the peer-to-peer road: it was deliberately
 * not read, because the point of that road is that a 400 MB attachment never has
 * to fit in memory. [size] is null only when neither the provider's `SIZE` column
 * nor the descriptor's `statSize` knew — in which case the bytes were read and
 * their length is the answer.
 */
private class PickedFile(
    val name: String,
    val mime: String,
    val size: Long?,
    val bytes: ByteArray?,
)
