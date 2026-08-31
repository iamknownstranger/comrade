package mullu.comrade.ui

import android.content.Context
import android.graphics.Bitmap
import android.media.projection.MediaProjectionManager
import android.net.Uri
import android.os.Build
import android.util.Log
import android.view.SurfaceHolder
import android.view.SurfaceView
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Person
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ElevatedCard
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Slider
import androidx.compose.material3.Switch
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.scale
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import com.pierfrancescosoffritti.androidyoutubeplayer.core.player.options.IFramePlayerOptions
import com.pierfrancescosoffritti.androidyoutubeplayer.core.player.views.YouTubePlayerView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.webrtc.RendererCommon
import org.webrtc.SurfaceViewRenderer
import org.webrtc.VideoTrack
import mullu.comrade.ComradeCore
import mullu.comrade.R
import mullu.comrade.call.CallManager
import mullu.comrade.together.LibraryResolver
import mullu.comrade.together.MediaLibraryAccess
import mullu.comrade.together.MediaSessionAccess
import mullu.comrade.together.MusicDownloads
import mullu.comrade.together.MusicLibrary
import mullu.comrade.together.PlaybackModeDecision
import mullu.comrade.together.PlaybackOwnership
import mullu.comrade.together.ShareTransfer
import mullu.comrade.together.PlayerEffects
import mullu.comrade.together.PlayerPrefs
import mullu.comrade.together.StreamingSourcesStore
import mullu.comrade.together.TogetherPlayer
import mullu.comrade.together.TogetherDecisions
import mullu.comrade.together.TogetherManager
import mullu.comrade.ui.theme.ComradeRadii
import mullu.comrade.ui.theme.GlassElevation
import mullu.comrade.ui.theme.Spacing
import mullu.comrade.ui.theme.glassSurface
import java.util.Locale

/**
 * The listen-together surface, and **the only way into a session** since
 * 2026-08-08.
 *
 * It used to be a screen you arrived at, having started a session from a ▶ in a
 * conversation's header. That button is gone and this is the whole flow: choose
 * something — the music on this phone, a file, or a link — then choose who to
 * listen with, and they get asked. One place, so "how do I listen with someone"
 * has one answer instead of depending on which screen you happen to be on.
 *
 * ## What still is not here
 * No sync logic at all. Every decision about what the player does lives in
 * [TogetherDecisions] (pure, unit-tested — the only half of this feature the JVM
 * lane can check before CI) and `comrade_core::together` (shared with desktop),
 * and the session outlives this composition because [TogetherManager] and its
 * foreground service own it. Disposing this screen must not stop the music.
 *
 * The two rules in the copy are the ones `docs/PRESENCE.md` §5 sets and
 * `docs/TOGETHER.md` §7 repeats: it never says "synced" or "in sync", because we
 * do not know that; and when the heartbeats stop it says we lost track of
 * *them*, because that is what we observed.
 *
 * @param onPickFileWith open the file picker to start a session with this person
 * @param onPickFileToJoin open the file picker to answer an invitation
 */
