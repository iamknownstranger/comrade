/// The long-press action set for one message, and the contextual set for a
/// multi-select — ported from
/// `android/app/src/main/java/mullu/comrade/ui/MessageActions.kt`. Same enum
/// entries, same order, same windows.
///
/// Kept free of Flutter imports so plain Dart unit tests
/// (`message_actions_test.dart`) can pin the windows, the toggle directions
/// and which entries are destructive, exactly as `ChatMenuAction` and its
/// test do for the conversation menu, and exactly as `MessageActionsTest`
/// does on Android. Labels are deliberately not here: the caller resolves the
/// string, the same division `chat_menu.dart` already uses.
///
/// This file does not depend on any bridge message type on purpose — the same
/// reason `MessageActions.kt` does not depend on `ComradeCore.MessageInfo`.
/// [MessageContext] is the caller's translation of whichever real message
/// type it holds into the handful of facts a decision here actually needs.
///
/// `reply`, `replyInThread` ("open thread") and `copy` already exist in the
/// conversation screen's message action sheet, alongside `assignTopic`. Their
/// relative order here — `reply` before `replyInThread` before `assignTopic`
/// before `copy` — matches Android's row order and `chat_menu.dart`'s
/// existing order rule, so a later widget pass extends it rather than
/// reshuffling rows under someone's thumb mid-gesture.
///
/// `react` covers the quick row *and* the "more" picker as one entry: which
/// six emoji sit in the quick row is `quickReactions`'s decision (see
/// `message_reactions.dart`), and opening the full picker from "more" is a
/// widget-layer concern. There is no further rule to encode for it here — it
/// is unconditionally offered, because whether a message is eligible for a
/// long-press menu **at all** is decided by the caller before this function
/// ever runs.
library;

/// An entry in a message's long-press action sheet or a selection's
/// contextual action bar.
enum MessageAction {
  /// Quick row + "more" — see the file header for why this is one entry.
  react,

  /// Quote this message in the composer.
  reply,

  /// Open (or start) the thread this message belongs to.
  replyInThread,

  /// Send a copy of this message's content to another chat.
  forward,

  /// Pin. Appears only when [unpin] does not — see [messageActions].
  pin,

  /// Unpin. Appears only when [pin] does not — see [messageActions].
  unpin,

  /// Star. Appears only when [unstar] does not — see [messageActions].
  star,

  /// Unstar. Appears only when [star] does not — see [messageActions].
  unstar,

  /// File the thread this message belongs to under a topic.
  assignTopic,

  /// Copy the text to the clipboard. Text messages only — see
  /// [messageActions].
  copy,

  /// Rewrite this message's text. Own text messages, inside [editWindowMs].
  edit,

  /// Enter multi-select, starting from this message.
  select,

  /// Hand this message's content to another app.
  share,

  /// Write this message's media to the device's own storage.
  saveMedia,

  /// Delivery/read metadata for this one message.
  messageInfo,

  /// Flag this message for review. Incoming only — see [messageActions].
  report,

  /// Remove this message from this device's own view. Always available.
  deleteForMe,

  /// Remove this message everywhere. Own messages, inside
  /// [deleteForEveryoneWindowMs].
  deleteForEveryone;

  /// Whether the row should be styled as a warning —
  /// `ChatMenuAction.destructive`'s rule, restated: does tapping this row
  /// silently change what someone else holds, in a way nothing else on the
  /// sheet does?
  ///
  /// [deleteForEveryone] is the only entry that qualifies. [deleteForMe]
  /// looks similar but never leaves this device — the other person's copy is
  /// untouched, so it is no more dangerous than any other local-only action
  /// on this sheet. [report] does not change delivery at all: the target
  /// never sees it happened. Spending the error colour on either would teach
  /// people to stop reading it before the row that actually needs it.
  bool get destructive => this == MessageAction.deleteForEveryone;
}

