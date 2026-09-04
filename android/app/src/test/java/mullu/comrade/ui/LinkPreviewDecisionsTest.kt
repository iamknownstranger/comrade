package mullu.comrade.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The link-preview decision vectors — pinned so a hostile `site_name` cannot
 * relabel the domain, and so a bare-link message still gets a card. */
class LinkPreviewDecisionsTest {

    // ── Whether a message gets a card ───────────────────────────────────────

    @Test
    fun aMessageWithNoUrlGetsNoCard() {
        assertFalse(hasLinkPreview("just saying hi"))
        assertNull(linkPreviewFor("just saying hi"))
    }

    @Test
    fun aMessageThatIsOnlyTheUrlStillGetsACard() {
        // The single most common way a link is shared — suppressing it here
        // would remove the preview for exactly the case it exists to serve.
        assertTrue(hasLinkPreview("https://example.com/offer"))
        assertEquals("example.com", linkPreviewFor("https://example.com/offer")?.domain)
    }

    @Test
    fun aMessageWithTextAndAUrlGetsACard() {
        assertTrue(hasLinkPreview("check this out: https://example.com/offer"))
    }

    @Test
    fun onlyTheFirstUrlBecomesTheCard() {
        val text = "https://first.example/a and also https://second.example/b"
        assertEquals("https://first.example/a", firstUrl(text))
        assertEquals("first.example", linkPreviewFor(text)?.domain)
    }

    @Test
    fun trailingSentencePunctuationIsNotPartOfTheUrl() {
        assertEquals("https://example.com/foo", firstUrl("Look: https://example.com/foo."))
        assertEquals("https://example.com/foo", firstUrl("Have you seen (https://example.com/foo)?"))
    }

    // ── The domain the card shows ────────────────────────────────────────────

    @Test
    fun domainComesFromTheHostNotFromASiteNameSincelinkPreviewDecisionsAcceptsNone() {
        // There is no site_name parameter anywhere in this file — see the
        // header. This test pins the one thing that would matter if there
        // were: a phishing host must not be able to relabel itself.
        assertEquals(
            "paypal-secure.evil.example",
            displayDomain("https://paypal-secure.evil.example/login"),
        )
    }

    @Test
    fun theRealHostWinsOverAUserinfoLookalike() {
        // A classic phishing shape: the legitimate-looking name sits before
        // the "@" as userinfo, and the host a client actually connects to —
        // the one this card must name — is what follows it.
        assertEquals(
            "evil.example",
            displayDomain("https://example.com@evil.example/path"),
        )
    }

    @Test
    fun wwwPrefixIsStrippedAndCaseIsNormalised() {
        assertEquals("example.com", displayDomain("https://WWW.Example.com/path"))
    }

    @Test
    fun aUrlWithNoHostGetsNoDomain() {
        assertNull(displayDomain("https:///no-host"))
    }

    @Test
    fun anUnparsableUrlGetsNoDomainRatherThanAGuess() {
        assertNull(displayDomain("https://[bad"))
    }
}
