package mullu.comrade

import android.app.Application
import android.os.SystemClock
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

/**
 * Warms the native core as soon as the process exists, off the main thread,
 * and owns the process-lifetime [appScope] used for work that must outlive
 * any single Activity/Service.
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

    /**
     * Process-lifetime coroutine scope — not tied to any Activity/Service,
     * so it survives configuration changes and backgrounding. Replaces what
     * used to be a bare `GlobalScope.launch` inside [ComradeCore]'s class
     * initialiser (untethered to *anything*, uncancellable, and not the
     * kind of handle a caller could ever await): registering here instead
     * gives the app one real owner for that startup work and for
     * [mullu.comrade.RelayConnectionService]'s own lifecycle.
     */
    val appScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

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

        // Started early so the listener is normally already registered by
        // the time the user finishes unlocking — but correctness never
        // depends on that race: `unlockVaultTyped` awaits this same
        // (idempotent) call itself before it does anything that could
        // publish an event. See ComradeCore.initializeEventBridge.
        appScope.launch(Dispatchers.IO) {
            runCatching { ComradeCore.initializeEventBridge() }
                .onFailure { Log.e(TAG, "event bridge init failed", it) }
        }
    }

    private companion object {
        const val TAG = "ComradeApplication"
    }
}
