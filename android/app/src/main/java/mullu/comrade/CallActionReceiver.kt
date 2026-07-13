package mullu.comrade

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import mullu.comrade.call.CallManager

/**
 * Handles the Decline action on the incoming-call notification's `CallStyle`
 * ([Notifier.notifyIncomingCall]). Accept has no entry here — it needs mic/
 * camera runtime permission a receiver cannot request, so its `PendingIntent`
 * just opens [MainActivity], where the ringing call screen already gates
 * Accept on those permissions.
 *
 * Declining works independent of whether [MainActivity]'s Compose tree is
 * currently active: it both ends the call ([CallManager.reject]) and clears
 * the notification directly, rather than relying on the UI's state-observing
 * side effect (which only runs while an Activity is composed).
 */
class CallActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val peer = intent.getStringExtra(EXTRA_PEER) ?: return
        if (intent.action == ACTION_DECLINE) {
            CallManager.reject()
            Notifier.clearCall(context, peer)
        }
    }

    companion object {
        const val ACTION_DECLINE = "mullu.comrade.call.DECLINE"
        const val EXTRA_PEER = "peer"
    }
}
