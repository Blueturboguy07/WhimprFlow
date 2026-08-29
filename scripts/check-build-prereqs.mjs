#!/usr/bin/env node
// Preflight check for the #1 first-build failure on Windows: a too-new
// LLVM/clang breaking bindgen 0.69 (used by whisper-rs-sys) with a confusing
// error buried at the end of a multi-minute `cargo build`. See
// docs/BUILD-PREREQUISITES.md for the full explanation and the fix.
//
// Plain Node (no deps) so it runs the same way on macOS and Windows with the
// toolchain this repo already requires — no separate .sh/.ps1 to keep in sync.
//
// Usage:
//   node scripts/check-build-prereqs.mjs
//
// Exit code 0 = safe to build, 1 = will fail partway through `cargo build`.
//
// Testing hook (not for normal use): set WHIMPR_FAKE_CLANG_VERSION to a
// version string (e.g. "22.0.0" or "18.1.1") to exercise both outcomes
// without needing a real LLVM install of that version.

import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";

// bindgen 0.69.5 (pinned transitively by whisper-rs-sys, see Cargo.lock) is
// the ceiling; LLVM/clang releases at or below this major version are known
// to work, and 19+ has broken it in the field.
const MAX_SAFE_CLANG_MAJOR = 18;

function color(code, s) {
  return process.stdout.isTTY ? `\x1b[${code}m${s}\x1b[0m` : s;
}
const red = (s) => color(31, s);
const green = (s) => color(32, s);
const yellow = (s) => color(33, s);
const bold = (s) => color(1, s);

function findClangOnPath() {
  const finder = process.platform === "win32" ? "where" : "which";
  try {
    const out = execFileSync(finder, ["clang"], { encoding: "utf8" });
    return out.split(/\r?\n/).map((s) => s.trim()).filter(Boolean)[0] ?? null;
  } catch {
    return null;
  }
}

function clangVersionString(clangPath) {
  return execFileSync(clangPath, ["--version"], { encoding: "utf8" });
}

/** Parse "clang version 22.0.0" / "Apple clang version 16.0.0 ..." etc. */
function parseClangVersion(versionOutput) {
  const isApple = /Apple (clang|LLVM)/i.test(versionOutput);
  const m = versionOutput.match(/(?:clang|LLVM) version (\d+)\.(\d+)\.(\d+)/i);
  if (!m) return { isApple, major: null };
  return { isApple, major: Number(m[1]) };
}

function main() {
  console.log(bold("WhimprFlow build preflight: LLVM/clang version check"));
  console.log("");

  const fake = process.env.WHIMPR_FAKE_CLANG_VERSION;
  let versionOutput;
  let source;

  if (fake) {
    versionOutput = `clang version ${fake}\n(simulated via WHIMPR_FAKE_CLANG_VERSION for testing)`;
    source = "WHIMPR_FAKE_CLANG_VERSION (test override)";
  } else if (process.env.LIBCLANG_PATH && existsSync(process.env.LIBCLANG_PATH)) {
    // LIBCLANG_PATH is set — most likely the documented libclang side-load
    // workaround (a bare .dll with no clang binary next to it), or a
    // hand-picked LLVM install. Either way the user has already pinned it
    // deliberately, so trust it rather than second-guessing via PATH.
    console.log(
      `${green("PASS")}  LIBCLANG_PATH is set (${process.env.LIBCLANG_PATH}) — trusting the pinned libclang.`,
    );
    console.log("");
    process.exit(0);
  } else {
    const clangPath = findClangOnPath();
    if (!clangPath) {
      console.log(
        `${yellow("WARN")}  No 'clang' found on PATH and LIBCLANG_PATH is unset. This is only a ` +
          "problem once a build actually needs bindgen (whisper-rs-sys) — if CMake/MSVC find " +
          "their own copy, or you haven't reached that build step yet, you're fine. If the " +
          "build later fails inside whisper-rs-sys, see docs/BUILD-PREREQUISITES.md.",
      );
      process.exit(0);
    }
    try {
      versionOutput = clangVersionString(clangPath);
      source = clangPath;
    } catch (e) {
      console.log(`${yellow("WARN")}  Found clang at ${clangPath} but couldn't run it (${e.message}).`);
      process.exit(0);
    }
  }

  const { isApple, major } = parseClangVersion(versionOutput);
  console.log(`  clang source : ${source}`);
  console.log(`  version      : ${versionOutput.split("\n")[0].trim()}`);
  console.log("");

  if (isApple) {
    console.log(`${green("PASS")}  Apple's bundled clang (Xcode Command Line Tools) — not the LLVM.org build, unaffected.`);
    process.exit(0);
  }

  if (major === null) {
    console.log(`${yellow("WARN")}  Couldn't parse a version number out of clang's output — skipping the check.`);
    process.exit(0);
  }

  if (major > MAX_SAFE_CLANG_MAJOR) {
    console.log(
      `${red("FAIL")}  clang/LLVM ${major}.x is newer than bindgen 0.69 supports (max known-good: ` +
        `LLVM ${MAX_SAFE_CLANG_MAJOR}.x).`,
    );
    console.log("");
    console.log("  This WILL fail partway through `cargo build` / `tauri build`, deep inside");
    console.log("  whisper-rs-sys's build script, after several minutes of compiling.");
    console.log("");
    console.log("  Fix — pick one:");
    console.log(`    1. Install LLVM 18.1.x instead of "latest": `);
    console.log("       https://github.com/llvm/llvm-project/releases/tag/llvmorg-18.1.8");
    console.log("    2. Keep your current LLVM and side-load a compatible libclang:");
    console.log("         pip install libclang==18.1.1");
    console.log("         $env:LIBCLANG_PATH = \"<site-packages>\\libclang\\native\"");
    console.log("");
    console.log("  Full details: docs/BUILD-PREREQUISITES.md");
    process.exit(1);
  }

  console.log(`${green("PASS")}  clang/LLVM ${major}.x is within bindgen 0.69's supported range.`);
  process.exit(0);
}

main();
