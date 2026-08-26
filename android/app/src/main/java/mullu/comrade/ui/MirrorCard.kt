package mullu.comrade.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedCard
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import mullu.comrade.ComradeCore
import mullu.comrade.R
import mullu.comrade.ui.theme.ComradeRadii
import mullu.comrade.ui.theme.GlassElevation
import mullu.comrade.ui.theme.glassSurface

/**
 * The attention **mirror** — phase 1 of `docs/ATTENTION.md`.
 *
 * Lives on the Journal tab rather than in a dashboard of its own, and that
 * placement is the design: seeing the number matters far less than what you do
 * next with it, and what this app offers next is reflection. A screen-time
 * dashboard people visit to feel bad is the thing being treated.
 *
 * Three rules the rendering keeps:
 * - **Self-relative only.** Today sits next to *this user's* own 7-day median.
 *   There is no target, no normative "healthy" figure, and no other user.
 * - **No red.** The comparison is stated in words, in ordinary colours. A red
 *   bar is a verdict, and this card does not get to reach one.
 * - **Honest about thin data.** With fewer than two prior days there is no
 *   "usual" to speak of, and it says so instead of inventing one.
 */
@Composable
fun MirrorCard(
    summary: ComradeCore.AttentionSummaryInfo?,
    hasAccess: Boolean,
    onEnable: () -> Unit,
    onPickApps: () -> Unit,
    modifier: Modifier = Modifier,
) {
    if (!hasAccess) {
        MirrorInviteCard(onEnable = onEnable, modifier = modifier)
        return
    }
    val today = summary?.today
    OutlinedCard(modifier.fillMaxWidth().testTag("mirror-card")) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(
                stringResource(R.string.mirror_title),
                style = MaterialTheme.typography.titleSmall,
                color = MaterialTheme.colorScheme.primary,
            )
            Text(
                stringResource(R.string.mirror_screen_time, formatMinutes(today?.screenMinutes ?: 0)),
                style = MaterialTheme.typography.bodyLarge,
            )
            Text(
                stringResource(R.string.mirror_pickups, today?.pickups ?: 0),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            if ((today?.doomMinutes ?: 0) > 0) {
                Text(
                    stringResource(R.string.mirror_doom_time, formatMinutes(today!!.doomMinutes)),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            // The comparison, in words. Fewer than two prior days is not a
            // baseline, and calling it one would be the card lying quietly.
            if (summary != null && summary.sampleDays >= 2) {
                Text(
                    stringResource(
                        R.string.mirror_vs_median,
                        formatMinutes(summary.medianScreenMinutes),
                    ),
                    style = MaterialTheme.typography.bodyMedium,
                )
                RelativeBar(
                    today = today?.screenMinutes ?: 0,
                    median = summary.medianScreenMinutes,
                )
            } else {
                Text(
                    stringResource(R.string.mirror_no_baseline),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            OutlinedButton(onClick = onPickApps, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.mirror_pick_apps))
            }
            Text(
                stringResource(R.string.mirror_not_a_score),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.outline,
            )
        }
    }
}

/**
 * Today against the user's own median, as two plain bars.
 *
 * Both bars use the same neutral colour on purpose. A version of this that
 * turned red above the median would be a scold, and one that turned green below
 * it would be a reward — either way it becomes a score, which gate 1 forbids.
 */
@Composable
private fun RelativeBar(today: Int, median: Int) {
    val scale = maxOf(today, median, 1).toFloat()
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        BarRow(label = "Today", fraction = today / scale)
        BarRow(label = "Usual", fraction = median / scale, muted = true)
    }
}

@Composable
private fun BarRow(label: String, fraction: Float, muted: Boolean = false) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(
            label,
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.width(44.dp),
        )
        Box(
            Modifier
                .weight(1f)
                .height(8.dp),
        ) {
            Surface(
                color = MaterialTheme.colorScheme.surfaceVariant,
                shape = RoundedCornerShape(4.dp),
                modifier = Modifier.fillMaxWidth().height(8.dp),
            ) {}
            Surface(
                color = if (muted) {
                    MaterialTheme.colorScheme.outlineVariant
                } else {
                    MaterialTheme.colorScheme.primary
                },
                shape = RoundedCornerShape(4.dp),
                modifier = Modifier
                    .fillMaxWidth(fraction.coerceIn(0f, 1f))
                    .height(8.dp),
            ) {}
        }
    }
}

/**
 * The opt-in invitation, shown until usage access is granted.
 *
 * It sells nothing: it states what Android would tell Comrade, what Comrade
 * keeps (three integers), where that lives (the same encrypted store as the
 * journal), and that it cannot go anywhere because there is no server. "Not
 * now" is a real option and stays available — every other part of this pillar
 * works without this permission.
 */
@Composable
private fun MirrorInviteCard(onEnable: () -> Unit, modifier: Modifier = Modifier) {
    OutlinedCard(modifier.fillMaxWidth().testTag("mirror-invite")) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(
                stringResource(R.string.mirror_enable_title),
                style = MaterialTheme.typography.titleSmall,
                color = MaterialTheme.colorScheme.primary,
            )
            Text(
                stringResource(R.string.mirror_enable_body),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            OutlinedButton(
                onClick = onEnable,
                modifier = Modifier.fillMaxWidth().testTag("mirror-enable"),
            ) { Text(stringResource(R.string.mirror_enable_action)) }
        }
    }
}

/**
 * The doom-app picker: the user's own list, with nothing pre-selected.
 *
 * `apps` is `(packageName, label)` for the installed, launchable apps.
 */
@Composable
fun DoomAppPicker(
    apps: List<Pair<String, String>>,
    selected: Set<String>,
    onToggle: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    val dialogShape = RoundedCornerShape(ComradeRadii.xl)
    AlertDialog(
        onDismissRequest = onDismiss,
        modifier = Modifier.glassSurface(GlassElevation.Sheet, shape = dialogShape),
        shape = dialogShape,
        containerColor = Color.Transparent,
        title = { Text(stringResource(R.string.mirror_pick_apps_title)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    stringResource(R.string.mirror_pick_apps_body),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                if (apps.isEmpty()) {
                    Text(
                        stringResource(R.string.mirror_pick_apps_none),
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                // Scrollable rather than paged: the list is the device's own
                // launchable apps, which is dozens, not thousands.
                LazyColumn(
                    Modifier.height(320.dp),
                    verticalArrangement = Arrangement.spacedBy(2.dp),
                ) {
                    items(apps.size) { index ->
                        val (pkg, label) = apps[index]
                        Row(
                            Modifier
                                .fillMaxWidth()
                                .padding(vertical = 2.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.SpaceBetween,
                        ) {
                            Text(
                                label,
                                style = MaterialTheme.typography.bodyMedium,
                                modifier = Modifier.weight(1f),
                            )
                            Checkbox(
                                checked = pkg in selected,
                                onCheckedChange = { onToggle(pkg) },
                            )
                        }
                    }
                }
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.close)) }
        },
    )
}

/** "1h 20m" / "45m" — never a bare minute count for a long day. */
fun formatMinutes(minutes: Int): String {
    if (minutes < 60) return "${minutes}m"
    val h = minutes / 60
    val m = minutes % 60
    return if (m == 0) "${h}h" else "${h}h ${m}m"
}
