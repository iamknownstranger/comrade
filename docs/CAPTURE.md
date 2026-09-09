# Capture — the camera, and the helmet cam

_Added 2026-09-08, from the same kind of owner request that shaped Ride: mount
the phone, press record, keep riding. Extended the same day, from the owner
again: make it a real camera app, and use the Pixel's own camera app where you
can — without losing the screen-off recording._

## 1. What this is

Two things that share one viewfinder.

**A camera app**, shaped like the Pixel's: a live preview the moment you open
the tab, photo and video modes, the zoom stops under the frame, tap to focus,
flash, a self-timer, a grid, and the last shot in the corner. §10 is what that
half is made of. Where the phone has its own camera app — Google Camera on a
Pixel — a photo or an ordinary clip can be handed straight to it instead; §9
is the rule that decides, and the one case where it refuses.

**And a GoPro made out of the phone already on the handlebars.** Pick a lens,
tap record, then press the hardware lock button — recording keeps going with
the screen off, which is the part no other camera app on the device can do.

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
- **A still photo is refused while a recording is running**, which a Pixel
  allows and this does not. Adding the `ImageReader` output to a live
  recording session is the same `createCaptureSession` reconfiguration §2
  already accepts for a screen-lock transition — but triggered by every
  shutter tap rather than by a rare lock event, during the one span where the
  whole point of the feature is that the recording must not be disturbed.
  `CaptureManager.takePhoto` says so and refuses, rather than half-doing it.
  The exit condition is a device with `LOGICAL_MULTI_CAMERA`-class session
  support where the still output can be configured up front for the whole
  recording; nothing here does that yet.
- **A killed process mid-handoff orphans a `MediaStore` row.** While the
  system camera app is in the foreground, Comrade has no visible Activity, so
  it can be killed under memory pressure — and the pending row §9 opened is
  then never resolved either way. Not corrupted and not visible in any
  gallery, just left. `SystemCameraHandoff`'s class doc carries the reason it
  cannot be handed off through `rememberSaveable` instead.
- **No emulator or device verification was possible in the sandbox that
  wrote this feature.** `CaptureScreen.kt`, the `MainActivity.kt` wiring, and
  this document were all written and type-checked
  (`.claude/scripts/android-typecheck-compose.sh`) without an Android SDK,
  an emulator, or a physical camera available to the agent. Everything about
  how the viewfinder actually looks, whether the reconfiguration in §2 is
  fast enough to feel acceptable, and whether any given device's Camera2
  implementation honours the physical-lens characteristics this feature
  depends on is unverified until this runs on a handset. The camera-app half
  adds to that list rather than shortening it: no gesture has reached a
  `TextureView` through Compose interop here, no `ImageReader` has produced a
  JPEG, no zoom ratio has been applied to a real sensor, and no handoff has
  round-tripped through Google Camera.

## 8. Not yet on the other frontends

Per `docs/FRONTEND_STRATEGY.md` §11, Android goes first when a feature cannot
land everywhere at once — and that rule is explicit that landing on Android
alone still has to be said, not assumed. Saying it: **none of this exists on
`desktop/` or in `app/` (Flutter)** — and that now includes the camera-app half
and the handoff in §9 and §10, not only the original helmet cam. The pure
files (`CaptureDecisions`, `CameraApp`) are portable and the rest is not, for
the reasons below.

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

## 9. Your own camera app, and the one thing it cannot do

The owner asked for the phone's default camera app — Google Camera on a Pixel
9 — and for the screen-off recording to survive. Those two are not compatible
in general, and saying so plainly is the design:

**No stock camera app keeps recording once the screen locks.** Not Google
Camera, not any third-party one. There is no intent extra, no flag and no API
that changes it: `ACTION_VIDEO_CAPTURE` hands control to another app's
Activity, and an Activity that is no longer visible does not keep a
`MediaRecorder` running. An earlier pass at this feature read the request as
being about the *output* (`AUDIT.md`, 2026-09-08) and declined the handoff
entirely for that reason. That was too blunt: it is only *rides* that cannot
be delegated. A photo can. An ordinary clip can.

So the ask is split rather than refused, and the split is one pure function:

```kotlin
CameraApp.handoff(mode, keepRecordingWhenLocked, destinationIsPrivate,
                  preferSystemCamera, systemCameraInstalled): Handoff
```

Precedence, first match wins:

1. **Video with "keep recording when the screen is off" on → always in-app**,
   `Fallback.RideMustStayInApp`. This outranks everything, including the
   user's own "use my camera app" switch, because honouring that switch would
   silently delete the feature they are standing in.
