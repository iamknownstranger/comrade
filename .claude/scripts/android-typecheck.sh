#!/usr/bin/env bash
#
# Type-check the non-Compose half of android/ with no Android SDK.
#
# ## What this is for
#
# `android/` is the priority frontend and the one this sandbox cannot build, so
# CI has been the first thing to compile it — a round trip per typo. It turns
# out only *Compose* actually needs the SDK's build system. Everything else
# type-checks against a real android.jar, the real generated uniffi bindings and
# the real AARs, all of which are downloadable.
#
# Coverage as of 2026-08-08: **84 of 87** Kotlin sources under
# `android/app/src/main/java/mullu/comrade`. The three left out are the ones
# that import `androidx.compose` (MainActivity, call/CallScreen, call/CallWidgets),
# plus the Compose screens in `ui/` — the Compose compiler plugin and its AAR
# set are the part that is genuinely not worth reproducing here.
#
# This caught nothing on its first run, because it was written *after* CI caught
# `refreshLive(durationMs = …)` on a function with no such parameter. That is
# exactly the class of error it exists to catch, and exactly the class the
# framework-free JUnit lane cannot see, because `TogetherManager` imports
# Android.
#
# ## What it is not
#
# Not a build, not a test run, and not a lint. It answers one question — does
# this Kotlin resolve and typecheck — which is the question CI was being asked
# to answer at several minutes a go. `./gradlew test` still needs the SDK, and
# `res/` correctness (a string id that does not exist, a bad manifest) is still
# CI's to find: R is *generated* here from the resource files, so a reference to
# a missing string resolves against a stub that has every real name in it, and
# nothing else.
#
# Usage:  .claude/scripts/android-typecheck.sh
# Cache:  ~/.cache/comrade-android-typecheck (delete to re-download)
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CACHE="${ANDROID_TYPECHECK_CACHE:-$HOME/.cache/comrade-android-typecheck}"
SRC="$REPO/android/app/src/main/java/mullu/comrade"
OUT="$CACHE/out"

# Pinned to android/build.gradle.kts's Kotlin plugin. A mismatch here is not
# cosmetic: the metadata version of the stdlib it links has to match, which is
# the same trap that rules out androidyoutubeplayer 13.x (see build.gradle.kts).
KOTLIN_VERSION="1.9.22"
# Robolectric's android-all is a complete android.jar for API 34 — the SDK's own
# is not redistributable and `sdkmanager` is absent, so this is the only way to
# get the platform classes. compileSdk is 34; keep them together.
ANDROID_ALL="14-robolectric-10818077"

mkdir -p "$CACHE/jars"
fetch() { # url dest
  [ -f "$2" ] || { echo "  fetching $(basename "$2")"; curl -sSL -o "$2" "$1"; }
}
aar_classes() { # url dest-jar
  local tmp="$CACHE/tmp-aar"
  [ -f "$2" ] && return 0
  echo "  fetching $(basename "$2")"
  rm -rf "$tmp"; mkdir -p "$tmp"
  curl -sSL -o "$tmp/a.aar" "$1"
  unzip -o -q "$tmp/a.aar" classes.jar -d "$tmp"
  mv "$tmp/classes.jar" "$2"
  rm -rf "$tmp"
}

echo "== toolchain =="
if [ ! -x "$CACHE/kotlinc/bin/kotlinc" ]; then
  fetch "https://github.com/JetBrains/kotlin/releases/download/v$KOTLIN_VERSION/kotlin-compiler-$KOTLIN_VERSION.zip" "$CACHE/kc.zip"
  unzip -q -o "$CACHE/kc.zip" -d "$CACHE"
fi

echo "== dependencies =="
MC=https://repo1.maven.org/maven2
GM=https://dl.google.com/dl/android/maven2
J="$CACHE/jars"
fetch "$MC/org/robolectric/android-all/$ANDROID_ALL/android-all-$ANDROID_ALL.jar" "$J/android-all.jar"
fetch "$MC/org/jetbrains/kotlinx/kotlinx-coroutines-core-jvm/1.8.1/kotlinx-coroutines-core-jvm-1.8.1.jar" "$J/coroutines.jar"
fetch "$MC/net/java/dev/jna/jna/5.17.0/jna-5.17.0.jar" "$J/jna.jar"
fetch "$MC/junit/junit/4.13.2/junit-4.13.2.jar" "$J/junit.jar"
fetch "$GM/androidx/lifecycle/lifecycle-common/2.6.2/lifecycle-common-2.6.2.jar" "$J/lifecycle-common.jar"
fetch "$GM/androidx/annotation/annotation/1.7.0/annotation-1.7.0.jar" "$J/annotation.jar"
aar_classes "$GM/androidx/core/core/1.12.0/core-1.12.0.aar" "$J/core.jar"
aar_classes "$GM/androidx/core/core-ktx/1.12.0/core-ktx-1.12.0.aar" "$J/core-ktx.jar"
aar_classes "$GM/androidx/lifecycle/lifecycle-runtime/2.6.2/lifecycle-runtime-2.6.2.aar" "$J/lifecycle-runtime.jar"
aar_classes "$GM/androidx/activity/activity/1.8.2/activity-1.8.2.aar" "$J/activity.jar"
aar_classes "$GM/androidx/versionedparcelable/versionedparcelable/1.1.1/versionedparcelable-1.1.1.aar" "$J/versionedparcelable.jar"
aar_classes "$MC/io/github/webrtc-sdk/android/125.6422.07/android-125.6422.07.aar" "$J/webrtc.jar"
aar_classes "$MC/com/alphacephei/vosk-android/0.3.47/vosk-android-0.3.47.aar" "$J/vosk.jar"
aar_classes "$MC/com/pierfrancescosoffritti/androidyoutubeplayer/core/12.1.2/core-12.1.2.aar" "$J/youtubeplayer.jar"

