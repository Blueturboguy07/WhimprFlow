#!/usr/bin/env bash
#
# Builds WhimprFlow for macOS the way it has to be built to be *installable* by
# someone who is not the person who built it.
#
# v0.1.0 shipped signed with "Apple Development: mannbellani@icloud.com" — a
# development certificate. It passed every local check and then refused to open
# on the first user's Mac ("cannot be opened because it is from an unidentified
# developer"), because a development certificate is not a distribution one.
# That is the bug this script exists to make impossible to repeat: the identity
# comes from the environment, and the script refuses to produce a release build
# with anything other than a Developer ID.
#
# Two notarization passes, on purpose:
#   1. Tauri notarizes and staples the .app when APPLE_ID / APPLE_PASSWORD /
#      APPLE_TEAM_ID are set.
#   2. Tauri then wraps that app in a dmg which it signs but never submits. An
#      unstapled dmg has to be checked against Apple over the network, so it
#      fails for a user who is offline or behind a filter — and the failure
#      reads as a corrupt download. So the dmg is submitted and stapled here,
#      separately, after the bundler finishes.
#
# Credentials, either way:
#   NOTARY_PROFILE=<name>   a profile stored by `xcrun notarytool
#                           store-credentials`, so no secret is ever passed in
#                           the environment. Preferred.
#   APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID
#                           an app-specific password. Required for Tauri's own
#                           pass, which cannot read a keychain profile.
#
# The publik app token: `PUBLIK_APP_TOKEN` must be in the environment for a
# release build. It is read at compile time (`option_env!` in
# src-tauri/src/publik.rs) and is what lets a downloaded copy mint its own
# publik API key after the user accepts the disclosure. It is an app
# identifier with abuse limits, not a secret — but a release without it ships
# a Settings card that says "not available in this build", which is a silent
# regression of the whole point of the publik option. Never echo its value.
#
# Usage: scripts/build-macos.sh [--target <triple>] [--skip-notarize]
set -euo pipefail

cd "$(dirname "$0")/.."
REPO_ROOT="$PWD"
TARGET=""
SKIP_NOTARIZE=0
DEFAULT_IDENTITY="Developer ID Application: Mann Bellani (R5R3ZS54LV)"

while [ $# -gt 0 ]; do
  case "$1" in
    --target) TARGET="$2"; shift 2 ;;
    --skip-notarize) SKIP_NOTARIZE=1; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

IDENTITY="${APPLE_SIGNING_IDENTITY:-$DEFAULT_IDENTITY}"

# A development certificate signs cleanly and is still refused by Gatekeeper.
# Catching that here costs a second; catching it in a bug report costs a user.
case "$IDENTITY" in
  "Developer ID Application:"*) ;;
  *)
    echo "Refusing to build: '$IDENTITY' is not a Developer ID Application identity." >&2
    echo "That is exactly what shipped in v0.1.0 and could not be opened." >&2
    exit 1
    ;;
esac

if ! security find-identity -v -p codesigning | grep -qF "$IDENTITY"; then
  echo "No such identity in the keychain: $IDENTITY" >&2
  security find-identity -v -p codesigning >&2
  exit 1
fi

# Same posture as the Developer-ID check: refuse, do not warn. Dev builds
# (--skip-notarize) may omit the token; BYO keys and Local keep working.
if [ -z "${PUBLIK_APP_TOKEN:-}" ] && [ "$SKIP_NOTARIZE" = "0" ]; then
  echo "Refusing to build a release without PUBLIK_APP_TOKEN in the environment." >&2
  echo "A release without it cannot offer publik API. Dev builds: pass --skip-notarize." >&2
  exit 1
fi
if [ -n "${PUBLIK_APP_TOKEN:-}" ]; then
  export PUBLIK_APP_TOKEN
  echo "==> publik app token present (value not shown)"
fi

TAURI="$REPO_ROOT/ui/node_modules/.bin/tauri"
[ -x "$TAURI" ] || TAURI="$(command -v tauri || true)"
if [ -z "$TAURI" ] || [ ! -x "$TAURI" ]; then
  echo "Tauri CLI missing — run 'pnpm install' in ui/ first." >&2
  exit 1
fi

NOTARIZE=0
if [ "$SKIP_NOTARIZE" = "0" ]; then
  if [ -n "${NOTARY_PROFILE:-}" ]; then
    NOTARIZE=1
    NOTARY_ARGS=(--keychain-profile "$NOTARY_PROFILE")
  elif [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_PASSWORD:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ]; then
    NOTARIZE=1
    NOTARY_ARGS=(--apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID")
  else
    echo "==> No notarization credentials — the build will be signed but NOT notarized." >&2
    echo "    Set NOTARY_PROFILE, or APPLE_ID + APPLE_PASSWORD + APPLE_TEAM_ID." >&2
  fi
fi

# Tauri notarizes the .app itself only from these three, and a partial set makes
# it half-start a submission — so pass all three or none.
if [ "$NOTARIZE" = "1" ] && [ -z "${APPLE_PASSWORD:-}" ]; then
  echo "==> Tauri cannot read a keychain profile; the .app will be stapled after the fact."
  unset APPLE_ID APPLE_TEAM_ID 2>/dev/null || true
fi

echo "==> Building with identity: $IDENTITY"
export APPLE_SIGNING_IDENTITY="$IDENTITY"

BUILD_ARGS=(build)
[ -n "$TARGET" ] && BUILD_ARGS+=(--target "$TARGET")

cd "$REPO_ROOT/src-tauri"
"$TAURI" "${BUILD_ARGS[@]}"

TARGET_DIR="$REPO_ROOT/target"
[ -n "$TARGET" ] && TARGET_DIR="$TARGET_DIR/$TARGET"
APP="$TARGET_DIR/release/bundle/macos/WhimprFlow.app"
DMG="$(/usr/bin/find "$TARGET_DIR/release/bundle/dmg" -name "*.dmg" -print -quit 2>/dev/null || true)"

[ -d "$APP" ] || { echo "No WhimprFlow.app was produced." >&2; exit 1; }

if [ "$NOTARIZE" = "1" ]; then
  # The app first. Submitting it on its own means its ticket exists before the
  # dmg is ever assessed, and a stapled app keeps working with no network.
  if ! xcrun stapler validate "$APP" >/dev/null 2>&1; then
    echo "==> Notarizing WhimprFlow.app"
    ZIP="$(mktemp -d)/WhimprFlow.zip"
    /usr/bin/ditto -c -k --keepParent "$APP" "$ZIP"
    xcrun notarytool submit "$ZIP" "${NOTARY_ARGS[@]}" --wait
    xcrun stapler staple "$APP"
    rm -f "$ZIP"
  fi

  # The dmg Tauri produced still contains the *unstapled* app, so rebuild it
  # around the stapled one rather than shipping a container whose payload needs
  # the network to validate.
  if [ -n "$DMG" ]; then
    echo "==> Rebuilding the dmg around the stapled app"
    STAGE="$(mktemp -d)"
    cp -R "$APP" "$STAGE/WhimprFlow.app"
    ln -s /Applications "$STAGE/Applications"
    rm -f "$DMG"
    hdiutil create -volname "WhimprFlow" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
    rm -rf "$STAGE"

    codesign --force --sign "$IDENTITY" --timestamp "$DMG"
    echo "==> Notarizing the disk image itself"
    xcrun notarytool submit "$DMG" "${NOTARY_ARGS[@]}" --wait
    xcrun stapler staple "$DMG"
  fi
fi

echo
echo "App: $APP"
[ -n "$DMG" ] && echo "Dmg: $DMG"
echo "Now run: scripts/verify-macos.sh"