2. **A `Private` destination → always in-app**, `Fallback.PrivateStaysInApp`.
   Not a preference: `Private` means a file that was never inserted into
   `MediaStore` (§5), so there is no `Uri` to hand another app as
   `EXTRA_OUTPUT`. Nothing to build, rather than something not built.
3. No camera app installed, then the user's own preference, then the handoff.

**The answer carries the reason it was taken**, which is the part that makes
this honest rather than merely correct. A switch that is silently ignored is
worse than one that is not there; `CaptureScreen` shows the `Fallback` — "your
camera app stops recording when the screen locks, so Comrade is recording this
one" — whenever the user asked for the handoff and did not get it.

`SystemCameraHandoff` is the Android half: `queryIntentActivities` to see
whether a camera app exists, a pending `MediaStore` row from
`CaptureSink.openGalleryTargetForExternalApp` handed over as `EXTRA_OUTPUT`
with `FLAG_GRANT_WRITE_URI_PERMISSION`, and `IS_PENDING` cleared on
`RESULT_OK` or the row deleted on cancel — the same discipline §4 already
describes, with another app doing the writing. The shot it takes is published
through `CaptureManager.publishExternalCapture`, so the tab's "last shot" row
is the same whichever camera took it.

**The `<queries>` element in the manifest is load-bearing**, not boilerplate.
From API 30 an app cannot see another app's intent filters without declaring
what it is looking for, and `queryIntentActivities` answers empty rather than
failing — so dropping it makes the handoff quietly disappear on exactly the
phones it was written for, with nothing red anywhere. §11 is the lane that
now checks for it.

## 10. What the camera-app half is made of

- **The camera opens for the viewfinder, not just for the recording.** The
  original helmet cam opened the `CameraDevice` when Record was pressed, so
  the tab was a black rectangle until then. `CaptureManager` now holds a
  `Camera` (the open device, its session, its outputs and the live request
  state) separately from a `Recording` (the recorder, segments, wake lock and
  guard jobs), and one function — `reconcileLocked` — owns every decision
  about which of them should exist and with which outputs. Starting a
  recording reconfigures the session the preview already opened rather than
  reopening the device.
- **Zoom** is `CONTROL_ZOOM_RATIO` on API 30+ and a computed
  `SCALER_CROP_REGION` below it, with the chips under the frame coming from
  `CameraApp.zoomStops` over the lens's own reported range.
- **Tap to focus** maps the tap through `CameraApp.meteringRect`, which
  rotates by sensor orientation and mirrors for a front lens — an unmirrored
  metering rectangle focuses on the opposite side of the frame, which is the
  classic version of this bug.
- **Flash** is an AE mode in photo and a torch in video; a lens reporting no
  flash unit does not show the control at all.
- **Every one of those survives a session reconfiguration.** The screen going
  off and on rebuilds the capture session (§2), and a zoom level or a torch
  silently reset by that would be a bug reported as "it forgets".

## 11. The lane that guards the parts nothing else can see

`scripts/check_capture_contract.py`, run by CI's `capture-contract` job.

Both of this tab's promises fail *silently*. Drop
`foregroundServiceType="camera|microphone"`, the `WAKE_LOCK` permission or the
`ACTION_SCREEN_ON`/`ACTION_SCREEN_OFF` receiver and the app still compiles,
`./gradlew test` is still green, and the emulator lane still passes — because
none of those lanes record anything. Drop the `<queries>` block and the
handoff disappears with no error anywhere. Both failures arrive on a handset,
an hour into a ride or on the first tap of a switch that does nothing.

So the gate is text-only: no SDK, no Gradle, under a second. It also checks
that the camera-intent action strings appear only as `CameraApp`'s own
constants, because the rule in §9 is enforced in one tested function and a
second file spelling out `"android.media.action.VIDEO_CAPTURE"` would walk
around it without failing a single test.

A second, unrelated lane was added beside it for a mistake this change
actually made: `android-resources` parses every `res/**/*.xml` in both
resource trees. A `--` inside an XML comment in `strings.xml` is illegal XML,
invisible to every Kotlin lane, and failed `packageDebugResources` two
minutes into the Flutter APK job and again in the Android JVM job — two
expensive lanes to find one wrong character. aapt2 is still the authority on
resources; this only asserts they are well-formed at all, which is the class
of mistake that is cheap to make by hand and expensive to find at the end of a
Gradle build.

Neither gate checks anything at runtime, and neither is a substitute for the
device verification §7 still lists as outstanding.