@Composable
fun TogetherScreen(
    onPickFileWith: (peer: String, label: String) -> Unit,
    onPickFileToJoin: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val state by TogetherManager.state.collectAsState()
    val context = LocalContext.current

    // An invitation is the one moment reading the library is obviously worth
    // something, so it is the one place besides `/play` that asks. Two separate
    // flags rather than one tri-state, because "refused" and "looked and it
    // isn't here" are different answers and only the second one has anything to
    // say — after a refusal the Join button already offers the route that needs
    // no permission. Hoisted out of the `when` so the launcher is created on
    // every composition rather than only while an invitation is showing.
    var libraryAsked by remember { mutableStateOf(false) }
    var libraryMissed by remember { mutableStateOf(false) }
    // Following another app is a *special access*: there is no dialog to launch
    // and no result to receive, only a system settings screen the user may or
    // may not have acted on. So the state here is the refusal to explain, and it
    // is re-asked on the next tap rather than watched for — which is also the
    // only honest way to detect the grant, since coming back from settings
    // produces no callback of any kind.
    var followRefusal by remember { mutableStateOf<TogetherManager.FollowRefusal?>(null) }

    // Streaming what this device is playing (docs/TOGETHER.md §15). Two system
    // prompts in sequence, both hoisted out of the `when` for the same reason
    // the library launcher is: a launcher created inside a branch is created and
    // destroyed as the session changes state.
    //
    // RECORD_AUDIO first, and it is worth knowing *why* a feature that is not
    // about the microphone asks for it: the media audio joins the outgoing
    // stream on the capture path, so there has to be a capture running at all.
    // A refusal is not fatal — the picture still goes.
    val askToCapture = rememberLauncherForActivityResult(
        ActivityResultContracts.StartActivityForResult(),
    ) { result ->
        // Refused is a real answer: the picture streams with no sound, rather
        // than nothing happening and the button looking broken.
        TogetherManager.startStreamingFromConsent(context, result.resultCode, result.data)
    }
    val askToRecord = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) {
        // Granted or not, go on to the capture consent — the two failures are
        // independent and the picture does not depend on either.
        val manager = context.getSystemService(MediaProjectionManager::class.java)
        val intent = manager?.createScreenCaptureIntent()
        if (intent != null) askToCapture.launch(intent)
    }
    val askToReadLibrary = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { granted ->
        runCatching { MediaLibraryAccess.rememberAsked(context) }
        libraryAsked = true
        // A match starts the session, so this only ever reads true on the
        // "allowed to look, and it is genuinely not here" path.
        libraryMissed = granted && !TogetherManager.lookAgain(context)
    }
    // The microphone, which is now offered in every mode rather than only in a
    // streamed one. A separate launcher from `askToRecord` above even though
    // both ask for `RECORD_AUDIO`, because what follows a grant is completely
    // different: that one goes on to the screen-capture consent, this one opens
    // a voice channel and nothing else.
    //
    // The microphone cannot be switched on right now, and it is not the
    // permission. One case only: a picture is already arriving on the single
    // connection this session has, and adding our voice to it means
    // renegotiating, which would take the picture away to add a microphone.
    var micBlocked by remember { mutableStateOf(false) }
    val askToTalk = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { granted ->
        // A refusal is silent on purpose: they were just shown the system
        // dialog and said no, so a sentence explaining the dialog they closed
        // would be telling them what they already decided.
        if (granted) micBlocked = !TogetherManager.toggleMic(context)
    }

    // Choosing something *else* to play, without being asked who with again.
    // Held here rather than inside `LiveSession` because it survives the
    // session states the chooser passes through — it is open across the end of
    // the old session and the start of the new one.
    var choosingAgain by remember { mutableStateOf(false) }

    TogetherOverlay(modifier) {
        val s = state
        // The chooser covers two cases and they are the same screen: nothing is
        // playing, or something is and they want something else. The pairing is
        // what carries between them — `PlayerHome` reads it and skips the
        // who-with sheet, so putting a second thing on is one step rather than
        // starting over.
        //
        // Written as an `if` rather than a `when` guard because guards are a
        // Kotlin 2.1 feature and this module is on 1.9.22.
        val choosing = s is TogetherManager.UiState.Idle ||
            (s is TogetherManager.UiState.Live && choosingAgain)
        if (choosing) {
            // Not wrapped in the scrolling column below: it owns its own
            // scrolling, because a library of two thousand tracks is a
            // LazyColumn and nesting one inside a `verticalScroll` measures
            // every row.
            PlayerHome(
                onPickFileWith = onPickFileWith,
                onClose = if (choosingAgain) ({ choosingAgain = false }) else null,
                onStarted = { choosingAgain = false },
            )
        } else {
            Column(
                modifier = Modifier
                    .fillMaxSize()
                    // A video surface plus controls plus the two honest notes
                    // overflows a short screen in landscape, which is exactly
                    // the orientation a film is watched in.
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = 20.dp, vertical = 16.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                when (s) {
                    is TogetherManager.UiState.Invited -> {
                        Text(
                            stringResource(R.string.together_invited, s.peerLabel, s.title),
                            style = MaterialTheme.typography.titleMedium,
                        )
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            // **What Join does is `TogetherDecisions.joinAction`'s
                            // to say, and it changed on 2026-08-08.** It used to
                            // open the document picker for a local file and ask
                            // the person to find their own copy — of the thing
                            // the invitation exists *because* they do not have.
                            // The answer that gets them to a player is their
                            // copy, over the session's own connection, playing
                            // as it lands (§12); the picker is the second
                            // answer, for whoever does have the file and would
                            // rather not spend the bytes.
                            when (TogetherDecisions.joinAction(s.youtube, s.contentKind)) {
                                TogetherDecisions.JoinAction.WatchVideo ->
                                    Button(onClick = { TogetherManager.joinEmbed(context) }) {
                                        Text(stringResource(R.string.together_join_video))
                                    }

                                TogetherDecisions.JoinAction.FetchTheStream ->
                                    Button(onClick = { TogetherManager.joinStream(context) }) {
                                        Text(stringResource(R.string.together_join_stream))
                                    }

                                TogetherDecisions.JoinAction.TakeTheirCopy -> {
                                    Button(onClick = { TogetherManager.askForTheirCopy(context) }) {
                                        Text(stringResource(R.string.together_join))
                                    }
                                    TextButton(onClick = onPickFileToJoin) {
                                        Text(stringResource(R.string.together_join_own_copy))
                                    }
                                }
                            }
                            TextButton(onClick = { TogetherManager.leave() }) {
                                Text(stringResource(R.string.together_not_now))
                            }
                        }
                        // What joining a stream actually does, before they do
                        // it: their device fetches a URL the other person named.
                        if (s.contentKind == TogetherDecisions.STREAM_KIND) {
                            Text(
                                stringResource(R.string.together_stream_join_note),
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                        FollowWhatIsPlaying(
                            invited = s,
                            refusal = followRefusal,
                            onTry = { followRefusal = TogetherManager.followExternal(context) },
                        )
                        // Only when a lookup could find anything: a YouTube or
                        // stream invitation names no recording we could match,
                        // and a blank title means none was carried.
                        val couldLook = !s.youtube &&
                            s.contentKind != TogetherDecisions.STREAM_KIND &&
                            s.title.isNotBlank()
                        // The same one-ask rule `/play` follows, and for the
                        // same reason: someone who has already refused gets no
                        // dialog from Android, so offering the button again would
                        // be offering a button that does nothing. `libraryAsked`
                        // is the local half — the preference is what persists,
                        // this is what recomposes.
                        val step = MediaLibraryAccess.next(
                            granted = runCatching { LibraryResolver.mayRead(context) }
                                .getOrDefault(false),
                            askedBefore = libraryAsked ||
                                runCatching { MediaLibraryAccess.asked(context) }.getOrDefault(true),
                        )
                        if (couldLook && step == MediaLibraryAccess.Step.Ask) {
                            TextButton(onClick = {
                                askToReadLibrary.launch(
                                    MediaLibraryAccess.permissionFor(Build.VERSION.SDK_INT),
                                )
                            }) {
                                Text(stringResource(R.string.together_look_in_library))
                            }
                        }
                        if (libraryMissed) {
                            Text(
                                stringResource(R.string.together_library_missed),
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                    }

                    is TogetherManager.UiState.Live -> LiveSession(
                        s = s,
                        onStream = {
                            askToRecord.launch(android.Manifest.permission.RECORD_AUDIO)
                        },
                        micBlocked = micBlocked,
                        onMic = {
                            micBlocked = false
                            if (!TogetherManager.toggleMic(context)) {
                                // Two reasons it can say no, and only one of
                                // them has a dialog behind it.
                                if (TogetherManager.micNeedsPermission(context)) {
                                    askToTalk.launch(android.Manifest.permission.RECORD_AUDIO)
                                } else {
                                    micBlocked = true
                                }
                            }
                        },
                        onPlaySomethingElse = { choosingAgain = true },
                    )

                    // Covered above; the compiler needs the arm.
                    is TogetherManager.UiState.Idle -> Unit
                }

                // Outside the inner `when` on purpose: the relay question can
                // arrive while a handover is running in any of these states, and
                // it must not be possible to leave it unanswered by whatever the
                // session does next.
                ShareRelayConsent()
            }
        }
    }
}

/**
 * The full-screen backdrop this screen is drawn on.
 *
 * `MainActivity` stacks this over the whole app for an invitation, so without a
 * background of its own the session drew as floating text over whatever tab was
 * behind it — the chat list showing through the film's controls — and taps on
 * the gaps between the controls reached that tab instead of stopping here. Both
 * halves of that are fixed in this one composable, matching `CallOverlay` in
 * `call/CallScreen.kt`, which covers the app the same way for the same reason.
 *
 * **It used to paint its own dark blue gradient, and that was the wrong call.**
 * The argument was that a picture wants a dark chrome around it whatever the
 * system theme says — true of a film, and this tab is mostly a music player,
 * reached by tapping a bottom-nav item next to four screens that do follow the
 * theme. So it landed as the one tab that ignored Material You and read as a
 * different app. It now sits on `colorScheme` like everything else; a hard-coded
 * dark room is not worth being the odd one out for.
 */
@Composable
private fun TogetherOverlay(modifier: Modifier, content: @Composable () -> Unit) {
    Box(
        modifier = modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background)
            // Swallow taps that miss a control. Compose routes a tap on an
            // unhandled area to whatever sits behind it, so a background alone
            // would still let someone open a chat through the film.
            .clickable(
                interactionSource = remember { MutableInteractionSource() },
                indication = null,
            ) {},
    ) { content() }
}

/*
 * The screen's five colours, every one of them a `colorScheme` token.
 *
 * Composable getters rather than constants, which is what makes them follow the
 * theme — including Material You, which the rest of the app has and this screen
 * did not. The names are kept short because they appear on nearly every line
 * below; what they must not do is claim a colour the theme is not providing,
 * which is why the old `OnDark`/`CardColor` pair went with the gradient.
 */

/** The sleeve behind the artwork, and the badge behind a source icon. */
private val TogetherSleeve: Color
    @Composable get() = MaterialTheme.colorScheme.surfaceVariant

/** Cards on the backdrop — the same fill the other tabs' rows use. */
private val TogetherCard: Color
    @Composable get() = MaterialTheme.colorScheme.surfaceVariant

private val TogetherText: Color
    @Composable get() = MaterialTheme.colorScheme.onBackground

private val TogetherMuted: Color
    @Composable get() = MaterialTheme.colorScheme.onSurfaceVariant

/** How far the skip buttons move. Matches the desktop transport. */
private const val SKIP_MS: Long = 10_000

// ── Choosing something to play ───────────────────────────────────────────────

/** Where the home screen is in the two-step "what, then who" flow. */
private sealed interface HomeStep {
    data object Choosing : HomeStep
    data object Browsing : HomeStep
    data object Linking : HomeStep
    data object Searching : HomeStep
    data object Streaming : HomeStep
    data object Collections : HomeStep
    data object Loved : HomeStep
    data object Recent : HomeStep
    data object Playlists : HomeStep
}

/**
 * What has been chosen and is waiting for a person to play it with.
 *
 * The "what" is settled before the "who" is asked, which is the order the flow
 * reads in: you find something, then you think of someone. The reverse order was
 * what the old ▶-in-a-chat did, and it made "listen to music with a friend" a
 * thing you could only start from a conversation.
 */
private sealed interface Chosen {
    /**
     * @param queue the list it was picked out of, so prev and next mean the
     *   list the person was looking at rather than the whole library.
     */
    data class Track(
        val track: TogetherDecisions.Track,
        val queue: List<TogetherDecisions.Track>,
        /**
         * Picked out of the phone's own library, which plays rather than asking
         * who with — [TogetherDecisions.startStepInLibrary]'s rule and the
         * reasoning behind it. A catalogue search produces the same
         * [TogetherDecisions.Track] and is **not** the library, so the flag is
         * carried rather than inferred from the type.
         */
        val fromLibrary: Boolean,
    ) : Chosen

    /** A YouTube video or a public media URL, already classified by core. */
    data class Link(val link: TogetherDecisions.Link) : Chosen

    /**
     * A row from your own streaming server (Subsonic/Navidrome).
     *
     * The candidate came from core with its URL **already guarded** —
     * `subsonic_search` drops rows whose URL would fail
     * `valid_stream_url` — so this carries the typed value rather than
     * re-running the paste parser, which a token-authenticated `/rest/stream`
     * URL could never pass (it names no media extension; that check exists for
     * pasted text and this never was one). Core still validates on the way
     * out of `together_start`, so nothing here can smuggle a URL onto the wire.
     */
    data class ServerTrack(val candidate: uniffi.comrade_ui.StreamCandidateDto) : Chosen

    /**
     * A file to be picked once we know who for.
     *
     * The one case where the picker cannot run first: its result arrives in
     * `MainActivity`, which has to know who the session is with before it can
     * start one — so the person is chosen and then the picker opens.
     */
    data object AFile : Chosen
}

/**
 * The tab's own screen: three ways to start, and the people to start with.
 *
 * **"And who with?" is asked once per session, not once per track.** The person
 * is [TogetherManager.pairing] and the step is
 * [TogetherDecisions.startStep], so the sheet appears when there is nobody yet
 * and never again for the length of the session. Choosing something while the
 * *other* person is the one playing is the one case that stops to ask, because
 * it takes their music off on both devices.
 *
 * @param onClose the way back to a session that is already running, or `null`
 *   when this is the idle tab and there is nothing behind it.
 * @param onStarted something began playing, so the caller can put the session
 *   back on screen.
 */
@Composable
private fun PlayerHome(
    onPickFileWith: (peer: String, label: String) -> Unit,
    onClose: (() -> Unit)?,
    onStarted: () -> Unit = {},
) {
    val context = LocalContext.current
    var step by remember { mutableStateOf<HomeStep>(HomeStep.Choosing) }
    var chosen by remember { mutableStateOf<Chosen?>(null) }
    // The person button in the library, armed. Held here rather than in the
    // browser because the sheet it leads to is drawn from here, and it is cleared
    // when a session actually starts — otherwise the tap *after* the one that
    // asked would ask again, having been answered.
    var choosingPerson by remember { mutableStateOf(false) }
    // Chosen, and waiting for a yes because it would stop what they put on.
    var confirming by remember { mutableStateOf<Pair<Chosen, TogetherDecisions.Pairing>?>(null) }
    val pairing by TogetherManager.pairing.collectAsState()
    val live by TogetherManager.state.collectAsState()
    // A start that failed. Held rather than logged and forgotten: core can
    // refuse the content (a stream URL it will not admit), and a tap that
    // produced neither a session nor a sentence is the failure mode the rest of
    // this feature is written to avoid.
    var failed by remember { mutableStateOf(false) }
    // Held rather than derived, because a grant arrives while this screen is on
    // screen: the launcher's result sets it and the browser redraws.
    var libraryGranted by remember {
        mutableStateOf(runCatching { LibraryResolver.mayRead(context) }.getOrDefault(false))
    }
    val askToReadLibrary = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { granted ->
        runCatching { MediaLibraryAccess.rememberAsked(context) }
        libraryGranted = granted
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(horizontal = 20.dp),
    ) {
        if (failed) {
            Text(
                stringResource(R.string.together_start_failed),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.error,
                modifier = Modifier.padding(top = 12.dp),
            )
        }
        when (step) {
            is HomeStep.Choosing -> ChooseASource(
                libraryGranted = libraryGranted,
                onBrowse = { step = HomeStep.Browsing },
                onPickAFile = { chosen = Chosen.AFile },
                onLink = { step = HomeStep.Linking },
                onSearch = { step = HomeStep.Searching },
                onServer = { step = HomeStep.Streaming },
                onCollections = { step = HomeStep.Collections },
                onFavourites = { step = HomeStep.Loved },
                onRecent = { step = HomeStep.Recent },
                onPlaylists = { step = HomeStep.Playlists },
                onClose = onClose,
            )

            is HomeStep.Browsing -> LibraryBrowser(
                libraryGranted = libraryGranted,
                onAsk = {
                    askToReadLibrary.launch(MediaLibraryAccess.permissionFor(Build.VERSION.SDK_INT))
                },
                onBack = { step = HomeStep.Choosing },
                // Both halves gated on the same answer, so an armed flag cannot
                // outlive the button: an invitation accepted while this screen is
                // open puts a real person in the session, and a note still
                // promising to ask who with would be a promise
                // `startStepInLibrary` no longer keeps.
                choosingPerson = choosingPerson && TogetherDecisions.mayChoosePerson(pairing),
                onChoosePerson = if (TogetherDecisions.mayChoosePerson(pairing)) {
                    { choosingPerson = !choosingPerson }
                } else {
                    null
                },
                onPlay = { track, shown ->
                    chosen = Chosen.Track(track, shown, fromLibrary = true)
                },
            )

            is HomeStep.Linking -> LinkField(
                onBack = { step = HomeStep.Choosing },
                onPlay = { link -> chosen = Chosen.Link(link) },
            )

            is HomeStep.Searching -> SearchByName(
                onBack = { step = HomeStep.Choosing },
                onPlay = { chosenTrack, shown ->
                    chosen = Chosen.Track(chosenTrack, shown, fromLibrary = false)
                },
            )

            is HomeStep.Streaming -> ServerSearch(
                onBack = { step = HomeStep.Choosing },
                onPlay = { candidate -> chosen = Chosen.ServerTrack(candidate) },
            )

            is HomeStep.Collections -> CollectionsScreen(
                onBack = { step = HomeStep.Choosing },
                onPlay = { candidate -> chosen = Chosen.ServerTrack(candidate) },
            )

            is HomeStep.Loved -> RememberedList(
                onBack = { step = HomeStep.Choosing },
                kind = RememberedKind.Favourites,
                onPlayLocal = { track, list ->
                    chosen = Chosen.Track(track, list, fromLibrary = true)
                },
                onPlayStream = { candidate -> chosen = Chosen.ServerTrack(candidate) },
            )

            is HomeStep.Recent -> RememberedList(
                onBack = { step = HomeStep.Choosing },
                kind = RememberedKind.History,
                onPlayLocal = { track, list ->
                    chosen = Chosen.Track(track, list, fromLibrary = true)
                },
                onPlayStream = { candidate -> chosen = Chosen.ServerTrack(candidate) },
            )

            is HomeStep.Playlists -> PlaylistsScreen(
                onBack = { step = HomeStep.Choosing },
                onPlayTracks = { tracks, index ->
                    val t = tracks.getOrNull(index) ?: return@PlaylistsScreen
                    if (t.kind == uniffi.comrade_ui.PlayerTrackKind.LOCAL &&
                        t.key.startsWith("local:")
                    ) {
                        chosen = Chosen.Track(
                            TogetherDecisions.Track(
                                uri = t.key.removePrefix("local:"),
                                title = t.title,
                                artist = t.artist,
                                album = t.album,
                                durationMs = t.durationMs.toLong(),
                                albumId = null,
                            ),
                            tracks.filter { it.kind == uniffi.comrade_ui.PlayerTrackKind.LOCAL }
                                .map {
                                    TogetherDecisions.Track(
                                        uri = it.key.removePrefix("local:"),
                                        title = it.title,
                                        artist = it.artist,
                                        album = it.album,
                                        durationMs = it.durationMs.toLong(),
                                        albumId = null,
                                    )
                                },
                            fromLibrary = true,
                        )
                    } else if (t.url != null) {
                        chosen = Chosen.ServerTrack(
                            uniffi.comrade_ui.StreamCandidateDto(
                                title = t.title,
                                artist = t.artist,
                                album = t.album,
                                durationMs = t.durationMs,
                                streamUrl = t.url!!,
                                artworkUrl = null,
                            ),
                        )
                    }
                },
            )
        }
    }

    // What happens next to whatever was just chosen. The three answers are
    // `TogetherDecisions.startStep`'s, so the sheet, the question and the
    // straight-to-playing case cannot disagree about which applies.
    //
    // **Two rules, and only the library's changed.** A tap in your own
    // collection plays — `startStepInLibrary`, and §18's argument for why. A
    // pasted link or a picked file is a gesture aimed at somebody, so those still
    // ask through `startStep` exactly as before.
    chosen?.let { what ->
        val sessionLive = live is TogetherManager.UiState.Live
        val weLead = (live as? TogetherManager.UiState.Live)?.weLead ?: false
        when (
            val next = if (what is Chosen.Track && what.fromLibrary) {
                TogetherDecisions.startStepInLibrary(
                    pairing = pairing,
                    choosingPerson = choosingPerson,
                    sessionLive = sessionLive,
                    weLead = weLead,
                )
            } else {
                TogetherDecisions.startStep(
                    pairing = pairing,
                    sessionLive = sessionLive,
                    weLead = weLead,
                )
            }
        ) {
            // The second step, over whichever of the three is showing. A sheet
            // rather than a screen because the thing they just chose is still
            // behind it, which is the difference between "and who with?" and
            // starting over.
            //
            // Dismissing it leaves the person button armed on purpose: closing
            // the sheet without picking anybody is "not them", not "never mind"
            // — the question has not been answered yet.
            is TogetherDecisions.StartStep.AskWho -> ListenWithSheet(
                onDismiss = { chosen = null },
                onChosen = { listener ->
                    chosen = null
                    choosingPerson = false
                    failed = !startWith(
                        context,
                        what,
                        TogetherDecisions.Pairing(listener.npub, listener.label),
                        onPickFileWith,
                    )
                    if (!failed) onStarted()
                },
                onAlone = {
                    chosen = null
                    choosingPerson = false
                    // The same call as any other, with the pairing that has
                    // nobody in it — which is what keeps listening alone from
                    // being a second implementation of the player.
                    failed = !startWith(context, what, TogetherDecisions.ALONE, onPickFileWith)
                    if (!failed) onStarted()
                },
            )

            is TogetherDecisions.StartStep.ConfirmTakeover -> {
                // Set from a side effect rather than drawn inline, so the
                // dialog below is the only thing that can clear `chosen` — two
                // places clearing it is how a tap ends up doing nothing.
                LaunchedEffect(what, next.pairing) {
                    confirming = what to next.pairing
                    chosen = null
                }
            }

            // Cleared last, deliberately: writing `chosen = null` first would
            // leave the rest of this block running in an effect whose composable
            // is on its way out of the composition.
            is TogetherDecisions.StartStep.PlayNow -> LaunchedEffect(what, next.pairing) {
                failed = !startWith(context, what, next.pairing, onPickFileWith)
                if (!failed) onStarted()
                chosen = null
            }
        }
    }

    // Taking the music off somebody else. Asked because it is their choice we
    // are ending, and answered on their own screen a moment later — the
    // follower may put something on as freely as the leader, which is what a
    // pair means, but not silently.
    confirming?.let { (what, with) ->
        val dialogShape = RoundedCornerShape(ComradeRadii.xl)
        AlertDialog(
            onDismissRequest = { confirming = null },
            modifier = Modifier.glassSurface(GlassElevation.Sheet, shape = dialogShape),
            shape = dialogShape,
            containerColor = Color.Transparent,
            title = { Text(stringResource(R.string.together_takeover_title)) },
            text = { Text(stringResource(R.string.together_takeover_body, with.label)) },
            confirmButton = {
                TextButton(onClick = {
                    confirming = null
                    failed = !startWith(context, what, with, onPickFileWith)
                    if (!failed) onStarted()
                }) {
                    Text(stringResource(R.string.together_takeover_yes))
                }
            },
            dismissButton = {
                TextButton(onClick = { confirming = null }) {
                    Text(stringResource(R.string.together_takeover_no))
                }
            },
        )
    }
}

/**
 * Start the session, or hand back to the picker when that is the next step.
 *
 * Every route out of the home screen funnels through here so there is one place
 * that knows how a choice becomes a session — and so a failure is reported once
 * rather than in three places that each forgot a different case.
 *
 * @return whether something actually happened. `false` is a refusal worth a
 *   sentence: core declining the content is the one outcome a tap can have that
 *   changes nothing on screen, and the caller says so rather than leaving a
 *   button that looks broken.
 */
private fun startWith(
    context: Context,
    what: Chosen,
    with: TogetherDecisions.Pairing,
    onPickFileWith: (peer: String, label: String) -> Unit,
): Boolean = when (what) {
    // The one route that carries a queue, because it is the only source that is
    // a list. Everything else here is one thing, and says so by passing none.
    is Chosen.Track -> TogetherManager.playTrack(context, with, what.track, what.queue)

    is Chosen.ServerTrack -> runCatching {
        // Built typed rather than through the paste parser — see
        // [Chosen.ServerTrack]. The recording and length come from the same
        // search that produced the URL, which is more than a pasted link ever
        // knows; the player still discovers the real length itself.
        val c = what.candidate
        val content = uniffi.comrade_core.TogetherContent.Stream(
            url = c.streamUrl,
            recording = uniffi.comrade_core.Recording(
                isrc = null,
                title = c.title,
                artist = c.artist,
                album = c.album,
            ),
            durationMs = c.durationMs.toULong().takeIf { it > 0uL },
        )
        TogetherManager.startStream(context, with.npub, with.label, content)
    }.onFailure { Log.w(TAG, "could not start on a server track", it) }.isSuccess

    is Chosen.Link -> when (val link = what.link) {
        is TogetherDecisions.Link.Video -> runCatching {
            TogetherManager.startEmbed(context, with.npub, with.label, link.videoId)
        }.onFailure { Log.w(TAG, "could not start on a video", it) }.isSuccess

        is TogetherDecisions.Link.Stream -> runCatching {
            // Rebuilt through core rather than carried as a typed value: the
            // content that goes on the wire has to be the one core validated,
            // and `Link.Stream` is the pure layer's answer with no Android and
            // no uniffi types in it. Core refuses again on the way out, so this
            // cannot smuggle a URL past `valid_stream_url` — and a refusal there
            // throws, which is what turns into the sentence above.
            val content = ComradeCore.togetherStreamContentTyped(link.url)
                as? uniffi.comrade_core.TogetherContent.Stream
                // The URL is deliberately not in the message: it goes to logcat,
                // it can be 2 kB, and it was core's own answer a moment ago —
                // there is nothing to learn from seeing it again.
                ?: error("core no longer accepts this stream URL")
            TogetherManager.startStream(context, with.npub, with.label, content)
        }.onFailure { Log.w(TAG, "could not start on a link", it) }.isSuccess

        // Unreachable: `LinkField` never offers an unplayable link. Answered
        // rather than ignored so a future caller cannot get a silent no.
        is TogetherDecisions.Link.NotPlayable -> false
    }

    // The picker is the next step, not the last one — so this succeeded at what
    // it was asked to do even though no session exists yet.
    is Chosen.AFile -> {
        onPickFileWith(with.npub, with.label)
        true
    }
}

private const val TAG = "TogetherScreen"

/** The ways in, plus the remembered lists one tap below the cards. */
@Composable
private fun ChooseASource(
    libraryGranted: Boolean,
    onBrowse: () -> Unit,
    onPickAFile: () -> Unit,
    onLink: () -> Unit,
    onSearch: () -> Unit,
    onServer: () -> Unit,
    onCollections: () -> Unit,
    onFavourites: () -> Unit,
    onRecent: () -> Unit,
    onPlaylists: () -> Unit,
    onClose: (() -> Unit)?,
) {
    Column(
        modifier = Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Spacer(Modifier.height(8.dp))
        // Only over a running session: on the idle tab there is nothing behind
        // this to go back to, and a back arrow that leads nowhere is worse than
        // none.
        onClose?.let { close ->
            Row(verticalAlignment = Alignment.CenterVertically) {
                IconButton(onClick = close) {
                    Icon(
                        Icons.AutoMirrored.Filled.ArrowBack,
                        contentDescription = stringResource(R.string.together_back_to_session),
                        tint = TogetherText,
                    )
                }
                Text(
                    stringResource(R.string.together_back_to_session),
                    style = MaterialTheme.typography.bodyMedium,
                    color = TogetherMuted,
                )
            }
        }
        Text(
            stringResource(R.string.together_home_title),
            style = MaterialTheme.typography.headlineSmall,
            fontWeight = FontWeight.SemiBold,
            color = TogetherText,
        )
        Text(
            stringResource(R.string.together_home_subtitle),
            style = MaterialTheme.typography.bodyMedium,
            color = TogetherMuted,
        )
        Spacer(Modifier.height(8.dp))
        // Order and content are `TogetherDecisions.sources`', so the list the
        // screen draws and the list the tests pin cannot come apart.
        TogetherDecisions.sources(libraryGranted).forEach { source ->
            when (source) {
                is TogetherDecisions.Source.OnThisPhone -> SourceCard(
                    icon = QueueMusicIcon,
                    title = stringResource(R.string.together_source_phone),
                    // Named while it is still true: after a grant the same card
                    // simply opens the list.
                    subtitle = stringResource(
                        if (source.needsPermission) {
                            R.string.together_source_phone_locked
                        } else {
                            R.string.together_source_phone_note
                        },
                    ),
                    onClick = onBrowse,
                )

                is TogetherDecisions.Source.PickAFile -> SourceCard(
                    icon = AttachFileIcon,
                    title = stringResource(R.string.together_source_file),
                    subtitle = stringResource(R.string.together_source_file_note),
                    onClick = onPickAFile,
                )

                is TogetherDecisions.Source.FromALink -> SourceCard(
                    icon = LinkIcon,
                    title = stringResource(R.string.together_source_link),
                    subtitle = stringResource(R.string.together_source_link_note),
                    onClick = onLink,
                )

                is TogetherDecisions.Source.SearchByName -> SourceCard(
                    icon = Icons.Filled.Search,
                    title = stringResource(R.string.together_source_search),
                    // Says out loud that this one leaves the phone. It is the
                    // only source that does, and a person choosing between four
                    // cards should not have to read the source to learn that.
                    subtitle = stringResource(R.string.together_source_search_note),
                    onClick = onSearch,
                )

                is TogetherDecisions.Source.YourServer -> SourceCard(
                    icon = ComputerIcon,
                    title = stringResource(R.string.together_source_server),
                    subtitle = stringResource(R.string.together_source_server_note),
                    onClick = onServer,
                )

                is TogetherDecisions.Source.PublicCollections -> SourceCard(
                    icon = LyricsIcon,
                    title = stringResource(R.string.together_source_collections),
                    subtitle = stringResource(R.string.together_source_collections_note),
                    onClick = onCollections,
                )
            }
        }

        // The player's own memory, one quiet row beneath the cards — lists of
        // what was loved and played, not sources. Text buttons rather than
        // cards so they read as places inside this tab, not more doors.
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            TextButton(onClick = onFavourites) {
                Text(stringResource(R.string.library_favourites), color = TogetherMuted)
            }
            TextButton(onClick = onRecent) {
                Text(stringResource(R.string.library_recent), color = TogetherMuted)
            }
            TextButton(onClick = onPlaylists) {
                Text(stringResource(R.string.library_playlists), color = TogetherMuted)
            }
        }

        ResumeCard()
        Spacer(Modifier.height(4.dp))
        Spacer(Modifier.height(4.dp))
        Text(
            stringResource(R.string.together_home_note),
            style = MaterialTheme.typography.bodySmall,
            color = TogetherMuted,
        )
        Spacer(Modifier.height(24.dp))
    }
}

