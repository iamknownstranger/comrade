package mullu.comrade.ui

import android.util.Base64
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import mullu.comrade.ComradeCore
import mullu.comrade.Notifier

/**
 * Identity-stable avatar hues: the same key renders the same colour on every
 * device (Telegram-style), so people become recognisable at a glance.
 */
private val AvatarPalette = listOf(
    Color(0xFF6366F1), // indigo
    Color(0xFF0EA5E9), // sky
    Color(0xFF10B981), // emerald
    Color(0xFFF59E0B), // amber
    Color(0xFFEF4444), // coral
    Color(0xFF8B5CF6), // violet
    Color(0xFFEC4899), // rose
    Color(0xFF14B8A6), // teal
)

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
        list == null -> Box(modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            CircularProgressIndicator(Modifier.size(28.dp))
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
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable { onOpen(convo.peer, convo.alias, convo.peerName) }
                        .padding(horizontal = 16.dp, vertical = 11.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(14.dp),
                ) {
                    PeerAvatar(title, seed = convo.peer)
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

/** Empty-state prompt for the chat list. */
@Composable
private fun EmptyChats(onNewChat: () -> Unit) {
    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(32.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Text("No chats yet", style = MaterialTheme.typography.titleMedium)
        Text(
            "Find someone by username, or share your key so they can find you.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(top = 4.dp, bottom = 16.dp),
        )
        Button(onClick = onNewChat) { Text("Start a chat") }
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
        verticalArrangement = Arrangement.spacedBy(10.dp),
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
                    .padding(vertical = 6.dp),
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
                        .padding(vertical = 6.dp),
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

@OptIn(ExperimentalFoundationApi::class)
@Composable
fun ConversationScreen(
    peer: String,
    chatTick: Int,
    modifier: Modifier = Modifier,
) {
    var messages by remember { mutableStateOf<List<ComradeCore.MessageInfo>>(emptyList()) }
    var draft by remember { mutableStateOf("") }
    var sending by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var replyingTo by remember { mutableStateOf<ComradeCore.MessageInfo?>(null) }
    var attaching by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    val listState = rememberLazyListState()
    val context = LocalContext.current

    // Quick lookup so a bubble carrying reply_to can show a quoted preview.
    val byId = remember(messages) { messages.associateBy { it.id } }

    LaunchedEffect(peer, chatTick) {
        messages = withContext(Dispatchers.IO) {
            runCatching { ComradeCore.messages(peer) }.getOrDefault(emptyList())
        }
        if (messages.isNotEmpty()) listState.scrollToItem(messages.size - 1)
    }

    // Opening the thread marks it read (sends a read receipt) and clears any
    // pending notification for this peer.
    LaunchedEffect(peer) {
        Notifier.clearForPeer(context, peer)
        withContext(Dispatchers.IO) {
            runCatching { ComradeCore.markConversationReadTyped(peer) }
        }
    }

    fun send() {
        val text = draft.trim()
        if (text.isEmpty() || sending) return
        sending = true
        error = null
        val replyId = replyingTo?.id
        scope.launch {
            runCatching {
                withContext(Dispatchers.IO) { ComradeCore.sendDmReplyTyped(peer, text, replyId) }
            }.onSuccess { sent ->
                draft = ""
                replyingTo = null
                sending = false
                messages = messages + sent
                scope.launch { listState.scrollToItem(messages.size - 1) }
            }.onFailure {
                sending = false
                error = it.message ?: "Could not send."
            }
        }
    }

    // Encrypt + send a picked file as an attachment (NIP-94 over the DM channel).
    val pickMedia = rememberLauncherForActivityResult(
        ActivityResultContracts.GetContent(),
    ) { uri ->
        if (uri == null || attaching) return@rememberLauncherForActivityResult
        attaching = true
        error = null
        scope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    val bytes = context.contentResolver.openInputStream(uri)?.use { it.readBytes() }
                        ?: throw IllegalStateException("Could not read the file.")
                    if (bytes.size > 10 * 1024 * 1024) {
                        throw IllegalStateException("Attachments are limited to 10 MB.")
                    }
                    val mime = context.contentResolver.getType(uri) ?: "application/octet-stream"
                    val b64 = Base64.encodeToString(bytes, Base64.NO_WRAP)
                    ComradeCore.sendMediaBytesTyped(peer, mime, "", b64)
                }
            }.onSuccess {
                attaching = false
                // Media isn't part of the text history; show a local marker line.
                messages = messages + ComradeCore.MessageInfo(
                    id = "media:${it.eventId}",
                    peer = peer,
                    content = "📎 ${it.mimeType}",
                    createdAt = it.createdAt,
                    outgoing = true,
                    status = "sent",
                    replyTo = null,
                )
                scope.launch { listState.scrollToItem(messages.size - 1) }
            }.onFailure {
                attaching = false
                error = it.message ?: "Could not send the attachment."
            }
        }
    }

    Column(modifier = modifier.fillMaxSize().imePadding()) {
        LazyColumn(
            state = listState,
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth(),
            contentPadding = PaddingValues(12.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            if (messages.isEmpty()) {
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
            items(messages, key = { it.id }) { msg ->
                val quoted = msg.replyTo?.let { byId[it] }
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .combinedClickable(
                            onClick = {},
                            onLongClick = { replyingTo = msg },
                        ),
                    horizontalArrangement = if (msg.outgoing) Arrangement.End else Arrangement.Start,
                ) {
                    Surface(
                        shape = RoundedCornerShape(
                            topStart = 18.dp,
                            topEnd = 18.dp,
                            bottomStart = if (msg.outgoing) 18.dp else 6.dp,
                            bottomEnd = if (msg.outgoing) 6.dp else 18.dp,
                        ),
                        color = if (msg.outgoing) {
                            MaterialTheme.colorScheme.primaryContainer
                        } else {
                            MaterialTheme.colorScheme.surfaceVariant
                        },
                        tonalElevation = 1.dp,
                        modifier = Modifier.widthIn(max = 300.dp),
                    ) {
                        Column(Modifier.padding(horizontal = 14.dp, vertical = 9.dp)) {
                            if (quoted != null) {
                                QuotedPreview(quoted.content)
                            }
                            Text(msg.content, style = MaterialTheme.typography.bodyLarge)
                            Row(
                                modifier = Modifier
                                    .align(Alignment.End)
                                    .padding(top = 2.dp),
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(4.dp),
                            ) {
                                Text(
                                    relativeTime(msg.createdAt),
                                    style = MaterialTheme.typography.labelSmall,
                                    color = MaterialTheme.colorScheme.outline,
                                )
                                if (msg.outgoing) {
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
                }
            }
        }

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
                        "↩ " + r.content,
                        style = MaterialTheme.typography.bodySmall,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.padding(horizontal = 12.dp, vertical = 6.dp),
                    )
                }
                TextButton(onClick = { replyingTo = null }) { Text("✕") }
            }
        }

        // Composer: attach + pill input + filled send, Telegram-style.
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 12.dp, vertical = 8.dp),
            verticalAlignment = Alignment.Bottom,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            IconButton(
                onClick = { if (!attaching) pickMedia.launch("*/*") },
                enabled = !attaching,
                modifier = Modifier.size(48.dp).testTag("dm-attach"),
            ) {
                Text(if (attaching) "…" else "📎", style = MaterialTheme.typography.titleLarge)
            }
            OutlinedTextField(
                value = draft,
                onValueChange = { draft = it },
                placeholder = { Text("Message") },
                shape = RoundedCornerShape(26.dp),
                modifier = Modifier
                    .weight(1f)
                    .testTag("dm-input"),
                maxLines = 4,
            )
            FilledIconButton(
                onClick = { send() },
                enabled = draft.isNotBlank() && !sending,
                modifier = Modifier
                    .size(52.dp)
                    .testTag("dm-send"),
            ) {
                Icon(
                    Icons.AutoMirrored.Filled.Send,
                    contentDescription = "Send",
                )
            }
        }
    }
}

/** A small quoted line rendered above a reply's own text. */
@Composable
private fun QuotedPreview(text: String) {
    Surface(
        shape = RoundedCornerShape(8.dp),
        color = MaterialTheme.colorScheme.surface.copy(alpha = 0.6f),
        modifier = Modifier
            .fillMaxWidth()
            .padding(bottom = 4.dp),
    ) {
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
        list == null -> Box(modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            CircularProgressIndicator(Modifier.size(28.dp))
        }
        list.isEmpty() -> Box(
            modifier.fillMaxSize().padding(32.dp),
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
                        .padding(horizontal = 16.dp, vertical = 10.dp),
                ) {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(14.dp),
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
                        Modifier.fillMaxWidth().padding(top = 6.dp),
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
