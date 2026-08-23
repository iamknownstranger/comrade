package mullu.comrade.together

import android.content.Context
import org.json.JSONObject

/**
 * Where the streaming sources' credentials live: this phone's own
 * SharedPreferences, read per call and never sent anywhere except to the
 * server they name.
 *
 * The vault-first rule would put these behind unlock, but a music search is
 * the same kind of pre-unlock act as the catalogue lookup it sits beside —
 * `catalogue_lookup` works before unlock deliberately — and neither resolver's
 * credential is an identity. They are also exactly the fields a settings
 * screen round-trips while the user is looking at them, which SharedPreferences
 * is for; [org.json] rather than kotlinx-serialization because everything else
 * in this package that touches preferences parses by hand and adds no
 * dependency to do it.
 *
 * **The Subsonic password leaves the device as a salted token** (see
 * `comrade_core::subsonic`'s header), so storing it here plainly is no worse
 * than any other app preference on an unlocked device — and the alternative,
 * asking for it every session, reads as the feature being broken.
 */
data class StreamingSources(
    val server: String,
    val username: String,
    val password: String,
    /** developer.jamendo.com client id, enabling the second catalogue. Empty = off. */
    val jamendoClientId: String,
) {
    /** A server worth searching has all three parts filled. */
    val subsonicConfigured: Boolean
        get() = server.isNotBlank() && username.isNotBlank() && password.isNotBlank()

    val jamendoConfigured: Boolean
        get() = jamendoClientId.isNotBlank()

    fun toJson(): String =
        JSONObject().apply {
            put(KEY_SERVER, server)
            put(KEY_USERNAME, username)
            put(KEY_PASSWORD, password)
            put(KEY_JAMENDO, jamendoClientId)
        }.toString()

    companion object {
        private const val KEY_SERVER = "server"
        private const val KEY_USERNAME = "username"
        private const val KEY_PASSWORD = "password"
        private const val KEY_JAMENDO = "jamendo_client_id"

        val EMPTY = StreamingSources("", "", "", "")

        fun fromJson(text: String?): StreamingSources {
            if (text.isNullOrBlank()) return EMPTY
            return runCatching {
                val obj = JSONObject(text)
                StreamingSources(
                    server = obj.optString(KEY_SERVER),
                    username = obj.optString(KEY_USERNAME),
                    password = obj.optString(KEY_PASSWORD),
                    jamendoClientId = obj.optString(KEY_JAMENDO),
                )
            }.getOrDefault(EMPTY)
        }
    }
}

/**
 * Read/write access to [StreamingSources], kept as its own object rather than
 * hanging off [TogetherManager] so a settings screen needs none of the
 * session machinery to load and save.
 *
 * One JSON blob under one prefs key rather than four keys, because the sources
 * are one fact about one device — "here is where your music comes from" — and
 * a half-written set of four keys after a crash is a config that lies.
 */
object StreamingSourcesStore {
    private const val PREFS = "together_sources"
    private const val KEY_SOURCES = "sources_json"

    fun load(context: Context): StreamingSources {
        val prefs = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        return StreamingSources.fromJson(prefs.getString(KEY_SOURCES, null))
    }

    fun save(context: Context, sources: StreamingSources) {
        prefs(context).edit().putString(KEY_SOURCES, sources.toJson()).apply()
    }

    fun clear(context: Context) {
        prefs(context).edit().remove(KEY_SOURCES).apply()
    }

    private fun prefs(context: Context) =
        context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
}
