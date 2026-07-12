package mullu.comrade.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
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
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
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

@Composable
fun ChatsScreen(
    chatTick: Int,
    onOpen: (peer: String, alias: String?, username: String?) -> Unit,
    onNewChat: () -> Unit,
    modifier: Modifier = Modifier,
) {
    var conversations by remember { mutableStateOf<List<ComradeCore.ConversationInfo>?>(null) }

    LaunchedEffect(chatTick) {
        conversations = withContext(Dispatchers.IO) {
            runCatching { ComradeCore.conversations() }.getOrDefault(emptyList())
        }
    }

    val list = conversations
    when {
        list == null -> Box(modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            CircularProgressIndicator(Modifier.size(28.dp))
        }
        list.isEmpty() -> Column(
            modifier = modifier
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
        else -> LazyColumn(modifier = modifier.fillMaxSize()) {
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
    val scope = rememberCoroutineScope()
    val listState = rememberLazyListState()

    LaunchedEffect(peer, chatTick) {
        messages = withContext(Dispatchers.IO) {
            runCatching { ComradeCore.messages(peer) }.getOrDefault(emptyList())
        }
        if (messages.isNotEmpty()) listState.scrollToItem(messages.size - 1)
    }

    fun send() {
        val text = draft.trim()
        if (text.isEmpty() || sending) return
        sending = true
        error = null
        scope.launch {
            runCatching {
                withContext(Dispatchers.IO) { ComradeCore.sendDmTyped(peer, text) }
            }.onSuccess { sent ->
                draft = ""
                sending = false
                messages = messages + sent
                scope.launch { listState.scrollToItem(messages.size - 1) }
            }.onFailure {
                sending = false
                error = it.message ?: "Could not send."
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
                Row(
                    modifier = Modifier.fillMaxWidth(),
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
                            Text(msg.content, style = MaterialTheme.typography.bodyLarge)
                            Text(
                                relativeTime(msg.createdAt),
                                style = MaterialTheme.typography.labelSmall,
                                color = MaterialTheme.colorScheme.outline,
                                modifier = Modifier
                                    .align(Alignment.End)
                                    .padding(top = 2.dp),
                            )
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

        // Composer: pill input + filled send, Telegram-style.
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 12.dp, vertical = 8.dp),
            verticalAlignment = Alignment.Bottom,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
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
