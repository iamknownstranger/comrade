package mullu.comrade.ui

import android.content.Context
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ElevatedCard
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import mullu.comrade.ComradeCore
import mullu.comrade.ui.theme.ComradeRadii
import mullu.comrade.ui.theme.GlassElevation
import mullu.comrade.ui.theme.glassSurface

private const val PREFS = "tara"
private const val KEY_ACCEPTED = "accepted"

/**
 * Tara — the reflective companion (wellbeing pillar #4). A private space to
 * think out loud: reflective prompts, feeling-mirroring, brainstorming.
 *
 * Two honesty gates (AUDIT §8) shape everything on this screen:
 *  • Tara is NOT therapy and never presents as one — the first-open explainer
 *    and the persistent footer both say so, and any message carrying distress
 *    cues switches the reply into a hand-off to real crisis helplines.
 *  • Everything is on-device: the reply engine is deterministic Rust code and
 *    the thread lives only in the encrypted store. No network, no cloud.
 */
@Composable
fun TaraScreen(modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val prefs = remember { context.getSharedPreferences(PREFS, Context.MODE_PRIVATE) }
    var accepted by remember { mutableStateOf(prefs.getBoolean(KEY_ACCEPTED, false)) }

    if (!accepted) {
        TaraExplainer(
            modifier = modifier,
            onAccept = {
                prefs.edit().putBoolean(KEY_ACCEPTED, true).apply()
                accepted = true
            },
        )
    } else {
        TaraThread(modifier = modifier)
    }
}

/** First-open explainer — the user opts in knowing exactly what Tara is not. */
@Composable
private fun TaraExplainer(onAccept: () -> Unit, modifier: Modifier = Modifier) {
    Column(
        modifier = modifier
            .fillMaxSize()
            .padding(24.dp),
        verticalArrangement = Arrangement.Center,
    ) {
        ElevatedCard(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text("Meet Tara", style = MaterialTheme.typography.headlineSmall)
                Text(
                    "A private space to reflect, vent, or think a decision through. " +
                        "Tara listens and asks questions — she doesn't judge, and " +
                        "nothing you say ever leaves this phone.",
                    style = MaterialTheme.typography.bodyMedium,
                )
                Text(
                    "Tara is not a therapist, doctor, or crisis service, and she " +
                        "never gives medical advice. If you're in crisis, she'll " +
                        "point you to real helplines — please use them.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Button(
                    onClick = onAccept,
                    modifier = Modifier
                        .fillMaxWidth()
                        .testTag("tara-accept"),
                ) { Text("I understand — let's talk") }
            }
        }
    }
}

/** Tara's reply while it is still arriving — see [TaraStream]. */
private data class StreamingReply(val text: String, val crisis: Boolean)

