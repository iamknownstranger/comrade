import { test } from "node:test";
import assert from "node:assert/strict";

import {
  AVATAR_MIME_ALLOWLIST,
  MAX_AVATAR_BYTES,
  actionRow,
  avatarRejection,
  bucketMedia,
  collapsedAvatarSize,
  collectLinks,
  extractLinks,
  handleOf,
  hostOf,
  infoRows,
  initialMediaTab,
  mayFetchAvatar,
  mediaTabCounts,
  mediaTabFor,
} from "./profile_view.mjs";

const NPUB = "npub1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq";
const RLO = String.fromCharCode(0x202e);

const kinds = (rows) => rows.map((r) => r.kind);

// ── The info block ───────────────────────────────────────────────────────────

test("the key row is present for every profile, however full or empty", () => {
  // The D1/D10 invariant, mechanically enforced for the first time: a
  // self-declared handle must never be shown with the key unreachable.
  const full = infoRows({
    npub: NPUB,
    name: "bob",
    about: "hi",
    nip05: "bob@example.com",
    lud16: "bob@wallet.example",
  });
  assert.deepEqual(kinds(full), ["bio", "handle", "nip05", "lud16", "key"]);

  const empty = infoRows({ npub: NPUB });
  assert.deepEqual(kinds(empty), ["key"], "a profile we know nothing about still shows the key");

  const nothingAtAll = infoRows({});
  assert.deepEqual(kinds(nothingAtAll), ["key"], "even a profile with no npub keeps the row");
});

test("the key row is last, because it is the least scannable line on the page", () => {
  const rows = infoRows({ npub: NPUB, name: "bob", about: "hi" });
  assert.equal(rows.at(-1).kind, "key");
});

test("a blank field produces no row rather than an empty one", () => {
  // "We know they have no bio" and "we have not fetched a bio" are different
  // facts, and a blank row states the first when only the second is true.
  const rows = infoRows({ npub: NPUB, about: "   ", name: "", nip05: null });
  assert.deepEqual(kinds(rows), ["key"]);
});

test("your own empty bio still gets a row, because that row is how you fill it in", () => {
  const mine = infoRows({ npub: NPUB }, { isSelf: true });
  assert.deepEqual(kinds(mine), ["bio", "key"]);
  assert.equal(mine[0].value, "", "empty, and the caller supplies the invitation wording");
  const theirs = infoRows({ npub: NPUB });
  assert.deepEqual(kinds(theirs), ["key"], "a peer's blank bio is not an affordance");
});

test("a bio cannot smuggle control characters onto the page", () => {
  const rows = infoRows({ npub: NPUB, about: `Trust me${RLO}` });
  assert.equal(rows[0].value, "Trust me", "peer-chosen text is drawn at heading size");
});

test("a bio that is forty lines long is collapsed to one", () => {
  const rows = infoRows({ npub: NPUB, about: "line\n".repeat(40) });
  assert.equal(
    rows[0].value,
    "line ".repeat(40).trim(),
    "a bio full of newlines is a way to push the rest of the profile off screen",
  );
});

test("the key is never truncated, because a shortened key is not a key", () => {
  const rows = infoRows({ npub: NPUB });
  assert.equal(rows.at(-1).value, NPUB);
  assert.equal(rows.at(-1).value.length, NPUB.length);
});

test("the bio is the one row that is not offered as copyable", () => {
  const rows = infoRows({
    npub: NPUB,
    about: "hi",
    name: "bob",
    nip05: "b@e.com",
    lud16: "b@w.example",
  });
  const copyable = Object.fromEntries(rows.map((r) => [r.kind, r.copyable]));
  assert.deepEqual(copyable, {
    bio: false,
    handle: true,
    nip05: true,
    lud16: true,
    key: true,
  });
});

test("a nip05 is its own row and carries no verified flag", () => {
  // The core does not verify NIP-05, so no frontend is handed a boolean it could
  // draw a checkmark from.
  const rows = infoRows({ npub: NPUB, nip05: "bob@example.com" });
  const row = rows.find((r) => r.kind === "nip05");
  assert.deepEqual(Object.keys(row).sort(), ["copyable", "kind", "value"]);
});

// ── The handle ───────────────────────────────────────────────────────────────

test("a handle renders with exactly one @, however many the peer published", () => {
  assert.equal(handleOf({ name: "bob" }), "@bob");
  assert.equal(handleOf({ name: "@bob" }), "@bob");
  assert.equal(handleOf({ name: "@@@bob" }), "@bob", "three @ is a peer testing the renderer");
  assert.equal(handleOf({ name: "  @bob  " }), "@bob");
});

test("a handle of nothing but @ is no handle at all", () => {
  assert.equal(handleOf({ name: "@" }), "");
  assert.equal(handleOf({ name: "@@@" }), "");
  assert.equal(handleOf({}), "");
});

