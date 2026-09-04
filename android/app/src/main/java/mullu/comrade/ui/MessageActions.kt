package mullu.comrade.ui

/**
 * The long-press action set for one message, and the contextual set for a
 * multi-select — the union of what WhatsApp and Telegram offer, cut down to
 * what Comrade can actually do.
 *
 * Kept free of Compose/Android imports so plain JVM unit tests
 * (`MessageActionsTest`) can pin the windows, the toggle directions and which
 * entries are destructive, exactly as [ChatMenuAction] and its test do for the
 * conversation menu. Labels are deliberately *not* here: the caller resolves
 * the string, the same division `conversationMenu` already uses.
 *
 * This file does not depend on `ComradeCore.MessageInfo` or
 * `ComradeCore.MediaMessageInfo` on purpose. Both live in a file that imports
 * `android.util.Log` and the uniffi bindings, and pulling either type in here
 * would drag this file — and the JVM lane that is the whole point of writing
 * it this way — onto a classpath the plain `kotlinc` run in `CLAUDE.md` does
 * not have. [MessageContext] is the caller's translation of whichever real
 * message type it holds into the handful of facts a decision here actually
 * needs.
 *
 * `Reply`, `ReplyInThread` ("open thread") and `Copy` already exist in
 * `ChatsScreen.kt`'s `MessageActionSheet`, alongside `AssignTopic` filed under
 * `ChatCommands.ComposerPlan.AssignTopic`. Their relative order here —
 * `Reply` before `ReplyInThread` before `AssignTopic` before `Copy` — matches
 * that sheet's existing row order so a later Compose pass extends it rather
 * than reshuffling rows under someone's thumb mid-gesture. No Flutter port of
 * this file exists yet; when one lands it inherits the same order for the
 * same reason `ChatMenuAction` and `app/lib/src/util/chat_menu.dart` do.
 *
 * `React` covers the quick row *and* the "more" picker as one entry: which
 * six emoji sit in the quick row is [QUICK_REACTIONS]'s decision, and opening
 * the full picker from "more" is already wired in `MessageActionSheet`. There
 * is no further rule to encode for it here — it is unconditionally offered,
 * because whether a message is eligible for a long-press menu **at all** is
 * decided by the caller before this function ever runs.
 */
enum class MessageAction {
    /** Quick row + "more" — see the file header for why this is one entry. */
    React,

    /** Quote this message in the composer. */
    Reply,

    /** Open (or start) the thread this message belongs to. */
    ReplyInThread,

    /** Send a copy of this message's content to another chat. */
    Forward,

    /** Pin. Appears only when [Unpin] does not — see [messageActions]. */
    Pin,

    /** Unpin. Appears only when [Pin] does not — see [messageActions]. */
    Unpin,

    /** Star. Appears only when [Unstar] does not — see [messageActions]. */
    Star,

    /** Unstar. Appears only when [Star] does not — see [messageActions]. */
    Unstar,

    /** File the thread this message belongs to under a topic. */
    AssignTopic,

    /** Copy the text to the clipboard. Text messages only — see [messageActions]. */
    Copy,

    /** Rewrite this message's text. Own text messages, inside [EDIT_WINDOW_MS]. */
    Edit,

    /** Enter multi-select, starting from this message. */
    Select,

    /** Hand this message's content to another app. */
    Share,

    /** Write this message's media to the device's own storage. */
    SaveMedia,

    /** Delivery/read metadata for this one message. */
    MessageInfo,

    /** Flag this message for review. Incoming only — see [messageActions]. */
    Report,

    /** Remove this message from this device's own view. Always available. */
    DeleteForMe,

    /** Remove this message everywhere. Own messages, inside [DELETE_FOR_EVERYONE_WINDOW_MS]. */
    DeleteForEveryone,
    ;

    /**
     * Whether the row should be styled as a warning — [ChatMenuAction.destructive]'s
     * rule, restated: does tapping this row silently change what someone else
     * holds, in a way nothing else on the sheet does?
     *
     * [DeleteForEveryone] is the only entry that qualifies. [DeleteForMe] looks
     * similar but never leaves this device — the other person's copy is
     * untouched, so it is no more dangerous than any other local-only action on
     * this sheet. [Report] does not change delivery at all: the target never
     * sees it happened. Spending the error colour on either would teach people
     * to stop reading it before the row that actually needs it.
     */
    val destructive: Boolean get() = this == DeleteForEveryone
}

