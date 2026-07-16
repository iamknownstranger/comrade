// Node's built-in test runner + assert — no package.json, no npm deps (see
// docs/COMMS_ARCHITECTURE.md §ADR-3: "Node ≥ 20 ships the built-in runner").
// Run with: node --test desktop/ui/*.test.mjs
// (a bare directory argument, e.g. `node --test desktop/ui/`, does NOT work
// on Node 22 — it tries to `require()` the directory itself and fails with
// MODULE_NOT_FOUND; a glob or explicit file list is required. See the CI
// job `desktop-js` in .github/workflows/ci.yml, which uses this same form.)
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  OFFER_DISPOSITION,
  decideOfferDisposition,
  isEndedCallId,
  rememberEndedCall,
  CONNECTION_ACTION,
  decideConnectionStateAction,
  ANSWER_DECISION,
  decideAnswer,
  shouldConnectTimeoutFire,
  formatSas,
} from "./call_decisions.mjs";

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

// ── decideConnectionStateAction ───────────────────────────────────────────────
// Vectors ported from CallManagerTest.kt's decideConnectionStateAction cases
// (:157-219), extended with the caller/callee + already-tried splits Android
// reaches one layer down in tryTurnFallbackOrFail (CallManager.kt:1063-1097) —
// desktop folds both into this one pure function (WP3). These are part of the
// ADR-2 conformance suite (docs/COMMS_ARCHITECTURE.md §ADR-2); keep them in
// lockstep with the Kotlin vectors.

test("pre-connect failed as the caller, TURN not yet tried, restarts with TURN (RESTART_WITH_TURN)", () => {
  // The WP3 P0 fix: main.js used to treat any "failed" as terminal (finishCall)
  // with no TURN fallback and no ICE restart, so a desktop caller behind CGNAT
  // could never relay. Android: decideConnectionStateAction(FAILED,
  // hasConnectedBefore=false) -> TRY_TURN_FALLBACK, then tryTurnFallbackOrFail
  // (caller, !triedTurn) widens to STUN+TURN and re-offers with an ICE restart.
  assert.equal(
    decideConnectionStateAction({
      connectionState: "failed",
      hasConnectedBefore: false,
      isCaller: true,
      triedTurn: false,
    }),
    CONNECTION_ACTION.RESTART_WITH_TURN,
  );
});

test("pre-connect failed as the caller with TURN already tried is terminal (FAIL), never a second restart", () => {
  // Regression vector for "already-tried fallback must not loop"
  // (docs/COMMS_ARCHITECTURE.md §6). Android: tryTurnFallbackOrFail with
  // s.triedTurn -> endWith(FAILED) (CallManager.kt:1069-1072).
  assert.equal(
    decideConnectionStateAction({
      connectionState: "failed",
      hasConnectedBefore: false,
      isCaller: true,
      triedTurn: true,
    }),
    CONNECTION_ACTION.FAIL,
  );
});

test("pre-connect failed as the callee WAITs for the caller's re-offer — no callee-side TURN retry", () => {
  // Android deliberately has no callee TURN retry: tryTurnFallbackOrFail returns
  // early when s.incoming (CallManager.kt:1065-1068), waiting for the caller's
  // rescue re-offer (backstopped by the connect timeout; see the comment at
  // :1051-1057 on why the callee must not Hangup on its own FAILED). triedTurn
  // is irrelevant on the callee.
  assert.equal(
    decideConnectionStateAction({
      connectionState: "failed",
      hasConnectedBefore: false,
      isCaller: false,
      triedTurn: false,
    }),
    CONNECTION_ACTION.WAIT,
  );
  assert.equal(
    decideConnectionStateAction({
      connectionState: "failed",
      hasConnectedBefore: false,
      isCaller: false,
      triedTurn: true,
    }),
    CONNECTION_ACTION.WAIT,
  );
});