/// The handful of facts a decision in this file needs about one message —
/// the caller's translation of whichever bridge message type (or local chat
/// item wrapping one) it holds.
///
/// [own] is whether this device sent it. [hasText] is whether it carries text
/// worth copying or editing — a media message with a caption still counts,
/// matching what [MessageAction.copy] would put on the clipboard. [isMedia]
/// is whether it carries a saveable attachment. [ageMs] is how long ago it was
/// sent, in milliseconds, as of the moment the decision is made — a duration
/// rather than a timestamp so nothing in this file reads a clock; the caller
/// does that once. [pinned] and [starred] are the current toggle states.
class MessageContext {
  const MessageContext({
    this.own = false,
    this.hasText = true,
    this.isMedia = false,
    this.ageMs = 0,
    this.pinned = false,
    this.starred = false,
  });

  final bool own;
  final bool hasText;
  final bool isMedia;
  final int ageMs;
  final bool pinned;
  final bool starred;
}

/// Telegram's edit window. Chosen over something tighter because there is no
/// protocol constraint forcing a shorter number, and chosen over something
/// looser deliberately: an edit is a silent rewrite — nothing records the
/// text it replaced — so an unbounded window would let a message that already
/// convinced someone of one thing quietly become a different message weeks
/// later. 48 hours is Telegram's own answer to that trade-off.
const int editWindowMs = 48 * 60 * 60 * 1000;

/// Deliberately **not shorter** than [editWindowMs], and the equality is the
/// point: if a message can still be rewritten, it can still be retracted.
///
/// An earlier version of this constant was half the edit window, on the
/// theory that unsend is the stronger remedy and so should expire first. That
/// gets the consequence backwards. Editing has no undo and records no prior
/// text, so a sender who can still edit at hour 30 but can no longer delete
/// simply rewrites the message down to a full stop — the retraction happens
/// anyway, just as a visible husk in the thread instead of a clean removal.
/// Closing the stronger door first does not remove the capability; it only
/// removes the tidy way to use it.
///
/// It also contradicted both apps this file unions. WhatsApp's delete-for-
/// everyone (about two days) far outlives its ~15-minute edit window, and
/// Telegram's private-chat delete-for-everyone has no limit at all against a
/// 48-hour edit. Neither ships the inversion.
///
/// So this tracks [editWindowMs] rather than holding a number of its own, and
/// `retractionOutlivesRewriting` (the test) pins the ordering so a later
/// tweak to either window cannot silently reintroduce it.
const int deleteForEveryoneWindowMs = editWindowMs;

/// How many messages a selection may hold before it stops growing.
///
/// There is no batched-forward call anywhere in the bridge — forwarding N
/// selected messages is N separate sends — and past this many there is no
/// per-item progress or partial-failure UI to fall back on either, so a bulk
/// forward past the cap would be an unresponsive sheet with no way to tell
/// whether it is working or stuck. 50 is comfortably above "the last dozen
/// photos in this chat", the case the feature exists for, and comfortably
/// below where a silent send-in-a-loop starts looking like a hang.
const int maxSelectionSize = 50;

/// Whether a selection holding [currentSize] messages may take one more.
bool canAddToSelection(int currentSize) => currentSize < maxSelectionSize;

/// Own message, has text, and still inside the edit window.
bool _canEdit(MessageContext ctx) =>
    ctx.own && ctx.hasText && ctx.ageMs <= editWindowMs;

/// Own message, and still inside the delete-for-everyone window.
bool _canDeleteForEveryone(MessageContext ctx) =>
    ctx.own && ctx.ageMs <= deleteForEveryoneWindowMs;

/// The long-press sheet for one message, top to bottom.
///
/// Exactly one of [MessageAction.pin]/[MessageAction.unpin] appears, chosen
/// by [MessageContext.pinned]; exactly one of
/// [MessageAction.star]/[MessageAction.unstar] appears, chosen by
/// [MessageContext.starred] — `conversationMenu`'s rule, so a row always
/// names what tapping it will *do* rather than describing the current state.
///
/// [MessageAction.deleteForEveryone] is the only entry gated on more than one
/// fact, and it is also the only [MessageAction.destructive] one, so it sorts
/// last by construction: nothing conditional is added after it. Because its
/// window equals [editWindowMs], [MessageAction.edit] never outlives it on an
/// own text message — see [deleteForEveryoneWindowMs] for why that ordering
/// is load-bearing rather than incidental.
List<MessageAction> messageActions(MessageContext ctx) => <MessageAction>[
      MessageAction.react,
      MessageAction.reply,
      MessageAction.replyInThread,
      MessageAction.forward,
      if (ctx.starred) MessageAction.unstar else MessageAction.star,
      if (ctx.pinned) MessageAction.unpin else MessageAction.pin,
      MessageAction.assignTopic,
      if (ctx.hasText) MessageAction.copy,
      if (_canEdit(ctx)) MessageAction.edit,
      MessageAction.select,
      MessageAction.share,
      if (ctx.isMedia) MessageAction.saveMedia,
      MessageAction.messageInfo,
      if (!ctx.own) MessageAction.report,
      MessageAction.deleteForMe,
      if (_canDeleteForEveryone(ctx)) MessageAction.deleteForEveryone,
    ];

