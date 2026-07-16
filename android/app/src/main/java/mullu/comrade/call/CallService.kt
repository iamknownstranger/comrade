package mullu.comrade.call

import android.app.Notification
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.util.Log
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

    /**
     * Whether [startForeground] actually promoted this service (set in
     * [onCreate]). If the platform refused that promotion — an API 31+
     * background-start disallowal or an API 34+ foreground-service-type
     * permission failure — there is no valid foreground service and can never
     * be one for this instance, so [onStartCommand] must not try again.
     */
    private var foregroundStarted = false

    /**
     * Satisfy the foreground-service contract the instant this service exists —
     * before [onStartCommand] can bail on a blank intent (below) and before a
     * stop-before-start race ([stop]'s `stopService` dispatched right after
     * [start]'s `startForegroundService`, i.e. place-then-instant-cancel) can
     * destroy this instance without it ever having gone foreground.
     *
     * After `Context.startForegroundService()` the platform REQUIRES a
     * `startForeground()` within ~10s even if the service then stops itself
     * immediately; skipping it makes the platform throw
     * `ForegroundServiceDidNotStartInTimeException` and kill the whole process
     * ~10s later, asynchronously — which no `try`/`catch` at the call site can
     * intercept. A minimal placeholder that the valid-peer path immediately
     * replaces (same [NOTIFICATION_ID]) or that the blank-peer path / an
     * incoming stop immediately removes is the platform-sanctioned pattern; that
     * momentary neutral notification is strictly better than the process kill
     * the old [onStartCommand] comment accepted in order to avoid a "blank"
     * notification.
     *
     * Mic-only type: the placeholder never carries video, so it needs only
     * [ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE] — a subset of the
     * manifest's `camera|microphone`. The camera type is only ever added later,
     * by [startForegroundNotified], when the real call is video.
     */
    override fun onCreate() {
        super.onCreate()
        Notifier.ensureChannels(this)
        val placeholder = NotificationCompat.Builder(this, Notifier.CHANNEL_CALLS)
            .setSmallIcon(android.R.drawable.sym_action_call)
            .setContentTitle("Call")
            .setOngoing(true)
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .build()
        runCatching {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                startForeground(
                    NOTIFICATION_ID,
                    placeholder,
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE,
                )
            } else {
                startForeground(NOTIFICATION_ID, placeholder)
            }
        }.onSuccess {
            foregroundStarted = true
        }.onFailure { e ->
            // API 31+ can throw ForegroundServiceStartNotAllowedException (the
            // platform disallowing a background start); API 34+ can throw on a
            // type-permission failure (SecurityException). When the platform
            // itself refuses the start, this can never become a valid
            // foreground service, so stop cleanly. Note this refusal is NOT the
            // did-not-start-in-time kill — that fires only when startForeground
            // is never CALLED; here it was called and rejected, leaving nothing
            // armed.
            Log.w(TAG, "startForeground (placeholder) refused by platform; stopping", e)
            stopSelf()
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (!foregroundStarted) {
            // onCreate's placeholder startForeground was refused (already
            // logged) and this service is stopping. Retrying startForeground
            // would throw the same way, uncaught — and the contract no longer
            // applies once the platform has refused the start.
            return START_NOT_STICKY
        }
        val peer = intent?.getStringExtra(EXTRA_PEER)
        if (peer.isNullOrEmpty()) {
            // A system-triggered restart (e.g. the process was killed under
            // memory pressure) can redeliver a blank/null intent with no
            // guarantee the original extras survived, and there is no call to
            // represent. The foreground-service contract is already satisfied
            // (onCreate posted the placeholder), so — unlike before — bailing
            // here is safe: remove the placeholder and stop. Skipping
            // startForeground is now impossible, so this path can no longer arm
            // the delayed did-not-start-in-time process kill.
            stopForeground(Service.STOP_FOREGROUND_REMOVE)
            stopSelf()
            return START_NOT_STICKY
        }
        val peerLabel = intent?.getStringExtra(EXTRA_PEER_LABEL)?.ifBlank { null } ?: peer
        val video = intent?.getBooleanExtra(EXTRA_VIDEO, false) ?: false
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
        private const val TAG = "CallService"
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