@Composable
private fun SourceCard(
    icon: ImageVector,
    title: String,
    subtitle: String,
    onClick: () -> Unit,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(20.dp))
            .background(TogetherCard)
            .clickable(onClick = onClick)
            .padding(16.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Box(
            modifier = Modifier
                .size(44.dp)
                .clip(CircleShape)
                .background(TogetherSleeve),
            contentAlignment = Alignment.Center,
        ) {
            Icon(icon, contentDescription = null, tint = TogetherText, modifier = Modifier.size(22.dp))
        }
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.titleMedium, color = TogetherText)
            Text(subtitle, style = MaterialTheme.typography.bodySmall, color = TogetherMuted)
        }
    }
}

/**
 * The phone's own music — the collection as covers, and a query as a list.
 *
 * Read on a background thread and once per grant, not per recomposition: a
 * `MediaStore` query with two thousand rows in it is not something to run while
 * somebody types into the search field. Which of the two views is showing is
 * [TogetherDecisions.browse]'s answer and the grouping is
 * [TogetherDecisions.albumsOf]'s, both over the list already in memory — which
 * is why typing is instant and why the behaviour is checked before CI.
 *
 * @param choosingPerson the person button is armed, so the next pick asks who
 *   with rather than playing. The caller gates it on the same answer as
 *   [onChoosePerson], so it is never `true` while that is `null` — this file may
 *   draw the armed note without checking twice.
 * @param onChoosePerson arm or disarm that button, or `null` when there is
 *   nobody to offer — which is [TogetherDecisions.mayChoosePerson]'s call and
 *   not this file's.
 */
@Composable
private fun LibraryBrowser(
    libraryGranted: Boolean,
    onAsk: () -> Unit,
    onBack: () -> Unit,
    choosingPerson: Boolean,
    onChoosePerson: (() -> Unit)?,
    /**
     * The track, **and the list it came out of** — the album it was opened from,
     * or the library as the search field had narrowed it. That is what the
     * person was looking at when they chose, so it is what next should mean.
     */
    onPlay: (TogetherDecisions.Track, List<TogetherDecisions.Track>) -> Unit,
) {
    val context = LocalContext.current
    var page by remember { mutableStateOf<MusicLibrary.Page?>(null) }
    var query by remember { mutableStateOf("") }
    // Which record is open, held as its key rather than as the album itself: the
    // library is re-read when a permission is granted, and a held `Album` would
    // go on drawing the tracks of a list that no longer exists.
    var openAlbum by remember { mutableStateOf<String?>(null) }
    // Hoisted out of the grid, which is the whole point. Opening a record removes
    // the grid from the composition, so a state remembered *inside* it is
    // discarded — and coming back out of a record would land at the top of a
    // library the person had scrolled halfway down, which makes the drill-in
    // useless past the first screenful. Same value the repo already ships for
    // chat threads: come back where you left off.
    val gridState = rememberLazyGridState()

    LaunchedEffect(libraryGranted) {
        page = if (libraryGranted) {
            withContext(Dispatchers.IO) { MusicLibrary.page(context) }
        } else {
            null
        }
    }
    // Covers are a cache in `MusicLibrary`, not a per-row leak — but a cache
    // held after the browser is gone is memory nothing is looking at.
    DisposableEffect(Unit) { onDispose { MusicLibrary.forgetArtwork() } }

    val loaded = page
    val view = remember(loaded, query) {
        TogetherDecisions.browse(loaded?.tracks.orEmpty(), query)
    }
    // Resolved out of the view rather than remembered beside it, so a record
    // that stopped existing between two reads closes itself instead of showing
    // an empty screen with a back button.
    //
    // Deliberately *not* wrapped in `remember`, and the reason is that there is
    // nothing worth caching: the scan is one pass over a few hundred albums
    // comparing strings. (It is **not** that the keys would be expensive —
    // `remember(view, openAlbum)` would compare `view` by reference first, since
    // a data class's `equals` opens with `this === other` and `view` is the same
    // instance until `loaded` or `query` changes. The `remember` two lines up
    // relies on exactly that short-circuit over a `Page` holding 2,000 tracks.)
    val open = (view as? TogetherDecisions.Browse.Albums)
        ?.albums
        ?.firstOrNull { it.key == openAlbum }
    val noAlbum = stringResource(R.string.together_album_none)

    // Only for the level this file added. Enabled only while a record is open, so
    // back out of the browser itself keeps whatever behaviour the activity has —
    // this is not the place to start owning the tab's whole back stack.
    BackHandler(enabled = open != null) { openAlbum = null }

    Column(Modifier.fillMaxSize()) {
        BrowserHeader(
            title = open?.let { it.title ?: noAlbum }
                ?: stringResource(R.string.together_source_phone),
            // One step up, not out: inside a record, the arrow closes the record.
            // An arrow that left the whole browser from two levels down is the
            // navigation bug every drill-in gets wrong once.
            //
            // The system back button and the predictive-back gesture are handled
            // separately, below — `MainActivity`'s single `BackHandler` does not
            // cover the Together tab at all, so this level has to bring its own
            // or the gesture people actually reach for inside a drill-in would
            // leave the screen entirely.
            onBack = { if (open != null) openAlbum = null else onBack() },
            choosingPerson = choosingPerson,
            onChoosePerson = onChoosePerson,
        )
        if (choosingPerson) {
            // The tint on the button says *armed*; this says what armed means.
            // A toggle whose only feedback is a colour is a toggle nobody is
            // sure they pressed.
            Text(
                stringResource(R.string.together_choosing_person_note),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.primary,
                modifier = Modifier.padding(horizontal = 4.dp),
            )
        }
        if (!libraryGranted) {
            // `if/else`, never an early `return@Column`: `libraryGranted` flips
            // the moment the permission is granted, and an early return makes
            // this Column emit a different number of composable groups either
            // side of that flip — which throws on the recomposition rather than
            // the first frame. `TaskListScreen` (2026-08-04) and `RideScreen`
            // (2026-08-15) both killed the process that way; see
            // `.claude/rules/android.md`.
            //
            // Asking, not an empty list: "no music here" and "not allowed to
            // look" are different sentences with different next steps, the same
            // distinction `MediaLibraryAccess` draws for an invitation.
            //
            // And `Step.Picker` is the third case, which is why the button is
            // conditional: after a refusal Android shows no dialog at all, so an
            // "Allow" here would be a button that does nothing — the one outcome
            // guaranteed to teach someone the feature is broken. That case gets
            // the sentence that names the only route left, which is Settings,
            // and the other two sources are one tap back.
            val step = MediaLibraryAccess.next(
                granted = false,
                askedBefore = runCatching { MediaLibraryAccess.asked(context) }
                    .getOrDefault(true),
            )
            Text(
                stringResource(
                    if (step == MediaLibraryAccess.Step.Ask) {
                        R.string.together_library_why
                    } else {
                        R.string.together_library_refused
                    },
                ),
                style = MaterialTheme.typography.bodyMedium,
                color = TogetherMuted,
                modifier = Modifier.padding(vertical = 12.dp),
            )
            if (step == MediaLibraryAccess.Step.Ask) {
                Button(onClick = onAsk) { Text(stringResource(R.string.together_library_allow)) }
            }
        } else {
            // Not while a record is open: everything on that screen is one album's
            // worth, so there is nothing there to narrow, and a field that switched
            // the view out from under the record would make back mean two things.
            if (open == null) {
                OutlinedTextField(
                    value = query,
                    onValueChange = { query = it },
                    singleLine = true,
                    leadingIcon = { Icon(Icons.Filled.Search, contentDescription = null) },
                    placeholder = { Text(stringResource(R.string.together_library_search)) },
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(vertical = 8.dp),
                )
            }
            when {
                loaded == null -> Text(
                    stringResource(R.string.together_library_loading),
                    style = MaterialTheme.typography.bodyMedium,
                    color = TogetherMuted,
                )

                // One record, opened from the grid. Its own tracks are the queue,
                // which is what the person was looking at when they chose.
                open != null -> LazyColumn(Modifier.fillMaxSize()) {
                    items(open.tracks, key = { it.uri }) { track ->
                        TrackRow(track, onClick = { onPlay(track, open.tracks) })
                    }
                    // A different sentence from the other two views, because "search
                    // to find the rest" names an action this screen does not have —
                    // the field is deliberately not drawn inside a record. What is
                    // true here is that this record may be missing rows, since the
                    // 2,000-row cut falls in *track title* order and lands inside
                    // whichever albums sort late.
                    if (loaded.truncated) {
                        item(key = "truncated") {
                            Text(
                                stringResource(R.string.together_album_partial),
                                style = MaterialTheme.typography.bodySmall,
                                color = TogetherMuted,
                                modifier = Modifier.padding(16.dp),
                            )
                        }
                    }
                }

                // The two "nothing" sentences, which are different sentences: with
                // nothing typed an empty answer means the phone has no music, and
                // with something typed it means that query found none. That used to
                // be two conditions read off one list; now it falls out of which
                // view [TogetherDecisions.browse] returned.
                view is TogetherDecisions.Browse.Albums && view.albums.isEmpty() -> Text(
                    stringResource(R.string.together_library_empty),
                    style = MaterialTheme.typography.bodyMedium,
                    color = TogetherMuted,
                )

                view is TogetherDecisions.Browse.Tracks && view.tracks.isEmpty() -> Text(
                    stringResource(R.string.together_library_no_match),
                    style = MaterialTheme.typography.bodyMedium,
                    color = TogetherMuted,
                )

                view is TogetherDecisions.Browse.Albums -> LazyVerticalGrid(
                    // Adaptive rather than a fixed column count, so the same code
                    // draws two columns on a phone and five on a tablet or in
                    // landscape — the alternative is a screen-width breakpoint this
                    // file would have to guess.
                    columns = GridCells.Adaptive(minSize = GRID_TILE_DP.dp),
                    state = gridState,
                    modifier = Modifier.fillMaxSize(),
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                    verticalArrangement = Arrangement.spacedBy(16.dp),
                    contentPadding = PaddingValues(vertical = 8.dp),
                ) {
                    // A stable key, like every data-driven list in this app: without
                    // one, tile state reattaches to the wrong item as the library is
                    // re-read.
                    items(view.albums, key = { it.key }) { album ->
                        AlbumTile(
                            album,
                            noAlbum = noAlbum,
                            // Not a fact about the record while the page was cut —
                            // see `albumSubtitle`.
                            partial = loaded.truncated,
                            onClick = { openAlbum = album.key },
                        )
                    }
                    if (loaded.truncated) {
                        item(key = "truncated", span = { GridItemSpan(maxLineSpan) }) { TruncatedNote() }
                    }
                }

                view is TogetherDecisions.Browse.Tracks -> LazyColumn(Modifier.fillMaxSize()) {
                    items(view.tracks, key = { it.uri }) { track ->
                        TrackRow(track, onClick = { onPlay(track, view.tracks) })
                    }
                    if (loaded.truncated) item(key = "truncated") { TruncatedNote() }
                }
            }
        }
    }
}

/**
 * Said out loud, at the end of whichever view is showing.
 *
 * A list silently cut off reads as "that is all your music", and the person
 * whose album is past the cut concludes the feature cannot see it.
 */
@Composable
private fun TruncatedNote() {
    Text(
        stringResource(R.string.together_library_truncated),
        style = MaterialTheme.typography.bodySmall,
        color = TogetherMuted,
        modifier = Modifier.padding(16.dp),
    )
}

/**
 * @param onChoosePerson the person button, or `null` when there is nobody left
 *   to offer — [TogetherDecisions.mayChoosePerson] decides, and the routing asks
 *   it again, so the button and the action cannot disagree about when it applies.
 */
