package mullu.comrade.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The Kotlin half of the mirrored profile-page suite. The same cases run in
 * `desktop/ui/profile_view.test.mjs` and `app/test/profile_view_test.dart`; a
 * change to one of the three rules has to change all three suites.
 *
 * The hostile characters are built with `Char(...)` rather than pasted, so they
 * are visible in a diff and cannot be silently stripped by an editor.
 */
class ProfileViewTest {
    private val npub = "npub1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq"

    /** U+202E RIGHT-TO-LEFT OVERRIDE — the one that makes gnp.exe read as .png. */
    private val rlo = Char(0x202E)

    private fun kinds(rows: List<ProfileRow>) = rows.map { it.kind }

    // ── The info block ───────────────────────────────────────────────────────

    @Test
    fun `the key row is present for every profile, however full or empty`() {
        // The trust invariant, mechanically enforced: a self-declared handle must
        // never be shown with the key unreachable.
        val full = infoRows(
            ProfileFields(
                npub = npub,
                name = "bob",
                about = "hi",
                nip05 = "bob@example.com",
                lud16 = "bob@wallet.example",
            ),
        )
        assertEquals(
            listOf(
                ProfileRowKind.Bio,
                ProfileRowKind.Handle,
                ProfileRowKind.Nip05,
                ProfileRowKind.Lud16,
                ProfileRowKind.Key,
            ),
            kinds(full),
        )

        assertEquals(listOf(ProfileRowKind.Key), kinds(infoRows(ProfileFields(npub = npub))))
        assertEquals(listOf(ProfileRowKind.Key), kinds(infoRows(ProfileFields(npub = ""))))
    }

    @Test
    fun `the key row is last, because it is the least scannable line on the page`() {
        val rows = infoRows(ProfileFields(npub = npub, name = "bob", about = "hi"))
        assertEquals(ProfileRowKind.Key, rows.last().kind)
    }

    @Test
    fun `a blank field produces no row rather than an empty one`() {
        // "We know they have no bio" and "we have not fetched a bio" are different
        // facts, and a blank row states the first when only the second is true.
        val rows = infoRows(ProfileFields(npub = npub, about = "   ", name = "", nip05 = null))
        assertEquals(listOf(ProfileRowKind.Key), kinds(rows))
    }

    @Test
    fun `your own empty bio still gets a row, because that row is how you fill it in`() {
        val mine = infoRows(ProfileFields(npub = npub), isSelf = true)
        assertEquals(listOf(ProfileRowKind.Bio, ProfileRowKind.Key), kinds(mine))
        assertEquals("", mine[0].value)
        assertEquals(listOf(ProfileRowKind.Key), kinds(infoRows(ProfileFields(npub = npub))))
    }

    @Test
    fun `a bio cannot smuggle control characters onto the page`() {
        val rows = infoRows(ProfileFields(npub = npub, about = "Trust me$rlo"))
        assertEquals("Trust me", rows[0].value)
    }

    @Test
    fun `a bio that is forty lines long is collapsed to one`() {
        val rows = infoRows(ProfileFields(npub = npub, about = "line\n".repeat(40)))
        assertEquals("line ".repeat(40).trim(), rows[0].value)
    }

    @Test
    fun `the key is never truncated, because a shortened key is not a key`() {
        val rows = infoRows(ProfileFields(npub = npub))
        assertEquals(npub, rows.last().value)
    }

    @Test
    fun `the bio is the one row that is not offered as copyable`() {
        val rows = infoRows(
            ProfileFields(
                npub = npub,
                about = "hi",
                name = "bob",
                nip05 = "b@e.com",
                lud16 = "b@w.example",
            ),
        )
        val copyable = rows.associate { it.kind to it.copyable }
        assertEquals(false, copyable[ProfileRowKind.Bio])
        assertEquals(true, copyable[ProfileRowKind.Handle])
        assertEquals(true, copyable[ProfileRowKind.Nip05])
        assertEquals(true, copyable[ProfileRowKind.Lud16])
        assertEquals(true, copyable[ProfileRowKind.Key])
    }

