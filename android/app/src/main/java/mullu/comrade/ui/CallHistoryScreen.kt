package mullu.comrade.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import mullu.comrade.ComradeCore
import mullu.comrade.R

/** Outcomes a call-history row renders with the "problem" (error) tint. */
private val ProblemOutcomes = setOf("missed", "declined", "busy", "failed")

/**
 * Call history: every logged voice/video call, incoming or outgoing, newest
 * first — [ComradeCore.callHistoryTyped] already returns it in that order, so
 * no client-side sorting is needed. Missed/declined/busy/failed rows get the
 * same error tint a phone dialer would use, so a missed call stands out at a
 * glance; tapping any row calls that peer back via [onCallBack].
 */
@Composable
fun CallHistoryScreen(
    onCallBack: (peer: String, peerLabel: String, video: Boolean) -> Unit,
    modifier: Modifier = Modifier,
) {
    var records by remember { mutableStateOf<List<ComradeCore.CallRecordInfo>?>(null) }
    var contactsByNpub by remember { mutableStateOf<Map<String, ComradeCore.ContactInfo>>(emptyMap()) }

    LaunchedEffect(Unit) {
        val (history, contactList) = withContext(Dispatchers.IO) {
            val history = runCatching { ComradeCore.callHistoryTyped() }.getOrDefault(emptyList())
            val contactList = runCatching { ComradeCore.contacts() }.getOrDefault(emptyList())
            history to contactList
        }
        records = history
        contactsByNpub = contactList.associateBy { it.npub }
    }

    val list = records
    when {
        list == null -> Box(modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            CircularProgressIndicator(Modifier.size(28.dp))
        }
        list.isEmpty() -> Box(
            modifier.fillMaxSize().padding(32.dp),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                stringResource(R.string.call_history_empty),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        else -> LazyColumn(modifier = modifier.fillMaxSize()) {
            items(list, key = { it.id }) { record ->
                val contact = contactsByNpub[record.peer]
                val title = peerTitle(record.peer, contact?.alias, contact?.name)
                CallHistoryRow(
                    record = record,
                    title = title,
                    onClick = { onCallBack(record.peer, title, record.media == "video") },
                )
                HorizontalDivider(
                    modifier = Modifier.padding(start = 76.dp),
                    color = MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.4f),
                )
            }
        }
    }
}

@Composable
private fun CallHistoryRow(
    record: ComradeCore.CallRecordInfo,
    title: String,
    onClick: () -> Unit,
) {
    val isVideo = record.media == "video"
    val problem = record.outcome in ProblemOutcomes
    val tint = if (problem) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant
    val direction = stringResource(
        if (record.incoming) R.string.call_direction_incoming else R.string.call_direction_outgoing,
    )
    val outcomeLabel = when (record.outcome) {
        "connected" -> formatCallDuration(record.durationSecs)
        "missed" -> stringResource(R.string.call_outcome_missed)
        "declined" -> stringResource(R.string.call_outcome_declined)
        "busy" -> stringResource(R.string.call_outcome_busy)
        "cancelled" -> stringResource(R.string.call_outcome_cancelled)
        "failed" -> stringResource(R.string.call_outcome_failed)
        else -> record.outcome
    }
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable { onClick() }
            .padding(horizontal = 16.dp, vertical = 11.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        PeerAvatar(title, seed = record.peer)
        Column(Modifier.weight(1f)) {
            Text(
                title,
                style = MaterialTheme.typography.titleMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                "$direction · $outcomeLabel · ${relativeTime(record.startedAt)}",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Icon(
            if (isVideo) VideocamIcon else CallIcon,
            contentDescription = stringResource(if (isVideo) R.string.call_video else R.string.call_voice),
            tint = tint,
        )
    }
}
