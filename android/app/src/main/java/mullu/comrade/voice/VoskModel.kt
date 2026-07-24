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
 * Reference counting for the shared Vosk model, with an idle linger before
 * the actual close: [acquire]/[release] track the recognisers using the
 * model; the last [release] hands back a token, and after the linger
 * [isIdleAt] confirms nothing re-acquired in between before the (expensive
 * to undo) close really happens. Pure bookkeeping, no Android/Vosk
 * dependency — behaviour pinned by `VoskModelTest`.
 */
internal class ModelRefCount {
    private var refs = 0
    private var generation = 0

    /** Take a reference — a recogniser that is about to use the model. */
    @Synchronized
    fun acquire() {
        refs++
        generation++
    }

    /**
     * Drop a reference. Returns a token when this was the last holder (pass
     * it to [isIdleAt] after the linger to decide whether the close is still
     * wanted); null while other holders remain. A stray release with no
     * holder is a harmless no-op, mirroring [MicHolderSet]'s tolerance.
     */
    @Synchronized
    fun release(): Int? {
        if (refs == 0) return null
        refs--
        generation++
        return if (refs == 0) generation else null
    }

    /** True while nothing has re-acquired (or released again) since [token] was handed out. */
    @Synchronized
    fun isIdleAt(token: Int): Boolean = refs == 0 && generation == token
}

/**
 * Process-wide, lazily-loaded, reference-counted holder for the offline Vosk
 * [Model].
 *
 * Two sources, tried in order:
 *  1. **bundled** — `assets/model-en-us` (staged by scripts/fetch-vosk-model.sh
 *     before build), unpacked by [StorageService] into the app's external
 *     files dir on first use;
 *  2. **downloaded** — `filesDir/voice-model`, installed on demand by
 *     [VoiceModelDownloader] after the user accepts the download prompt.
 *
 * The model is read-only and shared across the wake-word service, the
 * one-shot recogniser, the assist session, and the recognition service —
 * but it is *not* kept for the lifetime of the process: each user holds a
 * reference ([acquire]…[release]), and once the last one lets go the model
 * is closed after a short linger ([CLOSE_LINGER_MS], so back-to-back
 * dictations don't pay the multi-second reload). Turning the wake word off
 * therefore actually reclaims the model's RAM; while the wake word is
 * enabled the model stays resident by design — continuous recognition needs
 * it.
 */
object VoskModel {

    const val ASSET_PATH = "model-en-us"
    const val TARGET_DIR = "model"

    /** `filesDir` subdirectory [VoiceModelDownloader] installs into. */
    const val DOWNLOAD_DIR = "voice-model"

    /**
     * How long a fully released model stays loaded before closing. Long
     * enough to absorb a re-dictation or a wake-word toggle bounce, short
     * enough that "voice off" visibly returns the memory.
     */
    private const val CLOSE_LINGER_MS = 30_000L

    private val refs = ModelRefCount()

    /** Guards [cached] handoffs against the delayed close. */
    private val lock = Any()
    private var cached: Model? = null

    private val loader = Executors.newSingleThreadExecutor { r -> Thread(r, "vosk-model-load") }
    private val mainThread by lazy { Handler(Looper.getMainLooper()) }

    /** Where the on-demand download lands (a directory containing `am/`, `conf/`, …). */
    fun downloadedDir(context: Context): File = File(context.filesDir, DOWNLOAD_DIR)

    /**
     * Whether [acquire] can produce a model without user interaction —
     * already loaded, bundled in the APK, or previously downloaded. Voice
     * entry points check this up front and offer the download when false.
     */
    fun isAvailable(context: Context): Boolean =
        synchronized(lock) { cached != null } ||
            isBundled(context) ||
            VoiceModelInstaller.looksLikeModel(downloadedDir(context))

    // The uuid marker is the exact asset StorageService keys its unpacking on
    // — its absence is the "model-en-us/uuid" FileNotFoundException voice
    // errors reported before the on-demand download existed.
    private fun isBundled(context: Context): Boolean =
        runCatching { context.assets.open("$ASSET_PATH/uuid").close() }.isSuccess

    /**
     * Take a reference on the shared [Model] and deliver it to [onReady],
     * loading it first when needed. The caller owns that reference until its
     * matching [release] — release when the recogniser stops, not before.
     * When [onError] fires instead ([VoiceModelMissingException] if there is
     * no model anywhere, or the underlying failure) no reference is held and
     * no release is due. Callbacks arrive on the main thread (the
     * missing-model error synchronously, from the calling one).
     */
    fun acquire(context: Context, onReady: (Model) -> Unit, onError: (Throwable) -> Unit) {
        val app = context.applicationContext
        val ready = synchronized(lock) {
            // Taking the ref first also invalidates any pending idle close
            // (the generation moved), so the cached model can't be closed
            // out from under us between here and onReady.
            refs.acquire()
            cached
        }
        if (ready != null) {
            onReady(ready)
            return
        }
        val downloaded = downloadedDir(app)
        when {
            isBundled(app) -> StorageService.unpack(
                app,
                ASSET_PATH,
                TARGET_DIR,
                { model -> onReady(adopt(model)) },
                { exception ->
                    release()
                    onError(exception)
                },
            )
            VoiceModelInstaller.looksLikeModel(downloaded) -> loadDownloaded(downloaded, onReady, onError)
            else -> {
                release()
                onError(VoiceModelMissingException())
            }
        }
    }

    /**
     * Give back a reference taken by [acquire]. When the last one is
     * returned the model closes — freeing its RAM — after [CLOSE_LINGER_MS]
     * of staying idle.
     */
    fun release() {
        val token = refs.release() ?: return
        mainThread.postDelayed({ closeIfStillIdle(token) }, CLOSE_LINGER_MS)
    }

    /** Cache a freshly loaded model — or, if a parallel load won the race, close the duplicate and share the winner. */
    private fun adopt(model: Model): Model = synchronized(lock) {
        val winner = cached
        if (winner == null) {
            cached = model
            model
        } else {
            model.close()
            winner
        }
    }

    private fun closeIfStillIdle(token: Int) {
        val toClose: Model?
        synchronized(lock) {
            if (!refs.isIdleAt(token)) return
            toClose = cached
            cached = null
        }
        toClose?.close()
    }

    /** [Model] construction takes seconds — do it off the main thread, posting back like [StorageService] does. */
    private fun loadDownloaded(dir: File, onReady: (Model) -> Unit, onError: (Throwable) -> Unit) {
        loader.execute {
            // The single loader thread serialises concurrent callers: a
            // second request either sees the cache here or waits its turn.
            val existing = synchronized(lock) { cached }
            if (existing != null) {
                mainThread.post { onReady(existing) }
                return@execute
            }
            runCatching { Model(dir.absolutePath) }
                .onSuccess { model ->
                    val shared = adopt(model)
                    mainThread.post { onReady(shared) }
                }
                .onFailure { failure ->
                    release()
                    mainThread.post { onError(failure) }
                }
        }
    }
}
