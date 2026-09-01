package mullu.comrade.ui

import android.graphics.BitmapFactory
import android.util.Base64
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.AssistChip
import androidx.compose.material3.AssistChipDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LargeTopAppBar
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Tab
import androidx.compose.material3.TabRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.rememberTopAppBarState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.draw.clip
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import mullu.comrade.ComradeCore
import mullu.comrade.MutedChats
import mullu.comrade.Notifier
import mullu.comrade.R
import mullu.comrade.ui.theme.ComradeRadii
import mullu.comrade.ui.theme.GlassElevation
import mullu.comrade.ui.theme.Spacing
import mullu.comrade.ui.theme.glassSurface
import mullu.comrade.ui.theme.comradeSkeleton
import mullu.comrade.ui.theme.glassTopAppBarColors

/**
 * The profile page: a collapsing header, who this person is, and what has been
 * exchanged with them.
 *
 * Every decision on this screen comes from [ProfileView.kt][infoRows] — which
 * rows exist, which actions are offered, which tab opens, how fast the avatar
 * shrinks — and is pinned by `ProfileViewTest` and its desktop and Dart mirrors.
 * What is here is only Compose. The one thing this screen decides for itself is
 * the *wording*, which lives in `strings.xml`.
 *
 * Two things it deliberately does not do:
 *
 *  - **It never fetches anything.** [ComradeCore.peerAvatarTyped] reads the
 *    encrypted store; whether a picture was ever allowed to be fetched was
 *    decided in `comrade_core::avatar`, for every caller at once. Opening a
 *    stranger's profile cannot make this device call a host they chose.
 *  - **It never opens a link.** The Links tab copies, and draws the *host*
 *    large, because `https://evil.example/login?next=paypal.com` must not be
 *    presentable as a PayPal link (D38).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ProfileScreen(
    /** The peer to draw, or `null` for your own profile. */
    target: String?,
    ownProfile: ComradeCore.Profile,
    onOwnProfileChange: (ComradeCore.Profile) -> Unit,
    onBack: () -> Unit,
    onMessage: (peer: String, alias: String?, username: String?) -> Unit,
    onCall: (peer: String, label: String) -> Unit,
    /** Called after a successful block, so the caller can leave and refresh. */
    onBlocked: (peer: String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    val clipboard = LocalClipboardManager.current
    val scope = rememberCoroutineScope()
    val isSelf = target == null

    var reloadTick by remember { mutableIntStateOf(0) }
    var peer by remember(target) { mutableStateOf<ComradeCore.PeerProfileInfo?>(null) }
    // Starts true for your own page, whose *body* needs no read — the rows come
    // from `ownProfile`, and only the avatar is fetched from the store. For a
    // peer it gates the body, because an unread profile and a stranger's are
    // the same empty record, and drawing the second while waiting for the first
    // flashes "Add contact" at an old friend.
    var loadedOnce by remember(target) { mutableStateOf(isSelf) }
    var loadFailed by remember(target) { mutableStateOf(false) }
    var avatar by remember(target) { mutableStateOf<ImageBitmap?>(null) }
    var media by remember(target) { mutableStateOf<List<ComradeCore.MediaMessageInfo>>(emptyList()) }
    var links by remember(target) { mutableStateOf<List<SharedLink>>(emptyList()) }
    // Null until the history has been read: which tab opens is `initialMediaTab`'s
    // answer, and it cannot be asked before there is a history to ask about.
    var tab by remember(target) { mutableStateOf<SharedTab?>(null) }
    var muted by remember(target) {
        mutableStateOf(target?.let { MutedChats.isMuted(context, it) } == true)
    }
    var editingBio by remember { mutableStateOf(false) }
    var editingHandle by remember { mutableStateOf(false) }
    // A rejected handle is the common case, not an edge one — the core enforces
    // 3–24 characters — so the dialog stays open and says why, the way the
    // Settings editor already does. Closing on failure would look like a save.
    var saving by remember { mutableStateOf(false) }
    var editError by remember { mutableStateOf<String?>(null) }
    var confirmBlock by remember { mutableStateOf(false) }

    LaunchedEffect(target, reloadTick) {
        val npub = target ?: ownProfile.npub
        val loaded = withContext(Dispatchers.IO) {
            val profile = target?.let { runCatching { ComradeCore.peerProfileTyped(it) }.getOrNull() }
            val cached = if (target == null) ownProfile.avatarCached else profile?.avatarCached == true
            // `mayFetchAvatar` gates the *drawing*, not only the fetching, and
            // this is its first caller on any frontend. Turning the switch off
            // does not purge what is already in the store — `set_remote_avatars_
            // enabled` writes a flag and nothing else — so a page that asked
            // only "are there bytes?" would keep showing a peer-chosen picture
            // after the user turned pictures off, which is what the Settings
            // copy promises it will not do. The contact/blocked halves matter
            // for the same reason: bytes cached while someone was a contact
            // outlive their being one.
            val allowed = cached && mayFetchAvatar(
                url = if (target == null) ownProfile.picture else profile?.picture,
                remoteAvatarsEnabled = runCatching { ComradeCore.remoteAvatarsEnabledTyped() }
                    .getOrDefault(true),
                isContact = profile?.contact == true,
                isSelf = target == null,
                isBlocked = profile?.blocked == true,
            )
            val bytes = if (allowed) {
                runCatching { ComradeCore.peerAvatarTyped(npub) }.getOrNull()
            } else {
                null
            }
            val history = target?.let { runCatching { ComradeCore.media(it) }.getOrDefault(emptyList()) }
                ?: emptyList()
            val bodies = target?.let { runCatching { ComradeCore.messages(it) }.getOrDefault(emptyList()) }
                ?: emptyList()
            Loaded(
                profile = profile,
                avatar = bytes?.let(::decodeAvatar),
                media = history,
                links = collectLinks(
                    bodies.map { LinkMessage(it.content, it.createdAt, it.outgoing) },
                ),
            )
        }
        peer = loaded.profile
        loadFailed = target != null && loaded.profile == null
        avatar = loaded.avatar
        media = loaded.media
        links = loaded.links
        if (tab == null) {
            tab = SharedTab.of(initialMediaTab(loaded.media.map { it.mimeType }))
        }
        loadedOnce = true
    }

    val title = when {
        isSelf -> handleOf(ownProfile.username).ifEmpty { shortNpub(ownProfile.npub) }
        else -> peer?.let { peerTitle(it.npub, it.alias, it.name) } ?: shortNpub(target.orEmpty())
    }
    val scrollBehavior = TopAppBarDefaults.exitUntilCollapsedScrollBehavior(rememberTopAppBarState())

    Scaffold(
        modifier = modifier
            .fillMaxSize()
            .testTag("profile-screen")
            .nestedScroll(scrollBehavior.nestedScrollConnection),
        topBar = {
            LargeTopAppBar(
                modifier = Modifier.glassSurface(GlassElevation.Chrome),
                colors = glassTopAppBarColors(),
                scrollBehavior = scrollBehavior,
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(
                            Icons.AutoMirrored.Filled.ArrowBack,
                            contentDescription = "Back",
                        )
                    }
                },
                title = {
                    ProfileHeader(
                        title = title,
                        seed = target ?: ownProfile.npub,
                        avatar = avatar,
                        // The *curve* is shared with the desktop and Flutter
                        // headers; only the mechanism differs (a
                        // `LargeTopAppBar` state here, a scroll listener there).
                        avatarSize = collapsedAvatarSize(
                            fraction = scrollBehavior.state.collapsedFraction,
                            expandedPx = ExpandedAvatarDp,
                            collapsedPx = CollapsedAvatarDp,
                        ),
                        status = when {
                            isSelf -> stringResource(R.string.profile_self_status)
                            else -> peer?.let { statusLine(it) }.orEmpty()
                        },
                    )
                },
            )
        },
    ) { padding ->
        val fields = when {
            isSelf -> ProfileFields(
                npub = ownProfile.npub,
                name = ownProfile.username,
                about = ownProfile.about,
            )
            else -> peer?.let {
                ProfileFields(
                    npub = it.npub,
                    name = it.name,
                    about = it.about,
                    nip05 = it.nip05,
                    lud16 = it.lud16,
                )
            }
        }
        val rows = fields?.let { infoRows(it, isSelf = isSelf) } ?: emptyList()
        val actions = actionRow(
            isSelf = isSelf,
            isContact = peer?.contact == true,
            isComrade = peer?.comrade == true,
            isMuted = muted,
            isBlocked = peer?.blocked == true,
        )
        val counts = mediaTabCounts(media.map { it.mimeType })
        val buckets = bucketMedia(media) { it.mimeType }

        LazyColumn(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding),
        ) {
            // A profile that could not be read gets the sentence and nothing
            // else. The alternative — falling through to the default row —
            // offers Message and Add contact for someone we know nothing about,
            // which reads as a page that loaded rather than one that failed.
            if (loadFailed) {
                item("unavailable") {
                    Text(
                        stringResource(R.string.profile_unavailable),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(Spacing.space4),
                    )
                }
            } else if (!loadedOnce) {
                // §7.3: the row geometry, pulsing, rather than a spinner over a
                // blank page — the shape is known before the record arrives.
                item("skeleton") { ProfileBodySkeleton() }
            } else {
                item("actions") {
                    ProfileActionRow(
                        actions = actions,
                        onAction = { action ->
                            val npub = peer?.npub ?: target
                            when (action) {
                                ProfileAction.Message ->
                                    npub?.let { onMessage(it, peer?.alias, peer?.name) }
                                ProfileAction.Call -> npub?.let { onCall(it, title) }
                                ProfileAction.Mute, ProfileAction.Unmute -> npub?.let {
                                    MutedChats.setMuted(context, it, !muted)
                                    if (!muted) {
                                        // Muting with a notice already in the shade
                                        // would leave the buzz it was meant to stop
                                        // sitting there — the ⋮ menu's reasoning.
                                        Notifier.clearForPeer(context, it)
                                    }
                                    muted = !muted
                                }
                                ProfileAction.AddContact -> npub?.let {
                                    scope.launch {
                                        withContext(Dispatchers.IO) {
                                            runCatching { ComradeCore.addContactTyped(it, "") }
                                        }
                                        reloadTick++
                                    }
                                }
                                ProfileAction.AddComrade, ProfileAction.RemoveComrade -> npub?.let {
                                    val on = action == ProfileAction.AddComrade
                                    scope.launch {
                                        withContext(Dispatchers.IO) {
                                            runCatching { ComradeCore.setComradeTyped(it, on) }
                                        }
                                        reloadTick++
                                    }
                                }
                                ProfileAction.Block -> confirmBlock = true
                                ProfileAction.Edit -> editingHandle = true
                                ProfileAction.CopyKey -> {
                                    clipboard.setText(AnnotatedString(fields?.npub.orEmpty()))
                                }
                            }
                        },
                    )
                }

                // `actionRow` returns nothing for a blocked peer on purpose: there is
                // no unblock command in the core, so a button here would be a switch
                // that does nothing. The page states the fact instead (D36).
                if (actions.isEmpty() && peer?.blocked == true) {
                    item("blocked-note") {
                        Text(
                            stringResource(R.string.profile_blocked_note),
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.error,
                            modifier = Modifier.padding(horizontal = Spacing.space4, vertical = Spacing.space2),
                        )
                    }
                }

                items(rows, key = { it.kind.name }) { row ->
                    ProfileInfoRow(
                        row = row,
                        isSelf = isSelf,
                        // Android 13+ draws its own clipboard confirmation, so
                        // there is no toast here: it would say it twice.
                        onCopy = { clipboard.setText(AnnotatedString(row.value)) },
                        onEditBio = { editingBio = true },
                        onEditHandle = { editingHandle = true },
                    )
                }

                peer?.takeIf { it.updatedAt > 0L }?.let { known ->
                    item("updated") {
                        Text(
                            stringResource(R.string.profile_updated, relativeTime(known.updatedAt)),
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(horizontal = Spacing.space4, vertical = Spacing.space2),
                        )
                    }
                }

                // Your own page has no shared-media tabs: everything under them is
                // "what have we exchanged", and there is no we.
                if (!isSelf) {
                    item("tabs") {
                        SharedTabRow(
                            selected = tab ?: SharedTab.Media,
                            counts = counts,
                            linkCount = links.size,
                            onSelect = { tab = it },
                        )
                    }
                    val current = tab ?: SharedTab.Media
                    val mediaTab = current.mediaTab
                    if (mediaTab == null) {
                        if (links.isEmpty()) {
                            item("links-empty") { EmptyTabNote(current.emptyLabel) }
                        } else {
                            items(links, key = { it.url }) { link ->
                                SharedLinkRow(
                                    link = link,
                                    onCopy = { clipboard.setText(AnnotatedString(link.url)) },
                                )
                            }
                        }
                    } else {
                        val shown = buckets.getValue(mediaTab)
                        if (shown.isEmpty()) {
                            item("media-empty") { EmptyTabNote(current.emptyLabel) }
                        } else {
                            items(shown, key = { it.eventId }) { item -> SharedMediaRow(item) }
                        }
                    }
                }
            }
        }
    }

    if (editingBio) {
        ProfileTextDialog(
            title = stringResource(R.string.profile_bio_title),
            hint = stringResource(R.string.profile_bio_hint),
            initial = ownProfile.about.orEmpty(),
            busy = saving,
            error = editError,
            onDismiss = {
                editingBio = false
                editError = null
            },
            onSave = { next ->
                saving = true
                editError = null
                scope.launch {
                    runCatching {
                        withContext(Dispatchers.IO) { ComradeCore.setAboutTyped(next) }
                    }.onSuccess { saved ->
                        saving = false
                        editingBio = false
                        onOwnProfileChange(saved)
                        reloadTick++
                    }.onFailure {
                        saving = false
                        editError = it.message
                    }
                }
            },
        )
    }

    if (editingHandle) {
        ProfileTextDialog(
            title = stringResource(R.string.profile_handle_title),
            hint = stringResource(R.string.profile_handle_hint),
            initial = ownProfile.username.orEmpty(),
            busy = saving,
            error = editError,
            onDismiss = {
                editingHandle = false
                editError = null
            },
            onSave = { next ->
                saving = true
                editError = null
                scope.launch {
                    runCatching {
                        withContext(Dispatchers.IO) { ComradeCore.setUsernameTyped(next) }
                    }.onSuccess { saved ->
                        saving = false
                        editingHandle = false
                        onOwnProfileChange(saved)
                        reloadTick++
                    }.onFailure {
                        saving = false
                        editError = it.message
                    }
                }
            },
        )
    }

    if (confirmBlock) {
        val blocked = peer?.npub ?: target
        ProfileBlockDialog(
            onDismiss = { confirmBlock = false },
            onConfirm = {
                confirmBlock = false
                if (blocked != null) {
                    scope.launch {
                        val ok = withContext(Dispatchers.IO) {
                            runCatching { ComradeCore.blockConversationTyped(blocked) }.isSuccess
                        }
                        if (ok) {
                            Notifier.clearForPeer(context, blocked)
                            onBlocked(blocked)
                        }
                    }
                }
            },
        )
    }
}

