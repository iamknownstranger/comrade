package mullu.comrade.together

import android.content.Context
import android.media.audiofx.Equalizer

/**
 * The player's remembered shape: shuffle, repeat, speed, equalizer.
 *
 * Plain [android.content.SharedPreferences] rather than the vault, and that is
 * a considered answer: these are *preferences about sound*, not diary rows.
 * Favourites/history live behind unlock because they say what somebody listens
 * to; "shuffle was on" says almost nothing, and it must be right before any
 * vault exists — a queue resumed into file order because prefs woke up late is
 * the small confident lie this feature avoids elsewhere.
 *
 * One key per fact rather than one JSON blob ([StreamingSourcesStore]'s shape):
 * these are independent toggles written from different screens, and a crash
 * mid-write losing all four because they shared a blob would be worse than a
 * half-written boolean. `apply()` commits synchronously — every value here is
 * tiny, and a toggle that has not landed by process death did not happen.
 */
object PlayerPrefs {

    private const val PREFS = "player_prefs"
    private const val KEY_SHUFFLE = "shuffle"
    private const val KEY_REPEAT = "repeat"
    private const val KEY_SPEED = "speed"
    private const val KEY_EQ_ON = "eq_on"
    private const val KEY_EQ_BANDS = "eq_bands"

    fun shuffle(context: Context): Boolean =
        prefs(context).getBoolean(KEY_SHUFFLE, false)

    fun setShuffle(context: Context, on: Boolean) {
        prefs(context).edit().putBoolean(KEY_SHUFFLE, on).commit()
    }

    fun repeat(context: Context): TogetherDecisions.RepeatMode =
        runCatching {
            TogetherDecisions.RepeatMode.valueOf(prefs(context).getString(KEY_REPEAT, null) ?: "OFF")
        }.getOrDefault(TogetherDecisions.RepeatMode.OFF)

    fun setRepeat(context: Context, mode: TogetherDecisions.RepeatMode) {
        prefs(context).edit().putString(KEY_REPEAT, mode.name).commit()
    }

    /** Persisted as the bits of a float; 1.0 when nothing sensible is stored. */
    fun speed(context: Context): Float =
        Float.fromBits(prefs(context).getInt(KEY_SPEED, 1.0f.toRawBits()))

    fun setSpeed(context: Context, rate: Float) {
        prefs(context).edit().putInt(KEY_SPEED, rate.toRawBits()).commit()
    }

    /**
     * Band levels in millibels — [Equalizer.setBandLevel]'s own unit — so no
     * layer ever converts. Empty means "never shaped"; the count comes from
     * the device when the effect is first drawn.
     */
    fun equalizer(context: Context): Pair<Boolean, List<Int>> {
        val p = prefs(context)
        val bands = p.getString(KEY_EQ_BANDS, null)
            ?.split(',')
            ?.mapNotNull { it.toIntOrNull() }
            .orEmpty()
        return p.getBoolean(KEY_EQ_ON, false) to bands
    }

    fun setEqualizer(context: Context, on: Boolean, bandMillibels: List<Int>) {
        prefs(context)
            .edit()
            .putBoolean(KEY_EQ_ON, on)
            .putString(KEY_EQ_BANDS, bandMillibels.joinToString(","))
            .apply()
    }

    private fun prefs(context: Context) =
        context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
}