test("the own-profile username field is read when there is no published name", () => {
  assert.equal(handleOf({ username: "me" }), "@me");
  assert.equal(handleOf({ name: "published", username: "local" }), "@published");
});

// ── Shared-media buckets ─────────────────────────────────────────────────────

test("photos and videos share the grid, voice notes get their own list", () => {
  assert.equal(mediaTabFor("image/png"), "media");
  assert.equal(mediaTabFor("video/mp4"), "media");
  assert.equal(mediaTabFor("audio/ogg"), "voice");
  assert.equal(mediaTabFor("application/pdf"), "files");
});

test("no mime type is ever dropped, because a dropped attachment is unreachable", () => {
  for (const mime of ["", null, undefined, "garbage", "/", "image", "IMAGE/PNG"]) {
    const bucket = mediaTabFor(mime);
    assert.ok(
      ["media", "voice", "files"].includes(bucket),
      `${JSON.stringify(mime)} landed nowhere, which hides the file entirely`,
    );
  }
  assert.equal(mediaTabFor("IMAGE/PNG"), "media", "casing is the server's choice, not a category");
  assert.equal(mediaTabFor("  image/png  "), "media");
});

test("a bucketed history reads newest first, though the store hands it over oldest first", () => {
  const items = [
    { mimeType: "image/png", eventId: "old" },
    { mimeType: "image/png", eventId: "new" },
  ];
  assert.deepEqual(
    bucketMedia(items).media.map((i) => i.eventId),
    ["new", "old"],
    "a profile's media tab is a what-have-we-exchanged view; the recent end is the answer",
  );
});

test("bucketing reads snake_case as well as camelCase, since both bridges are in use", () => {
  const items = [{ mime_type: "audio/ogg" }, { mimeType: "audio/ogg" }];
  assert.equal(mediaTabCounts(items).voice, 2, "the Tauri IPC and the frb bridge spell it differently");
});

test("counts and buckets agree, and an empty history counts zero everywhere", () => {
  const items = [{ mimeType: "image/png" }, { mimeType: "audio/ogg" }, { mimeType: "text/plain" }];
  assert.deepEqual(mediaTabCounts(items), { media: 1, voice: 1, files: 1 });
  assert.deepEqual(mediaTabCounts([]), { media: 0, voice: 0, files: 0 });
  assert.deepEqual(mediaTabCounts(null), { media: 0, voice: 0, files: 0 });
});

test("a profile opens on a tab that has something in it", () => {
  // Opening on an empty Media tab makes a profile with plenty of files read as
  // "nothing shared", which is the failure this exists to prevent.
  assert.equal(initialMediaTab([{ mimeType: "application/pdf" }]), "files");
  assert.equal(initialMediaTab([{ mimeType: "audio/ogg" }]), "voice");
  assert.equal(initialMediaTab([{ mimeType: "image/png" }, { mimeType: "audio/ogg" }]), "media");
  assert.equal(initialMediaTab([]), "media", "with nothing at all, one honest empty state");
});

// ── Links ────────────────────────────────────────────────────────────────────

test("only http and https are ever treated as links", () => {
  const hostile =
    "javascript:alert(1) data:text/html,x file:///etc/passwd //evil.example ftp://x.example/a";
  assert.deepEqual(
    extractLinks(hostile),
    [],
    "an href is the one place the textContent discipline does not protect anything",
  );
});

test("a link's host is returned separately so it can be the prominent part", () => {
  const [link] = extractLinks("see https://evil.example/login?next=paypal.com");
  assert.equal(link.host, "evil.example");
  assert.equal(link.url, "https://evil.example/login?next=paypal.com");
});

test("userinfo never becomes the displayed host", () => {
  // The oldest phishing trick there is: everything before the @ is not the host.
  assert.equal(hostOf("https://paypal.com@evil.example/"), "evil.example");
  assert.equal(hostOf("https://a@b@evil.example/x"), "evil.example");
});

test("a port is not part of the host, and an IPv6 literal keeps its brackets", () => {
  assert.equal(hostOf("https://example.com:8443/x"), "example.com");
  assert.equal(hostOf("https://[2001:db8::1]:8443/x"), "[2001:db8::1]");
  assert.equal(hostOf("https://[2001:db8::1]/x"), "[2001:db8::1]");
});

test("a host is lowercased, so two spellings of one host read as one host", () => {
  assert.equal(hostOf("https://ExAmPlE.CoM/x"), "example.com");
});

