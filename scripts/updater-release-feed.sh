#!/usr/bin/env bash
# Writes the update feed (latest.json) of one WhimprFlow release from its update files, and
# attaches it to the release. publik serves this file byte for byte as
# https://publikhq.com/api/updates/whimprflow/latest.json once it approves the release
# (docs/updates.md). The urls in it are the absolute GitHub asset URLs of the release.
#
# Usage:
#   scripts/updater-release-feed.sh <tag> [--if-complete] [--version <v>]
#       Reads the files from the GitHub release <tag>, writes latest.json and uploads it
#       (it replaces an older latest.json on that release; nothing else is touched).
#   scripts/updater-release-feed.sh <tag> --dir <dir> [--version <v>]
#       Reads the files from <dir> and writes <dir>/latest.json. No network, no upload.
#
# The files, for version <v> (the tag without its "v", unless --version is given):
#   WhimprFlow_<v>_aarch64.app.tar.gz + .sig   (scripts/build-macos.sh, notarized build)
#   WhimprFlow_<v>_x64-setup.exe + .sig        (windows.yml / publik-build.yml, with the key)
# Both platforms are required. --if-complete: when one is missing, say so and exit 0 (the
# release workflows call it after each half lands; the second half writes the feed).
# scripts/updater-latest-json.mjs checks every signature against the key in tauri.conf.json
# and the signed version before anything is written.
# Env: GITHUB_REPOSITORY (default Blueturboguy07/WhimprFlow), GH_TOKEN for gh, NOTES (text).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TAG="${1:?usage: updater-release-feed.sh <tag> [--dir <dir>] [--if-complete] [--version <v>]}"
shift
DIR=""; IF_COMPLETE=0; VERSION="${TAG#v}"
while [ $# -gt 0 ]; do
  case "$1" in
    --dir) DIR="${2:?--dir needs a folder}"; shift 2 ;;
    --if-complete) IF_COMPLETE=1; shift ;;
    --version) VERSION="${2:?--version needs a value}"; shift 2 ;;
    *) echo "updater-release-feed: unknown argument $1" >&2; exit 2 ;;
  esac
done
REPO="${GITHUB_REPOSITORY:-Blueturboguy07/WhimprFlow}"
MAC="WhimprFlow_${VERSION}_aarch64.app.tar.gz"
WIN="WhimprFlow_${VERSION}_x64-setup.exe"
WANT=("$MAC" "$MAC.sig" "$WIN" "$WIN.sig")

missing() {
  if [ "$IF_COMPLETE" = 1 ]; then
    echo "updater-release-feed: $1. No latest.json yet; the other half of the release writes it." >&2
    exit 0
  fi
  echo "updater-release-feed: $1" >&2
  exit 1
}

UPLOAD=0
if [ -z "$DIR" ]; then
  UPLOAD=1
  DIR="$(mktemp -d)"
  trap 'rm -rf "$DIR"' EXIT
  ASSETS="$(gh release view "$TAG" -R "$REPO" --json assets -q '.assets[].name')"
  for f in "${WANT[@]}"; do
    printf '%s\n' "$ASSETS" | grep -qxF "$f" || missing "release $TAG has no $f"
  done
  for f in "${WANT[@]}"; do gh release download "$TAG" -R "$REPO" --pattern "$f" --dir "$DIR"; done
  if [ -z "${NOTES:-}" ]; then NOTES="$(gh release view "$TAG" -R "$REPO" --json body -q .body || true)"; fi
else
  for f in "${WANT[@]}"; do [ -f "$DIR/$f" ] || missing "$DIR has no $f"; done
fi

NOTES_FILE="$(mktemp)"
printf '%s\n' "${NOTES:-WhimprFlow $VERSION}" > "$NOTES_FILE"
node "$ROOT/scripts/updater-latest-json.mjs" --version "$VERSION" \
  --base-url "https://github.com/$REPO/releases/download/$TAG/" \
  --platform "darwin-aarch64=$DIR/$MAC" \
  --platform "windows-x86_64=$DIR/$WIN" \
  --notes-file "$NOTES_FILE" --out "$DIR/latest.json"
rm -f "$NOTES_FILE"

if [ "$UPLOAD" = 1 ]; then
  # latest.json is made from the other files, so a newer copy may replace it.
  gh release upload "$TAG" -R "$REPO" "$DIR/latest.json" --clobber
  echo "Attached latest.json to $TAG"
else
  echo "Wrote $DIR/latest.json"
fi
