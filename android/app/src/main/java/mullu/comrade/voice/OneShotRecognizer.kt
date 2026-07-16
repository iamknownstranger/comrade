package mullu.comrade.voice

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.util.Log
import org.json.JSONObject
import org.vosk.Recognizer
import org.vosk.android.RecognitionListener
import org.vosk.android.SpeechService

/**
 * Captures a *single* spoken utterance offline with Vosk and hands back the
 * recognised text. Used by the tap-to-talk mic button, the assist session, and
 * [ComradeRecognitionService] — none of which want the always-on wake loop of
 * [WakeWordService].
 *
 * The recogniser stops itself after the first final result or after
 * [timeoutMs], releasing its [VoskModel] reference as it finishes (the model's
 * RAM is reclaimed shortly after the last user lets go). One instance handles
 * one utterance — create a fresh one per capture.
 * [RECORD_AUDIO][android.Manifest.permission.RECORD_AUDIO] must already be
 * granted by the caller.
 */
class OneShotRecognizer(private val context: Context) {

    private val mainHandler = Handler(Looper.getMainLooper())
    private var speechService: SpeechService? = null
    private var finished = false
    private var holdsModel = false

    /**
     * @param onText recognised text (possibly empty if nothing was heard)
     * @param onError model/mic failure
     */
    fun listen(
        timeoutMs: Long = 7_000L,
        onText: (String) -> Unit,
        onError: (Throwable) -> Unit,
    ) {
        VoskModel.acquire(
            context,
            onReady = { model ->
                holdsModel = true
                runCatching {
                    val recognizer = Recognizer(model, SAMPLE_RATE)
                    val service = SpeechService(recognizer, SAMPLE_RATE)
                    speechService = service
                    val timeout = Runnable { complete("", onText) }
                    val listener = object : RecognitionListener {
                        override fun onPartialResult(hypothesis: String?) {}
                        override fun onResult(hypothesis: String?) {
                            mainHandler.removeCallbacksAndMessages(null)
                            complete(extract(hypothesis), onText)
                        }
                        override fun onFinalResult(hypothesis: String?) {
                            mainHandler.removeCallbacksAndMessages(null)
                            complete(extract(hypothesis), onText)
                        }
                        override fun onError(exception: Exception?) {
                            mainHandler.removeCallbacksAndMessages(null)
                            teardown()
                            releaseModel()
                            onError(exception ?: RuntimeException("recogniser error"))
                        }
                        override fun onTimeout() = complete("", onText)
                    }
                    service.startListening(listener)
                    mainHandler.postDelayed(timeout, timeoutMs)
                }.onFailure {
                    teardown()
                    releaseModel()
                    onError(it)
                }
            },
            onError = onError,
        )
    }

    private fun extract(hypothesis: String?): String =
        hypothesis?.let { runCatching { JSONObject(it).optString("text") }.getOrNull() }
            ?.trim().orEmpty()

    private fun complete(text: String, onText: (String) -> Unit) {
        if (finished) return
        finished = true
        teardown()
        releaseModel()
        onText(text)
    }

    /** Give back the shared-model reference exactly once (every finish path funnels here or through the error branches). */
    private fun releaseModel() {
        if (!holdsModel) return
        holdsModel = false
        VoskModel.release()
    }

    private fun teardown() {
        runCatching {
            speechService?.stop()
            speechService?.shutdown()
        }.onFailure { Log.w(TAG, "teardown", it) }
        speechService = null
    }

    private companion object {
        const val TAG = "OneShotRecognizer"
        const val SAMPLE_RATE = 16_000.0f
    }
}
