package mullu.comrade.together

import android.media.audiofx.Equalizer
import android.util.Log

/**
 * The equalizer, attached to whatever audio session the file player is using.
 *
 * Deliberately small: one toggle and the raw band gains, persisted through
 * [PlayerPrefs] so the shape survives the app. It exists because a music
 * player without any tone control is a music player for other people's rooms —
 * but it is bounded on purpose:
 *
 * - **It only ever reaches [TogetherPlayer].** The YouTube embed and an
 *   externally-followed app are out of reach by construction: their sound is
 *   mixed inside another process or another SDK, and no session id of ours
 *   touches it. A toggle that silently did nothing there would be worse than
 *   none, so the screen shows this control only where it works.
 * - **Levels are stored in millibels**, Android's own unit for
 *   [Equalizer.setBandLevel] — no conversion anywhere, one range
 *   (`bandLevelRange`, typically ±15 dB) asked from the device itself rather
 *   than assumed.
 * - **Re-attached per open, never per frame.** `MediaPlayer()` mints a fresh
 *   session id every open; [attach] is called from wherever a new player was
 *   minted. An equalizer whose bands reset every track is a broken one.
 * - **Every call is guarded.** `audiofx` throws on some devices (vendor effect
 *   limits, too many effects). Tone control must never cost playback.
 */
object PlayerEffects {

    private var equalizer: Equalizer? = null
    private var enabled = false
    private var bands: List<Int> = emptyList()

    /** How many bands this device offers, `0` when there is none to use. */
    fun bandCount(): Int = runCatching {
        val probe = Equalizer(0, 0)
        val count = probe.numberOfBands.toInt()
        probe.release()
        count
    }.onFailure { Log.w(TAG, "no equalizer on this device", it) }.getOrDefault(0)

    /**
     * Point the stored shape at a fresh audio session. Called wherever a new
     * [TogetherPlayer] was just opened; a no-op when disabled.
     */
    fun attach(sessionId: Int?) {
        runCatching { equalizer?.enabled = false }
        runCatching { equalizer?.release() }
        equalizer = null
        val id = sessionId ?: return
        if (!enabled) return
        runCatching {
            val eq = Equalizer(0, id)
            eq.enabled = true
            // `bandLevelRange` is a ShortArray — indexed, not destructured.
            val low = eq.bandLevelRange[0]
            val high = eq.bandLevelRange[1]
            bands.forEachIndexed { i, level ->
                if (i < eq.numberOfBands) {
                    eq.setBandLevel(i.toShort(), level.toShort().coerceIn(low, high))
                }
            }
            equalizer = eq
        }.onFailure { Log.w(TAG, "equalizer attach failed", it) }
    }

    fun detach() {
        runCatching { equalizer?.enabled = false }
        runCatching { equalizer?.release() }
        equalizer = null
    }

    /**
     * Apply the whole shape to the live effect only. **Persistence is the
     * caller's job** — [PlayerPrefs.setEqualizer] beside this call — so this
     * object needs no Context and can never disagree with prefs about which
     * one is the source of truth: prefs are, and this just sounds them.
     */
    fun applyLive(on: Boolean, bandMillibels: List<Int>, sessionId: Int?) {
        enabled = on
        bands = bandMillibels
        if (!on) detach() else attach(sessionId)
    }

    private const val TAG = "PlayerEffects"
}
