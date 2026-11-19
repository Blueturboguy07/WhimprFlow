#!/usr/bin/env node
// Prints the version a Windows exe carries in its version resource (the ProductVersion string,
// as Explorer shows it), so a Mac can check the Windows installer of a release without Windows.
// Node 18+, no packages. scripts/publik-release.sh runs it on the downloaded setup.exe.
//
// Usage: node scripts/exe-version.mjs <file.exe> [--expect <version>]
// Out:   one line: "<ProductVersion> (fixed <a.b.c.d>)". With --expect, exit 1 unless the
//        ProductVersion is <version> and the fixed product version is <version>.0.
import { readFileSync } from "node:fs";

const [file, flag, expect] = process.argv.slice(2);
if (!file || (flag && flag !== "--expect") || (flag && !expect)) die("usage: exe-version.mjs <file.exe> [--expect <version>]");
const buf = readFileSync(file);
if (buf.subarray(0, 2).toString("latin1") !== "MZ") die(`${file} is not a Windows executable`);

const u16 = (s) => Buffer.from(s, "utf16le");
// The first VS_VERSIONINFO block is the exe's own (an installer's payload is compressed after it).
const info = buf.indexOf(u16("VS_VERSION_INFO\0"));
if (info < 0) die(`${file} has no version resource`);
const fixedAt = buf.indexOf(Buffer.from([0xbd, 0x04, 0xef, 0xfe]), info);
if (fixedAt < 0 || fixedAt - info > 64) die(`${file}: no VS_FIXEDFILEINFO after VS_VERSION_INFO`);
const ms = buf.readUInt32LE(fixedAt + 16);
const ls = buf.readUInt32LE(fixedAt + 20);
const fixed = `${ms >>> 16}.${ms & 0xffff}.${ls >>> 16}.${ls & 0xffff}`;
const product = stringValue("ProductVersion");
console.log(`${product ?? "(no ProductVersion)"} (fixed ${fixed})`);
if (expect) {
  const want = expect.replace(/^v/, "");
  if (product !== want || fixed !== `${want}.0`) die(`${file} carries version ${product} (fixed ${fixed}), not ${want}`);
}

// A String entry: key (UTF-16, NUL), padding to 32 bits, then the value (UTF-16, NUL).
function stringValue(key) {
  let i = buf.indexOf(u16(`${key}\0`), info);
  if (i < 0) return null;
  i += (key.length + 1) * 2;
  while (i % 4) i++;
  let end = i;
  while (end + 1 < buf.length && buf.readUInt16LE(end) !== 0) end += 2;
  return buf.subarray(i, end).toString("utf16le");
}

function die(msg) { console.error(`exe-version: ${msg}`); process.exit(1); }
