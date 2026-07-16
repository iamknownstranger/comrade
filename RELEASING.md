# Releasing Comrade

How to go from "sideloaded debug build" to "anyone can install this without
scary warnings" — honestly, including what cannot be avoided.

## 1. One-time: create the release keystore

Run locally (needs a JDK). **This keystore is forever**: every future update
must be signed with the same key, or users have to uninstall/reinstall and
lose their local data. Back it up somewhere safe and offline.

```sh
keytool -genkeypair \
  -keystore comrade-release.jks \
  -alias comrade \
  -keyalg RSA -keysize 4096 -validity 10000 \
  -storepass "<strong store password>" \
  -keypass "<strong key password>" \
  -dname "CN=Comrade, O=<you>, C=IN"
```

Then add four repository secrets (**Settings → Secrets and variables →
Actions**):

| Secret | Value |
|---|---|
| `SIGNING_STORE_B64` | `base64 -w0 comrade-release.jks` |
| `SIGNING_KEY_ALIAS` | `comrade` |
| `SIGNING_KEY_PASSWORD` | the key password |
| `SIGNING_STORE_PASSWORD` | the store password |

> **Not to be confused with `android/debug.keystore`** (committed in this
> repo): that keystore is a separate, deliberately-public keystore used only
> for debug builds, so sideloaded/CI debug APKs share one signature and a
> newer build installs over an older one instead of forcing a reinstall.
> It exists for sideload/testing continuity only. Production signing always
> requires the four `SIGNING_*` secrets above — when they're set, they take
> precedence over the debug keystore for every release build (see the
> `signingConfigs` block in `android/app/build.gradle.kts`).

The **Release APK** workflow picks them up automatically (all four or none —
a partial set fails fast). It produces:

- `comrade-<version>.apk` — release-signed, for sideloading / GitHub
  Releases / Obtainium;
- `comrade-<version>.aab` — the Android App Bundle **Google Play requires**
  for new apps, signed with the same key (used as the Play *upload key*).

## 2. Cutting a release

1. Actions → **Release APK** → *Run workflow*, enter a version (e.g. `1.0.0`).
2. The workflow runs the Rust tests, cross-compiles the JNI libs for
   arm64-v8a / armeabi-v7a / x86_64, builds the signed APK + AAB, and
   publishes a GitHub Release with both attached.
3. `versionCode` is the CI run number (monotonic — required for upgrades);
   `versionName` is the string you entered.

## 3. Distribution options, honestly compared

| Channel | Install friction | What it takes |
|---|---|---|
| **Google Play** | None — no unknown-sources toggle, no Play Protect warning | One-time $25 developer account; upload the `.aab`; complete the **Data safety** form (easy case for Comrade: no data collected, no data shared — everything is on-device or E2E); host a **privacy policy URL** (required); pass review. Play App Signing re-signs with a Google-held key; our keystore becomes the upload key. |
| **GitHub Releases / Obtainium** | Medium — user enables "install unknown apps" for their browser once; Play Protect may show "unknown app" scan prompts | Nothing beyond step 1–2. Obtainium gives users auto-updates from GitHub Releases. |
| **F-Droid** | Medium-low — F-Droid users expect sideloading; reproducible builds earn trust | Submit metadata to fdroiddata; F-Droid builds from source. A good ideological fit for a privacy-first app; slower release cadence. |

## 4. What Play Protect will and won't do

- A **debug-signed** or unsigned build (what CI produces without the
  secrets) gets the harshest treatment: "unsafe app blocked" style prompts.
  Fixing that is exactly step 1.
- A **release-signed but rarely installed** app can still trigger
  "Unknown app — scan before install?" prompts. That's reputation-based and
  fades as installs accumulate; consistent signing is what lets reputation
  attach to the app at all.
- **Only store distribution removes the prompts entirely.** There is no
  legitimate switch that makes sideloaded APKs skip Play Protect — anything
  promising that is malware tooling, and we don't do it.
- Sideloading UX on modern Android: the user must grant "install unknown
  apps" to the app that opens the APK (browser/file manager), per source.
  That's an OS design decision, not something the APK can influence.

## 5. Pre-Play checklist (when ready)

- [ ] Signing secrets configured; a signed `.aab` builds in CI.
- [ ] Privacy policy URL (a page in this repo works) — state plainly: keys
      and journal never leave the device; DMs are E2E over public relays.
- [ ] Data-safety form: no collection, no sharing, data encrypted at rest,
      no account required.
- [ ] Target API level within Play's current requirement (currently
      targetSdk 34 — Play requires targeting within one year of the latest
      release; bump when Play warns).
- [ ] App content declarations (no ads, social features questionnaire).
