#!/usr/bin/env node
// Writes the Tauri updater feed (latest.json, static format) for one release and checks every
// signature against the public key in tauri.conf.json before it writes anything.
// Node 18+, no packages. Copy it to another Tauri app as is.
//
// Usage:
//   node scripts/updater-latest-json.mjs \
//     --version 0.2.2 \
//     --base-url https://github.com/<owner>/<repo>/releases/download/v0.2.2/ \
//     --platform darwin-aarch64=out/WhimprFlow_0.2.2_aarch64.app.tar.gz \
//     --platform windows-x86_64=out/WhimprFlow_0.2.2_x64-setup.exe \
//     [--notes "text" | --notes-file CHANGES.md] [--pub-date 2026-11-18T00:00:00Z] \
//     [--config src-tauri/tauri.conf.json] [--out out/latest.json]
//
// Each file needs its signature next to it (<file>.sig, from `tauri build` or `tauri signer sign`).
// The url of each platform is --base-url + the file name, so for a GitHub release it is the
// absolute asset URL. Checks (any failure writes nothing and exits 1):
//   - the signature verifies (minisign Ed25519) over the file with the configured public key;
//   - the signature's trusted comment carries `version:<--version>` (requireSignedVersion);
//   - every url is absolute https (http only with --allow-http, for a local test feed).
//
// Check mode (scripts/publik-release.sh runs it on the files it downloads back from the draft):
//   node scripts/updater-latest-json.mjs --check verify/latest.json --version 0.2.3 \
//     --base-url https://github.com/<owner>/<repo>/releases/download/v0.2.3/ \
//     --platform darwin-aarch64=verify/WhimprFlow_0.2.3_aarch64.app.tar.gz \
//     --platform windows-x86_64=verify/WhimprFlow_0.2.3_x64-setup.exe
// Reads the feed instead of writing one: its version must be --version, it must list exactly
// these platforms, each url must be --base-url + the file name, each signature in the feed must
// verify over the file (and name the version) and equal <file>.sig when that file is there.
// <feed>.sig, when it is there, must verify over the feed itself. Writes nothing; exit 1 on a failure.
import { createHash, createPublicKey, verify } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const args = process.argv.slice(2);
const opt = { platform: [] };
for (let i = 0; i < args.length; i++) {
  const a = args[i];
  if (!a.startsWith("--")) die(`unexpected argument ${a}`);
  const key = a.slice(2);
  if (key === "allow-http") { opt.allowHttp = true; continue; }
  const val = args[++i];
  if (val === undefined) die(`${a} needs a value`);
  if (key === "platform") opt.platform.push(val); else opt[key] = val;
}
if (!opt.version) die("--version is required");
if (!opt["base-url"]) die("--base-url is required");
if (opt.platform.length === 0) die("at least one --platform target=file is required");
const version = opt.version.replace(/^v/, "");
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$/.test(version)) die(`--version ${opt.version} is not semver`);
const base = opt["base-url"].endsWith("/") ? opt["base-url"] : `${opt["base-url"]}/`;
const baseUrl = new URL(base);
if (baseUrl.protocol !== "https:" && !(opt.allowHttp && baseUrl.protocol === "http:")) die(`--base-url must be https: ${base}`);

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const configPath = opt.config ?? join(root, "src-tauri", "tauri.conf.json");
const pubkeyB64 = JSON.parse(readFileSync(configPath, "utf8"))?.plugins?.updater?.pubkey;
if (!pubkeyB64) die(`${configPath} has no plugins.updater.pubkey`);
const pub = parsePublicKey(pubkeyB64);

const checkFeed = opt.check ? JSON.parse(readFileSync(opt.check, "utf8")) : null;
if (checkFeed) {
  if (checkFeed.version !== version) die(`${opt.check} announces version ${checkFeed.version}, not ${version}`);
  if (Number.isNaN(Date.parse(checkFeed.pub_date))) die(`${opt.check} has no valid pub_date`);
  const extra = Object.keys(checkFeed.platforms ?? {}).filter((t) => !opt.platform.some((spec) => spec.startsWith(`${t}=`)));
  if (extra.length) die(`${opt.check} lists platforms that were not given: ${extra.join(", ")}`);
}

