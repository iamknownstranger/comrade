package mullu.comrade

import android.content.Context
import android.util.Log

/**
 * Applies the TURN relay baked into the build (`BuildConfig.DEFAULT_TURN_*`,
 * populated from the `TURN_URL` / `TURN_USERNAME` / `TURN_PASSWORD` CI secrets —
 * empty in local/PR builds) as the *default* relay for calls, without ever
 * clobbering a relay the user set for themselves in Settings.
 *
 * Called once per unlock (the encrypted store that holds the relay config must
 * be open — see [ComradeCore.setTurnServerTyped]). Rotation-safe: we remember
 * the last URL we auto-seeded, so a rotated secret in a new build updates a
 * previous auto-seed, but a relay the user typed in is never overwritten.
 *
 * SECURITY NOTE: a baked-in credential ships inside the APK and is extractable.
 * It is a convenience default only; a user-set relay always wins. See the TURN
 * setup notes for the abuse/quota tradeoff.
 */
object CallRelayDefaults {
    private const val TAG = "CallRelayDefaults"
    private const val PREFS = "comrade_prefs"
    private const val KEY_SEEDED_URL = "seeded_turn_url"

    /**
     * Seed [BuildConfig.DEFAULT_TURN_URL] as the relay if appropriate. Safe to
     * call on every unlock; a no-op when there is no baked-in default, when the
     * user has configured their own relay, or when the default is already
     * applied. Never throws — a malformed baked-in URL is logged and ignored.
     */
    fun seedIfNeeded(context: Context) {
        val url = BuildConfig.DEFAULT_TURN_URL
        if (url.isBlank()) return // no baked-in default (local/PR build)

        val status = runCatching { ComradeCore.turnServerStatusTyped() }.getOrNull() ?: return
        val prefs = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val lastSeeded = prefs.getString(KEY_SEEDED_URL, null)

        // Respect a user-set relay: only apply the default when nothing is
        // configured, or when what's configured is one we auto-seeded before
        // (so a rotated secret can replace a stale default, but a relay the
        // user typed is left alone).
        val userOwnsCurrent = status.configured && status.url != lastSeeded
        if (userOwnsCurrent) return

        if (status.configured && status.url == url) {
            // Already on the current default — just (re)record the marker.
            prefs.edit().putString(KEY_SEEDED_URL, url).apply()
            return
        }

        runCatching {
            ComradeCore.setTurnServerTyped(
                url,
                BuildConfig.DEFAULT_TURN_USERNAME,
                BuildConfig.DEFAULT_TURN_PASSWORD,
            )
        }.onSuccess {
            prefs.edit().putString(KEY_SEEDED_URL, url).apply()
            Log.i(TAG, "Applied baked-in default TURN relay")
        }.onFailure {
            // set_turn_server validates the URL shape (turn:host:port); a
            // malformed TURN_URL secret lands here. Never crash unlock over it.
            Log.w(TAG, "Failed to apply default TURN relay — check the TURN_URL secret is a turn:host:port URL", it)
        }
    }
}
