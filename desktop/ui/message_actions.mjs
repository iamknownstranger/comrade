/**
 * The long-press/right-click action set for one message, and the contextual
 * set for a multi-select — the union of what WhatsApp and Telegram offer, cut
 * down to what Comrade can actually do.
 *
 * Mirrors `android/app/src/main/java/mullu/comrade/ui/MessageActions.kt`
 * exactly: same action names (as strings — JS has no enum), same order, same
 * windows, same "all not any" rule for the bulk actions. `message_actions.test.mjs`
 * ports `MessageActionsTest.kt`'s vectors, so drift between the two becomes a
 * failing test here rather than a field bug.
 *
 * Deliberately pure and dependency-free — no DOM, no Tauri, nothing that
 * reads a clock (the caller measures age once and passes `ageMs`, the same
 * discipline `call_decisions.mjs` and `chat_thread.mjs` already use). Labels
 * are not here either: the caller resolves the string, same division
 * `chat_commands.mjs` already draws between "what to do" and "what to say".
 */

// ── Action names ─────────────────────────────────────────────────────────

/** Quick row + "more" — whichever emoji sit in the quick row is the caller's
 * decision, and there is no further rule to encode for it here: whether a
 * message is eligible for a menu *at all* is decided before this module ever
 * runs. */
export const REACT = "react";
/** Quote this message in the composer. */
export const REPLY = "reply";
/** Open (or start) the thread this message belongs to. */
export const REPLY_IN_THREAD = "reply_in_thread";
/** Send a copy of this message's content to another chat. */
export const FORWARD = "forward";
/** Pin. Appears only when {@link UNPIN} does not — see {@link messageActions}. */
export const PIN = "pin";
/** Unpin. Appears only when {@link PIN} does not. */
export const UNPIN = "unpin";
/** Star. Appears only when {@link UNSTAR} does not. */
export const STAR = "star";
/** Unstar. Appears only when {@link STAR} does not. */
export const UNSTAR = "unstar";
/** File the thread this message belongs to under a topic. */
export const ASSIGN_TOPIC = "assign_topic";
/** Copy the text to the clipboard. Text messages only. */
export const COPY = "copy";
/** Rewrite this message's text. Own text messages, inside {@link EDIT_WINDOW_MS}. */
export const EDIT = "edit";
/** Enter multi-select, starting from this message. */
export const SELECT = "select";
/** Hand this message's content to another app/window. */
export const SHARE = "share";
/** Write this message's media to disk. */
export const SAVE_MEDIA = "save_media";
/** Delivery/read metadata for this one message. */
export const MESSAGE_INFO = "message_info";
/** Flag this message for review. Incoming only. */
export const REPORT = "report";
/** Remove this message from this device's own view. Always available. */
export const DELETE_FOR_ME = "delete_for_me";
/** Remove this message everywhere. Own messages, inside
 * {@link DELETE_FOR_EVERYONE_WINDOW_MS}. */
export const DELETE_FOR_EVERYONE = "delete_for_everyone";

/**
 * Whether an action should be styled as a warning — does tapping it silently
 * change what someone else holds, in a way nothing else on the sheet does?
 *
 * {@link DELETE_FOR_EVERYONE} is the only one that qualifies. {@link DELETE_FOR_ME}
 * looks similar but never leaves this device — the other person's copy is
 * untouched. {@link REPORT} does not change delivery at all: the target never
 * sees it happened. Spending the error colour on either would teach people to
 * stop reading it before the row that actually needs it.
 */
export function isDestructive(action) {
  return action === DELETE_FOR_EVERYONE;
}

// ── Windows ──────────────────────────────────────────────────────────────

/**
 * Telegram's edit window. Chosen over something tighter because there is no
 * protocol constraint forcing a shorter number, and chosen over something
 * looser deliberately: an edit is a silent rewrite — nothing in the stored
 * message keeps the text it replaced — so an unbounded window would let a
 * message that already convinced someone of one thing quietly become a
 * different message weeks later. 48 hours is Telegram's own answer to that
 * trade-off.
 */
