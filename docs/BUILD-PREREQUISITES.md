# Build prerequisites

Both platforms need Rust (stable), Node + pnpm, and CMake. Windows additionally
needs the pieces below spelled out exactly, because the two most common
first-build failures are LLVM/clang version skew (this page) and a pnpm
install quirk (see the Windows section of the README).

## LLVM / libclang version — the one that bites on Windows

`whisper-rs-sys` (Whisper bindings) generates its Rust FFI bindings at build
time with **bindgen 0.69**, which needs a **libclang.dll not newer than
LLVM/clang 18.x**. Point your LLVM installer at the "latest" release and you
get whatever LLVM ships that week — as of this writing that's LLVM 22, and
bindgen 0.69 cannot parse its output. The build fails deep in `cargo build`,
after minutes of compiling everything else, with an error inside
`whisper-rs-sys`'s build script (a bindgen/clang parse failure, not an
obviously-LLVM-shaped message) — reported in the wild as "the Windows
`tauri build` gets to the last ~20 crates and fails."

**Run the preflight check before you build** — it catches this in under a
second instead of after a multi-minute compile:

```powershell
node scripts/check-build-prereqs.mjs
```

### If you don't have LLVM installed yet

Install **LLVM 18.1.x specifically** — not whatever the download page defaults
to. Grab an `LLVM-18.1.x-win64.exe` installer from the
[LLVM 18.1.8 release page](https://github.com/llvm/llvm-project/releases/tag/llvmorg-18.1.8)
(or any 18.1.x patch release), check "Add LLVM to the system PATH", and
install. Confirm with `clang --version` — it should print `18.1.x`.

### If you already have a newer LLVM installed

The Windows LLVM installer refuses to install a second copy alongside an
existing one (the exact "LLVM installer won't co-exist with another LLVM"
problem reported against this project) — don't uninstall your existing LLVM
just for this. Side-load a compatible `libclang.dll` instead, without
touching your system LLVM at all:

```powershell
pip install libclang==18.1.1
```

This drops a working `libclang.dll` at
`<your Python site-packages>\libclang\native\libclang.dll` — find the exact
path with:

```powershell
python -c "import libclang, os; print(os.path.dirname(libclang.__file__) + r'\native')"
```

Then point the build at it for the current shell session before running
`cargo build` / `tauri build`:

```powershell
$env:LIBCLANG_PATH = "<path printed above>"
```

(Set it as a permanent user environment variable — System Properties →
Environment Variables — if you don't want to re-export it every session.)

### Why not just pin a newer bindgen?

`whisper-rs-sys` pins bindgen itself; this repo doesn't control that version.
A newer whisper-rs release may lift the ceiling eventually — until then, the
libclang side-load above is the reliable fix, and it's what `check-build-prereqs.mjs`
verifies.

## Everything else

See the "Build (macOS)" / "Build (Windows)" sections in the [README](../README.md)
for Rust/Node/CMake/Visual Studio Build Tools setup, and
[MODELS.md](MODELS.md) for which speech model to download.