/** What one load of this screen produced, so the IO block returns once. */
private data class Loaded(
    val profile: ComradeCore.PeerProfileInfo?,
    val avatar: ImageBitmap?,
    val media: List<ComradeCore.MediaMessageInfo>,
    val links: List<SharedLink>,
)

/** The expanded and collapsed avatar diameters the shared curve interpolates. */
private const val ExpandedAvatarDp = 72f
private const val CollapsedAvatarDp = 36f

/**
 * The four tabs of the shared block.
 *
 * [MediaTab] is the shared rule and has three cases; Links is a fourth tab with
 * no attachment behind it — it is scanned out of message *text*, because no DTO
 * carries links. Keeping it out of the shared enum is deliberate: `mediaTabFor`
 * classifies a MIME type, and a link has none.
 */
private enum class SharedTab(val mediaTab: MediaTab?, val label: Int, val emptyLabel: Int) {
    Media(MediaTab.Media, R.string.profile_tab_media, R.string.profile_empty_media),
    Files(MediaTab.Files, R.string.profile_tab_files, R.string.profile_empty_files),
    // Null, not a stand-in: `mediaTabFor` classifies a MIME type and a link has
    // none. The null is what makes the Links branch total rather than a special
    // case some later edit could forget.
    Links(null, R.string.profile_tab_links, R.string.profile_empty_links),
    Voice(MediaTab.Voice, R.string.profile_tab_voice, R.string.profile_empty_voice),
    ;

