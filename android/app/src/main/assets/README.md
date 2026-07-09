# Android assets

## `model-en-us/` — offline voice model (not committed)

The "Hey Comrade" voice assistant uses an offline [Vosk](https://alphacephei.com/vosk/)
model. It is ~40 MB and therefore **git-ignored**, not checked in.

Stage it before building an APK with voice support:

```sh
./scripts/fetch-vosk-model.sh
```

This downloads `vosk-model-small-en-us-0.15` and unpacks its contents into
`android/app/src/main/assets/model-en-us/` (so this folder ends up containing
`am/`, `conf/`, `graph/`, `ivector/`, …). At runtime the app unpacks it into
`filesDir/model` via `StorageService.unpack("model-en-us", "model")`.

If the model is absent the app still builds and runs — the voice foreground
service simply reports "Voice model missing" and voice features stay inert.
