package mullu.comrade.voice

import java.io.File
import java.io.IOException
import java.security.MessageDigest
import java.util.zip.ZipEntry
import java.util.zip.ZipOutputStream
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

/**
 * Behaviour of the on-demand voice-model install pipeline
 * ([VoiceModelInstaller]) — the part of the "download the speech model?" flow
 * that must never install unverified or path-escaping bytes:
 *
 *  - the official zips' single wrapper folder is flattened away, so the
 *    install dir directly contains `am/`, `conf/`, … (what Vosk's `Model`
 *    constructor expects);
 *  - a checksum mismatch refuses the download outright;
 *  - a zip-slip entry aborts extraction without writing outside the target;
 *  - cancellation and failure leave no partial install behind.
 */
class VoiceModelInstallerTest {

    @get:Rule
    val tmp = TemporaryFolder()

    // ── fixtures ─────────────────────────────────────────────────────────────

    /** Writes [entries] (name → bytes, null bytes = directory entry) into a fresh zip. */
    private fun zipOf(entries: Map<String, ByteArray?>): File {
        val zip = tmp.newFile("${entries.hashCode()}.zip")
        ZipOutputStream(zip.outputStream()).use { out ->
            for ((name, bytes) in entries) {
                out.putNextEntry(ZipEntry(name))
                bytes?.let { out.write(it) }
                out.closeEntry()
            }
        }
        return zip
    }

    /** The layout the official `vosk-model-small-en-us-0.15.zip` uses: one wrapper folder around everything. */
    private fun officialStyleModelZip(): File = zipOf(
        linkedMapOf(
            "vosk-model-small-en-us-0.15/" to null,
            "vosk-model-small-en-us-0.15/am/" to null,
            "vosk-model-small-en-us-0.15/am/final.mdl" to "acoustic-model".toByteArray(),
            "vosk-model-small-en-us-0.15/conf/" to null,
            "vosk-model-small-en-us-0.15/conf/model.conf" to "config".toByteArray(),
            "vosk-model-small-en-us-0.15/graph/phones/word_boundary.int" to "graph".toByteArray(),
            "vosk-model-small-en-us-0.15/README" to "readme".toByteArray(),
        ),
    )

    private fun sha256(file: File): String =
        MessageDigest.getInstance("SHA-256").digest(file.readBytes())
            .joinToString("") { "%02x".format(it) }

    // ── extraction ───────────────────────────────────────────────────────────

    @Test
    fun `extraction flattens the wrapper folder the official zips use`() {
        val dest = tmp.newFolder("model")

        VoiceModelInstaller.extractInto(officialStyleModelZip(), dest)

        assertEquals("acoustic-model", File(dest, "am/final.mdl").readText())
        assertEquals("config", File(dest, "conf/model.conf").readText())
        assertEquals("graph", File(dest, "graph/phones/word_boundary.int").readText())
        assertFalse(
            "the wrapper folder must not survive flattening",
            File(dest, "vosk-model-small-en-us-0.15").exists(),
        )
        assertTrue(VoiceModelInstaller.looksLikeModel(dest))
    }

    @Test
    fun `extraction keeps paths as-is when there is no single wrapper folder`() {
        val dest = tmp.newFolder("flat")
        val zip = zipOf(
            linkedMapOf(
                "am/" to null,
                "am/final.mdl" to "a".toByteArray(),
                "conf/model.conf" to "c".toByteArray(),
            ),
        )

        VoiceModelInstaller.extractInto(zip, dest)

        assertEquals("a", File(dest, "am/final.mdl").readText())
        assertEquals("c", File(dest, "conf/model.conf").readText())
    }

    @Test
    fun `a zip-slip entry aborts extraction without writing outside the target`() {
        val parent = tmp.newFolder("parent")
        val dest = File(parent, "inner").apply { check(mkdirs()) }
        val zip = zipOf(
            linkedMapOf(
                "ok.txt" to "fine".toByteArray(),
                "../escaped.txt" to "evil".toByteArray(),
            ),
        )

        assertThrows(IOException::class.java) { VoiceModelInstaller.extractInto(zip, dest) }

        assertFalse(
            "the traversal entry must not land next to the target dir",
            File(parent, "escaped.txt").exists(),
        )
    }

    // ── validation ───────────────────────────────────────────────────────────

    @Test
    fun `looksLikeModel demands the acoustic model and decoder config`() {
        val dir = tmp.newFolder("candidate")
        assertFalse(VoiceModelInstaller.looksLikeModel(dir))

        check(File(dir, "am").mkdirs())
        assertFalse("am/ alone is not a model", VoiceModelInstaller.looksLikeModel(dir))

        check(File(dir, "conf").mkdirs())
        assertTrue(VoiceModelInstaller.looksLikeModel(dir))
    }