    companion object {
        fun of(tab: MediaTab): SharedTab = when (tab) {
            MediaTab.Media -> Media
            MediaTab.Files -> Files
            MediaTab.Voice -> Voice
        }
    }
}

/**
 * The line under the name. Delegates to [presenceText], the same function the
 * conversation header and the comrade list use, rather than inventing a second
 * "last seen" vocabulary. It says nothing for a non-comrade — fine on a header,
 * but a blank on a page *about* this person reads as missing, so contact status
 * answers instead.
 */
@Composable
private fun statusLine(peer: ComradeCore.PeerProfileInfo): String = when {
    peer.comrade -> presenceText(peer.online, peer.lastSeenAt, peer.peerMarkedUs)
    peer.contact -> stringResource(R.string.profile_status_contact)
    else -> stringResource(R.string.profile_status_stranger)
}

/** Avatar, name and status — the title slot of the collapsing bar. */
@Composable
private fun ProfileHeader(
    title: String,
    seed: String,
    avatar: ImageBitmap?,
    avatarSize: Float,
    status: String,
) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Spacing.space3),
    ) {
        if (avatar == null) {
            PeerAvatar(title, seed = seed, size = avatarSize.dp)
        } else {
            Image(
                bitmap = avatar,
                contentDescription = null,
                contentScale = ContentScale.Crop,
                modifier = Modifier
                    .size(avatarSize.dp)
                    .clip(CircleShape),
            )
        }
        Column {
            Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (status.isNotEmpty()) {
                Text(
                    status,
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}

/**
 * The action row, in the order [actionRow] gave — which is the order all three
 * frontends draw, so a muscle-memory tap does the same thing on each.
 */
@Composable
private fun ProfileActionRow(actions: List<ProfileAction>, onAction: (ProfileAction) -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .horizontalScroll(rememberScrollState())
            .padding(horizontal = Spacing.space4, vertical = Spacing.space2),
        horizontalArrangement = Arrangement.spacedBy(Spacing.space2),
    ) {
        actions.forEach { action ->
            AssistChip(
                onClick = { onAction(action) },
                label = { Text(stringResource(action.labelRes)) },
                // Block is the only action here that silently changes what
                // reaches you, so it is the only one that earns the error
                // colour — `ChatMenuAction.destructive`'s reasoning, and
                // spending that emphasis elsewhere would teach people to
                // ignore it.
                colors = if (action == ProfileAction.Block) {
                    AssistChipDefaults.assistChipColors(
                        labelColor = MaterialTheme.colorScheme.error,
                    )
                } else {
                    AssistChipDefaults.assistChipColors()
                },
            )
        }
    }
}

/** The wording for one action; the rule that chose it is [actionRow]. */
private val ProfileAction.labelRes: Int
    get() = when (this) {
        ProfileAction.Message -> R.string.profile_action_message
        ProfileAction.Call -> R.string.profile_action_call
        ProfileAction.Mute -> R.string.profile_action_mute
        ProfileAction.Unmute -> R.string.profile_action_unmute
        ProfileAction.AddContact -> R.string.profile_action_add_contact
        ProfileAction.AddComrade -> R.string.profile_action_add_comrade
        ProfileAction.RemoveComrade -> R.string.profile_action_remove_comrade
        ProfileAction.Block -> R.string.profile_action_block
        ProfileAction.Edit -> R.string.profile_action_edit
        ProfileAction.CopyKey -> R.string.profile_action_copy_key
    }

/**
 * One row of the info block: a label, the value, and what can be done with it.
 *
 * The key row is monospace and selectable, never truncated — a shortened key is
 * not a key, and this page is the place D35 moved it to.
 */
@Composable
private fun ProfileInfoRow(
    row: ProfileRow,
    isSelf: Boolean,
    onCopy: () -> Unit,
    onEditBio: () -> Unit,
    onEditHandle: () -> Unit,
) {
    val label = stringResource(
        when (row.kind) {
            ProfileRowKind.Bio -> R.string.profile_row_bio
            ProfileRowKind.Handle -> R.string.profile_row_handle
            ProfileRowKind.Nip05 -> R.string.profile_row_nip05
            ProfileRowKind.Lud16 -> R.string.profile_row_lud16
            ProfileRowKind.Key -> R.string.profile_row_key
        },
    )
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = Spacing.space4, vertical = Spacing.space2),
    ) {
        Text(
            label,
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(Spacing.space2),
        ) {
            if (row.value.isEmpty()) {
                Text(
                    stringResource(R.string.profile_row_unset),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.weight(1f),
                )
            } else {
                SelectionContainer(modifier = Modifier.weight(1f)) {
                    Text(
                        row.value,
                        style = MaterialTheme.typography.bodyMedium,
                        fontFamily = if (row.kind == ProfileRowKind.Key) FontFamily.Monospace else null,
                    )
                }
            }
            if (row.copyable && row.value.isNotEmpty()) {
                TextButton(onClick = onCopy) {
                    Text(stringResource(R.string.profile_copy))
                }
            }
            if (isSelf && row.kind == ProfileRowKind.Bio) {
                TextButton(onClick = onEditBio) {
                    Text(
                        stringResource(
                            if (row.value.isEmpty()) R.string.profile_add else R.string.profile_edit,
                        ),
                    )
                }
            }
            if (isSelf && row.kind == ProfileRowKind.Handle) {
                TextButton(onClick = onEditHandle) { Text(stringResource(R.string.profile_edit)) }
            }
        }
    }
}

