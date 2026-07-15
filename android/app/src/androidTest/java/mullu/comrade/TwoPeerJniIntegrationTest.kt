package mullu.comrade

import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeoutOrNull
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Test
import org.junit.runner.RunWith
import uniffi.comrade.BridgeEventListener
import uniffi.comrade.Comrade
import uniffi.comrade_ui.BridgeEvent
import java.io.File
import java.util.concurrent.ConcurrentLinkedQueue

/**
 * COMMS-03: two independent `Comrade` FFI instances — own vault directory
 * each — exchanging an encrypted DM across the **real** JNI/uniffi boundary
 * this device runs. This is the Android-side complement to
 * `crates/comrade_ui/tests/two_peer_integration.rs`: that suite proves the
 * Rust signaling logic is correct against an in-process fake relay; this one
 * proves the generated Kotlin bindings — async `suspend fun` bridging,
 * `BridgeEventListener` callback delivery across the FFI boundary — actually
 * work on a real device, which pure Rust tests cannot touch at all.
 *
 * ## Isolated relay, not the public internet
 * Both instances connect to one relay address read from the
 * `comradeTestRelayUrl` instrumentation argument (see
 * `deploy/test-relay/README.md` for how CI supplies it — a relay container
 * reachable at `10.0.2.2:<port>` from inside the emulator, the standard
 * host-loopback address every Android emulator exposes). Without that
 * argument this test is **skipped**, not silently pointed at the public
 * relay pool — a two-peer test flaking on a real relay's availability/rate
 * limits would defeat the point of an isolated test environment.
 *
 * ## Two instances, not two installations
 * This uses two in-process `Comrade` objects rather than two separately
 * installed app IDs. `build.gradle.kts`'s `deviceHarnessRole` property is the
 * (lower-risk — see its own comment) mechanism that *does* produce two
 * installable app IDs with isolated storage for a fuller cross-app harness;
 * wiring that up is future work, not bundled into this default test target.
 */
@RunWith(AndroidJUnit4::class)
class TwoPeerJniIntegrationTest {

    private class RecordingListener : BridgeEventListener {
        val events = ConcurrentLinkedQueue<BridgeEvent>()
        override fun onEvent(event: BridgeEvent) {
            events.offer(event)
        }
    }

    private suspend fun waitFor(
        listener: RecordingListener,
        timeoutMs: Long = 15_000L,
        predicate: (BridgeEvent) -> Boolean,
    ): BridgeEvent? = withTimeoutOrNull(timeoutMs) {
        while (true) {
            listener.events.poll()?.let { if (predicate(it)) return@withTimeoutOrNull it }
            delay(100)
        }
        @Suppress("UNREACHABLE_CODE")
        null
    }

    @Test
    fun two_real_ffi_instances_exchange_a_gated_dm() {
        val relayUrl = InstrumentationRegistry.getArguments().getString("comradeTestRelayUrl")
        org.junit.Assume.assumeTrue(
            "requires an isolated test relay — pass -e comradeTestRelayUrl <ws-url> " +
                "(see deploy/test-relay/README.md); skipping rather than hitting the public relay pool",
            !relayUrl.isNullOrBlank(),
        )
        // Assume.assumeTrue doesn't smart-cast — relayUrl is still `String?` to
        // the compiler below, even though the assume above already guarantees
        // it's non-blank at runtime.
        val testRelayUrl = requireNotNull(relayUrl)

        val context = ApplicationProvider.getApplicationContext<android.content.Context>()
        val aliceDir = File(context.filesDir, "jni-2peer-alice")
        val bobDir = File(context.filesDir, "jni-2peer-bob")

        val alice = Comrade.newWithRelays(listOf(testRelayUrl))
        val bob = Comrade.newWithRelays(listOf(testRelayUrl))
        val aliceEvents = RecordingListener()
        val bobEvents = RecordingListener()

        runBlocking {
            alice.setEventListener(aliceEvents)
            bob.setEventListener(bobEvents)
            alice.unlockVault(aliceDir.absolutePath, "pin")
            bob.unlockVault(bobDir.absolutePath, "pin")
        }

        // Sync (non-suspend) FFI calls — called directly, no runBlocking needed.
        val aliceNpub = requireNotNull(alice.currentIdentity()).npub
        val bobNpub = requireNotNull(bob.currentIdentity()).npub

        runBlocking { alice.sendDm(bobNpub, "hello across the real JNI boundary") }

        val received = runBlocking {
            waitFor(bobEvents) { it is BridgeEvent.IncomingMessageRequest }
        }
        assertNotNull("bob's real Comrade instance must receive alice's DM as a message request", received)
        val request = (received as BridgeEvent.IncomingMessageRequest).v1
        assertEquals(aliceNpub, request.peer)
    }
}
