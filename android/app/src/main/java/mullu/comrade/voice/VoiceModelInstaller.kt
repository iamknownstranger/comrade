package mullu.comrade.voice

import java.io.File
import java.io.IOException
import java.io.InterruptedIOException
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest
import java.util.zip.ZipFile

/** Thrown by [VoiceModelInstaller.fetchAndInstall] when the caller cancels mid-flight. */
internal class InstallCancelledException : InterruptedIOException("model download cancelled")

/**
 * The pure-JVM half of the on-demand voice-model install: download, checksum
 * verification, archive extraction, and layout validation. Deliberately free
 * of Android types so `VoiceModelInstallerTest` exercises it on the host JVM;
 * the Android-facing state machine around it is [VoiceModelDownloader].
 */
internal object VoiceModelInstaller {

    /**
     * Run the full pipeline: stream [url] into [zipCache] (reporting
     * `(bytesRead, totalBytes)` to [onProgress]; total is -1 until the server
     * says), refuse anything whose sha256 differs from [expectedSha256],
     * extract into [stagingDir], sanity-check the layout, then swap it into
     * [installDir]. Every failure path deletes the partial artifacts, and
     * [isCancelled] flipping true aborts with [InstallCancelledException] —
     * a verified model either lands complete at [installDir] or not at all.
     */
    fun fetchAndInstall(
        url: URL,
        expectedSha256: String,
        zipCache: File,
        stagingDir: File,
        installDir: File,
        onProgress: (Long, Long) -> Unit = { _, _ -> },
        onInstalling: () -> Unit = {},
        isCancelled: () -> Boolean = { false },
    ) {
        try {
            download(url, zipCache, onProgress, isCancelled)
            onInstalling()
            val actual = sha256Hex(zipCache)
            if (!actual.equals(expectedSha256, ignoreCase = true)) {
                throw IOException("model checksum mismatch — refusing it (expected $expectedSha256, got $actual)")
            }
            if (isCancelled()) throw InstallCancelledException()
            stagingDir.deleteRecursively()
            extractInto(zipCache, stagingDir)
            if (!looksLikeModel(stagingDir)) {
                throw IOException("downloaded archive is not a Vosk model (no am/ + conf/)")
            }
            installDir.deleteRecursively()
            installDir.parentFile?.mkdirs()
            if (!stagingDir.renameTo(installDir)) {
                throw IOException("could not move the model into place")
            }
        } finally {
            zipCache.delete()
            stagingDir.deleteRecursively()
        }
    }

    private fun download(
        url: URL,
        dest: File,
        onProgress: (Long, Long) -> Unit,
        isCancelled: () -> Boolean,
    ) {
        val connection = url.openConnection()
        (connection as? HttpURLConnection)?.apply {
            connectTimeout = 15_000
            readTimeout = 30_000
            val code = responseCode
            if (code != HttpURLConnection.HTTP_OK) throw IOException("model download failed: HTTP $code")
        }
        try {
            val total = connection.contentLengthLong
            dest.parentFile?.mkdirs()
            connection.getInputStream().use { input ->
                dest.outputStream().use { output ->
                    val buffer = ByteArray(64 * 1024)
                    var copied = 0L
                    var lastReported = 0L
                    onProgress(0L, total)
                    while (true) {
                        if (isCancelled()) throw InstallCancelledException()
                        val read = input.read(buffer)
                        if (read < 0) break
                        output.write(buffer, 0, read)
                        copied += read
                        // Throttle the callback: every 256 KiB is plenty for a progress bar.
                        if (copied - lastReported >= 256 * 1024) {
                            lastReported = copied
                            onProgress(copied, total)
                        }
                    }
                    onProgress(copied, total)
                }
            }
        } finally {
            (connection as? HttpURLConnection)?.disconnect()
        }
    }

    /** Hex sha256 of [file], streamed. */
    fun sha256Hex(file: File): String {
        val digest = MessageDigest.getInstance("SHA-256")
        file.inputStream().use { input ->
            val buffer = ByteArray(64 * 1024)
            while (true) {
                val read = input.read(buffer)
                if (read < 0) break
                digest.update(buffer, 0, read)
            }
        }
        return digest.digest().joinToString("") { "%02x".format(it) }
    }

    /**
     * Extract [zip] into [dest], flattening the single wrapper folder the
     * official Vosk archives put everything under (so
     * `vosk-model-small-en-us-0.15/am/…` lands as `am/…`, matching what
     * scripts/fetch-vosk-model.sh stages into assets). An entry that would
     * resolve outside [dest] (zip-slip) aborts the whole extraction.
     */
    fun extractInto(zip: File, dest: File) {
        ZipFile(zip).use { archive ->
            val entries = archive.entries().toList()
            if (entries.isEmpty()) throw IOException("model archive is empty")
            // Flatten only when every entry sits under one shared root folder.
            val roots = entries.mapTo(HashSet()) { it.name.substringBefore('/') }
            val wrapper = roots.singleOrNull()?.takeIf { root -> entries.none { it.name == root } }
            val strip = wrapper?.length?.plus(1) ?: 0
            val destRoot = dest.canonicalFile
            for (entry in entries) {
                if (entry.name.length <= strip) continue // the wrapper folder itself
                val target = File(destRoot, entry.name.substring(strip))
                if (!target.canonicalPath.startsWith(destRoot.canonicalPath + File.separator)) {
                    throw IOException("archive entry escapes the install dir: ${entry.name}")
                }
                if (entry.isDirectory) {
                    if (!target.isDirectory && !target.mkdirs()) {
                        throw IOException("cannot create ${entry.name}")
                    }
                } else {
                    target.parentFile?.let {
                        if (!it.isDirectory && !it.mkdirs()) throw IOException("cannot create ${entry.name}")
                    }
                    archive.getInputStream(entry).use { input ->
                        target.outputStream().use { output -> input.copyTo(output) }
                    }
                }
            }
        }
    }

    /**
     * A directory Vosk can plausibly load: the acoustic model and decoder
     * config every model layout contains. Backs both the post-extract sanity
     * check and [VoskModel.isAvailable]'s "already downloaded?" probe (the
     * same shape scripts/fetch-vosk-model.sh keys its staging check on).
     */
    fun looksLikeModel(dir: File): Boolean =
        File(dir, "am").isDirectory && File(dir, "conf").isDirectory
}
