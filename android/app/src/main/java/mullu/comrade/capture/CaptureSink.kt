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
}