export const EDIT_WINDOW_MS = 48 * 60 * 60 * 1000;

/**
 * Deliberately **not shorter** than {@link EDIT_WINDOW_MS}, and the equality
 * is the point: if a message can still be rewritten, it can still be
 * retracted.
 *
 * An earlier version of this constant (on Android, before this file existed)
 * was half the edit window, on the theory that unsend is the stronger remedy
 * and so should expire first. That gets the consequence backwards. Editing
 * has no undo and records no prior text, so a sender who can still edit at
 * hour 30 but can no longer delete simply rewrites the message down to a full
 * stop — the retraction happens anyway, just as a visible husk in the thread
 * instead of a clean removal. Closing the stronger door first does not remove
 * the capability; it only removes the tidy way to use it.
 *
 * It also contradicted both apps this module unions. WhatsApp's
 * delete-for-everyone (about two days) far outlives its ~15-minute edit
 * window, and Telegram's private-chat delete-for-everyone has no limit at all
 * against a 48-hour edit. Neither ships the inversion.
 *
 * So this tracks {@link EDIT_WINDOW_MS} rather than holding a number of its
 * own — `retractionOutlivesRewriting` in the test file pins the ordering so a
 * later tweak to either window cannot silently reintroduce it.
 */
export const DELETE_FOR_EVERYONE_WINDOW_MS = EDIT_WINDOW_MS;

/**
 * How many messages a selection may hold before it stops growing.
 *
 * There is no batched-forward call anywhere in the runtime — forwarding N
 * selected messages is N separate sends — and past this many there is no
 * per-item progress or partial-failure UI to fall back on either, so a bulk
 * forward past the cap would be an unresponsive menu with no way to tell
 * whether it is working or stuck. 50 is comfortably above "the last dozen
 * photos in this chat", the case the feature exists for, and comfortably
 * below where a silent send-in-a-loop starts looking like a hang.
 */
export const MAX_SELECTION_SIZE = 50;

/** Whether a selection holding `currentSize` messages may take one more. */
export function canAddToSelection(currentSize) {
  return currentSize < MAX_SELECTION_SIZE;
}

/** Own message, has text, and still inside the edit window. */
function canEdit(ctx) {
  return Boolean(ctx.own) && Boolean(ctx.hasText) && ctx.ageMs <= EDIT_WINDOW_MS;
}

/** Own message, and still inside the delete-for-everyone window. */
function canDeleteForEveryone(ctx) {
  return Boolean(ctx.own) && ctx.ageMs <= DELETE_FOR_EVERYONE_WINDOW_MS;
}

// ── One message ──────────────────────────────────────────────────────────

/**
 * The context-menu row set for one message, top to bottom.
 *
 * `ctx` carries the handful of facts a decision here needs — `own`,
 * `hasText`, `isMedia`, `ageMs` (a duration, not a timestamp: the caller
 * measures the clock once, this file never does), `pinned`, `starred` — the
 * same shape `MessageContext` holds on Android.
 *
 * Exactly one of {@link PIN}/{@link UNPIN} appears, chosen by `ctx.pinned`;
 * exactly one of {@link STAR}/{@link UNSTAR} appears, chosen by `ctx.starred`
 * — a row always names what tapping it will *do*, never the current state.
 *
 * {@link DELETE_FOR_EVERYONE} is the only entry gated on more than one fact,
 * and it is also the only {@link isDestructive} one, so it sorts last by
 * construction: nothing conditional is added after it. Because its window
 * equals {@link EDIT_WINDOW_MS}, {@link EDIT} never outlives it on an own
 * text message — see {@link DELETE_FOR_EVERYONE_WINDOW_MS} for why that
 * ordering is load-bearing rather than incidental.
 */
