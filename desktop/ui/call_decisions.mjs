/* ============================================================================
 * Comrade desktop — pure call-signal decision helpers
 *
 * This module holds the decisions `desktop/ui/main.js` used to get wrong: it
 * answered `busy` to *any* offer that arrived while a call existed, even one
 * for the exact call already in progress. That rejects the same-`call_id`
 * re-offer an Android caller sends when it falls back from STUN to TURN via
 * ICE-restart, which breaks cross-platform calling (see
 * docs/COMMS_ARCHITECTURE.md §ADR-3, WP1).
 *
 * Every function here mirrors the pure decision functions in Android's
 * `CallManager.kt` (`decideOfferForExistingSession`, `isOfferForEndedCall`,
 * the bounded `endedCallIds` deque + `rememberEnded`) — see
 * android/app/src/main/java/mullu/comrade/call/CallManager.kt:1629-1671 and
 * :238-245. Desktop and Android are still two separately-maintained call
 * state machines (docs/COMMS_ARCHITECTURE.md §ADR-2), but this file is the
 * seed of the shared conformance suite: `call_decisions.test.mjs` ports the
 * vectors from `CallManagerTest.kt` verbatim, so drift between the two
 * implementations becomes a test failure here, not a field bug, and the same
 * vectors are meant to keep passing unmodified if/when this logic is later
 * lifted into a shared Rust module (§ADR-2 Wave 4 / WP15).
 *
 * Deliberately pure and dependency-free: no DOM, no WebRTC, no Tauri, no
 * Node built-ins, no imports at all. That is what makes it runnable under
 * `node --test` with zero npm dependencies (§ADR-3's testability decision) —
 * keep it that way; put any DOM/WebRTC glue in main.js instead.
 * ========================================================================== */

/**
 * The five outcomes for an incoming call `offer` signal. Mirrors Android's
 * `CallManager.OfferDecision` (`RENEGOTIATE`/`DUPLICATE_NOOP`/`BUSY`,
 * CallManager.kt:1655) plus two cases Android reaches one layer up, folded
 * in here so desktop has a single decision to switch on:
 *  - `ENDED_NOOP` is Android's `isOfferForEndedCall` short-circuit in
 *    `onIncomingSignal` (CallManager.kt:433), checked before dispatch.
 *  - `NEW_INCOMING` is Android's `handleRemoteOffer`'s `existing == null`
 *    branch (CallManager.kt:880), i.e. no session yet — ring as usual.
 * @type {Readonly<Record<string, string>>}
 */
export const OFFER_DISPOSITION = Object.freeze({
  /** A redelivered offer for a call_id we already tore down — drop silently, never ring again. */
  ENDED_NOOP: "ENDED_NOOP",
  /** Same call_id, and a peer connection already exists — answer on the existing pc (e.g. the caller's TURN ICE-restart re-offer). */
  RENEGOTIATE: "RENEGOTIATE",
  /** Same call_id, but no peer connection yet (still ringing, pre-accept) — a redelivered duplicate of the offer we're already handling. */
  DUPLICATE_NOOP: "DUPLICATE_NOOP",
  /** A different call_id while a call is already live — reject with `busy`. */
  BUSY: "BUSY",
  /** No live call at all — this is a brand-new incoming call; ring it. */
  NEW_INCOMING: "NEW_INCOMING",
});