    // ── The handle ───────────────────────────────────────────────────────────

    @Test
    fun `a handle renders with exactly one at-sign, however many the peer published`() {
        assertEquals("@bob", handleOf("bob"))
        assertEquals("@bob", handleOf("@bob"))
        assertEquals("@bob", handleOf("@@@bob"))
        assertEquals("@bob", handleOf("  @bob  "))
    }

    @Test
    fun `a handle of nothing but at-signs is no handle at all`() {
        assertEquals("", handleOf("@"))
        assertEquals("", handleOf("@@@"))
        assertEquals("", handleOf(null))
        assertEquals("", handleOf(""))
    }

    // ── Shared-media buckets ─────────────────────────────────────────────────

    @Test
    fun `photos and videos share the grid, voice notes get their own list`() {
        assertEquals(MediaTab.Media, mediaTabFor("image/png"))
        assertEquals(MediaTab.Media, mediaTabFor("video/mp4"))
        assertEquals(MediaTab.Voice, mediaTabFor("audio/ogg"))
        assertEquals(MediaTab.Files, mediaTabFor("application/pdf"))
    }

    @Test
    fun `no mime type is ever dropped, because a dropped attachment is unreachable`() {
        for (mime in listOf("", "garbage", "/", "image", "IMAGE/PNG", "  image/png  ")) {
            // Every input must land in one of the three; the enum makes that
            // total, and this asserts the classifier does not throw on any of it.
            assertTrue(MediaTab.entries.contains(mediaTabFor(mime)))
        }
        assertEquals(MediaTab.Media, mediaTabFor("IMAGE/PNG"))
        assertEquals(MediaTab.Media, mediaTabFor("  image/png  "))
    }

    @Test
    fun `a profile opens on a tab that has something in it`() {
        // Opening on an empty Media tab makes a profile with plenty of files read
        // as "nothing shared", which is the failure this exists to prevent.
        assertEquals(MediaTab.Files, initialMediaTab(listOf("application/pdf")))
        assertEquals(MediaTab.Voice, initialMediaTab(listOf("audio/ogg")))
        assertEquals(MediaTab.Media, initialMediaTab(listOf("image/png", "audio/ogg")))
        assertEquals(MediaTab.Media, initialMediaTab(emptyList()))
    }

    @Test
    fun `a bucketed history reads newest first, though the store hands it over oldest first`() {
        val items = listOf("image/png" to "old", "image/png" to "new")
        assertEquals(
            // A profile's media tab is a what-have-we-exchanged view; the recent
            // end is the answer.
            listOf("new", "old"),
            bucketMedia(items) { it.first }.getValue(MediaTab.Media).map { it.second },
        )
    }

    @Test
    fun `counts and buckets agree, and an empty history counts zero everywhere`() {
        val items = listOf("image/png", "audio/ogg", "text/plain")
        assertEquals(
            mapOf(MediaTab.Media to 1, MediaTab.Voice to 1, MediaTab.Files to 1),
            mediaTabCounts(items),
        )
        assertEquals(
            mapOf(MediaTab.Media to 0, MediaTab.Voice to 0, MediaTab.Files to 0),
            mediaTabCounts(emptyList()),
        )
        for (tab in MediaTab.entries) {
            assertEquals(
                // Every tab is present in both, so a strip can render "Files 0"
                // rather than omitting a tab and moving the ones beside it.
                mediaTabCounts(items).getValue(tab),
                bucketMedia(items) { it }.getValue(tab).size,
            )
        }
    }

    // ── Links ────────────────────────────────────────────────────────────────

    @Test
    fun `only http and https are ever treated as links`() {
        val hostile =
            "javascript:alert(1) data:text/html,x file:///etc/passwd //evil.example ftp://x.example/a"
        assertEquals(emptyList<ProfileLink>(), extractLinks(hostile))
    }

