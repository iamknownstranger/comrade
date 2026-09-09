package mullu.comrade.capture

import android.content.ContentResolver
import android.content.ContentValues
import android.content.Context
import android.media.MediaRecorder
import android.media.MediaScannerConnection
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.os.ParcelFileDescriptor
import android.provider.MediaStore
import java.io.File

/**
 * Where a finished (or finishing) recording is written, and what makes it
 * indistinguishable from the stock camera app's.
 *
 * ## What "destination" actually buys you
 *
 * Comrade cannot make Google Photos upload anything — that is a setting the
 * user controls entirely outside this app. What it *can* do is write to the
 * folder Photos' default backup already watches ([CaptureDecisions.GALLERY_RELATIVE_PATH],
 * `DCIM/Camera`), under the stock camera app's own `VID_yyyyMMdd_HHmmss`
 * naming ([CaptureDecisions.baseName]), so a gallery-destination recording is
 * picked up by whatever backup the user already configured — the same way
 * every other clip in that folder is.
 *
 * [CaptureDecisions.Destination.Private] is the opt-out, and it is **not
 * encryption**. The file lives in this app's own external-storage directory
 * ([Context.getExternalFilesDir]), never inserted into [MediaStore], so it is
 * not in the shared collection and therefore not visible to Photos or any
 * other gallery app — and it goes away the moment the app is uninstalled,
 * exactly like every other file under that directory. Say this plainly to
 * whoever reads this file next: "private" means *not shared*, not *protected*.
 *
 * ## `IS_PENDING`, again
 *
 * Same discipline as [mullu.comrade.together.MusicDownloads]: a MediaStore row
 * is created pending and only cleared once [MediaRecorder] has actually
 * finished writing it, and a failed segment deletes its own row rather than
 * leaving a broken, zero-byte clip sitting in the user's gallery with the
 * right name and nothing playable behind it.
 *
 * ## Stills
 *
 * [Still] is [Segment]'s photo counterpart, for the Pixel-Camera-shaped half
 * of Capture ([CameraApp]) — same destinations, same naming convention
 * ([CaptureDecisions.photoBaseName]/[CaptureDecisions.photoFileName]), same
 * `IS_PENDING` discipline, one write instead of a `MediaRecorder` stream.
 * [openGalleryTargetForExternalApp] is the one exception to "this object only
 * ever writes what this process captured": it is a target handed to the
 * *system* camera app so a one-off shot taken there lands in the same place
 * and under the same naming as one taken in-process.
 */
object CaptureSink {

    /**
     * One opened segment target. [setAsOutput] wires it into a fresh
     * [MediaRecorder] before `prepare()`; [setAsNextOutput] arms it as the
     * *next* file on a recorder that is already running, for a rollover that
     * does not drop a frame (`MediaRecorder.setNextOutputFile`, API 26+).
     * [finish] is always called exactly once per segment, and never both ways
     * at once: `true` once `MediaRecorder` has actually stopped writing this
     * segment cleanly, `false` to abandon it (recorder never wrote it, or the
     * recording failed) — for a MediaStore target that deletes the pending
     * row instead of leaving it stuck; for a plain file it deletes the file.
     */
    sealed interface Segment {
        val displayName: String
        fun setAsOutput(recorder: MediaRecorder)
        fun setAsNextOutput(recorder: MediaRecorder)

        /** The published [Uri], or `null` for a private segment (nothing is
         *  published) or a segment that failed to finish. */
        fun finish(success: Boolean): Uri?
    }

