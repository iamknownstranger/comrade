package mullu.comrade.ui

/**
 * The rules a profile page has to agree on across every frontend, kept pure so
 * they can be tested once and mirrored exactly.
 *
 * The Dart port is `app/lib/src/util/profile_view.dart` and the desktop one is
 * `desktop/ui/profile_view.mjs`; all three are pinned by mirrored tests, so a
 * change here is a change in three places.
 *
 * What is *not* here: any wording. These functions return row and action kinds,
 * never labels, for the same reason [PresenceLabel] does — the rule is shared,
 * the strings live in `strings.xml` where they can be translated.
 */

/** The largest profile picture that will be accepted — `MAX_AVATAR_BYTES` in `comrade_core::avatar`. */
const val MAX_AVATAR_BYTES = 256L * 1024

/** The longest bio this UI will draw, matching `MAX_ABOUT_LEN` in the core. */
const val MAX_ABOUT_CHARS = 512

/** The longest published handle this UI will draw on one row. */
const val MAX_HANDLE_CHARS = 64

/**
 * The image types a profile picture may be, mirroring the allowlist the core
 * enforces on fetch.
 *
 * SVG is absent deliberately and permanently — it is a document that can carry
 * script and fetch remote resources, not a picture.
 */
val AVATAR_MIME_ALLOWLIST = listOf("image/jpeg", "image/png", "image/webp")

/** Which kind of row this is; the caller resolves the label. */
enum class ProfileRowKind { Bio, Handle, Nip05, Lud16, Key }

/** One row of the info block. */
data class ProfileRow(
    val kind: ProfileRowKind,
    val value: String,
    val copyable: Boolean,
)

/** What the action row offers; the caller resolves labels and icons. */
enum class ProfileAction {
    Message,
    Call,
    Mute,
    Unmute,
    AddContact,
    AddComrade,
    RemoveComrade,
    Block,
    Edit,
    CopyKey,
}

/** Which shared-media tab an item belongs to. */
enum class MediaTab { Media, Voice, Files }

/**
 * The fields a profile page reads, whether it came from `ProfileDto` (your own)
 * or `PeerProfileDto` (someone else's).
 *
 * A single shape so [infoRows] has one signature rather than two near-identical
 * ones — the two DTOs differ in which fields they carry, not in what a row means.
 */
data class ProfileFields(
    val npub: String,
    val name: String? = null,
    val about: String? = null,
    val nip05: String? = null,
    val lud16: String? = null,
)

/**
 * The rows of the info block, in order.
 *
 * Empty values are dropped — a blank "Bio" row states that we know the person
 * has no bio, which is not the same as not having fetched one. Every peer-chosen
 * value goes through [sanitizeDisplayText] on the way, because these are drawn at
 * heading size next to an avatar: the same threat a transfer card's filename has,
 * and the same answer.
 *
 * The key row is the exception: it is **never** dropped, and it is last. The
 * reasoning is [peerTitle]'s — a self-declared handle shown without the key
 * reachable "is the exact shape of an impersonation" — and the owner call of
 * 2026-07-30 moved the key out of the conversation header on the grounds that it
 * was reachable on demand one tap away. This page *is* that place, so the key is
 * not optional here: every other row is a claim the person made about
 * themselves, and this is the one row that is a fact.
 */
fun infoRows(fields: ProfileFields, isSelf: Boolean = false): List<ProfileRow> {
    val rows = mutableListOf<ProfileRow>()
    fun push(kind: ProfileRowKind, raw: String?, max: Int) {
        val clean = sanitizeDisplayText(raw, max)
        if (clean.isNotEmpty()) rows.add(ProfileRow(kind, clean, kind != ProfileRowKind.Bio))
    }
    push(ProfileRowKind.Bio, fields.about, MAX_ABOUT_CHARS)
    push(ProfileRowKind.Handle, handleOf(fields.name), MAX_HANDLE_CHARS)
    push(ProfileRowKind.Nip05, fields.nip05, MAX_HANDLE_CHARS)
    push(ProfileRowKind.Lud16, fields.lud16, MAX_HANDLE_CHARS)
    // Never conditional, and always last: the long monospace string is the least
    // scannable row and the one nobody reads unless they came for it. Not
    // sanitized and not truncated — it is bech32 from our own parser, and a
    // shortened key is not a key.
    rows.add(ProfileRow(ProfileRowKind.Key, fields.npub.trim(), copyable = true))
    // Your own empty bio still gets a row, because on your own page an empty row
    // is the affordance to fill it in. A peer's does not: there is nothing to act
    // on, and a blank row would read as a bio that says nothing.
    if (isSelf && rows.none { it.kind == ProfileRowKind.Bio }) {
        rows.add(0, ProfileRow(ProfileRowKind.Bio, "", copyable = false))
    }
    return rows
}