/**
 * Decide how to handle an incoming `offer` signal.
 *
 * Mirrors, in priority order, Android's `CallManager`:
 *  1. `isOfferForEndedCall` (CallManager.kt:1629) — checked *first* and
 *     unconditionally, exactly as `onIncomingSignal` (CallManager.kt:433)
 *     does before it even looks at whether a session exists. An offer whose
 *     `call_id` is in the ended-call memory is always dropped.
 *  2. `handleRemoteOffer`'s `existing == null` branch (CallManager.kt:880) —
 *     no live call at all means this is a fresh incoming call.
 *  3. `decideOfferForExistingSession` (CallManager.kt:1663-1671) — same
 *     `call_id` **and** an existing peer connection is a renegotiation
 *     (e.g. a same-`call_id` re-offer from the caller's STUN→TURN
 *     ICE-restart fallback); same `call_id` with no peer connection yet is a
 *     pre-accept duplicate; any other `call_id` while a call is live is
 *     `busy`.
 *
 * This is the P0 fix from docs/COMMS_ARCHITECTURE.md §ADR-3/WP1: the desktop
 * bug was answering `BUSY` for case 3's same-`call_id` branch instead of
 * `RENEGOTIATE`.
 *
 * @param {object} input
 * @param {boolean} input.hasCall - Whether a call session currently exists (desktop: `!!state.call`; Android: `session != null`).
 * @param {boolean} input.sameCallId - Whether the incoming offer's `call_id` equals the existing session's `call_id`. Meaningless (ignored) when `hasCall` is false.
 * @param {boolean} input.hasPc - Whether the existing session already has a live RTCPeerConnection (desktop: `!!state.call.pc`; Android: `existing.pc != null`). Meaningless (ignored) when `hasCall` is false.
 * @param {boolean} input.isEndedCallId - Whether the offer's `call_id` is already in the bounded ended-call memory (see `isEndedCallId`/`rememberEndedCall` below). Checked first, independent of `hasCall`.
 * @returns {"ENDED_NOOP"|"RENEGOTIATE"|"DUPLICATE_NOOP"|"BUSY"|"NEW_INCOMING"} One of `OFFER_DISPOSITION`'s values.
 */
export function decideOfferDisposition({ hasCall, sameCallId, hasPc, isEndedCallId }) {
  if (isEndedCallId) return OFFER_DISPOSITION.ENDED_NOOP;
  if (!hasCall) return OFFER_DISPOSITION.NEW_INCOMING;
  if (sameCallId) return hasPc ? OFFER_DISPOSITION.RENEGOTIATE : OFFER_DISPOSITION.DUPLICATE_NOOP;
  return OFFER_DISPOSITION.BUSY;
}

/**
 * Whether `callId` is already in the bounded ended-call memory produced by
 * `rememberEndedCall`. Mirrors Android's `isOfferForEndedCall`
 * (CallManager.kt:1629), a one-line `callId in endedCallIds` membership test
 * over the `ArrayDeque` `endedCallIds` (CallManager.kt:238).
 *
 * @param {Iterable<string>} endedIds
 * @param {string} callId
 * @returns {boolean}
 */
export function isEndedCallId(endedIds, callId) {
  for (const id of endedIds) if (id === callId) return true;
  return false;
}

/**
 * Record `callId` as ended. Mirrors Android's bounded `endedCallIds`
 * `ArrayDeque` + `rememberEnded` (CallManager.kt:238-245): insertion-order
 * memory, capped at `cap` (Android hardcodes `ENDED_CALL_IDS_CAP = 32`,
 * CallManager.kt:136, which is this function's default), oldest evicted
 * first once at capacity. A blank/nullish `callId` is not worth
 * remembering — mirrors Android's `if (callId.isEmpty()) return`, guarding
 * a provisional call that never got a real id.
 *
 * Purity contract: this function never mutates `endedIds` — it always
 * returns a **new** array (a copy of `endedIds`, with `callId` appended and,
 * if that pushes the length past `cap`, the oldest entries dropped from the
 * front). Callers must assign the result back, e.g.:
 *   `state.endedCallIds = rememberEndedCall(state.endedCallIds, callId);`
 * — there is no in-place mutation to rely on.
 *
 * @param {Iterable<string>} endedIds
 * @param {string} callId
 * @param {number} [cap]
 * @returns {string[]}
 */
export function rememberEndedCall(endedIds, callId, cap = 32) {
  const next = Array.from(endedIds);
  if (!callId) return next;
  next.push(callId);
  while (next.length > cap) next.shift();
  return next;
}

