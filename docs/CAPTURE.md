# Capture — the helmet cam

_Added 2026-09-08, from the same kind of owner request that shaped Ride: mount
the phone, press record, keep riding._

## 1. What this is

A GoPro made out of the phone already on the handlebars. Open the recorder,
pick a lens, tap record, then press the hardware lock button — recording
keeps going with the screen off, which is the entire feature.

The screen is what a helmet mount cannot get rid of: it is what heats a
phone with no airflow and what drains its battery over a two-hour ride, and
the recorder's own controls under a helmet visor are the last thing anyone
wants to be tapping at speed. Locking the phone solves both at once — the
display goes dark, the buttons stop responding to a stray glove — provided
recording survives it. Before this feature, it did not: the stock camera app
and every third-party one stops recording the moment the screen locks,
because none of them were built to keep a `MediaRecorder` alive through it.

Every physical camera on the device is selectable — ultrawide, main, tele,
front, an external USB lens if one is attached — because a helmet mount is
usually the back camera but is sometimes the front one (chin-mounted,
watching the rider rather than the road), and there is no way to know which
in advance.

## 2. Why the screen going off is the design centre

[`CaptureDecisions.previewAttached`](../android/app/src/main/java/mullu/comrade/capture/CaptureDecisions.kt)
is one function and it is the whole feature:

```kotlin
fun previewAttached(screenOn: Boolean, uiVisible: Boolean): Boolean = screenOn && uiVisible
```

When it returns `false`, `CaptureManager.reconfigureOutputs` reconfigures the
live `CameraCaptureSession` down to the encoder surface alone. No preview
surface means no frames for the display or the GPU compositing them to do
anything with — that is what makes recording survive a screen-off mount
instead of cooking it.

**Be precise about what this costs.** Camera2 cannot add or remove an output
from a *live* capture session — there is no API for it — so attaching or
detaching the preview is a fresh `createCaptureSession` call on the same open
`CameraDevice`. The `MediaRecorder` is never stopped across that
reconfiguration — the file keeps growing, and its timestamps absorb the
gap — but frames genuinely stop arriving for the length of the swap. **A few
frames are lost at every screen on/off transition.** That is a real cost of
the design, not a bug to file, and nothing in the code or this document
should describe it as seamless. What it buys in exchange is that the
recording itself never stops, and the file that comes out the other end is
one continuous (if not perfectly contiguous) clip rather than a new file
every time the phone locks.

**And a reconfiguration can fail, not just cost frames.** The old session is
never closed before its replacement is confirmed — Camera2 replaces a session
when a new one is created, so closing up front bought nothing and risked
everything: an earlier version of `reconfigureOutputs` did exactly that, and a
failed `createCaptureSession` left the recording pointing at a session it had
already closed, writing no video at all while the timer and the notification
carried on. On a helmet mount that is not discovered until you get home. The
answer now has three steps, in order: try the output set that was asked for;
if that fails and it included the preview, retry with the encoder alone, since
the viewfinder is the part the recording does not need; and if even that will
not configure, stop the recording deliberately, publish what was shot, and say
so on screen as `Reason.StoppedCameraLost`. A recorder that has silently
stopped recording must never look like one that is still going.

`CaptureManager.setUiVisible` is what `CaptureScreen` calls from its own
`ON_START`/`ON_STOP` lifecycle observer; `CaptureService` calls the screen-lock
half of the same input directly from `ACTION_SCREEN_ON`/`ACTION_SCREEN_OFF`,
because the *screen* going dark and the *recorder screen* leaving composition
are two different signals and either one alone is reason enough to drop the
preview.

## 3. Why Camera2, not CameraX

Two reasons, in order of how much they mattered:

- **Physical-lens enumeration.** `CameraManager.getCameraIdList()` walks
  every physical camera the device reports — ultrawide, main, tele, front, an
  external USB lens — which is exactly the "every camera this phone has"
  requirement in §1. `androidx.camera.core.CameraSelector` is built around
  "the" front camera and "the" back camera; getting at a second back lens
  needs the same Camera2 characteristics CameraX would otherwise have hidden.
- **A new dependency would have to be added twice.** `CameraX` is not
  currently a dependency of `android/`, and per `CLAUDE.md`'s traps, adding
  one here means adding it to *two* Gradle files —
  `android/app/build.gradle.kts` and `app/android/app/build.gradle.kts`,
  because `stagePreservedServices` recompiles every non-Compose `.kt` file
  under this package into the Flutter Android app — or the Flutter Android
  APK lane breaks on an unresolved reference nobody who reads the diff would
  recognise as a missing dependency. Camera2 and `MediaRecorder` are both
  framework classes; they cost neither Gradle file anything.

## 4. The gallery contract