@Composable
private fun BrowserHeader(
    title: String,
    onBack: () -> Unit,
    choosingPerson: Boolean = false,
    onChoosePerson: (() -> Unit)? = null,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(top = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconButton(onClick = onBack) {
            Icon(
                Icons.AutoMirrored.Filled.ArrowBack,
                contentDescription = stringResource(R.string.together_back),
                tint = TogetherText,
            )
        }
        Text(
            title,
            style = MaterialTheme.typography.titleLarge,
            color = TogetherText,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
        onChoosePerson?.let { choose ->
            IconButton(onClick = choose) {
                Icon(
                    Icons.Filled.Person,
                    contentDescription = stringResource(R.string.together_choose_person),
                    tint = if (choosingPerson) {
                        MaterialTheme.colorScheme.primary
                    } else {
                        TogetherMuted
                    },
                )
            }
        }
    }
}

/**
 * One record in the grid: its cover, its name, and who it is by.
 *
 * @param noAlbum what to call the group whose files name no album. Passed in
 *   rather than read here so the header and the tile cannot call it two
 *   different things.
 * @param partial the library read was cut at its row cap, so no album's track
 *   count is a fact about the record — see [albumSubtitle].
 */
@Composable
private fun AlbumTile(
    album: TogetherDecisions.Album,
    noAlbum: String,
    partial: Boolean,
    onClick: () -> Unit,
) {
    Column(
        modifier = Modifier
            .clip(RoundedCornerShape(12.dp))
            .clickable(onClick = onClick),
        verticalArrangement = Arrangement.spacedBy(Spacing.space2),
    ) {
        Cover(
            uri = album.cover.uri,
            albumId = album.cover.albumId,
            requestDp = GRID_COVER_DP,
            corner = 12.dp,
            glyphDp = 34,
            // Square, and sized by the column rather than by a fixed number:
            // `GridCells.Adaptive` decides the width, and a tile that set its
            // own would stop being square on every screen but one.
            modifier = Modifier
                .fillMaxWidth()
                .aspectRatio(1f),
        )
        Text(
            album.title ?: noAlbum,
            style = MaterialTheme.typography.bodyMedium,
            color = TogetherText,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Text(
            albumSubtitle(album, partial),
            style = MaterialTheme.typography.bodySmall,
            color = TogetherMuted,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/**
 * The line under an album's name.
 *
 * The three artist answers are [TogetherDecisions.AlbumArtist]'s, and each gets
 * its own sentence: naming one person, saying they differ, or saying nothing at
 * all. "Various artists" over four untagged rips would be inventing the one
 * fact the files withheld, which is why `Unknown` is not folded into `Various`.
 */
@Composable
private fun albumSubtitle(album: TogetherDecisions.Album, partial: Boolean): String {
    val count = pluralStringResource(
        // "at least" while the page was cut, because the cut falls in *track
        // title* order across the whole library rather than at an album
        // boundary: a twelve-track record whose last five titles sort past row
        // 2,000 is here with seven of them, and "7 tracks" would be a claim
        // about the record made out of a fact about the page.
        if (partial) R.plurals.together_album_tracks_partial else R.plurals.together_album_tracks,
        album.tracks.size,
        album.tracks.size,
    )
    val artist = when (val by = album.artist) {
        is TogetherDecisions.AlbumArtist.One -> by.name
        TogetherDecisions.AlbumArtist.Various -> stringResource(R.string.together_album_various)
        TogetherDecisions.AlbumArtist.Unknown -> null
    }
    return listOfNotNull(artist, count).joinToString(TogetherDecisions.SUBTITLE_SEPARATOR)
}

@Composable
private fun TrackRow(track: TogetherDecisions.Track, onClick: () -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Spacing.space4),
    ) {
        Cover(
            uri = track.uri,
            albumId = track.albumId,
            requestDp = ROW_COVER_DP,
            corner = 8.dp,
            glyphDp = 22,
            modifier = Modifier.size(ROW_COVER_DP.dp),
        )
        Column(Modifier.weight(1f)) {
            Text(
                track.title,
                style = MaterialTheme.typography.bodyLarge,
                color = TogetherText,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                TogetherDecisions.trackSubtitle(track),
                style = MaterialTheme.typography.bodySmall,
                color = TogetherMuted,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Icon(Icons.Filled.PlayArrow, contentDescription = null, tint = TogetherMuted)
    }
}

/**
 * An album cover, or the note glyph when the file carries none.
 *
 * Decoded on the IO dispatcher and keyed on what it is for, so scrolling does
 * not decode on the frame thread and a recomposition does not re-decode. The
 * `null` result is cached only by `MusicLibrary`'s own cache of *hits* — a miss
 * is re-attempted, which costs one failed provider call on a file with no art
 * and keeps this from having to hold a second negative cache.
 *
 * [requestDp] is what the provider is asked for and [modifier] is what the box
 * actually measures, which are two different numbers on purpose: the session's
 * sleeve fills its parent and its width is not known until layout, so asking for
 * a fixed reasonable square is what stops a rotation re-decoding the cover at a
 * new size.
 */
@Composable
private fun Cover(
    uri: String?,
    albumId: Long?,
    requestDp: Int,
    corner: Dp,
    glyphDp: Int,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    val requestPx = with(LocalDensity.current) { requestDp.dp.roundToPx() }
    var art by remember(uri, albumId) { mutableStateOf<ImageBitmap?>(null) }
    LaunchedEffect(uri, albumId, requestPx) {
        art = uri?.let { at ->
            withContext(Dispatchers.IO) {
                runCatching { MusicLibrary.artwork(context, at, albumId, requestPx) }
                    .getOrNull()
                    ?.let(Bitmap::asImageBitmap)
            }
        }
    }
    Box(
        modifier = modifier
            .clip(RoundedCornerShape(corner))
            .background(TogetherSleeve),
        contentAlignment = Alignment.Center,
    ) {
        val bitmap = art
        if (bitmap != null) {
            Image(
                bitmap = bitmap,
                contentDescription = null,
                contentScale = ContentScale.Crop,
                modifier = Modifier.fillMaxSize(),
            )
        } else {
            Icon(
                QueueMusicIcon,
                contentDescription = null,
                tint = TogetherMuted,
                modifier = Modifier.size(glyphDp.dp),
            )
        }
    }
}

/**
 * The pasted-link field.
 *
 * **Core classifies, this only asks.** `play_query` knows the service hosts and
 * `TogetherContent::stream` knows what a media URL is; the ordering between
 * their two answers is [TogetherDecisions.classifyLink], which the JVM lane
 * pins. Nothing in here looks at the text itself, which is the whole point — a
 * third opinion about what a link is would be the drift `docs/CHAT_ACTIONS.md`
 * §7 records for `/pay`.
 */
@Composable
private fun LinkField(onBack: () -> Unit, onPlay: (TogetherDecisions.Link) -> Unit) {
    var text by remember { mutableStateOf("") }
    var refused by remember { mutableStateOf(false) }
    var asking by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        BrowserHeader(stringResource(R.string.together_source_link), onBack)
        Text(
            stringResource(R.string.together_link_explainer),
            style = MaterialTheme.typography.bodyMedium,
            color = TogetherMuted,
        )
        OutlinedTextField(
            value = text,
            onValueChange = {
                text = it
                refused = false
            },
            singleLine = true,
            placeholder = { Text(stringResource(R.string.together_link_placeholder)) },
            modifier = Modifier.fillMaxWidth(),
        )
        Button(
            enabled = text.isNotBlank() && !asking,
            onClick = {
                asking = true
                scope.launch {
                    // Off the main thread: both of these cross the FFI and
                    // `play_query` takes the runtime's read lock, which is the
                    // same reason `ChatsScreen` runs the identical pair on
                    // `Dispatchers.IO`.
                    val link = withContext(Dispatchers.IO) {
                        TogetherDecisions.classifyLink(
                            videoId = (
                                runCatching { ComradeCore.playQuery(text, null).content }
                                    .getOrNull() as? uniffi.comrade_core.TogetherContent.Youtube
                                )?.videoId,
                            streamUrl = (
                                ComradeCore.togetherStreamContentTyped(text)
                                    as? uniffi.comrade_core.TogetherContent.Stream
                                )?.url,
                        )
                    }
                    asking = false
                    if (link is TogetherDecisions.Link.NotPlayable) refused = true else onPlay(link)
                }
            },
        ) {
            Text(stringResource(R.string.together_link_go))
        }
        if (refused) {
            // Names what would work rather than what was wrong: someone who
            // pasted a page link wants to know a direct file link is the thing
            // to look for.
            Text(
                stringResource(R.string.together_link_refused),
                style = MaterialTheme.typography.bodySmall,
                color = TogetherMuted,
            )
        }
    }
}

/**
 * Search a public catalogue by name, and play whichever local copy it turns out
 * to mean.
 *
 * **This screen finds a *recording*, not audio**, and the distinction is the
 * whole design. `catalogue_lookup` answers "what is that song" — title, artist,
 * album, ISRC when known — and `audio_plan` then decides where the bytes come
 * from. What this build can act on today is the `library` tier: the catalogue
 * tells us the proper name, `MediaStore` is searched for it, and a session opens
 * on the copy already on the phone. The other three tiers are named honestly and
 * do not pretend to be buttons that work — see [TogetherDecisions.CandidateAction]
 * and `catalogue.rs`'s header for why `EmbedOrNameIt` is a floor rather than a
 * gap.
 *
 * Every decision here is [TogetherDecisions]', which the JVM lane pins: the five
 * outcomes, the tier naming, and — the one that matters most — that "this build
 * cannot search" never renders as "no such song".
 */
@Composable
private fun SearchByName(
    onBack: () -> Unit,
    onPlay: (TogetherDecisions.Track, List<TogetherDecisions.Track>) -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var query by remember { mutableStateOf("") }
    var outcome by remember {
        mutableStateOf<TogetherDecisions.SearchOutcome>(TogetherDecisions.SearchOutcome.Idle)
    }
    // Kept alongside the drawn candidates so a tap can ask `audio_plan` about the
    // same match the catalogue returned, rather than re-deriving one from the row.
    var matches by remember {
        mutableStateOf<List<uniffi.comrade_core.CatalogueMatch>>(emptyList())
    }
    // Which row is fetching, by index. One at a time: a phone downloading four
    // tracks at once on a shared connection finishes none of them sooner, and the
    // per-row spinner has somewhere to be.
    var downloadingIndex by remember { mutableStateOf<Int?>(null) }
    // What happened to the last download. Shown above the list rather than in a
    // snackbar so it survives the row it belonged to scrolling away.
    var downloadNote by remember { mutableStateOf<String?>(null) }

    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        BrowserHeader(stringResource(R.string.together_source_search), onBack)
        Text(
            stringResource(R.string.together_search_explainer),
            style = MaterialTheme.typography.bodyMedium,
            color = TogetherMuted,
        )
        OutlinedTextField(
            value = query,
            onValueChange = {
                query = it
                // Clearing the field clears the answer. Leaving a previous
                // result under an empty query invites playing the wrong song.
                if (it.isBlank()) {
                    outcome = TogetherDecisions.SearchOutcome.Idle
                    matches = emptyList()
                }
            },
            singleLine = true,
            leadingIcon = { Icon(Icons.Filled.Search, contentDescription = null) },
            placeholder = { Text(stringResource(R.string.together_search_placeholder)) },
            modifier = Modifier.fillMaxWidth(),
        )
        Button(
            enabled = query.isNotBlank() &&
                outcome !is TogetherDecisions.SearchOutcome.Searching,
            onClick = {
                outcome = TogetherDecisions.SearchOutcome.Searching
                scope.launch {
                    // `Dispatchers.IO`, not the main thread: this is a relay-free
                    // but genuinely networked call, and `runBlocking` on it from
                    // a click handler is the ANR of docs/TOGETHER.md §17.
                    //
                    // The Jamendo key rides along when one is saved; without
                    // it the catalogue search simply answers from MusicBrainz
                    // alone, which is an ordinary answer and not an error. Read
                    // here rather than remembered, so a key added in Settings
                    // counts from the next search without reopening this screen.
                    val result = withContext(Dispatchers.IO) {
                        val jamendo = StreamingSourcesStore.load(context).jamendoClientId
                        ComradeCore.catalogueLookup(
                            query,
                            jamendo.takeIf { it.isNotBlank() },
                        )
                    }
                    matches = (result as? ComradeCore.CatalogueResult.Found)?.matches.orEmpty()
                    outcome = TogetherDecisions.searchOutcome(
                        unavailable = result is ComradeCore.CatalogueResult.Unavailable,
                        error = (result as? ComradeCore.CatalogueResult.Failed)?.reason,
                        candidates = matches.map { it.asCandidate() },
                    )
                }
            },
        ) {
            Text(stringResource(R.string.together_search_go))
        }

        when (val shown = outcome) {
            is TogetherDecisions.SearchOutcome.Idle -> Unit

            is TogetherDecisions.SearchOutcome.Searching -> Text(
                stringResource(R.string.together_search_running),
                style = MaterialTheme.typography.bodyMedium,
                color = TogetherMuted,
            )

            // The two that must not read alike. This one is about the build.
            is TogetherDecisions.SearchOutcome.NoCatalogue -> Text(
                stringResource(R.string.together_search_no_catalogue),
                style = MaterialTheme.typography.bodyMedium,
                color = TogetherMuted,
            )

            // …and this one is about the song.
            is TogetherDecisions.SearchOutcome.NothingFound -> Text(
                stringResource(R.string.together_search_nothing_found),
                style = MaterialTheme.typography.bodyMedium,
                color = TogetherMuted,
            )

            is TogetherDecisions.SearchOutcome.Failed -> Text(
                stringResource(R.string.together_search_failed, shown.reason),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.error,
            )

            // Both list-bearing outcomes draw the same rows, and that is the fix:
            // tapping a row that has no local copy must not take the other rows
            // away. `NotOnThisPhone` adds a line about the row that was tapped and
            // leaves the rest to try.
            is TogetherDecisions.SearchOutcome.Found,
            is TogetherDecisions.SearchOutcome.NotOnThisPhone,
            -> Column {
                val rows = when (shown) {
                    is TogetherDecisions.SearchOutcome.Found -> shown.candidates
                    is TogetherDecisions.SearchOutcome.NotOnThisPhone -> shown.candidates
                    // Unreachable: this arm covers exactly the two above.
                    else -> emptyList()
                }
                (shown as? TogetherDecisions.SearchOutcome.NotOnThisPhone)?.let { miss ->
                    Text(
                        // Names the song rather than the search, because the
                        // search worked. No "Couldn't search" prefix.
                        stringResource(R.string.together_search_not_on_phone, miss.wanted.title),
                        style = MaterialTheme.typography.bodyMedium,
                        color = TogetherMuted,
                        modifier = Modifier.padding(bottom = 4.dp),
                    )
                }
                downloadNote?.let {
                    Text(
                        it,
                        style = MaterialTheme.typography.bodyMedium,
                        color = TogetherMuted,
                        modifier = Modifier.padding(bottom = 4.dp),
                    )
                }
                rows.forEachIndexed { i, candidate ->
                    val match = matches.getOrNull(i)
                    val verdict = match?.let { ComradeCore.downloadVerdict(it) }
                    CandidateRow(
                        candidate = candidate,
                        downloading = downloadingIndex == i,
                        // A button only where the licence permits one. Everything
                        // else gets a sentence or nothing — see CandidateRow.
                        onDownload = if (
                            match != null &&
                            verdict is uniffi.comrade_ui.DownloadVerdictDto.Permitted &&
                            downloadingIndex == null
                        ) {
                            {
                                downloadingIndex = i
                                downloadNote = null
                                scope.launch {
                                    val note = withContext(Dispatchers.IO) {
                                        downloadInto(context, match)
                                    }
                                    downloadingIndex = null
                                    downloadNote = note
                                }
                            }
                        } else {
                            null
                        },
                        downloadNote = when (verdict) {
                            // Named, because "no download button" with no reason
                            // reads as a missing feature. `NoAudio` is the normal
                            // case for a metadata catalogue and says so.
                            is uniffi.comrade_ui.DownloadVerdictDto.NoAudio ->
                                stringResource(R.string.together_download_no_audio)
                            is uniffi.comrade_ui.DownloadVerdictDto.NotOpenlyLicensed ->
                                stringResource(R.string.together_download_not_licensed)
                            // An insecure URL is a catalogue bug, not something to
                            // explain to a listener. Silent.
                            else -> null
                        },
                        onClick = {
                            // The outer `match` is this row's; no second lookup.
                            val want = match ?: return@CandidateRow
                            scope.launch {
                                val opened = withContext(Dispatchers.IO) {
                                    openLocalCopyOf(context, want)
                                }
                                if (opened == null) {
                                    // No local copy above the confidence bar. Say
                                    // so rather than opening the nearest thing —
                                    // MATCH_CONFIDENT exists precisely so this
                                    // asks instead of guessing.
                                    //
                                    // **Not `Failed`.** That is what shipped, and
                                    // it rendered a search that worked as
                                    // "Couldn't search: …" while wiping the list,
                                    // so there was no other candidate left to try.
                                    // The search succeeded; only this row has no
                                    // copy here.
                                    outcome = TogetherDecisions.notOnThisPhone(
                                        candidates = rows,
                                        wanted = candidate,
                                    )
                                } else {
                                    onPlay(opened.first, opened.second)
                                }
                            }
                        },
                    )
                }
            }
        }
    }
}

/**
 * Search your own streaming server — the fifth source card's screen.
 *
 * The shape mirrors [SearchByName] deliberately (field, one button, outcomes
 * below), because both are "type words, get rows" and the two must not feel
 * like different apps. Where they differ is the point of each sentence:
 *
 * - **No server configured** is a setup step, shown as its own card-sized
 *   sentence rather than an empty result — "you have no server saved" and
 *   "nothing matched" are different answers, and collapsing them is exactly
 *   the wrong-answer shape §20 exists to prevent.
 * - **A failed search names what failed**, verbatim from core: the server's
 *   own refusal ("Wrong username or password") or the network's.
 * - **Rows play without leaving this screen**: tapping asks who with through
 *   [startWith]'s ordinary path, because a server track is a gesture aimed at
 *   somebody the same way a catalogue row is — not a library tap.
 *
 * Every candidate URL arrived already guarded in core, so a tap can start a
 * session without a second validation pass — but `together_start` still runs
 * its own check on the way out, which is where a mid-air config change would
 * surface as a thrown refusal rather than a silent send.
 */
@Composable
private fun ServerSearch(
    onBack: () -> Unit,
    onPlay: (uniffi.comrade_ui.StreamCandidateDto) -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    // Read once per entry into the screen; a change made in Settings lands the
    // next time this screen opens, not under the current results.
    var sources by remember {
        mutableStateOf(StreamingSourcesStore.load(context))
    }
    var query by remember { mutableStateOf("") }
    var outcome by remember { mutableStateOf<ComradeCore.StreamResult?>(null) }
    var searching by remember { mutableStateOf(false) }

    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        BrowserHeader(stringResource(R.string.together_source_server), onBack)
        Text(
            stringResource(R.string.together_server_explainer),
            style = MaterialTheme.typography.bodyMedium,
            color = TogetherMuted,
        )

        if (!sources.subsonicConfigured) {
            // Setup gate, drawn as a card so it reads as a place rather than a
            // dead end. Nothing here links into Settings directly — the tab has
            // no navigation to it — so the sentence says where to go instead.
            //
            // An if/else, not an early return: this Column recomposes when the
            // search state below changes, and a branch that emits a different
            // number of groups either side of a state flip is the defect that
            // killed TaskListScreen (AUDIT 2026-08-04) and RideScreen
            // (2026-08-15) on their *second* frame. The search UI lives in the
            // else arm for the same reason.
            ElevatedCard(Modifier.fillMaxWidth()) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(
                        stringResource(R.string.together_server_none_title),
                        style = MaterialTheme.typography.titleSmall,
                        color = TogetherText,
                    )
                    Text(
                        stringResource(R.string.together_server_none_body),
                        style = MaterialTheme.typography.bodyMedium,
                        color = TogetherMuted,
                    )
                }
            }
        } else {
        OutlinedTextField(
            value = query,
            onValueChange = {
                query = it
                if (it.isBlank()) outcome = null
            },
            singleLine = true,
            leadingIcon = { Icon(Icons.Filled.Search, contentDescription = null) },
            placeholder = { Text(stringResource(R.string.together_server_search_hint)) },
            modifier = Modifier.fillMaxWidth(),
        )
        Button(
            enabled = query.isNotBlank() && !searching,
            onClick = {
                searching = true
                scope.launch {
                    val cfg = uniffi.comrade_core.SubsonicConfig(
                        server = sources.server.trim(),
                        username = sources.username.trim(),
                        password = sources.password,
                    )
                    val result = withContext(Dispatchers.IO) {
                        ComradeCore.subsonicSearch(cfg, query)
                    }
                    outcome = result
                    searching = false
                }
            },
        ) {
            Text(stringResource(R.string.together_search_go))
        }

        when (val shown = outcome) {
            null -> Unit

            // The two non-answers that must not read alike: no setup versus a
            // build that cannot stream at all. Both are about this device;
            // neither is about the songs.
            is ComradeCore.StreamResult.NotConfigured -> Text(
                stringResource(R.string.together_server_none_body),
                style = MaterialTheme.typography.bodyMedium,
                color = TogetherMuted,
            )

            is ComradeCore.StreamResult.CannotStream -> Text(
                stringResource(R.string.together_server_cannot_stream),
                style = MaterialTheme.typography.bodyMedium,
                color = TogetherMuted,
            )

            is ComradeCore.StreamResult.Failed -> Text(
                stringResource(R.string.together_server_failed, shown.reason),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.error,
            )

            // An empty list here really does mean "the server has no such
            // song" — the only arm allowed to say that, because it is the only
            // one that asked.
            is ComradeCore.StreamResult.Found ->
                if (shown.candidates.isEmpty()) {
                    Text(
                        stringResource(R.string.together_search_nothing_found),
                        style = MaterialTheme.typography.bodyMedium,
                        color = TogetherMuted,
                    )
                } else {
                    Column {
                        shown.candidates.forEach { candidate ->
                            ServerRow(candidate = candidate, onClick = { onPlay(candidate) })
                        }
                    }
                }
        }
        }
    }
}

