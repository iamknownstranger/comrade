package mullu.comrade.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The message-action decision vectors — each pinned so inverting the rule it
 * names would fail, matching `ChatMenuTest` and `TogetherDecisionsTest`.
 */
class MessageActionsTest {

    private fun ctx(
        own: Boolean = false,
        hasText: Boolean = true,
        isMedia: Boolean = false,
        ageMs: Long = 0,
        pinned: Boolean = false,
        starred: Boolean = false,
    ) = MessageContext(own, hasText, isMedia, ageMs, pinned, starred)

    // ── Edit ─────────────────────────────────────────────────────────────────

    @Test
    fun editOffersOnAnOwnTextMessageInsideTheWindow() {
        val actions = messageActions(ctx(own = true, hasText = true, ageMs = EDIT_WINDOW_MS - 1))
        assertTrue(actions.contains(MessageAction.Edit))
    }

    @Test
    fun editIsGoneTheInstantTheWindowCloses() {
        val actions = messageActions(ctx(own = true, hasText = true, ageMs = EDIT_WINDOW_MS + 1))
        assertFalse(actions.contains(MessageAction.Edit))
    }

    @Test
    fun editNeverOffersOnSomeoneElsesMessage() {
        val actions = messageActions(ctx(own = false, hasText = true, ageMs = 0))
        assertFalse(actions.contains(MessageAction.Edit))
    }

    @Test
    fun editNeverOffersOnAMessageWithNoText() {
        val actions = messageActions(ctx(own = true, hasText = false, ageMs = 0))
        assertFalse(actions.contains(MessageAction.Edit))
    }

    // ── DeleteForEveryone / DeleteForMe ──────────────────────────────────────

    @Test
    fun deleteForEveryoneOffersOnAnOwnMessageInsideItsWindow() {
        val actions =
            messageActions(ctx(own = true, ageMs = DELETE_FOR_EVERYONE_WINDOW_MS - 1))
        assertTrue(actions.contains(MessageAction.DeleteForEveryone))
    }

    @Test
    fun deleteForEveryoneIsGoneOnceItsWindowCloses() {
        val actions =
            messageActions(ctx(own = true, ageMs = DELETE_FOR_EVERYONE_WINDOW_MS + 1))
        assertFalse(actions.contains(MessageAction.DeleteForEveryone))
    }

    @Test
    fun deleteForEveryoneNeverOffersOnSomeoneElsesMessage() {
        val actions = messageActions(ctx(own = false, ageMs = 0))
        assertFalse(actions.contains(MessageAction.DeleteForEveryone))
    }

    /**
     * The ordering that must never invert: anything still rewritable is still
     * retractable. A sender who can edit but not delete rewrites the message to
     * nothing instead, so closing the delete window first buys no restraint —
     * it only takes away the clean way to do what they will do anyway.
     */
    @Test
    fun retractionOutlivesRewriting() {
        assertTrue(DELETE_FOR_EVERYONE_WINDOW_MS >= EDIT_WINDOW_MS)
    }

    /** Whenever [MessageAction.Edit] is offered, so is [MessageAction.DeleteForEveryone]. */
    @Test
    fun editIsNeverOfferedWithoutDeleteForEveryone() {
        for (age in listOf(0L, EDIT_WINDOW_MS / 2, EDIT_WINDOW_MS - 1, EDIT_WINDOW_MS)) {
            val actions = messageActions(ctx(own = true, hasText = true, ageMs = age))
            if (actions.contains(MessageAction.Edit)) {
                assertTrue(
                    "editable at ${'$'}age ms but not retractable",
                    actions.contains(MessageAction.DeleteForEveryone),
                )
            }
        }
    }

    @Test
    fun deleteForMeIsAlwaysThereRegardlessOfOwnershipOrAge() {
        assertTrue(
            messageActions(ctx(own = false, ageMs = Long.MAX_VALUE / 2))
                .contains(MessageAction.DeleteForMe),
        )
        assertTrue(
            messageActions(ctx(own = true, ageMs = Long.MAX_VALUE / 2))
                .contains(MessageAction.DeleteForMe),
        )
    }