A finished recording's default destination is `DCIM/Camera` — the stock
camera app's own folder — under its own `VID_yyyyMMdd_HHmmss` naming
convention (`CaptureDecisions.baseName`, `GALLERY_RELATIVE_PATH`). On API 29+
this is a `MediaStore.Video.Media` row with that `RELATIVE_PATH`, written
`IS_PENDING` until the file is complete, exactly the discipline
`mullu.comrade.together.MusicDownloads` already uses for the same reason: a
half-written file must never appear finished to anything reading the
gallery early.

**Comrade cannot make Google Photos, or any other gallery app, upload
anything.** There is no upload code here and no Photos API call. What this
folder buys is narrower and it is the whole point: `DCIM/Camera` is the
folder Photos' *default* backup configuration already watches, because it is
where the stock camera app has always written. A recording that lands there
looks, to Photos, identical to a clip shot with the stock app — so whatever
backup behaviour the user already has configured (or none) applies to it
exactly as it would to any other video in that folder. Turn off Photos'
backup, or exclude `DCIM/Camera` from it, and Comrade's footage is excluded
too, the same as the stock app's would be.

## 5. The private option

The `Private` destination is not a MediaStore row at all —
`Environment.DIRECTORY_MOVIES` under `context.getExternalFilesDir()`
(`CaptureSink.openPrivate`), which is app-scoped external storage. Nothing
indexes it: not the system gallery, not Photos, not any other app that reads
MediaStore, because MediaStore was never told the file exists.

**Say plainly that this is not encryption.** The bytes on disk are exactly
as readable as any other file this app writes — the recording is invisible
to Photos because nothing announced it, not because anything is protecting
it. It goes away entirely if Comrade is uninstalled: app-scoped external
storage is removed with the app that owns it, same as internal storage
would be. A rider choosing `Private` is choosing "not on my public camera
roll," not "encrypted."

## 6. Segment rollover

A running recording rolls to a new file at `MAX_SEGMENT_BYTES` (3.5 GB) or
`MAX_SEGMENT_MS` (30 minutes), whichever comes first
(`CaptureDecisions.shouldRoll`). Both numbers exist for the same reason:

- **The 4 GiB ceiling.** An MP4's `stco` box stores 32-bit chunk offsets, and
  FAT32 (still the format of a great many phones' external storage, and the
  lowest common denominator either way) caps a single file at 2³²−1 bytes.
  3.5 GB leaves headroom for the last write before the size check runs again,
  rather than rolling exactly at the wire and racing a write past the limit.
- **A crash costs at most one segment.** 30 minutes bounds the other
  failure mode — a battery pull, a process kill, anything that ends the
  recording *without* going through `CaptureManager.stop` — to losing the
  segment in progress, not the whole ride. `segmentFileName` numbers segments
  so they sort in recording order in the gallery next to the un-suffixed
  first one.

## 7. Known limitations

- **Lens switching is idle-only.** Camera2 cannot change which physical
  camera a session is reading from without closing the file being written to
  it, so `CaptureManager.switchLens` is a no-op while a recording is running
  and `CaptureScreen` disables the lens button for the same span. Switching
  lenses mid-ride means stopping, switching, and starting a new file.
- **No emulator or device verification was possible in the sandbox that
  wrote this feature.** `CaptureScreen.kt`, the `MainActivity.kt` wiring, and
  this document were all written and type-checked
  (`.claude/scripts/android-typecheck-compose.sh`) without an Android SDK,
  an emulator, or a physical camera available to the agent. Everything about
  how the viewfinder actually looks, whether the reconfiguration in §2 is
  fast enough to feel acceptable, and whether any given device's Camera2
  implementation honours the physical-lens characteristics this feature
  depends on is unverified until this runs on a handset.

## 8. Not yet on the other frontends

Per `docs/FRONTEND_STRATEGY.md` §11, Android goes first when a feature cannot
land everywhere at once — and that rule is explicit that landing on Android
alone still has to be said, not assumed. Saying it: **none of this exists on
`desktop/` or in `app/` (Flutter).**

- **`app/` (Flutter)** would need its own Camera2-equivalent capture path —
  `camera`/`camerax` plugins do not expose physical-lens enumeration any more
  than CameraX does on Android proper, so the same §3 argument applies there
  even harder, and the "screen off keeps recording" behaviour would need a
  foreground-service-equivalent (a platform channel into a real Android
  foreground service, since Flutter has no service of its own) rather than
  anything portable through `flutter_rust_bridge`. This is not core logic
  behind an FFI boundary — it is a platform capability — so there is no frb
  regeneration step that would carry it across the way there is for, say,
  Together's control plane.
- **`desktop/`** has no camera-mount use case in the first place: a laptop or
  desktop machine is not something anyone straps to a helmet, and Tauri's
  webview has no exposure to Camera2 or `MediaRecorder` regardless. If a
  future desktop feature ever wanted a screen-recording-style capture, it
  would start from a different set of platform APIs entirely and would not
  share code with this one beyond `CaptureDecisions`' file-naming and
  rollover arithmetic, which is pure enough to port if anyone asked for it.
