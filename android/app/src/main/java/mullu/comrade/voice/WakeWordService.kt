package mullu.comrade.voice

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log
import mullu.comrade.MainActivity
import mullu.comrade.R
import org.json.JSONObject
import org.vosk.Model
import org.vosk.Recognizer
import org.vosk.android.RecognitionListener
import org.vosk.android.SpeechService
import org.vosk.android.StorageService
import java.util.concurrent.Executors

/** An independent feature that can need the mic exclusively (see [MicHolderSet]). */
internal enum class MicHolder { CALL, VOICE_NOTE }

/**
 * Tracks which [MicHolder]s currently need the wake-word recogniser paused,
 * so it only actually restarts once every holder has released it — a call
 * and a voice-note recording can overlap (see [mullu.comrade.call.CallManager]
 * and [mullu.comrade.media.VoiceRecorder]), and whichever one finishes first
 * must not hand the mic back while the other still holds it.
 *
 * Pure bookkeeping, no Android/Vosk dependency, safe to call unevenly: a
 * duplicate [acquire], or a [release] with no matching [acquire] (e.g. a call
 * that ends before its own setup ever paused), is always a harmless no-op —
 * see `WakeWordServiceTest`.
 */
internal class MicHolderSet {
    private val holders = mutableSetOf<MicHolder>()

    /** Returns `true` iff [holder] just became the *first* active holder — the caller should actually pause. */
    @Synchronized
    fun acquire(holder: MicHolder): Boolean = holders.add(holder) && holders.size == 1

    /** Returns `true` iff [holder] just released the *last* active holder — the caller should actually resume. */
    @Synchronized
    fun release(holder: MicHolder): Boolean = holders.remove(holder) && holders.isEmpty()
}

/**
 * Always-listening foreground service implementing the "Hey Comrade" wake word
 * with the offline Vosk recogniser.
 *
 * Pipeline: [SpeechService] streams 16 kHz PCM from the mic into a free-form
 * [Recognizer]. Each finalised hypothesis is inspected — while [State.IDLE] we
 * look for the wake phrase; once heard we flip to [State.LISTENING] and treat
 * the next utterance as a command, routed through [VoiceCommand] →
 * [CommandDispatcher] and spoken back via [ComradeTts].
 *
 * This is an *app-scoped* wake word, not the OS-level "Hey Google" DSP hotword —
 * it only runs while this foreground service is alive, shows a persistent
 * notification, and keeps the mic open (battery cost). A free-form recogniser is
 * used (rather than grammar-restricted keyword spotting) because command bodies
 * — e.g. the text of a post — are open vocabulary.
 */
class WakeWordService : Service(), RecognitionListener {

    private enum class State { IDLE, LISTENING }

    private val mainHandler = Handler(Looper.getMainLooper())
    private val worker = Executors.newSingleThreadExecutor()

    private var model: Model? = null
    private var speechService: SpeechService? = null
    private var tts: ComradeTts? = null
    private lateinit var dispatcher: CommandDispatcher

    @Volatile private var state = State.IDLE
    private val revertToIdle = Runnable {
        if (state == State.LISTENING) {
            state = State.IDLE
            tts?.speak("Never mind.")
        }
    }

