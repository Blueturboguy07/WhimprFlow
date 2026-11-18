#!/usr/bin/env bash
# Updater proof on a macOS GitHub runner (updater-proof.yml). Never run it on a desk Mac: it
# installs into /Applications and starts the app.
#
# Usage: tests/updater/proof-macos.sh <N WhimprFlow.app> <N+1 WhimprFlow.app> <out dir>
# Env:   TAURI_SIGNING_PRIVATE_KEY(+_PASSWORD) (the real update key, for the N+1 bundle),
#        FEED_PORT (the port the test builds were compiled to use).
#
# 1. Signs N and N+1 with one throwaway code-signing identity (the repo has no Developer ID
#    secrets), with the codesign steps of scripts/build-macos.sh: the local-LLM worker, then
#    the app with the hardened runtime and src-tauri/Entitlements.plist.
# 2. Packs and signs N+1 with scripts/updater-pack-macos.sh and writes the feed with
#    scripts/updater-latest-json.mjs (the release tools), plus three bad feeds.
# 3. Installs N in /Applications and runs its updater self-test against each feed. The
#    self-test opens no window, shows no sign-in gate and never starts the microphone.
# 4. Copies the packed N+1 bundle and its .sig to <out>/release-files for the release-feed job.
set -euo pipefail
N_APP="${1:?N app}"; N1_APP="${2:?N+1 app}"; OUT="${3:?out dir}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PORT="${FEED_PORT:?FEED_PORT}"
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"
WORK="$(mktemp -d)"; FEED="$WORK/feed"; mkdir -p "$FEED"
INSTALLED=/Applications/WhimprFlow.app
BIN="$INSTALLED/Contents/MacOS/whimpr-tauri"
WORKER=Contents/MacOS/whimpr-llm-worker
PASS=0; FAILS=0
RESULTS="$OUT/results.md"
echo "| Check | Result | Detail |" > "$RESULTS"; echo "|---|---|---|" >> "$RESULTS"
pass() { PASS=$((PASS + 1)); echo "PASS  $1 ${2:+($2)}"; echo "| $1 | PASS | ${2:-} |" >> "$RESULTS"; }
fail() { FAILS=$((FAILS + 1)); echo "FAIL  $1 ${2:+($2)}"; echo "| $1 | **FAIL** | ${2:-} |" >> "$RESULTS"; }
check() { local name="$1" detail="$2"; shift 2; if "$@"; then pass "$name" "$detail"; else fail "$name" "$detail"; fi; }
plist() { /usr/libexec/PlistBuddy -c "Print :$2" "$1/Contents/Info.plist"; }
cdhash() { codesign -dvvv "$1" 2>&1 | sed -n 's/^CDHash=//p'; }
dr() { codesign -d -r- "$1" 2>&1 | sed -n 's/^designated => //p'; }
leaf() { local d; d="$(mktemp -d)"; (cd "$d" && codesign -d --extract-certificates "$1" >/dev/null 2>&1 && shasum -a 256 codesign0 | cut -d' ' -f1); rm -rf "$d"; }
# On-screen windows owned by a process or by UserNotificationCenter (CGWindowList; no permission
# needed for owner and count). The dialog plugin (rfd) shows a question without a parent window
# with CFUserNotificationDisplayAlert, so macOS draws it in UserNotificationCenter, not in the app.
# A compiled Swift helper first, then the same call through JXA. Errors go to $OUT/windows.log.
build_windows_helper() {
  cat > "$WORK/wins.swift" <<'EOF'
import Foundation
import CoreGraphics
let pid = Int(CommandLine.arguments[1])!
let list = (CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as NSArray?) as? [NSDictionary] ?? []
let mine = list.filter {
  ($0[kCGWindowOwnerPID as String] as? NSNumber)?.intValue == pid
    || ($0[kCGWindowOwnerName as String] as? String) == "UserNotificationCenter"
}
let names = mine.map { "\($0[kCGWindowOwnerName as String] ?? "?"):\($0[kCGWindowName as String] ?? "")" }
FileHandle.standardError.write("swift: \(list.count) on-screen windows, \(mine.count) of pid \(pid) or UserNotificationCenter \(names)\n".data(using: .utf8)!)
print(mine.count)
EOF
  cat > "$WORK/wins.js" <<'EOF'
ObjC.import('CoreGraphics');
function run(argv) {
  const pid = parseInt(argv[0], 10);
  const list = ObjC.deepUnwrap(ObjC.castRefToObject($.CGWindowListCopyWindowInfo($.kCGWindowListOptionOnScreenOnly, $.kCGNullWindowID))) || [];
  return String(list.filter(w => w.kCGWindowOwnerPID === pid || w.kCGWindowOwnerName === 'UserNotificationCenter').length);
}
EOF
  swiftc -O -o "$WORK/wins" "$WORK/wins.swift" >> "$OUT/windows.log" 2>&1 || echo "swiftc failed" >> "$OUT/windows.log"
}
windows_of() {
  local n=""
  [ -x "$WORK/wins" ] && n="$("$WORK/wins" "$1" 2>> "$OUT/windows.log" || true)"
  case "$n" in ''|0) n="$(osascript -l JavaScript "$WORK/wins.js" "$1" 2>> "$OUT/windows.log" || true)"; echo "jxa: $n" >> "$OUT/windows.log" ;; esac
  echo "${n:-0}"
}
field() { python3 -c 'import json,sys; lines=[l for l in open(sys.argv[1]) if l.strip()]; d=json.loads(lines[-1]); print(d.get(sys.argv[2], ""))' "$1" "$2" 2>/dev/null || true; }