    @Test
    fun `a link's host is returned separately so it can be the prominent part`() {
        val link = extractLinks("see https://evil.example/login?next=paypal.com").first()
        assertEquals("evil.example", link.host)
        assertEquals("https://evil.example/login?next=paypal.com", link.url)
    }

    @Test
    fun `userinfo never becomes the displayed host`() {
        // The oldest phishing trick there is: everything before the @ is not the host.
        assertEquals("evil.example", hostOf("https://paypal.com@evil.example/"))
        assertEquals("evil.example", hostOf("https://a@b@evil.example/x"))
    }

    @Test
    fun `a port is not part of the host, and an IPv6 literal keeps its brackets`() {
        assertEquals("example.com", hostOf("https://example.com:8443/x"))
        assertEquals("[2001:db8::1]", hostOf("https://[2001:db8::1]:8443/x"))
        assertEquals("[2001:db8::1]", hostOf("https://[2001:db8::1]/x"))
    }

    @Test
    fun `a host is lowercased, so two spellings of one host read as one host`() {
        assertEquals("example.com", hostOf("https://ExAmPlE.CoM/x"))
    }

    @Test
    fun `a string with no scheme has no host, rather than a mangled one`() {
        // Not reachable through `extractLinks`, which tests the scheme first —
        // but `hostOf` is public, and a function that answers `ample.com` for
        // `example.com/x` is a wrong host waiting for its first caller.
        assertEquals("", hostOf("example.com/x"))
        assertEquals("", hostOf(""))
        assertEquals("", hostOf(null))
    }

    @Test
    fun `a scheme with no host is not a link`() {
        assertEquals(emptyList<ProfileLink>(), extractLinks("https:// https://"))
    }

    @Test
    fun `trailing prose punctuation is not part of the URL`() {
        assertEquals("https://example.com/a", extractLinks("see https://example.com/a.").first().url)
        assertEquals("https://example.com/a", extractLinks("(https://example.com/a)").first().url)
        assertEquals("https://example.com/a", extractLinks("'https://example.com/a',").first().url)
    }

    @Test
    fun `a bracket the URL itself opened is kept`() {
        // Stripping it would break every Wikipedia link ever pasted.
        assertEquals(
            "https://en.example.org/wiki/Foo_(bar)",
            extractLinks("https://en.example.org/wiki/Foo_(bar)").first().url,
        )
    }

    @Test
    fun `a bidi override inside a URL cannot reorder the host on screen`() {
        val links = extractLinks("https://example.com/${rlo}gnp.exe")
        assertFalse(links.first().url.contains(rlo))
    }

    @Test
    fun `the same link twice is one link`() {
        assertEquals(1, extractLinks("https://example.com/a https://example.com/a").size)
    }

    @Test
    fun `links across a thread are newest first and de-duplicated across messages`() {
        val messages = listOf(
            LinkMessage("old https://example.com/a", 100L, outgoing = false),
            LinkMessage("new https://example.com/b", 300L, outgoing = true),
            LinkMessage("again https://example.com/a", 200L, outgoing = false),
        )
        val links = collectLinks(messages)
        assertEquals(
            listOf("https://example.com/b", "https://example.com/a"),
            links.map { it.url },
        )
        assertEquals(300L, links[0].at)
        assertTrue(links[0].outgoing)
        // The newest sighting of a repeated link is the one kept.
        assertEquals(200L, links[1].at)
    }

    @Test
    fun `collecting links tolerates a message with no body`() {
        assertEquals(emptyList<SharedLink>(), collectLinks(emptyList()))
        assertEquals(
            emptyList<SharedLink>(),
            collectLinks(listOf(LinkMessage(null, 0L, false), LinkMessage("no link here", 1L, false))),
        )
    }

    // ── The collapsing header ────────────────────────────────────────────────

