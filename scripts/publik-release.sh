#!/usr/bin/env bash
# The whole WhimprFlow release in one command, on the release Mac (docs/updates.md, "Cut a release"):
#
#   scripts/publik-release.sh <version> [--notes "<one line for the feed and the release>"]
#
# Before it: put <version> in Cargo.toml ([workspace.package]), Cargo.lock (the whimpr-* crates),
# src-tauri/tauri.conf.json and ui/package.json, commit, push to main.
#
# What it does, for macOS arm64 and Windows x64 (the two systems WhimprFlow ships):
#  1. Checks: on main, clean tree, pushed; the version in every file; higher than the last release;
#     the tag is free; the Developer ID, the notary profile and the update key are there; disk.
#  2. Runs publik-build.yml on that commit (the unsigned Mac app; the Windows installer with its
#     updater .sig, the Rust tests and scripts/verify-windows.ps1) and waits for it.
#  3. Downloads the two artifacts. The installer's .sig must verify with the app's public key.
#  4. scripts/sign-ci-macos.sh: signs the app (inner code first, hardened runtime,
#     src-tauri/Entitlements.plist) with the Developer ID, notarizes and staples it, packs and
#     signs the update bundle from the stapled app, builds the dmg, signs, notarizes, staples it.
#  5. Writes latest.json (absolute GitHub URLs) and signs it with the update key (latest.json.sig).
#  6. Uploads everything to a DRAFT release.
#  7. Downloads the draft back and verifies it: asset list, checksums, the feed and every signature
#     with the app's public key, versions (Info.plist, exe), codesign, spctl, stapler, the
#     designated requirement and the publik app token against the last release.
#  8. Publishes, then checks the tag and every feed URL.
#
# Run the same command again after a failure: it reuses the CI run of the commit, the files in the
# work folder and the draft, and goes on from there. For a published release it only runs the
# checks of step 7. It stops before publishing on any failed check (the draft stays), starts no
# app, and prints no secret: the update key goes from the keychain into the signer's environment
# only, and the app token is compared by hash, never shown.
#
# Env: PUBLIK_RELEASE_WORK  work folder (default <repo>/.release/v<version>, git-ignored);
#                           deleted after a successful publish
#      APPLE_SIGNING_IDENTITY, NOTARY_PROFILE (default "publik"), TAURI_CLI
#      UPDATE_KEY_SERVICE   keychain service of the update key (default "publik-update-key",
#                           accounts mann-private and mann-password)
#      PUBLIK_RELEASE_NO_PUBLISH=1  stop after the verify and leave the draft
#      PUBLIK_RELEASE_MIN_FREE_MB   free disk the release needs (default 1500)
set -euo pipefail

V="${1:?usage: scripts/publik-release.sh <version> [--notes \"text\"]}"
shift
NOTES_LINE=""
while [ $# -gt 0 ]; do
  case "$1" in
    --notes) NOTES_LINE="${2:?--notes needs a text}"; shift 2 ;;
    *) echo "publik-release: unknown argument $1" >&2; exit 2 ;;
  esac
done
V="${V#v}"
TAG="v$V"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

APP=WhimprFlow
SLUG=whimprflow
BUNDLE_ID=com.whimpr.whimprflow
MAIN_EXE=whimpr-tauri
BRANCH=main
CI_WORKFLOW=publik-build.yml
MAC_ARTIFACT=whimprflow-macos-arm64-unsigned
MAC_ZIP=WhimprFlow-macos-arm64-unsigned.app.zip
WIN_ARTIFACT=whimprflow-windows-installer
IDENTITY="${APPLE_SIGNING_IDENTITY:-Developer ID Application: Mann Bellani (R5R3ZS54LV)}"
TEAM=R5R3ZS54LV
PROFILE="${NOTARY_PROFILE:-publik}"
SERVICE="${UPDATE_KEY_SERVICE:-publik-update-key}"
WORK="${PUBLIK_RELEASE_WORK:-$ROOT/.release/$TAG}"
MIN_FREE_MB="${PUBLIK_RELEASE_MIN_FREE_MB:-1500}"
DMG="${APP}_${V}_aarch64.dmg"
EXE="${APP}_${V}_x64-setup.exe"
TGZ="${APP}_${V}_aarch64.app.tar.gz"
ASSETS=("$DMG" "$EXE" "$TGZ" "$TGZ.sig" "$EXE.sig" latest.json latest.json.sig SHA256SUMS.txt)
TOKEN_RE='pat_whimprflow_[a-z0-9]{32}'