/**
 * What a peer-connection `connectionstatechange` should trigger. This folds
 * *two* of Android's decisions into one, because desktop wires a single
 * connection-state handler where Android has a two-stage split:
 *  - Android's `decideConnectionStateAction` (CallManager.kt:1695-1704) maps
 *    `(newState, hasConnectedBefore)` → `NONE`/`RECOVER_NOW`/
 *    `RECOVER_AFTER_GRACE`/`TRY_TURN_FALLBACK`, and then
 *  - `tryTurnFallbackOrFail` (CallManager.kt:1063-1097) resolves that
 *    `TRY_TURN_FALLBACK` further using the `Session`'s `incoming`/`triedTurn`.
 * Desktop has no reason to reproduce the split, so this one function takes all
 * four inputs and returns the final action. See docs/COMMS_ARCHITECTURE.md
 * §ADR-3 / WP3.
 *
 *  - `RESTART_WITH_TURN` — pre-connect `failed`, we're the caller, TURN not yet
 *    tried: widen to STUN+TURN and re-offer with an ICE restart (Android:
 *    `tryTurnFallbackOrFail` sets `triedTurn` and re-offers, CallManager.kt:1073-1094).
 *  - `FAIL` — pre-connect `failed`, we're the caller, TURN already tried:
 *    terminal (Android: `endWith(FAILED)`, CallManager.kt:1069-1072). This is
 *    the "already-tried fallback must not loop" regression guard (§6).
 *  - `WAIT` — pre-connect `failed` as the callee: do nothing and wait for the
 *    caller's rescue re-offer. Android deliberately has *no* callee-side TURN
 *    retry (CallManager.kt:1065-1068) — the comment at :1051-1057 explains why
 *    a callee must not Hangup the instant its own ICE agent reports FAILED — and
 *    relies on the connect timeout armed at accept as the backstop.
 *  - `RECOVER_NOW` — a *post*-connect `failed`: an already-Active call's media
 *    path dropped (Android arms an immediate recovery countdown, `RECOVER_NOW`,
 *    CallManager.kt:1699-1700).
 *  - `RECOVER_AFTER_GRACE` — a *post*-connect `disconnected`: maybe a transient
 *    blip or an ICE restart in flight; tolerate it for a grace period before the
 *    recovery countdown (Android `RECOVER_AFTER_GRACE`, CallManager.kt:1701-1702).
 *  - `NONE` — nothing to do: pre-connect `disconnected` (Android `NONE`,
 *    CallManager.kt:1702) and every other state (`new`/`connecting`/`connected`/
 *    `closed`). `connected` itself is handled directly by main.js's
 *    `onCallConnected`, not here — exactly as Android handles CONNECTED in
 *    `peerObserver` rather than in the decider (CallManager.kt:1825-1828).
 *
 * @type {Readonly<Record<string, string>>}
 */
export const CONNECTION_ACTION = Object.freeze({
  NONE: "NONE",
  RESTART_WITH_TURN: "RESTART_WITH_TURN",
  FAIL: "FAIL",
  WAIT: "WAIT",
  RECOVER_NOW: "RECOVER_NOW",
  RECOVER_AFTER_GRACE: "RECOVER_AFTER_GRACE",
});

/**
 * Decide what a peer-connection state change means for the call. Mirrors
 * Android's `decideConnectionStateAction` (CallManager.kt:1695) folded with
 * `tryTurnFallbackOrFail`'s caller/callee/already-tried branching
 * (CallManager.kt:1063-1097).
 *
 * `hasConnectedBefore` is the pre-/post-connect split (Android's
 * `s.connectedAtMs > 0`). `isCaller`/`triedTurn` are consulted **only** for a
 * pre-connect `failed` — every other branch ignores them, exactly as Android
 * only reaches `tryTurnFallbackOrFail` for a pre-connect FAILED.
 *
 * @param {object} input
 * @param {string} input.connectionState - `RTCPeerConnection.connectionState`: one of `"new"|"connecting"|"connected"|"disconnected"|"failed"|"closed"`.
 * @param {boolean} input.hasConnectedBefore - Whether this call reached `"connected"` at least once (desktop: `state.call.connected`; Android: `s.connectedAtMs > 0`).
 * @param {boolean} input.isCaller - Whether this side placed the call (desktop: `!state.call.incoming`; Android: `!s.incoming`). Only consulted for a pre-connect `"failed"`.
 * @param {boolean} input.triedTurn - Whether the caller already widened to TURN and re-offered once (one-shot). Only consulted for a pre-connect `"failed"`.
 * @returns {"NONE"|"RESTART_WITH_TURN"|"FAIL"|"WAIT"|"RECOVER_NOW"|"RECOVER_AFTER_GRACE"} One of `CONNECTION_ACTION`'s values.
 */
