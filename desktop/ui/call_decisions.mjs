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
