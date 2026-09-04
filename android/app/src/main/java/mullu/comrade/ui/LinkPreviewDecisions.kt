package mullu.comrade.ui

import java.net.URI

/**
 * The Telegram-style link-preview card: whether a message earns one, and what
 * domain it names.
 *
 * Kept free of Compose/Android imports, like [MessageAction] and
 * [mullu.comrade.together.TogetherDecisions] — the fetch itself (following the
 * URL, reading `title`/`description`/`site_name`/an image) is somebody else's
 * network code; nothing here makes a request. What is here is the two
 * decisions that stay true regardless of how the fetch is done.
 *
 * **The domain shown is derived from the URL, never from `site_name`, and there
 * is no parameter here that could change that.** A link preview's `title`,
 * `description` and `site_name` are supplied by whatever page the URL points
 * to — which is to say, by the sender, since they chose the URL. A message
 * card is trusted chrome: it sits in the bubble looking like Comrade vouches
 * for it. If the domain line could be filled from `site_name`, a message
 * linking to `https://paypal-secure.example.net/login` could carry a
 * `site_name` of "PayPal" and the card would say one site while the link goes
 * to another — the exact bait a phishing preview needs. [displayDomain] takes
 * only the URL. Not "ignores `site_name` if given" — it is never given one, so
 * there is no call site left that could pass it in a year from now and quietly
 * reopen this.
 */

/** One message's worth of preview: the URL it names, and the domain to show for it. */
data class LinkPreviewCandidate(val url: String, val domain: String)

/**
 * A bare `http(s)://` URL in already-typed message text.
 *
 * Trailing punctuation most people leave attached to a link when they write a
 * sentence around it — `.`, `,`, `!`, `?`, closing brackets and quotes — is
 * trimmed off the match, because none of those characters end a real URL path
 * often enough to be worth breaking the far more common "check this out:
 * https://example.com/foo." case.
 */
private val URL_REGEX = Regex("""https?://\S+""")
private val TRAILING_PUNCTUATION = charArrayOf('.', ',', '!', '?', ')', ']', '}', '"', '\'')

/**
 * The first URL in [text], or `null` if it has none.
 *
 * The *first* one, deliberately, even when a message names several: one card
 * per message is the same restraint `MessageAction.React`'s single emoji row
 * has, and picking the first matches what the sender typed first rather than
 * an arbitrary later link nobody was looking at when they hit send.
 */
fun firstUrl(text: String): String? =
    URL_REGEX.find(text)?.value?.trimEnd(*TRAILING_PUNCTUATION)?.ifEmpty { null }

/**
 * Whether [text] earns a preview card at all.
 *
 * Yes even when [text] is *only* the URL and nothing else — that is the single
 * most common way a link gets shared, not an edge case to special-case away.
 * Suppressing the card there would remove the preview for exactly the message
 * it exists to serve, and unlike a card competing with a paragraph of typed
 * text, a bare-link message has nothing else in the bubble for it to crowd.
 */
fun hasLinkPreview(text: String): Boolean = firstUrl(text) != null

/**
 * The domain [displayDomain] would put on the card for the message's first
 * URL, or `null` when [text] carries none.
 */
fun linkPreviewFor(text: String): LinkPreviewCandidate? {
    val url = firstUrl(text) ?: return null
    val domain = displayDomain(url) ?: return null
    return LinkPreviewCandidate(url, domain)
}

/**
 * The host [url] actually resolves to, lower-cased and stripped of a leading
 * `www.` — never anything supplied by the page itself. See the file header for
 * why this takes no other input.
 *
 * Parsed with [URI] rather than a `substringAfter("://")` scan on purpose:
 * `https://example.com@evil.example/path` is a URL a hand-rolled parser can
 * easily read as `example.com` (everything between the scheme and the next
 * `/`), when the host a real client connects to — and the one this card must
 * name — is `evil.example`. [URI.getHost] already resolves the authority
 * component correctly, so this only has to trust it.
 *
 * `null` for anything [URI] cannot parse, or with no host at all — a broken or
 * schemeless URL gets no card rather than a guessed one.
 */
fun displayDomain(url: String): String? {
    val host = try {
        URI(url).host
    } catch (e: java.net.URISyntaxException) {
        null
    } ?: return null
    return host.lowercase().removePrefix("www.")
}
