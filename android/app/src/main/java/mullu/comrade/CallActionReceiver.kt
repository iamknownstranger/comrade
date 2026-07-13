package mullu.comrade

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import mullu.comrade.call.CallManager

/**
 * Handles the Decline and Hang Up actions on the call notifications'
 * `CallStyle` ([Notifier.notifyIncomingCall], [mullu.comrade.call.CallService]'s
 * ongoing-call notification). Accept has no entry here — it needs mic/camera
 * runtime permission a receiver cannot request, so its `PendingIntent` just
 * opens [MainActivity], where the ringing call screen already gates Accept on
 * those permissions.
 *
 * Declining works independent of whether [MainActivity]'s Compose tree is
 * currently active: it both ends the call ([CallManager.reject]) and clears
 * the notification directly, rather than relying on the UI's state-observing
 * side effect (which only runs while an Activity is composed). Hanging up
 * needs no such direct clear — [CallManager.hangup] tears the call down, which
 * stops [mullu.comrade.call.CallService] and removes its notification with it.
 */
class CallActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val peer = intent.getStringExtra(EXTRA_PEER) ?: return
        when (intent.action) {
            ACTION_DECLINE -> {
                CallManager.reject()
                Notifier.clearCall(context, peer)
            }
            ACTION_HANGUP -> CallManager.hangup()
        }
    }

    companion object {
        const val ACTION_DECLINE = "mullu.comrade.call.DECLINE"
        const val ACTION_HANGUP = "mullu.comrade.call.HANGUP"
        const val EXTRA_PEER = "peer"
    }
}
