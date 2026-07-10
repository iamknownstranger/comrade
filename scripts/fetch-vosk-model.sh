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

# Integrity pin for the default model zip (AUDIT M1-7). To (re)pin: run once,
# copy the sha256 the script prints, verify it against a second source, and
# set it here. While empty, the script warns loudly instead of verifying.
# TODO(M1-7): pin — could not be computed from the audit environment (host
# blocked by egress policy); run this script from a networked machine.
PINNED_SHA256=""
# A caller-supplied MODEL_SHA256 always wins (required when overriding MODEL_URL).
EXPECTED_SHA256="${MODEL_SHA256:-$PINNED_SHA256}"
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

actual_sha256="$(sha256sum "$tmp/model.zip" | cut -d' ' -f1)"
if [[ -n "$EXPECTED_SHA256" ]]; then
  if [[ "$actual_sha256" != "$EXPECTED_SHA256" ]]; then
    echo "ERROR: model checksum mismatch — refusing to stage it." >&2
    echo "  expected: $EXPECTED_SHA256" >&2
    echo "  actual:   $actual_sha256" >&2
    exit 1
  fi
  echo "Checksum OK ($actual_sha256)"
else
  echo "WARNING: download NOT verified — no checksum is pinned." >&2
  echo "  sha256: $actual_sha256" >&2
  echo "  Verify this against a trusted source, then set PINNED_SHA256 in" >&2
  echo "  scripts/fetch-vosk-model.sh (or pass MODEL_SHA256=...)." >&2
fi

echo "Unpacking ..."
unzip -q "$tmp/model.zip" -d "$tmp/extracted"

# The zip contains a single top-level folder (e.g. vosk-model-small-en-us-0.15);
# move its *contents* into assets/model-en-us so StorageService.unpack finds am/.
inner="$(find "$tmp/extracted" -maxdepth 1 -mindepth 1 -type d | head -n1)"
mkdir -p "$TARGET_DIR"
mv "$inner"/* "$TARGET_DIR"/

echo "Vosk model staged at $TARGET_DIR"
