package mullu.comrade.ui

import android.content.ComponentCallbacks2
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.util.Base64
import android.util.LruCache
import androidx.core.content.FileProvider
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import mullu.comrade.ComradeCore

/**
 * Decrypts an encrypted NIP-94/96 attachment on demand and caches the
 * plaintext, so re-viewing (or scrolling past and back to) an attachment
 * never re-decrypts or re-downloads it.
 *
 * Images decode to an in-memory [Bitmap] (bounded by *bytes*, not by count —
 * see [bitmapCache]) since that's the common, low-risk case. Audio, video, and
 * generic files need an actual file path/URI for `MediaPlayer`/`VideoView`/
 * `Intent.ACTION_VIEW` to work at all, so those are written to the
 * app-private cache dir (never backed up — see AndroidManifest's
 * `allowBackup=false`) and reused by event id.
 *
 * **One decoded size, shared by the chat bubble ([MediaAttachmentBubble] in
 * `MediaAttachment.kt`) and the full-screen viewer ([MediaViewerDialog] in
 * `MediaViewer.kt`)**, rather than a small thumbnail plus a separate
 * full-resolution decode for the viewer (the shape
 * `MusicLibrary.artwork`, `together/MusicLibrary.kt`, needs, for three
 * genuinely different sizes). The reason is [ComradeCore.downloadMediaTyped]:
 * it is a real HTTPS fetch and AES-GCM decrypt every call, not a cache read
 * (`fetch_and_decrypt_media` in `comrade_ui::runtime`), so a second size would
 * mean a second network round trip just to open a photo already showing in
 * its bubble. [VIEWER_MAX_PIXELS] is picked generous enough to still look
 * sharp at the bubble's 240 dp and at a full screen 1×; pinching past that on
 * the viewer's up-to-6× zoom shows the same interpolation any bounded viewer
 * cache does past its native resolution — the honest trade for not holding a
 * 12-megapixel camera photo's full ARGB_8888 decode (~48 MB) per open
 * attachment.
 *
 * Kept in its own file, importing no `androidx.compose.*`, on purpose:
 * `app/android/app/build.gradle.kts`'s `stagePreservedServices` drops any
 * source that imports Compose when it stages `android/`'s Kotlin into the
 * Flutter build, and `ComradeApplication.onTrimMemory` — which is *not*
 * Compose and *is* staged there — calls straight into this object. Keeping
 * `MediaCache` here rather than beside the composables it backs is what keeps
 * that call resolving in both builds.
 */
internal object MediaCache {
    /** ~4 megapixels: comfortably above a Pixel 9's ~2.6 MP screen (so the
     * common cases — the 240 dp bubble and a full-screen 1× view — are never
     * upscaled), while keeping a single bitmap's worst case (ARGB_8888, no
     * downsampling headroom left) at 4,000,000 × 4 ≈ 15.3 MB rather than the
     * ~48.8 MB a 4032×3024 camera photo decodes to today.
     */
    private const val VIEWER_MAX_PIXELS = 4_000_000L

    /**
     * Bounded by decoded bytes ([Bitmap.getAllocationByteCount]), the only
     * unit that means anything for bitmaps of wildly different resolutions —
     * a count-based cap (the old `BITMAP_CACHE_CAPACITY = 24`) let 24 full
     * 4032×3024 photos retain up to ~1.17 GB, which is most of the ~680 MB a
     * Pixel 9 report attributed to this cache alone.
     *
     * 64 MB holds four images at that worst case, and eight or so of the
     * ordinary ones — a photo shared from a chat is usually already under
     * [VIEWER_MAX_PIXELS], so it decodes at native size and costs a third of
     * the worst case. That is enough for a couple of screens of a photo-heavy
     * thread without the cache growing with how far anyone has scrolled.
     *
     * The budget is set higher than the arithmetic alone would ask for
     * because **a miss here is not a re-decode, it is a network round trip**:
     * [ComradeCore.downloadMediaTyped] re-fetches and re-decrypts every call.
     * Trading a few MB of ceiling against re-downloading a photo the person
     * just scrolled past is the right way round.
     */
    private const val CACHE_BYTES = 64 * 1024 * 1024

    private val bitmapCache = object : LruCache<String, Bitmap>(CACHE_BYTES) {
        override fun sizeOf(key: String, value: Bitmap): Int = value.allocationByteCount
    }
    private val fileMemo = HashMap<String, File>()