    // ── Report ───────────────────────────────────────────────────────────────

    @Test
    fun reportOffersOnlyOnIncomingMessages() {
        assertTrue(messageActions(ctx(own = false)).contains(MessageAction.Report))
    }

    @Test
    fun reportNeverOffersOnYourOwnMessage() {
        assertFalse(messageActions(ctx(own = true)).contains(MessageAction.Report))
    }

    // ── SaveMedia / Copy ─────────────────────────────────────────────────────

    @Test
    fun saveMediaOffersOnlyOnMediaMessages() {
        assertTrue(messageActions(ctx(isMedia = true)).contains(MessageAction.SaveMedia))
        assertFalse(messageActions(ctx(isMedia = false)).contains(MessageAction.SaveMedia))
    }

    @Test
    fun copyOffersOnlyWhenThereIsText() {
        assertTrue(messageActions(ctx(hasText = true)).contains(MessageAction.Copy))
        assertFalse(messageActions(ctx(hasText = false)).contains(MessageAction.Copy))
    }

    // ── Pin/Unpin, Star/Unstar ───────────────────────────────────────────────

    @Test
    fun exactlyOneOfPinOrUnpinAppears() {
        val unpinned = messageActions(ctx(pinned = false))
        assertTrue(unpinned.contains(MessageAction.Pin))
        assertFalse(unpinned.contains(MessageAction.Unpin))

        val pinned = messageActions(ctx(pinned = true))
        assertTrue(pinned.contains(MessageAction.Unpin))
        assertFalse(pinned.contains(MessageAction.Pin))
    }

    @Test
    fun exactlyOneOfStarOrUnstarAppears() {
        val unstarred = messageActions(ctx(starred = false))
        assertTrue(unstarred.contains(MessageAction.Star))
        assertFalse(unstarred.contains(MessageAction.Unstar))

        val starred = messageActions(ctx(starred = true))
        assertTrue(starred.contains(MessageAction.Unstar))
        assertFalse(starred.contains(MessageAction.Star))
    }

    // ── Destructive sorts last ───────────────────────────────────────────────

    @Test
    fun deleteForEveryoneIsTheOnlyDestructiveEntry() {
        assertEquals(
            listOf(MessageAction.DeleteForEveryone),
            MessageAction.values().filter { it.destructive },
        )
    }

    @Test
    fun destructiveNeverHasAnythingAfterIt() {
        val actions = messageActions(ctx(own = true, hasText = true, isMedia = true, ageMs = 0))
        val idx = actions.indexOf(MessageAction.DeleteForEveryone)
        assertTrue("expected DeleteForEveryone to be offered", idx >= 0)
        assertEquals(actions.size - 1, idx)
    }

    // ── Order relationship with the existing sheet ───────────────────────────

    @Test
    fun replyReplyInThreadAssignTopicCopyKeepTheirExistingRelativeOrder() {
        val actions = messageActions(ctx(hasText = true))
        val reply = actions.indexOf(MessageAction.Reply)
        val replyInThread = actions.indexOf(MessageAction.ReplyInThread)
        val assignTopic = actions.indexOf(MessageAction.AssignTopic)
        val copy = actions.indexOf(MessageAction.Copy)
        assertTrue(reply < replyInThread)
        assertTrue(replyInThread < assignTopic)
        assertTrue(assignTopic < copy)
    }

    @Test
    fun reactIsAlwaysFirst() {
        assertEquals(MessageAction.React, messageActions(ctx()).first())
    }

    // ── Selection: only what applies to every item ──────────────────────────

    @Test
    fun forwardIsFineOnAMixedSelection() {
        val items = listOf(
            ctx(hasText = true, isMedia = false),
            ctx(hasText = false, isMedia = true),
        )
        assertTrue(selectionActions(items).contains(MessageAction.Forward))
    }

    @Test
    fun editingOneOfManySelectedIsNotOffered() {
        val items = listOf(
            ctx(own = true, hasText = true, ageMs = 0),
            ctx(own = false, hasText = true, ageMs = 0),
        )
        assertFalse(selectionActions(items).contains(MessageAction.Edit))
    }