/** The tab strip, with the count each tab would show. */
@Composable
private fun SharedTabRow(
    selected: SharedTab,
    counts: Map<MediaTab, Int>,
    linkCount: Int,
    onSelect: (SharedTab) -> Unit,
) {
    TabRow(selectedTabIndex = selected.ordinal) {
        SharedTab.entries.forEach { entry ->
            val n = entry.mediaTab?.let { counts[it] ?: 0 } ?: linkCount
            Tab(
                selected = entry == selected,
                onClick = { onSelect(entry) },
                text = {
                    Text(
                        stringResource(R.string.profile_tab_count, stringResource(entry.label), n),
                        maxLines = 1,
                    )
                },
            )
        }
    }
}

/**
 * The body's silhouette while the cached record is read: a row of action chips,
 * then two info rows. §7.3's rule — the real geometry, pulsing.
 */
@Composable
private fun ProfileBodySkeleton() {
    val pill = RoundedCornerShape(ComradeRadii.sm)
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = Spacing.space4, vertical = Spacing.space2),
        verticalArrangement = Arrangement.spacedBy(Spacing.space3),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(Spacing.space2)) {
            repeat(3) {
                Box(
                    Modifier
                        .size(width = 88.dp, height = 32.dp)
                        .comradeSkeleton(RoundedCornerShape(ComradeRadii.xl)),
                )
            }
        }
        repeat(2) {
            Column(verticalArrangement = Arrangement.spacedBy(Spacing.space1)) {
                Box(Modifier.fillMaxWidth(0.25f).height(12.dp).comradeSkeleton(pill))
                Box(Modifier.fillMaxWidth(0.7f).height(16.dp).comradeSkeleton(pill))
            }
        }
    }
}

