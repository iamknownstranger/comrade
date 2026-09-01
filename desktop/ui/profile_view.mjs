// The rules a profile page has to agree on across every frontend, kept pure so
// they can be tested once and mirrored exactly.
//
// Ports: `app/lib/src/util/profile_view.dart`,
// `android/.../ui/ProfileView.kt`. Same cases, same answers — a change here is a
// change in three places.
//
// What is *not* here: any wording. These functions return row and action kinds,
// never labels, for the same reason `PresenceLabel` in `DisplayName.kt` does —
// the rule is shared, the strings live in `strings.xml` / the DOM / `.arb` where
// they can be translated.

import { attachmentPreviewKind, formatAttachmentSize } from "./attachment_caption.mjs";
import { sanitizeDisplayText } from "./display_text.mjs";

/**
 * The largest profile picture that will be accepted — `MAX_AVATAR_BYTES` in
 * `comrade_core::avatar`.
 *
 * Two orders of magnitude below the attachment cap on purpose. An avatar is
 * fetched on sight, for every peer whose profile you open, and every one of those
 * bytes is buffered before anything can be drawn; the attachment cap is what one
 * deliberate send may cost, which is a different question.
 */
export const MAX_AVATAR_BYTES = 256 * 1024;

/** The longest bio this UI will draw, matching `MAX_ABOUT_LEN` in the core. */
export const MAX_ABOUT_CHARS = 512;

/** The longest published handle this UI will draw on one row. */
export const MAX_HANDLE_CHARS = 64;

/**
 * The image types a profile picture may be, mirroring the allowlist the core
 * enforces on fetch.
 *
 * An allowlist rather than "starts with image/": the set of things a platform
 * image decoder will attempt is much wider than the set anyone needs for an
 * avatar, and every extra format is another decoder reachable by a URL a peer
 * chose. SVG is absent deliberately — it is a document that can fetch and
 * script, not a picture.
 */
export const AVATAR_MIME_ALLOWLIST = Object.freeze([
  "image/jpeg",
  "image/png",
  "image/webp",
]);

/**
 * The rows of the info block, in order.
 *
 * Empty values are dropped — a blank "Bio" row states that we know the person
 * has no bio, which is not the same as not having fetched one. Every peer-chosen
 * value goes through `sanitizeDisplayText` on the way, because these are drawn at
 * heading size next to an avatar: the same threat a transfer card's filename has,
 * and the same answer.
 *
 * The `key` row is the exception: it is **never** dropped, and it is last.
 * D1/D10's reasoning is that a self-declared handle shown without the key
 * reachable "is the exact shape of an impersonation", and the owner call of
 * 2026-07-30 (AUDIT.md) moved the key out of the conversation header on the
 * grounds that it was reachable on demand one tap away. This page *is* that
 * place, so the key is not optional here — every other row is a claim the person
 * made about themselves, and this is the one row that is a fact.
 */
export function infoRows(profile, { isSelf = false } = {}) {
  const p = profile ?? {};
  const rows = [];
  const push = (kind, value, maxChars) => {
    const clean = sanitizeDisplayText(value, maxChars);
    if (clean) rows.push({ kind, value: clean, copyable: kind !== "bio" });
  };
  push("bio", p.about, MAX_ABOUT_CHARS);
  push("handle", handleOf(p), MAX_HANDLE_CHARS);
  push("nip05", p.nip05, MAX_HANDLE_CHARS);
  push("lud16", p.lud16, MAX_HANDLE_CHARS);
  // Never conditional, and always last: the long monospace string is the least
  // scannable row and the one nobody reads unless they came for it. Not
  // sanitized and not truncated — it is bech32 from our own parser, and a
  // shortened key is not a key.
  rows.push({ kind: "key", value: String(p.npub ?? "").trim(), copyable: true });
  // Your own empty bio still gets a row, because on your own page an empty row
  // is the affordance to fill it in. A peer's does not: there is nothing to act
  // on, and a blank row would read as a bio that says nothing.
  if (isSelf && !rows.some((r) => r.kind === "bio")) {
    rows.unshift({ kind: "bio", value: "", copyable: false });
  }
  return rows;
}