    @Test
    fun copyAppliesIfAnySelectedItemHasText() {
        val items = listOf(ctx(hasText = false, isMedia = true), ctx(hasText = true))
        assertTrue(selectionActions(items).contains(MessageAction.Copy))
    }

    @Test
    fun copyIsAbsentWhenNoSelectedItemHasText() {
        val items = listOf(ctx(hasText = false, isMedia = true), ctx(hasText = false, isMedia = true))
        assertFalse(selectionActions(items).contains(MessageAction.Copy))
    }

    @Test
    fun saveMediaAppliesIfAnySelectedItemIsMedia() {
        val items = listOf(ctx(isMedia = false), ctx(isMedia = true))
        assertTrue(selectionActions(items).contains(MessageAction.SaveMedia))
    }

    @Test
    fun starTogglesToUnstarOnlyWhenAllAreStarred() {
        val mixed = listOf(ctx(starred = true), ctx(starred = false))
        assertTrue(selectionActions(mixed).contains(MessageAction.Star))
        assertFalse(selectionActions(mixed).contains(MessageAction.Unstar))

        val allStarred = listOf(ctx(starred = true), ctx(starred = true))
        assertTrue(selectionActions(allStarred).contains(MessageAction.Unstar))
        assertFalse(selectionActions(allStarred).contains(MessageAction.Star))
    }

    @Test
    fun reportRequiresEveryItemToBeIncoming() {
        val mixed = listOf(ctx(own = false), ctx(own = true))
        assertFalse(selectionActions(mixed).contains(MessageAction.Report))

        val allIncoming = listOf(ctx(own = false), ctx(own = false))
        assertTrue(selectionActions(allIncoming).contains(MessageAction.Report))
    }

    @Test
    fun deleteForEveryoneRequiresEveryItemToQualify() {
        val mixed = listOf(
            ctx(own = true, ageMs = 0),
            ctx(own = true, ageMs = DELETE_FOR_EVERYONE_WINDOW_MS + 1),
        )
        assertFalse(selectionActions(mixed).contains(MessageAction.DeleteForEveryone))

        val allQualify = listOf(ctx(own = true, ageMs = 0), ctx(own = true, ageMs = 0))
        assertTrue(selectionActions(allQualify).contains(MessageAction.DeleteForEveryone))
    }

    @Test
    fun deleteForMeIsAlwaysInASelection() {
        val items = listOf(ctx(own = false), ctx(own = true))
        assertTrue(selectionActions(items).contains(MessageAction.DeleteForMe))
    }

    @Test
    fun selectionHasNoReplyReplyInThreadAssignTopicEditSelectOrMessageInfo() {
        val items = listOf(ctx(own = true, hasText = true, ageMs = 0))
        val actions = selectionActions(items)
        assertFalse(actions.contains(MessageAction.Reply))
        assertFalse(actions.contains(MessageAction.ReplyInThread))
        assertFalse(actions.contains(MessageAction.AssignTopic))
        assertFalse(actions.contains(MessageAction.Edit))
        assertFalse(actions.contains(MessageAction.Select))
        assertFalse(actions.contains(MessageAction.MessageInfo))
        assertFalse(actions.contains(MessageAction.React))
    }

    // ── Selection cap ────────────────────────────────────────────────────────

    @Test
    fun canAddToSelectionBelowTheCap() {
        assertTrue(canAddToSelection(MAX_SELECTION_SIZE - 1))
    }

    @Test
    fun cannotAddToSelectionAtTheCap() {
        assertFalse(canAddToSelection(MAX_SELECTION_SIZE))
    }

    @Test
    fun selectionActionsIsEmptyPastTheCap() {
        val tooMany = List(MAX_SELECTION_SIZE + 1) { ctx() }
        assertTrue(selectionActions(tooMany).isEmpty())
    }

    @Test
    fun selectionActionsIsNotEmptyAtTheCap() {
        val atCap = List(MAX_SELECTION_SIZE) { ctx() }
        assertTrue(selectionActions(atCap).isNotEmpty())
    }
}
