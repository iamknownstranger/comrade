package mullu.comrade.voice

import android.content.Context
import org.vosk.Model
import org.vosk.android.StorageService

/**
 * Process-wide, lazily-unpacked holder for the offline Vosk [Model].
 *
 * The model (shipped under `assets/model-en-us`, see README) is unpacked into
 * `filesDir/model` on first use and then cached for the lifetime of the
 * process — it is read-only and safe to share across the wake-word service, the
 * one-shot recogniser, the assist session, and the recognition service.
 */
object VoskModel {

    const val ASSET_PATH = "model-en-us"
    const val TARGET_DIR = "model"

    @Volatile private var cached: Model? = null

    /**
     * Deliver the shared [Model] to [onReady], unpacking + loading it the first
     * time. [onError] fires if the model assets are missing or fail to load.
     * Callbacks are posted on the main thread by [StorageService].
     */
    fun withModel(context: Context, onReady: (Model) -> Unit, onError: (Throwable) -> Unit) {
        cached?.let { onReady(it); return }
        StorageService.unpack(
            context.applicationContext,
            ASSET_PATH,
            TARGET_DIR,
            { model ->
                cached = model
                onReady(model)
            },
            { exception -> onError(exception) },
        )
    }
}