# Runs the installed app's self-test. Sets RC (124 on a 180 s timeout).
selftest() {
  local mode="$1" report="$2" t=0 pid
  rm -f "$report"
  RUST_LOG=info,tauri_plugin_updater=debug PUBLIK_UPDATE_SELF_TEST="$mode" PUBLIK_UPDATE_SELF_TEST_OUT="$report" \
    "$BIN" > "$report.stdout" 2> "$report.log" &
  pid=$!
  while kill -0 "$pid" 2>/dev/null; do
    sleep 1; t=$((t + 1))
    if [ "$t" -ge 180 ]; then kill -9 "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; RC=124; return; fi
  done
  RC=0; wait "$pid" || RC=$?
}

build_windows_helper
echo "==> A throwaway code-signing identity (not Developer ID: the runner has no Apple certificate)"
KC="$WORK/proof.keychain-db"; KPW="proof-$RANDOM"
cat > "$WORK/cert.cnf" <<'EOF'
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = WhimprFlow Updater Proof
[ext]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
EOF
/usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 2 -config "$WORK/cert.cnf" -keyout "$WORK/key.pem" -out "$WORK/cert.pem" 2>/dev/null
/usr/bin/openssl pkcs12 -export -inkey "$WORK/key.pem" -in "$WORK/cert.pem" -name "WhimprFlow Updater Proof" -out "$WORK/id.p12" -passout pass:proof
security create-keychain -p "$KPW" "$KC"
security set-keychain-settings -lut 21600 "$KC"
security unlock-keychain -p "$KPW" "$KC"
security import "$WORK/id.p12" -k "$KC" -P proof -T /usr/bin/codesign >/dev/null
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KPW" "$KC" >/dev/null
security list-keychains -d user -s "$KC" $(security list-keychains -d user | tr -d '"')
ID="$(/usr/bin/openssl x509 -in "$WORK/cert.pem" -noout -fingerprint -sha1 | sed 's/.*=//; s/://g')"
sign_app() {
  # The codesign steps of scripts/build-macos.sh (the worker, then the app), without the Apple
  # timestamp, which a throwaway certificate cannot get.
  [ -f "$1/$WORKER" ] || { echo "no local-LLM worker in $1" >&2; return 1; }
  codesign --force --options runtime --timestamp=none --keychain "$KC" --sign "$ID" "$1/$WORKER" &&
  codesign --force --options runtime --timestamp=none --entitlements "$ROOT/src-tauri/Entitlements.plist" --keychain "$KC" --sign "$ID" "$1"
}
cp -R "$N_APP" "$WORK/N.app"; cp -R "$N1_APP" "$WORK/WhimprFlow.app"
if ! sign_app "$WORK/N.app" 2>"$WORK/sign.err"; then
  echo "codesign refused the untrusted identity; trusting it for code signing on this runner"; cat "$WORK/sign.err"
  sudo security add-trusted-cert -d -r trustRoot -p codeSign -k /Library/Keychains/System.keychain "$WORK/cert.pem"
  sign_app "$WORK/N.app"
fi
sign_app "$WORK/WhimprFlow.app"
codesign --verify --strict --deep "$WORK/N.app"
codesign --verify --strict --deep "$WORK/WhimprFlow.app"
N="$(plist "$WORK/N.app" CFBundleShortVersionString)"; N1="$(plist "$WORK/WhimprFlow.app" CFBundleShortVersionString)"
echo "N=$N  N+1=$N1"

echo "==> The N+1 update bundle and the feeds, with the release tools"
"$ROOT/scripts/updater-pack-macos.sh" "$WORK/WhimprFlow.app" "$FEED"
PKG="$(cd "$FEED" && ls *.app.tar.gz)"
check "release pack names the bundle by version and arch" "$PKG" test "$PKG" = "WhimprFlow_${N1}_aarch64.app.tar.gz"
node "$ROOT/scripts/updater-latest-json.mjs" --version "$N1" --allow-http --base-url "http://127.0.0.1:$PORT/" \
  --platform "darwin-aarch64=$FEED/$PKG" --notes "updater proof" --out "$FEED/latest-good.json"