    @Test
    fun `the avatar shrinks from expanded to collapsed across the fraction`() {
        assertEquals(96f, collapsedAvatarSize(0f, 96f, 36f), 0.001f)
        assertEquals(36f, collapsedAvatarSize(1f, 96f, 36f), 0.001f)
        assertEquals(66f, collapsedAvatarSize(0.5f, 96f, 36f), 0.001f)
    }

    @Test
    fun `an overscroll fraction outside the range cannot invert or oversize the avatar`() {
        // All three platforms report a fraction slightly outside 0..1 at some
        // point during an overscroll.
        assertEquals(96f, collapsedAvatarSize(-0.5f, 96f, 36f), 0.001f)
        assertEquals(36f, collapsedAvatarSize(1.5f, 96f, 36f), 0.001f)
        assertEquals(96f, collapsedAvatarSize(Float.NaN, 96f, 36f), 0.001f)
    }

    @Test
    fun `the avatar never grows while the header collapses`() {
        var previous = Float.MAX_VALUE
        for (i in 0..100) {
            val size = collapsedAvatarSize(i / 100f, 96f, 36f)
            assertTrue("size grew at fraction ${i / 100f}, which reads as a stutter", size <= previous)
            previous = size
        }
    }

    // ── The action row ───────────────────────────────────────────────────────

    @Test
    fun `a stranger is never offered a call`() {
        // Placing one makes this device gather ICE for whoever is on the other
        // end — the same bar the accepted-conversation gate holds a signal to.
        val actions = actionRow(isContact = false)
        assertFalse(actions.contains(ProfileAction.Call))
        assertEquals(
            listOf(ProfileAction.Message, ProfileAction.AddContact, ProfileAction.Block),
            actions,
        )
    }

    @Test
    fun `a contact gets a call, a mute and the comrade toggle`() {
        assertEquals(
            listOf(
                ProfileAction.Message,
                ProfileAction.Call,
                ProfileAction.Mute,
                ProfileAction.AddComrade,
                ProfileAction.Block,
            ),
            actionRow(isContact = true),
        )
        assertEquals(
            listOf(
                ProfileAction.Message,
                ProfileAction.Call,
                ProfileAction.Unmute,
                ProfileAction.RemoveComrade,
                ProfileAction.Block,
            ),
            actionRow(isContact = true, isMuted = true, isComrade = true),
        )
    }

    @Test
    fun `a blocked peer is offered nothing, because there is no unblock to offer`() {
        // `STATE_BLOCKED` is written in the runtime and read only by the incoming
        // gate; there is no unblock command and no getter to drive one. A button
        // here would be the fake switch the settings screen's own rule forbids.
        // When an unblock command exists, this is the test that says so by failing.
        assertEquals(emptyList<ProfileAction>(), actionRow(isBlocked = true))
        assertEquals(
            emptyList<ProfileAction>(),
            actionRow(isBlocked = true, isContact = true, isComrade = true),
        )
    }

    @Test
    fun `your own profile offers neither a message nor a block`() {
        val mine = actionRow(isSelf = true)
        assertEquals(listOf(ProfileAction.Edit, ProfileAction.CopyKey), mine)
        assertFalse(mine.contains(ProfileAction.Message))
        assertFalse(mine.contains(ProfileAction.Block))
    }

    @Test
    fun `self outranks every other flag, so no combination leaks a block onto your own page`() {
        assertEquals(
            listOf(ProfileAction.Edit, ProfileAction.CopyKey),
            actionRow(
                isSelf = true,
                isContact = true,
                isComrade = true,
                isMuted = true,
                isBlocked = true,
            ),
        )
    }

    @Test
    fun `no argument at all is a stranger, not a crash`() {
        assertEquals(
            listOf(ProfileAction.Message, ProfileAction.AddContact, ProfileAction.Block),
            actionRow(),
        )
    }

    // ── Publishing your own picture ──────────────────────────────────────────