@Composable
private fun TaraThread(modifier: Modifier = Modifier) {
    val scope = rememberCoroutineScope()
    var messages by remember { mutableStateOf<List<ComradeCore.TaraMessageInfo>?>(null) }
    var opener by remember { mutableStateOf<String?>(null) }
    var crisisResources by remember { mutableStateOf<List<ComradeCore.CrisisResourceInfo>>(emptyList()) }
    var draft by remember { mutableStateOf("") }
    // The turn in flight: the user's message shown immediately, then a
    // thinking indicator, then the reply streaming in. All three are local —
    // the persisted thread is only re-read once the turn settles, so nothing
    // renders twice.
    var pendingUser by remember { mutableStateOf<String?>(null) }
    var thinking by remember { mutableStateOf(false) }
    var streaming by remember { mutableStateOf<StreamingReply?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var confirmClear by remember { mutableStateOf(false) }
    val listState = rememberLazyListState()

    val busy = thinking || streaming != null

    suspend fun reload() {
        val (thread, hello) = withContext(Dispatchers.IO) {
            val t = runCatching { ComradeCore.taraThread() }.getOrDefault(emptyList())
            val h = if (t.isEmpty()) {
                runCatching { ComradeCore.taraOpener() }.getOrNull()
            } else {
                null
            }
            t to h
        }
        messages = thread
        opener = hello
    }
    LaunchedEffect(Unit) {
        crisisResources = withContext(Dispatchers.IO) {
            runCatching { ComradeCore.taraCrisisResources() }.getOrDefault(emptyList())
        }
        reload()
    }

    // Follow the conversation only while the reader is already at the bottom —
    // scrolling back to re-read something must not be yanked away (the same
    // rule the chat thread follows).
    val atBottom by remember {
        derivedStateOf {
            val info = listState.layoutInfo
            val lastVisible = info.visibleItemsInfo.lastOrNull()?.index ?: 0
            info.totalItemsCount == 0 || lastVisible >= info.totalItemsCount - 2
        }
    }
    LaunchedEffect(messages?.size, pendingUser, thinking, streaming?.text?.length) {
        val count = listState.layoutInfo.totalItemsCount
        if (count > 0 && atBottom) listState.scrollToItem(count - 1)
    }

    fun send() {
        val text = draft.trim()
        if (text.isEmpty() || busy) return
        draft = ""
        error = null
        pendingUser = text
        thinking = true
        scope.launch {
            val reply = withContext(Dispatchers.IO) {
                runCatching { ComradeCore.taraSendTyped(text) }
            }
            thinking = false
            reply.onSuccess { message ->
                if (message.crisis) {
                    // Never drip-feed a crisis hand-off: helpline numbers
                    // appear complete, at once, the moment they are known.
                    streaming = StreamingReply(message.text, crisis = true)
                } else {
                    streaming = StreamingReply("", crisis = false)
                    TaraStream.stream(message.text).collect { soFar ->
                        streaming = StreamingReply(soFar, crisis = false)
                    }
                }
                // The turn is already persisted (taraSend wrote both sides);
                // re-read it, then drop the local copies in the same frame.
                reload()
                pendingUser = null
                streaming = null
            }.onFailure {
                pendingUser = null
                streaming = null
                error = it.message ?: "Could not send."
            }
        }
    }

    Column(modifier.fillMaxSize()) {
        val list = messages
        LazyColumn(
            state = listState,
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth()
                .padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            when {
                list == null -> item {
                    Box(
                        Modifier
                            .fillMaxWidth()
                            .padding(top = 24.dp),
                    ) {
                        CircularProgressIndicator(Modifier.align(Alignment.Center))
                    }
                }
                list.isEmpty() && pendingUser == null -> item(key = "opener") {
                    opener?.let { TaraBubble(text = it, fromTara = true) }
                }
                else -> items(list, key = { it.id }) { msg ->
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        TaraBubble(text = msg.text, fromTara = msg.fromTara)
                        if (msg.crisis && msg.fromTara) CrisisCard(crisisResources)
                    }
                }
            }

            // The turn in flight.
            pendingUser?.let { pending ->
                item(key = "pending-user") { TaraBubble(text = pending, fromTara = false) }
            }
            if (thinking) {
                item(key = "thinking") { ThinkingBubble() }
            }
            streaming?.let { reply ->
                item(key = "streaming") {
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        if (reply.text.isNotEmpty()) {
                            TaraBubble(
                                text = reply.text,
                                fromTara = true,
                                tag = "tara-streaming",
                            )
                        }
                        if (reply.crisis) CrisisCard(crisisResources)
                    }
                }
            }
            item(key = "footer-space") { Spacer(Modifier.padding(2.dp)) }
        }

        ElevatedCard(
            Modifier
                .fillMaxWidth()
                .padding(horizontal = 12.dp, vertical = 8.dp),
        ) {
            Column(Modifier.padding(10.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Row(
                    verticalAlignment = Alignment.Bottom,
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    OutlinedTextField(
                        value = draft,
                        onValueChange = { draft = it },
                        placeholder = { Text("Think out loud…") },
                        maxLines = 4,
                        modifier = Modifier
                            .weight(1f)
                            .testTag("tara-input"),
                    )
                    Button(
                        onClick = { send() },
                        enabled = draft.isNotBlank() && !busy,
                        modifier = Modifier.testTag("tara-send"),
                    ) { Text(if (busy) "…" else "Send") }
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        "Not a therapist or crisis service. Stays on this phone.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.weight(1f),
                    )
                    if (!messages.isNullOrEmpty() && !busy) {
                        TextButton(
                            onClick = { confirmClear = true },
                            modifier = Modifier.testTag("tara-clear"),
                        ) { Text("Clear") }
                    }
                }
                error?.let {
                    Text(
                        it,
                        color = MaterialTheme.colorScheme.error,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
        }
    }

    if (confirmClear) {
        val dialogShape = RoundedCornerShape(ComradeRadii.xl)
        AlertDialog(
            onDismissRequest = { confirmClear = false },
            modifier = Modifier.glassSurface(GlassElevation.Sheet, shape = dialogShape),
            shape = dialogShape,
            containerColor = Color.Transparent,
            title = { Text("Clear this conversation?") },
            text = { Text("Every message will be removed from this phone. There is no other copy.") },
            confirmButton = {
                TextButton(
                    onClick = {
                        confirmClear = false
                        scope.launch {
                            withContext(Dispatchers.IO) {
                                runCatching { ComradeCore.clearTaraThreadTyped() }
                            }
                            reload()
                        }
                    },
                ) { Text("Clear") }
            },
            dismissButton = {
                TextButton(onClick = { confirmClear = false }) { Text("Cancel") }
            },
        )
    }
}

@Composable
private fun TaraBubble(text: String, fromTara: Boolean, tag: String? = null) {
    Row(Modifier.fillMaxWidth()) {
        if (!fromTara) Spacer(Modifier.weight(1f))
        Card(
            colors = CardDefaults.cardColors(
                containerColor = if (fromTara) {
                    MaterialTheme.colorScheme.surfaceVariant
                } else {
                    MaterialTheme.colorScheme.primaryContainer
                },
            ),
            modifier = Modifier
                .widthIn(max = 300.dp)
                .then(if (tag != null) Modifier.testTag(tag) else Modifier),
        ) {
            Text(
                text,
                style = MaterialTheme.typography.bodyLarge,
                modifier = Modifier.padding(horizontal = 12.dp, vertical = 8.dp),
            )
        }
        if (fromTara) Spacer(Modifier.weight(1f))
    }
}

/**
 * The "…" that fills the gap between sending and the first streamed chunk.
 * Hand-rolled from a delay loop rather than the animation APIs so this screen
 * adds no dependency the app doesn't already declare.
 */
@Composable
private fun ThinkingBubble() {
    var dots by remember { mutableStateOf(1) }
    LaunchedEffect(Unit) {
        while (true) {
            delay(350)
            dots = if (dots >= 3) 1 else dots + 1
        }
    }
    TaraBubble(text = ".".repeat(dots), fromTara = true, tag = "tara-thinking")
}

/**
 * Real places to turn — rendered under any reply that detected distress.
 *
 * The chat composer cannot use this (its note is one `Text`, not a card slot),
 * so it renders the same resources as lines through `ChatCommands.crisisLines`.
 * Both paths exist because the honesty gate (`AUDIT.md` §8) applies to *any*
 * reply that tripped the detector, not only the ones in this tab.
 */
@Composable
private fun CrisisCard(resources: List<ComradeCore.CrisisResourceInfo>) {
    if (resources.isEmpty()) return
    Card(
        colors = CardDefaults.cardColors(
            containerColor = MaterialTheme.colorScheme.errorContainer,
        ),
        modifier = Modifier
            .fillMaxWidth()
            .testTag("tara-crisis-card"),
    ) {
        Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(
                "You don't have to carry this alone",
                style = MaterialTheme.typography.titleSmall,
                color = MaterialTheme.colorScheme.onErrorContainer,
            )
            resources.forEach { r ->
                Column {
                    Text(
                        "${r.name} — ${r.contact}",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onErrorContainer,
                    )
                    Text(
                        r.note,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onErrorContainer,
                    )
                }
            }
        }
    }
}
