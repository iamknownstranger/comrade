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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import mullu.comrade.ComradeCore

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

@Composable
private fun TaraThread(modifier: Modifier = Modifier) {
    val scope = rememberCoroutineScope()
    var messages by remember { mutableStateOf<List<ComradeCore.TaraMessageInfo>?>(null) }
    var opener by remember { mutableStateOf<String?>(null) }
    var crisisResources by remember { mutableStateOf<List<ComradeCore.CrisisResourceInfo>>(emptyList()) }
    var draft by remember { mutableStateOf("") }
    var sending by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var confirmClear by remember { mutableStateOf(false) }
    val listState = rememberLazyListState()

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

    // Keep the newest turn visible as the thread grows.
    LaunchedEffect(messages?.size) {
        val count = messages?.size ?: 0
        if (count > 0) listState.animateScrollToItem(count - 1)
    }

    fun send() {
        val text = draft.trim()
        if (text.isEmpty() || sending) return
        sending = true
        error = null
        scope.launch {
            runCatching {
                withContext(Dispatchers.IO) { ComradeCore.taraSendTyped(text) }
            }.onSuccess {
                draft = ""
                sending = false
                reload()
            }.onFailure {
                sending = false
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
                    Box(Modifier.fillMaxWidth().padding(top = 24.dp)) {
                        CircularProgressIndicator(Modifier.align(Alignment.Center))
                    }
                }
                list.isEmpty() -> item(key = "opener") {
                    opener?.let { TaraBubble(text = it, fromTara = true) }
                }
                else -> items(list, key = { it.id }) { msg ->
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        TaraBubble(text = msg.text, fromTara = msg.fromTara)
                        if (msg.crisis && msg.fromTara) CrisisCard(crisisResources)
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
                        enabled = draft.isNotBlank() && !sending,
                        modifier = Modifier.testTag("tara-send"),
                    ) { Text(if (sending) "…" else "Send") }
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        "Not a therapist or crisis service. Stays on this phone.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.weight(1f),
                    )
                    if (!messages.isNullOrEmpty()) {
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
        AlertDialog(
            onDismissRequest = { confirmClear = false },
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
private fun TaraBubble(text: String, fromTara: Boolean) {
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
            modifier = Modifier.widthIn(max = 300.dp),
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

/** Real places to turn — rendered under any reply that detected distress. */
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