/** One streaming-server answer: title over artist, ready to listen with. */
@Composable
private fun ServerRow(
    candidate: uniffi.comrade_ui.StreamCandidateDto,
    onClick: () -> Unit,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(vertical = Spacing.space3),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Spacing.space4),
    ) {
        Icon(QueueMusicIcon, contentDescription = null, tint = TogetherMuted)
        Column(Modifier.weight(1f)) {
            Text(
                candidate.title,
                style = MaterialTheme.typography.bodyLarge,
                color = TogetherText,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                candidate.artist.ifBlank {
                    stringResource(R.string.together_search_unknown_artist)
                },
                style = MaterialTheme.typography.bodySmall,
                color = TogetherMuted,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Icon(Icons.Filled.PlayArrow, contentDescription = null, tint = TogetherMuted)
    }
}

/**
 * One catalogue answer, with the catalogue that gave it named on the row and a
 * download button only where the licence actually permits one.
 *
 * [onDownload] is `null` for every row core refused, and the row then says
 * nothing about downloading at all — a greyed-out button would invite a tap and
 * then explain a licence, which is worse than not offering it. Where there *is* a
 * reason worth naming ([downloadNote]) it is a line of text, not a disabled
 * control.
 */@Composable
private fun CandidateRow(
    candidate: TogetherDecisions.Candidate,
    onClick: () -> Unit,
    onDownload: (() -> Unit)? = null,
    downloadNote: String? = null,
    downloading: Boolean = false,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(vertical = Spacing.space3),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Spacing.space4),
    ) {
        Icon(QueueMusicIcon, contentDescription = null, tint = TogetherMuted)
        Column(Modifier.weight(1f)) {
            Text(
                candidate.title,
                style = MaterialTheme.typography.bodyLarge,
                color = TogetherText,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                // The catalogue is named on every row, not once at the top of the
                // list: `CatalogueResolver::name`'s contract is that a guess
                // presented without its source is worse than no guess, and a
                // header scrolls away while the row does not.
                stringResource(
                    R.string.together_search_result_subtitle,
                    candidate.artist.ifBlank { stringResource(R.string.together_search_unknown_artist) },
                    candidate.catalogue,
                ),
                style = MaterialTheme.typography.bodySmall,
                color = TogetherMuted,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            downloadNote?.let {
                Text(
                    it,
                    style = MaterialTheme.typography.bodySmall,
                    color = TogetherMuted,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        if (downloading) {
            Text(
                stringResource(R.string.together_downloading),
                style = MaterialTheme.typography.bodySmall,
                color = TogetherMuted,
            )
        } else {
            onDownload?.let { download ->
                TextButton(onClick = download) {
                    Text(stringResource(R.string.together_download), color = TogetherText)
                }
            }
        }
    }
}

/** [uniffi.comrade_core.CatalogueMatch] in the shape the pure decisions take. */
private fun uniffi.comrade_core.CatalogueMatch.asCandidate() = TogetherDecisions.Candidate(
    title = recording.title,
    artist = recording.artist,
    album = recording.album,
    durationMs = durationMs?.toLong() ?: 0L,
    // Named on the row, not once at the top: with more than one catalogue
    // wired (`docs/TOGETHER.md` §23's Jamendo alongside MusicBrainz) the name
    // has to travel on the match itself — the exact move §20 predicted when it
    // called the old constant a lie waiting to happen.
    catalogue = source,
    durationKnown = durationMs != null,
)

/**
 * Fetch an openly-licensed track and put it in the phone's music library.
 *
 * Returns the sentence to show. **Blocking** — `Dispatchers.IO` only; it is a
 * network transfer followed by a `ContentResolver` write.
 *
 * The three outcomes stay three sentences. "You already have this" is not a
 * failure and must not read as one, which is why [MusicDownloads.Outcome] is a
 * sealed interface rather than a `Result` — the same argument as
 * `ComradeCore.CatalogueResult`.
 */
private fun downloadInto(
    context: Context,
    match: uniffi.comrade_core.CatalogueMatch,
): String {
    val fetched = kotlinx.coroutines.runBlocking { ComradeCore.downloadTrack(match) }
    val track = fetched.getOrElse { e ->
        return context.getString(
            R.string.together_download_failed,
            e.message ?: e::class.java.simpleName,
        )
    }
    return when (val saved = MusicDownloads.save(context, track)) {
        is MusicDownloads.Outcome.Saved ->
            context.getString(R.string.together_download_saved, saved.displayName)
        is MusicDownloads.Outcome.AlreadyThere ->
            context.getString(R.string.together_download_already, saved.displayName)
        is MusicDownloads.Outcome.Failed ->
            context.getString(R.string.together_download_failed, saved.reason)
    }
}

/**
 * Find this phone's own copy of `match`, or `null`.
 *
 * The `library` rung of `audio_plan`, carried out — and deliberately delegated
 * to [LibraryResolver] rather than reimplemented. That object already supplies
 * `MediaStore` candidates and applies `comrade_core::together::match_score`, so
 * the "is this the same recording" judgement stays core's one answer instead of
 * becoming a second opinion in this file. An earlier draft of this screen scored
 * titles itself, which is exactly the drift `TogetherDecisions`' header exists to
 * prevent.
 *
 * `null` covers both "no copy" and "nothing confident enough", deliberately:
 * `MATCH_CONFIDENT` exists so that opening the wrong track on somebody's behalf
 * is not a thing this does.
 */
private fun openLocalCopyOf(
    context: Context,
    match: uniffi.comrade_core.CatalogueMatch,
): Pair<TogetherDecisions.Track, List<TogetherDecisions.Track>>? {
    val wantMs = (match.durationMs ?: 0UL).toLong()
    // Core decides which rung applies; this screen can only act on `library`.
    // The other three are real answers with no button behind them yet, and the
    // caller says so rather than silently doing nothing.
    val resolved = LibraryResolver.resolve(context, match.recording, wantMs) ?: return null
    val page = runCatching { MusicLibrary.page(context) }.getOrNull() ?: return null
    // The queue is the library the person is choosing from, so prev/next mean
    // that list — the same contract `LibraryBrowser` passes to `onPlay`.
    val track = page.tracks.firstOrNull { it.uri == resolved.uri.toString() } ?: return null
    return track to page.tracks
}

/**
 * "And who with?" — the second half of starting a session.
 *
 * Comrades first and online first, which is [TogetherDecisions.listenersFor]'s
 * rule and not this composable's. Contacts who are not comrades are still
 * offered: an invitation is a DM like any other, and presence is a thing you opt
 * into mutually rather than a precondition for asking.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ListenWithSheet(
    onDismiss: () -> Unit,
    onChosen: (TogetherDecisions.Listener) -> Unit,
    onAlone: () -> Unit,
) {
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    var all by remember { mutableStateOf<List<TogetherDecisions.Listener>>(emptyList()) }
    var query by remember { mutableStateOf("") }
    var loaded by remember { mutableStateOf(false) }

    LaunchedEffect(Unit) {
        all = withContext(Dispatchers.IO) { listeners() }
        loaded = true
    }

    val sheetShape = RoundedCornerShape(topStart = ComradeRadii.xl, topEnd = ComradeRadii.xl)
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
        modifier = Modifier.glassSurface(GlassElevation.Sheet, shape = sheetShape),
        shape = sheetShape,
        containerColor = Color.Transparent,
    ) {
        Column(
            Modifier
                .fillMaxWidth()
                .padding(horizontal = 20.dp)
                .padding(bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                stringResource(R.string.together_who_title),
                style = MaterialTheme.typography.titleLarge,
            )
            Text(
                stringResource(R.string.together_who_note),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            // Above the people and outside the search, because it is not a
            // person and because it is the answer that always works: it needs no
            // contact, no permission and no network, which is exactly what makes
            // the tab usable as an ordinary music player.
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable { onAlone() }
                    .padding(vertical = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Icon(
                    QueueMusicIcon,
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Column(Modifier.weight(1f)) {
                    Text(
                        stringResource(R.string.together_who_alone),
                        style = MaterialTheme.typography.bodyLarge,
                    )
                    Text(
                        stringResource(R.string.together_who_alone_note),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            HorizontalDivider()
            OutlinedTextField(
                value = query,
                onValueChange = { query = it },
                singleLine = true,
                leadingIcon = { Icon(Icons.Filled.Search, contentDescription = null) },
                placeholder = { Text(stringResource(R.string.together_who_search)) },
                modifier = Modifier.fillMaxWidth(),
            )
            val shown = remember(all, query) { TogetherDecisions.listenersFor(all, query) }
            when {
                !loaded -> Text(
                    stringResource(R.string.together_who_loading),
                    style = MaterialTheme.typography.bodyMedium,
                )

                shown.isEmpty() -> Text(
                    stringResource(
                        if (all.isEmpty()) {
                            R.string.together_who_nobody
                        } else {
                            R.string.together_who_no_match
                        },
                    ),
                    style = MaterialTheme.typography.bodyMedium,
                )

                else -> LazyColumn(Modifier.fillMaxWidth().height(320.dp)) {
                    items(shown, key = { it.npub }) { listener ->
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable { onChosen(listener) }
                                .padding(vertical = 12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(12.dp),
                        ) {
                            Icon(
                                Icons.Filled.Person,
                                contentDescription = null,
                                tint = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                            Text(
                                listener.label,
                                style = MaterialTheme.typography.bodyLarge,
                                modifier = Modifier.weight(1f),
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                            // Only when it is true, and never a grey dot for
                            // "offline": presence is mutual, so its absence
                            // means "we cannot see them" as often as it means
                            // they are away, and a grey dot claims the second.
                            if (listener.online) {
                                Text(
                                    stringResource(R.string.together_who_online),
                                    style = MaterialTheme.typography.labelSmall,
                                    color = MaterialTheme.colorScheme.primary,
                                )
                            }
                        }
                    }
                }
            }
        }
    }
}

/**
 * Everyone this device could invite, with presence where there is any.
 *
 * Contacts are the population and comrades are the presence: a comrade is a
 * contact we exchange beacons with, so the two lists are joined on the npub
 * rather than one being read instead of the other. Reading only comrades would
 * hide everybody the user has not chosen, and reading only contacts would drop
 * the one signal that says who is actually there.
 */
private fun listeners(): List<TogetherDecisions.Listener> {
    val online = runCatching { ComradeCore.comrades() }.getOrDefault(emptyList())
        .associateBy { it.npub }
    return runCatching { ComradeCore.contacts() }.getOrDefault(emptyList()).map { contact ->
        TogetherDecisions.Listener(
            npub = contact.npub,
            label = peerTitle(contact.npub, contact.alias, contact.name),
            comrade = contact.comrade,
            online = online[contact.npub]?.online == true,
        )
    }
}

// ── The session ──────────────────────────────────────────────────────────────

/**
 * Where the picture goes.
 *
 * **This is the fix for a film playing as sound only**: `MediaPlayer` decodes
 * video to whatever surface it is given and silently discards it when given
 * none, and until now nothing gave it one. The surface is created and destroyed
 * on every rotation while the session and the player outlive both, so ownership
 * runs one way — the holder callbacks tell [TogetherManager] what exists, and
 * the player re-attaches whenever it is handed something.
 *
 * Rendered only once the decoder reports a picture, so a shared album gets the
 * controls with no black rectangle above them.
 */
@Composable
private fun VideoSurface(picture: TogetherDecisions.Picture.Video, modifier: Modifier = Modifier) {
    // The sleeve that contains this already carries the aspect ratio, so the
    // surface only fills it. Two things applying a ratio is how a film ends up
    // letterboxed inside a box that was already the right shape.
    if (TogetherDecisions.aspectRatioOf(picture) == null) return
    AndroidView(
        modifier = modifier.fillMaxSize(),
        factory = { ctx ->
            SurfaceView(ctx).apply {
                holder.addCallback(object : SurfaceHolder.Callback {
                    override fun surfaceCreated(holder: SurfaceHolder) {
                        TogetherManager.attachSurface(holder.surface)
                    }

                    override fun surfaceChanged(
                        holder: SurfaceHolder,
                        format: Int,
                        width: Int,
                        height: Int,
                    ) = Unit

                    // Not tidiness: a destroyed Surface the decoder still holds
                    // is a use-after-free in the media server.
                    override fun surfaceDestroyed(holder: SurfaceHolder) {
                        TogetherManager.attachSurface(null)
                    }
                })
            }
        },
    )
}

/**
 * The third way to accept an invitation: follow the app this phone is already
 * playing in.
 *
 * `docs/TOGETHER.md` §13. Offered only for content Comrade cannot play itself —
 * [PlaybackModeDecision.ownershipFor] is what says so, and the manager asks it
 * rather than this screen guessing from the kind string.
 *
 * **The copy names no service and no client, and that is a rule rather than an
 * oversight.** The feature drives whatever published a media session; naming a
 * particular app — especially a patched one — would convert a neutral tool into
 * a targeted one regardless of what the code does. §13 explains why that
 * distinction matters, and it applies to strings, docs and store listing alike.
 */
@Composable
private fun FollowWhatIsPlaying(
    invited: TogetherManager.UiState.Invited,
    refusal: TogetherManager.FollowRefusal?,
    onTry: () -> Unit,
) {
    val context = LocalContext.current
    // A video plays here and a file is opened here, so neither wants this. Asked
    // of the same decision the manager will apply, so the button and the action
    // cannot disagree about when it is available.
    val couldFollow = PlaybackModeDecision.ownershipFor(
        contentKind = invited.contentKind,
        haveOurCopy = false,
        externalSessionAvailable = true,
    ) == PlaybackOwnership.EXTERNAL
    if (!couldFollow) return

    TextButton(onClick = onTry) { Text(stringResource(R.string.together_follow)) }

    when (refusal) {
        // The explainer comes *before* the settings screen, not after: the
        // permission cannot be requested in-app, so the system screen arrives
        // with no context of its own and a notification-access prompt with no
        // explanation is one people are right to refuse.
        TogetherManager.FollowRefusal.NeedsAccess -> {
            Text(
                stringResource(R.string.together_follow_explainer),
                style = MaterialTheme.typography.bodySmall,
            )
            TextButton(onClick = {
                runCatching { context.startActivity(MediaSessionAccess.settingsIntent()) }
            }) {
                Text(stringResource(R.string.together_follow_grant))
            }
        }
        // Granted, and there is simply nothing to follow. A different sentence
        // on purpose: sending someone back to a settings screen they have
        // already used is the refusal that teaches people the button is broken.
        TogetherManager.FollowRefusal.NothingPlaying -> Text(
            stringResource(R.string.together_follow_nothing_playing),
            style = MaterialTheme.typography.bodySmall,
        )
        // The invitation went away underneath us; the screen is about to change
        // anyway, so it says nothing.
        TogetherManager.FollowRefusal.NoInvitation, null -> Unit
    }
}

/**
 * A `VideoTrack` on screen, for a streamed session (`docs/TOGETHER.md` §15).
 *
 * Deliberately a much plainer thing than the call screen's renderer: there is no
 * mirroring (nobody is looking at themselves), no picture-in-picture z-order and
 * no letterbox decision, because the sleeve around this already carries the
 * aspect ratio. What it keeps is the part that is not optional — `release()` on
 * disposal, and detaching the sink before that, since a renderer left attached
 * to a live track is a native buffer nobody frees.
 */
@Composable
private fun StreamRenderer(track: VideoTrack?, modifier: Modifier = Modifier) {
    val egl = CallManager.eglBaseContext
    if (egl == null) {
        // No WebRTC on this device: an empty sleeve is honest, where a black
        // rectangle would look like a picture that failed to arrive.
        return
    }
    val context = LocalContext.current
    val renderer = remember {
        SurfaceViewRenderer(context).apply {
            init(egl, null)
            setEnableHardwareScaler(true)
            setScalingType(RendererCommon.ScalingType.SCALE_ASPECT_FIT)
        }
    }
    DisposableEffect(renderer) { onDispose { renderer.release() } }
    DisposableEffect(track, renderer) {
        track?.addSink(renderer)
        onDispose { track?.removeSink(renderer) }
    }
    AndroidView(factory = { renderer }, modifier = modifier.fillMaxSize())
}

/**
 * The YouTube embed, hosted in our own window.
 *
 * **The standard player, with its controls and its ads, and that is a term of
 * use rather than a default nobody changed.** YouTube's API Services Terms
 * prohibit hiding the player or stripping ads; `docs/TOGETHER.md` §11a records
 * why the ReVanced/InnerTube route is declined and what the ad-free answer
 * actually is (§11a's `Stream` sources). So `controls(1)`, and no custom UI.
 *
 * `enableAutomaticInitialization` is switched off because the automatic path
 * binds the view to a `LifecycleOwner` and releases the player when that owner
 * stops — which is a Compose screen here, and a session must not end because a
 * screen was disposed. [TogetherManager] owns the player's lifetime instead,
 * exactly as it owns the `MediaPlayer`'s, and `onRelease` below hands the view
 * back rather than tearing the session down.
 */
@Composable
private fun EmbedSurface(modifier: Modifier = Modifier) {
    AndroidView(
        modifier = modifier.fillMaxSize(),
        factory = { ctx ->
            YouTubePlayerView(ctx).apply {
                enableAutomaticInitialization = false
                initialize(
                    object : com.pierfrancescosoffritti.androidyoutubeplayer.core.player.listeners.AbstractYouTubePlayerListener() {},
                    // Network events handled: the library re-loads the player
                    // when connectivity comes back, which is the difference
                    // between a session that survives a tunnel and one that
                    // needs the app restarting.
                    true,
                    IFramePlayerOptions.Builder()
                        .controls(1)
                        // Autoplay off: the session decides when playback
                        // starts, so both people start together rather than one
                        // of them starting on arrival.
                        .autoplay(0)
                        .rel(0)
                        .build(),
                )
                TogetherManager.attachEmbedView(this)
            }
        },
        // Not tidiness, and the same shape as `surfaceDestroyed` above: a
        // released `WebView` the session still holds is a player being driven
        // into a dead page.
        onRelease = { view ->
            TogetherManager.attachEmbedView(null)
            view.release()
        },
    )
}

/**
 * The one question this feature asks before spending someone else's bandwidth.
 *
 * Modal because it is genuinely blocking — the transfer sits still until it is
 * answered — and dismissible only into "no", since there is no third outcome
 * and a silently dropped question would be the stall this was built to fix.
 */
@Composable
private fun ShareRelayConsent() {
    val question by ShareTransfer.consentQuestion.collectAsState()
    val text = question ?: return
    val dialogShape = RoundedCornerShape(ComradeRadii.xl)
    AlertDialog(
        onDismissRequest = { ShareTransfer.refuseShareConsent() },
        modifier = Modifier.glassSurface(GlassElevation.Sheet, shape = dialogShape),
        shape = dialogShape,
        containerColor = Color.Transparent,
        title = { Text(stringResource(R.string.together_relay_title)) },
        text = { Text(text) },
        confirmButton = {
            TextButton(onClick = { ShareTransfer.grantShareConsent() }) {
                Text(stringResource(R.string.together_relay_yes))
            }
        },
        dismissButton = {
            TextButton(onClick = { ShareTransfer.refuseShareConsent() }) {
                Text(stringResource(R.string.together_relay_no))
            }
        },
    )
}

/**
 * What to say when the embed will not play this — and where to go instead.
 *
 * The panel above it is YouTube's own, and §11a is why it may not be replaced or
 * hidden. This is the session's answer underneath it, which until now was
 * nothing at all: the error reached logcat, the transport stayed, and the status
 * line went on saying we were waiting for the other person to open something
 * that was never going to open.
 *
 * By far the most common cause is a video whose owner does not allow it outside
 * YouTube, and that one has a real next step — the video is fine, it just has to
 * be watched over there. Which sentence applies is
 * [TogetherDecisions.embedFailure]'s to decide.
 */
@Composable
private fun EmbedRefusal() {
    val context = LocalContext.current
    val failure by TogetherManager.embedFailure.collectAsState()
    val why = failure ?: return
    Text(
        stringResource(
            when (why) {
                TogetherDecisions.EmbedFailure.NotEmbeddable -> R.string.together_embed_not_embeddable
                TogetherDecisions.EmbedFailure.NotFound -> R.string.together_embed_not_found
                TogetherDecisions.EmbedFailure.Unknown -> R.string.together_embed_failed
            },
        ),
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.error,
    )
    // Only when there is somewhere to send them. `watchUrl` answers null for
    // anything that is not an id, which is the one rule for building a URL out
    // of a string that arrived over the wire.
    TogetherManager.watchUrl()?.let { url ->
        TextButton(onClick = {
            runCatching {
                context.startActivity(
                    android.content.Intent(android.content.Intent.ACTION_VIEW, Uri.parse(url)),
                )
            }.onFailure { Log.w(TAG, "no browser for the video", it) }
        }) {
            Text(stringResource(R.string.together_embed_watch_on_youtube))
        }
    }
    // Said plainly, because leaving for YouTube takes them out of the session:
    // the embed pauses in the background and nothing on the other device
    // changes.
    Text(
        stringResource(R.string.together_embed_watch_note),
        style = MaterialTheme.typography.bodySmall,
        color = TogetherMuted,
    )
}

@Composable
private fun LiveSession(
    s: TogetherManager.UiState.Live,
    onStream: () -> Unit,
    micBlocked: Boolean,
    onMic: () -> Unit,
    onPlaySomethingElse: () -> Unit,
) {
    // Hold the screen awake for a playing film and nothing else — two hours of
    // music must not burn the battery lighting up a screen with nothing on it.
    // The rule is TogetherDecisions.keepScreenOn, tested there; this only
    // applies it and hands it back on the way out.
    val view = LocalView.current
    val keepOn = TogetherDecisions.keepScreenOn(s.picture, s.playing)
    DisposableEffect(keepOn) {
        view.keepScreenOn = keepOn
        onDispose { view.keepScreenOn = false }
    }

    // The centrepiece, and music-first: a square sleeve with the cover in it,
    // and the video surface *inside* the same block when the recording turns out
    // to have a picture. One block, so an album gets a cover and a film gets a
    // screen without two layouts to keep in step — the same shape the desktop
    // player uses.
    //
    // Absent entirely when another app holds the playback (docs/TOGETHER.md
    // §13): there is nothing of ours to draw, and an empty sleeve over somebody
    // else's music would be a picture of a player Comrade does not have.
    if (!s.external) Sleeve(s)

    Spacer(Modifier.height(4.dp))
    Text(
        s.title.ifBlank { s.peerLabel },
        style = MaterialTheme.typography.headlineSmall,
        fontWeight = FontWeight.SemiBold,
        color = TogetherText,
        maxLines = 2,
        overflow = TextOverflow.Ellipsis,
    )
    // Everything from here to the transport is *about the other person*, so
    // listening alone draws none of it — there is no name to put in "with …",
    // no status that is not about them, and no gap between two playheads when
    // there is one playhead. What a solo session keeps is the sleeve, the
    // title, the transport and the queue, which is a music player.
    if (!s.solo) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(Spacing.space2),
        ) {
            Text(
                stringResource(R.string.together_with, s.peerLabel),
                style = MaterialTheme.typography.bodyMedium,
                color = TogetherMuted,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text("·", color = TogetherMuted)
            Text(statusLabel(s), style = MaterialTheme.typography.bodyMedium, color = TogetherMuted)
        }

        // The measured half, which the desktop player has had since 2026-08-05
        // and this one did not. Recomputed on every recomposition rather than
        // stored, because whether these may be shown at all depends on how old
        // the reading is *now* — see `TogetherDecisions.measurement`. The
        // position poll drives a recomposition every 250 ms while playing, which
        // is what ages them off the screen once corrections stop arriving.
        val measured = TogetherDecisions.measurement(
            driftMs = s.driftMs,
            qualityMs = s.qualityMs,
            ageMs = System.currentTimeMillis() - s.correctedAtMs,
        )
        // Before the drift line, deliberately: a stalled stream is the *cause* of
        // the gap the drift line is about to report, and reading them in the other
        // order makes the gap look like a sync fault. Until now nothing showed
        // this at all — the transport said "playing", the playhead stopped, and
        // the only visible effect was the drift line growing.
        if (s.buffering) {
            Text(
                stringResource(R.string.together_buffering),
                style = MaterialTheme.typography.bodySmall,
                color = TogetherMuted,
            )
        }
        driftLabel(measured.drift)?.let {
            Text(it, style = MaterialTheme.typography.bodySmall, color = TogetherMuted)
        }
        // Deliberately not colour-coded, on either frontend: "we've lost track
        // of them" is an honest report of poor measurement, not a fault, and red
        // would say otherwise.
        qualityLabel(measured.quality)?.let {
            Text(it, style = MaterialTheme.typography.bodySmall, color = TogetherMuted)
        }

        // What we tried to tell them did not go. The player kept playing, which
        // is the point — your own music has no business waiting on a relay — so
        // this is what admits the other half did not happen.
        val unsent by TogetherManager.sendFailed.collectAsState()
        if (unsent) {
            Text(
                stringResource(R.string.together_send_failed),
                style = MaterialTheme.typography.bodySmall,
                color = TogetherMuted,
            )
        }
    }

    // Control-and-status, and the honest limit of it, while another app plays.
    if (s.external) {
        Text(
            stringResource(R.string.together_follow_note),
            style = MaterialTheme.typography.bodySmall,
            color = TogetherMuted,
        )
    }

    // The source refused to play. Said here rather than only in logcat, because
    // a pasted link that turns out to be a web page fails several seconds after
    // the session opens and nothing else on this screen would change.
    val failed by TogetherManager.openFailed.collectAsState()
    if (failed) {
        Text(
            stringResource(R.string.together_could_not_play),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.error,
        )
    }

    EmbedRefusal()

    Transport(s, onMic = onMic, onPlaySomethingElse = onPlaySomethingElse)

    if (micBlocked) {
        Text(
            stringResource(R.string.together_mic_unavailable),
            style = MaterialTheme.typography.bodySmall,
            color = TogetherMuted,
        )
    }

    // The third answer to §9a's question, beside "find your own copy" and "take
    // mine": let them watch this one as it plays. Offered only by the side that
    // holds the file and only for our own player — an embed is already on both
    // screens, and an external session is somebody else's audio to send.
    if (s.weLead && !s.solo && !s.embed && !s.external && !s.streaming) {
        TextButton(onClick = onStream) { Text(stringResource(R.string.together_stream)) }
        Text(
            stringResource(R.string.together_stream_note),
            style = MaterialTheme.typography.bodySmall,
            color = TogetherMuted,
        )
        // Before the system dialog, not after: it arrives with no explanation of
        // its own, and a recording prompt nobody can account for is one people
        // are right to refuse.
        Text(
            stringResource(R.string.together_stream_consent),
            style = MaterialTheme.typography.bodySmall,
            color = TogetherMuted,
        )
    }
    if (s.streaming) {
        Text(
            stringResource(R.string.together_streaming),
            style = MaterialTheme.typography.bodyMedium,
            color = TogetherText,
        )
        // The one thing about the microphone that is not obvious from the icon.
        Text(
            stringResource(R.string.together_mic_note),
            style = MaterialTheme.typography.bodySmall,
            color = TogetherMuted,
        )
    }

    TextButton(onClick = { TogetherManager.leave() }) {
        Text(
            stringResource(
                if (s.solo) R.string.together_stop else R.string.together_leave,
            ),
        )
    }

    // The honest limits, on screen rather than in a doc nobody reads.
    Text(
        stringResource(R.string.together_accuracy_note),
        style = MaterialTheme.typography.bodySmall,
        color = TogetherMuted,
    )
    // The background promise is true of our own player and false of an embed —
    // YouTube pauses a backgrounded one, and turning that off is a feature of
    // their client rather than something this app may grant on their behalf. A
    // note that claimed otherwise would be the kind of comment-shaped lie the
    // repo's conventions call a bug, printed at the user instead.
    Text(
        stringResource(
            if (s.embed) R.string.together_embed_background_note else R.string.together_background_note,
        ),
        style = MaterialTheme.typography.bodySmall,
        color = TogetherMuted,
    )
    Spacer(Modifier.height(24.dp))
}

/**
 * The artwork block: a cover, a video surface, an embed or an incoming stream —
 * whichever this session turned out to have, in one frame that owns the shape.
 *
 * The gentle scale between playing and paused is the only animation on this
 * screen. It is there because a paused player and a playing one otherwise look
 * identical apart from one glyph, and a still cover that visibly settles when
 * the other person pauses says "something happened" before the status line has
 * been read.
 */
@Composable
private fun Sleeve(s: TogetherManager.UiState.Live) {
    val video = s.picture as? TogetherDecisions.Picture.Video
    val scale by animateFloatAsState(
        targetValue = if (s.playing) 1f else 0.94f,
        animationSpec = tween(durationMillis = 260),
        label = "sleeve",
    )
    Box(
        modifier = Modifier
            .fillMaxWidth()
            .then(
                video
                    ?.let { p -> TogetherDecisions.aspectRatioOf(p)?.let { Modifier.aspectRatio(it) } }
                    ?: Modifier.aspectRatio(1f),
            )
            // Only the cover breathes. A video surface and a WebView are handed
            // to the framework, and scaling either one costs a re-layout of a
            // view that is decoding — so the transform is applied to the still
            // case only, which is also the only case it says anything about.
            .then(
                if (video == null && !s.embed && !s.streaming) {
                    Modifier.scale(scale)
                } else {
                    Modifier
                },
            )
            .clip(RoundedCornerShape(24.dp))
            .background(TogetherSleeve),
        contentAlignment = Alignment.Center,
    ) {
        when {
            // Streaming takes the player's only output surface, so the sender
            // cannot also watch a SurfaceView — they watch the very track the
            // other person receives. One picture path instead of two, and the
            // same thing the call screen does with local camera video.
            s.streaming -> {
                val outgoing by TogetherManager.localVideo.collectAsState()
                val incoming by TogetherManager.remoteVideo.collectAsState()
                // Whichever exists: the sender has an outgoing track and no
                // incoming one, the receiver the reverse. Asked this way round
                // rather than from `weLead` because a stream's direction is a
                // property of which tracks are present, and those are what the
                // renderer actually needs.
                StreamRenderer(outgoing ?: incoming)
            }
            // The embed draws itself, controls and all, inside the same sleeve
            // the file path uses — so a video has one owner of the aspect ratio
            // whichever player is behind it.
            s.embed -> EmbedSurface()
            video == null -> Cover(
                uri = s.sourceUri,
                // Not known for a session: `MediaStore`'s album id is a library
                // detail and a session remembers only what it opened. Costs the
                // cover below API 29 — see `MusicLibrary.artwork`.
                albumId = null,
                requestDp = SLEEVE_COVER_DP,
                corner = 24.dp,
                glyphDp = 72,
                modifier = Modifier.fillMaxSize(),
            )
            else -> VideoSurface(video)
        }
    }
}

/**
 * How big a cover is asked of the provider.
 *
 * Roughly the width each one is drawn at, and fixed rather than measured — see
 * [Cover]. The sleeve one is deliberately under a phone's full width: a cover is
 * a JPEG inside an MP3, so asking for more pixels than it has buys an upscale
 * and a bigger bitmap in the cache.
 */
private const val SLEEVE_COVER_DP = 320
private const val ROW_COVER_DP = 48

/**
 * The narrowest a cover tile may be before the grid drops a column.
 *
 * Chosen so a 360 dp phone gets two columns with room for the name under each,
 * and a tablet or a landscape phone gets as many as fit — which is what
 * `GridCells.Adaptive` is for, and it is why this is a minimum rather than a
 * column count.
 */
private const val GRID_TILE_DP = 152

/**
 * What the provider is asked for, per tile.
 *
 * Roughly a tile's width and not more. `GridCells.Adaptive` gives a 360 dp phone
 * two columns of about 154 dp, so this asks for a little under what it draws —
 * a ~10% saving rather than a dramatic one, and the honest reason to keep it a
 * separate constant is that the tile's width is decided at layout while the
 * decode has to be asked for before it. Requesting *more* than the tile is what
 * would matter: these are decoded a screenful at a time into a bounded cache
 * (`MusicLibrary.CACHE_BYTES`), and a cover is a photograph, so there is nothing
 * to gain above the size it is drawn at.
 */
private const val GRID_COVER_DP = 144

/**
 * Scrubber and transport.
 *
 * The scrubber is drawn only when there is a distance for it to express —
 * [TogetherDecisions.scrubbable], which is the rule and not a local judgement. A
 * `MediaSession` carries no duration we can trust and an embed reports none
 * until it loads, so both would otherwise get a bar with no end on it: a
 * scrubber that lies about where the end is, which is worse than no scrubber.
 * Play, pause and the four skips all still work, because those need no length.
 *
 * **Two rows, because this is a music player.** The top one is track-level —
 * previous, ten back, play, ten forward, next — and the bottom one is the two
 * controls that are about the session rather than the playhead: the microphone
 * and the way to put something else on. Cramming seven controls into one line
 * makes the play button small, and the play button is the one thing anybody
 * reaches for without looking.
 *
 * @param onMic pressed the microphone. Hoisted because turning it on may need a
 *   runtime permission, and a launcher belongs at the top of the screen rather
 *   than inside a row that is created and destroyed as the session changes.
 * @param onPlaySomethingElse open the chooser without asking who again.
 */
@Composable
private fun Transport(
    s: TogetherManager.UiState.Live,
    onMic: () -> Unit,
    onPlaySomethingElse: () -> Unit,
) {
    // While a finger is on the slider the poll must not move it — the decision
    // is TogetherDecisions.pollMayMoveSlider, and the manager honours it; this
    // only has to report the drag boundaries.
    var dragging by remember { mutableFloatStateOf(-1f) }
    val max = s.durationMs.coerceAtLeast(1L).toFloat()
    val shown = if (dragging >= 0f) dragging.toLong() else s.positionMs

    if (TogetherDecisions.scrubbable(s.durationMs, s.external)) {
        Slider(
            value = shown.toFloat().coerceIn(0f, max),
            onValueChange = {
                if (dragging < 0f) TogetherManager.onScrubStart()
                dragging = it
            },
            onValueChangeFinished = {
                val target = dragging.toLong()
                dragging = -1f
                TogetherManager.onScrubRelease(target)
            },
            valueRange = 0f..max,
            colors = SliderDefaults.colors(
                thumbColor = TogetherText,
                activeTrackColor = TogetherText,
                inactiveTrackColor = TogetherCard,
            ),
            modifier = Modifier.fillMaxWidth(),
        )
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            Text(
                TogetherDecisions.clock(shown),
                style = MaterialTheme.typography.labelMedium,
                color = TogetherMuted,
            )
            // Nothing at all rather than `0:00` when no length is known — the
            // decision is `remainingClock`'s, tested there.
            TogetherDecisions.remainingClock(shown, s.durationMs)?.let {
                Text(it, style = MaterialTheme.typography.labelMedium, color = TogetherMuted)
            }
        }
    }

    val context = LocalContext.current
    // Previous / back / play-pause / forward / next, centred. Every one of them
    // goes through `setState` or through the manager's queue, so they are
    // ordered by the same Lamport counter as the other person's and cannot race
    // them.
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(top = 8.dp),
        horizontalArrangement = Arrangement.Center,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val skip = { delta: Long ->
            // Clamped at the top only when a length is known: an external
            // session reports none, and clamping to zero there would turn every
            // skip into "back to the start".
            val ceiling = if (s.durationMs > 0) s.durationMs else Long.MAX_VALUE
            val target = (s.positionMs + delta).coerceIn(0L, ceiling)
            TogetherManager.setState(target, s.playing)
        }
        // Previous is always live, because it always does something: with no
        // track behind this one it restarts this one, which is
        // `TogetherDecisions.backStep`'s answer and the reason a back button
        // never has to be greyed out.
        IconButton(onClick = { TogetherManager.skipBack(context) }) {
            Icon(
                SkipPreviousIcon,
                contentDescription = stringResource(R.string.together_previous),
                tint = TogetherText,
            )
        }
        TextButton(onClick = { skip(-SKIP_MS) }) {
            Text(stringResource(R.string.together_back_ten), color = TogetherText)
        }
        // The one big control. A filled circle rather than a Button, because at
        // this size the label would be the shape — and because it is the only
        // thing on the screen anybody reaches for in the dark.
        Box(
            modifier = Modifier
                .padding(horizontal = 8.dp)
                .size(64.dp)
                .clip(CircleShape)
                .background(MaterialTheme.colorScheme.primary)
                .clickable { TogetherManager.setState(s.positionMs, !s.playing) },
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                if (s.playing) PauseIcon else Icons.Filled.PlayArrow,
                contentDescription = stringResource(
                    if (s.playing) R.string.together_pause else R.string.together_play,
                ),
                tint = MaterialTheme.colorScheme.onPrimary,
                modifier = Modifier.size(32.dp),
            )
        }
        TextButton(onClick = { skip(SKIP_MS) }) {
            Text(stringResource(R.string.together_forward_ten), color = TogetherText)
        }
        // Next, unlike previous, genuinely has nothing to do at the end of a
        // queue or with no queue at all — a pasted link and a picked file are
        // one thing each. Drawn and disabled rather than absent: a control that
        // appears and disappears under the thumb is worse than one that is
        // visibly not available.
        val queue by TogetherManager.queue.collectAsState()
        val hasNext = TogetherDecisions.nextTrack(queue) != null
        IconButton(
            onClick = { TogetherManager.skipForward(context) },
            enabled = hasNext,
        ) {
            Icon(
                SkipNextIcon,
                contentDescription = stringResource(R.string.together_next),
                tint = if (hasNext) TogetherText else TogetherMuted.copy(alpha = 0.4f),
            )
        }
    }

    // The session-level row. The microphone first, because it is the one people
    // look for.
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(bottom = 4.dp),
        horizontalArrangement = Arrangement.Center,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        // **In every mode, not only a streamed one.** It used to appear only
        // while streaming, on the argument that there was otherwise no audio of
        // ours going anywhere — true of the wire and beside the point for the
        // person: listening to an album together with no way to say "this bit"
        // is a worse version of listening alone. What differs between the modes
        // is only what the voice rides on, which is [TogetherManager.toggleMic]'s
        // problem rather than this row's.
        //
        // Off by default, always. A session that opened with a live microphone
        // would have decided something about a room it cannot see.
        val micOn by TogetherManager.micEnabled.collectAsState()
        IconButton(onClick = onMic) {
            Icon(
                if (micOn) MicIcon else MicOffIcon,
                contentDescription = stringResource(
                    if (micOn) R.string.together_mic_off else R.string.together_mic_on,
                ),
                tint = if (micOn) MaterialTheme.colorScheme.primary else TogetherMuted,
            )
        }
        // The second half of "choose a person once": something else to play,
        // with no second trip through the who-with sheet. Absent for an
        // external session, where Comrade holds no player and putting something
        // on would mean taking the playback away from the app that has it.
        if (!s.external) {
            TextButton(onClick = onPlaySomethingElse) {
                Text(stringResource(R.string.together_play_something_else), color = TogetherText)
            }
        }
    }
}