    /**
     * Open the next segment named [displayName] (already carries its `.mp4`
     * suffix — see [CaptureDecisions.segmentFileName]) for [destination].
     *
     * `null` means the target could not be created at all: a refused
     * MediaStore insert, a full disk, storage this process cannot write to.
     * For the *first* segment of a recording the caller treats that as reason
     * enough to fail the whole thing; for the pre-armed *next* segment mid-ride
     * it is reason enough to keep going on the current file instead and try
     * arming again shortly (`CaptureManager.armNextSegment`) — the guard loop
     * is what actually ends the recording if this is a genuine "disk is full".
     */
    fun open(context: Context, destination: CaptureDecisions.Destination, displayName: String): Segment? =
        when (destination) {
            CaptureDecisions.Destination.Private -> openPrivate(context, displayName)
            CaptureDecisions.Destination.Gallery ->
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    openGalleryMediaStore(context, displayName)
                } else {
                    openGalleryLegacy(context, displayName)
                }
        }

    /**
     * API 29+: a MediaStore row under [CaptureDecisions.GALLERY_RELATIVE_PATH],
     * written straight into the [ParcelFileDescriptor] MediaStore hands back —
     * no `WRITE_EXTERNAL_STORAGE` needed, an app may always insert its own
     * media on scoped storage.
     */
    private fun openGalleryMediaStore(context: Context, displayName: String): Segment? {
        val resolver = context.contentResolver
        val values = ContentValues().apply {
            put(MediaStore.Video.Media.DISPLAY_NAME, displayName)
            put(MediaStore.Video.Media.MIME_TYPE, CaptureDecisions.MIME_TYPE)
            put(MediaStore.Video.Media.RELATIVE_PATH, CaptureDecisions.GALLERY_RELATIVE_PATH)
            put(MediaStore.Video.Media.DATE_TAKEN, System.currentTimeMillis())
            // Hidden from every scanner (and every other gallery app) until
            // the bytes are all there — see the class doc.
            put(MediaStore.Video.Media.IS_PENDING, 1)
        }
        val uri = runCatching { resolver.insert(MediaStore.Video.Media.EXTERNAL_CONTENT_URI, values) }
            .getOrNull() ?: return null
        val pfd = runCatching { resolver.openFileDescriptor(uri, "w") }.getOrNull()
        if (pfd == null) {
            // The row exists but we cannot write it — do not leave a pending
            // entry nobody will ever finish.
            runCatching { resolver.delete(uri, null, null) }
            return null
        }
        return MediaStoreSegment(resolver, uri, pfd, displayName)
    }

    /**
     * API 26–28: scoped storage does not exist yet, so this is a plain file
     * under the public `DCIM/Camera` directory (`WRITE_EXTERNAL_STORAGE`,
     * capped at `maxSdkVersion="28"` in the manifest, is only for this path),
     * scanned in afterwards so it actually shows up rather than sitting there
     * invisible to every gallery app until the next full media scan.
     */
    private fun openGalleryLegacy(context: Context, displayName: String): Segment? {
        @Suppress("DEPRECATION") // the scoped-storage replacement is API 29+; this path is pre-29 only
        val dcim = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DCIM)
        val dir = File(dcim, "Camera")
        if (!dir.exists() && !dir.mkdirs()) return null
        return FileSegment(File(dir, displayName), context.applicationContext, publish = true, displayName)
    }

    /**
     * Off the gallery path entirely: an app-external directory under
     * `Movies/`[CaptureDecisions.PRIVATE_SUBDIR], never inserted into
     * MediaStore. Not indexed by anything on API 29+ by construction, but the
     * legacy media scanner on 26–28 sweeps the *whole* external volume
     * including app-specific directories — a `.nomedia` file is the one
     * documented way to opt a directory out of that, which is the entire
     * promise [CaptureDecisions.Destination.Private] makes on those releases.
     */
    private fun openPrivate(context: Context, displayName: String): Segment? {
        val movies = context.getExternalFilesDir(Environment.DIRECTORY_MOVIES) ?: return null
        val dir = File(movies, CaptureDecisions.PRIVATE_SUBDIR)
        if (!dir.exists() && !dir.mkdirs()) return null
        val nomedia = File(dir, ".nomedia")
        if (!nomedia.exists()) runCatching { nomedia.createNewFile() }
        return FileSegment(File(dir, displayName), context.applicationContext, publish = false, displayName)
    }

    private class MediaStoreSegment(
        private val resolver: ContentResolver,
        private val uri: Uri,
        private val pfd: ParcelFileDescriptor,
        override val displayName: String,
    ) : Segment {
        override fun setAsOutput(recorder: MediaRecorder) {
            recorder.setOutputFile(pfd.fileDescriptor)
        }

        override fun setAsNextOutput(recorder: MediaRecorder) {
            recorder.setNextOutputFile(pfd.fileDescriptor)
        }

        override fun finish(success: Boolean): Uri? {
            runCatching { pfd.close() }
            if (!success) {
                runCatching { resolver.delete(uri, null, null) }
                return null
            }
            val done = ContentValues().apply { put(MediaStore.Video.Media.IS_PENDING, 0) }
            val published = runCatching { resolver.update(uri, done, null, null) }.isSuccess
            if (!published) {
                runCatching { resolver.delete(uri, null, null) }
                return null
            }
            return uri
        }
    }

    private class FileSegment(
        private val file: File,
        private val context: Context,
        /** Scan it in and hand back a `file://` [Uri] once finished — the
         *  legacy-gallery case. `false` for [CaptureDecisions.Destination.Private]:
         *  neither scanning nor a returned Uri would honour "not visible
         *  anywhere", so [finish] answers `null` for a private segment even
         *  on success. */
        private val publish: Boolean,
        override val displayName: String,
    ) : Segment {
        override fun setAsOutput(recorder: MediaRecorder) {
            recorder.setOutputFile(file.absolutePath)
        }

        override fun setAsNextOutput(recorder: MediaRecorder) {
            recorder.setNextOutputFile(file)
        }

        override fun finish(success: Boolean): Uri? {
            if (!success) {
                file.delete()
                return null
            }
            if (!publish) return null
            MediaScannerConnection.scanFile(context, arrayOf(file.absolutePath), arrayOf(CaptureDecisions.MIME_TYPE), null)
            return Uri.fromFile(file)
        }
    }

    // ── Stills ───────────────────────────────────────────────────────────

    /**
     * The photo counterpart of [Segment]. A still is one write, not a stream
     * a `MediaRecorder` owns for minutes at a time, so [write] takes the
     * whole JPEG at once rather than being handed a `Surface`/file descriptor
     * up front — by the time [CaptureManager] has bytes to give this, the
     * `ImageReader` callback that produced them has already run on
     * [CaptureManager]'s own camera thread, so there is nothing an
     * incremental API here would buy.
     *
     * Same `IS_PENDING`-until-written discipline as [Segment], and the same
     * rule about never publishing a broken file: [write] returning `false` —
     * a full disk, a closed resolver — is [finish]'s cue to delete the
     * row/file rather than leave a zero-byte JPEG with the right name sitting
     * in the gallery.
     */
    sealed interface Still {
        val displayName: String

        /** The target the bytes go to. Non-null for a gallery (MediaStore)
         *  target, which is also what can be handed to another app as
         *  `EXTRA_OUTPUT` — see [openGalleryTargetForExternalApp]. `null` for
         *  a private target: there is nothing to share, which is the entire
         *  point of choosing it (`docs/CAPTURE.md` §5). */
        val uri: Uri?

        /** Write the whole JPEG in one call. `false` — an empty [bytes], a
         *  write that threw — means [finish] must not publish anything. */
        fun write(bytes: ByteArray): Boolean

        /** Always called exactly once: `true` once [write] has actually
         *  produced a complete file (clears `IS_PENDING`/scans the file in);
         *  `false` to abandon it, deleting whatever [write] already wrote. */
        fun finish(success: Boolean): Uri?
    }

    /**
     * Open a target for a still named [displayName] (already carries its
     * `.jpg` suffix — see [CaptureDecisions.photoFileName]) at [destination].
     * `null` means the target could not even be created — a refused
     * MediaStore insert, storage this process cannot write to — and unlike a
     * mid-ride video segment there is no "current file" to fall back to
     * keeping open: a failed photo is just a failed photo.
     */
    fun openStill(context: Context, destination: CaptureDecisions.Destination, displayName: String): Still? =
        when (destination) {
            CaptureDecisions.Destination.Private -> openPrivateStill(context, displayName)
            CaptureDecisions.Destination.Gallery ->
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    openGalleryStillMediaStore(context, displayName)
                } else {
                    openGalleryStillLegacy(context, displayName)
                }
        }

    /**
     * A gallery target another app may be granted write access to via
     * `Intent.EXTRA_OUTPUT` — MediaStore only; a private destination has no
     * shareable [Uri], which is the whole point of it. This is what
     * [CaptureManager.publishExternalCapture]'s system-camera handoff hands
     * the other app to write into — the same pending-row contract
     * [openGalleryStillMediaStore] uses for a shot this process takes itself;
     * the only difference is who does the writing, so [write] is never
     * actually called for that path even though the interface still offers
     * it.
     *
     * **The other app's result code is not evidence a file exists.** All the
     * caller can observe is that the camera app's Activity returned
     * `RESULT_OK`, which says nothing about whether it wrote into the Uri it
     * was granted. [MediaStoreStill.finish] checks the row's own length
     * before publishing it for that reason; do not remove that check on the
     * grounds that the result code already covers it.
     *
     * `null` before API 29: granting a MediaStore row's `Uri` to another app
     * needs scoped storage's insert-then-grant model, and the legacy
     * public-directory path a pre-29 [Segment]/[Still] otherwise uses needs
     * this app itself to hold `WRITE_EXTERNAL_STORAGE` — which does not
     * extend to a Uri handed to a different app. A caller on that old a
     * release has to fall back to a `FileProvider` Uri of its own; that is
     * outside this object.
     */
    fun openGalleryTargetForExternalApp(
        context: Context,
        displayName: String,
        isVideo: Boolean,
    ): Still? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            if (isVideo) openGalleryVideoMediaStore(context, displayName) else openGalleryStillMediaStore(context, displayName)
        } else {
            null
        }

    private fun openGalleryStillMediaStore(context: Context, displayName: String): Still? {
        val resolver = context.contentResolver
        val values = ContentValues().apply {
            put(MediaStore.Images.Media.DISPLAY_NAME, displayName)
            put(MediaStore.Images.Media.MIME_TYPE, CaptureDecisions.PHOTO_MIME_TYPE)
            put(MediaStore.Images.Media.RELATIVE_PATH, CaptureDecisions.GALLERY_RELATIVE_PATH)
            put(MediaStore.Images.Media.DATE_TAKEN, System.currentTimeMillis())
            put(MediaStore.Images.Media.IS_PENDING, 1)
        }
        val uri = runCatching { resolver.insert(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, values) }
            .getOrNull() ?: return null
        return MediaStoreStill(resolver, uri, displayName)
    }

    /** Only reached from [openGalleryTargetForExternalApp] — nothing in this
     *  object records an in-process video Segment through the [Still] shape,
     *  only ever the video the *system* camera app is about to write via the
     *  granted [Uri]. */
    private fun openGalleryVideoMediaStore(context: Context, displayName: String): Still? {
        val resolver = context.contentResolver
        val values = ContentValues().apply {
            put(MediaStore.Video.Media.DISPLAY_NAME, displayName)
            put(MediaStore.Video.Media.MIME_TYPE, CaptureDecisions.MIME_TYPE)
            put(MediaStore.Video.Media.RELATIVE_PATH, CaptureDecisions.GALLERY_RELATIVE_PATH)
            put(MediaStore.Video.Media.DATE_TAKEN, System.currentTimeMillis())
            put(MediaStore.Video.Media.IS_PENDING, 1)
        }
        val uri = runCatching { resolver.insert(MediaStore.Video.Media.EXTERNAL_CONTENT_URI, values) }
            .getOrNull() ?: return null
        return MediaStoreStill(resolver, uri, displayName)
    }

    private fun openGalleryStillLegacy(context: Context, displayName: String): Still? {
        @Suppress("DEPRECATION") // the scoped-storage replacement is API 29+; this path is pre-29 only
        val dcim = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DCIM)
        val dir = File(dcim, "Camera")
        if (!dir.exists() && !dir.mkdirs()) return null
        return FileStill(File(dir, displayName), context.applicationContext, publish = true, displayName)
    }

    /** Beside the private videos ([openPrivate]), under `Pictures/` instead of
     *  `Movies/` — same [CaptureDecisions.PRIVATE_SUBDIR], same `.nomedia`
     *  opt-out for the legacy media scanner on API 26–28. */
    private fun openPrivateStill(context: Context, displayName: String): Still? {
        val pictures = context.getExternalFilesDir(Environment.DIRECTORY_PICTURES) ?: return null
        val dir = File(pictures, CaptureDecisions.PRIVATE_SUBDIR)
        if (!dir.exists() && !dir.mkdirs()) return null
        val nomedia = File(dir, ".nomedia")
        if (!nomedia.exists()) runCatching { nomedia.createNewFile() }
        return FileStill(File(dir, displayName), context.applicationContext, publish = false, displayName)
    }

    /**
     * Deliberately holds no [ParcelFileDescriptor] open across calls: [write]
     * opens the resolver's output stream fresh and closes it again, so a
     * [Still] handed to another app via [openGalleryTargetForExternalApp] —
     * where this process never calls [write] at all — never leaves a
     * descriptor of its own competing with the other app's writer for the
     * same row.
     */
    private class MediaStoreStill(
        private val resolver: ContentResolver,
        override val uri: Uri,
        override val displayName: String,
    ) : Still {
        override fun write(bytes: ByteArray): Boolean {
            if (bytes.isEmpty()) return false
            return runCatching {
                val out = resolver.openOutputStream(uri) ?: return false
                out.use { it.write(bytes) }
            }.isSuccess
        }

        /**
         * `success` is the caller's *intent* to publish, and for the
         * system-camera handoff it is only ever `resultCode == RESULT_OK` —
         * which says the other app's Activity returned, not that it wrote
         * anything into the Uri it was granted. So the row's own length is
         * checked here before `IS_PENDING` is cleared, and this is not
         * belt-and-braces: a camera app that returns OK without completing
         * the save (interrupted by a call, killed under memory pressure,
         * simply buggy) would otherwise publish a zero-byte file under a
         * real-looking name — exactly the half-written-clip-in-the-gallery
         * failure this object's whole pending discipline exists to prevent.
         *
         * A length that cannot be read at all is treated as written rather
         * than as empty: the shot probably happened, and deleting somebody's
         * photo because a `statSize` query failed is the worse error of the
         * two.
         */
        override fun finish(success: Boolean): Uri? {
            if (!success || !hasBytes()) {
                runCatching { resolver.delete(uri, null, null) }
                return null
            }
            val done = ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) }
            val published = runCatching { resolver.update(uri, done, null, null) }.isSuccess
            if (!published) {
                runCatching { resolver.delete(uri, null, null) }
                return null
            }
            return uri
        }

        private fun hasBytes(): Boolean = runCatching {
            resolver.openFileDescriptor(uri, "r")?.use { it.statSize > 0L }
        }.getOrNull() ?: true
    }

    private class FileStill(
        private val file: File,
        private val context: Context,
        /** Same meaning as [FileSegment.publish]: scan it in and hand back a
         *  `file://` Uri for the legacy gallery case; `false`, and always
         *  `null` from [finish], for [CaptureDecisions.Destination.Private]. */
        private val publish: Boolean,
        override val displayName: String,
    ) : Still {
        override val uri: Uri? = null // a legacy file:// path is not a content Uri another app can be granted

        override fun write(bytes: ByteArray): Boolean {
            if (bytes.isEmpty()) return false
            return runCatching { file.outputStream().use { it.write(bytes) } }.isSuccess
        }

        override fun finish(success: Boolean): Uri? {
            if (!success) {
                file.delete()
                return null
            }
            if (!publish) return null
            MediaScannerConnection.scanFile(context, arrayOf(file.absolutePath), arrayOf(CaptureDecisions.PHOTO_MIME_TYPE), null)
            return Uri.fromFile(file)
        }
    }
}
