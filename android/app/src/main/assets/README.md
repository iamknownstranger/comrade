# Android assets

## `model-en-us/` — offline voice model (not committed)

The "Hey Comrade" voice assistant uses an offline [Vosk](https://alphacephei.com/vosk/)
model. It is ~40 MB and therefore **git-ignored**, not checked in.

Baking it into the APK is *optional*. To stage it before building:

```sh
./scripts/fetch-vosk-model.sh
```

This downloads `vosk-model-small-en-us-0.15` (sha256-pinned), unpacks its
contents into `android/app/src/main/assets/model-en-us/` (so this folder ends
up containing `am/`, `conf/`, `graph/`, `ivector/`, …) and writes the `uuid`
marker file `StorageService` requires — without that marker the runtime
unpack fails with `FileNotFoundException: model-en-us/uuid` even when the
model files are present. At runtime the app unpacks the bundled model into
the app's external files dir via `StorageService.unpack("model-en-us", "model")`.

If the model is **not** baked in (CI APKs, for example), the app still builds
and runs — the first tap on a voice feature (tap-to-talk, "Hey Comrade",
journal dictation) offers a one-time in-app download instead, verified
against the same pinned sha256 and installed under `filesDir/voice-model`
(see `VoiceModelDownloader`/`VoskModel`).