test("a string with no scheme has no host, rather than a mangled one", () => {
  // Not reachable through `extractLinks`, which tests the scheme first — but
  // `hostOf` is exported, and a function that answers `ample.com` for
  // `example.com/x` is a wrong host waiting for its first caller.
  assert.equal(hostOf("example.com/x"), "");
  assert.equal(hostOf(""), "");
  assert.equal(hostOf(null), "");
});

test("a scheme with no host is not a link", () => {
  assert.deepEqual(extractLinks("https:// https://"), []);
});

test("trailing prose punctuation is not part of the URL", () => {
  assert.equal(extractLinks("see https://example.com/a.")[0].url, "https://example.com/a");
  assert.equal(extractLinks("(https://example.com/a)")[0].url, "https://example.com/a");
  assert.equal(extractLinks("'https://example.com/a',")[0].url, "https://example.com/a");
});

test("a bracket the URL itself opened is kept", () => {
  assert.equal(
    extractLinks("https://en.example.org/wiki/Foo_(bar)")[0].url,
    "https://en.example.org/wiki/Foo_(bar)",
    "stripping it would break every Wikipedia link ever pasted",
  );
});

test("a bidi override inside a URL cannot reorder the host on screen", () => {
  const links = extractLinks(`https://example.com/${RLO}gnp.exe`);
  assert.ok(!links[0].url.includes(RLO), "the override is gone before anything is drawn");
});

test("an ideographic space separates two links, as it does in the prose it comes from", () => {
  // The case that proved Java's ASCII-only `\s` had drifted from this file:
  // U+3000 is the ordinary word space in Japanese and Chinese, and one between
  // two URLs must not fuse them into a single link labelled with the first
  // one's host.
  const links = extractLinks("https://good.example/\u3000https://evil.example/");
  assert.deepEqual(
    links.map((l) => l.url),
    ["https://good.example/", "https://evil.example/"],
  );
});

test("the same link twice is one link", () => {
  const links = extractLinks("https://example.com/a https://example.com/a");
  assert.equal(links.length, 1);
});

test("links across a thread are newest first and de-duplicated across messages", () => {
  const messages = [
    { content: "old https://example.com/a", createdAt: 100, outgoing: false },
    { content: "new https://example.com/b", createdAt: 300, outgoing: true },
    { content: "again https://example.com/a", createdAt: 200, outgoing: false },
  ];
  const links = collectLinks(messages);
  assert.deepEqual(
    links.map((l) => l.url),
    ["https://example.com/b", "https://example.com/a"],
  );
  assert.equal(links[0].at, 300);
  assert.equal(links[0].outgoing, true);
  assert.equal(links[1].at, 200, "the newest sighting of a repeated link is the one kept");
});

test("collecting links tolerates a missing body and a missing timestamp", () => {
  assert.deepEqual(collectLinks(null), []);
  assert.deepEqual(collectLinks([{}, { content: null }]), []);
  const [only] = collectLinks([{ content: "https://example.com/a" }]);
  assert.equal(only.at, 0, "no timestamp is 0, not NaN, so a sort cannot scramble the list");
});

// ── The collapsing header ────────────────────────────────────────────────────

test("the avatar shrinks from expanded to collapsed across the fraction", () => {
  assert.equal(collapsedAvatarSize(0, 96, 36), 96);
  assert.equal(collapsedAvatarSize(1, 96, 36), 36);
  assert.equal(collapsedAvatarSize(0.5, 96, 36), 66);
});

test("an overscroll fraction outside 0..1 cannot invert or oversize the avatar", () => {
  // All three platforms report a fraction slightly outside the range at some
  // point during an overscroll.
  assert.equal(collapsedAvatarSize(-0.5, 96, 36), 96);
  assert.equal(collapsedAvatarSize(1.5, 96, 36), 36);
  assert.equal(collapsedAvatarSize(NaN, 96, 36), 96, "nonsense reads as fully expanded");
  assert.equal(collapsedAvatarSize(undefined, 96, 36), 96);
});

test("the avatar never grows while the header collapses", () => {
  let previous = Infinity;
  for (let i = 0; i <= 100; i += 1) {
    const size = collapsedAvatarSize(i / 100, 96, 36);
    assert.ok(size <= previous, `size grew at fraction ${i / 100}, which reads as a stutter`);
    previous = size;
  }
});

// ── The action row ───────────────────────────────────────────────────────────

test("a stranger is never offered a call", () => {
  // Placing one makes this device gather ICE for whoever is on the other end —
  // the same bar the accepted-conversation gate holds an incoming signal to.
  const actions = actionRow({ isContact: false });
  assert.ok(!actions.includes("call"), "the only outcomes would be a leak or an error");
  assert.deepEqual(actions, ["message", "addContact", "block"]);
});