/**
 * The published handle with at most one leading `@`.
 *
 * [peerTitle] already owns the display-precedence rule (alias → handle → key);
 * this is only the prefix normalisation, so `name`, `@name` and `@@name` render
 * identically instead of three ways.
 */
fun handleOf(name: String?): String {
    val bare = (name ?: "").trim().trimStart('@')
    return if (bare.isEmpty()) "" else "@$bare"
}

/**
 * Which of the shared-media tabs an item belongs to.
 *
 * Delegates the MIME reading to [attachmentPreviewKind] rather than repeating it,
 * so a bubble, a preview sheet and a profile tab can never disagree about what a
 * file is. Photos and videos share one tab because that is the grid people scan
 * visually; a voice note has nothing to show and belongs in a list.
 */
fun mediaTabFor(mimeType: String): MediaTab =
    when (attachmentPreviewKind(mimeType)) {
        AttachmentPreviewKind.Image, AttachmentPreviewKind.Video -> MediaTab.Media
        AttachmentPreviewKind.Audio -> MediaTab.Voice
        AttachmentPreviewKind.File -> MediaTab.Files
    }

/**
 * Split a peer's media history into the three tabs, newest first within each.
 *
 * [ComradeCore.media] hands back the merged both-directions history oldest
 * first. A profile's tabs are a "what have we exchanged" view, where the recent
 * end is what anyone is looking for, so this reverses — once, here, rather than
 * in every renderer.
 *
 * Nothing is ever dropped: an unrecognised MIME type lands in Files, because a
 * bucket that silently swallows an attachment makes it unreachable.
 *
 * Generic over the row type, with [mimeOf] doing the reading, so the rule can be
 * tested without a `MediaMessageInfo` and the screen can keep drawing the real
 * one — the alternative is a second shape that exists only to be mapped into.
 */
fun <T> bucketMedia(items: List<T>, mimeOf: (T) -> String): Map<MediaTab, List<T>> {
    val buckets = MediaTab.entries.associateWith { mutableListOf<T>() }
    for (item in items) buckets.getValue(mediaTabFor(mimeOf(item))).add(item)
    return buckets.mapValues { (_, rows) -> rows.asReversed().toList() }
}

/** How many items each tab would show — what the tab strip's counts render. */
fun mediaTabCounts(mimeTypes: List<String>): Map<MediaTab, Int> {
    val counts = MediaTab.entries.associateWith { 0 }.toMutableMap()
    for (mime in mimeTypes) {
        val tab = mediaTabFor(mime)
        counts[tab] = counts.getValue(tab) + 1
    }
    return counts
}

/** One link found in a message body: the URL, and its host as a separate field. */
data class ProfileLink(val url: String, val host: String)

/**
 * The http(s) URLs in a message body, in the order they appear, de-duplicated.
 *
 * Deliberately **not** a regex for the matching. Three languages means three
 * regex dialects, and a link scanner is exactly the rule that drifts invisibly
 * when each port writes its own pattern — so this is whitespace splitting plus a
 * scheme test, which mirrors identically from `desktop/ui/profile_view.mjs`.
 *
 * Only `http://` and `https://`. `javascript:`, `data:`, `file:` and
 * scheme-relative `//host` are refused *here*, so no frontend has to remember.
 *
 * [ProfileLink.host] is returned **separately** and callers must render it as
 * the prominent part: `https://evil.example/login?next=paypal.com` must not be
 * presentable as a PayPal link.
 */
