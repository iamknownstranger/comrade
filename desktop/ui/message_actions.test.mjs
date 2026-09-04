import { test } from "node:test";
import assert from "node:assert/strict";
import {
  messageActions,
  selectionActions,
  canAddToSelection,
  isDestructive,
  EDIT_WINDOW_MS,
  DELETE_FOR_EVERYONE_WINDOW_MS,
  MAX_SELECTION_SIZE,
  REACT,
  REPLY,
  REPLY_IN_THREAD,
  FORWARD,
  ASSIGN_TOPIC,
  COPY,
  EDIT,
  SELECT,
  MESSAGE_INFO,
  REPORT,
  DELETE_FOR_ME,
  DELETE_FOR_EVERYONE,
  PIN,
  UNPIN,
  STAR,
  UNSTAR,
} from "./message_actions.mjs";

// Vectors ported 1:1 from
// android/app/src/test/java/mullu/comrade/ui/MessageActionsTest.kt — same
// cases, same answers.

function ctx({
  own = false,
  hasText = true,
  isMedia = false,
  ageMs = 0,
  pinned = false,
  starred = false,
} = {}) {
  return { own, hasText, isMedia, ageMs, pinned, starred };
}

// ── Edit ─────────────────────────────────────────────────────────────────

test("edit offers on an own text message inside the window", () => {
  const actions = messageActions(ctx({ own: true, hasText: true, ageMs: EDIT_WINDOW_MS - 1 }));
  assert.ok(actions.includes(EDIT));
});

test("edit is gone the instant the window closes", () => {
  const actions = messageActions(ctx({ own: true, hasText: true, ageMs: EDIT_WINDOW_MS + 1 }));
  assert.ok(!actions.includes(EDIT));
});

test("edit never offers on someone else's message", () => {
  const actions = messageActions(ctx({ own: false, hasText: true, ageMs: 0 }));
  assert.ok(!actions.includes(EDIT));
});

test("edit never offers on a message with no text", () => {
  const actions = messageActions(ctx({ own: true, hasText: false, ageMs: 0 }));
  assert.ok(!actions.includes(EDIT));
});

// ── DeleteForEveryone / DeleteForMe ─────────────────────────────────────

test("delete-for-everyone offers on an own message inside its window", () => {
  const actions = messageActions(ctx({ own: true, ageMs: DELETE_FOR_EVERYONE_WINDOW_MS - 1 }));
  assert.ok(actions.includes(DELETE_FOR_EVERYONE));
});

test("delete-for-everyone is gone once its window closes", () => {
  const actions = messageActions(ctx({ own: true, ageMs: DELETE_FOR_EVERYONE_WINDOW_MS + 1 }));
  assert.ok(!actions.includes(DELETE_FOR_EVERYONE));
});

test("delete-for-everyone never offers on someone else's message", () => {
  const actions = messageActions(ctx({ own: false, ageMs: 0 }));
  assert.ok(!actions.includes(DELETE_FOR_EVERYONE));
});

/**
 * The ordering that must never invert: anything still rewritable is still
 * retractable. A sender who can edit but not delete rewrites the message to
 * nothing instead, so closing the delete window first buys no restraint — it
 * only takes away the clean way to do what they will do anyway.
 */
test("retraction outlives rewriting", () => {
  assert.ok(DELETE_FOR_EVERYONE_WINDOW_MS >= EDIT_WINDOW_MS);
});

test("edit is never offered without delete-for-everyone", () => {
  for (const age of [0, EDIT_WINDOW_MS / 2, EDIT_WINDOW_MS - 1, EDIT_WINDOW_MS]) {
    const actions = messageActions(ctx({ own: true, hasText: true, ageMs: age }));
    if (actions.includes(EDIT)) {
      assert.ok(
        actions.includes(DELETE_FOR_EVERYONE),
        `editable at ${age} ms but not retractable`,
      );
    }
  }
});

