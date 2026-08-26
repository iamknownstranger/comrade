package mullu.comrade.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
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
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import mullu.comrade.ComradeCore
import mullu.comrade.R
import mullu.comrade.topic.ThreadFilter
import mullu.comrade.topic.ThreadRow
import mullu.comrade.topic.TopicDecisions
import mullu.comrade.topic.TopicRow
import mullu.comrade.ui.theme.ComradeRadii
import mullu.comrade.ui.theme.GlassElevation
import mullu.comrade.ui.theme.glassSurface

/*
 * The thread sheet and the topic sheet — Telegram's forum topics and Slack's
 * threads, over a two-person chat.
 *
 * Two sheets, not one screen. A thread is read *beside* the conversation it came
 * from, not instead of it: pushing a route would make going back a navigation
 * decision, and the whole point of a thread is that you dip into it and return.
 * `ModalBottomSheet` is what the long-press action sheet already uses here.
 *
 * ## What is decided elsewhere
 *
 * Ordering, filtering, which rows are hidden and what `/assign` does are
 * `mullu.comrade.topic.TopicDecisions` — pure, JVM-tested, and mirrored by
 * `desktop/ui/topics.mjs`. This file draws them. The slug rules and the reply
 * graph are `comrade_core::topic`, over uniffi.
 *
 * The one thing this file decides on its own is the *word* for a preview core
 * deliberately left unworded ([TopicDecisions.PreviewKind]) — because the word
 * belongs in `strings.xml` where it can be translated, and core cannot reach
 * that.
 */