fun extractLinks(text: String?): List<ProfileLink> {
    val found = mutableListOf<ProfileLink>()
    val seen = mutableSetOf<String>()
    // Sanitize first: a bidi override inside a URL can reorder a displayed host.
    for (token in sanitizeDisplayText(text, 0).split(" ")) {
        val url = trimUrlPunctuation(token)
        if (url.isEmpty()) continue
        val lower = url.lowercase()
        if (!lower.startsWith("https://") && !lower.startsWith("http://")) continue
        val host = hostOf(url)
        // A scheme with no host is not a link.
        if (host.isEmpty()) continue
        if (!seen.add(url)) continue
        found.add(ProfileLink(url, host))
    }
    return found
}

/**
 * The host of an http(s) URL, lowercased, without userinfo or port.
 *
 * Hand-parsed rather than through `java.net.URI`, because the desktop and Dart
 * ports must agree with it character for character and three platform URL
 * parsers do not. Userinfo is dropped rather than shown:
 * `https://paypal.com@evil.example/` has a host of `evil.example`, and rendering
 * the part before the `@` is the oldest phishing trick there is.
 */
fun hostOf(url: String?): String {
    val raw = url ?: ""
    val marker = raw.indexOf("://")
    // No scheme, no answer — the desktop original sliced at the index anyway
    // and turned `example.com/x` into `ample.com`. A wrong host is the one
    // output this function must never produce, so both ports refuse instead.
    if (marker == -1) return ""
    val afterScheme = raw.substring(marker + 3)
    val authority = afterScheme.takeWhile { it != '/' && it != '?' && it != '#' }
    val afterUserinfo = if (authority.contains('@')) {
        authority.substring(authority.lastIndexOf('@') + 1)
    } else {
        authority
    }
    // An IPv6 literal keeps its brackets; anything else loses a :port.
    if (afterUserinfo.startsWith("[")) {
        val close = afterUserinfo.indexOf(']')
        return if (close == -1) "" else afterUserinfo.substring(0, close + 1).lowercase()
    }
    return afterUserinfo.substringBefore(':').lowercase()
}

/** Strip the punctuation a URL collects from the prose around it. */
private fun trimUrlPunctuation(token: String): String {
    var url = token
    while (url.isNotEmpty() && url.first() in "(<[\"'") url = url.substring(1)
    while (true) {
        val last = url.lastOrNull() ?: break
        if (last !in ".,;:!?)]}>\"'") break
        // Keep a closing bracket the URL itself opened — Wikipedia links carry
        // balanced parens.
        if (last == ')' && url.count { it == '(' } > url.count { it == ')' } - 1) break
        if (last == ']' && url.count { it == '[' } > url.count { it == ']' } - 1) break
        url = url.dropLast(1)
    }
    return url
}

/**
 * The fields the Links tab reads off a message, so [collectLinks] has one
 * signature rather than a lambda per field — [ProfileFields]'s reasoning.
 */
data class LinkMessage(val content: String?, val createdAt: Long, val outgoing: Boolean)

/** One link exchanged with a peer, with where it came from. */
data class SharedLink(
    val url: String,
    val host: String,
    val at: Long,
    val outgoing: Boolean,
)

/**
 * Every link exchanged with a peer, newest first — the Links tab.
 *
 * Sourced from message *text*, because no DTO carries links: a message has a
 * body and nothing else, so the alternative to scanning is not having the tab.
 *
 * A URL sent twice appears once, at its newest occurrence, because the tab
 * answers "where did that link go" and not "how often was it repeated".
 */
fun collectLinks(messages: List<LinkMessage>): List<SharedLink> {
    val out = mutableListOf<SharedLink>()
    val seen = mutableSetOf<String>()
    for (msg in messages.sortedByDescending { it.createdAt }) {
        for (link in extractLinks(msg.content)) {
            if (!seen.add(link.url)) continue
            out.add(SharedLink(link.url, link.host, msg.createdAt, msg.outgoing))
        }
    }
    return out
}

/**
 * The avatar's side length at a given header collapse fraction, where 0 is fully
 * expanded and 1 fully collapsed.
 *
 * Linear, and clamped at both ends so a platform that reports a fraction
 * slightly outside 0..1 during an overscroll (all three do, at some point)
 * cannot produce an avatar larger than the header or an inverted one.
 *
 * The three renderers are necessarily different — a Compose `LargeTopAppBar`
 * state, a Flutter `FlexibleSpaceBar`, a scroll listener over a CSS custom
 * property — so what is shared is the *curve*, which is the part a user would
 * notice drifting between platforms.
 */
