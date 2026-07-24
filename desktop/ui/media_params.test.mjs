// Node's built-in test runner + assert — no package.json, no npm deps (see
// docs/COMMS_ARCHITECTURE.md §ADR-3: "Node ≥ 20 ships the built-in runner").
// Run with: node --test desktop/ui/*.test.mjs
// (a bare directory argument, e.g. `node --test desktop/ui/`, does NOT work
// on Node 22 — it tries to `require()` the directory itself and fails with
// MODULE_NOT_FOUND; a glob or explicit file list is required. See the CI
// job `desktop-js` in .github/workflows/ci.yml, which uses this same form —
// it already globs `desktop/ui/*.test.mjs`, so this file needs no workflow
// change to run in CI.)
import { test } from "node:test";
import assert from "node:assert/strict";

import { buildCaptureConstraints, buildVideoSenderParameters } from "./media_params.mjs";

// ── buildCaptureConstraints ─────────────────────────────────────────────────
// WP17 (docs/COMMS_ARCHITECTURE.md): bound desktop's capture the way
// Android's Camera2 capture is bounded (CallManager.kt:91-93: 1280x720@30),
// using `ideal` (never `min`/`exact`) so a webcam that can't do 720p still
// negotiates down instead of hard-failing getUserMedia.

test("audio-only constraints have video:false", () => {
  const c = buildCaptureConstraints("audio");
  assert.deepEqual(c, { audio: true, video: false });
});

test("video constraints carry the 1280/720/30 bounds", () => {
  const c = buildCaptureConstraints("video");
  assert.equal(c.audio, true);
  assert.deepEqual(c.video, {
    width: { ideal: 1280 },
    height: { ideal: 720 },
    frameRate: { ideal: 30, max: 30 },
  });
});

test("video constraints use ideal, never min/exact, so a weaker webcam still connects", () => {
  const c = buildCaptureConstraints("video");
  assert.ok("ideal" in c.video.width, "width must be ideal-bounded");
  assert.ok(!("min" in c.video.width) && !("exact" in c.video.width));
  assert.ok("ideal" in c.video.height, "height must be ideal-bounded");
  assert.ok(!("min" in c.video.height) && !("exact" in c.video.height));
  assert.ok("ideal" in c.video.frameRate, "frameRate must be ideal-bounded");
  assert.ok(!("exact" in c.video.frameRate));
});

test("audio is always requested and unconstrained, for both audio and video calls", () => {
  assert.equal(buildCaptureConstraints("audio").audio, true);
  assert.equal(buildCaptureConstraints("video").audio, true);
});

test("an unrecognized or missing media kind is treated as audio-only, mirroring the prior `media === \"video\"` check", () => {
  assert.equal(buildCaptureConstraints(undefined).video, false);
  assert.equal(buildCaptureConstraints(null).video, false);
  assert.equal(buildCaptureConstraints("").video, false);
  assert.equal(buildCaptureConstraints("screen").video, false);
});

test("buildCaptureConstraints returns a fresh object each call — mutating one result must not leak into the next", () => {
  const first = buildCaptureConstraints("video");
  first.video.width.ideal = 99999;
  first.audio = false;
  const second = buildCaptureConstraints("video");
  assert.equal(second.video.width.ideal, 1280, "second call must be unaffected by mutating the first's result");
  assert.equal(second.audio, true);
});

// ── buildVideoSenderParameters ───────────────────────────────────────────────
// WP17: cap the outgoing video RTCRtpSender's bitrate (~1.5 Mbps,
// docs/COMMS_ARCHITECTURE.md WP17) via
// `sender.setParameters(buildVideoSenderParameters(sender.getParameters(), cap))`.
// Real `RTCRtpSender.getParameters()` results vary in shape across webviews;
// these vectors cover missing, empty, and already-populated `encodings`.

test("sets maxBitrate when encodings is missing entirely", () => {
  const result = buildVideoSenderParameters({}, 1_500_000);
  assert.deepEqual(result.encodings, [{ maxBitrate: 1_500_000 }]);
});

test("sets maxBitrate when encodings is present but empty", () => {
  const result = buildVideoSenderParameters({ encodings: [] }, 1_500_000);
  assert.deepEqual(result.encodings, [{ maxBitrate: 1_500_000 }]);
});

test("sets maxBitrate on an already-populated single encoding, preserving its other fields", () => {
  const result = buildVideoSenderParameters(
    { encodings: [{ active: true, rid: "a" }] },
    1_500_000,
  );
  assert.deepEqual(result.encodings, [{ active: true, rid: "a", maxBitrate: 1_500_000 }]);
});

test("overwrites an existing maxBitrate on encodings[0] rather than leaving the old value", () => {
  const result = buildVideoSenderParameters({ encodings: [{ maxBitrate: 300_000 }] }, 1_500_000);
  assert.equal(result.encodings[0].maxBitrate, 1_500_000);
});

test("only encodings[0] is capped; later encodings (simulcast) are preserved untouched", () => {
  const result = buildVideoSenderParameters(
    { encodings: [{ rid: "lo" }, { rid: "hi", maxBitrate: 900_000 }] },
    1_500_000,
  );
  assert.deepEqual(result.encodings, [
    { rid: "lo", maxBitrate: 1_500_000 },
    { rid: "hi", maxBitrate: 900_000 },
  ]);
});

test("preserves other top-level params fields (e.g. transactionId), since some webviews require it to round-trip", () => {
  const result = buildVideoSenderParameters(
    { transactionId: "txn-1", codecs: ["vp8"], encodings: [{}] },
    1_500_000,
  );
  assert.equal(result.transactionId, "txn-1");
  assert.deepEqual(result.codecs, ["vp8"]);
});

test("treats a missing/nullish existingParams as empty rather than throwing", () => {
  assert.deepEqual(buildVideoSenderParameters(undefined, 1_500_000).encodings, [
    { maxBitrate: 1_500_000 },
  ]);
  assert.deepEqual(buildVideoSenderParameters(null, 1_500_000).encodings, [
    { maxBitrate: 1_500_000 },
  ]);
});

test("accepts any maxBitrate value — not hardcoded to a single constant", () => {
  assert.equal(buildVideoSenderParameters({}, 500_000).encodings[0].maxBitrate, 500_000);
  assert.equal(buildVideoSenderParameters({}, 2_000_000).encodings[0].maxBitrate, 2_000_000);
});

test("does not mutate the input — top level, encodings array, and encoding entries are all new", () => {
  const input = { transactionId: "txn-1", encodings: [{ active: true }] };
  const snapshot = JSON.parse(JSON.stringify(input));
  const result = buildVideoSenderParameters(input, 1_500_000);

  assert.deepEqual(input, snapshot, "input object must be unchanged");
  assert.notEqual(result, input, "must return a new top-level object");
  assert.notEqual(result.encodings, input.encodings, "must return a new encodings array");
  assert.notEqual(result.encodings[0], input.encodings[0], "must return a new encoding object");
});

test("does not mutate an input encodings entry even when it is untouched (index > 0)", () => {
  const secondEncoding = { rid: "hi" };
  const input = { encodings: [{ rid: "lo" }, secondEncoding] };
  const result = buildVideoSenderParameters(input, 1_500_000);

  assert.notEqual(result.encodings[1], secondEncoding, "even a copied-through entry must be a new object");
  assert.deepEqual(secondEncoding, { rid: "hi" }, "the original entry must be untouched");
});