    @Test
    fun `sha256Hex matches the known digest of abc`() {
        val file = tmp.newFile("abc.txt").apply { writeText("abc") }
        assertEquals(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            VoiceModelInstaller.sha256Hex(file),
        )
    }

    // ── the full pipeline ────────────────────────────────────────────────────

    @Test
    fun `fetchAndInstall installs a verified model end to end`() {
        val zip = officialStyleModelZip()
        val zipCache = File(tmp.root, "cache/model.zip.part")
        val staging = File(tmp.root, "voice-model.staging")
        val install = File(tmp.root, "voice-model")
        val progress = mutableListOf<Pair<Long, Long>>()
        var installingSignalled = false

        VoiceModelInstaller.fetchAndInstall(
            url = zip.toURI().toURL(),
            expectedSha256 = sha256(zip),
            zipCache = zipCache,
            stagingDir = staging,
            installDir = install,
            onProgress = { read, total -> progress += read to total },
            onInstalling = { installingSignalled = true },
        )

        assertTrue("the finished install must look like a model", VoiceModelInstaller.looksLikeModel(install))
        assertEquals("acoustic-model", File(install, "am/final.mdl").readText())
        assertTrue("the installing phase must be signalled", installingSignalled)
        assertEquals("download must report completion", zip.length(), progress.last().first)
        assertFalse("the downloaded zip must be cleaned up", zipCache.exists())
        assertFalse("the staging dir must be cleaned up", staging.exists())
    }

    @Test
    fun `fetchAndInstall replaces a previous broken install`() {
        val zip = officialStyleModelZip()
        val install = File(tmp.root, "voice-model")
        check(File(install, "am").mkdirs()) // stale halfway install, no conf/
        File(install, "stale.txt").writeText("stale")

        VoiceModelInstaller.fetchAndInstall(
            url = zip.toURI().toURL(),
            expectedSha256 = sha256(zip),
            zipCache = File(tmp.root, "model.zip.part"),
            stagingDir = File(tmp.root, "staging"),
            installDir = install,
        )

        assertTrue(VoiceModelInstaller.looksLikeModel(install))
        assertFalse("stale content must be gone", File(install, "stale.txt").exists())
    }

    @Test
    fun `a checksum mismatch refuses the download and installs nothing`() {
        val zip = officialStyleModelZip()
        val zipCache = File(tmp.root, "model.zip.part")
        val staging = File(tmp.root, "staging")
        val install = File(tmp.root, "voice-model")

        val failure = assertThrows(IOException::class.java) {
            VoiceModelInstaller.fetchAndInstall(
                url = zip.toURI().toURL(),
                expectedSha256 = "0".repeat(64),
                zipCache = zipCache,
                stagingDir = staging,
                installDir = install,
            )
        }

        assertTrue("the error must say why", failure.message.orEmpty().contains("checksum mismatch"))
        assertFalse("nothing may be installed from unverified bytes", install.exists())
        assertFalse("the rejected zip must be deleted", zipCache.exists())
        assertFalse("staging must be cleaned up", staging.exists())
    }

    @Test
    fun `an archive without a model layout is refused`() {
        val zip = zipOf(linkedMapOf("wrapper/" to null, "wrapper/readme.txt" to "not a model".toByteArray()))
        val install = File(tmp.root, "voice-model")

        val failure = assertThrows(IOException::class.java) {
            VoiceModelInstaller.fetchAndInstall(
                url = zip.toURI().toURL(),
                expectedSha256 = sha256(zip),
                zipCache = File(tmp.root, "model.zip.part"),
                stagingDir = File(tmp.root, "staging"),
                installDir = install,
            )
        }

        assertTrue(failure.message.orEmpty().contains("not a Vosk model"))
        assertFalse(install.exists())
    }

    @Test
    fun `cancelling mid-download aborts and leaves nothing behind`() {
        val zip = officialStyleModelZip()
        val zipCache = File(tmp.root, "model.zip.part")
        val install = File(tmp.root, "voice-model")
        var checks = 0

        assertThrows(InstallCancelledException::class.java) {
            VoiceModelInstaller.fetchAndInstall(
                url = zip.toURI().toURL(),
                expectedSha256 = sha256(zip),
                zipCache = zipCache,
                stagingDir = File(tmp.root, "staging"),
                installDir = install,
                // First poll lets the download start; the next aborts it.
                isCancelled = { checks++ > 0 },
            )
        }

        assertFalse("a cancelled download must not install", install.exists())
        assertFalse("the partial zip must be deleted", zipCache.exists())
    }
}
