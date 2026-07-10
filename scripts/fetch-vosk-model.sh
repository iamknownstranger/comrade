#!/usr/bin/env bash
# Fetch the offline Vosk speech model used by the "Hey Comrade" voice assistant
# and stage it under the Android assets directory the app unpacks at runtime.
#
#   android/app/src/main/assets/model-en-us/{am,conf,graph,ivector,...}
#
# The model is ~40 MB and is intentionally git-ignored (see .gitignore); run
# this once before building an APK that ships voice support. Override MODEL_URL
# to use a larger/other-language model.
set -euo pipefail

MODEL_URL="${MODEL_URL:-https://alphacephei.com/vosk/models/vosk-model-small-en-us-0.15.zip}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ASSETS_DIR="$REPO_ROOT/android/app/src/main/assets"
TARGET_DIR="$ASSETS_DIR/model-en-us"

if [[ -d "$TARGET_DIR" && -d "$TARGET_DIR/am" ]]; then
  echo "Model already present at $TARGET_DIR — nothing to do."
  exit 0
fi

command -v curl >/dev/null || { echo "curl is required" >&2; exit 1; }
command -v unzip >/dev/null || { echo "unzip is required" >&2; exit 1; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $MODEL_URL ..."
curl -fL "$MODEL_URL" -o "$tmp/model.zip"

echo "Unpacking ..."
unzip -q "$tmp/model.zip" -d "$tmp/extracted"

# The zip contains a single top-level folder (e.g. vosk-model-small-en-us-0.15);
# move its *contents* into assets/model-en-us so StorageService.unpack finds am/.
inner="$(find "$tmp/extracted" -maxdepth 1 -mindepth 1 -type d | head -n1)"
if [[ -z "$inner" ]]; then
  echo "Unexpected zip layout: no top-level model directory found" >&2
  exit 1
fi
mkdir -p "$TARGET_DIR"
mv "$inner"/* "$TARGET_DIR"/

echo "Vosk model staged at $TARGET_DIR"
