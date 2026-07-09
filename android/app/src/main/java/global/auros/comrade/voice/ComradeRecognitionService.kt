package global.auros.comrade.voice

import android.content.Intent
import android.os.Bundle
import android.speech.RecognitionService
import android.speech.SpeechRecognizer
import android.util.Log

/**
 * A minimal offline [RecognitionService] backed by Vosk via [OneShotRecognizer].
 *
 * The assist-app registration (`res/xml/interaction_service.xml`) requires a
 * recognition service component; this provides one that works fully offline
 * rather than depending on Google's cloud recogniser. It captures a single
 * utterance and returns it under [SpeechRecognizer.RESULTS_RECOGNITION].
 */
class ComradeRecognitionService : RecognitionService() {

    private var recognizer: OneShotRecognizer? = null

    override fun onStartListening(recognizerIntent: Intent?, listener: Callback?) {
        if (listener == null) return
        val oneShot = OneShotRecognizer(this)
        recognizer = oneShot
        runCatching { listener.beginningOfSpeech() }
        oneShot.listen(
            onText = { text ->
                runCatching {
                    listener.endOfSpeech()
                    if (text.isBlank()) {
                        listener.error(SpeechRecognizer.ERROR_NO_MATCH)
                    } else {
                        val results = Bundle().apply {
                            putStringArrayList(
                                SpeechRecognizer.RESULTS_RECOGNITION,
                                arrayListOf(text),
                            )
                        }
                        listener.results(results)
                    }
                }.onFailure { Log.w(TAG, "callback dispatch failed", it) }
            },
            onError = {
                runCatching { listener.error(SpeechRecognizer.ERROR_AUDIO) }
            },
        )
    }

    override fun onStopListening(listener: Callback?) { /* one-shot stops itself */ }

    override fun onCancel(listener: Callback?) {
        recognizer = null
    }

    private companion object {
        const val TAG = "ComradeRecognition"
    }
}