/**
 * The handful of facts a decision in this file needs about one message —
 * the caller's translation of `ComradeCore.MessageInfo` /
 * `ComradeCore.MediaMessageInfo` (or the local `ChatItem` wrapping them).
 *
 * @param own whether this device sent it — mirrors `MessageInfo.outgoing`.
 * @param hasText whether it carries text worth copying or editing. A media
 *   message with a caption still counts, matching what `Copy` would put on the
 *   clipboard.
 * @param isMedia whether it carries a saveable attachment.
 * @param ageMs how long ago it was sent, in milliseconds, as of the moment the
 *   decision is made. A duration rather than a timestamp so nothing in this
 *   file reads a clock — the caller does that once and this stays as pure as
 *   [mullu.comrade.together.TogetherDecisions].
 * @param pinned current pin state.
 * @param starred current star state.
 */
data class MessageContext(
    val own: Boolean,
    val hasText: Boolean,
    val isMedia: Boolean,
    val ageMs: Long,
    val pinned: Boolean,
    val starred: Boolean,
)

/**
 * Telegram's edit window. Chosen over something tighter because there is no
 * protocol constraint forcing a shorter number, and chosen over something
 * looser deliberately: an edit is a silent rewrite — nothing in `MessageInfo`
 * records the text it replaced — so an unbounded window would let a message
 * that already convinced someone of one thing quietly become a different
 * message weeks later. 48 hours is Telegram's own answer to that trade-off.
 */
const val EDIT_WINDOW_MS: Long = 48L * 60 * 60 * 1000

/**
 * Deliberately **not shorter** than [EDIT_WINDOW_MS], and the equality is the
 * point: if a message can still be rewritten, it can still be retracted.
 *
 * An earlier version of this constant was half the edit window, on the theory
 * that unsend is the stronger remedy and so should expire first. That gets the
 * consequence backwards. Editing has no undo and records no prior text, so a
 * sender who can still edit at hour 30 but can no longer delete simply rewrites
 * the message down to a full stop — the retraction happens anyway, just as a
 * visible husk in the thread instead of a clean removal. Closing the stronger
 * door first does not remove the capability; it only removes the tidy way to
 * use it.
 *
 * It also contradicted both apps this file unions. WhatsApp's delete-for-
 * everyone (about two days) far outlives its ~15-minute edit window, and
 * Telegram's private-chat delete-for-everyone has no limit at all against a
 * 48-hour edit. Neither ships the inversion.
 *
 * So this tracks [EDIT_WINDOW_MS] rather than holding a number of its own, and
 * `retractionOutlivesRewriting` pins the ordering so a later tweak to either
 * window cannot silently reintroduce it.
 */
const val DELETE_FOR_EVERYONE_WINDOW_MS: Long = EDIT_WINDOW_MS

/**
 * How many messages a selection may hold before it stops growing.
 *
 * There is no batched-forward call anywhere in `ComradeCore` — forwarding N
 * selected messages is N separate sends — and past this many there is no
 * per-item progress or partial-failure UI to fall back on either, so a bulk
 * forward past the cap would be an unresponsive sheet with no way to tell
 * whether it is working or stuck. 50 is comfortably above "the last dozen
 * photos in this chat", the case the feature exists for, and comfortably
 * below where a silent send-in-a-loop starts looking like a hang.
 */
const val MAX_SELECTION_SIZE: Int = 50

/** Whether a selection holding [currentSize] messages may take one more. */
fun canAddToSelection(currentSize: Int): Boolean = currentSize < MAX_SELECTION_SIZE

/** Own message, has text, and still inside the edit window. */
private fun canEdit(ctx: MessageContext): Boolean =
    ctx.own && ctx.hasText && ctx.ageMs <= EDIT_WINDOW_MS

/** Own message, and still inside the delete-for-everyone window. */
private fun canDeleteForEveryone(ctx: MessageContext): Boolean =
    ctx.own && ctx.ageMs <= DELETE_FOR_EVERYONE_WINDOW_MS

/**
 * The long-press sheet for one message, top to bottom.
 *
 * Exactly one of [MessageAction.Pin]/[MessageAction.Unpin] appears, chosen by
 * [MessageContext.pinned]; exactly one of [MessageAction.Star]/[MessageAction.Unstar]
 * appears, chosen by [MessageContext.starred] — [conversationMenu]'s rule, so a
 * row always names what tapping it will *do* rather than describing the
 * current state.
 *
 * [MessageAction.DeleteForEveryone] is the only entry gated on more than one
 * fact, and it is also the only [MessageAction.destructive] one, so it sorts
 * last by construction: nothing conditional is added after it. Because its
 * window equals [EDIT_WINDOW_MS], [MessageAction.Edit] never outlives it on an
 * own text message — see [DELETE_FOR_EVERYONE_WINDOW_MS] for why that ordering
 * is load-bearing rather than incidental.
 */
