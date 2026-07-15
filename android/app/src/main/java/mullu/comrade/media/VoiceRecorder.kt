package mullu.comrade.media

import android.content.Context
import android.media.MediaRecorder
import android.os.Build
import android.util.Log
import java.io.File
import mullu.comrade.voice.MicHolder
import mullu.comrade.voice.WakeWordService

/**
 * A minimal push-to-talk audio recorder for chat voice notes, built on the
 * platform [MediaRecorder].
 *
 * The output is AAC in an ADTS stream (`OutputFormat.AAC_ADTS` + `Encoder.AAC`):
 * a single, seekable elementary stream that [android.media.MediaPlayer] plays
 * back directly (see `MediaAttachment.InlineAudio`) and whose `audio/aac` MIME
 * type the media pipeline already knows how to name on disk. Bit rate and
 * sample rate are tuned for speech, not music — small clips that upload fast
 * over a relay yet stay perfectly intelligible.
 *
 * The clip is written to a throwaway file in the app's [Context.getCacheDir]
 * so it never touches shared/backed-up storage; the caller reads the bytes and
 * is expected to delete the file the moment the encrypted send resolves (the
 * plaintext voice note must not outlive the send — mirrors the decrypt-cache
 * cleanup for the receive side, AUDIT S-4).
 *
 * Not thread-safe: drive it from a single UI gesture (press to [start], release
 * to [stop]). One instance records one clip at a time.
 */
class VoiceRecorder(private val context: Context) {

    private var recorder: MediaRecorder? = null
    private var outputFile: File? = null
    private var startedAtMs: Long = 0L

    /** Whether a recording is currently in progress. */
    val isRecording: Boolean get() = recorder != null

    /**
     * Begin capturing from the microphone into a fresh cache file.
     *
     * Returns `true` once capture has started, or `false` if the recorder could
     * not be set up (device without a mic, `RECORD_AUDIO` not granted, encoder
     * busy). A `false` return leaves nothing to clean up — no file is created
     * and no partial recorder is left running.
     */
    fun start(): Boolean {
        if (recorder != null) return false // already recording

        // Release the wake-word recogniser's hold on the mic first — two
        // consumers fighting over MediaRecorder.AudioSource.MIC is exactly
        // the kind of contention that used to make this fail with a busy
        // device. Restored in stop()/cancel(), and on a failed start below.
        WakeWordService.pause(MicHolder.VOICE_NOTE)

        val dir = File(context.cacheDir, "voice-notes").apply { mkdirs() }
        // A non-colliding name; the file is short-lived and deleted after send.
        val file = File(dir, "vn-${System.nanoTime()}.aac")

        val rec = newRecorder()
        return try {
            rec.setAudioSource(MediaRecorder.AudioSource.MIC)
            rec.setOutputFormat(MediaRecorder.OutputFormat.AAC_ADTS)
            rec.setAudioEncoder(MediaRecorder.AudioEncoder.AAC)
            // Speech-tuned: mono, 32 kbps AAC at 44.1 kHz. Small and clear.
            rec.setAudioChannels(1)
            rec.setAudioEncodingBitRate(32_000)
            rec.setAudioSamplingRate(44_100)
            rec.setOutputFile(file.absolutePath)
            rec.prepare()
            rec.start()
            recorder = rec
            outputFile = file
            startedAtMs = System.currentTimeMillis()
            true
        } catch (e: Exception) {
            // prepare()/start() throw IllegalState/IOException/RuntimeException
            // on a busy or mic-less device. Roll back cleanly.
            Log.w(TAG, "Could not start voice recording", e)
            runCatching { rec.release() }
            file.delete()
            recorder = null
            outputFile = null
            WakeWordService.resume(MicHolder.VOICE_NOTE) // never got the mic — give it back
            false
        }
    }

    /**
     * Stop capturing and return the recorded clip, or `null` if the recording
     * was too short to be usable.
     *
     * A [MediaRecorder] that is stopped almost immediately after starting has
     * captured no frames and throws from `stop()`; that (an accidental tap
     * rather than a held press) is treated as "no voice note" — the file is
     * deleted and `null` returned. On success the caller owns the returned file
     * and must delete it once the bytes have been sent.
     */
    fun stop(): File? {
        val rec = recorder ?: return null
        val file = outputFile
        val heldMs = System.currentTimeMillis() - startedAtMs
        recorder = null
        outputFile = null

        return try {
            rec.stop()
            rec.release()
            // Guard against a too-brief press even when stop() didn't throw.
            if (file != null && file.exists() && file.length() > 0 && heldMs >= MIN_CLIP_MS) {
                file
            } else {
                file?.delete()
                null
            }
        } catch (e: RuntimeException) {
            // stop() throws when no valid data was captured (very short press).
            Log.d(TAG, "Voice recording too short; discarding", e)
            runCatching { rec.release() }
            file?.delete()
            null
        } finally {
            WakeWordService.resume(MicHolder.VOICE_NOTE)
        }
    }

    /** Abort an in-progress recording and delete its partial file. */
    fun cancel() {
        val rec = recorder ?: return
        recorder = null
        val file = outputFile
        outputFile = null
        runCatching { rec.stop() }
        runCatching { rec.release() }
        file?.delete()
        WakeWordService.resume(MicHolder.VOICE_NOTE)
    }

    private fun newRecorder(): MediaRecorder =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            MediaRecorder(context)
        } else {
            @Suppress("DEPRECATION")
            MediaRecorder()
        }

    companion object {
        private const val TAG = "VoiceRecorder"

        /** The MIME type of the clips this recorder produces. */
        const val MIME_TYPE = "audio/aac"

        /** Shorter presses than this are treated as accidental taps, not notes. */
        private const val MIN_CLIP_MS = 500L
    }
}