@Composable
private fun EmptyTabNote(label: Int) {
    Box(
        modifier = Modifier
            .fillMaxWidth()
            .padding(Spacing.space8),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            stringResource(label),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

/**
 * One row of a shared-media tab. Deliberately not a thumbnail grid (D39):
 * drawing one means downloading and decrypting every blob on the tab, and the
 * bytes would land in the in-memory cache sized for a conversation, not for a
 * back-catalogue. The bubble in the thread already loads on demand.
 */
@Composable
private fun SharedMediaRow(item: ComradeCore.MediaMessageInfo) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = Spacing.space4, vertical = Spacing.space3),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Spacing.space3),
    ) {
        Text(mediaKindGlyph(item.mimeType), style = MaterialTheme.typography.titleMedium)
        Column(Modifier.weight(1f)) {
            Text(
                // A peer chose this caption, and it is drawn at body size beside
                // their name — the same threat a transfer card's filename has.
                sanitizeDisplayText(item.caption, MAX_HANDLE_CHARS).ifEmpty {
                    stringResource(if (item.outgoing) R.string.profile_sent else R.string.profile_received)
                },
                style = MaterialTheme.typography.bodyMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                "${mediaKindLabel(item.mimeType)} · ${formatAttachmentSize(item.size)} · " +
                    relativeTime(item.createdAt),
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

/**
 * One row of the Links tab: the host large, the URL small, and a copy button.
 *
 * Nothing opens on a tap, on purpose. Fetching a sender-chosen URL leaks this
 * device's IP and an implicit read receipt to whoever sent it, and the host has
 * to be the prominent field or a URL can dress itself up as another site (D38).
 */
@Composable
private fun SharedLinkRow(link: SharedLink, onCopy: () -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable { onCopy() }
            .padding(horizontal = Spacing.space4, vertical = Spacing.space3),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Spacing.space3),
    ) {
        Icon(
            LinkIcon,
            contentDescription = stringResource(R.string.profile_link_copy),
            tint = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Column(Modifier.weight(1f)) {
            Text(
                link.host,
                style = MaterialTheme.typography.bodyMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                link.url,
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Text(
            relativeTime(link.at),
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

/** The one-field editor both self-edit rows use. */
@Composable
private fun ProfileTextDialog(
    title: String,
    hint: String,
    initial: String,
    busy: Boolean,
    error: String?,
    onDismiss: () -> Unit,
    onSave: (String) -> Unit,
) {
    var value by remember(initial) { mutableStateOf(initial) }
    val dialogShape = RoundedCornerShape(ComradeRadii.xl)
    AlertDialog(
        onDismissRequest = onDismiss,
        modifier = Modifier.glassSurface(GlassElevation.Sheet, shape = dialogShape),
        shape = dialogShape,
        containerColor = Color.Transparent,
        title = { Text(title) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(Spacing.space2)) {
                OutlinedTextField(
                    value = value,
                    onValueChange = { value = it },
                    singleLine = false,
                    modifier = Modifier.fillMaxWidth(),
                )
                Text(
                    hint,
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                error?.let {
                    Text(
                        it,
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                }
            }
        },
        confirmButton = {
            TextButton(enabled = !busy, onClick = { onSave(value) }) {
                Text(stringResource(R.string.profile_save))
            }
        },
        dismissButton = {
            TextButton(enabled = !busy, onClick = onDismiss) { Text(stringResource(R.string.cancel)) }
        },
    )
}

/** The block confirmation, worded exactly as the ⋮ menu's is. */
@Composable
private fun ProfileBlockDialog(onDismiss: () -> Unit, onConfirm: () -> Unit) {
    val dialogShape = RoundedCornerShape(ComradeRadii.xl)
    AlertDialog(
        onDismissRequest = onDismiss,
        modifier = Modifier.glassSurface(GlassElevation.Sheet, shape = dialogShape),
        shape = dialogShape,
        containerColor = Color.Transparent,
        title = { Text(stringResource(R.string.block_title)) },
        text = { Text(stringResource(R.string.block_body)) },
        confirmButton = {
            TextButton(onClick = onConfirm) {
                Text(stringResource(R.string.block_confirm), color = MaterialTheme.colorScheme.error)
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) }
        },
    )
}

/**
 * Cached avatar bytes to a bitmap, or null.
 *
 * A picture that will not decode is cosmetic: the header falls back to the
 * generated initial rather than reporting anything, because there is nothing the
 * user could do and nothing is broken.
 */
private fun decodeAvatar(bytes: ComradeCore.MediaBytesInfo): ImageBitmap? = runCatching {
    val raw = Base64.decode(bytes.base64, Base64.DEFAULT)
    BitmapFactory.decodeByteArray(raw, 0, raw.size)?.asImageBitmap()
}.getOrNull()