test("a previously-connected FAILED arms immediate recovery (RECOVER_NOW), caller and callee alike (ported)", () => {
  // Ported from CallManagerTest.kt:157 — hasConnectedBefore short-circuits
  // before isCaller/triedTurn are ever consulted, so a post-connect failure is
  // RECOVER_NOW no matter who placed the call or whether TURN was tried.
  for (const isCaller of [true, false]) {
    for (const triedTurn of [true, false]) {
      assert.equal(
        decideConnectionStateAction({
          connectionState: "failed",
          hasConnectedBefore: true,
          isCaller,
          triedTurn,
        }),
        CONNECTION_ACTION.RECOVER_NOW,
      );
    }
  }
});

test("a previously-connected DISCONNECTED starts the disconnect grace (RECOVER_AFTER_GRACE) (ported)", () => {
  // Ported from CallManagerTest.kt:176.
  assert.equal(
    decideConnectionStateAction({
      connectionState: "disconnected",
      hasConnectedBefore: true,
      isCaller: true,
      triedTurn: false,
    }),
    CONNECTION_ACTION.RECOVER_AFTER_GRACE,
  );
});

test("a pre-connect DISCONNECTED is not a failure worth acting on (NONE) (ported)", () => {
  // Ported from CallManagerTest.kt:184.
  assert.equal(
    decideConnectionStateAction({
      connectionState: "disconnected",
      hasConnectedBefore: false,
      isCaller: true,
      triedTurn: false,
    }),
    CONNECTION_ACTION.NONE,
  );
});

test("decideConnectionStateAction is a no-op for every other connection state (ported)", () => {
  // Ported from CallManagerTest.kt:204. The browser's
  // RTCPeerConnection.connectionState values are new/connecting/connected/
  // disconnected/failed/closed; "connected" is handled by onCallConnected in
  // main.js, not the decider (as Android handles CONNECTED in peerObserver).
  for (const connectionState of ["new", "connecting", "connected", "closed"]) {
    for (const hasConnectedBefore of [true, false]) {
      assert.equal(
        decideConnectionStateAction({
          connectionState,
          hasConnectedBefore,
          isCaller: true,
          triedTurn: false,
        }),
        CONNECTION_ACTION.NONE,
      );
    }
  }
});

// ── decideAnswer ──────────────────────────────────────────────────────────────
// Vectors ported from CallManagerTest.kt's decideAnswer cases (:75-97), with the
// browser's lowercase-hyphenated RTCSignalingState strings standing in for
// Android's PeerConnection.SignalingState enum.

test("decideAnswer applies only in have-local-offer — a fresh answer rings through (ported)", () => {
  assert.equal(decideAnswer("have-local-offer"), ANSWER_DECISION.APPLY);
});

test("decideAnswer ignores a duplicate answer once the pc has settled to stable (ported)", () => {
  // STABLE is exactly the state a pc settles into right after the first Answer
  // applies — a redelivered second Answer must not re-apply and tear the live
  // call down (CallManagerTest.kt:84).
  assert.equal(decideAnswer("stable"), ANSWER_DECISION.IGNORE);
});

test("decideAnswer ignores a null/undefined signaling state — no pc yet (ported)", () => {
  assert.equal(decideAnswer(null), ANSWER_DECISION.IGNORE);
  assert.equal(decideAnswer(undefined), ANSWER_DECISION.IGNORE);
});

test("second answer after a STUN->TURN ICE-restart re-offer is APPLYed, not dropped (fallback regression)", () => {
  // Regression vector for the fallback's second answer: after the ICE-restart
  // re-offer's setLocalDescription the caller's pc is back in "have-local-offer",
  // so the peer's SECOND answer must apply. main.js keys applyRemoteAnswer off
  // signalingState (decideAnswer), never a one-shot latch like c.remoteSet — a
  // latch would silently drop this answer and the TURN fallback would hang.
  assert.equal(decideAnswer("have-local-offer"), ANSWER_DECISION.APPLY);
  // ...while every non-offer state stays IGNORE, so a redelivered duplicate
  // answer can't throw in setRemoteDescription and terminate a live call.
  assert.equal(decideAnswer("have-remote-offer"), ANSWER_DECISION.IGNORE);
  assert.equal(decideAnswer("closed"), ANSWER_DECISION.IGNORE);
});