/**
 * The published handle with at most one leading `@`.
 *
 * `peerTitle` already owns the display-precedence rule (alias → handle → key);
 * this is only the prefix normalisation, so `name`, `@name` and `@@name` render
 * identically instead of three ways.
 */
export function handleOf(profile) {
  const raw = String(profile?.name ?? profile?.username ?? "").trim();
  const bare = raw.replace(/^@+/, "");
  return bare ? `@${bare}` : "";
}

/**
 * Which of the shared-media tabs an item belongs to: "media" | "voice" | "files".
 *
 * Delegates the MIME reading to `attachmentPreviewKind` rather than repeating it,
 * so a bubble, a preview sheet and a profile tab can never disagree about what a
 * file is. Photos and videos share one tab because that is the grid people scan
 * visually; a voice note has nothing to show and belongs in a list.
 */
export function mediaTabFor(mimeType) {
  const kind = attachmentPreviewKind(mimeType);
  if (kind === "image" || kind === "video") return "media";
  if (kind === "audio") return "voice";
  return "files";
}

/**
 * Split a peer's media history into the three tabs, newest first within each.
 *
 * `media_with` hands back the merged both-directions history oldest-first (see
 * its test, `media_with_lists_history_oldest_first_with_correct_direction`). A
 * profile's tabs are a "what have we exchanged" view, where the recent end is
 * what anyone is looking for, so this reverses — once, here, rather than in three
 * renderers.
 *
 * Nothing is ever dropped: an unrecognised MIME type lands in Files, because a
 * bucket that silently swallows an attachment makes it unreachable.
 */
export function bucketMedia(items) {
  const buckets = { media: [], voice: [], files: [] };
  for (const item of items ?? []) {
    buckets[mediaTabFor(item?.mimeType ?? item?.mime_type)].push(item);
  }
  for (const key of Object.keys(buckets)) buckets[key].reverse();
  return buckets;
}

/** How many items each tab would show — what the tab strip's counts render. */
export function mediaTabCounts(items) {
  const b = bucketMedia(items);
  return { media: b.media.length, voice: b.voice.length, files: b.files.length };
}

/**
 * The http(s) URLs in a message body, in the order they appear, de-duplicated.
 *
 * Deliberately **not** a regex for the matching. Three languages means three
 * regex dialects, and a link scanner is exactly the rule that drifts invisibly
 * when each port writes its own pattern — so this is whitespace splitting plus a
 * scheme test, which mirrors identically into Kotlin and Dart.
 *
 * Only `http://` and `https://`. `javascript:`, `data:`, `file:` and
 * scheme-relative `//host` are refused *here*, so no frontend has to remember —
 * and the desktop especially, where an `href` is the one place `el()`'s
 * `textContent` discipline does not protect anything.
 *
 * `host` is returned **separately** and callers must render it as the prominent
 * part. `https://evil.example/login?next=paypal.com` must not be presentable as
 * a PayPal link.
 */
export function extractLinks(text) {
  const found = [];
  const seen = new Set();
  // Sanitize first: a bidi override inside a URL can reorder a displayed host.
  for (const token of sanitizeDisplayText(text, 0).split(" ")) {
    const url = trimUrlPunctuation(token);
    if (!url) continue;
    const lower = url.toLowerCase();
    if (!lower.startsWith("https://") && !lower.startsWith("http://")) continue;
    const host = hostOf(url);
    // A scheme with no host is not a link.
    if (!host) continue;
    if (seen.has(url)) continue;
    seen.add(url);
    found.push({ url, host });
  }
  return found;
}

/**
 * The host of an http(s) URL, lowercased, without userinfo or port.
 *
 * Hand-parsed rather than via `new URL()` because the Kotlin and Dart ports must
 * agree with it character for character, and three platform URL parsers do not.
 * Userinfo is dropped rather than shown: `https://paypal.com@evil.example/` has
 * a host of `evil.example`, and rendering the part before the `@` is the oldest
 * phishing trick there is.
 */