/**
 * One thread, opened over the conversation: the root, its replies, and a
 * composer that always addresses the root.
 *
 * [rootId] may name any message in the thread — core walks up to the root, so a
 * sheet opened from a reply shows the same thread as one opened from the top.
 *
 * Reloads on [tick] the way the conversation does, so a reply arriving while the
 * sheet is open appears in it.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ThreadSheet(
    peer: String,
    rootId: String,
    tick: Int,
    onDismiss: () -> Unit,
    onFile: () -> Unit,
    modifier: Modifier = Modifier,
) {
    var thread by remember(peer, rootId) { mutableStateOf<ComradeCore.ThreadInfo?>(null) }
    var draft by remember(peer, rootId) { mutableStateOf("") }
    var sending by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val listState = rememberLazyListState()
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)

    LaunchedEffect(peer, rootId, tick) {
        val loaded = withContext(Dispatchers.IO) {
            runCatching { ComradeCore.thread(peer, rootId) }.getOrNull()
        }
        thread = loaded
    }

    // The merged, time-ordered thread — the same interleave `ConversationScreen`
    // does over its own two lists, because core hands up two lists on purpose
    // (see `comrade_ui::ThreadDto`) rather than inventing a third ordering.
    val entries = remember(thread) {
        val t = thread
        if (t == null) {
            emptyList()
        } else {
            (t.messages.map(ThreadEntry::Text) + t.media.map(ThreadEntry::Media))
                .sortedBy { it.createdAt }
        }
    }

    // Newest last, and a thread is short — so land at the bottom on open and on
    // every arrival, with none of the conversation's "are they scrolled up"
    // bookkeeping. A sheet you opened deliberately is a sheet you are reading.
    LaunchedEffect(entries.size) {
        if (entries.isNotEmpty()) listState.scrollToItem(entries.size - 1)
    }

    val sheetShape = RoundedCornerShape(topStart = ComradeRadii.xl, topEnd = ComradeRadii.xl)
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
        modifier = modifier.glassSurface(GlassElevation.Sheet, shape = sheetShape),
        shape = sheetShape,
        containerColor = Color.Transparent,
    ) {
        Column(Modifier.imePadding().testTag("thread-sheet")) {
            Row(
                modifier = Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Column {
                    Text(
                        stringResource(R.string.threads_title),
                        style = MaterialTheme.typography.titleMedium,
                    )
                    val replies = (entries.size - 1).coerceAtLeast(0)
                    Text(
                        pluralStringResource(R.plurals.thread_replies, replies, replies),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    // The topic this thread is in, if any — a chip rather than a
                    // line, so it reads as a label on the thread rather than as
                    // something somebody said.
                    thread?.topicSlug?.let { slug ->
                        Surface(
                            shape = RoundedCornerShape(50),
                            color = MaterialTheme.colorScheme.secondaryContainer,
                            modifier = Modifier.padding(end = 8.dp),
                        ) {
                            Text(
                                "#$slug",
                                style = MaterialTheme.typography.labelMedium,
                                modifier = Modifier.padding(horizontal = 10.dp, vertical = 4.dp),
                            )
                        }
                    }
                    TextButton(onClick = onFile, modifier = Modifier.testTag("thread-file")) {
                        Icon(TagIcon, contentDescription = null, modifier = Modifier.size(18.dp))
                        Text(
                            stringResource(R.string.message_action_assign_topic),
                            modifier = Modifier.padding(start = 6.dp),
                        )
                    }
                }
            }
            HorizontalDivider()

            LazyColumn(
                state = listState,
                modifier = Modifier.weight(1f, fill = false).heightIn(min = 120.dp),
                contentPadding = androidx.compose.foundation.layout.PaddingValues(
                    horizontal = 16.dp,
                    vertical = 8.dp,
                ),
                verticalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                items(entries, key = { it.key }) { entry ->
                    ThreadBubble(entry)
                }
            }

            error?.let {
                Text(
                    it,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.error,
                    modifier = Modifier.padding(horizontal = 20.dp),
                )
            }

            Row(
                modifier = Modifier.fillMaxWidth().padding(12.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                OutlinedTextField(
                    value = draft,
                    onValueChange = { draft = it },
                    modifier = Modifier.weight(1f).testTag("thread-input"),
                    placeholder = { Text(stringResource(R.string.thread_reply_hint)) },
                    maxLines = 4,
                )
                FilledIconButton(
                    onClick = {
                        val text = draft.trim()
                        if (text.isEmpty() || sending) return@FilledIconButton
                        sending = true
                        scope.launch {
                            // Addressed to the thread's root rather than to the
                            // last message, which is what keeps the thread flat
                            // — see `ComradeRuntime::send_thread_reply`.
                            val sent = withContext(Dispatchers.IO) {
                                runCatching {
                                    ComradeCore.sendThreadReplyTyped(peer, rootId, text)
                                }
                            }
                            sent
                                .onSuccess { message ->
                                    draft = ""
                                    error = null
                                    // Appended rather than reloaded: the row is
                                    // already stored, and a reload would race
                                    // the relay.
                                    thread = thread?.let { it.copy(messages = it.messages + message) }
                                }
                                .onFailure { error = it.message ?: "Could not send that." }
                            sending = false
                        }
                    },
                    enabled = draft.isNotBlank() && !sending,
                    modifier = Modifier.testTag("thread-send"),
                ) {
                    if (sending) {
                        CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                    } else {
                        Icon(
                            Icons.AutoMirrored.Filled.Send,
                            contentDescription = stringResource(R.string.thread_send),
                        )
                    }
                }
            }
        }
    }
}

/**
 * One item in a thread — text or an attachment, both of which are ordinary
 * events and can be either end of a reply.
 *
 * A local sealed type rather than reusing `ChatsScreen`'s `ChatItem`: that one
 * is private to the conversation screen and carries reply and reaction state a
 * thread bubble has no use for. Two small types beat one type with half its
 * fields unused in each caller.
 */
private sealed interface ThreadEntry {
    val createdAt: Long
    val key: String
    val outgoing: Boolean

    data class Text(val msg: ComradeCore.MessageInfo) : ThreadEntry {
        override val createdAt get() = msg.createdAt
        override val key get() = msg.id
        override val outgoing get() = msg.outgoing
    }

    data class Media(val info: ComradeCore.MediaMessageInfo) : ThreadEntry {
        override val createdAt get() = info.createdAt
        // Namespaced so a media event id cannot collide with a message id — the
        // same rule `ChatItem.key` follows.
        override val key get() = "media:${info.eventId}"
        override val outgoing get() = info.outgoing
    }
}

