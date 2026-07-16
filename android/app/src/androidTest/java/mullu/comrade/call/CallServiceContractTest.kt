package mullu.comrade.call

import android.Manifest
import android.content.Context
import android.content.Intent
import android.os.SystemClock
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.rule.GrantPermissionRule
import org.junit.After
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * On-device regression tests for the foreground-service contract in
 * [CallService]. Both are deterministic reproductions of a production crash +
 * CI flake:
 * `android.app.RemoteServiceException$ForegroundServiceDidNotStartInTimeException`
 * — after `Context.startForegroundService()`, the platform kills the whole
 * process ~10s later (asynchronously, and uncatchably at the call site) unless
 * the service calls `startForeground()` at least once, even if it stops itself
 * immediately. These need the real device runtime because that kill is enforced
 * by the platform's activity-manager, not by anything a JVM unit test can
 * observe.
 *
 * Shape mirrors [CallManagerLifecycleTest]: [AndroidJUnit4] +
 * [ApplicationProvider], no Compose/Activity rule needed since these drive the
 * service directly.
 *
 * Permissions: on API 34+ a `microphone`-typed `startForeground()` throws
 * unless [Manifest.permission.RECORD_AUDIO] is granted at call time (the
 * device lanes run API 35). The fix's placeholder in [CallService.onCreate]
 * uses exactly that mic type, so the grant below is what lets the placeholder
 * *succeed* and satisfy the contract on those lanes — making the after-fix pass
 * deterministic rather than dependent on the platform's (version-specific)
 * handling of a *rejected* startForeground. In production RECORD_AUDIO is
 * always held by the time a call reaches [CallService] (the call UI gates on
 * it), so this mirrors the real path. POST_NOTIFICATIONS is granted too so the
 * placeholder notification actually posts; `startForeground()` itself does not
 * require it (the service still runs, the notification just wouldn't show), but
 * granting it keeps the exercised path faithful to production. The grant is
 * app-wide and not revoked, matching how [androidx.test.rule.GrantPermissionRule]
 * works elsewhere in this module ([mullu.comrade.MainActivityUiTest]).
 *
 * Both tests use [assumeTrue] to *skip* (not fail) if the platform refuses the
 * foreground-service start outright — an API 34+ background-FGS-start
 * disallowal is possible in the bare instrumentation process before any
 * Activity is foregrounded. A refused start never creates the service, so the
 * ~10s did-not-start-in-time kill can't arm and the scenario is moot; only an
 * accepted start can exercise the bug. Skipping there is deliberate: a hard
 * failure on a lane that can't run the scenario would just reintroduce a flake.
 * (The observed CI crash proves `startForegroundService` *is* accepted in this
 * instrumentation environment, so in practice these run rather than skip.)
 */
@RunWith(AndroidJUnit4::class)
class CallServiceContractTest {

    @get:Rule
    val permissions: GrantPermissionRule = GrantPermissionRule.grant(
        Manifest.permission.RECORD_AUDIO,
        Manifest.permission.POST_NOTIFICATIONS,
    )

    /** Outlast the platform's ~10s did-not-start-in-time deadline with margin. */
    private val contractDeadlineMarginMs = 12_000L

    private fun appContext(): Context = ApplicationProvider.getApplicationContext()

    @After
    fun stopService() {
        // Guarantee the service is torn down even if a test left it running.
        runCatching { CallService.stop(appContext()) }
    }

    /**
     * Variant (a): a blank/null intent (the redelivered-restart shape). Before
     * the fix, `onStartCommand` bailed with `stopSelf()` *without* ever calling
     * `startForeground()`, so ~10s later the platform killed the whole process
     * — which would take this test process down mid-run. After the fix,
     * `onCreate` satisfies the contract with a placeholder before
     * `onStartCommand` removes it and stops.
     */
    @Test
    fun blank_intent_start_does_not_crash_the_process() {
        val ctx = appContext()
        val started = runCatching {
            ctx.startForegroundService(Intent(ctx, CallService::class.java))
        }
        assumeTrue(
            "platform refused the foreground-service start; contract-kill scenario not reachable on this lane",
            started.isSuccess,
        )

        SystemClock.sleep(contractDeadlineMarginMs)

        // Only runs if the process survived the deadline: re-acquiring a live
        // application context proves the process was not killed.
        assertTrue(
            "process survived the foreground-service-contract deadline after a blank-intent start",
            appContext().packageName.isNotEmpty(),
        )
    }

    /**
     * Variant (b): the stop-before-start race — [CallService.stop]
     * (`stopService`) dispatched immediately after [CallService.start]
     * (`startForegroundService`), i.e. place-then-instant-cancel, the pattern
     * [CallManagerLifecycleTest] exercises one layer up. If `stopService`
     * destroys the instance before `onStartCommand` would have called
     * `startForeground()`, the pre-fix service went foreground never — arming
     * the ~10s kill. After the fix, `onCreate` calls `startForeground()` the
     * moment the instance is created, so the contract holds even when stop wins
     * the race. The peer string is opaque to [CallService] (used only as a
     * notification label / PendingIntent request code), so a placeholder value
     * exercises the valid-peer path without needing a real key.
     */
    @Test
    fun instant_stop_after_start_does_not_crash_the_process() {
        val ctx = appContext()
        val started = runCatching {
            CallService.start(ctx, "npub1testtesttesttesttesttesttesttesttest", "Test Peer", video = false)
            CallService.stop(ctx)
        }
        assumeTrue(
            "platform refused the foreground-service start; stop-before-start race not reachable on this lane",
            started.isSuccess,
        )

        SystemClock.sleep(contractDeadlineMarginMs)

        assertTrue(
            "process survived the foreground-service-contract deadline after an instant stop-after-start",
            appContext().packageName.isNotEmpty(),
        )
    }
}