export function hostOf(url) {
  const raw = String(url ?? "");
  const marker = raw.indexOf("://");
  // No scheme, no answer. Slicing at `indexOf(...) + 3` regardless — which this
  // did until the Kotlin port was written against it — takes two characters off
  // the front of a schemeless string and calls the remainder a host, so
  // `example.com/x` reads as `ample.com`. A wrong host is the one output this
  // function must never produce.
  if (marker === -1) return "";
  const afterScheme = raw.slice(marker + 3);
  const authority = afterScheme.split(/[/?#]/)[0] ?? "";
  const afterUserinfo = authority.includes("@")
    ? authority.slice(authority.lastIndexOf("@") + 1)
    : authority;
  // An IPv6 literal keeps its brackets; anything else loses a :port.
  if (afterUserinfo.startsWith("[")) {
    const close = afterUserinfo.indexOf("]");
    return close === -1 ? "" : afterUserinfo.slice(0, close + 1).toLowerCase();
  }
  return afterUserinfo.split(":")[0].toLowerCase();
}

/** Strip the punctuation a URL collects from the prose around it. */
function trimUrlPunctuation(token) {
  let url = String(token ?? "");
  while (url && "(<[\"'".includes(url[0])) url = url.slice(1);
  for (;;) {
    const last = url.at(-1);
    if (!last || !".,;:!?)]}>\"'".includes(last)) break;
    // Keep a closing bracket the URL itself opened — Wikipedia links carry
    // balanced parens.
    if (last === ")" && countOf(url, "(") > countOf(url, ")") - 1) break;
    if (last === "]" && countOf(url, "[") > countOf(url, "]") - 1) break;
    url = url.slice(0, -1);
  }
  return url;
}

function countOf(text, ch) {
  let n = 0;
  for (const c of text) if (c === ch) n += 1;
  return n;
}

/**
 * Every link exchanged with a peer, newest first — the Links tab.
 *
 * Sourced from message *text*, because no DTO carries links: `MessageDto` has a
 * body and nothing else, so the alternative to scanning is not having the tab.
 */
export function collectLinks(messages) {
  const out = [];
  const seen = new Set();
  const ordered = [...(messages ?? [])].sort((a, b) => atOf(b) - atOf(a));
  for (const msg of ordered) {
    for (const { url, host } of extractLinks(msg?.content)) {
      if (seen.has(url)) continue;
      seen.add(url);
      out.push({ url, host, at: atOf(msg), outgoing: !!msg?.outgoing });
    }
  }
  return out;
}

function atOf(msg) {
  return Number(msg?.createdAt ?? msg?.created_at ?? 0) || 0;
}

/**
 * The avatar's side length at a given header collapse fraction, where 0 is fully
 * expanded and 1 fully collapsed.
 *
 * Linear, and clamped at both ends so a platform that reports a fraction
 * slightly outside 0..1 during an overscroll (all three do, at some point) cannot
 * produce an avatar larger than the header or an inverted one.
 *
 * The three renderers are necessarily different — a Compose `LargeTopAppBar`
 * state, a Flutter `FlexibleSpaceBar`, a scroll listener over a CSS custom
 * property — so what is shared is the *curve*, which is the part a user would
 * notice drifting between platforms.
 */
export function collapsedAvatarSize(fraction, expandedPx, collapsedPx) {
  const expanded = Number(expandedPx) || 0;
  const collapsed = Number(collapsedPx) || 0;
  const raw = Number(fraction);
  const f = Number.isFinite(raw) ? Math.min(1, Math.max(0, raw)) : 0;
  return expanded + (collapsed - expanded) * f;
}

/**
 * Which actions the row under the header offers, in order.
 *
 * Four rules carry weight:
 *
 * 1. **A blocked peer offers nothing at all** — not even Unblock. There is no
 *    unblock command in the core and no getter for the state to drive one
 *    (`STATE_BLOCKED` is written at `runtime.rs:2276` and read only by
 *    `IncomingGate`), so a button here would be a fake switch, which is the one
 *    thing `SettingsScreen`'s own rule forbids. The page says you blocked them
 *    and offers no lie about undoing it. When an unblock command exists, this is
 *    the function that changes, and its test is what will say so.
 * 2. **A stranger gets no Call button.** Placing a call makes this device gather
 *    ICE for whoever is on the other end — the same bar the accepted-conversation
 *    gate already holds an incoming call signal to, for the same reason. Offering
 *    it before acceptance is a button whose only outcomes are a leak or an error.
 * 3. **Mute is only meaningful for someone you hear from.** A stranger's messages
 *    are already gated behind a request.
 * 4. **Your own profile offers no Message and no Block.** Both are nonsense
 *    against yourself, and a Block that half-worked would be worse than absent.
 */
export function actionRow({
  isSelf = false,
  isContact = false,
  isComrade = false,
  isMuted = false,
  isBlocked = false,
} = {}) {
  if (isSelf) return ["edit", "copyKey"];
  if (isBlocked) return [];
  const actions = ["message"];
  if (isContact) {
    actions.push("call");
    actions.push(isMuted ? "unmute" : "mute");
    actions.push(isComrade ? "removeComrade" : "addComrade");
  } else {
    actions.push("addContact");
  }
  actions.push("block");
  return actions;
}

/**
 * Which tab a profile should open on: the first non-empty in the canonical order
 * Media → Files → Voice, or "media" when there is nothing at all.
 *
 * Opening on an empty Media tab makes a profile with plenty of files read as
 * "nothing shared", which is the failure this exists to prevent.
 */
export function initialMediaTab(items) {
  const counts = mediaTabCounts(items);
  if (counts.media > 0) return "media";
  if (counts.files > 0) return "files";
  if (counts.voice > 0) return "voice";
  return "media";
}

/**
 * Why this image cannot be used as a profile picture, or null when it can.
 *
 * Checked before the confirmation dialog opens. The core enforces the same cap
 * and the same allowlist on the way back *in* (a peer's URL is attacker-chosen),
 * so this is the courtesy half of a guard that exists on both sides — never the
 * guard itself.
 */
export function avatarRejection(mimeType, bytes) {
  const mime = String(mimeType ?? "")
    .trim()
    .toLowerCase();
  const n = Number(bytes) || 0;
  if (n <= 0) return "That image is empty — there is nothing to publish.";
  if (!AVATAR_MIME_ALLOWLIST.includes(mime)) {
    return "A profile picture has to be a JPEG, PNG or WebP.";
  }
  if (n > MAX_AVATAR_BYTES) {
    return (
      `That image is ${formatAttachmentSize(n)} — profile pictures are ` +
      `limited to ${formatAttachmentSize(MAX_AVATAR_BYTES)}.`
    );
  }
  return null;
}

/**
 * Whether a peer's `picture` URL may be fetched at all.
 *
 * The URL comes out of a Kind-0 event, so it is chosen by whoever published it —
 * which for a stranger is someone who has not been accepted for anything else
 * either. Two gates, and the caller must pass both:
 *
 * - the user has not turned remote pictures off, and
 * - the profile belongs to someone already accepted, so opening a stranger's
 *   profile cannot make this device call out to a host they picked.
 *
 * Scheme, host and size are *not* checked here. They are enforced in the core,
 * where every caller gets them whether or not it remembered to ask — the same
 * reasoning `MAX_MEDIA_BYTES`'s doc comment gives for living there. This
 * function is the "should we ask at all", not the "is it safe".
 */
export function mayFetchAvatar({
  url,
  remoteAvatarsEnabled = true,
  isContact = false,
  isSelf = false,
  isBlocked = false,
} = {}) {
  if (!String(url ?? "").trim()) return false;
  if (!remoteAvatarsEnabled) return false;
  if (isBlocked) return false;
  return isSelf || isContact;
}