export function messageActions(ctx) {
  const out = [REACT, REPLY, REPLY_IN_THREAD, FORWARD];
  out.push(ctx.starred ? UNSTAR : STAR);
  out.push(ctx.pinned ? UNPIN : PIN);
  out.push(ASSIGN_TOPIC);
  if (ctx.hasText) out.push(COPY);
  if (canEdit(ctx)) out.push(EDIT);
  out.push(SELECT, SHARE);
  if (ctx.isMedia) out.push(SAVE_MEDIA);
  out.push(MESSAGE_INFO);
  if (!ctx.own) out.push(REPORT);
  out.push(DELETE_FOR_ME);
  if (canDeleteForEveryone(ctx)) out.push(DELETE_FOR_EVERYONE);
  return out;
}

// ── A multi-message selection ───────────────────────────────────────────

/**
 * The contextual action set for a multi-message selection.
 *
 * Only what is meaningful for *every* message in `items`, not just some of
 * them — a bulk action that quietly applies to a subset of what was selected
 * is worse than not offering it, because the button's whole promise is that
 * it covers everything under the checkmarks. That is why several
 * single-message actions are absent here rather than degraded:
 *
 *  - {@link REPLY} and {@link REPLY_IN_THREAD} would have to pick one message
 *    out of the selection to quote or open, discarding the rest of what was
 *    checked.
 *  - {@link ASSIGN_TOPIC} files the thread *containing* one message, found by
 *    walking up its reply chain — a selection can span several threads, and
 *    filing all of them under one slug would silently merge conversations
 *    that were never one thread.
 *  - {@link EDIT} is a rewrite of one message's text; "edit this selection"
 *    has no single meaning.
 *  - {@link SELECT} is how a selection starts, not a further action once
 *    inside one.
 *  - {@link MESSAGE_INFO} shows one message's delivery/read metadata; a
 *    selection has no single timestamp for it to show.
 *  - {@link REACT} targets one message's reaction row; there is no
 *    established meaning for reacting to several at once.
 *
 * What *does* survive a mixed selection: {@link FORWARD} (sending a batch of
 * mixed content is exactly what forwarding several messages means),
 * {@link COPY} (if *any* item has text — it concatenates what there is and
 * skips what there isn't, the same tolerance {@link FORWARD} has for mixed
 * content), {@link SHARE} and {@link SAVE_MEDIA} (if *any* item is media).
 * {@link STAR}/{@link UNSTAR} and {@link PIN}/{@link UNPIN} use the *all*
 * state — the pair shown is the one that makes every item's state uniform,
 * so tapping it never leaves part of the selection in its old state.
 *
 * {@link REPORT} and {@link DELETE_FOR_EVERYONE} both require the qualifying
 * fact to hold for *every* item (`own` is false for all of them; the delete
 * window for all of them) — the same "no partial application" reasoning as
 * above, sharpened because both are otherwise-irreversible or other-facing
 * actions where a silent partial failure is the worst version of the
 * mistake.
 *
 * {@link DELETE_FOR_ME} is unconditional, matching {@link messageActions}: it
 * only ever touches this device's own view.
 *
 * Returns `[]` past {@link MAX_SELECTION_SIZE} (or for an empty selection) —
 * a selection that large should never have been allowed to form (see
 * {@link canAddToSelection}), and a context menu is not the place to
 * discover that it did.
 */
export function selectionActions(items) {
  if (!items || items.length === 0 || items.length > MAX_SELECTION_SIZE) return [];
  const out = [FORWARD];
  if (items.some((it) => it.hasText)) out.push(COPY);
  out.push(items.every((it) => it.starred) ? UNSTAR : STAR);
  out.push(items.every((it) => it.pinned) ? UNPIN : PIN);
  out.push(SHARE);
  if (items.some((it) => it.isMedia)) out.push(SAVE_MEDIA);
  if (items.every((it) => !it.own)) out.push(REPORT);
  out.push(DELETE_FOR_ME);
  if (items.every((it) => canDeleteForEveryone(it))) out.push(DELETE_FOR_EVERYONE);
  return out;
}
