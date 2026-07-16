package mullu.comrade.call

import android.app.Notification
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.app.Person
import mullu.comrade.CallActionReceiver
import mullu.comrade.MainActivity
import mullu.comrade.Notifier

/**
 * Keeps the process alive and visible while a call has live media, so
 * backgrounding the app (checking another app mid-call, letting the screen
 * lock) can't get the process reclaimed and the call dropped — a plain
 * foreground Activity offers no such guarantee once the user navigates away.
 *
 * Holds only the foreground-service contract (the ongoing notification with
 * its hang-up action and tap-to-return); [CallManager] still owns all the
 * actual media/signaling state. [CallManager] is also what starts and stops
 * this service — from the same call-setup/teardown points that already start
 * and stop audio routing — rather than the UI layer, so it doesn't depend on
 * any Activity/Compose tree being alive to fire correctly.
 */
class CallService : Service() {

    override fun onCreate() {
        super.onCreate()
        // Honor the foreground-service contract *immediately*: this service
        // is always started with startForegroundService(), which demands a
        // startForeground() call before the service stops — including when
        // onStartCommand bails on a blank redelivered intent, and when
        // CallManager's teardown stopService() lands before onStartCommand
        // has even run (a call that failed in the same breath it was
        // placed). Missing that window is not an error state Android logs —
        // it is a hard crash of the whole process
        // (ForegroundServiceDidNotStartInTimeException). Go foreground with
        // a generic placeholder here; onStartCommand replaces it in place
        // (same NOTIFICATION_ID) with the real peer-labeled notification.
        startForegroundNotified(peer = "", peerLabel = getString(mullu.comrade.R.string.call_voice), video = false)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val peer = intent?.getStringExtra(EXTRA_PEER)
        if (peer.isNullOrEmpty()) {
            // A system-triggered restart (e.g. the process was killed under
            // memory pressure) can redeliver a blank/null intent with no
            // guarantee the original extras survived. There is no call to
            // show — stop. Safe contract-wise: onCreate already called
            // startForeground, and stopping removes its notification.
            stopSelf()
            return START_NOT_STICKY
        }
        val peerLabel = intent.getStringExtra(EXTRA_PEER_LABEL)?.ifBlank { null } ?: peer
        val video = intent.getBooleanExtra(EXTRA_VIDEO, false)
        startForegroundNotified(peer, peerLabel, video)
        return START_NOT_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun startForegroundNotified(peer: String, peerLabel: String, video: Boolean) {
        Notifier.ensureChannels(this)
        val notification = buildNotification(peer, peerLabel)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            val type = if (video) {
                ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE or ServiceInfo.FOREGROUND_SERVICE_TYPE_CAMERA
            } else {
                ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE
            }
            startForeground(NOTIFICATION_ID, notification, type)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    /** [NotificationCompat.CallStyle.forOngoingCall] — a hang-up action, tap-to-return via [MainActivity]. */
    private fun buildNotification(peer: String, peerLabel: String): Notification {
        val openApp = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java).apply {
                flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
            },
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val hangUpIntent = PendingIntent.getBroadcast(
            this,
            peer.hashCode(),
            Intent(this, CallActionReceiver::class.java)
                .setAction(CallActionReceiver.ACTION_HANGUP)
                .putExtra(CallActionReceiver.EXTRA_PEER, peer),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val person = Person.Builder().setName(peerLabel).build()
        val style = NotificationCompat.CallStyle.forOngoingCall(person, hangUpIntent)
        return NotificationCompat.Builder(this, Notifier.CHANNEL_CALLS)
            .setSmallIcon(android.R.drawable.sym_action_call)
            .setContentTitle(peerLabel)
            .setStyle(style)
            .addPerson(person)
            .setOngoing(true)
            .setUsesChronometer(true) // a live-ticking duration, no polling needed on our side
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setContentIntent(openApp)
            .build()
    }

    companion object {
        private const val NOTIFICATION_ID = 0xCA11
        private const val EXTRA_PEER = "peer"
        private const val EXTRA_PEER_LABEL = "peerLabel"
        private const val EXTRA_VIDEO = "video"

        fun start(context: Context, peer: String, peerLabel: String, video: Boolean) {
            val intent = Intent(context, CallService::class.java)
                .putExtra(EXTRA_PEER, peer)
                .putExtra(EXTRA_PEER_LABEL, peerLabel)
                .putExtra(EXTRA_VIDEO, video)
            context.startForegroundService(intent)
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, CallService::class.java))
        }
    }
}