/**
 * The gap, or nothing. Mirrors `driftLabel` in `desktop/ui/player_view.mjs`;
 * the decision to say anything at all is [TogetherDecisions.measurement]'s, and
 * this only puts it into words.
 */
@Composable
private fun driftLabel(drift: TogetherDecisions.Drift): String? = when (drift) {
    is TogetherDecisions.Drift.Silent -> null
    is TogetherDecisions.Drift.Gap -> stringResource(
        if (drift.weAreAhead) R.string.together_drift_ahead else R.string.together_drift_behind,
        secondsText(drift.ms, decimals = 1),
    )
}

/** How well we can measure. Mirrors `qualityLabel` in the desktop module. */
@Composable
private fun qualityLabel(quality: TogetherDecisions.Quality): String? = when (quality) {
    is TogetherDecisions.Quality.Unknown -> null
    is TogetherDecisions.Quality.Known -> stringResource(
        if (quality.direct) R.string.together_quality_direct else R.string.together_quality_relayed,
        secondsText(quality.ms, quality.decimals),
    )
}

/**
 * Milliseconds as seconds, to a fixed number of places.
 *
 * Formatted in the reader's own locale rather than [Locale.ROOT] — this number
 * sits inside a translated sentence, and "0,05" is what a decimal comma reader
 * expects to see there. The *arithmetic* is what the JVM tests pin, in
 * `TogetherDecisionsTest`, which is why it is not in here.
 */