test("a contact gets a call, a mute and the comrade toggle", () => {
  assert.deepEqual(actionRow({ isContact: true }), [
    "message",
    "call",
    "mute",
    "addComrade",
    "block",
  ]);
  assert.deepEqual(actionRow({ isContact: true, isMuted: true, isComrade: true }), [
    "message",
    "call",
    "unmute",
    "removeComrade",
    "block",
  ]);
});

test("a blocked peer is offered nothing, because there is no unblock to offer", () => {
  // `STATE_BLOCKED` is written in the runtime and read only by `IncomingGate`;
  // there is no unblock command and no getter to drive one. A button here would
  // be the fake switch the settings screen's own rule forbids. When an unblock
  // command exists, this is the test that will say so by failing.
  assert.deepEqual(actionRow({ isBlocked: true }), []);
  assert.deepEqual(
    actionRow({ isBlocked: true, isContact: true, isComrade: true }),
    [],
    "being a contact does not resurrect a send affordance for someone you blocked",
  );
});

test("your own profile offers neither a message nor a block", () => {
  const mine = actionRow({ isSelf: true });
  assert.deepEqual(mine, ["edit", "copyKey"]);
  assert.ok(!mine.includes("message"), "messaging yourself is not a feature");
  assert.ok(!mine.includes("block"), "a block that half-worked is worse than absent");
});

test("self outranks every other flag, so no combination leaks a block onto your own page", () => {
  assert.deepEqual(
    actionRow({ isSelf: true, isContact: true, isComrade: true, isMuted: true, isBlocked: true }),
    ["edit", "copyKey"],
  );
});

test("no argument at all is a stranger, not a crash", () => {
  assert.deepEqual(actionRow(), ["message", "addContact", "block"]);
  assert.deepEqual(actionRow(undefined), ["message", "addContact", "block"]);
});

// ── Publishing your own picture ──────────────────────────────────────────────

test("an avatar has to be one of the three formats the core will fetch back", () => {
  assert.equal(avatarRejection("image/png", 1024), null);
  assert.equal(avatarRejection("image/jpeg", 1024), null);
  assert.equal(avatarRejection("image/webp", 1024), null);
  assert.match(avatarRejection("image/svg+xml", 1024), /JPEG, PNG or WebP/);
  assert.match(avatarRejection("image/gif", 1024), /JPEG, PNG or WebP/);
  assert.match(avatarRejection("application/zip", 1024), /JPEG, PNG or WebP/);
});

test("SVG is refused, because it is a document that can fetch and script", () => {
  assert.ok(!AVATAR_MIME_ALLOWLIST.includes("image/svg+xml"));
});

test("an empty image is refused before its type is even considered", () => {
  assert.match(avatarRejection("image/png", 0), /empty/);
  assert.match(avatarRejection("image/png", -1), /empty/);
});

test("an oversize image is refused and the message names both sizes", () => {
  const why = avatarRejection("image/png", MAX_AVATAR_BYTES + 1);
  assert.match(why, /256 KB/, "a refusal that does not name the limit cannot be acted on");
});

test("an image exactly at the cap is accepted", () => {
  assert.equal(avatarRejection("image/png", MAX_AVATAR_BYTES), null);
});

test("the type is read case- and whitespace-insensitively", () => {
  assert.equal(avatarRejection("  IMAGE/PNG  ", 1024), null);
});

// ── Whether to fetch a peer's picture at all ─────────────────────────────────

test("a stranger's picture URL is never fetched", () => {
  // Opening a stranger's profile must not make this device call out to a host
  // they chose.
  assert.equal(mayFetchAvatar({ url: "https://x.example/a.png", isContact: false }), false);
});

test("turning remote pictures off stops every fetch, contact or not", () => {
  assert.equal(
    mayFetchAvatar({
      url: "https://x.example/a.png",
      isContact: true,
      remoteAvatarsEnabled: false,
    }),
    false,
  );
  assert.equal(
    mayFetchAvatar({ url: "https://x.example/a.png", isSelf: true, remoteAvatarsEnabled: false }),
    false,
  );
});

test("a blocked peer's picture is not fetched even if they are still a contact", () => {
  assert.equal(
    mayFetchAvatar({ url: "https://x.example/a.png", isContact: true, isBlocked: true }),
    false,
  );
});

test("a contact with a picture is fetched, and so is your own", () => {
  assert.equal(mayFetchAvatar({ url: "https://x.example/a.png", isContact: true }), true);
  assert.equal(mayFetchAvatar({ url: "https://x.example/a.png", isSelf: true }), true);
});

test("no URL means nothing to fetch", () => {
  assert.equal(mayFetchAvatar({ url: "", isContact: true }), false);
  assert.equal(mayFetchAvatar({ url: "   ", isContact: true }), false);
  assert.equal(mayFetchAvatar({ isContact: true }), false);
  assert.equal(mayFetchAvatar(), false);
});
