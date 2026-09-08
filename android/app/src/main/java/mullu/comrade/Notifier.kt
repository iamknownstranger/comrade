package mullu.comrade

import android.annotation.SuppressLint
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import android.util.Log
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
     * Messages and requests that arrived during Comrade's own quiet hours.
     *
     * A separate channel rather than a flag, because on Android O+ the channel
     * is the only thing that actually governs sound: a `setSilent(true)` on a
     * builder posting to an `IMPORTANCE_HIGH` channel is advisory at best. This
     * one is `IMPORTANCE_LOW`, so a message that lands at 02:00 reaches the
     * shade — and waits there — without making a sound.
     *
     * It exists because the alternative shipped for a while: quiet hours used to
     * mean *no notification at all*, so a message sent overnight was invisible
     * until the app was next opened. See [NotifyDecision.Silent].
     */
    const val CHANNEL_MESSAGES_QUIET = "comrade_messages_quiet"

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

    /**
     * On-demand model downloads (speech, companion): the ongoing progress
     * notification plus the tappable "ready" / "failed" one that replaces it.
     * See [mullu.comrade.model.ModelDownloadService].
     */
    const val CHANNEL_MODELS = "comrade_models"

    /**
     * A comrade came online. Its own channel so it can be silenced (or turned
     * off entirely) from system settings without touching message or call
     * alerts — "someone I chose is around" is a gentler class of notification
     * than "someone messaged you", and a user who wants the dot but not the
     * buzz should be able to say so in one place.
     */
    const val CHANNEL_COMRADES = "comrade_presence"

    /**
     * A newer Comrade has been published. Its own channel because it is the one
     * class of notification that is not about a person: someone who wants to be
     * told about messages but not about releases (or the reverse) can say so in
     * system settings without collateral damage. See [mullu.comrade.update.UpdateChecker].
     */
    const val CHANNEL_UPDATES = "comrade_updates"

    /**
     * Someone asked you to listen or watch with them.
     *
     * Its own channel rather than sharing [CHANNEL_CALLS], because it is not a
     * call: nothing is ringing and nothing is waiting on a timer at the other
     * end. It is also not [CHANNEL_MESSAGES] — an invitation expires in the
     * sense that the other person is sitting there, so it is worth an
     * interruption where a message may not be, and someone who wants messages
     * silent and this loud (or the reverse) can say so in one place.
     */
    const val CHANNEL_TOGETHER = "comrade_together"

    /**
     * A helmet-cam recording is in progress (`capture/CaptureService.kt`).
     * Its own channel because it is ongoing for as long as a ride is, and it
     * must never sound: `IMPORTANCE_LOW` here is not cosmetic, it is the
     * whole reason a several-hour recording doesn't buzz the phone every time
     * the notification is refreshed with a new segment count.
     */
    const val CHANNEL_CAPTURE = "comrade_capture"

    private const val GROUP_MESSAGES = "comrade_messages_group"
    private const val GROUP_COMRADES = "comrade_presence_group"

    /**
     * How long an "is online" notice may sit in the shade unattended.
     * Matches `comrade_core::presence::PRESENCE_TTL_SECS` — the point past
     * which the claim it was raised from is no longer believed anywhere else
     * either, so leaving it up would be the notification shade disagreeing
     * with every dot in the app.
     */
    private const val ONLINE_TIMEOUT_MS = 480_000L

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
                CHANNEL_MESSAGES_QUIET,
                "Messages during quiet hours",
                // LOW is the whole point: it reaches the shade and stays there
                // without a sound, which is what "quiet" should have always meant.
                NotificationManager.IMPORTANCE_LOW,
            ).apply {
                description = "Messages that arrived inside your quiet hours — delivered without a sound"
                setSound(null, null)
                enableVibration(false)
            },
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
        mgr.createNotificationChannel(
            NotificationChannel(
                CHANNEL_COMRADES,
                "Comrades online",
                // DEFAULT, not HIGH: worth a glance, never worth interrupting
                // what you were doing.
                NotificationManager.IMPORTANCE_DEFAULT,
            ).apply { description = "When someone you chose as a comrade comes online" },
        )
        mgr.createNotificationChannel(
            NotificationChannel(
                CHANNEL_MODELS,
                "Model downloads",
                // LOW: a long download should show progress without buzzing;
                // the one-off "ready" notification still lands in the shade.
                NotificationManager.IMPORTANCE_LOW,
            ).apply { description = "Progress for on-device speech and companion model downloads" },
        )
        mgr.createNotificationChannel(
            NotificationChannel(
                CHANNEL_UPDATES,
                "App updates",
                // LOW: a new release is worth finding in the shade, never worth
                // interrupting anything. It is also the one notice a user may
                // reasonably want off entirely — hence its own channel.
                NotificationManager.IMPORTANCE_LOW,
            ).apply { description = "When a newer version of Comrade has been published" },
        )
        mgr.createNotificationChannel(
            NotificationChannel(
                CHANNEL_TOGETHER,
                "Listen together",
                // HIGH: the other person is waiting at the other end of this
                // one, which is what separates it from "someone came online".
                NotificationManager.IMPORTANCE_HIGH,
            ).apply { description = "When someone asks you to listen or watch something with them" },
        )
        mgr.createNotificationChannel(
            NotificationChannel(
                CHANNEL_CAPTURE,
                context.getString(R.string.capture_channel_name),
                // LOW: an ongoing recording notification must not make a
                // sound, on this ride or the next one.
                NotificationManager.IMPORTANCE_LOW,
            ).apply { description = context.getString(R.string.capture_channel_description) },
        )
    }

    private const val TAG = "Notifier"

    private fun canPost(context: Context): Boolean =
        NotificationManagerCompat.from(context).areNotificationsEnabled()

    /**
     * Post [notification] under [id], absorbing a platform refusal.
     *
     * Every post in this file goes through here, and the reason is the caller
     * rather than the post: these run on [EventPump]'s drain loop, the
     * process's only consumer of the native event queue. `notify` can throw
     * under conditions that are ordinary rather than exotic — a `CallStyle`
     * the platform declines is the known case (`call/CallService.kt` has
     * guarded its own post for this reason for some time) — and an escaping
     * throw used to end that loop, so failing to show one notification
     * silently cost every later message and call too.
     *
     * `drainLoop` now catches this class of failure as well. This is the inner
     * of the two guards and the one that keeps the loop from ever having to:
     * losing a single notification is a small, local failure, and it should
     * read as one in the log rather than as a dropped event.
     */
    private fun post(context: Context, id: Int, notification: Notification) {
        runCatching { NotificationManagerCompat.from(context).notify(id, notification) }
            .onFailure { Log.w(TAG, "notification $id was refused by the platform", it) }
    }

    /**
     * The id of the message group's summary. Posted alongside per-peer message
     * notifications so several of them collapse into one "Comrade" entry
     * instead of stacking; dropped again by [pruneMessageSummary] once no
     * children are left, because a summary alone in the shade is a
     * notification that says nothing.
     */
    private const val MESSAGE_SUMMARY_ID = 0xC0DE20

    /**
     * Where a tapped notification lands.
     *
     * [peer] opens that conversation (WP11 in `docs/COMMS_ARCHITECTURE.md`:
     * before this, every notification dumped the user on whatever screen they
     * had left, and they had to find the chat themselves). [screen] pushes a
     * non-tab destination, e.g. Settings for an update notice.
     *
     * The request code is derived from the target rather than left at 0.
     * `FLAG_UPDATE_CURRENT` mutates the *existing* PendingIntent when the
     * request code matches, so a shared 0 would have every message
     * notification in the shade quietly re-point at whichever peer notified
     * last — a bug that only shows up with two conversations unread.
     */
    private fun openAppIntent(
        context: Context,
        peer: String? = null,
        screen: String? = null,
    ): PendingIntent {
        val intent = Intent(context, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
            peer?.let { putExtra(AppNavigation.EXTRA_OPEN_PEER, it) }
            screen?.let { putExtra(AppNavigation.EXTRA_OPEN_TAB, it) }
        }
        val requestCode = when {
            peer != null -> "open:$peer".hashCode()
            screen != null -> "screen:$screen".hashCode()
            else -> 0
        }
        return PendingIntent.getActivity(
            context,
            requestCode,
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }

    /**
     * A new encrypted DM from an accepted conversation. Tapping it opens that
     * conversation.
     *
     * Callers decide *whether* to notify — [NotificationPolicy.messageDecision]
     * holds that rule (the thread on screen, per-conversation mute, and the
     * nightly quiet window) so it is testable and identical for DMs and
     * attachments. [silent] is that decision's [NotifyDecision.Silent] arm: same
     * notification, posted on [CHANNEL_MESSAGES_QUIET] so it makes no sound.
     *
     * Rendered with [NotificationCompat.MessagingStyle], which is what makes a
     * *conversation* rather than a single line. The per-peer notification id is
     * stable — that part was always right — but with only `setContentText` to
     * work with, each message overwrote the last, so two messages from one peer
     * showed as one and the earlier text was simply gone from the shade.
     * [MessageShade] keeps the recent lines so every re-post carries the whole
     * exchange, and Android renders it as the sender's conversation.
     */
    @SuppressLint("MissingPermission") // guarded by canPost() / areNotificationsEnabled()
    fun notifyMessage(
        context: Context,
        peer: String,
        title: String,
        preview: String,
        silent: Boolean = false,
    ) {
        if (!canPost(context)) return
        val label = title.ifBlank { shortNpub(peer) }
        val now = System.currentTimeMillis()
        val lines = MessageShade.record(peer, ShadeLine(preview, now))
        val sender = Person.Builder().setName(label).setKey(peer).build()
        // The "you" of the conversation. Named from a string resource rather
        // than the user's own handle: the handle is identifying, and this label
        // is only ever drawn next to messages we sent, which there are none of
        // here.
        val self = Person.Builder().setName(context.getString(R.string.notification_you)).build()
        val style = NotificationCompat.MessagingStyle(self)
        for (line in lines) style.addMessage(line.text, line.atMs, sender)
        val channel = if (silent) CHANNEL_MESSAGES_QUIET else CHANNEL_MESSAGES
        val n = NotificationCompat.Builder(context, channel)
            .setSmallIcon(android.R.drawable.sym_action_chat)
            // Kept for platforms and launchers that ignore the style.
            .setContentTitle(label)
            .setContentText(preview)
            .setStyle(style)
            .addPerson(sender)
            .setWhen(now)
            .setShowWhen(true)
            .setAutoCancel(true)
            .setGroup(GROUP_MESSAGES)
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
            // Belt-and-braces with the channel: on pre-O the channel does not
            // exist and this flag is the only thing that keeps a quiet hour quiet.
            .setSilent(silent)
            .setContentIntent(openAppIntent(context, peer = peer))
            .build()
        // Stable per-peer id so repeated messages from one peer collapse.
        post(context, peer.hashCode(), n)
        postMessageSummary(context)
    }

    /**
     * The group summary for message notifications. Needed for them to collapse
     * into one entry rather than stack; carries no message content of its own.
     *
     * Always silent. It is re-posted on every message, and without that it
     * alerted alongside the child that actually had something to say — so one
     * arriving message made the phone sound twice. The child is what alerts;
     * this is only the thing that keeps two conversations from becoming two
     * separate shade entries.
     */
    @SuppressLint("MissingPermission") // guarded by canPost() / areNotificationsEnabled()
    private fun postMessageSummary(context: Context) {
        val n = NotificationCompat.Builder(context, CHANNEL_MESSAGES)
            .setSmallIcon(android.R.drawable.sym_action_chat)
            .setContentTitle(context.getString(R.string.notification_messages_summary))
            .setGroup(GROUP_MESSAGES)
            .setGroupSummary(true)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setAutoCancel(true)
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
            .setContentIntent(openAppIntent(context))
            .build()
        post(context, MESSAGE_SUMMARY_ID, n)
    }

    /**
     * Drop the message summary once its last child is gone. Android clears a
     * summary by itself when children are *dismissed*, but not when the app
     * cancels them (which is what opening a conversation does), and an orphaned
     * summary reads as "you have a message" pointing at nothing.
     */
    private fun pruneMessageSummary(context: Context) {
        val mgr = NotificationManagerCompat.from(context)
        val children = runCatching {
            mgr.activeNotifications.count { it.id != MESSAGE_SUMMARY_ID && it.notification.group == GROUP_MESSAGES }
        }.getOrDefault(0)
        if (NotificationPolicy.summaryStale(children)) mgr.cancel(MESSAGE_SUMMARY_ID)
    }

    /**
     * A stranger's DM landed in the message-requests bucket. Tapping it opens
     * the requests list — not the conversation, which does not exist until the
     * request is accepted.
     */
    @SuppressLint("MissingPermission") // guarded by canPost() / areNotificationsEnabled()
    fun notifyRequest(context: Context, peer: String, preview: String, silent: Boolean = false) {
        if (!canPost(context)) return
        val n = NotificationCompat.Builder(
            context,
            if (silent) CHANNEL_MESSAGES_QUIET else CHANNEL_REQUESTS,
        )
            .setSmallIcon(android.R.drawable.sym_action_chat)
            .setContentTitle("Message request")
            .setContentText(preview)
            .setAutoCancel(true)
            .setSilent(silent)
            .setContentIntent(openAppIntent(context, screen = AppNavigation.SCREEN_REQUESTS))
            .build()
        post(context, "req:$peer".hashCode(), n)
    }

    /**
     * A newer Comrade has been published. Content-light on purpose: the version
     * and the fact, with the release notes a tap away in Settings rather than
     * pasted into the shade.
     */
    @SuppressLint("MissingPermission") // guarded by canPost() / areNotificationsEnabled()
    fun notifyUpdateAvailable(context: Context, version: String) {
        // Not redundant with the services that call this at their own onCreate:
        // since `update.UpdateCheckJob`, this can be the first thing a process
        // started only to run the daily check does, and posting to a channel that
        // does not exist yet is dropped in silence from Android 8 — which would
        // lose exactly the notice the scheduled check exists to deliver.
        ensureChannels(context)
        if (!canPost(context)) return
        val n = NotificationCompat.Builder(context, CHANNEL_UPDATES)
            .setSmallIcon(android.R.drawable.stat_sys_download_done)
            .setContentTitle(context.getString(R.string.update_available_title, version))
            .setContentText(context.getString(R.string.update_available_text))
            .setAutoCancel(true)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .setContentIntent(openAppIntent(context, screen = AppNavigation.SCREEN_SETTINGS))
            .build()
        post(context, UPDATE_ID, n)
    }

    /** Drop the update notice — it was skipped, or the update has been installed. */
    fun clearUpdate(context: Context) {
        NotificationManagerCompat.from(context).cancel(UPDATE_ID)
    }

    /** One update notice at a time, replaced in place when a newer one lands. */
    private const val UPDATE_ID = 0xC0DE21

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
        post(context, "call:$peer".hashCode(), n)
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
        post(context, "missed:$peer".hashCode(), n)
    }

    /**
     * A comrade — someone the user deliberately chose — just came online.
     * The title is their name ("Ana is online"), resolved by the caller with
     * the same alias → published-handle → key precedence the chat list uses.
     *
     * Content-light like the rest: a name and the fact itself, nothing about
     * what they are doing (the beacon doesn't carry it, and the notification
     * store is not the place for it either). Tapping opens the app;
     * [clearForPeer] drops it when their chat is opened, [clearComradeOnline]
     * when they go offline again — and [ONLINE_TIMEOUT_MS] drops it on its own
     * if neither happens, so the shade can never keep claiming someone is
     * around long after their presence claim has lapsed.
     */
    @SuppressLint("MissingPermission") // guarded by canPost() / areNotificationsEnabled()
    fun notifyComradeOnline(context: Context, peer: String, title: String) {
        if (!canPost(context)) return
        val n = NotificationCompat.Builder(context, CHANNEL_COMRADES)
            .setSmallIcon(R.drawable.ic_notification_comrade)
            .setContentTitle(context.getString(R.string.comrade_online_title, title.ifBlank { shortNpub(peer) }))
            .setContentText(context.getString(R.string.comrade_online_text))
            .setAutoCancel(true)
            .setGroup(GROUP_COMRADES)
            .setCategory(NotificationCompat.CATEGORY_SOCIAL)
            .setTimeoutAfter(ONLINE_TIMEOUT_MS)
            .setContentIntent(openAppIntent(context))
            .build()
        post(context, "online:$peer".hashCode(), n)
    }

    /**
     * A comrade wrote something for the user and never sent it (see
     * `comrade_core::nudge`). Same title as [notifyComradeOnline] — they are
     * around, which is the part that makes this actionable — with a body that
     * says they might need you.
     *
     * **Deliberately the same notification id as [notifyComradeOnline]**, so
     * this replaces "Your comrade is around" in place instead of stacking a
     * second line about one person. That cannot silently downgrade back to the
     * gentler wording either: the online notice only fires on a transition
     * *into* online, and the only way a peer becomes non-online is an event
     * that routes through [clearComradeOnline] first.
     *
     * Content-light for a stronger reason than the rest: there is nothing else
     * to say. The envelope carries no draft, no length, and no difference
     * between "cleared it" and "walked away" — [ONLINE_TIMEOUT_MS] then drops
     * it on its own, because the nudge's own TTL matches presence's.
     */
    @SuppressLint("MissingPermission") // guarded by canPost() / areNotificationsEnabled()
    fun notifyComradeNudge(context: Context, peer: String, title: String) {
        if (!canPost(context)) return
        val n = NotificationCompat.Builder(context, CHANNEL_COMRADES)
            .setSmallIcon(R.drawable.ic_notification_comrade)
            .setContentTitle(context.getString(R.string.comrade_online_title, title.ifBlank { shortNpub(peer) }))
            .setContentText(context.getString(R.string.comrade_nudge_text))
            .setAutoCancel(true)
            .setGroup(GROUP_COMRADES)
            .setCategory(NotificationCompat.CATEGORY_SOCIAL)
            .setTimeoutAfter(ONLINE_TIMEOUT_MS)
            // Tapping lands in their conversation, not the app's last screen:
            // the one thing a person wants from this notice is the chat it is
            // about (WP11, the same rule message notifications follow).
            .setContentIntent(openAppIntent(context, peer = peer))
            .build()
        post(context, "online:$peer".hashCode(), n)
    }

    /**
     * Someone wants to listen or watch something with you.
     *
     * The alert the whole flow rests on: `TogetherManager.onInvited` runs off a
     * bridge event whether or not anybody is looking at the app, and before this
     * the only sign of an invitation was an overlay the person found later.
     *
     * **Names who, and deliberately not what.** The title an invitation carries
     * is a recording the other person chose, or a URL's host — putting either in
     * the shade would disclose what somebody is listening to on a lock screen
     * that other people can see. Who asked is what makes it actionable; the
     * screen says the rest once the phone is unlocked.
     *
     * Tapping lands on the Together tab, which is where the invitation is. Not
     * their conversation: the two buttons that answer this are there and nowhere
     * else, and a chat thread is one more tap away from the thing they came for.
     */
    @SuppressLint("MissingPermission") // guarded by canPost() / areNotificationsEnabled()
    fun notifyTogetherInvite(context: Context, peer: String, peerLabel: String) {
        if (!canPost(context)) return
        val who = peerLabel.ifBlank { shortNpub(peer) }
        val n = NotificationCompat.Builder(context, CHANNEL_TOGETHER)
            .setSmallIcon(R.drawable.ic_notification_comrade)
            .setContentTitle(context.getString(R.string.together_invite_notification_title, who))
            .setContentText(context.getString(R.string.together_invite_notification_text))
            .setAutoCancel(true)
            .setCategory(NotificationCompat.CATEGORY_SOCIAL)
            .setContentIntent(openAppIntent(context, screen = TOGETHER_SCREEN))
            .build()
        post(context, TOGETHER_INVITE_ID, n)
    }

    /**
     * Drop the invitation notice — it was answered, withdrawn, or the session
     * has moved on.
     *
     * One id rather than one per peer, because there is only ever one session
     * (`ComradeRuntime::together` keeps at most one), so a second invitation
     * replaces the first in the shade exactly as it replaces it in the app.
     */
    fun clearTogetherInvite(context: Context) {
        NotificationManagerCompat.from(context).cancel(TOGETHER_INVITE_ID)
    }

    private const val TOGETHER_INVITE_ID = 0xC0DE22

    /** Matches the `MainTab` entry name `AppNavigation` resolves tabs against. */
    private const val TOGETHER_SCREEN = "Together"

    /**
     * Drop the "is online" notice for `peer` — they went offline (or their
     * claim aged out) before the user got to it. A stale "Ana is online" is
     * worse than no notification: it invites a call to someone who isn't
     * there any more. Drops a nudge for the same peer with it: it shares this
     * id, and "they might need you" is no truer than "they're around" once
     * they have gone.
     */
    fun clearComradeOnline(context: Context, peer: String) {
        NotificationManagerCompat.from(context).cancel("online:$peer".hashCode())
    }

    /** Clear any notification we posted for `peer` (e.g. on opening the chat). */
    fun clearForPeer(context: Context, peer: String) {
        val mgr = NotificationManagerCompat.from(context)
        mgr.cancel(peer.hashCode())
        // …and the lines behind it, or the next message would re-post a
        // conversation the user has already read.
        MessageShade.clear(peer)
        mgr.cancel("req:$peer".hashCode())
        mgr.cancel("call:$peer".hashCode())
        mgr.cancel("online:$peer".hashCode())
        // The summary outlives its children unless we take it down ourselves.
        pruneMessageSummary(context)
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
