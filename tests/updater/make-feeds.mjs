#!/usr/bin/env node
// Test feeds for .github/workflows/updater-proof.yml. Reads the good feed (written by
// scripts/updater-latest-json.mjs) and writes three feeds the app must REFUSE:
//   latest-badsig.json    the genuine package, a signature made with another key
//   latest-tampered.json  the package with one byte changed, the genuine signature
//   latest-version.json   the genuine package and signature, announced as a higher version than
//                         the one the signature names (requireSignedVersion must refuse it)
// Usage: node tests/updater/make-feeds.mjs <feed dir> <target> <package file name> <foreign .sig>
import { copyFileSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [dir, target, name, foreignSig] = process.argv.slice(2);
if (!foreignSig) { console.error("usage: make-feeds.mjs <feed dir> <target> <package name> <foreign .sig>"); process.exit(1); }
const good = JSON.parse(readFileSync(join(dir, "latest-good.json"), "utf8"));
const entry = good.platforms[target];
if (!entry) { console.error(`latest-good.json has no ${target}`); process.exit(1); }
const write = (file, feed) => { writeFileSync(join(dir, file), `${JSON.stringify(feed, null, 2)}\n`); console.log(`wrote ${file}`); };
const withEntry = (patch, extra = {}) => ({ ...good, ...extra, platforms: { [target]: { ...entry, ...patch } } });

// 1. Another key.
write("latest-badsig.json", withEntry({ signature: readFileSync(foreignSig, "utf8").trim() }));

// 2. One byte changed in the middle of the package.
const tampered = `tampered-${name}`;
copyFileSync(join(dir, name), join(dir, tampered));
const bytes = readFileSync(join(dir, tampered));
const at = Math.floor(bytes.length / 2);
bytes[at] ^= 0x01;
writeFileSync(join(dir, tampered), bytes);
console.log(`flipped byte ${at} of ${bytes.length} in ${tampered}`);
write("latest-tampered.json", withEntry({ url: entry.url.replace(encodeURIComponent(name), encodeURIComponent(tampered)) }));

// 3. A version the signature does not name.
const [major, minor, patch] = good.version.split(/[.+-]/).map(Number);
write("latest-version.json", withEntry({}, { version: `${major}.${minor}.${patch + 1}` }));
