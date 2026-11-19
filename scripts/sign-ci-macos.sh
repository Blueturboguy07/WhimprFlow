#!/usr/bin/env bash
# Sign, notarize and package the unsigned WhimprFlow.app that publik-build.yml makes (artifact
# whimprflow-macos-arm64-unsigned). The same steps as scripts/build-macos.sh from "copy the
# worker" on, for an app that CI built. Nothing opens a window: codesign, notarytool, stapler and
# hdiutil only, so it is safe on a Mac someone is using. scripts/publik-release.sh calls it.
#
# Usage: scripts/sign-ci-macos.sh <WhimprFlow-macos-arm64-unsigned.app.zip> <out dir>
# Env:   NOTARY_PROFILE (notarytool keychain profile, default "publik"),
#        APPLE_SIGNING_IDENTITY (default: the Developer ID below),
#        TAURI_SIGNING_PRIVATE_KEY(+_PASSWORD) or the "publik-update-key" keychain item, and
#        TAURI_CLI (scripts/updater-pack-macos.sh).
# Out:   <out>/WhimprFlow_<version>_aarch64.dmg (signed, notarized, stapled; the app inside too),
#        <out>/WhimprFlow_<version>_aarch64.app.tar.gz + .sig (the same stapled app, for the
#        updater; docs/updates.md).
set -euo pipefail
ZIP="${1:?unsigned app zip}"
OUT="${2:?out dir}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IDENTITY="${APPLE_SIGNING_IDENTITY:-Developer ID Application: Mann Bellani (R5R3ZS54LV)}"
PROFILE="${NOTARY_PROFILE:-publik}"
ENTITLEMENTS="$ROOT/src-tauri/Entitlements.plist"
case "$IDENTITY" in "Developer ID Application:"*) ;; *) echo "Refusing: '$IDENTITY' is not a Developer ID Application identity." >&2; exit 1 ;; esac
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
WORK="$(mktemp -d)"
trap 'hdiutil detach "$WORK/mnt" -quiet 2>/dev/null || true; rm -rf "$WORK"' EXIT

notarize() {
  local file="$1" log="$WORK/notary.log" id
  xcrun notarytool submit "$file" --keychain-profile "$PROFILE" --wait 2>&1 | tee "$log"
  if ! grep -q "status: Accepted" "$log"; then
    id="$(sed -n 's/^ *id: //p' "$log" | head -1)"
    [ -z "$id" ] || xcrun notarytool log "$id" --keychain-profile "$PROFILE" >&2 || true
    echo "Notarization of $(basename "$file") was not accepted." >&2
    exit 1
  fi
}

ditto -x -k "$ZIP" "$WORK/x"
APP="$WORK/x/WhimprFlow.app"
[ -d "$APP" ] || { echo "No WhimprFlow.app in $ZIP" >&2; exit 1; }
[ "$(ls -A "$WORK/x")" = "WhimprFlow.app" ] || { echo "$ZIP holds more than WhimprFlow.app" >&2; exit 1; }
PLIST="$APP/Contents/Info.plist"
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$PLIST")"
MAIN="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$PLIST")"
echo "==> WhimprFlow $VERSION: signing with $IDENTITY"

# Nested bundles would need their own signing order. WhimprFlow has none; refuse if one appears.
NESTED="$(find "$APP/Contents" -mindepth 1 \( -name '*.framework' -o -name '*.app' -o -name '*.xpc' -o -name '*.appex' -o -name '*.bundle' \) -print)"
[ -z "$NESTED" ] || { echo "Nested bundles in the app; this script signs plain Mach-O files only:" >&2; echo "$NESTED" >&2; exit 1; }

# Inner code first: every Mach-O file other than the main executable (the local-LLM worker in
# Contents/MacOS, any dylib), deepest first. Hardened runtime and a secure timestamp, no
# entitlements: the same as scripts/build-macos.sh signs the worker.
find "$APP/Contents" -type f ! -path "$APP/Contents/MacOS/$MAIN" -print0 |
  while IFS= read -r -d '' f; do
    if file -b "$f" | grep -q "Mach-O"; then printf '%s\t%s\n' "$(printf '%s' "$f" | tr -cd '/' | wc -c)" "$f"; fi
  done | sort -rn | cut -f2- > "$WORK/inner.txt"
while IFS= read -r f; do
  echo "    inner code: ${f#"$APP/"}"
  codesign --force --options runtime --timestamp --sign "$IDENTITY" "$f"
done < "$WORK/inner.txt"
grep -q "/Contents/MacOS/whimpr-llm-worker$" "$WORK/inner.txt" || { echo "No whimpr-llm-worker in the app: the CI build is incomplete." >&2; exit 1; }
# Then the app: hardened runtime, the repo's entitlements (the microphone entitlement that
# dictation needs under the hardened runtime), a timestamp. This also signs the main executable.
codesign --force --options runtime --timestamp --entitlements "$ENTITLEMENTS" --sign "$IDENTITY" "$APP"
codesign --verify --strict --deep --verbose=2 "$APP"
codesign -d --entitlements - --xml "$APP" 2>/dev/null | grep -q "com.apple.security.device.audio-input" ||
  { echo "The signed app has no microphone entitlement." >&2; exit 1; }

echo "==> Notarizing the app"
ditto -c -k --keepParent "$APP" "$WORK/WhimprFlow.zip"
notarize "$WORK/WhimprFlow.zip"
rm -f "$WORK/WhimprFlow.zip"
xcrun stapler staple "$APP"

# The update bundle is the stapled app itself, so an update keeps the Developer ID identity (and
# with it the microphone and Accessibility grants).
echo "==> Update bundle"
"$ROOT/scripts/updater-pack-macos.sh" "$APP" "$OUT"

echo "==> Building the dmg"
# The folder holds only the app; the Applications link is added next to it (no second copy).
ln -s /Applications "$WORK/x/Applications"
DMG="$OUT/WhimprFlow_${VERSION}_aarch64.dmg"
rm -f "$DMG"
hdiutil create -volname "WhimprFlow" -srcfolder "$WORK/x" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$WORK/x"
codesign --force --sign "$IDENTITY" --timestamp "$DMG"
echo "==> Notarizing the dmg"
notarize "$DMG"
xcrun stapler staple "$DMG"

echo "==> Checks (no launch)"
xcrun stapler validate "$DMG"
spctl -a -vv -t open --context context:primary-signature "$DMG"
mkdir -p "$WORK/mnt"
hdiutil attach -nobrowse -noautoopen -readonly -mountpoint "$WORK/mnt" "$DMG" >/dev/null
xcrun stapler validate "$WORK/mnt/WhimprFlow.app"
spctl -a -vv -t exec "$WORK/mnt/WhimprFlow.app"
codesign -dvv "$WORK/mnt/WhimprFlow.app" 2>&1 | grep -E "Authority=Developer ID Application|flags=|TeamIdentifier"
hdiutil detach "$WORK/mnt" -quiet
shasum -a 256 "$DMG"
echo "Dmg: $DMG"
