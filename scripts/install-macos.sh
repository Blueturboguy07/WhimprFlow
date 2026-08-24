#!/usr/bin/env bash
#
# Installs a built WhimprFlow.app into /Applications — and refuses to install
# one that macOS cannot keep a permission grant attached to.
#
# The bug this exists to make impossible: a bundle assembled around a bare
# `cargo build` binary is *ad-hoc* signed, and an ad-hoc signature's designated
# requirement is a plain hash of the executable:
#
#     designated => cdhash H"53b870a9dcfc5a05b5a7e48367bca975aee26130"
#
# macOS records that requirement when the user grants Accessibility. Rebuild the
# app and the hash changes, so the grant stops matching — while System Settings
# keeps listing WhimprFlow with its switch on, because that list is drawn from
# the bundle name and path, not the signature. The app then reports Accessibility
# as missing forever, the user re-grants it to no effect, and nothing about the
# symptom points anywhere near the build. A Developer ID signature's requirement
# is keyed to the team identifier instead and is identical across rebuilds, so
# the grant is given once and survives.
#
# Usage: scripts/install-macos.sh [path/to/WhimprFlow.app]
#        (defaults to the newest bundle under target/, i.e. what
#         scripts/build-macos.sh just produced)
set -euo pipefail

cd "$(dirname "$0")/.."
REPO_ROOT="$PWD"
DEST="/Applications/WhimprFlow.app"

APP="${1:-}"
if [ -z "$APP" ]; then
  APP="$(/usr/bin/find "$REPO_ROOT/target" -maxdepth 4 -name "WhimprFlow.app" -type d \
         -path "*/release/bundle/macos/*" -print 2>/dev/null | head -1)"
fi

[ -n "$APP" ] && [ -d "$APP" ] || {
  echo "No WhimprFlow.app to install. Run scripts/build-macos.sh first." >&2
  exit 1
}
echo "==> Candidate: $APP"

# 1. The signature has to be structurally valid. A hand-assembled bundle (binary
#    copied into Contents/MacOS by hand) fails here with "code has no resources
#    but signature indicates they must be present".
if ! codesign --verify --deep --strict "$APP" 2>/tmp/whimpr-verify.$$; then
  echo "Refusing to install: the bundle's signature does not verify." >&2
  sed 's/^/    /' /tmp/whimpr-verify.$$ >&2
  rm -f /tmp/whimpr-verify.$$
  echo "    Build via scripts/build-macos.sh — never by copying a cargo binary" >&2
  echo "    into a bundle by hand." >&2
  exit 1
fi
rm -f /tmp/whimpr-verify.$$

# 2. The designated requirement has to survive a rebuild. This is the check that
#    would have caught the stale-Accessibility-grant bug on the day it shipped.
REQ="$(codesign -d -r- "$APP" 2>/dev/null | sed -n 's/^.*designated => //p')"
case "$REQ" in
  *cdhash*)
    echo "Refusing to install: this build is ad-hoc signed." >&2
    echo "    designated => $REQ" >&2
    echo "" >&2
    echo "    Its identity is a hash of the binary, so every rebuild looks like a" >&2
    echo "    different app to macOS and silently drops the Accessibility grant." >&2
    echo "    Rebuild with a Developer ID: scripts/build-macos.sh" >&2
    exit 1
    ;;
  "")
    echo "Refusing to install: the bundle has no designated requirement (unsigned)." >&2
    exit 1
    ;;
esac
echo "==> Designated requirement is stable across rebuilds:"
echo "    $REQ"

# 3. Replace the installed copy. Quit it first — swapping the bundle under a
#    running process leaves it running the deleted one.
if pgrep -x "whimpr-tauri" >/dev/null 2>&1; then
  echo "==> Quitting the running WhimprFlow"
  osascript -e 'quit app "WhimprFlow"' >/dev/null 2>&1 || true
  for _ in $(seq 20); do
    pgrep -x "whimpr-tauri" >/dev/null 2>&1 || break
    sleep 0.25
  done
  pkill -x "whimpr-tauri" 2>/dev/null || true
fi

if [ -d "$DEST" ]; then
  BACKUP="${TMPDIR:-/tmp}/WhimprFlow.app.previous"
  rm -rf "$BACKUP"
  echo "==> Keeping the previous install at $BACKUP"
  mv "$DEST" "$BACKUP"
fi

echo "==> Installing to $DEST"
/usr/bin/ditto "$APP" "$DEST"

# 4. Prove the installed copy is what we checked — ditto preserves the
#    signature, but an install that silently corrupted it is exactly the class
#    of failure this script exists to catch.
codesign --verify --deep --strict "$DEST"
echo "==> Installed. Verified:"
codesign -d -r- "$DEST" 2>/dev/null | sed -n 's/^.*designated => /    designated => /p'

cat <<'EOF'

One last re-grant is needed, and only one.
The previously installed copy had a different identity, so the grant you already
gave is attached to that one. In System Settings → Privacy & Security →
Accessibility, select WhimprFlow, remove it with the - button, then launch
WhimprFlow and grant it again. From here on it survives every rebuild.
EOF
