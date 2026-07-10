package global.auros.comrade.voice

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
                // isLanguageAvailable returns a tiered >= 0 result
                // (LANG_AVAILABLE=0, COUNTRY=1, COUNTRY_VAR=2); an == check
                // wrongly rejected exact country matches like en_US.
                engine?.language = Locale.getDefault().takeIf {
                    (engine?.isLanguageAvailable(it) ?: TextToSpeech.LANG_NOT_SUPPORTED) >=
                        TextToSpeech.LANG_AVAILABLE
                } ?: Locale.US
                // Flip ready and drain under the same lock speak() enqueues
                // with, so no utterance can slip between check and enqueue.
                synchronized(pending) {
                    ready.set(true)
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
        synchronized(pending) {
            if (!ready.get()) {
                pending.addLast(text)
                return
            }
        }
        speakNow(text)
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