say() { printf '\n==> %s\n' "$*"; }
ok() { printf '  ok    %s\n' "$*"; }
note() { printf '  note  %s\n' "$*"; }
die() { printf '\nSTOP: %s\n' "$*" >&2; exit 1; }
FAILS=0
bad() { printf '  FAIL  %s\n' "$*"; FAILS=$((FAILS + 1)); }
# expect <description> <command...>: ok when the command succeeds, FAIL otherwise.
expect() { local what="$1"; shift; if "$@"; then ok "$what"; else bad "$what"; fi; }
plist() { /usr/libexec/PlistBuddy -c "Print :$2" "$1/Contents/Info.plist"; }
sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
dr() { codesign -d -r- "$1" 2>&1 | sed -n 's/^designated => //p'; }
stapled() { xcrun stapler validate "$1" >/dev/null 2>&1; }
# The publik app token compiled into a binary, as a hash: the token itself is never printed.
token_hash() { LC_ALL=C grep -a -o -E "$TOKEN_RE" "$1" 2>/dev/null | sort -u | shasum -a 256 | cut -c1-64; }
token_count() { LC_ALL=C grep -a -o -E "$TOKEN_RE" "$1" 2>/dev/null | sort -u | wc -l | tr -d ' '; }
free_mb() { df -k "$1" | awk 'NR == 2 { print int($4 / 1024) }'; }

[[ "$V" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "the version must be x.y.z, not $V"
for t in gh git node npm xcrun codesign hdiutil security shasum spctl ditto tar unzip file curl; do
  command -v "$t" >/dev/null || die "$t is not installed"
done
gh auth status >/dev/null 2>&1 || die "gh is not logged in"
mkdir -p "$WORK"
# gh for reads and downloads, with retries: the GitHub API limit (5000 calls an hour) is per user
# and other jobs of the same user share it; a 5xx or a dropped connection is retried too.
ghr() {
  local out err i
  for i in $(seq 1 45); do
    if out="$(gh "$@" 2> "$WORK/gh.err")"; then
      [ -z "$out" ] || printf '%s\n' "$out"
      return 0
    fi
    err="$(tr '\n' ' ' < "$WORK/gh.err")"
    case "$err" in
      *"rate limit"* | *"HTTP 5"* | *"timed out"* | *"timeout"* | *"connection"* | *"EOF"* | *"TLS"*) ;;
      *) printf '%s\n' "$err" >&2; return 1 ;;
    esac
    echo "    GitHub API: ${err:0:110}... waiting 90 s (try $i of 45)" >&2
    sleep 90
  done
  printf '%s\n' "$err" >&2
  return 1
}
REPO="$(ghr repo view --json nameWithOwner -q .nameWithOwner)"
BASE_URL="https://github.com/$REPO/releases/download/$TAG/"
STATE="$WORK/state"
touch "$STATE"
get_state() { sed -n "s/^$1=//p" "$STATE" | tail -1; }
set_state() { echo "$1=$2" >> "$STATE"; }

# ---------------------------------------------------------------------------------------------
say "$APP $V: checks"
RELEASE="$(ghr release view "$TAG" -R "$REPO" --json isDraft,targetCommitish 2>/dev/null || true)"
if [ -z "$RELEASE" ]; then RSTATE=none
elif [ "$(node -p 'JSON.parse(process.argv[1]).isDraft' "$RELEASE")" = true ]; then RSTATE=draft
else RSTATE=published; fi
ok "release $TAG: $RSTATE"
PREV_TAG="$(ghr release list -R "$REPO" --exclude-drafts --exclude-pre-releases -L 20 --json tagName -q '.[].tagName' | grep -vx "$TAG" | head -1)"
[ -n "$PREV_TAG" ] || die "no earlier published release"
ok "last published release: $PREV_TAG"
git fetch -q origin "$BRANCH" --tags

if [ "$RSTATE" = published ]; then
  SHA="$(git rev-parse "$TAG^{commit}")"
  note "$TAG is already published: verifying it only"