    /**
     * React to system memory pressure without waiting for the app to
     * background. [purgeDecryptedMedia] already drops everything on
     * backgrounding, but for a different reason (AUDIT S-4: plaintext must
     * not sit at rest) and only from `MainActivity.onStop`, which a
     * still-foreground app trimmed mid-scroll never calls.
     *
     * `TRIM_MEMORY_RUNNING_CRITICAL` (15) and above drops the whole LRU —
     * that threshold also covers every backgrounded level (`UI_HIDDEN` 20
     * through `COMPLETE` 80; Android numbers both families on one scale, so
     * this one comparison catches both). Anything milder just halves what is
     * retained: a bitmap already downsampled to [VIEWER_MAX_PIXELS] is cheap
     * to re-decode, so there is no reason to hold on to more than the system
     * is asking for.
     */
    fun onTrimMemory(level: Int) {
        when {
            level >= ComponentCallbacks2.TRIM_MEMORY_RUNNING_CRITICAL -> bitmapCache.evictAll()
            level >= ComponentCallbacks2.TRIM_MEMORY_RUNNING_LOW ->
                bitmapCache.trimToSize(bitmapCache.size() / 2)
        }
    }

    private fun extensionFor(mime: String): String = when (mime) {
        "image/jpeg" -> "jpg"
        "image/png" -> "png"
        "image/webp" -> "webp"
        "image/gif" -> "gif"
        "audio/mpeg" -> "mp3"
        "audio/ogg", "audio/oga" -> "ogg"
        "audio/wav" -> "wav"
        "audio/aac" -> "aac"
        "video/mp4" -> "mp4"
        "application/pdf" -> "pdf"
        else -> "bin"
    }

    suspend fun decodeImage(info: ComradeCore.MediaMessageInfo): Bitmap = withContext(Dispatchers.IO) {
        bitmapCache.get(info.eventId)?.let { return@withContext it }
        val bytes = ComradeCore.downloadMediaTyped(info.eventId)
        val raw = Base64.decode(bytes.base64, Base64.NO_WRAP)
        // Bounds first, then the real decode at a sample size that keeps this
        // under VIEWER_MAX_PIXELS — the shape AttachmentPreview.decodePreview
        // established; the arithmetic itself is BitmapBudget.sampleSizeFor.
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(raw, 0, raw.size, bounds)
        val options = BitmapFactory.Options().apply {
            inSampleSize = BitmapBudget.sampleSizeFor(bounds.outWidth, bounds.outHeight, VIEWER_MAX_PIXELS)
            // ARGB_8888 for everything, including the JPEGs that have no alpha
            // to lose. RGB_565 would halve every one of those, and it was
            // written that way first — but this bitmap is what the full-screen
            // viewer draws and pinch-zooms, and 16-bit colour bands visibly
            // across a sky or any other gradient. The sample size above and the
            // byte budget below are what fixed the memory problem (~48.8 MB to
            // ~15.3 MB worst case, and a 1.17 GB ceiling to 64 MB); halving an
            // already-bounded cache again is not worth making every photo in
            // the app look worse. Revisit only if a profiler on a real device
            // shows this cache still dominating.
            inPreferredConfig = Bitmap.Config.ARGB_8888
        }
        val bitmap = BitmapFactory.decodeByteArray(raw, 0, raw.size, options)
            ?: error("Could not decode image")
        bitmapCache.put(info.eventId, bitmap)
        bitmap
    }

    suspend fun resolveFile(context: Context, info: ComradeCore.MediaMessageInfo): File =
        withContext(Dispatchers.IO) {
            synchronized(fileMemo) { fileMemo[info.eventId] }
                ?.let { if (it.exists()) return@withContext it }
            val dir = File(context.cacheDir, "media").apply { mkdirs() }
            val file = File(dir, "${info.eventId}.${extensionFor(info.mimeType)}")
            if (!file.exists()) {
                val bytes = ComradeCore.downloadMediaTyped(info.eventId)
                file.writeBytes(Base64.decode(bytes.base64, Base64.NO_WRAP))
            }
            synchronized(fileMemo) { fileMemo[info.eventId] = file }
            file
        }

    fun uriFor(context: Context, file: File) =
        FileProvider.getUriForFile(context, "${context.packageName}.fileprovider", file)

    /**
     * Drop every decrypted plaintext this cache is holding: the in-memory image
     * LRU and each file written under `cacheDir/media`. Anything still needed is
     * transparently re-decrypted on next view, so this is safe to call any time
     * the app should not be sitting on plaintext (backgrounded / vault locked).
     */
    fun clear(context: Context) {
        bitmapCache.evictAll()
        synchronized(fileMemo) { fileMemo.clear() }
        val dir = File(context.cacheDir, "media")
        dir.listFiles()?.forEach { it.delete() }
    }
}

/**
 * Wipe all decrypted media the app has cached to `cacheDir/media` (and the
 * in-memory bitmap LRU). Called when the app is backgrounded or the vault is
 * locked so plaintext attachments — including received voice notes — never
 * outlive a foreground session on disk (AUDIT S-4).
 */
internal fun purgeDecryptedMedia(context: Context) = MediaCache.clear(context)
