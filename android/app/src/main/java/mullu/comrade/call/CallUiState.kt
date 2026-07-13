package mullu.comrade.call

/**
 * The four call phases the UI renders, plus [Idle]. Mirrors the desktop
 * webview's `phase` field (`calling|ringing|connecting|connected`), collapsed to
 * the states this task specifies: Ringing, Connecting, Active, Ended.
 *
 * `incoming` distinguishes the callee (who sees Accept/Decline) from the caller
 * (who sees Cancel) while ringing, and picks the "Calling…" vs "Ringing…" label.
 * `peer` is the npub; `peerLabel` is the already-resolved display title.
 */
sealed interface CallUiState {
    /** No call in flight — the overlay is hidden. */
    data object Idle : CallUiState

    /** Incoming call ringing (callee), or outgoing call placed (caller). */
    data class Ringing(
        val peer: String,
        val peerLabel: String,
        val video: Boolean,
        val incoming: Boolean,
        /** Caller side: the callee's device has acked the ring ("Ringing…" vs "Calling…"). */
        val remoteRinging: Boolean = false,
    ) : CallUiState

    /** Negotiating: offer/answer exchanged, waiting for the media path. */
    data class Connecting(
        val peer: String,
        val peerLabel: String,
        val video: Boolean,
        val incoming: Boolean,
    ) : CallUiState

    /** Connected — media flowing. [connectedAtMs] seeds the duration timer. */
    data class Active(
        val peer: String,
        val peerLabel: String,
        val video: Boolean,
        val incoming: Boolean,
        val connectedAtMs: Long,
    ) : CallUiState

    /**
     * Terminal card shown briefly before returning to [Idle]. [peer]/[incoming]
     * ride alongside [peerLabel] so call-history/missed-call logic can key off
     * them once the call's underlying session is already gone.
     */
    data class Ended(
        val peer: String,
        val peerLabel: String,
        val video: Boolean,
        val incoming: Boolean,
        val outcome: String,
    ) : CallUiState
}
