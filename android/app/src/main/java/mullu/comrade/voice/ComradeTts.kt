package mullu.comrade.voice

import android.content.Context
import android.speech.tts.TextToSpeech
import android.util.Log
import java.util.Locale
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Thin lifecycle-safe wrapper around Android [TextToSpeech].
 *
 * Utterances requested before the engine finishes initialising are queued and
 * flushed on [TextToSpeech.OnInitListener]. Always call [shutdown] from the
 * owner's teardown (service `onDestroy`, activity `onDestroy`).
 */
class ComradeTts(context: Context) {

    private val ready = AtomicBoolean(false)
    private val pending = ArrayDeque<String>()
    private var engine: TextToSpeech? = null

    init {
        engine = TextToSpeech(context.applicationContext) { status ->
            if (status == TextToSpeech.SUCCESS) {
                engine?.language = Locale.getDefault().takeIf {
                    engine?.isLanguageAvailable(it) == TextToSpeech.LANG_AVAILABLE
                } ?: Locale.US
                ready.set(true)
                synchronized(pending) {
                    while (pending.isNotEmpty()) speakNow(pending.removeFirst())
                }
            } else {
                Log.w(TAG, "TextToSpeech init failed with status=$status")
            }
        }
    }

    /** Speak [text], flushing any in-progress utterance. */
    fun speak(text: String) {
        if (text.isBlank()) return
        if (ready.get()) {
            speakNow(text)
        } else {
            synchronized(pending) { pending.addLast(text) }
        }
    }

    private fun speakNow(text: String) {
        engine?.speak(text, TextToSpeech.QUEUE_FLUSH, null, "comrade-${text.hashCode()}")
    }

    fun shutdown() {
        ready.set(false)
        engine?.stop()
        engine?.shutdown()
        engine = null
    }

    private companion object {
        const val TAG = "ComradeTts"
    }
}