    override fun onCreate() {
        super.onCreate()
        isRunning = true
        instance = this
        dispatcher = CommandDispatcher(ComradeCoreBackend())
        tts = ComradeTts(this)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                stopSelf()
                return START_NOT_STICKY
            }
        }
        startForegroundNotified(getString(R.string.voice_listening))
        if (speechService == null) initRecogniser()
        return START_STICKY
    }

    // ── Model + recogniser bootstrap ─────────────────────────────────────────

    private fun initRecogniser() {
        // Unpack the Vosk model shipped under assets/model-en-us into filesDir.
        StorageService.unpack(
            this,
            MODEL_ASSET,
            MODEL_TARGET,
            { unpacked ->
                model = unpacked
                startRecognition(unpacked)
            },
            { exception ->
                Log.e(TAG, "Vosk model unavailable", exception)
                updateNotification(getString(R.string.voice_model_missing))
            },
        )
    }

    private fun startRecognition(model: Model) {
        runCatching {
            val recognizer = Recognizer(model, SAMPLE_RATE)
            val service = SpeechService(recognizer, SAMPLE_RATE)
            service.startListening(this)
            speechService = service
        }.onFailure {
            Log.e(TAG, "Failed to start SpeechService", it)
            updateNotification(getString(R.string.voice_mic_error))
        }
    }

    // ── RecognitionListener ──────────────────────────────────────────────────

    override fun onPartialResult(hypothesis: String?) { /* no-op: act on finals */ }

    override fun onResult(hypothesis: String?) = onFinalised(hypothesis)

    override fun onFinalResult(hypothesis: String?) = onFinalised(hypothesis)

    private fun onFinalised(hypothesis: String?) {
        val text = hypothesis?.let { runCatching { JSONObject(it).optString("text") }.getOrNull() }
            ?.trim().orEmpty()
        if (text.isEmpty()) return

        when (state) {
            State.IDLE -> {
                val idx = text.indexOf(VoiceCommand.WAKE_PHRASE)
                if (idx < 0) return
                val remainder = text.substring(idx + VoiceCommand.WAKE_PHRASE.length).trim()
                if (remainder.isEmpty()) {
                    state = State.LISTENING
                    updateNotification(getString(R.string.voice_go_ahead))
                    tts?.speak("Yes?")
                    mainHandler.removeCallbacks(revertToIdle)
                    mainHandler.postDelayed(revertToIdle, COMMAND_WINDOW_MS)
                } else {
                    dispatch(remainder)
                }
            }
            State.LISTENING -> {
                mainHandler.removeCallbacks(revertToIdle)
                state = State.IDLE
                updateNotification(getString(R.string.voice_listening))
                dispatch(text)
            }
        }
    }

    override fun onError(exception: Exception?) {
        Log.e(TAG, "Recogniser error", exception)
        updateNotification(getString(R.string.voice_mic_error))
    }

    override fun onTimeout() { state = State.IDLE }

    /** Parse + execute a command off the audio thread, then speak the reply. */
    private fun dispatch(commandText: String) {
        worker.execute {
            val reply = runCatching {
                dispatcher.handle(VoiceCommand.parse(commandText))
            }.getOrElse { "Something went wrong. ${it.message ?: ""}".trim() }
            tts?.speak(reply)
        }
    }

    // ── Foreground notification ──────────────────────────────────────────────

    private fun startForegroundNotified(status: String) {
        ensureChannel()
        val notification = buildNotification(status)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(
                NOTIFICATION_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE,
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    private fun updateNotification(status: String) {
        ensureChannel()
        (getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager)
            .notify(NOTIFICATION_ID, buildNotification(status))
    }

    private fun buildNotification(status: String): Notification {
        val openApp = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val stopIntent = PendingIntent.getService(
            this,
            1,
            Intent(this, WakeWordService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return Notification.Builder(this, CHANNEL_ID)
            .setContentTitle(getString(R.string.voice_notification_title))
            .setContentText(status)
            .setSmallIcon(R.drawable.ic_launcher_foreground)
            .setContentIntent(openApp)
            .setOngoing(true)
            .addAction(
                Notification.Action.Builder(null, getString(R.string.voice_stop), stopIntent)
                    .build(),
            )
            .build()
    }

    private fun ensureChannel() {
        val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (manager.getNotificationChannel(CHANNEL_ID) == null) {
            manager.createNotificationChannel(
                NotificationChannel(
                    CHANNEL_ID,
                    getString(R.string.voice_channel_name),
                    NotificationManager.IMPORTANCE_LOW,
                ).apply { setShowBadge(false) },
            )
        }
    }

    override fun onDestroy() {
        isRunning = false
        instance = null
        mainHandler.removeCallbacks(revertToIdle)
        speechService?.stop()
        speechService?.shutdown()
        speechService = null
        model?.close()
        model = null
        tts?.shutdown()
        tts = null
        worker.shutdownNow()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private val pauseHolders = MicHolderSet()

    /** Stop the Vosk recogniser (releasing the mic) without tearing down the foreground service/notification. */
    private fun pauseListening(holder: MicHolder) {
        if (!pauseHolders.acquire(holder)) return
        runCatching { speechService?.stop() }.onFailure { Log.w(TAG, "pauseListening failed", it) }
    }

    /** Restart the Vosk recogniser once every [MicHolder] from [pauseListening] has released it. */
    private fun resumeListening(holder: MicHolder) {
        if (!pauseHolders.release(holder)) return
        runCatching { speechService?.startListening(this) }.onFailure { Log.w(TAG, "resumeListening failed", it) }
    }

    companion object {
        private const val TAG = "WakeWordService"
        const val ACTION_START = "mullu.comrade.voice.START"
        const val ACTION_STOP = "mullu.comrade.voice.STOP"

        /**
         * Whether the service is currently alive. The Voice screen is disposed
         * on every tab switch, so its toggle re-seeds from this instead of
         * assuming the service died with the composable.
         */
        @Volatile var isRunning: Boolean = false
            private set

        /** The live instance, if the service is running — backs [pause]/[resume]. */
        @Volatile private var instance: WakeWordService? = null

        /**
         * Release the mic (Vosk [SpeechService] stopped) without stopping the
         * foreground service/notification — for another mic consumer (a call,
         * a voice-note recording) to use it exclusively for a while. A no-op
         * if the service isn't running. The recogniser stays paused until
         * every [holder] that has called [pause] has also called [resume] —
         * see [MicHolderSet].
         */
        fun pause(holder: MicHolder) {
            instance?.pauseListening(holder)
        }

        /** Restart the wake-word recogniser once [holder] is the last to call this after [pause]. A no-op if the service isn't running. */
        fun resume(holder: MicHolder) {
            instance?.resumeListening(holder)
        }

        private const val CHANNEL_ID = "comrade_voice"
        private const val NOTIFICATION_ID = 0x0C0DE
        private const val SAMPLE_RATE = 16_000.0f
        private const val COMMAND_WINDOW_MS = 6_000L

        /** Assets subfolder holding the unpacked Vosk model (see README). */
        private const val MODEL_ASSET = "model-en-us"
        private const val MODEL_TARGET = "model"

        fun start(context: Context) {
            val intent = Intent(context, WakeWordService::class.java)
                .setAction(ACTION_START)
            context.startForegroundService(intent)
        }

        fun stop(context: Context) {
            context.startService(
                Intent(context, WakeWordService::class.java).setAction(ACTION_STOP),
            )
        }
    }
}
