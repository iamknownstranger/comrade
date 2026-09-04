import { test } from "node:test";
import assert from "node:assert/strict";
import { firstUrl, hasLinkPreview, linkPreviewFor, displayDomain } from "./link_preview.mjs";

// Vectors ported 1:1 from
// android/app/src/test/java/mullu/comrade/ui/LinkPreviewDecisionsTest.kt —
// pinned so a hostile `site_name` cannot relabel the domain, and so a
// bare-link message still gets a card.

// ── Whether a message gets a card ───────────────────────────────────────

test("a message with no URL gets no card", () => {
  assert.equal(hasLinkPreview("just saying hi"), false);
  assert.equal(linkPreviewFor("just saying hi"), null);
});

test("a message that is only the URL still gets a card", () => {
  // The single most common way a link is shared — suppressing it here would
  // remove the preview for exactly the case it exists to serve.
  assert.equal(hasLinkPreview("https://example.com/offer"), true);
  assert.equal(linkPreviewFor("https://example.com/offer").domain, "example.com");
});

test("a message with text and a URL gets a card", () => {
  assert.equal(hasLinkPreview("check this out: https://example.com/offer"), true);
});

test("only the first URL becomes the card", () => {
  const text = "https://first.example/a and also https://second.example/b";
  assert.equal(firstUrl(text), "https://first.example/a");
  assert.equal(linkPreviewFor(text).domain, "first.example");
});

test("trailing sentence punctuation is not part of the URL", () => {
  assert.equal(firstUrl("Look: https://example.com/foo."), "https://example.com/foo");
  assert.equal(firstUrl("Have you seen (https://example.com/foo)?"), "https://example.com/foo");
});

// ── The domain the card shows ────────────────────────────────────────────

test("domain comes from the host, not from a site_name, since displayDomain accepts none", () => {
  // There is no site_name parameter anywhere in this file — see the header.
  // This test pins the one thing that would matter if there were: a
  // phishing host must not be able to relabel itself.
  assert.equal(
    displayDomain("https://paypal-secure.evil.example/login"),
    "paypal-secure.evil.example",
  );
});

test("the real host wins over a userinfo lookalike", () => {
  // A classic phishing shape: the legitimate-looking name sits before the
  // "@" as userinfo, and the host a client actually connects to — the one
  // this card must name — is what follows it.
  assert.equal(displayDomain("https://example.com@evil.example/path"), "evil.example");
});

test("www prefix is stripped and case is normalised", () => {
  assert.equal(displayDomain("https://WWW.Example.com/path"), "example.com");
});

test("a URL with no host gets no domain", () => {
  // WHATWG's `URL` gives a `file:`-style URL an empty-string host rather
  // than Java's `null` for the same "no authority" shape — same outcome,
  // different reason, so this pins the empty-host branch on its own rather
  // than folding it into the unparsable case below.
  assert.equal(displayDomain("file:///no-host"), null);
});

test("an unparsable URL gets no domain rather than a guess", () => {
  assert.equal(displayDomain("https://[bad"), null);
});