# The release form of the same feed (absolute GitHub asset URLs), kept for the record.
node "$ROOT/scripts/updater-latest-json.mjs" --version "$N1" --base-url "https://github.com/${GITHUB_REPOSITORY:-owner/repo}/releases/download/v$N1/" \
  --platform "darwin-aarch64=$FEED/$PKG" --notes "updater proof" --out "$OUT/latest-release-form.json"
# A signature over the same bundle from another key.
"$ROOT/ui/node_modules/.bin/tauri" signer generate --ci -w "$WORK/other.key" -p "" >/dev/null 2>&1
cp "$FEED/$PKG" "$WORK/$PKG"
env -u TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD= "$ROOT/ui/node_modules/.bin/tauri" signer sign -f "$WORK/other.key" --app-version "$N1" "$WORK/$PKG" >/dev/null
node "$ROOT/tests/updater/make-feeds.mjs" "$FEED" darwin-aarch64 "$PKG" "$WORK/$PKG.sig"
cp "$FEED"/latest-*.json "$OUT/"
mkdir -p "$OUT/release-files"; cp "$FEED/$PKG" "$FEED/$PKG.sig" "$OUT/release-files/"

echo "==> Install N in /Applications"
rm -rf "$INSTALLED"; ditto "$WORK/N.app" "$INSTALLED"
N_CDHASH="$(cdhash "$INSTALLED")"; N_DR="$(dr "$INSTALLED")"; N_LEAF="$(leaf "$INSTALLED")"; N_BUNDLE="$(plist "$INSTALLED" CFBundleIdentifier)"
N1_CDHASH="$(cdhash "$WORK/WhimprFlow.app")"; N1_WORKER_CDHASH="$(cdhash "$WORK/WhimprFlow.app/$WORKER")"
codesign -dvvv "$INSTALLED" > "$OUT/codesign-N.txt" 2>&1; codesign -d -r- "$INSTALLED" >> "$OUT/codesign-N.txt" 2>&1
echo "N: bundle $N_BUNDLE, cdhash $N_CDHASH"; echo "N designated requirement: $N_DR"

python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$FEED" > "$OUT/feed-server.log" 2>&1 &
SERVER=$!
trap 'kill $SERVER 2>/dev/null || true' EXIT
for _ in $(seq 1 20); do curl -fs "http://127.0.0.1:$PORT/latest-good.json" >/dev/null && break; sleep 1; done

selftest version "$OUT/r-version-N.jsonl"
check "N reports its version" "$(field "$OUT/r-version-N.jsonl" version)" test "$(field "$OUT/r-version-N.jsonl" version)" = "$N"
cp "$FEED/latest-good.json" "$FEED/latest.json"
selftest check "$OUT/r-check-N.jsonl"
check "N finds N+1 on the feed" "result=$(field "$OUT/r-check-N.jsonl" result) new=$(field "$OUT/r-check-N.jsonl" new_version)" \
  test "$(field "$OUT/r-check-N.jsonl" result)/$(field "$OUT/r-check-N.jsonl" new_version)" = "available/$N1"

for case in badsig tampered version; do
  cp "$FEED/latest-$case.json" "$FEED/latest.json"
  R="$OUT/r-$case.jsonl"
  selftest install "$R"
  check "[$case] the update is refused (exit 2)" "exit=$RC; $(field "$R" error | head -c 160)" test "$RC" = 2 -a "$(field "$R" result)" = error
  check "[$case] the app on disk is still N" "$(plist "$INSTALLED" CFBundleShortVersionString)" test "$(plist "$INSTALLED" CFBundleShortVersionString)" = "$N"
  check "[$case] the app on disk is the same code (cdhash)" "$(cdhash "$INSTALLED")" test "$(cdhash "$INSTALLED")" = "$N_CDHASH"
  check "[$case] its signature is intact" "" codesign --verify --strict --deep "$INSTALLED"
done
# The reasons, as tauri-plugin-updater / minisign-verify word them.
check "[badsig] refused because of the key" "" grep -q "created with a different key" "$OUT/r-badsig.jsonl"
check "[tampered] refused because the signature does not verify" "" grep -q "signature verification failed" "$OUT/r-tampered.jsonl"
check "[version] refused because the signed version differs" "" grep -q "signed for version $N1 but the update endpoint announced" "$OUT/r-version.jsonl"

cp "$FEED/latest-good.json" "$FEED/latest.json"
R="$OUT/r-install.jsonl"
selftest install "$R"
check "[good] N downloads, verifies and installs N+1 (exit 0)" "exit=$RC result=$(field "$R" result) new=$(field "$R" new_version)" \
  test "$RC/$(field "$R" result)/$(field "$R" new_version)" = "0/installed/$N1"
