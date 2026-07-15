package mullu.comrade

import android.annotation.SuppressLint
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.Person
import androidx.core.content.ContextCompat
import mullu.comrade.ui.shortNpub

/**
 * System notifications for incoming activity (messages, message requests,
 * calls). Kept deliberately small and content-light — a notification never
 * carries decrypted message text into the OS notification store beyond a short
 * preview, matching the app's privacy posture.
 *
 * Honest scope: these fire while the app process is alive (foreground, or held
 * by the in-call/connection service). Always-on background delivery when the
 * process is dead would need a persistent connection service or push, which is
 * tracked as follow-up in AUDIT §7.
 */
object Notifier {
    const val CHANNEL_MESSAGES = "comrade_messages"
    const val CHANNEL_REQUESTS = "comrade_requests"

    /**
     * `_v2`: channel settings (including sound) are sticky once created — the
     * OS never lets an app change them after the fact, so an existing install
     * would keep its original ringtone forever. Bumping the id is what makes
     * a silent channel actually take effect for upgrading users, not just
     * fresh installs. See [Ringer], which owns call audio exclusively; this
     * channel deliberately posts silent (see [ensureChannels]) so the two
     * never sound at once.
     */
    const val CHANNEL_CALLS = "comrade_calls_v2"
    const val CHANNEL_CONNECTION = "comrade_connection"

    private const val GROUP_MESSAGES = "comrade_messages_group"

    /** Register notification channels once (no-op on < O). */
    fun ensureChannels(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val mgr = context.getSystemService(NotificationManager::class.java) ?: return
        mgr.createNotificationChannel(
            NotificationChannel(
                CHANNEL_MESSAGES,
                "Messages",
                NotificationManager.IMPORTANCE_HIGH,
            ).apply { description = "New encrypted direct messages" },
        )
        mgr.createNotificationChannel(
            NotificationChannel(
                CHANNEL_REQUESTS,
                "Message requests",
                NotificationManager.IMPORTANCE_DEFAULT,
            ).apply { description = "Messages from people you haven't accepted yet" },
        )
        mgr.createNotificationChannel(
            NotificationChannel(
                CHANNEL_CALLS,
                "Calls",
                NotificationManager.IMPORTANCE_HIGH,
            ).apply {
                description = "Incoming voice and video calls"
                // Ringer (CallScreen/MainActivity) owns call audio exclusively
                // — a second ringtone from the notification itself would
                // double up on top of it.
                setSound(null, null)
            },
        )
        mgr.createNotificationChannel(
            NotificationChannel(
                CHANNEL_CONNECTION,
                "Background connection",
                NotificationManager.IMPORTANCE_MIN,
            ).apply { description = "Comrade is staying connected while unlocked in the background" },
        )
    }

    private fun canPost(context: Context): Boolean =
        NotificationManagerCompat.from(context).areNotificationsEnabled()

