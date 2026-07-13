package mullu.comrade.call

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioManager
import android.media.Ringtone
import android.media.RingtoneManager
import android.os.Build
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import android.util.Log

/**
 * Rings and vibrates for an incoming call, honoring the device's ringer mode.
 * Deliberately its own object rather than folded into [CallManager] — a
 * fire-and-forget [start]/[stop] pair is easy to audit for the one thing that
 * must never happen: a ringtone that outlives the call. [stop] is safe to call
 * repeatedly and when nothing is ringing.
 */
object Ringer {
    private const val TAG = "Ringer"

    /**
     * Vibration waveform: buzz for 1s, pause for 1s, repeat. `VIBRATE_REPEAT_FROM`
     * tells [VibrationEffect] to loop from index 1 (the buzz) rather than
     * replaying the leading zero-delay from the start of the array each cycle.
     */
    private val VIBRATE_PATTERN = longArrayOf(0, 1_000, 1_000)
    private const val VIBRATE_REPEAT_FROM = 1

    private var ringtone: Ringtone? = null
    private var vibrator: Vibrator? = null

    /** Start ringing/vibrating for an incoming call. No-op if already started. */
    @Synchronized
    fun start(context: Context) {
        if (ringtone != null || vibrator != null) return
        val mode = (context.getSystemService(Context.AUDIO_SERVICE) as? AudioManager)
            ?.ringerMode ?: AudioManager.RINGER_MODE_NORMAL

        if (mode == AudioManager.RINGER_MODE_NORMAL) startRingtone(context)
        if (mode != AudioManager.RINGER_MODE_SILENT) startVibration(context)
    }

    /** Stop ringing/vibrating. Safe to call any number of times, including when idle. */
    @Synchronized
    fun stop() {
        ringtone?.let { runCatching { it.stop() } }
        ringtone = null
        vibrator?.let { runCatching { it.cancel() } }
        vibrator = null
    }

    private fun startRingtone(context: Context) {
        runCatching {
            val uri = RingtoneManager.getActualDefaultRingtoneUri(context, RingtoneManager.TYPE_RINGTONE)
                ?: return
            val ring = RingtoneManager.getRingtone(context, uri) ?: return
            ring.audioAttributes = AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_NOTIFICATION_RINGTONE)
                .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                .build()
            // Ringtone.setLooping was only added in API 28; below that, ring once
            // rather than crash with a NoSuchMethodError on minSdk 26/27 devices.
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) ring.isLooping = true
            ring.play()
            ringtone = ring
        }.onFailure { Log.w(TAG, "ringtone playback failed", it) }
    }

    private fun startVibration(context: Context) {
        val vib = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            (context.getSystemService(Context.VIBRATOR_MANAGER_SERVICE) as? VibratorManager)?.defaultVibrator
        } else {
            @Suppress("DEPRECATION")
            context.getSystemService(Context.VIBRATOR_SERVICE) as? Vibrator
        }
        if (vib == null || !vib.hasVibrator()) return
        runCatching {
            vib.vibrate(VibrationEffect.createWaveform(VIBRATE_PATTERN, VIBRATE_REPEAT_FROM))
            vibrator = vib
        }.onFailure { Log.w(TAG, "vibration failed", it) }
    }
}