export function decideConnectionStateAction({ connectionState, hasConnectedBefore, isCaller, triedTurn }) {
  if (connectionState === "failed") {
    if (hasConnectedBefore) return CONNECTION_ACTION.RECOVER_NOW;
    if (!isCaller) return CONNECTION_ACTION.WAIT;
    return triedTurn ? CONNECTION_ACTION.FAIL : CONNECTION_ACTION.RESTART_WITH_TURN;
  }
  if (connectionState === "disconnected") {
    return hasConnectedBefore ? CONNECTION_ACTION.RECOVER_AFTER_GRACE : CONNECTION_ACTION.NONE;
  }
  return CONNECTION_ACTION.NONE;
}

/**
 * The two outcomes for an incoming `answer` signal. Mirrors Android's
 * `CallManager.AnswerDecision` (CallManager.kt:1640).
 * @type {Readonly<Record<string, string>>}
 */
export const ANSWER_DECISION = Object.freeze({
  APPLY: "APPLY",
  IGNORE: "IGNORE",
});

/**
 * Whether to apply an incoming `answer`, given the pc's current signaling
 * state. Mirrors Android's `decideAnswer` (CallManager.kt:1648): an `answer` is
 * only meaningful while we're the caller still holding an unanswered local
 * offer (`have-local-offer`). Applying one in any other state — a redelivered
 * duplicate that arrives once the pc has settled to `stable`, or one with no pc
 * (`null`) — makes `setRemoteDescription` throw and tears the live call down.
 *
 * This is precisely what makes the STUN→TURN fallback's **second** answer work:
 * after the ICE-restart re-offer's `setLocalDescription`, the caller's pc is
 * back in `have-local-offer`, so the peer's second answer applies. A one-shot
 * flag (e.g. `c.remoteSet`) would instead drop that legitimate second answer
 * and the fallback would hang — so keep the answer path keyed off
 * `signalingState`, never a latch.
 *
 * Accepts the browser's lowercase-hyphenated `RTCSignalingState` strings
 * (`"stable"`, `"have-local-offer"`, …) and `null`/`undefined` (no pc yet).
 *
 * @param {string|null|undefined} signalingState - `RTCPeerConnection.signalingState`.
 * @returns {"APPLY"|"IGNORE"} One of `ANSWER_DECISION`'s values.
 */
export function decideAnswer(signalingState) {
  return signalingState === "have-local-offer" ? ANSWER_DECISION.APPLY : ANSWER_DECISION.IGNORE;
}

/**
 * Whether a fired connect-phase timeout should actually end the call. Mirrors
 * the guard inside Android's `armTimeout` coroutine body (CallManager.kt:1586):
 * `session === s && !s.ended && s.connectedAtMs == 0L`. A timeout that survives
 * to fire must be ignored if, by the time it runs, the call it was armed for is
 * no longer the current one (a *later* call replaced it — the zombie-timer
 * hazard), has already ended (a `Hangup` won the race — the double-finish
 * hazard), or has since connected. Keeping this a pure predicate turns those
 * three guards into a conformance vector instead of a comment.
 *
 * The connect timeout itself (arming/clearing via `setTimeout`/`clearTimeout`,
 * the 30s duration mirroring Android's `CONNECT_TIMEOUT_MS`) is timer glue and
 * lives in main.js; only this decision is pure.
 *
 * @param {object} input
 * @param {boolean} input.isCurrentCall - Whether the call the timeout was armed for is still the live one (desktop: `state.call === c`; Android: `session === s`).
 * @param {boolean} input.ended - Whether that call has already been torn down (desktop: `c.ended`; Android: `s.ended`).
 * @param {boolean} input.connected - Whether that call has reached "connected" (desktop: `c.connected`; Android: `s.connectedAtMs != 0`).
 * @returns {boolean} `true` only when the call is still current, not ended, and never connected.
 */
export function shouldConnectTimeoutFire({ isCurrentCall, ended, connected }) {
  return !!isCurrentCall && !ended && !connected;
}
