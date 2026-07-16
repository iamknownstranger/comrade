package mullu.comrade.voice

import android.content.Context
import android.os.Bundle
import android.util.Log
import java.util.concurrent.Executors

/**
 * The assist session: when the assist gesture opens Comrade, greet the user,
 * capture one spoken command with [OneShotRecognizer], route it through
 * [VoiceCommand] → [CommandDispatcher], speak the reply, then dismiss.
 */
class ComradeInteractionSession(context: Context) :
    android.service.voice.VoiceInteractionSession(context) {

    private val tts = ComradeTts(context)
    private val recognizer = OneShotRecognizer(context)
    private val dispatcher = CommandDispatcher(ComradeCoreBackend())
    private val worker = Executors.newSingleThreadExecutor()

    override fun onShow(args: Bundle?, showFlags: Int) {
        super.onShow(args, showFlags)
        tts.speak("Yes?")
        recognizer.listen(
            onText = { text ->
                if (text.isBlank()) {
                    tts.speak("I didn't catch that.")
                    hide()
                    return@listen
                }
                worker.execute {
                    val reply = runCatching {
                        dispatcher.handle(VoiceCommand.parse(text))
                    }.getOrElse { "Something went wrong." }
                    tts.speak(reply)
                }
            },
            onError = { error ->
                Log.e(TAG, "assist recognition failed", error)
                // No UI of its own to host the download dialog — point the
                // user at the app, where any voice button offers the download.
                tts.speak(
                    if (error is VoiceModelMissingException) {
                        "The voice model isn't downloaded yet. Open Comrade and tap a voice button to set it up."
                    } else {
                        "Voice isn't available right now."
                    },
                )
                hide()
            },
        )
    }

    override fun onDestroy() {
        worker.shutdownNow()
        tts.shutdown()
        super.onDestroy()
    }

    private companion object {
        const val TAG = "ComradeAssistSession"
    }
}
