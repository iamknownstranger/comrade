package mullu.comrade.ui

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
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
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
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import mullu.comrade.ComradeCore

/** Short display form of an npub: `npub1abcd…wxyz`. */
fun shortNpub(npub: String): String =
    if (npub.length > 16) "${npub.take(10)}…${npub.takeLast(4)}" else npub

/** Display title for a peer: saved alias if any, else the shortened key. */
fun peerTitle(peer: String, alias: String?): String = alias ?: shortNpub(peer)

/** Rough relative timestamp for list rows. */
fun relativeTime(epochSecs: Long): String {
    val d = System.currentTimeMillis() / 1000 - epochSecs
    return when {
        d < 60 -> "now"
        d < 3600 -> "${d / 60}m"
        d < 86_400 -> "${d / 3600}h"
        else -> "${d / 86_400}d"
    }
}

@Composable
fun PeerAvatar(title: String, modifier: Modifier = Modifier) {
    Surface(
        shape = CircleShape,
        color = MaterialTheme.colorScheme.primaryContainer,
        modifier = modifier.size(44.dp),
    ) {
        Box(contentAlignment = Alignment.Center) {
            Text(
                text = title.trimStart('@').take(1).uppercase().ifEmpty { "?" },
                style = MaterialTheme.typography.titleMedium,
                color = MaterialTheme.colorScheme.onPrimaryContainer,
            )
        }
    }
}

// ── Chat list ────────────────────────────────────────────────────────────────

@Composable
fun ChatsScreen(
    chatTick: Int,
    onOpen: (peer: String, alias: String?) -> Unit,
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
                val title = peerTitle(convo.peer, convo.alias)
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable { onOpen(convo.peer, convo.alias) }
                        .padding(horizontal = 16.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    PeerAvatar(title)
                    Column(Modifier.weight(1f)) {
                        Text(
                            title,
                            style = MaterialTheme.typography.titleSmall,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        Text(
                            text = (if (convo.lastOutgoing) "You: " else "") + convo.lastMessage,
                            style = MaterialTheme.typography.bodySmall,
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
                HorizontalDivider(modifier = Modifier.padding(start = 72.dp))
            }
        }
    }
}

// ── New chat (find people) ───────────────────────────────────────────────────

@Composable
fun NewChatScreen(
    onOpen: (peer: String, alias: String?) -> Unit,
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

    fun startChat(npub: String, alias: String?) {
        scope.launch {
            val saved = withContext(Dispatchers.IO) {
                runCatching { ComradeCore.addContactTyped(npub, alias ?: "") }.getOrNull()
            }
            if (saved == null) {
                error = "That doesn't look like a valid key."
            } else {
                onOpen(saved.npub, saved.alias)
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
                PeerAvatar(title)
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
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable { onOpen(contact.npub, contact.alias) }
                        .padding(vertical = 6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    PeerAvatar(contact.alias)
                    Column {
                        Text(contact.alias, style = MaterialTheme.typography.titleSmall)
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
                            topStart = 16.dp,
                            topEnd = 16.dp,
                            bottomStart = if (msg.outgoing) 16.dp else 4.dp,
                            bottomEnd = if (msg.outgoing) 4.dp else 16.dp,
                        ),
                        color = if (msg.outgoing) {
                            MaterialTheme.colorScheme.primaryContainer
                        } else {
                            MaterialTheme.colorScheme.surfaceVariant
                        },
                        modifier = Modifier.widthIn(max = 300.dp),
                    ) {
                        Column(Modifier.padding(horizontal = 12.dp, vertical = 8.dp)) {
                            Text(msg.content, style = MaterialTheme.typography.bodyMedium)
                            Text(
                                relativeTime(msg.createdAt),
                                style = MaterialTheme.typography.labelSmall,
                                color = MaterialTheme.colorScheme.outline,
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

        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 12.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            OutlinedTextField(
                value = draft,
                onValueChange = { draft = it },
                placeholder = { Text("Message") },
                modifier = Modifier
                    .weight(1f)
                    .testTag("dm-input"),
                maxLines = 4,
            )
            IconButton(
                onClick = { send() },
                enabled = draft.isNotBlank() && !sending,
                modifier = Modifier.testTag("dm-send"),
            ) {
                Icon(
                    Icons.AutoMirrored.Filled.Send,
                    contentDescription = "Send",
                    tint = MaterialTheme.colorScheme.primary,
                )
            }
        }
    }
}