    @Test
    fun `an avatar has to be one of the three formats the core will fetch back`() {
        assertNull(avatarRejection("image/png", 1024L))
        assertNull(avatarRejection("image/jpeg", 1024L))
        assertNull(avatarRejection("image/webp", 1024L))
        assertEquals(AvatarRefusal.WrongType, avatarRejection("image/svg+xml", 1024L))
        assertEquals(AvatarRefusal.WrongType, avatarRejection("image/gif", 1024L))
        assertEquals(AvatarRefusal.WrongType, avatarRejection("application/zip", 1024L))
    }

    @Test
    fun `SVG is refused, because it is a document that can fetch and script`() {
        assertFalse(AVATAR_MIME_ALLOWLIST.contains("image/svg+xml"))
    }

    @Test
    fun `an empty image is refused before its type is even considered`() {
        assertEquals(AvatarRefusal.Empty, avatarRejection("image/png", 0L))
        assertEquals(AvatarRefusal.Empty, avatarRejection("image/png", -1L))
    }

    @Test
    fun `an oversize image is refused and one exactly at the cap is not`() {
        assertEquals(AvatarRefusal.TooLarge, avatarRejection("image/png", MAX_AVATAR_BYTES + 1))
        assertNull(avatarRejection("image/png", MAX_AVATAR_BYTES))
    }

    @Test
    fun `the type is read case- and whitespace-insensitively`() {
        assertNull(avatarRejection("  IMAGE/PNG  ", 1024L))
    }

    // ── Whether to fetch a peer's picture at all ─────────────────────────────

    @Test
    fun `a stranger's picture URL is never fetched`() {
        // Opening a stranger's profile must not make this device call out to a
        // host they chose.
        assertFalse(mayFetchAvatar("https://x.example/a.png", isContact = false))
    }

    @Test
    fun `turning remote pictures off stops every fetch, contact or not`() {
        assertFalse(
            mayFetchAvatar("https://x.example/a.png", remoteAvatarsEnabled = false, isContact = true),
        )
        assertFalse(
            mayFetchAvatar("https://x.example/a.png", remoteAvatarsEnabled = false, isSelf = true),
        )
    }

    @Test
    fun `a blocked peer's picture is not fetched even if they are still a contact`() {
        assertFalse(mayFetchAvatar("https://x.example/a.png", isContact = true, isBlocked = true))
    }

    @Test
    fun `a contact with a picture is fetched, and so is your own`() {
        assertTrue(mayFetchAvatar("https://x.example/a.png", isContact = true))
        assertTrue(mayFetchAvatar("https://x.example/a.png", isSelf = true))
    }

    @Test
    fun `no URL means nothing to fetch`() {
        assertFalse(mayFetchAvatar("", isContact = true))
        assertFalse(mayFetchAvatar("   ", isContact = true))
        assertFalse(mayFetchAvatar(null, isContact = true))
    }

    // ── The shared text sanitiser ────────────────────────────────────────────

    @Test
    fun `a stripped control becomes a space, so two words never silently merge`() {
        // The failure this pins: \n is itself a control, so deleting outright
        // turns a two-line name into one word nobody is called.
        assertEquals("Bob Smith", sanitizeDisplayText("Bob\nSmith", 0))
        assertEquals("holiday gnp.exe", sanitizeDisplayText("holiday${rlo}gnp.exe", 0))
    }

    @Test
    fun `text longer than the bound is truncated with an ellipsis inside the bound`() {
        val out = sanitizeDisplayText("abcdefghij", 5)
        assertEquals("abcd…", out)
        assertEquals(5, out.length)
    }

    @Test
    fun `a bound of zero means no bound rather than an empty string`() {
        assertEquals("abcdefghij", sanitizeDisplayText("abcdefghij", 0))
        assertEquals("abcde", sanitizeDisplayText("abcde", 5))
    }

    @Test
    fun `nothing usable becomes the empty string, never invented wording`() {
        for (input in listOf(null, "", "   ", " $rlo", "\n\n", Char(0x200E).toString())) {
            assertEquals("", sanitizeDisplayText(input, 32))
        }
    }
}
