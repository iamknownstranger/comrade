package mullu.comrade.ui

import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AssistChip
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ElevatedCard
import androidx.compose.material3.FilterChip
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedCard
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import mullu.comrade.ComradeCore
import mullu.comrade.R
import mullu.comrade.travel.TravelDecisions
import mullu.comrade.travel.TravelLocation

/**
 * The Travel tab — where the locals eat, what there is to do, and what this
 * place is (`docs/TRAVEL.md`).
 *
 * Deliberately thin, like `RideScreen`. Every rule worth arguing about is
 * elsewhere and is tested: what counts as legendary, how places are ranked and
 * how far the coordinate is blurred live in `comrade_core::travel`; when a fix
 * is too old, when a walk is far enough to re-query, and how a rating reads at
 * a glance live in [TravelDecisions], which is pure and JVM-tested. This file
 * turns those answers into widgets and nothing more.
 *
 * **No early returns.** Every branch is an `if`/`when` inside the layout, for
 * the reason `.claude/rules/android.md` gives: an early `return@Column` changes
 * the composable-group count across a branch and kills the screen on the
 * *recomposition*, not the first frame — which has already happened twice in
 * this app.
 */
@Composable
fun TravelScreen(modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()

    var hasPermission by remember { mutableStateOf(TravelLocation.hasPermission(context)) }
    var fix by remember { mutableStateOf<TravelDecisions.Fix?>(null) }
    var guide by remember { mutableStateOf<TravelDecisions.Guide?>(null) }
    var loading by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var permissionRefused by remember { mutableStateOf(false) }
    var radiusM by rememberSaveable { mutableIntStateOf(DEFAULT_RADIUS_M) }

    val askForLocation = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { granted ->
        hasPermission = granted
        permissionRefused = !granted
    }

    // The one place the blocking core call happens, on IO. `refresh` is the
    // user saying "ask again", which bypasses [TravelDecisions.shouldRefetch]
    // entirely — second-guessing an explicit pull is how a refresh button ends
    // up doing nothing.
    fun load(refresh: Boolean) {
        if (loading) return
        scope.launch {
            loading = true
            error = null
            val newFix = withContext(Dispatchers.IO) { TravelLocation.currentFix(context) }
            if (newFix != null) {
                val shouldFetch = refresh || TravelDecisions.shouldRefetch(
                    lastFix = fix,
                    newFix = newFix,
                    guideFetchedAtMs = guide?.fetchedAt?.times(1_000),
                    nowMs = System.currentTimeMillis(),
                )
                fix = newFix
                if (shouldFetch) {
                    runCatching {
                        withContext(Dispatchers.IO) {
                            ComradeCore.travelGuideTyped(newFix.lat, newFix.lon, radiusM, refresh)
                        }
                    }.onSuccess { guide = it }
                        .onFailure { error = it.message ?: "Could not look around here" }
                }
            }
            loading = false
        }
    }

    // Radius is in the key on purpose: changing "how far to look" is a request
    // for a different guide, not a filter over the one on screen.
    LaunchedEffect(hasPermission, radiusM) {
        if (hasPermission) load(refresh = true)
    }

    val state = TravelDecisions.state(
        hasPermission = hasPermission,
        fix = fix,
        nowMs = System.currentTimeMillis(),
        loading = loading,
        guide = guide,
        error = error,
    )

    Column(
        // The screen root, tagged because the device test needs something that
        // identifies *this screen* whatever the location state: `travel_guide`
        // below only exists once a guide loads, and the word "Travel" is also
        // the drawer item's label, so a text finder matches the drawer entry
        // still composed behind this screen as well as the bar above it.
        modifier = modifier.fillMaxSize().testTag("travel-screen"),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        // A thin bar rather than a full-screen spinner once there is something
        // to look at: a refresh should not blank a guide the user is reading.
        if (loading && guide != null) {
            LinearProgressIndicator(Modifier.fillMaxWidth())
        }
        when (state) {
            TravelDecisions.State.NeedsPermission -> PermissionPrompt(
                refused = permissionRefused,
                onAsk = { askForLocation.launch(TravelLocation.PERMISSION) },
            )

            TravelDecisions.State.Locating -> Centered {
                CircularProgressIndicator()
                Text(stringResource(R.string.travel_locating))
            }

            TravelDecisions.State.Loading -> Centered {
                CircularProgressIndicator()
                Text(stringResource(R.string.travel_loading))
            }

            TravelDecisions.State.NoFix -> Centered {
                Text(stringResource(R.string.travel_no_fix))
                Button(onClick = { load(refresh = true) }) {
                    Text(stringResource(R.string.travel_retry))
                }
            }

            is TravelDecisions.State.Failed -> Centered {
                Text(state.message, style = MaterialTheme.typography.bodyMedium)
                Button(onClick = { load(refresh = true) }) {
                    Text(stringResource(R.string.travel_retry))
                }
            }

            is TravelDecisions.State.Ready -> GuideList(
                guide = state.guide,
                radiusM = radiusM,
                onRadius = { radiusM = it },
                onRefresh = { load(refresh = true) },
                onOpen = { place -> openPlace(context, place) },
                onOpenUrl = { url -> openUrl(context, url) },
            )
        }
    }
}