fun messageActions(ctx: MessageContext): List<MessageAction> = buildList {
    add(MessageAction.React)
    add(MessageAction.Reply)
    add(MessageAction.ReplyInThread)
    add(MessageAction.Forward)
    add(if (ctx.starred) MessageAction.Unstar else MessageAction.Star)
    add(if (ctx.pinned) MessageAction.Unpin else MessageAction.Pin)
    add(MessageAction.AssignTopic)
    if (ctx.hasText) add(MessageAction.Copy)
    if (canEdit(ctx)) add(MessageAction.Edit)
    add(MessageAction.Select)
    add(MessageAction.Share)
    if (ctx.isMedia) add(MessageAction.SaveMedia)
    add(MessageAction.MessageInfo)
    if (!ctx.own) add(MessageAction.Report)
    add(MessageAction.DeleteForMe)
    if (canDeleteForEveryone(ctx)) add(MessageAction.DeleteForEveryone)
}

/**
 * The contextual action set for a multi-message selection.
 *
 * Only what is meaningful for *every* message in [items], not just some of
 * them — a bulk action that quietly applies to a subset of what was selected
 * is worse than not offering it, because the button's whole promise is that
 * it covers everything under the checkmarks. That is why several
 * single-message actions are absent here rather than degraded:
 *
 * - [MessageAction.Reply] and [MessageAction.ReplyInThread] would have to pick
 *   one message out of the selection to quote or open, discarding the rest of
 *   what was checked.
 * - [MessageAction.AssignTopic] files the thread *containing* one message,
 *   found by walking up its reply chain — a selection can span several
 *   threads, and filing all of them under one slug would silently merge
 *   conversations that were never one thread.
 * - [MessageAction.Edit] is a rewrite of one message's text; "edit this
 *   selection" has no single meaning.
 * - [MessageAction.Select] is how a selection starts, not a further action
 *   once inside one.
 * - [MessageAction.MessageInfo] shows one message's delivery/read metadata;
 *   a selection has no single timestamp for it to show.
 * - [MessageAction.React] targets one message's reaction row; there is no
 *   established meaning for reacting to several at once.
 *
 * What *does* survive a mixed selection: [MessageAction.Forward] (sending a
 * batch of mixed content is exactly what forwarding several messages means),
 * [MessageAction.Copy] (if *any* item has text — it concatenates what there
 * is and skips what there isn't, the same tolerance [MessageAction.Forward]
 * has for mixed content), [MessageAction.Share] and [MessageAction.SaveMedia]
 * (if *any* item is media). [MessageAction.Star]/[MessageAction.Unstar] and
 * [MessageAction.Pin]/[MessageAction.Unpin] use the *all* state — the pair
 * shown is the one that makes every item's state uniform, so tapping it never
 * leaves part of the selection in its old state.
 *
 * [MessageAction.Report] and [MessageAction.DeleteForEveryone] both require
 * the qualifying fact to hold for *every* item ([MessageContext.own] is
 * `false` for all of them; [canDeleteForEveryone] for all of them) — the same
 * "no partial application" reasoning as above, sharpened because both are
 * otherwise-irreversible or other-facing actions where a silent partial
 * failure is the worst version of the mistake.
 *
 * [MessageAction.DeleteForMe] is unconditional, matching [messageActions]: it
 * only ever touches this device's own view.
 *
 * Returns nothing past [MAX_SELECTION_SIZE] — a selection that large should
 * never have been allowed to form (see [canAddToSelection]), and an action
 * sheet is not the place to discover that it did.
 */
fun selectionActions(items: List<MessageContext>): List<MessageAction> {
    if (items.isEmpty() || items.size > MAX_SELECTION_SIZE) return emptyList()
    return buildList {
        add(MessageAction.Forward)
        if (items.any { it.hasText }) add(MessageAction.Copy)
        add(if (items.all { it.starred }) MessageAction.Unstar else MessageAction.Star)
        add(if (items.all { it.pinned }) MessageAction.Unpin else MessageAction.Pin)
        add(MessageAction.Share)
        if (items.any { it.isMedia }) add(MessageAction.SaveMedia)
        if (items.all { !it.own }) add(MessageAction.Report)
        add(MessageAction.DeleteForMe)
        if (items.all { canDeleteForEveryone(it) }) add(MessageAction.DeleteForEveryone)
    }
}
