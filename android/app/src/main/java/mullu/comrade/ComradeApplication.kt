package mullu.comrade

import android.app.Application
import android.os.SystemClock
import android.util.Log

/**
 * Warms the native core as soon as the process exists, off the main thread.
 *
 * `libcomrade_jni.so` statically links the whole Rust core (tokio, nostr,
 * libp2p, sled …), so the first touch of [ComradeCore] pays for
 * `System.loadLibrary` — dynamic-linker mapping and relocation of a
 * multi-megabyte library. Left to happen lazily, that cost lands on the main
 * thread during the first Compose frame and shows up as slow app startup.
 *
 * Kicking the class-initialiser here on a background thread runs the load in
 * parallel with Activity/Compose bring-up. JVM class-init locking makes this
 * safe: any later touch from another thread either finds the library ready or
 * briefly waits for this one instead of redoing the work.
 */
class ComradeApplication : Application() {

    override fun onCreate() {
        super.onCreate()
        Thread({
            val started = SystemClock.uptimeMillis()
            runCatching { ComradeCore.getVersion() }
                .onSuccess { version ->
                    Log.i(TAG, "comrade_jni v$version warmed in ${SystemClock.uptimeMillis() - started} ms")
                }
                .onFailure { Log.e(TAG, "comrade_jni warm-up failed", it) }
        }, "comrade-core-warmup").start()
    }

    private companion object {
        const val TAG = "ComradeApplication"
    }
}
