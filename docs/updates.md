# Updates

WhimprFlow updates itself with `tauri-plugin-updater`. Every update package is signed with the
publik update key. The app checks each package against the public key in
`src-tauri/tauri.conf.json` before it installs anything.

## How it works

- The app reads `https://publikhq.com/api/updates/whimprflow/latest.json`. publik serves the
  `latest.json` asset of the scan-approved release tag, byte for byte. publik does not sign or
  change it, so the release `latest.json` must be complete and correct. The route answers 404
  until a scan-approved release carries a `latest.json` asset. A 404 is silent in the background
  check, and a manual check shows it as an error.
- The app checks 10 s after launch and then once a day. "Check for Updates…" in the tray menu
  checks at once. `PUBLIK_NO_AUTO_UPDATE=1` turns off the automatic checks (the CI workflows set it).
- macOS: the app downloads the new `WhimprFlow.app`, verifies it, puts it in place of the old
  one, and asks "Restart" or "Later". If the user cannot write to the app's folder, the app does
  not replace it in the background. It asks "Install and Restart" or "Later", and macOS asks for
  an admin password only after "Install and Restart".
- Windows: the app downloads the installer, verifies it, and asks "Restart and Install" or
  "Later". The installer runs with a progress bar only, then opens WhimprFlow again.
- The signature must also name the version that `latest.json` announces
  (`requireSignedVersion`). The app never installs an older version.
- A failed automatic check goes to the log (stderr, `[whimpr] updates:`) only. A failed manual
  check shows the error.
- The microphone and Accessibility grants stay after an update on macOS because the update
  bundle is the stapled Developer ID app itself: same bundle id, same certificate, same
  designated requirement.
- Code: `src-tauri/src/updates.rs`. Proof: `.github/workflows/updater-proof.yml`.

## The key

| Where | What |
|---|---|
| Release Mac, login keychain, service `publik-update-key` | `mann-private` (private key), `mann-password` (its password), `mann-public` (public key) |
| GitHub secrets on this repo | `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (used by `windows.yml`, `publik-build.yml`, `release.yml` and `updater-proof.yml`) |
| `src-tauri/tauri.conf.json` | `plugins.updater.pubkey` (public, key ID `F5E0515C77912BD0`) |

Keep a backup of the private key and its password. Without them, installed copies can never
update again.

## Which build makes which update file

`tauri.conf.json` leaves `bundle.createUpdaterArtifacts` off, so a source build (the README
steps, the publik install guide) needs no key and makes no update files.

| Build | Update files |
|---|---|
| `scripts/build-macos.sh`, notarized | `target/release/bundle/updater/WhimprFlow_<v>_aarch64.app.tar.gz` + `.sig`, packed by `scripts/updater-pack-macos.sh` from the stapled app (the key comes from the env or the keychain) |
| `scripts/build-macos.sh --skip-notarize` | none |
| `publik-build.yml`, macOS job | none: the app is unsigned. On the release Mac, after stapling, run `scripts/updater-pack-macos.sh <WhimprFlow.app> <out>` |
| `windows.yml`, `publik-build.yml` Windows job | `WhimprFlow_<v>_x64-setup.exe.sig` next to the installer (`--config tauri.updater.conf.json` and the key secret) |

Do not turn `createUpdaterArtifacts` on for a macOS build: every macOS build here is re-signed
after `tauri build` (the local-LLM worker is copied in), so a bundle that `tauri build` packed
would not be the app that users get.

## Cut a release

1. Set the new version in `Cargo.toml` (workspace), `src-tauri/tauri.conf.json` and
   `ui/package.json`. Push.
2. Run `windows.yml` (or `publik-build.yml`) on that commit. Its `whimprflow-windows-installer`
   artifact holds `WhimprFlow_<v>_x64-setup.exe` and `WhimprFlow_<v>_x64-setup.exe.sig`.
3. On the release Mac: `scripts/build-macos.sh` (see the README, "Releasing (macOS)"), then
   `scripts/verify-macos.sh --require-notarized`. You get the dmg and the update bundle with its
   `.sig`.
4. Make a draft release with all the files:
   ```sh
   gh release create v<v> --draft --title "WhimprFlow <v>" --notes-file notes.md \
     target/release/bundle/dmg/WhimprFlow_<v>_aarch64.dmg \
     target/release/bundle/updater/WhimprFlow_<v>_aarch64.app.tar.gz \
     target/release/bundle/updater/WhimprFlow_<v>_aarch64.app.tar.gz.sig \
     win/WhimprFlow_<v>_x64-setup.exe win/WhimprFlow_<v>_x64-setup.exe.sig SHA256SUMS.txt
   ```
5. Write the feed. The script refuses a signature that does not verify or names another version:
   ```sh
   scripts/updater-release-feed.sh v<v>
   ```
   It downloads the four update files from the release, writes `latest.json` with the absolute
   GitHub asset URLs and attaches it.
6. Publish the release. Installed copies update when publik approves the release and serves its
   `latest.json`.

CI path: `release.yml` (a `v*` tag, with the Apple secrets) attaches the dmg and the update
bundle. `windows.yml` (a published release, or a manual run with `release_tag`) attaches the
installer and its `.sig`. Each of them then runs `scripts/updater-release-feed.sh --if-complete`,
so the one that lands second writes `latest.json`. If the feed is still missing, run step 5.
`windows.yml` skips a release that already has an installer; it then writes no feed either.

## Roll back

The app installs only a version higher than its own. You cannot push an older version.

1. Stop the bad release at once: withdraw it in publik, or delete its `latest.json` asset.
   Copies that did not update yet stay on the old version.
2. Revert the bad change, give it a higher version (for example 0.2.9 after a bad 0.2.8), and cut
   a release. Copies on the bad version update to it.

Do not turn on `allowDowngrades`. It removes the only guard against an old, signed release
served as a new one.

## Rotate the key

The public key is compiled into each copy, so a new key reaches users only through an update.

1. Make a new key: `ui/node_modules/.bin/tauri signer generate -w new.key`. Store the private
   key, its password and the public key in the keychain items and the repo secrets (new names
   first, so the old key stays available).
2. Put the new public key in `tauri.conf.json`. Release this version signed with the OLD key.
3. From the next release on, sign with the new key. Copies that skipped the bridge release can
   still update only to a release signed with the old key, so keep the old key until you accept
   that those copies stop updating.
4. If the private key leaks, cut the bridge release at once. A leaked key alone cannot push an
   update: the attacker must also control the feed on publikhq.com.

## Test

`updater-proof.yml` (workflow_dispatch, or a push to `feat/updater` or `updater/**`) builds
0.0.1 and 0.0.2, installs 0.0.1 on `macos-latest` and `windows-latest`, and checks that 0.0.1
refuses three bad feeds and then updates to 0.0.2. On macOS it also checks that the bundle id,
the signing certificate and the designated requirement do not change. Then it starts 0.0.1 once
more and checks that the restart question is on screen while the app keeps running. Last, it
runs `scripts/updater-release-feed.sh` on the two 0.0.2 packages.

The macOS builds in the proof are signed with a throwaway certificate made on the runner, with
the codesign steps of `scripts/build-macos.sh`. The repo has no Developer ID secrets.

The app has a test hook for this: `PUBLIK_UPDATE_SELF_TEST=version|check|install` runs the
updater with no window, no sign-in gate, no microphone and no prompt, and prints one JSON line.
`prompt` runs the background path and shows the real question. The hook does not click the
buttons.