# The generated uniffi bindings, from the real cdylib rather than a stub — this
# is what makes an FFI signature change show up here instead of in CI. Same
# thing Gradle's generateUniffiBindings does, and it needs only cargo.
echo "== uniffi bindings =="
(cd "$REPO" && cargo build -q -p comrade_jni)
rm -rf "$CACHE/uniffi"; mkdir -p "$CACHE/uniffi"
(cd "$REPO" && cargo run -q -p comrade_uniffi_bindgen -- generate \
  --library "$REPO/target/debug/libcomrade_jni.so" \
  --language kotlin --out-dir "$CACHE/uniffi")

# R and BuildConfig, which AGP generates and kotlinc has never heard of. R is
# built from the real resource files, so every id that exists resolves and every
# id that does not still fails — which is the useful half of what AGP's R gives.
echo "== R / BuildConfig stubs =="
mkdir -p "$CACHE/stub"
python3 - "$REPO" "$CACHE/stub/Generated.kt" <<'PY'
import glob, os, re, sys
repo, dest = sys.argv[1], sys.argv[2]
res = os.path.join(repo, 'android/app/src/main/res')
buckets = {}
for f in glob.glob(f'{res}/values*/*.xml'):
    txt = open(f, encoding='utf-8').read()
    for tag, kind in [('string', 'string'), ('color', 'color'), ('dimen', 'dimen'),
                      ('integer', 'integer'), ('bool', 'bool'), ('style', 'style'),
                      ('string-array', 'array'), ('plurals', 'plurals')]:
        for m in re.finditer(r'<%s\s+name="([^"]+)"' % tag, txt):
            buckets.setdefault(kind, set()).add(m.group(1))
for d in glob.glob(f'{res}/*'):
    kind = os.path.basename(d).split('-')[0]
    if kind == 'values':
        continue
    for f in glob.glob(f'{d}/*'):
        buckets.setdefault(kind, set()).add(os.path.basename(f).split('.')[0])
ident = re.compile(r'^[a-zA-Z_][a-zA-Z0-9_]*$')
out = ['package mullu.comrade', '',
       '// GENERATED by .claude/scripts/android-typecheck.sh — never committed.', '',
       'object R {']
for kind in sorted(buckets):
    out.append(f'    object {kind} {{')
    for n in sorted(buckets[kind]):
        safe = n.replace('.', '_')   # AGP flattens dotted style names
        if ident.match(safe):
            out.append(f'        const val {safe}: Int = 0')
    out.append('    }')
out += ['}', '', 'object BuildConfig {',
        '    const val DEBUG: Boolean = false',
        '    const val APPLICATION_ID: String = "mullu.comrade"',
        '    const val VERSION_NAME: String = "0.0.0"',
        '    const val VERSION_CODE: Int = 0',
        '    const val DEFAULT_TURN_URL: String = ""',
        '    const val DEFAULT_TURN_USERNAME: String = ""',
        '    const val DEFAULT_TURN_PASSWORD: String = ""',
        '}', '',
        '// Compose, so excluded from the compile — but its *type* is referenced by',
        '// the notification Intents that reopen the app, so it needs to exist.',
        'class MainActivity : android.app.Activity()', '']
open(dest, 'w').write('\n'.join(out) + '\n')
PY

echo "== compiling =="
# Everything except the files that import androidx.compose. Discovered rather
# than listed, so a new Compose file drops out and a new plain one is picked up
# without editing this script.
#
# The pattern is anchored to an actual import line, and must stay that way: it
# mirrors `composeImport` in app/android/app/build.gradle.kts, which decides
# which sources `stagePreservedServices` copies into the Flutter build. An
# unanchored grep was here first and matched the words "androidx.compose"
# inside a KDoc comment, which dropped a plain file out of this lane while
# Gradle still staged it — so the lane reported an unresolved reference against
# a class that compiles perfectly well in both real builds. A lane that is
# stricter than the rule it stands in for reports bugs that do not exist.
FILES=()
while IFS= read -r f; do
  grep -qE "^[[:space:]]*import[[:space:]]+androidx\.compose" "$f" || FILES+=("$f")
done < <(find "$SRC" -name '*.kt' | sort)
while IFS= read -r f; do FILES+=("$f"); done < <(find "$CACHE/uniffi" -name '*.kt')
FILES+=("$CACHE/stub/Generated.kt")

CP="$(find "$J" -name '*.jar' | tr '\n' ':')"
rm -rf "$OUT"
echo "  ${#FILES[@]} files"
"$CACHE/kotlinc/bin/kotlinc" -J-Xmx8g -nowarn "${FILES[@]}" -cp "$CP" -d "$OUT"
echo "== clean =="