test("delete-for-me is always there regardless of ownership or age", () => {
  assert.ok(
    messageActions(ctx({ own: false, ageMs: Number.MAX_SAFE_INTEGER / 2 })).includes(
      DELETE_FOR_ME,
    ),
  );
  assert.ok(
    messageActions(ctx({ own: true, ageMs: Number.MAX_SAFE_INTEGER / 2 })).includes(
      DELETE_FOR_ME,
    ),
  );
});

// ── Report ───────────────────────────────────────────────────────────────

test("report offers only on incoming messages", () => {
  assert.ok(messageActions(ctx({ own: false })).includes(REPORT));
});

test("report never offers on your own message", () => {
  assert.ok(!messageActions(ctx({ own: true })).includes(REPORT));
});

// ── SaveMedia / Copy ─────────────────────────────────────────────────────

test("save-media offers only on media messages", () => {
  assert.ok(messageActions(ctx({ isMedia: true })).includes("save_media"));
  assert.ok(!messageActions(ctx({ isMedia: false })).includes("save_media"));
});

test("copy offers only when there is text", () => {
  assert.ok(messageActions(ctx({ hasText: true })).includes(COPY));
  assert.ok(!messageActions(ctx({ hasText: false })).includes(COPY));
});

// ── Pin/Unpin, Star/Unstar ───────────────────────────────────────────────

test("exactly one of pin or unpin appears", () => {
  const unpinned = messageActions(ctx({ pinned: false }));
  assert.ok(unpinned.includes(PIN));
  assert.ok(!unpinned.includes(UNPIN));

  const pinned = messageActions(ctx({ pinned: true }));
  assert.ok(pinned.includes(UNPIN));
  assert.ok(!pinned.includes(PIN));
});

test("exactly one of star or unstar appears", () => {
  const unstarred = messageActions(ctx({ starred: false }));
  assert.ok(unstarred.includes(STAR));
  assert.ok(!unstarred.includes(UNSTAR));

  const starred = messageActions(ctx({ starred: true }));
  assert.ok(starred.includes(UNSTAR));
  assert.ok(!starred.includes(STAR));
});

// ── Destructive sorts last ───────────────────────────────────────────────

test("delete-for-everyone is the only destructive entry", () => {
  const all = [
    REACT, REPLY, REPLY_IN_THREAD, FORWARD, PIN, UNPIN, STAR, UNSTAR,
    ASSIGN_TOPIC, COPY, EDIT, SELECT, "share", "save_media", MESSAGE_INFO,
    REPORT, DELETE_FOR_ME, DELETE_FOR_EVERYONE,
  ];
  assert.deepEqual(all.filter(isDestructive), [DELETE_FOR_EVERYONE]);
});

test("destructive never has anything after it", () => {
  const actions = messageActions(ctx({ own: true, hasText: true, isMedia: true, ageMs: 0 }));
  const idx = actions.indexOf(DELETE_FOR_EVERYONE);
  assert.ok(idx >= 0, "expected delete_for_everyone to be offered");
  assert.equal(idx, actions.length - 1);
});

// ── Order relationship with the existing sheet ───────────────────────────

test("reply, reply-in-thread, assign-topic, copy keep their existing relative order", () => {
  const actions = messageActions(ctx({ hasText: true }));
  const reply = actions.indexOf(REPLY);
  const replyInThread = actions.indexOf(REPLY_IN_THREAD);
  const assignTopic = actions.indexOf(ASSIGN_TOPIC);
  const copy = actions.indexOf(COPY);
  assert.ok(reply < replyInThread);
  assert.ok(replyInThread < assignTopic);
  assert.ok(assignTopic < copy);
});

test("react is always first", () => {
  assert.equal(messageActions(ctx())[0], REACT);
});

// ── Selection: only what applies to every item ──────────────────────────

test("forward is fine on a mixed selection", () => {
  const items = [ctx({ hasText: true, isMedia: false }), ctx({ hasText: false, isMedia: true })];
  assert.ok(selectionActions(items).includes(FORWARD));
});