// ── shouldConnectTimeoutFire ──────────────────────────────────────────────────
// Mirrors the guard inside Android's armTimeout coroutine body
// (CallManager.kt:1586, `session === s && !s.ended && s.connectedAtMs == 0L`).
// This backs desktop's connect-phase timeout (WP3, the CONNECT_TIMEOUT_MS
// mirror) and pins the zombie-timer + double-finish guards the lead review
// asked to close: the callee's WAIT-on-failed is only safe with this backstop.

test("shouldConnectTimeoutFire fires only for the current, un-ended, never-connected call", () => {
  assert.equal(
    shouldConnectTimeoutFire({ isCurrentCall: true, ended: false, connected: false }),
    true,
  );
});

test("shouldConnectTimeoutFire does not fire once the call has connected (would kill a live call)", () => {
  // onCallConnected clears the timer, but a timer that already fired must still
  // no-op — mirrors Android's `s.connectedAtMs == 0L` guard.
  assert.equal(
    shouldConnectTimeoutFire({ isCurrentCall: true, ended: false, connected: true }),
    false,
  );
});

test("shouldConnectTimeoutFire does not fire after the call ended (double-finish / timeout-vs-hangup race)", () => {
  // A Hangup that won the race already ran finishCall (which sets c.ended and
  // clears the timer); a timeout that fires anyway must not double-finish.
  assert.equal(
    shouldConnectTimeoutFire({ isCurrentCall: true, ended: true, connected: false }),
    false,
  );
});

test("shouldConnectTimeoutFire does not fire for a superseded call (zombie-timer guard)", () => {
  // A stray timer armed for an earlier call must never act on the call that
  // replaced it — mirrors Android's `session === s`.
  assert.equal(
    shouldConnectTimeoutFire({ isCurrentCall: false, ended: false, connected: false }),
    false,
  );
});

// ── formatSas ─────────────────────────────────────────────────────────────
// The WP4 SAS UI: formatSas mirrors the honest-none contract of
// comrade_ui::ComradeRuntime::call_sas (runtime.rs:1258-1270) / the Tauri
// call_sas command — None/null means one side's SDP had no
// `a=fingerprint:` line, an honest "can't verify", not an error.

test("formatSas joins a real 4-emoji SAS with spaces (happy path)", () => {
  assert.equal(formatSas(["🐶", "🦊", "🐝", "🐳"]), "🐶 🦊 🐝 🐳");
});

test("formatSas returns null for a null SAS — no fingerprint on one side, honest can't-verify (required)", () => {
  // The required "no fingerprint -> no SAS" branch (docs/COMMS_ARCHITECTURE.md
  // WP4 acceptance criteria): call_sas resolves to None/null when either SDP
  // lacks an a=fingerprint: line, and that must format to null, never a
  // fabricated or partial code.
  assert.equal(formatSas(null), null);
});

test("formatSas returns null for undefined (not yet derived / older backend)", () => {
  assert.equal(formatSas(undefined), null);
});

test("formatSas returns null for an empty array", () => {
  assert.equal(formatSas([]), null);
});

test("formatSas returns null for anything other than exactly 4 emoji (never trust a partial code)", () => {
  assert.equal(formatSas(["🐶", "🦊"]), null);
  assert.equal(formatSas(["🐶", "🦊", "🐝", "🐳", "🐧"]), null);
});

test("formatSas returns null for a non-array input", () => {
  assert.equal(formatSas("🐶 🦊 🐝 🐳"), null);
  assert.equal(formatSas(42), null);
});
