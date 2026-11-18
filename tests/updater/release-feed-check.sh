#!/usr/bin/env bash
# The release feed tool (scripts/updater-release-feed.sh) on real N+1 packages, as a release
# would carry them (updater-proof.yml, release-feed job). No network, no release.
#
# Usage: tests/updater/release-feed-check.sh <macOS files dir> <Windows files dir> <out dir>
#   <macOS files dir>   WhimprFlow_<N1>_aarch64.app.tar.gz + .sig (packed by proof-macos.sh)
#   <Windows files dir> WhimprFlow_<N1>_x64-setup.exe + .sig (tauri build with the update key)
# Env: N1 (the N+1 version), GITHUB_REPOSITORY.
set -euo pipefail
MAC_DIR="${1:?macOS files}"; WIN_DIR="${2:?Windows files}"; OUT="${3:?out dir}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
V="${N1:?N1}"; TAG="v$V"; REPO="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY}"
MAC="WhimprFlow_${V}_aarch64.app.tar.gz"; WIN="WhimprFlow_${V}_x64-setup.exe"
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"
WORK="$(mktemp -d)"; REL="$WORK/release"; mkdir -p "$REL"
cp "$MAC_DIR/$MAC" "$MAC_DIR/$MAC.sig" "$WIN_DIR/$WIN" "$WIN_DIR/$WIN.sig" "$REL/"
PASS=0; FAILS=0
RESULTS="$OUT/results.md"
echo "| Check | Result | Detail |" > "$RESULTS"; echo "|---|---|---|" >> "$RESULTS"
pass() { PASS=$((PASS + 1)); echo "PASS  $1 ${2:+($2)}"; echo "| $1 | PASS | ${2:-} |" >> "$RESULTS"; }
fail() { FAILS=$((FAILS + 1)); echo "FAIL  $1 ${2:+($2)}"; echo "| $1 | **FAIL** | ${2:-} |" >> "$RESULTS"; }
check() { local name="$1" detail="$2"; shift 2; if "$@"; then pass "$name" "$detail"; else fail "$name" "$detail"; fi; }
jq_() { node -e 'const f=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")); const v=process.argv[2].split(".").reduce((o,k)=>o?.[k],f); process.stdout.write(v===undefined?"":String(v))' "$@"; }

# The complete release: both platforms.
RC=0; NOTES="updater proof" "$ROOT/scripts/updater-release-feed.sh" "$TAG" --dir "$REL" > "$OUT/feed.log" 2>&1 || RC=$?
cat "$OUT/feed.log"
check "the feed is written for a complete release" "exit $RC" test "$RC" = 0 -a -s "$REL/latest.json"
cp "$REL/latest.json" "$OUT/latest.json" 2>/dev/null || true
F="$REL/latest.json"
check "it announces N+1" "$(jq_ "$F" version)" test "$(jq_ "$F" version)" = "$V"
for target in darwin-aarch64:"$MAC" windows-x86_64:"$WIN"; do
  t="${target%%:*}"; f="${target#*:}"
  want="https://github.com/$REPO/releases/download/$TAG/$f"
  check "[$t] url is the absolute GitHub asset URL" "$(jq_ "$F" "platforms.$t.url")" test "$(jq_ "$F" "platforms.$t.url")" = "$want"
  check "[$t] signature is the .sig on the release" "" test "$(jq_ "$F" "platforms.$t.signature")" = "$(tr -d '\n' < "$REL/$f.sig")"
done
check "only the two release platforms" "$(node -p "Object.keys(require('$F').platforms).sort().join(',')")" \
  test "$(node -p "Object.keys(require('$F').platforms).sort().join(',')")" = "darwin-aarch64,windows-x86_64"

# Half a release: --if-complete says so and writes nothing; without it the tool fails.
HALF="$WORK/half"; mkdir -p "$HALF"; cp "$REL/$MAC" "$REL/$MAC.sig" "$REL/$WIN" "$HALF/"
RC=0; "$ROOT/scripts/updater-release-feed.sh" "$TAG" --dir "$HALF" --if-complete > "$OUT/half.log" 2>&1 || RC=$?
check "[half] --if-complete waits for the other half (exit 0, no feed)" "exit $RC" test "$RC" = 0 -a ! -e "$HALF/latest.json"
RC=0; "$ROOT/scripts/updater-release-feed.sh" "$TAG" --dir "$HALF" >> "$OUT/half.log" 2>&1 || RC=$?
check "[half] without --if-complete it fails" "exit $RC" test "$RC" != 0 -a ! -e "$HALF/latest.json"

# A changed package or a tag of another version is refused, and no feed is written.
BAD="$WORK/bad"; mkdir -p "$BAD"; cp "$REL/$MAC" "$REL/$MAC.sig" "$REL/$WIN" "$REL/$WIN.sig" "$BAD/"
printf 'x' >> "$BAD/$WIN"
RC=0; "$ROOT/scripts/updater-release-feed.sh" "$TAG" --dir "$BAD" > "$OUT/bad.log" 2>&1 || RC=$?
check "[tampered] a changed installer is refused" "exit $RC; $(grep -o 'does not verify.*' "$OUT/bad.log" | head -1)" test "$RC" != 0 -a ! -e "$BAD/latest.json"
OTHER="$WORK/other"; mkdir -p "$OTHER"
for f in "$MAC" "$MAC.sig" "$WIN" "$WIN.sig"; do cp "$REL/$f" "$OTHER/${f/_${V}_/_9.9.9_}"; done
RC=0; "$ROOT/scripts/updater-release-feed.sh" v9.9.9 --dir "$OTHER" > "$OUT/other.log" 2>&1 || RC=$?
check "[version] files signed for N+1 cannot be announced as 9.9.9" "exit $RC; $(grep -o 'signed for version.*' "$OUT/other.log" | head -1)" test "$RC" != 0 -a ! -e "$OTHER/latest.json"

rm -rf "$WORK"
echo
echo "$PASS passed, $FAILS failed"
[ -n "${GITHUB_STEP_SUMMARY:-}" ] && { echo "### Release feed: $PASS passed, $FAILS failed"; echo; cat "$RESULTS"; } >> "$GITHUB_STEP_SUMMARY"
[ "$FAILS" -eq 0 ]