private fun secondsText(ms: Long, decimals: Int): String =
    String.format(Locale.getDefault(), "%.${decimals}f", ms / 1000.0)

/** The status vocabulary, mirroring `sessionStatusLabel` in the desktop module. */
@Composable
private fun statusLabel(s: TogetherManager.UiState.Live): String = when (s.status) {
    TogetherManager.Status.WaitingForThem -> stringResource(R.string.together_waiting_for_them)
    TogetherManager.Status.OpenYourCopy -> stringResource(R.string.together_open_your_copy)
    TogetherManager.Status.Together -> stringResource(R.string.together_together)
    TogetherManager.Status.CatchingUp -> stringResource(R.string.together_catching_up)
    TogetherManager.Status.LostTrack -> stringResource(R.string.together_lost_track)
    TogetherManager.Status.TheyPaused -> stringResource(R.string.together_they_paused, s.peerLabel)
}

// ── The extras row: shuffle, repeat, speed, sleep, EQ, lyrics ────────────────

/**
 * One quiet row of player conveniences above the transport.
 *
 * Every control is conditional on where it is *true*, not where it fits: speed
 * exists only solo ([TogetherDecisions.speedAllowed] — in a session the
 * correction ladder owns the rate), the equalizer only where the sound comes
 * from our own decoder (an embed and a followed external app mix elsewhere),
 * shuffle and repeat only mean something with a queue behind the session. A
 * control that appears and silently does nothing is worse than one absent —
 * the same argument §22 made about greyed-out download buttons.
 */
@Composable
private fun ExtrasRow(s: TogetherManager.UiState.Live) {
    val context = LocalContext.current
    val extras by TogetherManager.extras.collectAsState()
    val queue by TogetherManager.queue.collectAsState()
    LaunchedEffect(Unit) { TogetherManager.loadExtras(context) }

    var showSpeed by remember { mutableStateOf(false) }
    var showSleep by remember { mutableStateOf(false) }
    var showEq by remember { mutableStateOf(false) }
    var showLyrics by remember { mutableStateOf(false) }
    var showQueue by remember { mutableStateOf(false) }
    val filePlayer = !s.embed && !s.external

    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.Center,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (queue != null) {
            IconButton(onClick = { TogetherManager.toggleShuffle(context) }) {
                Icon(
                    ShuffleIcon,
                    contentDescription = stringResource(R.string.extras_shuffle),
                    tint = if (extras.shuffle) MaterialTheme.colorScheme.primary else TogetherMuted,
                )
            }
            IconButton(onClick = { TogetherManager.cycleRepeat(context) }) {
                Icon(
                    if (extras.repeat == TogetherDecisions.RepeatMode.ONE) RepeatOneIcon else RepeatIcon,
                    contentDescription = stringResource(
                        if (extras.repeat == TogetherDecisions.RepeatMode.ONE) {
                            R.string.extras_repeat_one
                        } else {
                            R.string.extras_repeat
                        },
                    ),
                    tint = if (extras.repeat != TogetherDecisions.RepeatMode.OFF) {
                        MaterialTheme.colorScheme.primary
                    } else {
                        TogetherMuted
                    },
                )
            }
        }
        if (TogetherDecisions.speedAllowed(s.solo)) {
            IconButton(onClick = { showSpeed = true }) {
                Icon(SpeedIcon, contentDescription = stringResource(R.string.extras_speed), tint = TogetherMuted)
            }
        }
        IconButton(onClick = { showSleep = true }) {
            Icon(BedtimeIcon, contentDescription = stringResource(R.string.extras_sleep), tint = TogetherMuted)
        }
        if (filePlayer && PlayerEffects.bandCount() > 0) {
            IconButton(onClick = { showEq = true }) {
                Icon(TuneIcon, contentDescription = stringResource(R.string.extras_equalizer), tint = TogetherMuted)
            }
        }
        if (!queue?.current?.title.isNullOrBlank() || s.title.isNotBlank()) {
            IconButton(onClick = { showLyrics = true }) {
                Icon(LyricsIcon, contentDescription = stringResource(R.string.extras_lyrics), tint = TogetherMuted)
            }
        }
        if ((queue?.tracks?.size ?: 0) > 1) {
            IconButton(onClick = { showQueue = true }) {
                Icon(QueueMusicIcon, contentDescription = stringResource(R.string.extras_queue), tint = TogetherMuted)
            }
        }
    }

    if (showSpeed) {
        SpeedSheet(current = extras.speed, onDismiss = { showSpeed = false })
    }
    if (showSleep) {
        SleepSheet(endsAtMs = extras.sleepEndsAtMs, onDismiss = { showSleep = false })
    }
    if (showEq) {
        EqualizerSheet(sessionId = (TogetherManager.filePlayer() as? TogetherPlayer)?.audioSessionId, onDismiss = { showEq = false })
    }
    if (showLyrics) {
        LyricsSheet(
            title = queue?.current?.title ?: s.title,
            artist = queue?.current?.artist.orEmpty(),
            durationMs = s.durationMs,
            positionMs = s.positionMs,
            onDismiss = { showLyrics = false },
        )
    }
    if (showQueue) {
        QueueSheet(onDismiss = { showQueue = false })
    }
}

/**
 * Up next — the queue, arrangeable.
 *
 * Reordering and removing here are local: the session syncs a playhead, not a
 * playlist, so what this device plays *after* the current track is its own
 * business ([TogetherManager.reorderQueue] / [removeFromQueue] send nothing to
 * the peer). The row that is playing is marked and cannot be removed from this
 * sheet — that is a skip decision, and the transport controls own it. A plain
 * Column rather than a LazyColumn, like `PlaylistsScreen`: the queue is short
 * and the rows carry no state worth keying.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun QueueSheet(onDismiss: () -> Unit) {
    val queue by TogetherManager.queue.collectAsState()
    ModalBottomSheet(onDismissRequest = onDismiss) {
        val q = queue
        Column(
            Modifier.padding(20.dp).verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text(stringResource(R.string.queue_sheet_title), style = MaterialTheme.typography.titleMedium)
            if (q == null || q.tracks.isEmpty()) {
                Text(stringResource(R.string.queue_empty), color = TogetherMuted)
            } else {
                val lastIndex = q.tracks.size - 1
                q.tracks.forEachIndexed { i, t ->
                    val playing = i == q.index
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f).padding(vertical = 6.dp)) {
                            Text(
                                t.title.ifBlank { stringResource(R.string.queue_untitled) },
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                                color = if (playing) MaterialTheme.colorScheme.primary else TogetherText,
                            )
                            if (playing) {
                                Text(
                                    stringResource(R.string.queue_now_playing),
                                    style = MaterialTheme.typography.labelSmall,
                                    color = MaterialTheme.colorScheme.primary,
                                )
                            }
                        }
                        TextButton(enabled = i > 0, onClick = { TogetherManager.reorderQueue(i, i - 1) }) {
                            Text(stringResource(R.string.library_move_up))
                        }
                        TextButton(enabled = i < lastIndex, onClick = { TogetherManager.reorderQueue(i, i + 1) }) {
                            Text(stringResource(R.string.library_move_down))
                        }
                        TextButton(enabled = !playing, onClick = { TogetherManager.removeFromQueue(i) }) {
                            Text(stringResource(R.string.library_remove_from_playlist))
                        }
                    }
                }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SpeedSheet(current: Float, onDismiss: () -> Unit) {
    val context = LocalContext.current
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(stringResource(R.string.speed_sheet_title), style = MaterialTheme.typography.titleMedium)
            var value by remember(current) { mutableStateOf(current) }
            Slider(
                value = value,
                onValueChange = { value = TogetherDecisions.clampSpeed(it) },
                onValueChangeFinished = { TogetherManager.setSpeed(context, value) },
                valueRange = 0.5f..2f,
            )
            Text("×${"%.2f".format(value)}", fontFamily = FontFamily.Monospace)
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SleepSheet(endsAtMs: Long?, onDismiss: () -> Unit) {
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(stringResource(R.string.sleep_sheet_title), style = MaterialTheme.typography.titleMedium)
            listOf(15, 30, 45, 60).forEach { minutes ->
                TextButton(onClick = {
                    TogetherManager.startSleepTimer(minutes)
                    onDismiss()
                }) { Text(stringResource(R.string.sleep_minutes, minutes), color = TogetherText) }
            }
            endsAtMs?.let {
                TextButton(onClick = {
                    TogetherManager.cancelSleepTimer()
                    onDismiss()
                }) { Text(stringResource(R.string.sleep_off), color = MaterialTheme.colorScheme.error) }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun EqualizerSheet(sessionId: Int?, onDismiss: () -> Unit) {
    val context = LocalContext.current
    val bands = remember { PlayerEffects.bandCount() }
    val saved = remember { PlayerPrefs.equalizer(context) }
    var enabled by remember { mutableStateOf(saved.first) }
    var levels by remember {
        mutableStateOf(saved.second.ifEmpty { List(bands) { 0 } })
    }
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(stringResource(R.string.eq_sheet_title), style = MaterialTheme.typography.titleMedium)
            if (bands <= 0) {
                Text(stringResource(R.string.eq_unavailable), color = TogetherMuted)
            } else {
                levels.forEachIndexed { i, level ->
                    Text("${i + 1}", style = MaterialTheme.typography.labelSmall, color = TogetherMuted)
                    Slider(
                        value = level.toFloat(),
                        onValueChange = { newLevel ->
                            val next = levels.toMutableList().also { it[i] = newLevel.toInt() }
                            levels = next
                            PlayerEffects.applyLive(enabled, next, sessionId)
                            PlayerPrefs.setEqualizer(context, enabled, next)
                        },
                        valueRange = -1500f..1500f,
                    )
                }
                Switch(
                    checked = enabled,
                    onCheckedChange = { on ->
                        enabled = on
                        PlayerEffects.applyLive(on, levels, sessionId)
                        PlayerPrefs.setEqualizer(context, on, levels)
                    },
                )
            }
        }
    }
}

/** Synced lyrics, highlighted line by line against the live playhead. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun LyricsSheet(title: String, artist: String, durationMs: Long, positionMs: Long, onDismiss: () -> Unit) {
    val scope = rememberCoroutineScope()
    var lines by remember { mutableStateOf<List<TogetherDecisions.LyricLine>?>(null) }
    var note by remember { mutableStateOf<String?>(null) }
    // Resolved here, outside the effect: `stringResource` is composable-only,
    // and the failure sentences carry an argument, so those go through the
    // context instead.
    val loadingNote = stringResource(R.string.lyrics_loading)
    val noneNote = stringResource(R.string.lyrics_none)
    val cannotSearchNote = stringResource(R.string.together_server_cannot_stream)
    val context = LocalContext.current
    LaunchedEffect(title, artist) {
        note = loadingNote
        val result = withContext(Dispatchers.IO) {
            ComradeCore.lyricsLookup(title, artist, durationMs)
        }
        when (result) {
            is ComradeCore.LyricsResult.Found -> {
                lines = result.lines.map {
                    TogetherDecisions.LyricLine(it.atMs.toLong(), it.text)
                }
                note = if (result.lines.isEmpty()) noneNote else null
            }
            is ComradeCore.LyricsResult.CannotSearch -> note = cannotSearchNote
            is ComradeCore.LyricsResult.Failed ->
                note = context.getString(R.string.lyrics_failed, result.reason)
        }
    }
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.padding(20.dp).heightIn(max = 480.dp)) {
            Text(title, style = MaterialTheme.typography.titleMedium)
            Spacer(Modifier.height(8.dp))
            when {
                lines == null -> Text(note.orEmpty(), color = TogetherMuted)
                lines!!.isEmpty() -> Text(note.orEmpty(), color = TogetherMuted)
                else -> {
                    val active = TogetherDecisions.lyricIndexAt(lines!!, positionMs)
                    Column(verticalArrangement = Arrangement.spacedBy(Spacing.space3)) {
                        lines!!.forEachIndexed { i, line ->
                            Text(
                                line.text,
                                style = MaterialTheme.typography.bodyLarge,
                                color = if (i == active) MaterialTheme.colorScheme.primary else TogetherMuted,
                                fontWeight = if (i == active) FontWeight.SemiBold else FontWeight.Normal,
                            )
                        }
                    }
                }
            }
        }
    }
}

// ── Resume card ──────────────────────────────────────────────────────────────

/**
 * The saved queue, offered once at the top of Choosing.
 *
 * Solo only — [TogetherManager.resumeSavedQueue] enforces the same rule — and
 * hidden entirely when there is nothing saved or nothing resolvable. A card
 * that says "continue" and then starts over would be the small confident lie
 * this screen exists to avoid.
 */