else
  [ "$(git rev-parse --abbrev-ref HEAD)" = "$BRANCH" ] || die "check out $BRANCH first"
  [ -z "$(git status --porcelain)" ] || die "the working tree is not clean"
  SHA="$(git rev-parse HEAD)"
  [ "$SHA" = "$(git rev-parse "origin/$BRANCH")" ] || die "HEAD is not origin/$BRANCH: push (or pull) first"
  ok "on $BRANCH at ${SHA:0:12}, clean, pushed"
  if git ls-remote --exit-code --tags origin "refs/tags/$TAG" >/dev/null 2>&1; then die "the tag $TAG exists but has no published release"; fi
  ok "tag $TAG is free"
  if [ "$RSTATE" = draft ]; then
    TARGET="$(node -p 'JSON.parse(process.argv[1]).targetCommitish' "$RELEASE")"
    [ "$TARGET" = "$SHA" ] || die "the draft $TAG is for commit $TARGET, not HEAD $SHA: delete the draft or check out that commit"
    ok "resuming the draft $TAG"
  fi
  # The version in every file a bump touches (the 0.2.8 bump: Cargo.toml, Cargo.lock,
  # src-tauri/tauri.conf.json, ui/package.json).
  CONF_V="$(node -p 'require("./src-tauri/tauri.conf.json").version')"
  UI_V="$(node -p 'require("./ui/package.json").version')"
  CARGO_V="$(awk '/^\[workspace.package\]/ { f = 1; next } /^\[/ { f = 0 } f && /^version = / { gsub(/version = |"/, ""); print; exit }' Cargo.toml)"
  for pair in "src-tauri/tauri.conf.json:$CONF_V" "ui/package.json:$UI_V" "Cargo.toml:$CARGO_V"; do
    [ "${pair#*:}" = "$V" ] || die "${pair%%:*} has version ${pair#*:}, not $V"
  done
  for crate in $(awk '/^name = "whimpr-/ { gsub(/name = |"/, ""); print }' Cargo.lock); do
    LOCK_V="$(awk -v n="$crate" '$0 == "name = \"" n "\"" { getline; gsub(/version = |"/, ""); print; exit }' Cargo.lock)"
    [ "$LOCK_V" = "$V" ] || die "Cargo.lock has $crate $LOCK_V, not $V"
  done
  ok "version $V in Cargo.toml, Cargo.lock (every whimpr-* crate), src-tauri/tauri.conf.json and ui/package.json"
  [ "$(printf '%s\n%s\n' "${PREV_TAG#v}" "$V" | sort -V | tail -1)" = "$V" ] && [ "${PREV_TAG#v}" != "$V" ] ||
    die "$V is not higher than the last release ${PREV_TAG#v}"
  ok "$V is higher than ${PREV_TAG#v}"
  security find-identity -v -p codesigning | grep -qF "\"$IDENTITY\"" || die "no signing identity \"$IDENTITY\" in the keychain"
  ok "signing identity: $IDENTITY"
  security find-generic-password -s "$SERVICE" -a mann-private >/dev/null 2>&1 || die "no update key: keychain service $SERVICE, account mann-private"
  ok "update key: keychain item $SERVICE (not read yet)"
  xcrun notarytool history --keychain-profile "$PROFILE" >/dev/null 2>&1 || die "the notary profile \"$PROFILE\" does not work (xcrun notarytool store-credentials)"
  ok "notary profile: $PROFILE"
fi
# The work folder belongs to one commit: files from another commit's builds are never reused.
if [ "$RSTATE" != published ]; then
  if [ -n "$(get_state sha)" ] && [ "$(get_state sha)" != "$SHA" ]; then
    note "the work folder has the builds of $(get_state sha | cut -c1-12): starting it again for ${SHA:0:12}"
    rm -rf "$WORK/in" "$WORK/out" "$WORK/verify" "$WORK/feed-notes.txt" "$WORK/release-notes.md"
    : > "$STATE"
  fi
  [ "$(get_state sha)" = "$SHA" ] || set_state sha "$SHA"
fi
[ "$(free_mb "$WORK")" -ge "$MIN_FREE_MB" ] || die "only $(free_mb "$WORK") MB free on the disk of $WORK; the release needs $MIN_FREE_MB MB"
ok "disk: $(free_mb "$WORK") MB free (needs $MIN_FREE_MB MB)"

# The Tauri CLI signs the update files: the repo's, else the locked version in the work folder.
TAURI="${TAURI_CLI:-}"
if [ "$RSTATE" != published ]; then
  for c in "$ROOT/node_modules/.bin/tauri" "$ROOT/ui/node_modules/.bin/tauri" "$WORK/tauri-cli/node_modules/.bin/tauri"; do
    [ -z "$TAURI" ] && [ -x "$c" ] && TAURI="$c"
  done
  if [ -z "$TAURI" ]; then
    CLI_V="$(awk "/^      '@tauri-apps\/cli':\$/ { f = 1; next } f && /version:/ { print \$2; exit }" ui/pnpm-lock.yaml)"
    [ -n "$CLI_V" ] || die "no @tauri-apps/cli version in ui/pnpm-lock.yaml"
    npm install --silent --no-save --no-package-lock --no-audit --no-fund --prefix "$WORK/tauri-cli" "@tauri-apps/cli@$CLI_V" >/dev/null
    TAURI="$WORK/tauri-cli/node_modules/.bin/tauri"
  fi
  ok "Tauri CLI $("$TAURI" --version | awk '{ print $NF }')"
  export TAURI_CLI="$TAURI"
fi

# Runs a command with the update key in its environment. The key is never printed.
with_update_key() {
  if [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then "$@"; return; fi
  local key pw
  key="$(security find-generic-password -s "$SERVICE" -a mann-private -w 2>/dev/null)" || die "cannot read the update key"
  pw="$(security find-generic-password -s "$SERVICE" -a mann-password -w 2>/dev/null || true)"
  TAURI_SIGNING_PRIVATE_KEY="$key" TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$pw" "$@"
}

# ---------------------------------------------------------------------------------------------
# At most one GitHub API call per 90 s while waiting: other jobs of the user share the limit.
POLL=90
run_state() { ghr run view "$1" -R "$REPO" --json status,conclusion -q '"\(.status) \(.conclusion)"'; }
# The newest workflow_dispatch run of the workflow on $SHA, or nothing.
last_run() {
  ghr run list -R "$REPO" --workflow "$1" --commit "$SHA" --event workflow_dispatch -L 20 \
    --json databaseId,createdAt -q 'sort_by(.createdAt) | reverse | .[0].databaseId // empty'
}
# Starts the workflow on $BRANCH and prints the new run's id.
start_run() {
  local wf="$1" before id
  before="$(ghr run list -R "$REPO" --workflow "$wf" -L 50 --json databaseId -q '.[].databaseId' | sort)"
  gh workflow run "$wf" -R "$REPO" --ref "$BRANCH" >/dev/null
  sleep 15
  for _ in $(seq 1 10); do
    id="$(ghr run list -R "$REPO" --workflow "$wf" --commit "$SHA" --event workflow_dispatch -L 20 --json databaseId -q '.[].databaseId' |
      sort | comm -13 <(printf '%s\n' "$before") - | tail -1)"
    [ -n "$id" ] && { echo "$id"; return; }
    sleep "$POLL"
  done
  die "$wf did not start"
}
# Waits for a run to complete; its conclusion is then in $RUN_CONCLUSION.
wait_run() {
  local id="$1" t=0 st
  while :; do
    st="$(run_state "$id")"
    [ "${st%% *}" = completed ] && break
    [ $((t % (POLL * 4))) -eq 0 ] && echo "    run $id: ${st%% *} ($((t / 60)) min)"
    sleep "$POLL"; t=$((t + POLL))
    [ "$t" -lt 9000 ] || die "run $id did not finish in 150 minutes"
  done
  RUN_CONCLUSION="${st#* }"
}
# A run of the workflow on $SHA that is successful or still running; else a new one.
ensure_run() {
  local wf="$1" id st=""
  id="$(last_run "$wf")"
  [ -z "$id" ] || st="$(run_state "$id")"
  if [ -n "$id" ] && [ "${st%% *}" = completed ] && [ "${st#* }" != success ]; then
    echo "    $wf run $id on ${SHA:0:12} ended with ${st#* }: starting a new one" >&2
    id=""
  fi
  if [ -z "$id" ]; then
    id="$(start_run "$wf")"
    echo "    started $wf: https://github.com/$REPO/actions/runs/$id" >&2
  fi
  echo "$id"
}

IN="$WORK/in"
OUT="$WORK/out"
mkdir -p "$IN" "$OUT"
# "name sha256:<hex>" of every uploaded asset of the release.
asset_digests() { ghr release view "$TAG" -R "$REPO" --json assets -q '.assets[] | select(.state == "uploaded") | "\(.name) \(.digest)"'; }
have_all_assets() {
  [ "$(asset_digests | cut -d' ' -f1 | sort)" = "$(printf '%s\n' "${ASSETS[@]}" | sort)" ]
}

if [ "$RSTATE" != published ]; then
  if [ "$RSTATE" = draft ] && [ ! -f "$OUT/$DMG" ] && have_all_assets; then
    note "the draft has every file and the work folder has none: verifying the draft"
    CI_RUN="$(get_state ci_run)"
  else
    say "CI build of ${SHA:0:12} ($CI_WORKFLOW: macOS app, Windows installer, Rust tests, verify-windows.ps1)"
    CI_RUN="$(ensure_run "$CI_WORKFLOW")"
    wait_run "$CI_RUN"
    [ "$RUN_CONCLUSION" = success ] || die "$CI_WORKFLOW run $CI_RUN ended with $RUN_CONCLUSION: https://github.com/$REPO/actions/runs/$CI_RUN"
    ok "$CI_WORKFLOW: https://github.com/$REPO/actions/runs/$CI_RUN"
    set_state ci_run "$CI_RUN"

    say "Download the CI builds"
    if [ ! -f "$OUT/$EXE" ] || [ ! -f "$OUT/$EXE.sig" ]; then
      rm -rf "$IN/win"
      ghr run download "$CI_RUN" -R "$REPO" -n "$WIN_ARTIFACT" -D "$IN/win"
      [ -f "$IN/win/$EXE" ] && [ -f "$IN/win/$EXE.sig" ] || die "the $WIN_ARTIFACT artifact has no $EXE and $EXE.sig (it has: $(ls "$IN/win" | tr '\n' ' '))"
      mv "$IN/win/$EXE" "$IN/win/$EXE.sig" "$OUT/"
      rm -rf "$IN/win"
    fi
    # The CI signature over the installer must verify with the app's public key and name $V.
    if ! { node scripts/updater-latest-json.mjs --version "$V" --base-url "$BASE_URL" --platform "windows-x86_64=$OUT/$EXE" --out "$WORK/win-check.json" 2>&1 | sed 's/^/        /'; }; then
      die "the CI installer's updater signature does not verify (see above)"
    fi
    rm -f "$WORK/win-check.json"
    node scripts/exe-version.mjs "$OUT/$EXE" --expect "$V" >/dev/null || die "the CI installer does not carry version $V"
    [ -n "$(get_state win_sha)" ] || set_state win_sha "$(sha "$OUT/$EXE")"
    [ "$(get_state win_sha)" = "$(sha "$OUT/$EXE")" ] || die "$EXE in the work folder is not the file downloaded from CI"
    ok "$EXE and its .sig (version $V; the CI signature verifies with the app's public key)"

    say "Sign, notarize and staple the Mac app; update bundle; dmg"
    if [ -f "$OUT/$DMG" ] && [ -f "$OUT/$TGZ" ] && [ -f "$OUT/$TGZ.sig" ]; then
      note "already done in $OUT"
    else
      rm -f "$OUT/$DMG" "$OUT/$TGZ" "$OUT/$TGZ.sig" "$OUT/latest.json" "$OUT/latest.json.sig"
      [ -f "$IN/$MAC_ZIP" ] || ghr run download "$CI_RUN" -R "$REPO" -n "$MAC_ARTIFACT" -D "$IN"
      [ -f "$IN/$MAC_ZIP" ] || die "the $MAC_ARTIFACT artifact has no $MAC_ZIP"
      unzip -p "$IN/$MAC_ZIP" WhimprFlow.app/Contents/Info.plist > "$IN/Info.plist"
      [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$IN/Info.plist")" = "$V" ] || die "the CI app is not version $V"
      [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$IN/Info.plist")" = "$BUNDLE_ID" ] || die "the CI app is not $BUNDLE_ID"
      ok "$MAC_ZIP (version $V, $BUNDLE_ID)"
      with_update_key "$ROOT/scripts/sign-ci-macos.sh" "$IN/$MAC_ZIP" "$OUT"
      [ -f "$OUT/$DMG" ] && [ -f "$OUT/$TGZ" ] && [ -f "$OUT/$TGZ.sig" ] || die "scripts/sign-ci-macos.sh did not write $DMG, $TGZ and $TGZ.sig"
      rm -rf "$IN"
      mkdir -p "$IN"
    fi

    say "Feed and checksums"
    [ -n "$NOTES_LINE" ] || NOTES_LINE="$(get_state notes)"
    [ -n "$NOTES_LINE" ] || NOTES_LINE="Version $V."
    [ "$(get_state notes)" = "$NOTES_LINE" ] || set_state notes "$NOTES_LINE"
    if [ ! -f "$OUT/latest.json" ]; then
      printf '%s %s: %s\n' "$APP" "$V" "$NOTES_LINE" > "$WORK/feed-notes.txt"
      node scripts/updater-latest-json.mjs --version "$V" --base-url "$BASE_URL" \
        --platform "darwin-aarch64=$OUT/$TGZ" --platform "windows-x86_64=$OUT/$EXE" \
        --notes-file "$WORK/feed-notes.txt" --out "$OUT/latest.json"
      rm -f "$OUT/latest.json.sig"
    fi
    if [ ! -f "$OUT/latest.json.sig" ]; then
      with_update_key "$TAURI" signer sign --app-version "$V" "$OUT/latest.json" >/dev/null
    fi
    [ -s "$OUT/latest.json.sig" ] || die "latest.json was not signed"
    ok "latest.json and latest.json.sig"
    (cd "$OUT" && shasum -a 256 "$DMG" "$EXE" "$TGZ" > SHA256SUMS.txt)
    ok "SHA256SUMS.txt"

    say "Draft release $TAG"
    {
      printf '%s\n\n' "$NOTES_LINE"
      printf 'Files:\n\n'
      printf -- '- **macOS (Apple silicon):** `%s`. Signed with Developer ID, notarized, stapled.\n' "$DMG"
      printf -- '- **Windows 10/11:** `%s`. The installer is not code-signed, so Windows SmartScreen shows a warning. Click **More info -> Run anyway**.\n' "$EXE"
      printf -- '- Update files (the app downloads these itself): `%s`, `%s` and their `.sig` files, and the feed `latest.json` with its signature `latest.json.sig`. publik serves the feed at `https://publikhq.com/api/updates/%s/latest.json` after it approves this release.\n' "$TGZ" "$EXE" "$SLUG"
      printf -- '- Checksums of the dmg, the installer and the macOS update bundle: `SHA256SUMS.txt`.\n\n'
      printf 'WhimprFlow 0.2.8 and later install this version themselves. Every update is signed with the publik update key, and WhimprFlow checks the signature and the version before it installs anything.\n\n'
      printf 'Built by https://github.com/%s/actions/runs/%s (the Windows installer, the Rust tests and the Windows install check; the macOS app unsigned, then signed and notarized on the release Mac with `scripts/publik-release.sh`).\n' "$REPO" "$CI_RUN"
    } > "$WORK/release-notes.md"
    if [ "$RSTATE" = none ]; then
      (cd "$OUT" && gh release create "$TAG" -R "$REPO" --draft --target "$SHA" --title "$APP $V" \
        --notes-file "$WORK/release-notes.md" "${ASSETS[@]}" >/dev/null)
      RSTATE=draft
      ok "draft created with ${#ASSETS[@]} files"
    else
      DIGESTS="$(asset_digests)"
      for a in "${ASSETS[@]}"; do
        if printf '%s\n' "$DIGESTS" | grep -qxF "$a sha256:$(sha "$OUT/$a")"; then continue; fi
        gh release upload "$TAG" -R "$REPO" --clobber "$OUT/$a" >/dev/null
        ok "uploaded $a"
      done
      gh release edit "$TAG" -R "$REPO" --notes-file "$WORK/release-notes.md" >/dev/null
      ok "draft up to date"
    fi
    # The draft holds every file byte for byte: the large local copies go (disk), the verify
    # below downloads them back.
    DIGESTS="$(asset_digests)"
    for a in "${ASSETS[@]}"; do
      printf '%s\n' "$DIGESTS" | grep -qxF "$a sha256:$(sha "$OUT/$a")" || die "the draft's $a is not the file in $OUT"
    done
    ok "the draft holds every file, byte for byte (GitHub sha256 digests)"
    rm -f "$OUT/$DMG" "$OUT/$TGZ" "$OUT/$EXE"
  fi
fi

# ---------------------------------------------------------------------------------------------
say "Verify $TAG as published files (downloaded back)"
VER="$WORK/verify"
rm -rf "$VER"
mkdir -p "$VER/prev" "$VER/tgz" "$VER/mnt"
[ "$(free_mb "$WORK")" -ge 800 ] || die "only $(free_mb "$WORK") MB free; the verify needs 800 MB"
ghr release download "$TAG" -R "$REPO" -D "$VER" --clobber >/dev/null
GOT="$(cd "$VER" && find . -maxdepth 1 -type f | sed 's|^\./||' | sort)"
if [ "$GOT" = "$(printf '%s\n' "${ASSETS[@]}" | sort)" ]; then ok "the release has exactly: ${ASSETS[*]}"
else bad "the release has exactly: ${ASSETS[*]}"; echo "        it has: $(echo "$GOT" | tr '\n' ' ')"; fi
expect "SHA256SUMS.txt matches the dmg, the installer and the update bundle" sh -c "cd '$VER' && shasum -a 256 -c SHA256SUMS.txt >/dev/null"
git show "$SHA:src-tauri/tauri.conf.json" > "$VER/tauri.conf.json"
git show "$PREV_TAG:src-tauri/tauri.conf.json" > "$VER/tauri.conf.prev.json"
pubkey_of() { node -p 'require(process.argv[1]).plugins?.updater?.pubkey ?? ""' "$1"; }
endpoints_of() { node -p 'JSON.stringify(require(process.argv[1]).plugins?.updater?.endpoints ?? [])' "$1"; }
if [ -n "$(pubkey_of "$VER/tauri.conf.prev.json")" ]; then
  expect "the update public key is the one $PREV_TAG ships (installed copies trust it)" \
    test "$(pubkey_of "$VER/tauri.conf.json")" = "$(pubkey_of "$VER/tauri.conf.prev.json")"
else
  note "$PREV_TAG has no updater: no public key to compare"
fi
expect "the feed URL is https://publikhq.com/api/updates/$SLUG/latest.json" \
  test "$(endpoints_of "$VER/tauri.conf.json")" = "[\"https://publikhq.com/api/updates/$SLUG/latest.json\"]"
if node scripts/updater-latest-json.mjs --check "$VER/latest.json" --config "$VER/tauri.conf.json" --version "$V" --base-url "$BASE_URL" \
  --platform "darwin-aarch64=$VER/$TGZ" --platform "windows-x86_64=$VER/$EXE" 2>&1 | sed 's/^/        /'; then
  ok "latest.json: version $V, absolute URLs, every signature (and latest.json.sig) verifies with the app's public key"
else
  bad "latest.json and its signatures"
fi
expect "latest.json.sig is there" test -s "$VER/latest.json.sig"
expect "Windows installer version resource is $V" node scripts/exe-version.mjs "$VER/$EXE" --expect "$V"
if [ -n "$(get_state win_sha)" ]; then
  expect "Windows installer is the CI build, byte for byte" test "$(sha "$VER/$EXE")" = "$(get_state win_sha)"
fi

# The previous release's executable, streamed from its public download URL (no file kept but the
# binary): its designated requirement and its app token are the reference.
PREV_TGZ_URL="https://github.com/$REPO/releases/download/$PREV_TAG/${APP}_${PREV_TAG#v}_aarch64.app.tar.gz"
PREV_BIN="$VER/prev/$MAIN_EXE"
if curl -fsSL "$PREV_TGZ_URL" | tar -xzf - -C "$VER/prev" -s "|^$APP.app/Contents/MacOS/$MAIN_EXE\$|$MAIN_EXE|" "$APP.app/Contents/MacOS/$MAIN_EXE" && [ -f "$PREV_BIN" ]; then
  PREV_DR="$(dr "$PREV_BIN")"
  ok "$PREV_TAG executable read from $PREV_TGZ_URL"
else
  PREV_DR=""
  bad "$PREV_TAG executable could not be read from $PREV_TGZ_URL"
fi

# The dmg and the app in it, and the app of the update bundle.
SPCTL_DMG="$(spctl -a -vv -t open --context context:primary-signature "$VER/$DMG" 2>&1 || true)"
expect "dmg: Gatekeeper accepts it (Notarized Developer ID)" sh -c 'printf "%s\n" "$1" | grep -q ": accepted" && printf "%s\n" "$1" | grep -q "source=Notarized Developer ID"' _ "$SPCTL_DMG"
expect "dmg: notarization ticket stapled" stapled "$VER/$DMG"
expect "dmg: signed by $IDENTITY" sh -c 'codesign -dvv "$1" 2>&1 | grep -qxF "Authority=$2"' _ "$VER/$DMG" "$IDENTITY"
hdiutil attach -nobrowse -noautoopen -readonly -mountpoint "$VER/mnt" "$VER/$DMG" >/dev/null
trap 'hdiutil detach "$VER/mnt" -quiet 2>/dev/null || true' EXIT
COPYFILE_DISABLE=1 tar -xzf "$VER/$TGZ" -C "$VER/tgz"
expect "update bundle: no AppleDouble (._) files" sh -c '! tar -tzf "$1" | grep -q "/\._"' _ "$VER/$TGZ"
expect "update bundle: one top folder, $APP.app" test "$(tar -tzf "$VER/$TGZ" | cut -d/ -f1 | sort -u)" = "$APP.app"
PREV_TOKEN="$( [ -f "$PREV_BIN" ] && token_hash "$PREV_BIN" || true)"
for where in "dmg:$VER/mnt/$APP.app" "update bundle:$VER/tgz/$APP.app"; do
  w="${where%%:*}"
  a="${where#*:}"
  info="$(codesign -dvv "$a" 2>&1)"
  expect "$w app: Info.plist version $V" test "$(plist "$a" CFBundleShortVersionString)" = "$V"
  expect "$w app: bundle id $BUNDLE_ID" test "$(plist "$a" CFBundleIdentifier)" = "$BUNDLE_ID"
  expect "$w app: codesign --verify --strict --deep" codesign --verify --strict --deep "$a"
  expect "$w app: $IDENTITY, team $TEAM, hardened runtime" sh -c 'printf "%s\n" "$1" | grep -qxF "Authority=$2" && printf "%s\n" "$1" | grep -qxF "TeamIdentifier=$3" && printf "%s\n" "$1" | grep -q "flags=.*runtime"' _ "$info" "$IDENTITY" "$TEAM"
  expect "$w app: the microphone entitlement" sh -c 'codesign -d --entitlements - --xml "$1" 2>/dev/null | grep -q com.apple.security.device.audio-input' _ "$a"
  expect "$w app: the worker is signed by the same team, hardened runtime" sh -c 'i="$(codesign -dvv "$1/Contents/MacOS/whimpr-llm-worker" 2>&1)" && printf "%s\n" "$i" | grep -qxF "TeamIdentifier=$2" && printf "%s\n" "$i" | grep -q "flags=.*runtime"' _ "$a" "$TEAM"
  expect "$w app: notarization ticket stapled" stapled "$a"
  SPCTL_APP="$(spctl -a -vv -t exec "$a" 2>&1 || true)"
  expect "$w app: Gatekeeper accepts it (Notarized Developer ID)" sh -c 'printf "%s\n" "$1" | grep -q ": accepted" && printf "%s\n" "$1" | grep -q "source=Notarized Developer ID"' _ "$SPCTL_APP"
  expect "$w app: designated requirement as in $PREV_TAG (TCC grants carry over)" test -n "$PREV_DR" -a "$(dr "$a")" = "$PREV_DR"
  BIN="$a/Contents/MacOS/$(plist "$a" CFBundleExecutable)"
  expect "$w app: one publik app token compiled in, the same as in $PREV_TAG (compared by hash)" \
    test "$(token_count "$BIN")" = 1 -a -n "$PREV_TOKEN" -a "$(token_hash "$BIN")" = "$PREV_TOKEN"
done
CD_DMG="$(codesign -dvvv "$VER/mnt/$APP.app" 2>&1 | sed -n 's/^CDHash=//p')"
CD_TGZ="$(codesign -dvvv "$VER/tgz/$APP.app" 2>&1 | sed -n 's/^CDHash=//p')"
expect "the update bundle is the app of the dmg (CDHash $CD_DMG)" test -n "$CD_DMG" -a "$CD_DMG" = "$CD_TGZ"
echo "  designated requirement: $(dr "$VER/tgz/$APP.app")"
hdiutil detach "$VER/mnt" -quiet
trap - EXIT
if [ "$RSTATE" = draft ]; then
  expect "the draft targets ${SHA:0:12}" test "$(ghr api "repos/$REPO/releases" -q ".[] | select(.tag_name == \"$TAG\") | .target_commitish")" = "$SHA"
fi
if [ "$RSTATE" = draft ]; then [ "$FAILS" -eq 0 ] || die "$FAILS check(s) failed. $TAG stays a draft: fix the cause and run the command again."; fi
[ "$FAILS" -eq 0 ] || die "$FAILS check(s) failed on the published $TAG."
ok "every check passed"

# ---------------------------------------------------------------------------------------------
if [ "$RSTATE" = draft ]; then
  if [ "${PUBLIK_RELEASE_NO_PUBLISH:-0}" = 1 ]; then
    rm -rf "$VER"
    say "Not published (PUBLIK_RELEASE_NO_PUBLISH=1): https://github.com/$REPO/releases (draft $TAG)"
    exit 0
  fi
  say "Publish $TAG"
  gh release edit "$TAG" -R "$REPO" --draft=false --latest >/dev/null
  git fetch -q origin tag "$TAG"
  expect "tag $TAG points at ${SHA:0:12}" test "$(git rev-parse "$TAG^{commit}")" = "$SHA"
  expect "the release is public" test "$(ghr release view "$TAG" -R "$REPO" --json isDraft -q .isDraft)" = false
  for url in $(node -p 'Object.values(require(process.argv[1]).platforms).map((p) => p.url).join("\n")' "$VER/latest.json") "${BASE_URL}latest.json" "${BASE_URL}latest.json.sig" "${BASE_URL}$DMG"; do
    code="$(curl -s -o /dev/null -L -r 0-0 -w '%{http_code}' "$url")"
    expect "$url answers ($code)" test "$code" = 200 -o "$code" = 206
  done
  [ "$FAILS" -eq 0 ] || die "$FAILS check(s) failed after publishing $TAG: look at https://github.com/$REPO/releases/tag/$TAG"
  rm -rf "$WORK"
  say "Published: https://github.com/$REPO/releases/tag/$TAG"
  echo "publik takes the release in at its next daily intake (10:00 UTC) and then serves its"
  echo "latest.json at https://publikhq.com/api/updates/$SLUG/latest.json."
else
  rm -rf "$VER"
  say "$TAG is published and passes every check: https://github.com/$REPO/releases/tag/$TAG"
fi
