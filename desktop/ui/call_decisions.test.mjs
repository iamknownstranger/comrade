// Node's built-in test runner + assert — no package.json, no npm deps (see
// docs/COMMS_ARCHITECTURE.md §ADR-3: "Node ≥ 20 ships the built-in runner").
// Run with: node --test desktop/ui/*.test.mjs
// (a bare directory argument, e.g. `node --test desktop/ui/`, does NOT work
// on Node 22 — it tries to `require()` the directory itself and fails with
// MODULE_NOT_FOUND; a glob or explicit file list is required. See the CI
// job `desktop-js` in .github/workflows/ci.yml, which uses this same form.)
import { test } from "node:test";
import assert from "node:assert/strict";

import { OFFER_DISPOSITION, decideOfferDisposition, isEndedCallId, rememberEndedCall } from "./call_decisions.mjs";

// ── decideOfferDisposition ───────────────────────────────────────────────────
// Vectors ported from android/app/src/test/java/mullu/comrade/call/
// CallManagerTest.kt's `decideOfferForExistingSession`/`isOfferForEndedCall`
// cases, plus the two branches (NEW_INCOMING, ENDED_NOOP-while-busy) desktop
// folds into the same decision. These are the seed of the ADR-2 conformance
// suite (docs/COMMS_ARCHITECTURE.md §ADR-2) — keep them in lockstep with the
// Kotlin vectors.

test("re-offer with same call_id during an active call is RENEGOTIATE, never BUSY", () => {
  // The P0 regression this WP fixes: main.js used to Busy ANY offer while a
  // call existed (main.js:996, reached from onCallSignal before the callId
  // guard at :1096), which rejected an Android caller's same-call_id TURN
  // ICE-restart re-offer and broke cross-platform TURN fallback.
  const disposition = decideOfferDisposition({
    hasCall: true,
    sameCallId: true,
    hasPc: true,
    isEndedCallId: false,
  });
  assert.equal(disposition, OFFER_DISPOSITION.RENEGOTIATE);
  assert.notEqual(disposition, OFFER_DISPOSITION.BUSY);
});

test("decideOfferForExistingSession renegotiates the same call once a pc exists (ported)", () => {
  assert.equal(
    decideOfferDisposition({ hasCall: true, sameCallId: true, hasPc: true, isEndedCallId: false }),
    OFFER_DISPOSITION.RENEGOTIATE,
  );
});

test("decideOfferForExistingSession no-ops a same-call duplicate offer received pre-accept (ported)", () => {
  assert.equal(
    decideOfferDisposition({ hasCall: true, sameCallId: true, hasPc: false, isEndedCallId: false }),
    OFFER_DISPOSITION.DUPLICATE_NOOP,
  );
});

test("decideOfferForExistingSession treats a different call id as busy regardless of pc state (ported)", () => {
  assert.equal(
    decideOfferDisposition({ hasCall: true, sameCallId: false, hasPc: true, isEndedCallId: false }),
    OFFER_DISPOSITION.BUSY,
  );
  assert.equal(
    decideOfferDisposition({ hasCall: true, sameCallId: false, hasPc: false, isEndedCallId: false }),
    OFFER_DISPOSITION.BUSY,
  );
});

test("no live call: a fresh offer is NEW_INCOMING regardless of sameCallId/hasPc", () => {
  assert.equal(
    decideOfferDisposition({ hasCall: false, sameCallId: false, hasPc: false, isEndedCallId: false }),
    OFFER_DISPOSITION.NEW_INCOMING,
  );
  // hasCall: false makes sameCallId/hasPc meaningless — must not leak into BUSY/RENEGOTIATE.
  assert.equal(
    decideOfferDisposition({ hasCall: false, sameCallId: true, hasPc: true, isEndedCallId: false }),
    OFFER_DISPOSITION.NEW_INCOMING,
  );
});

test("an ended call_id is dropped even while a different call is live", () => {
  assert.equal(
    decideOfferDisposition({ hasCall: true, sameCallId: false, hasPc: true, isEndedCallId: true }),
    OFFER_DISPOSITION.ENDED_NOOP,
  );
});

