#!/usr/bin/env bash
# Pack a FINAL macOS app (Developer ID signed, notarized, stapled) as the Tauri updater bundle,
# and sign that bundle with the update key. Run it after the app is stapled, never before: the
# updater installs exactly these bytes, so they carry the code signature (and the TCC identity)
# that users get. Nothing opens a window. Copy it to another Tauri app as is.
#
# Usage: scripts/updater-pack-macos.sh <App.app> <out dir>
# Out:   <out>/<Name>_<version>_<arch>.app.tar.gz and the same name + .sig
#        (the version is bound into the signature, for requireSignedVersion).
# Key:   TAURI_SIGNING_PRIVATE_KEY and TAURI_SIGNING_PRIVATE_KEY_PASSWORD from the environment;
#        when unset, read from the login keychain item UPDATE_KEY_SERVICE (default
#        "publik-update-key", accounts mann-private and mann-password). Never printed.
# CLI:   TAURI_CLI, else node_modules/.bin/tauri or ui/node_modules/.bin/tauri in the repo.
set -euo pipefail
APP="${1:?App.app}"
OUT="${2:?out dir}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[ -d "$APP/Contents/MacOS" ] || { echo "Not an app bundle: $APP" >&2; exit 1; }

TAURI="${TAURI_CLI:-}"
for c in "$ROOT/node_modules/.bin/tauri" "$ROOT/ui/node_modules/.bin/tauri"; do
  [ -z "$TAURI" ] && [ -x "$c" ] && TAURI="$c"
done
[ -n "$TAURI" ] || { echo "No Tauri CLI: set TAURI_CLI or install the node packages." >&2; exit 1; }

SERVICE="${UPDATE_KEY_SERVICE:-publik-update-key}"
if [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
  TAURI_SIGNING_PRIVATE_KEY="$(security find-generic-password -s "$SERVICE" -a "${UPDATE_KEY_ACCOUNT:-mann-private}" -w 2>/dev/null || true)"
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$(security find-generic-password -s "$SERVICE" -a "${UPDATE_KEY_PASSWORD_ACCOUNT:-mann-password}" -w 2>/dev/null || true)"
fi
[ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ] || { echo "No update key: set TAURI_SIGNING_PRIVATE_KEY or add the keychain item $SERVICE." >&2; exit 1; }
export TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"

PLIST="$APP/Contents/Info.plist"
NAME="$(basename "$APP" .app)"
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$PLIST")"
BIN="$APP/Contents/MacOS/$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$PLIST")"
case "$(lipo -archs "$BIN")" in
  arm64) ARCH=aarch64 ;;
  x86_64) ARCH=x64 ;;
  *) ARCH=universal ;;
esac
# A broken seal here would be shipped to every user: check it first.
codesign --verify --strict --deep "$APP"

mkdir -p "$OUT"
TARBALL="$OUT/${NAME}_${VERSION}_${ARCH}.app.tar.gz"
rm -f "$TARBALL" "$TARBALL.sig"
# The updater drops the first path part and unpacks the rest as the new bundle. No AppleDouble
# (._*) files and no extended attributes: an extra file inside the bundle breaks its seal.
COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs -czf "$TARBALL" -C "$(dirname "$APP")" "$(basename "$APP")"
if tar -tzf "$TARBALL" | grep -q '/\._'; then echo "AppleDouble files in $TARBALL" >&2; exit 1; fi

"$TAURI" signer sign --app-version "$VERSION" "$TARBALL" >/dev/null
[ -s "$TARBALL.sig" ] || { echo "No signature was written for $TARBALL" >&2; exit 1; }
echo "Update bundle: $TARBALL"
echo "Signature:     $TARBALL.sig (version $VERSION)"