@Composable
private fun ThreadBubble(entry: ThreadEntry) {
    val text = when (entry) {
        is ThreadEntry.Text -> entry.msg.content
        is ThreadEntry.Media -> entry.info.caption.ifBlank {
            stringResource(R.string.thread_preview_attachment)
        }
    }
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = if (entry.outgoing) Arrangement.End else Arrangement.Start,
    ) {
        Surface(
            shape = RoundedCornerShape(14.dp),
            color = if (entry.outgoing) {
                MaterialTheme.colorScheme.primaryContainer
            } else {
                MaterialTheme.colorScheme.surfaceVariant
            },
        ) {
            Text(
                text,
                style = MaterialTheme.typography.bodyMedium,
                modifier = Modifier.padding(horizontal = 12.dp, vertical = 8.dp),
            )
        }
    }
}

/**
 * Every thread in the conversation, grouped by topic — the sheet the header
 * button and a bare `/assign` both open.
 *
 * Filing happens from here too: [pendingFile] is the message id `/assign` or a
 * long-press was about to file, and when it is set every topic row becomes a
 * destination rather than a place to read.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TopicSheet(
    peer: String,
    tick: Int,
    pendingFile: String?,
    onDismiss: () -> Unit,
    onOpenThread: (String) -> Unit,
    onFiled: (String?) -> Unit,
    modifier: Modifier = Modifier,
) {
    var topics by remember(peer) { mutableStateOf<List<TopicRow>>(emptyList()) }
    var threads by remember(peer) { mutableStateOf<List<ThreadRow>>(emptyList()) }
    var filter by remember(peer) { mutableStateOf<ThreadFilter>(ThreadFilter.All) }
    var showArchived by remember(peer) { mutableStateOf(false) }
    var naming by remember { mutableStateOf(false) }
    var newName by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)

    suspend fun reload() {
        val loaded = withContext(Dispatchers.IO) {
            runCatching { ComradeCore.topics(peer) to ComradeCore.threads(peer) }.getOrNull()
        }
        loaded?.let {
            topics = it.first
            threads = it.second
        }
    }

    LaunchedEffect(peer, tick) { reload() }

    /** File [pendingFile]'s thread under [name], or unfile it with `null`. */
    fun file(name: String?) {
        val target = pendingFile ?: return
        busy = true
        scope.launch {
            withContext(Dispatchers.IO) {
                runCatching { ComradeCore.assignThreadTyped(peer, target, name) }
            }
                .onSuccess {
                    error = null
                    reload()
                    onFiled(it.topicSlug)
                }
                .onFailure { error = it.message ?: "Could not file that." }
            busy = false
        }
    }

    val sheetShape = RoundedCornerShape(topStart = ComradeRadii.xl, topEnd = ComradeRadii.xl)
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
        modifier = modifier.glassSurface(GlassElevation.Sheet, shape = sheetShape),
        shape = sheetShape,
        containerColor = Color.Transparent,
    ) {
        Column(Modifier.imePadding().testTag("topic-sheet")) {
            Row(
                modifier = Modifier.fillMaxWidth().padding(horizontal = 20.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Text(
                    stringResource(
                        if (pendingFile != null) {
                            R.string.message_action_assign_topic
                        } else {
                            R.string.threads_open
                        },
                    ),
                    style = MaterialTheme.typography.titleMedium,
                )
                TextButton(onClick = { naming = true }, modifier = Modifier.testTag("topic-new")) {
                    Text(stringResource(R.string.topics_new))
                }
            }

            if (naming) {
                Row(
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    OutlinedTextField(
                        value = newName,
                        onValueChange = { newName = it },
                        modifier = Modifier.weight(1f).testTag("topic-name"),
                        placeholder = { Text(stringResource(R.string.topics_name_hint)) },
                        singleLine = true,
                    )
                    TextButton(
                        onClick = {
                            val name = newName.trim()
                            if (name.isEmpty()) return@TextButton
                            busy = true
                            scope.launch {
                                // Naming and filing in one gesture when there is
                                // something to file: someone who typed a name
                                // while a message was selected meant both, and
                                // making them tap the row they just created is a
                                // step that exists only because the code has two
                                // calls.
                                if (pendingFile != null) {
                                    file(name)
                                } else {
                                    withContext(Dispatchers.IO) {
                                        runCatching { ComradeCore.createTopicTyped(peer, name) }
                                    }
                                        .onSuccess { error = null; reload() }
                                        .onFailure { error = it.message }
                                }
                                newName = ""
                                naming = false
                                busy = false
                            }
                        },
                        enabled = newName.isNotBlank() && !busy,
                    ) { Text(stringResource(R.string.topics_create)) }
                    TextButton(onClick = { naming = false; newName = "" }) {
                        Text(stringResource(R.string.topics_cancel))
                    }
                }
            }

            error?.let {
                Text(
                    it,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.error,
                    modifier = Modifier.padding(horizontal = 20.dp, vertical = 4.dp),
                )
            }

            val visible = remember(topics, showArchived) {
                TopicDecisions.visibleTopics(topics, includeClosed = showArchived)
            }

            LazyColumn(
                modifier = Modifier.weight(1f, fill = false),
                contentPadding = androidx.compose.foundation.layout.PaddingValues(bottom = 24.dp),
            ) {
                item {
                    Row(
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        FilterChip(
                            selected = filter is ThreadFilter.All,
                            onClick = { filter = ThreadFilter.All },
                            label = { Text(stringResource(R.string.threads_all)) },
                        )
                        FilterChip(
                            selected = filter is ThreadFilter.Unfiled,
                            onClick = { filter = ThreadFilter.Unfiled },
                            label = { Text(stringResource(R.string.threads_unfiled)) },
                        )
                        if (topics.any { it.closed }) {
                            FilterChip(
                                selected = showArchived,
                                onClick = { showArchived = !showArchived },
                                label = { Text(stringResource(R.string.topics_show_archived)) },
                            )
                        }
                    }
                }

                if (visible.isEmpty()) {
                    item {
                        Text(
                            stringResource(R.string.topics_none),
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(20.dp),
                        )
                    }
                }

                items(visible, key = { it.slug }) { topic ->
                    TopicRowItem(
                        topic = topic,
                        unread = TopicDecisions.unreadThreadCount(threads, topic.slug),
                        filing = pendingFile != null,
                        selected = (filter as? ThreadFilter.InTopic)?.slug == topic.slug,
                        onClick = {
                            if (pendingFile != null) {
                                file(topic.name)
                            } else {
                                filter = if ((filter as? ThreadFilter.InTopic)?.slug == topic.slug) {
                                    ThreadFilter.All
                                } else {
                                    ThreadFilter.InTopic(topic.slug)
                                }
                            }
                        },
                        onArchive = {
                            busy = true
                            scope.launch {
                                withContext(Dispatchers.IO) {
                                    runCatching {
                                        ComradeCore.setTopicClosedTyped(
                                            peer,
                                            topic.slug,
                                            !topic.closed,
                                        )
                                    }
                                }
                                    .onSuccess { error = null; reload() }
                                    .onFailure { error = it.message }
                                busy = false
                            }
                        },
                    )
                }

                // Taking a thread out of every topic is a destination too, and
                // only exists while something is being filed.
                if (pendingFile != null) {
                    item {
                        Text(
                            stringResource(R.string.topics_remove_from),
                            style = MaterialTheme.typography.bodyMedium,
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable { file(null) }
                                .padding(horizontal = 20.dp, vertical = 14.dp)
                                .testTag("topic-unfile"),
                        )
                    }
                }

                if (pendingFile == null) {
                    item { HorizontalDivider(Modifier.padding(vertical = 8.dp)) }
                    val rows = TopicDecisions.threadsFor(
                        threads,
                        filter,
                        // Under a named topic, a thread of one is there because
                        // somebody filed it — hiding it would lose the filing.
                        includeSingletons = filter is ThreadFilter.InTopic,
                    )
                    if (rows.isEmpty()) {
                        item {
                            Text(
                                stringResource(
                                    if (filter is ThreadFilter.InTopic) {
                                        R.string.threads_empty_topic
                                    } else {
                                        R.string.threads_empty
                                    },
                                ),
                                style = MaterialTheme.typography.bodyMedium,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                modifier = Modifier.padding(20.dp),
                            )
                        }
                    }
                    items(rows, key = { it.rootId }) { row ->
                        ThreadRowItem(row) { onOpenThread(row.rootId) }
                    }
                }
            }
        }
    }
}