const platforms = {};
for (const spec of opt.platform) {
  const eq = spec.indexOf("=");
  if (eq < 1) die(`--platform ${spec}: use target=file, e.g. darwin-aarch64=out/App.app.tar.gz`);
  const target = spec.slice(0, eq);
  const file = spec.slice(eq + 1);
  if (!/^(darwin|windows|linux)-(aarch64|x86_64|i686|armv7)(-(app|nsis|msi|appimage|deb|rpm))?$/.test(target)) die(`unknown updater target ${target}`);
  const data = readFileSync(file);
  const url = new URL(encodeURIComponent(basename(file)), baseUrl).toString();
  let signature;
  if (checkFeed) {
    const entry = checkFeed.platforms?.[target];
    if (!entry) die(`${opt.check} has no ${target} entry`);
    if (entry.url !== url) die(`${opt.check}: the ${target} url is ${entry.url}, not ${url}`);
    signature = String(entry.signature ?? "").trim();
    if (existsSync(`${file}.sig`) && readFileSync(`${file}.sig`, "utf8").trim() !== signature) die(`${file}.sig is not the ${target} signature in ${opt.check}`);
  } else {
    signature = readFileSync(`${file}.sig`, "utf8").trim();
  }
  const signed = checkSignature(data, signature, pub, file);
  if (signed !== version) die(`the signature of ${basename(file)} names version ${signed ?? "(none)"}, not ${version}`);
  platforms[target] = { signature, url };
  console.error(`ok  ${target}  ${basename(file)}  (${data.length} bytes, key ${pub.keyId}, version ${signed})`);
}

if (checkFeed) {
  const feedSig = `${opt.check}.sig`;
  if (existsSync(feedSig)) {
    const signed = checkSignature(readFileSync(opt.check), readFileSync(feedSig, "utf8").trim(), pub, opt.check);
    if (signed !== version) die(`${feedSig} names version ${signed ?? "(none)"}, not ${version}`);
    console.error(`ok  feed  ${basename(feedSig)} verifies over ${basename(opt.check)}  (key ${pub.keyId}, version ${signed})`);
  } else {
    console.error(`--  feed  no ${basename(feedSig)} next to ${basename(opt.check)}`);
  }
  console.error(`ok  ${basename(opt.check)}: version ${version}, ${opt.platform.length} platform(s), every url and signature checked`);
  process.exit(0);
}

const notes = opt["notes-file"] ? readFileSync(opt["notes-file"], "utf8").trim() : (opt.notes ?? `Version ${version}`);
const pubDate = opt["pub-date"] ?? new Date().toISOString().replace(/\.\d{3}Z$/, "Z");
const feed = { version, notes, pub_date: pubDate, platforms };
const text = `${JSON.stringify(feed, null, 2)}\n`;
if (opt.out) { writeFileSync(opt.out, text); console.error(`wrote ${opt.out}`); } else process.stdout.write(text);

function die(msg) { console.error(`updater-latest-json: ${msg}`); process.exit(1); }

function b64(s) { return Buffer.from(s.trim(), "base64"); }

// minisign public key: "untrusted comment: ...\n" + base64("Ed" | key id (8) | key (32)).
function parsePublicKey(b64text) {
  const lines = b64(b64text).toString("utf8").trim().split("\n");
  const raw = b64(lines[lines.length - 1]);
  if (raw.length !== 42 || raw.subarray(0, 2).toString() !== "Ed") die("the configured pubkey is not a minisign Ed25519 key");
  const der = Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), raw.subarray(10)]);
  return { keyId: hexId(raw.subarray(2, 10)), id: raw.subarray(2, 10), key: createPublicKey({ key: der, format: "der", type: "spki" }) };
}

// minisign signature, base64 encoded as Tauri writes it:
//   untrusted comment / base64(alg (2) | key id (8) | sig (64)) / trusted comment / base64(global sig (64))
// "ED" signs the BLAKE2b-512 of the file, "Ed" the file itself. The global signature covers
// sig | trusted comment. Returns the signed version (trusted comment `version:`), if any.
function checkSignature(data, sigB64, pub, file) {
  const lines = b64(sigB64).toString("utf8").split("\n");
  if (lines.length < 4 || !lines[2].startsWith("trusted comment: ")) die(`${file}.sig is not a minisign signature`);
  const blob = b64(lines[1]);
  const alg = blob.subarray(0, 2).toString();
  const id = blob.subarray(2, 10);
  const sig = blob.subarray(10);
  if (blob.length !== 74) die(`${file}.sig: bad signature length`);
  if (!id.equals(pub.id)) die(`${file}.sig was made with key ${hexId(id)}, but tauri.conf.json has key ${pub.keyId}`);
  const message = alg === "ED" ? createHash("blake2b512").update(data).digest() : alg === "Ed" ? data : die(`${file}.sig: unknown algorithm ${alg}`);
  if (!verify(null, message, pub.key, sig)) die(`${file}.sig does not verify: the file or the signature is wrong`);
  const trusted = lines[2].slice("trusted comment: ".length);
  if (!verify(null, Buffer.concat([sig, Buffer.from(trusted, "utf8")]), pub.key, b64(lines[3]))) die(`${file}.sig: the trusted comment does not verify`);
  return trusted.split("\t").find((f) => f.startsWith("version:"))?.slice("version:".length);
}

// minisign prints key ids as the little-endian 64-bit number in hex.
function hexId(id) { return Buffer.from(id).reverse().toString("hex").toUpperCase(); }