@Composable
private fun ResumeCard() {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var saved by remember { mutableStateOf<uniffi.comrade_ui.SavedQueueDto?>(null) }
    LaunchedEffect(Unit) {
        saved = withContext(Dispatchers.IO) { runCatching { ComradeCore.queueLoad() }.getOrNull() }
    }
    val snapshot = saved ?: return
    ElevatedCard(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(stringResource(R.string.resume_title), style = MaterialTheme.typography.titleSmall)
            Text(
                stringResource(
                    R.string.resume_body,
                    snapshot.tracks.size,
                    java.text.DateFormat.getTimeInstance(java.text.DateFormat.SHORT)
                        .format(java.util.Date(snapshot.savedAtMs.toLong())),
                ),
                style = MaterialTheme.typography.bodySmall,
                color = TogetherMuted,
            )
            Button(onClick = {
                scope.launch { TogetherManager.resumeSavedQueue(context) }
            }) { Text(stringResource(R.string.resume_button)) }
        }
    }
}

// ── Favourites / recently played / playlists ─────────────────────────────────

/** Which remembered list a [RememberedList] is drawing. */
private enum class RememberedKind { Favourites, History }

/**
 * One of the player's own lists — favourites or recently played.
 *
 * Rows resolve to sessions through the same two doors everything else uses: a
 * local key that still opens plays as a library tap; a stream URL re-fetches.
 * A row that can no longer do either is skipped silently rather than drawn as
 * a broken button — the list remembers, but it does not haunt.
 */
@Composable
private fun RememberedList(
    onBack: () -> Unit,
    kind: RememberedKind,
    onPlayLocal: (TogetherDecisions.Track, List<TogetherDecisions.Track>) -> Unit,
    onPlayStream: (uniffi.comrade_ui.StreamCandidateDto) -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var rows by remember {
        mutableStateOf<List<Pair<uniffi.comrade_ui.PlayerTrackDto, Boolean>>>(emptyList())
    }
    var loaded by remember { mutableStateOf(false) }

    LaunchedEffect(kind) {
        val entries: List<uniffi.comrade_ui.PlayerTrackDto> = withContext(Dispatchers.IO) {
            runCatching {
                when (kind) {
                    RememberedKind.Favourites -> ComradeCore.favouritesList()
                    RememberedKind.History ->
                        ComradeCore.historyList().map { it.track }
                }
            }.getOrDefault(emptyList())
        }
        rows = entries.map { dto ->
            val uri = dto.key.removePrefix("local:")
            val exists = dto.kind == uniffi.comrade_ui.PlayerTrackKind.LOCAL && (
                uri.startsWith("content://") || java.io.File(android.net.Uri.parse(uri).path ?: "/").exists()
                )
            dto to (dto.kind == uniffi.comrade_ui.PlayerTrackKind.STREAM || exists)
        }
        loaded = true
    }

    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        BrowserHeader(
            stringResource(
                if (kind == RememberedKind.Favourites) {
                    R.string.library_favourites
                } else {
                    R.string.library_recent
                },
            ),
            onBack,
        )
        if (!loaded) {
            return@Column
        }
        if (rows.isEmpty()) {
            Text(
                stringResource(
                    if (kind == RememberedKind.Favourites) {
                        R.string.library_empty_favourites
                    } else {
                        R.string.library_empty_recent
                    },
                ),
                color = TogetherMuted,
            )
            return@Column
        }
        rows.forEach { (dto, playable) ->
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .then(if (playable) Modifier.clickable {
                        if (dto.kind == uniffi.comrade_ui.PlayerTrackKind.STREAM && dto.url != null) {
                            onPlayStream(
                                uniffi.comrade_ui.StreamCandidateDto(
                                    title = dto.title,
                                    artist = dto.artist,
                                    album = dto.album,
                                    durationMs = dto.durationMs,
                                    streamUrl = dto.url!!,
                                    artworkUrl = null,
                                ),
                            )
                        } else {
                            val track = TogetherDecisions.Track(
                                uri = dto.key.removePrefix("local:"),
                                title = dto.title,
                                artist = dto.artist,
                                album = dto.album,
                                durationMs = dto.durationMs.toLong(),
                                albumId = null,
                            )
                            // The queue is this list's locals, in list order —
                            // prev and next mean what this screen shows.
                            val queueList = rows.filter { (d, ok) ->
                                ok && d.kind == uniffi.comrade_ui.PlayerTrackKind.LOCAL
                            }.map { (d, _) ->
                                TogetherDecisions.Track(
                                    uri = d.key.removePrefix("local:"),
                                    title = d.title,
                                    artist = d.artist,
                                    album = d.album,
                                    durationMs = d.durationMs.toLong(),
                                    albumId = null,
                                )
                            }
                            onPlayLocal(track, queueList)
                        }
                    } else Modifier),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(Spacing.space4),
            ) {
                Icon(QueueMusicIcon, contentDescription = null, tint = TogetherMuted)
                Column(Modifier.weight(1f)) {
                    Text(dto.title, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(
                        dto.artist.ifBlank { stringResource(R.string.together_search_unknown_artist) },
                        style = MaterialTheme.typography.bodySmall,
                        color = TogetherMuted,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
        }
    }
}

/**
 * Playlists: create, open (play all), remove tracks. The one list with
 * editing, because a playlist is the only thing here that is *authored*.
 */
@Composable
private fun PlaylistsScreen(
    onBack: () -> Unit,
    onPlayTracks: (List<uniffi.comrade_ui.PlayerTrackDto>, Int) -> Unit,
) {
    val scope = rememberCoroutineScope()
    var lists by remember { mutableStateOf<List<uniffi.comrade_ui.PlaylistDto>>(emptyList()) }
    var loaded by remember { mutableStateOf(false) }
    var openId by remember { mutableStateOf<String?>(null) }
    var newName by remember { mutableStateOf("") }
    // Cleared whenever a different playlist is opened, so a half-typed rename
    // never bleeds from one list onto another.
    var renameDraft by remember(openId) { mutableStateOf("") }

    fun refresh() {
        scope.launch {
            lists = withContext(Dispatchers.IO) { runCatching { ComradeCore.playlistsList() }.getOrDefault(emptyList()) }
            loaded = true
        }
    }
    LaunchedEffect(Unit) { refresh() }

    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        BrowserHeader(stringResource(R.string.library_playlists), onBack)
        if (!loaded) return@Column

        openId?.let { id ->
            val list = lists.firstOrNull { it.id == id }
            if (list == null) {
                openId = null
                return@Column
            }
            BrowserHeader(list.name, onBack = { openId = null })

            // Rename: the name is authored, like the list itself. Placeholder
            // carries the current name so an empty field reads as "unchanged",
            // and core refuses a blank one anyway.
            Row(
                Modifier.fillMaxWidth().padding(vertical = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                OutlinedTextField(
                    value = renameDraft,
                    onValueChange = { renameDraft = it },
                    placeholder = { Text(list.name) },
                    singleLine = true,
                    modifier = Modifier.weight(1f),
                )
                TextButton(
                    enabled = renameDraft.isNotBlank() && renameDraft.trim() != list.name,
                    onClick = {
                        val name = renameDraft.trim()
                        scope.launch {
                            withContext(Dispatchers.IO) {
                                runCatching { ComradeCore.playlistRename(id, name) }
                            }
                            renameDraft = ""
                            refresh()
                        }
                    },
                ) { Text(stringResource(R.string.library_rename)) }
            }

            val lastIndex = list.tracks.size - 1
            list.tracks.forEachIndexed { i, t ->
                Row(
                    Modifier.fillMaxWidth().clickable { onPlayTracks(list.tracks, i) },
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(Modifier.weight(1f).padding(vertical = 8.dp)) {
                        Text(t.title, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                    // Up/down rather than a drag handle: a drag reorder in a
                    // plain scrolling Column is fiddly to get right and
                    // impossible to verify without a device. The arithmetic is
                    // shared with core via TogetherDecisions.reorderedOrder.
                    TextButton(
                        enabled = i > 0,
                        onClick = {
                            scope.launch {
                                withContext(Dispatchers.IO) {
                                    runCatching { ComradeCore.playlistReorder(id, i, i - 1) }
                                }
                                refresh()
                            }
                        },
                    ) { Text(stringResource(R.string.library_move_up)) }
                    TextButton(
                        enabled = i < lastIndex,
                        onClick = {
                            scope.launch {
                                withContext(Dispatchers.IO) {
                                    runCatching { ComradeCore.playlistReorder(id, i, i + 1) }
                                }
                                refresh()
                            }
                        },
                    ) { Text(stringResource(R.string.library_move_down)) }
                    TextButton(onClick = {
                        scope.launch {
                            withContext(Dispatchers.IO) {
                                runCatching { ComradeCore.playlistRemoveTrack(id, t.key) }
                            }
                            refresh()
                        }
                    }) { Text(stringResource(R.string.library_remove_from_playlist)) }
                }
            }
            return@Column
        }

        OutlinedTextField(
            value = newName,
            onValueChange = { newName = it },
            placeholder = { Text(stringResource(R.string.library_new_playlist_hint)) },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        Button(enabled = newName.isNotBlank(), onClick = {
            scope.launch {
                withContext(Dispatchers.IO) {
                    runCatching {
                        ComradeCore.playlistCreate(newName.trim(), System.currentTimeMillis())
                    }
                }
                newName = ""
                refresh()
            }
        }) { Text(stringResource(R.string.library_create)) }

        lists.forEach { list ->
            SourceCard(icon = QueueMusicIcon, title = list.name, subtitle = "${list.tracks.size}") {
                openId = list.id
            }
        }
    }
}

// ── Public collections: Internet Archive + podcast feeds ─────────────────────

/**
 * The keyless online sources, behind one door.
 *
 * Two tabs because the two flows genuinely differ (words-to-recordings versus
 * a feed address-to-episodes); one screen because both end in the same place:
 * guarded stream candidates that play exactly like server results. Every URL
 * here was built and guarded inside core — this screen never sees an
 * unvalidated one.
 */
@Composable
private fun CollectionsScreen(
    onBack: () -> Unit,
    onPlay: (uniffi.comrade_ui.StreamCandidateDto) -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var tab by remember { mutableStateOf(0) }
    var query by remember { mutableStateOf("") }
    var items by remember { mutableStateOf<List<uniffi.comrade_ui.ArchiveItemDto>>(emptyList()) }
    var note by remember { mutableStateOf<String?>(null) }
    var searching by remember { mutableStateOf(false) }
    var openItem by remember { mutableStateOf<String?>(null) }
    var tracks by remember { mutableStateOf<List<uniffi.comrade_ui.StreamCandidateDto>>(emptyList()) }

    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        BrowserHeader(stringResource(R.string.together_source_collections), onBack)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            TextButton(onClick = { tab = 0; note = null }) {
                Text(
                    stringResource(R.string.collections_tab_archive),
                    color = if (tab == 0) TogetherText else TogetherMuted,
                    fontWeight = if (tab == 0) FontWeight.SemiBold else FontWeight.Normal,
                )
            }
            TextButton(onClick = { tab = 1; note = null; items = emptyList() }) {
                Text(
                    stringResource(R.string.collections_tab_podcasts),
                    color = if (tab == 1) TogetherText else TogetherMuted,
                    fontWeight = if (tab == 1) FontWeight.SemiBold else FontWeight.Normal,
                )
            }
        }

        OutlinedTextField(
            value = query,
            onValueChange = { query = it },
            singleLine = true,
            leadingIcon = { Icon(Icons.Filled.Search, contentDescription = null) },
            placeholder = {
                Text(
                    stringResource(
                        if (tab == 0) R.string.collections_archive_hint else R.string.collections_feed_hint,
                    ),
                )
            },
            modifier = Modifier.fillMaxWidth(),
        )

        when (tab) {
            0 -> {
                Button(
                    enabled = query.isNotBlank() && !searching,
                    onClick = {
                        searching = true
                        scope.launch {
                            val result = withContext(Dispatchers.IO) { ComradeCore.archiveSearch(query) }
                            items = (result as? ComradeCore.CollectionResult.Found)?.items.orEmpty()
                            note = when (result) {
                                is ComradeCore.CollectionResult.Failed -> result.reason
                                is ComradeCore.CollectionResult.CannotSearch ->
                                    context.getString(R.string.together_server_cannot_stream)
                                else -> null
                            }
                            searching = false
                        }
                    },
                ) { Text(stringResource(R.string.together_search_go)) }
                items.forEach { item ->
                    SourceCard(
                        icon = QueueMusicIcon,
                        title = item.title,
                        subtitle = item.creator.ifBlank { item.identifier },
                    ) {
                        searching = true
                        scope.launch {
                            val result = withContext(Dispatchers.IO) { ComradeCore.archiveTracks(item.identifier) }
                            tracks = (result as? ComradeCore.TrackListResult.Found)?.candidates.orEmpty()
                            note = (result as? ComradeCore.TrackListResult.Failed)?.reason
                            searching = false
                            openItem = item.identifier
                        }
                    }
                }
            }
            else -> {
                Button(
                    enabled = query.isNotBlank() && !searching,
                    onClick = {
                        searching = true
                        scope.launch {
                            val result = withContext(Dispatchers.IO) { ComradeCore.podcastEpisodes(query.trim()) }
                            tracks = (result as? ComradeCore.TrackListResult.Found)?.candidates.orEmpty()
                            note = when (result) {
                                is ComradeCore.TrackListResult.Refused -> result.reason
                                is ComradeCore.TrackListResult.Failed -> result.reason
                                is ComradeCore.TrackListResult.CannotSearch ->
                                    context.getString(R.string.together_server_cannot_stream)
                                else -> null
                            }
                            searching = false
                        }
                    },
                ) { Text(stringResource(R.string.collections_open)) }
                if (openItem == null || tab == 1) {
                    tracks.forEach { candidate -> ServerRow(candidate = candidate) { onPlay(candidate) } }
                }
            }
        }

        openItem?.let { id ->
            if (tab == 0) {
                BrowserHeader(
                    items.firstOrNull { it.identifier == id }?.title ?: id,
                    onBack = { openItem = null },
                )
                tracks.forEach { candidate -> ServerRow(candidate = candidate) { onPlay(candidate) } }
            }
        }

        if (searching) {
            Text(stringResource(R.string.together_search_running), color = TogetherMuted)
        }
        note?.let { Text(it, color = MaterialTheme.colorScheme.error) }
    }
}
