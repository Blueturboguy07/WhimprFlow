#!/usr/bin/env bash
#
# Puts the speech model a release bundles at
# src-tauri/resources/models/ggml-base.en.bin, checked against a pinned
# SHA-256. scripts/build-macos.sh runs this before `tauri build`, and
# src-tauri/tauri.bundled-model.conf.json copies the file into
# WhimprFlow.app/Contents/Resources/models/.
#
# Why: v0.2.1 shipped a 4.9 MB dmg with no speech model, so a fresh install
# could not dictate at all. The app still has a download button for builds
# that skip this (source builds, `tauri build` run by hand) — see
# src-tauri/src/asr_model.rs, which pins the same URL, size and checksum.
#
# Usage: scripts/fetch-speech-model.sh   (no network use if a good copy exists)
set -euo pipefail

cd "$(dirname "$0")/.."

URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin"
SHA256="a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002"
DEST="src-tauri/resources/models/ggml-base.en.bin"
# The same file the in-app button downloads. Reuse it when it is valid.
USER_COPY="$HOME/Library/Application Support/WhimprFlow/models/ggml-base.en.bin"

# shasum on macOS; sha256sum in Git Bash on the Windows runner.
sha256() { if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1"; else sha256sum "$1"; fi; }
good() { [ -f "$1" ] && [ "$(sha256 "$1" | awk '{print $1}')" = "$SHA256" ]; }

if good "$DEST"; then
  echo "==> Speech model present, checksum matches ($DEST)"
  exit 0
fi

mkdir -p "$(dirname "$DEST")"
rm -f "$DEST" "$DEST.part"

if good "$USER_COPY"; then
  echo "==> Copying the speech model from $USER_COPY"
  cp "$USER_COPY" "$DEST.part"
else
  echo "==> Downloading the speech model (148 MB) from huggingface.co"
  curl -fL --retry 3 --retry-delay 2 -o "$DEST.part" "$URL"
fi

if ! good "$DEST.part"; then
  rm -f "$DEST.part"
  echo "The speech model's checksum does not match $SHA256 — refusing to bundle it." >&2
  exit 1
fi
mv "$DEST.part" "$DEST"
echo "==> Speech model ready ($DEST)"
