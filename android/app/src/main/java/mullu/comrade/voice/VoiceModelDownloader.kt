package mullu.comrade.voice

import android.content.Context
import android.os.SystemClock
import android.util.Log
import java.io.File
import java.net.URI
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

/**
 * Process-wide manager for the on-demand voice-model download — the
 * "download the on-device speech model?" flow that
 * [mullu.comrade.ui.VoiceModelDownloadDialog] renders.
 *
 * APKs built without running scripts/fetch-vosk-model.sh (CI's, notably) ship
 * no bundled model; the first time the user reaches for a voice feature on
 * such a build the UI offers this download instead of dead-ending on
 * "voice unavailable". The state is one process-wide [StateFlow] so every
 * entry point (Settings' tap-to-talk and "Hey Comrade" toggle, the journal
 * dictation mic) sees the same download, and dismissing the dialog leaves an
 * in-flight download running in the background.
 */
object VoiceModelDownloader {

    /** The official Vosk small-English model (Apache-2.0), ~40 MB zipped. */
    const val MODEL_URL = "https://alphacephei.com/vosk/models/vosk-model-small-en-us-0.15.zip"

    /**
     * sha256 of the zip at [MODEL_URL] — the same pin as
     * scripts/fetch-vosk-model.sh (see there for how it was verified). A
     * download that hashes to anything else is deleted, never installed.
     */
    const val MODEL_SHA256 = "30f26242c4eb449f948e42cb302dd7a686cb29a3423a8367f99ff41780942498"

    /** Size of the zip at [MODEL_URL]: progress denominator until the server reports one, and the prompt's size figure. */
    const val MODEL_ZIP_BYTES = 41_205_931L

    sealed class State {
        /** Nothing in flight — the dialog shows the download offer. */
        object Idle : State()

        data class Downloading(val bytesRead: Long, val totalBytes: Long) : State()

        /** Download finished; checksum + unzip + move into place (quick, but not instant). */
        object Installing : State()

        data class Failed(val message: String) : State()

        /** Model installed where [VoskModel.withModel] looks for downloaded models. */
        object Ready : State()
    }

    private val _state = MutableStateFlow<State>(State.Idle)
    val state: StateFlow<State> = _state

    @Volatile private var cancelRequested = false
    private var worker: Thread? = null

    /**
     * Kick off the download unless one is already running (idempotent —
     * a second tap from another screen just re-observes [state]).
     */
    @Synchronized
    fun start(context: Context) {
        if (worker?.isAlive == true) return
        val app = context.applicationContext
        val installDir = VoskModel.downloadedDir(app)
        if (VoiceModelInstaller.looksLikeModel(installDir)) {
            _state.value = State.Ready
            return
        }
        cancelRequested = false
        _state.value = State.Downloading(0L, MODEL_ZIP_BYTES)
        worker = Thread({ run(app, installDir) }, "voice-model-download").apply { start() }
    }

    /** Abort an in-flight download: partial files are deleted and [state] returns to [State.Idle]. */
    fun cancel() {
        cancelRequested = true
    }

    /**
     * Re-arm the offer when a stale in-memory [State.Ready] no longer holds
     * (the downloaded files vanished, e.g. cleared storage) — otherwise the
     * dialog would keep firing its ready-callback into a load that keeps
     * failing. Called by the dialog before trusting Ready.
     */
    @Synchronized
    fun reofferIfGone(context: Context) {
        if (worker?.isAlive == true) return
        if (_state.value is State.Ready &&
            !VoiceModelInstaller.looksLikeModel(VoskModel.downloadedDir(context.applicationContext))
        ) {
            _state.value = State.Idle
        }
    }

    private fun run(app: Context, installDir: File) {
        val startedAt = SystemClock.elapsedRealtime()
        try {
            VoiceModelInstaller.fetchAndInstall(
                url = URI(MODEL_URL).toURL(),
                expectedSha256 = MODEL_SHA256,
                zipCache = File(app.cacheDir, "vosk-model.zip.part"),
                stagingDir = File(app.filesDir, "${VoskModel.DOWNLOAD_DIR}.staging"),
                installDir = installDir,
                onProgress = { read, total ->
                    _state.value = State.Downloading(read, if (total > 0) total else MODEL_ZIP_BYTES)
                },
                onInstalling = { _state.value = State.Installing },
                isCancelled = { cancelRequested },
            )
            Log.i(TAG, "voice model installed in ${SystemClock.elapsedRealtime() - startedAt} ms")
            _state.value = State.Ready
        } catch (cancelled: InstallCancelledException) {
            Log.i(TAG, "voice model download cancelled after ${SystemClock.elapsedRealtime() - startedAt} ms")
            _state.value = State.Idle
        } catch (failure: Exception) {
            Log.w(TAG, "voice model download failed", failure)
            _state.value = State.Failed(failure.message ?: failure.javaClass.simpleName)
        }
    }

    private const val TAG = "VoiceModelDownloader"
}