test("editing one of many selected is not offered", () => {
  const items = [
    ctx({ own: true, hasText: true, ageMs: 0 }),
    ctx({ own: false, hasText: true, ageMs: 0 }),
  ];
  assert.ok(!selectionActions(items).includes(EDIT));
});

test("copy applies if any selected item has text", () => {
  const items = [ctx({ hasText: false, isMedia: true }), ctx({ hasText: true })];
  assert.ok(selectionActions(items).includes(COPY));
});

test("copy is absent when no selected item has text", () => {
  const items = [ctx({ hasText: false, isMedia: true }), ctx({ hasText: false, isMedia: true })];
  assert.ok(!selectionActions(items).includes(COPY));
});

test("save-media applies if any selected item is media", () => {
  const items = [ctx({ isMedia: false }), ctx({ isMedia: true })];
  assert.ok(selectionActions(items).includes("save_media"));
});

test("star toggles to unstar only when all are starred", () => {
  const mixed = [ctx({ starred: true }), ctx({ starred: false })];
  assert.ok(selectionActions(mixed).includes(STAR));
  assert.ok(!selectionActions(mixed).includes(UNSTAR));

  const allStarred = [ctx({ starred: true }), ctx({ starred: true })];
  assert.ok(selectionActions(allStarred).includes(UNSTAR));
  assert.ok(!selectionActions(allStarred).includes(STAR));
});

test("report requires every item to be incoming", () => {
  const mixed = [ctx({ own: false }), ctx({ own: true })];
  assert.ok(!selectionActions(mixed).includes(REPORT));

  const allIncoming = [ctx({ own: false }), ctx({ own: false })];
  assert.ok(selectionActions(allIncoming).includes(REPORT));
});

test("delete-for-everyone requires every item to qualify", () => {
  const mixed = [
    ctx({ own: true, ageMs: 0 }),
    ctx({ own: true, ageMs: DELETE_FOR_EVERYONE_WINDOW_MS + 1 }),
  ];
  assert.ok(!selectionActions(mixed).includes(DELETE_FOR_EVERYONE));

  const allQualify = [ctx({ own: true, ageMs: 0 }), ctx({ own: true, ageMs: 0 })];
  assert.ok(selectionActions(allQualify).includes(DELETE_FOR_EVERYONE));
});

test("delete-for-me is always in a selection", () => {
  const items = [ctx({ own: false }), ctx({ own: true })];
  assert.ok(selectionActions(items).includes(DELETE_FOR_ME));
});

test("selection has no reply, reply-in-thread, assign-topic, edit, select or message-info", () => {
  const items = [ctx({ own: true, hasText: true, ageMs: 0 })];
  const actions = selectionActions(items);
  assert.ok(!actions.includes(REPLY));
  assert.ok(!actions.includes(REPLY_IN_THREAD));
  assert.ok(!actions.includes(ASSIGN_TOPIC));
  assert.ok(!actions.includes(EDIT));
  assert.ok(!actions.includes(SELECT));
  assert.ok(!actions.includes(MESSAGE_INFO));
  assert.ok(!actions.includes(REACT));
});

// ── Selection cap ────────────────────────────────────────────────────────

test("can add to selection below the cap", () => {
  assert.ok(canAddToSelection(MAX_SELECTION_SIZE - 1));
});

test("cannot add to selection at the cap", () => {
  assert.ok(!canAddToSelection(MAX_SELECTION_SIZE));
});

test("selection actions is empty past the cap", () => {
  const tooMany = Array.from({ length: MAX_SELECTION_SIZE + 1 }, () => ctx());
  assert.equal(selectionActions(tooMany).length, 0);
});

test("selection actions is not empty at the cap", () => {
  const atCap = Array.from({ length: MAX_SELECTION_SIZE }, () => ctx());
  assert.ok(selectionActions(atCap).length > 0);
});

test("selection actions is empty for an empty selection", () => {
  assert.equal(selectionActions([]).length, 0);
});