@Composable
private fun TopicRowItem(
    topic: TopicRow,
    unread: Int,
    filing: Boolean,
    selected: Boolean,
    onClick: () -> Unit,
    onArchive: () -> Unit,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(
                if (selected) {
                    MaterialTheme.colorScheme.secondaryContainer
                } else {
                    androidx.compose.ui.graphics.Color.Transparent
                },
            )
            .clickable(onClick = onClick)
            .padding(horizontal = 20.dp, vertical = 12.dp)
            .testTag("topic-${topic.slug}"),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Icon(
            TagIcon,
            contentDescription = null,
            tint = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.size(20.dp),
        )
        Column(Modifier.weight(1f)) {
            Text(
                topic.name,
                style = MaterialTheme.typography.bodyLarge,
                fontWeight = if (unread > 0) FontWeight.SemiBold else FontWeight.Normal,
            )
            Text(
                if (topic.closed) {
                    stringResource(R.string.topics_archived)
                } else {
                    stringResource(
                        R.string.topics_counts,
                        topic.threadCount,
                        topic.messageCount,
                    )
                },
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        if (unread > 0) {
            Surface(shape = RoundedCornerShape(50), color = MaterialTheme.colorScheme.primary) {
                Text(
                    unread.toString(),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onPrimary,
                    modifier = Modifier.padding(horizontal = 7.dp, vertical = 2.dp),
                )
            }
        }
        // Archiving while filing would be a second meaning for one tap on a row
        // that is currently a destination, so it is out of the way then.
        if (!filing) {
            TextButton(onClick = onArchive) {
                Text(
                    stringResource(
                        if (topic.closed) R.string.topics_unarchive else R.string.topics_archive,
                    ),
                    style = MaterialTheme.typography.labelMedium,
                )
            }
        }
    }
}

@Composable
private fun ThreadRowItem(row: ThreadRow, onClick: () -> Unit) {
    // The one place this file decides a word, and it decides it from
    // `PreviewKind` so the branch is what the JVM test pins and the sentence is
    // what `strings.xml` translates.
    val preview = when (TopicDecisions.previewKind(row)) {
        TopicDecisions.PreviewKind.Text -> row.preview
        TopicDecisions.PreviewKind.CaptionedMedia -> "📎 ${row.preview}"
        TopicDecisions.PreviewKind.BareMedia -> stringResource(R.string.thread_preview_attachment)
        TopicDecisions.PreviewKind.RootMissing -> stringResource(R.string.thread_root_missing)
        TopicDecisions.PreviewKind.Empty -> stringResource(R.string.thread_preview_empty)
    }
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(horizontal = 20.dp, vertical = 12.dp)
            .testTag("thread-${row.rootId}"),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Icon(
            ChatBubbleIcon,
            contentDescription = null,
            tint = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.size(18.dp),
        )
        Column(Modifier.weight(1f)) {
            Text(
                preview,
                style = MaterialTheme.typography.bodyMedium,
                maxLines = 2,
                fontWeight = if (row.unread) FontWeight.SemiBold else FontWeight.Normal,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    pluralStringResource(
                        R.plurals.thread_replies,
                        row.replyCount,
                        row.replyCount,
                    ),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                row.topicSlug?.let {
                    Text(
                        "#$it",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.primary,
                    )
                }
            }
        }
        if (row.unread) {
            Box(
                Modifier
                    .size(8.dp)
                    .background(MaterialTheme.colorScheme.primary, RoundedCornerShape(50)),
            )
        }
    }
}
