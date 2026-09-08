package mullu.comrade.capture

import android.app.Notification
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import mullu.comrade.MainActivity
import mullu.comrade.Notifier
import mullu.comrade.R

/**
 * Keeps the process — and with it, camera access — alive while a helmet-cam
 * recording is running, including with the screen off. That is not a nice-to
 * -have on top of the feature: since Android 11 a process with no visible
 * `Activity` cannot open the camera *at all* without a running foreground
 * service of type `camera`, so this service is what makes "press record,
 * press the lock button, keep riding" legal for the rest of the ride rather
 * than something the platform kills the moment the screen goes dark.
 *
 * Holds only the foreground-service contract and the ongoing notification —
 * [CaptureManager] still owns the camera, the recorder and every decision
 * about them, and is also what starts and stops this service, from the same
 * points it opens and closes the camera rather than from any Compose tree. A
 * recording must not end because a screen was disposed.
 *
 * ## The promotion contract
 * `startForeground` happens in [onCreate], before any intent is examined, and
 * again first thing in every [onStartCommand] — see
 * `.claude/rules/android.md`'s foreground-service section. The obligation
 * arms per *call* to `startForegroundService`, a refused promotion does not
 * cancel the pending kill, and a `runCatching` at the call site cannot catch
 * a throw that lands later on this service's own looper. The only safe move
 * is to make promotion succeed immediately and refine the notification (once
 * the real destination is known) afterwards.
 *
 * ## Screen on/off
 * `ACTION_SCREEN_OFF`/`ACTION_SCREEN_ON` cannot be declared in the manifest —
 * the platform only delivers them to a receiver registered at runtime — so
 * this service registers one dynamically in [onCreate] and unregisters it in
 * [onDestroy], routing both into [CaptureManager.onScreenStateChanged], which
 * is what actually reconfigures the capture session.
 *
 * ## Foreground service type
 * `camera|microphone`, matching what recording actually uses (there is no
 * "sometimes microphone" case the way a voice-only call has no camera — this
 * feature always uses both). On Android 14+ an FGS of these types may not be
 * *started* from the background, which this app never needs: the only way
 * into this feature is a tap on a visible screen, in [CaptureManager.start].
 */
class CaptureService : Service() {

    private var destination: CaptureDecisions.Destination = CaptureDecisions.Destination.Gallery

    private val screenReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            when (intent.action) {
                Intent.ACTION_SCREEN_OFF -> CaptureManager.onScreenStateChanged(screenOn = false)
                Intent.ACTION_SCREEN_ON -> CaptureManager.onScreenStateChanged(screenOn = true)
            }
        }
    }

    override fun onCreate() {
        super.onCreate()
        Notifier.ensureChannels(this)
        promote(destination)
        ContextCompat.registerReceiver(
            this,
            screenReceiver,
            IntentFilter().apply {
                addAction(Intent.ACTION_SCREEN_OFF)
                addAction(Intent.ACTION_SCREEN_ON)
            },
            // These are protected system broadcasts — only the platform can
            // ever send them — so there is no other app this could reasonably
            // accept the intent from either way.
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        intent?.getStringExtra(EXTRA_DESTINATION)?.let { raw ->
            destination = runCatching { CaptureDecisions.Destination.valueOf(raw) }.getOrDefault(destination)
        }
        promote(destination)
        if (intent?.action == ACTION_STOP) {
            CaptureManager.stop(this)
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        runCatching { unregisterReceiver(screenReceiver) }
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun promote(destination: CaptureDecisions.Destination) {
        val notification = build(destination)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(
                NOTIFICATION_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_CAMERA or ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE,
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    private fun build(destination: CaptureDecisions.Destination): Notification {
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val stop = PendingIntent.getService(
            this,
            1,
            Intent(this, CaptureService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val text = getString(
            if (destination == CaptureDecisions.Destination.Gallery) {
                R.string.capture_notification_gallery
            } else {
                R.string.capture_notification_private
            },
        )
        return NotificationCompat.Builder(this, Notifier.CHANNEL_CAPTURE)
            .setSmallIcon(android.R.drawable.ic_menu_camera)
            .setContentTitle(getString(R.string.capture_notification_title))
            .setContentText(text)
            .setContentIntent(open)
            .setOngoing(true)
            // An ongoing recording must never make a sound of its own — see
            // Notifier.CHANNEL_CAPTURE, which is IMPORTANCE_LOW for the same
            // reason; this is belt-and-braces for platforms that ignore the
            // channel.
            .setSilent(true)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .addAction(0, getString(R.string.capture_action_stop), stop)
            .build()
    }

    companion object {
        private const val NOTIFICATION_ID = 0xCA33
        private const val EXTRA_DESTINATION = "destination"
        const val ACTION_STOP = "mullu.comrade.capture.STOP"

        /**
         * Start — or re-announce with a changed [destination] label — the
         * recording service. Re-announcing reuses [NOTIFICATION_ID], so it
         * updates the row rather than adding a second one.
         */
        fun start(context: Context, destination: CaptureDecisions.Destination) {
            context.startForegroundService(
                Intent(context, CaptureService::class.java)
                    .putExtra(EXTRA_DESTINATION, destination.name),
            )
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, CaptureService::class.java))
        }
    }
}
