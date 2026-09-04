package mullu.comrade

import android.app.Application
import android.content.ComponentCallbacks2
import android.os.SystemClock
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import mullu.comrade.ui.MediaCache
import mullu.comrade.update.UpdateCheckJob

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
// `open` for the Flutter build's `ComradeFlutterApplication`, which extends this
// to add one line (starting `CallStateReactor` at process start) while keeping
// the native warm-up and [appScope] below — see
// app/android/app/src/main/kotlin/mullu/comrade/PLATFORM_CHANNELS.md §4.3.
// Nothing here is overridden and no behaviour changes for the Compose build.
open class ComradeApplication : Application() {

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

        // Before anything can report the mesh running. `MeshRadio` needs an
        // application context to reach `WifiManager`, and the status event that
        // makes it take the multicast lock can arrive from the background
        // service without an Activity ever existing — so the one place
        // guaranteed to have run first is here.
        MeshRadio.attach(this)

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

        // Off the main thread because it touches SharedPreferences and the job
        // queue. Idempotent by design — it is a *reconcile*, not a schedule, so
        // running it at every process start does not reset the period of a job
        // that is already queued. See UpdateCheckJob.sync.
        appScope.launch(Dispatchers.IO) {
            runCatching { UpdateCheckJob.sync(this@ComradeApplication) }
                .onFailure { Log.w(TAG, "could not reconcile the update check job", it) }
        }
    }

    /**
     * The signal `MainActivity.onStop`'s S-4 purge cannot see: a
     * still-foreground app that the system is trimming, most often while
     * scrolling a photo-heavy thread — [MediaCache]'s bitmap LRU is bounded
     * in bytes now (see its class doc), but bounded still means "up to 48 MB
     * held for no reason once the system has said memory is tight."
     */
    override fun onTrimMemory(level: Int) {
        super.onTrimMemory(level)
        MediaCache.onTrimMemory(level)
    }

    /**
     * Pre-`ComponentCallbacks2` devices (API < 14, none this app still
     * targets) only ever call this, never [onTrimMemory] — kept anyway
     * because `Application` still declares it, and a caller reading only
     * this override should see the same worst-case behaviour
     * [onTrimMemory] gives `TRIM_MEMORY_COMPLETE`, not silence.
     */
    override fun onLowMemory() {
        super.onLowMemory()
        MediaCache.onTrimMemory(ComponentCallbacks2.TRIM_MEMORY_COMPLETE)
    }

    private companion object {
        const val TAG = "ComradeApplication"
    }
}