check "[good] Info.plist on disk says N+1" "$(plist "$INSTALLED" CFBundleShortVersionString)" test "$(plist "$INSTALLED" CFBundleShortVersionString)" = "$N1"
selftest version "$OUT/r-version-after.jsonl"
check "[good] the app on disk reports N+1" "$(field "$OUT/r-version-after.jsonl" version)" test "$(field "$OUT/r-version-after.jsonl" version)" = "$N1"
check "[good] the installed code is the signed N+1, byte for byte (cdhash)" "$(cdhash "$INSTALLED")" test "$(cdhash "$INSTALLED")" = "$N1_CDHASH"
check "[good] its signature is intact" "" codesign --verify --strict --deep "$INSTALLED"
check "[good] the local-LLM worker inside is N+1's (cdhash)" "$(cdhash "$INSTALLED/$WORKER")" test "$(cdhash "$INSTALLED/$WORKER")" = "$N1_WORKER_CDHASH"
codesign -dvvv "$INSTALLED" > "$OUT/codesign-after.txt" 2>&1; codesign -d -r- "$INSTALLED" >> "$OUT/codesign-after.txt" 2>&1
check "[TCC] same bundle id" "$(plist "$INSTALLED" CFBundleIdentifier)" test "$(plist "$INSTALLED" CFBundleIdentifier)" = "$N_BUNDLE"
check "[TCC] same signing certificate" "leaf sha256 $(leaf "$INSTALLED" | head -c 16)…" test "$(leaf "$INSTALLED")" = "$N_LEAF"
check "[TCC] same designated requirement" "$(dr "$INSTALLED")" test "$(dr "$INSTALLED")" = "$N_DR"
check "[TCC] N+1 satisfies the requirement recorded for N" "" codesign --verify -R="$N_DR" "$INSTALLED"
check "[TCC] no leftover temp bundle next to the app" "" test -z "$(ls -d /Applications/*tauri* 2>/dev/null)"
selftest check "$OUT/r-check-after.jsonl"
check "[good] N+1 finds no newer version" "$(field "$OUT/r-check-after.jsonl" result)" test "$(field "$OUT/r-check-after.jsonl" result)" = none

echo "==> The restart question, from N again (the background check as users get it)"
rm -rf "$INSTALLED"; ditto "$WORK/N.app" "$INSTALLED"
R="$OUT/r-prompt.jsonl"; rm -f "$R"
BEFORE="$(windows_of 0)"
PUBLIK_UPDATE_SELF_TEST=prompt PUBLIK_UPDATE_SELF_TEST_OUT="$R" "$BIN" > "$R.stdout" 2> "$R.log" &
APP_PID=$!
for _ in $(seq 1 120); do grep -q '"asking"' "$R" 2>/dev/null && break; kill -0 "$APP_PID" 2>/dev/null || break; sleep 1; done
sleep 5
ALIVE=no; kill -0 "$APP_PID" 2>/dev/null && ALIVE=yes
check "[prompt] N asks to restart and keeps running (no forced restart)" "result=$(field "$R" result) running=$ALIVE version=$(field "$R" version)" \
  test "$(field "$R" result)/$ALIVE/$(field "$R" version)" = "asking/yes/$N"
check "[prompt] N+1 is already installed in the background" "$(plist "$INSTALLED" CFBundleShortVersionString), waits_for_yes=$(field "$R" install_waits_for_yes)" \
  test "$(plist "$INSTALLED" CFBundleShortVersionString)" = "$N1"
WINS=0
for _ in $(seq 1 10); do WINS="$(windows_of "$APP_PID")"; [ "${WINS:-0}" -gt "${BEFORE:-0}" ] 2>/dev/null && break; sleep 1; done
check "[prompt] the question is on screen" "$WINS window(s) of the app or its system alert, $BEFORE before launch" \
  test "${WINS:-0}" -gt "${BEFORE:-0}"
screencapture -x "$OUT/prompt-macos.png" 2>/dev/null || echo "screencapture unavailable"
kill "$APP_PID" 2>/dev/null || true; wait "$APP_PID" 2>/dev/null || true
sleep 2; echo "after the app quit: $(windows_of 0)" >> "$OUT/windows.log"

security delete-keychain "$KC" 2>/dev/null || true
rm -rf "$INSTALLED"
echo
echo "$PASS passed, $FAILS failed"
[ -n "${GITHUB_STEP_SUMMARY:-}" ] && { echo "### macOS updater proof: $PASS passed, $FAILS failed"; echo; cat "$RESULTS"; } >> "$GITHUB_STEP_SUMMARY"
[ "$FAILS" -eq 0 ]
