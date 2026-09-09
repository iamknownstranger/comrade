package mullu.comrade.capture

import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.provider.MediaStore
import java.util.TimeZone

/**
 * The Android half of [CameraApp.handoff]: turns a `Route.SystemCamera`
 * decision into an actual `Intent` another app can capture into, and turns
 * that app's result back into something [CaptureManager] can publish.
 *
 * Deliberately no Compose import — this is staged into the Flutter build the
 * same way every other non-Compose file under this package is
 * (`app/android/app/build.gradle.kts`'s `stagePreservedServices`, see
 * `CLAUDE.md`'s traps), and it has no UI of its own to stage.
 *
 * ## Gallery-only, and why that is not a limitation worth working around
 *
 * A [CaptureDecisions.Destination.Private] shot has no shareable `Uri` by
 * construction (`CaptureSink`'s own class doc, `docs/CAPTURE.md` §5) — it is
 * never inserted into `MediaStore`, which is the entire point of choosing it.
 * There is therefore nothing to hand another app as `EXTRA_OUTPUT` for a
 * private destination, and [CameraApp.handoff] already refuses the handoff
 * before any of this runs (`Fallback.PrivateStaysInApp`). This file has no
 * private-storage branch to fall back to and should never grow one — a
 * `FileProvider` Uri would be shareable, which is exactly what `Private` is
 * choosing not to be.
 *
 * ## What survives a killed process, and what does not
 *
 * [CaptureSink.openGalleryTargetForExternalApp] returns a live
 * [CaptureSink.Still] — an object holding a `ContentResolver` and the
 * pending row's `Uri`, not a `Uri` alone — because [finish] needs it to
 * clear `IS_PENDING` or delete the row exactly the way every other
 * `CaptureSink` target does. That object cannot be written into a `Bundle`
 * (it holds a resolver, not primitives), so the caller cannot carry it
 * through `rememberSaveable` and it does not survive the process being
 * killed while the system camera app is in the foreground — a real
 * possibility under memory pressure, since Comrade briefly has no visible
 * Activity of its own for the length of that handoff. When that happens the
 * `IS_PENDING` row this opened is orphaned: not corrupted, not visible in
 * any gallery, just never resolved either way. This is the same shape of
 * cost `docs/CAPTURE.md` §2 already accepts for the screen-off preview
 * reconfiguration — stated here rather than silently risked.
 */
object SystemCameraHandoff {

    /**
     * What [prepare] hands back: the [Intent] to launch (already carrying
     * `EXTRA_OUTPUT` and the write-grant flag) and the open [CaptureSink.Still]
     * [finish] needs once that activity returns.
     */
    data class Prepared(
        val intent: Intent,
        val still: CaptureSink.Still,
        val displayName: String,
        val isVideo: Boolean,
    )

    /**
     * Whether *some* app on this device answers [action] at all.
     *
     * Package visibility (API 30+) hides every other app's manifest from
     * `queryIntentActivities` unless this app declares it needs to see them —
     * the `<queries>` element in `AndroidManifest.xml`, right next to a
     * comment pointing back here. Without it this silently returns an empty
     * list on every device new enough to enforce package visibility,
     * `CameraApp.handoff` reports `Fallback.NoSystemCameraInstalled`
     * regardless of what is actually installed, and the "hand a shot to the
     * phone's own camera app" feature disappears without a single exception
     * being thrown anywhere.
     */
    fun installedFor(context: Context, action: String): Boolean {
        val intent = Intent(action)
        val results = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            context.packageManager.queryIntentActivities(intent, PackageManager.ResolveInfoFlags.of(0))
        } else {
            @Suppress("DEPRECATION") // the ResolveInfoFlags overload is API 33+; this app's minSdk is 26
            context.packageManager.queryIntentActivities(intent, 0)
        }
        return results.isNotEmpty()
    }

    /**
     * Opens a gallery target for [action]
     * ([CameraApp.ACTION_IMAGE_CAPTURE]/[CameraApp.ACTION_VIDEO_CAPTURE]) and
     * builds the [Intent] a system camera app needs to write straight into
     * it, named the same way an in-app shot would be
     * ([CaptureDecisions.baseName]/[CaptureDecisions.photoBaseName]) so it
     * sorts and groups with every other clip in the gallery.
     *
     * `null` means [CaptureSink.openGalleryTargetForExternalApp] itself
     * refused — a full disk, a refused `MediaStore` insert, or (below API 29)
     * no scoped-storage grant model to hand another app a `Uri` through at
     * all. The caller's answer is the same as
     * `Fallback.NoSystemCameraInstalled`: stay in the app.
     */
    fun prepare(context: Context, action: String, isVideo: Boolean): Prepared? {
        val startedAtEpochMs = System.currentTimeMillis()
        val utcOffsetMinutes = TimeZone.getDefault().getOffset(startedAtEpochMs) / 60_000
        val displayName = if (isVideo) {
            CaptureDecisions.segmentFileName(
                CaptureDecisions.baseName(startedAtEpochMs, utcOffsetMinutes),
                segment = 0,
            )
        } else {
            CaptureDecisions.photoFileName(
                CaptureDecisions.photoBaseName(startedAtEpochMs, utcOffsetMinutes),
            )
        }
        val still = CaptureSink.openGalleryTargetForExternalApp(context, displayName, isVideo) ?: return null
        val uri = still.uri ?: return null // gallery-only by construction; see the class doc.
        val intent = Intent(action)
            .putExtra(MediaStore.EXTRA_OUTPUT, uri)
            .addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
        return Prepared(intent, still, displayName, isVideo)
    }

    /**
     * Resolves [prepared] once the system camera activity returns. `ok` true
     * clears `IS_PENDING` so the shot actually shows up in the gallery;
     * `false` deletes the row outright — the system camera never wrote to it
     * (the user backed out, or the activity crashed), so nothing worth
     * keeping was ever there. Either way this calls [CaptureSink.Still.finish]
     * exactly once, the same one-shot contract every other `CaptureSink`
     * target keeps.
     *
     * `null` means there is nothing to publish: [ok] was `false`, or
     * `finish` itself could not confirm the write (the row vanished from
     * under it, `MediaStore` refused the update). The caller should not treat
     * `null` as an error to show — a cancelled handoff is not a failure, and
     * a genuine failure here has nothing more specific to say than "nothing
     * was saved".
     */
    fun finish(prepared: Prepared, ok: Boolean): CaptureManager.Saved? {
        val uri = prepared.still.finish(ok) ?: return null
        return CaptureManager.Saved(
            uri = uri.toString(),
            displayName = prepared.displayName,
            destination = CaptureDecisions.Destination.Gallery,
            kind = if (prepared.isVideo) CaptureManager.Kind.Video else CaptureManager.Kind.Photo,
        )
    }
}