/** The default "how far to look" — a walk, matching `travel::DEFAULT_RADIUS_M`. */
private const val DEFAULT_RADIUS_M = 2_000

/** The radii offered. Clamped again in core, so this list cannot ask for a country. */
private val RADIUS_CHOICES = listOf(1_000 to "1 km", 2_000 to "2 km", 5_000 to "5 km")

@Composable
private fun GuideList(
    guide: TravelDecisions.Guide,
    radiusM: Int,
    onRadius: (Int) -> Unit,
    onRefresh: () -> Unit,
    onOpen: (TravelDecisions.Place) -> Unit,
    onOpenUrl: (String) -> Unit,
) {
    val nowMs = System.currentTimeMillis()
    LazyColumn(
        modifier = Modifier.fillMaxSize().testTag("travel_guide"),
        contentPadding = androidx.compose.foundation.layout.PaddingValues(16.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        item(key = "header") {
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                guide.area?.let {
                    Text(it, style = MaterialTheme.typography.titleLarge)
                }
                Row(
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        stringResource(R.string.travel_radius),
                        style = MaterialTheme.typography.labelMedium,
                    )
                    RADIUS_CHOICES.forEach { (metres, label) ->
                        FilterChip(
                            selected = radiusM == metres,
                            onClick = { onRadius(metres) },
                            label = { Text(label) },
                        )
                    }
                }
                TravelDecisions.lastCheckedLine(guide, nowMs)?.let {
                    Text(
                        it,
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                // The privacy line lives on the tab, not buried in settings,
                // because it is the reason somebody might use this at all.
                Text(
                    stringResource(R.string.travel_privacy_note),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                TravelDecisions.ratingsExplainer(guide)?.let {
                    OutlinedCard(Modifier.fillMaxWidth()) {
                        Text(
                            it,
                            style = MaterialTheme.typography.bodySmall,
                            modifier = Modifier.padding(12.dp),
                        )
                    }
                }
                OutlinedButton(onClick = onRefresh) {
                    Text(stringResource(R.string.travel_refresh))
                }
            }
        }

        if (TravelDecisions.isEmpty(guide)) {
            item(key = "empty") {
                Text(
                    stringResource(R.string.travel_empty),
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
        }

        if (guide.eat.isNotEmpty()) {
            item(key = "eat_heading") { SectionHeading(R.string.travel_eat_heading) }
            items(guide.eat, key = { "eat:${it.id}" }) { place ->
                PlaceCard(place, onOpen)
            }
        }

        if (guide.thingsToDo.isNotEmpty()) {
            item(key = "do_heading") { SectionHeading(R.string.travel_do_heading) }
            items(guide.thingsToDo, key = { "do:${it.id}" }) { place ->
                PlaceCard(place, onOpen)
            }
        }

        if (guide.facts.isNotEmpty()) {
            item(key = "facts_heading") { SectionHeading(R.string.travel_facts_heading) }
            items(guide.facts, key = { "fact:${it.title}" }) { fact ->
                FactCard(fact, onOpenUrl)
            }
        }
    }
}

@Composable
private fun SectionHeading(resId: Int) {
    Text(
        stringResource(resId),
        style = MaterialTheme.typography.titleMedium,
        fontWeight = FontWeight.SemiBold,
        modifier = Modifier.padding(top = 8.dp),
    )
}

@Composable
private fun PlaceCard(place: TravelDecisions.Place, onOpen: (TravelDecisions.Place) -> Unit) {
    ElevatedCard(Modifier.fillMaxWidth()) {
        Column(
            Modifier.padding(14.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Row(
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    place.name,
                    style = MaterialTheme.typography.titleMedium,
                    modifier = Modifier.weight(1f),
                )
                TravelDecisions.badge(place)?.let {
                    AssistChip(onClick = { onOpen(place) }, label = { Text(it) })
                }
            }
            Text(
                "${TravelDecisions.kindLabel(place.kind)} · " +
                    TravelDecisions.formatDistance(place.distanceM),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Row(
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    TravelDecisions.ratingLine(place),
                    style = MaterialTheme.typography.bodyMedium,
                )
                // A card that shows a number has to be able to say who said it.
                TravelDecisions.sourceLabel(place.source)?.let {
                    Text(
                        it,
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            place.cuisine?.let {
                Text(it, style = MaterialTheme.typography.bodySmall)
            }
            place.note?.let {
                Text(it, style = MaterialTheme.typography.bodySmall)
            }
            place.address?.let {
                Text(
                    it,
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            TextButton(onClick = { onOpen(place) }) {
                Text(stringResource(R.string.travel_open))
            }
        }
    }
}

@Composable
private fun FactCard(fact: TravelDecisions.Fact, onOpenUrl: (String) -> Unit) {
    OutlinedCard(Modifier.fillMaxWidth()) {
        Column(
            Modifier.padding(14.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text(fact.title, style = MaterialTheme.typography.titleSmall)
            fact.distanceM?.let {
                Text(
                    TravelDecisions.formatDistance(it),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Text(fact.text, style = MaterialTheme.typography.bodySmall)
            fact.url?.let { url ->
                TextButton(onClick = { onOpenUrl(url) }) {
                    Text(stringResource(R.string.travel_read_more))
                }
            }
        }
    }
}

@Composable
private fun PermissionPrompt(refused: Boolean, onAsk: () -> Unit) {
    Centered {
        Text(
            stringResource(
                if (refused) R.string.travel_permission_denied else R.string.travel_need_permission,
            ),
            style = MaterialTheme.typography.bodyMedium,
        )
        Text(
            stringResource(R.string.travel_privacy_note),
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Button(onClick = onAsk) { Text(stringResource(R.string.travel_grant_location)) }
    }
}

@Composable
private fun Centered(content: @Composable () -> Unit) {
    Column(
        modifier = Modifier.fillMaxSize().padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        content()
    }
}

/**
 * Hand a place to whatever the user uses for maps.
 *
 * The provider's own canonical link when there is one, and a `geo:` URI
 * carrying the coordinate otherwise — never a name-only search, which is how a
 * tap lands at another branch of the same chain (see
 * [TravelDecisions.openTarget]). A device with nothing that handles either is
 * left alone rather than crashed.
 */
private fun openPlace(context: android.content.Context, place: TravelDecisions.Place) {
    openUrl(context, TravelDecisions.openTarget(place))
}

private fun openUrl(context: android.content.Context, url: String) {
    val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url))
    try {
        context.startActivity(intent)
    } catch (e: ActivityNotFoundException) {
        android.util.Log.w("TravelScreen", "nothing handles $url: ${e.message}")
    }
}
