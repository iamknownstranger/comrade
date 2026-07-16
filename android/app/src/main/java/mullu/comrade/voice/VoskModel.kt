package mullu.comrade.voice

import android.content.Context
import android.os.Handler
import android.os.Looper
import java.io.File
import java.io.IOException
import java.util.concurrent.Executors
import org.vosk.Model
import org.vosk.android.StorageService

/**
 * No usable speech model on this device: none was baked into the APK
 * (`assets/model-en-us`, see the assets README) and none has been downloaded
 * yet. Voice entry points catch this (or pre-check [VoskModel.isAvailable])
 * and offer the on-demand download ([VoiceModelDownloader]) instead of
 * surfacing a dead-end error.
 */
class VoiceModelMissingException : IOException("speech model not installed")

/**
 * Process-wide, lazily-loaded holder for the offline Vosk [Model].
 *
 * Two sources, tried in order:
 *  1. **bundled** — `assets/model-en-us` (staged by scripts/fetch-vosk-model.sh
 *     before build), unpacked by [StorageService] into the app's external
 *     files dir on first use;
 *  2. **downloaded** — `filesDir/voice-model`, installed on demand by
 *     [VoiceModelDownloader] after the user accepts the download prompt.
 *
 * Whichever loads is cached for the lifetime of the process — it is read-only
 * and shared across the wake-word service, the one-shot recogniser, the
 * assist session, and the recognition service. Nobody may close it.
 */
object VoskModel {

    const val ASSET_PATH = "model-en-us"
    const val TARGET_DIR = "model"

    /** `filesDir` subdirectory [VoiceModelDownloader] installs into. */
    const val DOWNLOAD_DIR = "voice-model"

    @Volatile private var cached: Model? = null

    private val loader = Executors.newSingleThreadExecutor { r -> Thread(r, "vosk-model-load") }

    /** Where the on-demand download lands (a directory containing `am/`, `conf/`, …). */
    fun downloadedDir(context: Context): File = File(context.filesDir, DOWNLOAD_DIR)

    /**
     * Whether [withModel] can produce a model without user interaction —
     * already loaded, bundled in the APK, or previously downloaded. Voice
     * entry points check this up front and offer the download when false.
     */
    fun isAvailable(context: Context): Boolean =
        cached != null ||
            isBundled(context) ||
            VoiceModelInstaller.looksLikeModel(downloadedDir(context))

    // The uuid marker is the exact asset StorageService keys its unpacking on
    // — its absence is the "model-en-us/uuid" FileNotFoundException voice
    // errors reported before the on-demand download existed.
    private fun isBundled(context: Context): Boolean =
        runCatching { context.assets.open("$ASSET_PATH/uuid").close() }.isSuccess

    /**
     * Deliver the shared [Model] to [onReady], loading it the first time.
     * [onError] fires with [VoiceModelMissingException] when there is no
     * model anywhere to load, or with the underlying failure when loading
     * breaks. Callbacks run on the main thread ([VoiceModelMissingException]
     * synchronously, from the calling one).
     */
    fun withModel(context: Context, onReady: (Model) -> Unit, onError: (Throwable) -> Unit) {
        cached?.let { onReady(it); return }
        val app = context.applicationContext
        val downloaded = downloadedDir(app)
        when {
            isBundled(app) -> StorageService.unpack(
                app,
                ASSET_PATH,
                TARGET_DIR,
                { model ->
                    cached = model
                    onReady(model)
                },
                { exception -> onError(exception) },
            )
            VoiceModelInstaller.looksLikeModel(downloaded) -> loadDownloaded(downloaded, onReady, onError)
            else -> onError(VoiceModelMissingException())
        }
    }

    /** [Model] construction takes seconds — do it off the main thread, posting back like [StorageService] does. */
    private fun loadDownloaded(dir: File, onReady: (Model) -> Unit, onError: (Throwable) -> Unit) {
        val mainThread = Handler(Looper.getMainLooper())
        loader.execute {
            // The single loader thread serialises concurrent callers: a
            // second request either sees the cache here or waits its turn.
            cached?.let { model ->
                mainThread.post { onReady(model) }
                return@execute
            }
            runCatching { Model(dir.absolutePath) }
                .onSuccess { model ->
                    cached = model
                    mainThread.post { onReady(model) }
                }
                .onFailure { failure -> mainThread.post { onError(failure) } }
        }
    }
}