fun collapsedAvatarSize(fraction: Float, expandedPx: Float, collapsedPx: Float): Float {
    val f = if (fraction.isNaN()) 0f else fraction.coerceIn(0f, 1f)
    return expandedPx + (collapsedPx - expandedPx) * f
}

/**
 * Which actions the row under the header offers, in order.
 *
 * Four rules carry weight:
 *
 * 1. **A blocked peer offers nothing at all** — not even Unblock. There is no
 *    unblock command in the core and no getter for the state to drive one, so a
 *    button here would be a fake switch, which is the one thing the settings
 *    screen's own rule forbids. The page says you blocked them and offers no lie
 *    about undoing it. When an unblock command exists, this is the function that
 *    changes, and its test is what will say so.
 * 2. **A stranger gets no Call button.** Placing a call makes this device gather
 *    ICE for whoever is on the other end — the same bar the accepted-conversation
 *    gate already holds an incoming call signal to, for the same reason. Offered
 *    before acceptance, its only outcomes are a leak or an error.
 * 3. **Mute is only meaningful for someone you hear from.** A stranger's messages
 *    are already gated behind a request.
 * 4. **Your own profile offers no Message and no Block.** Both are nonsense
 *    against yourself, and a Block that half-worked would be worse than absent.
 */
fun actionRow(
    isSelf: Boolean = false,
    isContact: Boolean = false,
    isComrade: Boolean = false,
    isMuted: Boolean = false,
    isBlocked: Boolean = false,
): List<ProfileAction> {
    if (isSelf) return listOf(ProfileAction.Edit, ProfileAction.CopyKey)
    if (isBlocked) return emptyList()
    val actions = mutableListOf(ProfileAction.Message)
    if (isContact) {
        actions.add(ProfileAction.Call)
        actions.add(if (isMuted) ProfileAction.Unmute else ProfileAction.Mute)
        actions.add(if (isComrade) ProfileAction.RemoveComrade else ProfileAction.AddComrade)
    } else {
        actions.add(ProfileAction.AddContact)
    }
    actions.add(ProfileAction.Block)
    return actions
}

/**
 * Which tab a profile should open on: the first non-empty in the canonical order
 * Media → Files → Voice, or Media when there is nothing at all.
 *
 * Opening on an empty Media tab makes a profile with plenty of files read as
 * "nothing shared", which is the failure this exists to prevent.
 */
fun initialMediaTab(mimeTypes: List<String>): MediaTab {
    val tabs = mimeTypes.map { mediaTabFor(it) }
    return when {
        tabs.contains(MediaTab.Media) -> MediaTab.Media
        tabs.contains(MediaTab.Files) -> MediaTab.Files
        tabs.contains(MediaTab.Voice) -> MediaTab.Voice
        else -> MediaTab.Media
    }
}

/** Why this image cannot be used as a profile picture, or null when it can. */
fun avatarRejection(mimeType: String?, bytes: Long): AvatarRefusal? {
    val mime = (mimeType ?: "").trim().lowercase()
    if (bytes <= 0L) return AvatarRefusal.Empty
    if (!AVATAR_MIME_ALLOWLIST.contains(mime)) return AvatarRefusal.WrongType
    if (bytes > MAX_AVATAR_BYTES) return AvatarRefusal.TooLarge
    return null
}

/** Why an avatar was refused; the caller resolves the wording. */
enum class AvatarRefusal { Empty, WrongType, TooLarge }

/**
 * Whether a peer's `picture` URL may be fetched at all.
 *
 * Two gates, and the caller must pass both: the user has not turned remote
 * pictures off, and the profile belongs to someone already accepted — so opening
 * a stranger's profile cannot make this device call out to a host they picked.
 *
 * Scheme, host and size are *not* checked here. They are enforced in the core,
 * where every caller gets them whether or not it remembered to ask. This is the
 * "should we ask at all", not the "is it safe".
 */
fun mayFetchAvatar(
    url: String?,
    remoteAvatarsEnabled: Boolean = true,
    isContact: Boolean = false,
    isSelf: Boolean = false,
    isBlocked: Boolean = false,
): Boolean {
    if ((url ?: "").isBlank()) return false
    if (!remoteAvatarsEnabled) return false
    if (isBlocked) return false
    return isSelf || isContact
}