    private fun openAppIntent(context: Context): PendingIntent {
        val intent = Intent(context, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
        }
        return PendingIntent.getActivity(
            context,
            0,
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }

    /** A new encrypted DM from an accepted conversation. */
    @SuppressLint("MissingPermission") // guarded by canPost() / areNotificationsEnabled()
    fun notifyMessage(context: Context, peer: String, title: String, preview: String) {
        if (!canPost(context)) return
        val n = NotificationCompat.Builder(context, CHANNEL_MESSAGES)
            .setSmallIcon(android.R.drawable.sym_action_chat)
            .setContentTitle(title.ifBlank { shortNpub(peer) })
            .setContentText(preview)
            .setAutoCancel(true)
            .setGroup(GROUP_MESSAGES)
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
            .setContentIntent(openAppIntent(context))
            .build()
        // Stable per-peer id so repeated messages from one peer collapse.
        NotificationManagerCompat.from(context).notify(peer.hashCode(), n)
    }

    /** A stranger's DM landed in the message-requests bucket. */
    @SuppressLint("MissingPermission") // guarded by canPost() / areNotificationsEnabled()
    fun notifyRequest(context: Context, peer: String, preview: String) {
        if (!canPost(context)) return
        val n = NotificationCompat.Builder(context, CHANNEL_REQUESTS)
            .setSmallIcon(android.R.drawable.sym_action_chat)
            .setContentTitle("Message request")
            .setContentText(preview)
            .setAutoCancel(true)
            .setContentIntent(openAppIntent(context))
            .build()
        NotificationManagerCompat.from(context).notify("req:$peer".hashCode(), n)
    }

    /**
     * An incoming call is ringing. Uses [NotificationCompat.CallStyle] so the
     * notification renders as a real incoming call — Decline and Answer actions,
     * full-screen on the lock screen — with AndroidX's compat rendering on
     * older platforms. Decline broadcasts to [CallActionReceiver] (no runtime
     * permission needed); Answer just opens the app, where the ringing screen
     * gates Accept on the mic/camera permission a receiver couldn't request.
     */
    @SuppressLint("MissingPermission") // guarded by canPost() / areNotificationsEnabled()
    fun notifyIncomingCall(context: Context, peer: String, title: String, video: Boolean) {
        if (!canPost(context)) return
        val kind = if (video) "video" else "voice"
        val openApp = openAppIntent(context)
        val declineIntent = PendingIntent.getBroadcast(
            context,
            peer.hashCode(),
            Intent(context, CallActionReceiver::class.java)
                .setAction(CallActionReceiver.ACTION_DECLINE)
                .putExtra(CallActionReceiver.EXTRA_PEER, peer),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val caller = Person.Builder().setName(title.ifBlank { shortNpub(peer) }).build()
        val style = NotificationCompat.CallStyle.forIncomingCall(caller, declineIntent, openApp)
        val n = NotificationCompat.Builder(context, CHANNEL_CALLS)
            .setSmallIcon(android.R.drawable.sym_action_call)
            .setContentTitle("Incoming $kind call")
            .setContentText(title.ifBlank { shortNpub(peer) })
            .setStyle(style)
            .addPerson(caller)
            // A ringing call isn't swipe-dismissable — Decline/Answer are the
            // intended exits, matching how the OS's own dialer behaves.
            .setOngoing(true)
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setFullScreenIntent(openApp, true)
            .setContentIntent(openApp)
            .build()
        NotificationManagerCompat.from(context).notify("call:$peer".hashCode(), n)
    }

    /**
     * The callee side missed an incoming call — the ring timed out unanswered
     * before the user accepted or declined. Posted on the messages channel
     * (a "you missed something" notice, not an active-call alert) under a
     * `missed:`-prefixed id so it can't collide with, or be silently cleared
     * by, [clearCall]/[clearForPeer]'s `call:$peer` id.
     */
    @SuppressLint("MissingPermission") // guarded by canPost() / areNotificationsEnabled()
    fun notifyMissedCall(context: Context, peer: String, peerLabel: String) {
        if (!canPost(context)) return
        val n = NotificationCompat.Builder(context, CHANNEL_MESSAGES)
            .setSmallIcon(android.R.drawable.sym_action_call)
            .setContentTitle("Missed call")
            .setContentText(peerLabel)
            .setAutoCancel(true)
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setContentIntent(openAppIntent(context))
            .build()
        NotificationManagerCompat.from(context).notify("missed:$peer".hashCode(), n)
    }

    /** Clear any notification we posted for `peer` (e.g. on opening the chat). */
    fun clearForPeer(context: Context, peer: String) {
        val mgr = NotificationManagerCompat.from(context)
        mgr.cancel(peer.hashCode())
        mgr.cancel("req:$peer".hashCode())
        mgr.cancel("call:$peer".hashCode())
    }

    /** Clear only the incoming-call notification for `peer` (call answered/ended). */
    fun clearCall(context: Context, peer: String) {
        NotificationManagerCompat.from(context).cancel("call:$peer".hashCode())
    }

    /** Whether POST_NOTIFICATIONS is granted (always true below Android 13). */
    fun hasPermission(context: Context): Boolean =
        Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
            ContextCompat.checkSelfPermission(
                context,
                android.Manifest.permission.POST_NOTIFICATIONS,
            ) == android.content.pm.PackageManager.PERMISSION_GRANTED
}