/// The contextual action set for a multi-message selection.
///
/// Only what is meaningful for *every* message in [items], not just some of
/// them — a bulk action that quietly applies to a subset of what was selected
/// is worse than not offering it, because the button's whole promise is that
/// it covers everything under the checkmarks. That is why several
/// single-message actions are absent here rather than degraded:
///
/// - [MessageAction.reply] and [MessageAction.replyInThread] would have to
///   pick one message out of the selection to quote or open, discarding the
///   rest of what was checked.
/// - [MessageAction.assignTopic] files the thread *containing* one message,
///   found by walking up its reply chain — a selection can span several
///   threads, and filing all of them under one slug would silently merge
///   conversations that were never one thread.
/// - [MessageAction.edit] is a rewrite of one message's text; "edit this
///   selection" has no single meaning.
/// - [MessageAction.select] is how a selection starts, not a further action
///   once inside one.
/// - [MessageAction.messageInfo] shows one message's delivery/read metadata;
///   a selection has no single timestamp for it to show.
/// - [MessageAction.react] targets one message's reaction row; there is no
///   established meaning for reacting to several at once.
///
/// What *does* survive a mixed selection: [MessageAction.forward] (sending a
/// batch of mixed content is exactly what forwarding several messages
/// means), [MessageAction.copy] (if *any* item has text — it concatenates
/// what there is and skips what there isn't, the same tolerance
/// [MessageAction.forward] has for mixed content), [MessageAction.share] and
/// [MessageAction.saveMedia] (if *any* item is media).
/// [MessageAction.star]/[MessageAction.unstar] and
/// [MessageAction.pin]/[MessageAction.unpin] use the *all* state — the pair
/// shown is the one that makes every item's state uniform, so tapping it
/// never leaves part of the selection in its old state.
///
/// [MessageAction.report] and [MessageAction.deleteForEveryone] both require
/// the qualifying fact to hold for *every* item ([MessageContext.own] is
/// `false` for all of them; the delete window holds for all of them) — the
/// same "no partial application" reasoning as above, sharpened because both
/// are otherwise-irreversible or other-facing actions where a silent partial
/// failure is the worst version of the mistake.
///
/// [MessageAction.deleteForMe] is unconditional, matching [messageActions]:
/// it only ever touches this device's own view.
///
/// Returns nothing past [maxSelectionSize] — a selection that large should
/// never have been allowed to form (see [canAddToSelection]), and an action
/// sheet is not the place to discover that it did.
List<MessageAction> selectionActions(List<MessageContext> items) {
  if (items.isEmpty || items.length > maxSelectionSize) {
    return <MessageAction>[];
  }
  return <MessageAction>[
    MessageAction.forward,
    if (items.any((MessageContext c) => c.hasText)) MessageAction.copy,
    if (items.every((MessageContext c) => c.starred))
      MessageAction.unstar
    else
      MessageAction.star,
    if (items.every((MessageContext c) => c.pinned))
      MessageAction.unpin
    else
      MessageAction.pin,
    MessageAction.share,
    if (items.any((MessageContext c) => c.isMedia)) MessageAction.saveMedia,
    if (items.every((MessageContext c) => !c.own)) MessageAction.report,
    MessageAction.deleteForMe,
    if (items.every(_canDeleteForEveryone)) MessageAction.deleteForEveryone,
  ];
}