test("an ended call_id is dropped even for what would otherwise be a fresh incoming call", () => {
  // Mirrors onIncomingSignal's ordering (CallManager.kt:433): the ended check
  // runs before any session-based dispatch, so it wins even with no live call.
  assert.equal(
    decideOfferDisposition({ hasCall: false, sameCallId: false, hasPc: false, isEndedCallId: true }),
    OFFER_DISPOSITION.ENDED_NOOP,
  );
});

test("an ended call_id wins even over what would otherwise be a renegotiation", () => {
  assert.equal(
    decideOfferDisposition({ hasCall: true, sameCallId: true, hasPc: true, isEndedCallId: true }),
    OFFER_DISPOSITION.ENDED_NOOP,
  );
});

// ── isEndedCallId ─────────────────────────────────────────────────────────────
// Ported from CallManagerTest.kt's
// `isOfferForEndedCall drops an ended-id offer and rings a fresh one`.

test("isEndedCallId drops an ended-id offer and rings a fresh one (ported)", () => {
  const ended = ["call-1", "call-2"];
  assert.equal(isEndedCallId(ended, "call-1"), true);
  assert.equal(isEndedCallId(ended, "call-3"), false, "a callId not in the ended set must still ring");
  assert.equal(isEndedCallId([], "call-1"), false);
});

// ── rememberEndedCall ─────────────────────────────────────────────────────────
// Bounded insertion-order memory mirroring Android's `endedCallIds`
// ArrayDeque + `rememberEnded` (CallManager.kt:238-245, cap 32).

test("rememberEndedCall remembers a fresh id", () => {
  assert.deepEqual(rememberEndedCall([], "call-1"), ["call-1"]);
});

test("rememberEndedCall preserves insertion order across multiple calls", () => {
  let ended = [];
  ended = rememberEndedCall(ended, "call-1");
  ended = rememberEndedCall(ended, "call-2");
  ended = rememberEndedCall(ended, "call-3");
  assert.deepEqual(ended, ["call-1", "call-2", "call-3"]);
});

test("rememberEndedCall evicts the single oldest id once past cap", () => {
  const ended = rememberEndedCall(["call-1", "call-2", "call-3"], "call-4", 3);
  assert.deepEqual(ended, ["call-2", "call-3", "call-4"]);
});

test("rememberEndedCall evicts however many ids are needed to get back under cap", () => {
  // Not reachable through normal one-at-a-time calls with a fixed cap, but
  // the eviction loop (mirroring Android's `while (size > CAP) removeFirst()`)
  // must not leave the result over cap no matter how it got there.
  const ended = rememberEndedCall(["call-1", "call-2", "call-3", "call-4"], "call-5", 2);
  assert.deepEqual(ended, ["call-4", "call-5"]);
});

test("rememberEndedCall defaults its cap to 32, mirroring Android's ENDED_CALL_IDS_CAP", () => {
  let ended = [];
  for (let i = 0; i < 40; i++) ended = rememberEndedCall(ended, `call-${i}`);
  assert.equal(ended.length, 32);
  assert.equal(ended[0], "call-8"); // call-0..call-7 (8 ids) evicted to get back to 32
  assert.equal(ended[ended.length - 1], "call-39");
});

test("rememberEndedCall ignores a blank or nullish call id, mirroring callId.isEmpty()", () => {
  assert.deepEqual(rememberEndedCall(["call-1"], ""), ["call-1"]);
  assert.deepEqual(rememberEndedCall(["call-1"], undefined), ["call-1"]);
  assert.deepEqual(rememberEndedCall(["call-1"], null), ["call-1"]);
});

test("rememberEndedCall never mutates its input", () => {
  const original = ["call-1", "call-2"];
  const snapshot = original.slice();
  const result = rememberEndedCall(original, "call-3");
  assert.deepEqual(original, snapshot, "input array must be unchanged");
  assert.notEqual(result, original, "must return a new array, not the mutated input");
});

test("rememberEndedCall accepts any iterable, not just arrays (mirrors Collection<String>)", () => {
  const ended = rememberEndedCall(new Set(["call-1", "call-2"]), "call-3");
  assert.deepEqual(ended, ["call-1", "call-2", "call-3"]);
});
