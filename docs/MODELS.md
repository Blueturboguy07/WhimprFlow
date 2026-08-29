# Which model do I download?

WhimprFlow does **not** download models for you — you place them by hand in a
folder, and the app picks up whichever ones it finds. This page exists
because "it tells me to download a ggml-base speech model, which one do I
download?" was a real support report: the Hugging Face pages this points at
list dozens of files, and only a few exact names are ones WhimprFlow actually
looks for.

There are two *separate* models. **You only ever need the first one.**

| Model | What it's for | Required? |
|---|---|---|
| Whisper (`ggml-*.bin`) | Speech-to-text — turns your voice into a raw transcript | **Yes** |
| Qwen (`*.gguf`) | Optional *local, offline* cleanup pass (fillers, punctuation, self-corrections) | **No** — skip this and either use the cloud Cleanup Engine (OpenAI/Anthropic/OpenRouter key in Settings) or set Auto Cleanup to **None** for raw, verbatim text. Nothing extra to download either way. |

If dictation types nothing at all, that's not a missing model problem — see
the Troubleshooting section in the README first (Accessibility permission on
macOS, or the loud in-app error the pill/Hub now show).

## 1. Speech-to-text (Whisper) — required

Download **one** file and put it in your models folder:

**Recommended default — `ggml-base.en.bin` (148 MB, English-only, fastest to load):**

```
https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin
```

Save it with that exact name, no renaming needed. Place it at:

- **macOS**: `~/Library/Application Support/WhimprFlow/models/ggml-base.en.bin`
- **Windows**: `%APPDATA%\WhimprFlow\models\ggml-base.en.bin`

That's it — restart WhimprFlow and dictation works.

### Want better accuracy? (optional)

WhimprFlow checks for a short list of exact filenames, in this order, and
uses the first (best) one it finds — so a bigger model doesn't need any
settings change, just drop it in the same folder alongside or instead of the
base one:

| File (exact name) | Size | Platform | Notes |
|---|---|---|---|
| `ggml-large-v3-turbo.bin` | 1.62 GB | macOS only | Best accuracy, still fast on Apple Silicon (Metal) |
| `ggml-medium.en.bin` | 1.53 GB | macOS + Windows | Noticeably better than base, slower |
| `ggml-small.en.bin` | 488 MB | macOS + Windows | Good middle ground |
| `ggml-base.en.bin` | 148 MB | macOS + Windows | **The recommended default above** |

All are official builds from the same repo — swap `ggml-base.en.bin` for any
of the names above in the URL pattern:

```
https://huggingface.co/ggerganov/whisper.cpp/resolve/main/<exact filename>
```

Any other filename in that repo (quantized `-q5_1`/`-q8_0` variants, non-`.en`
multilingual models, CoreML `.mlmodelc.zip` bundles, `tiny.en`) is **not**
recognized by WhimprFlow today — renaming one of those to a name in the table
above will not work; the file format has to match too, not just the name.

## 2. Local cleanup LLM (Qwen, GGUF) — optional

Skip this section unless you specifically want cleanup to run fully
on-device/offline instead of via a cloud API key. Everything else in
WhimprFlow (recording, transcription, pasting) works without it.

Download **one** file, then **rename it to the exact lowercase name shown**
(Hugging Face's own filenames use mixed case; WhimprFlow's default is
case-sensitive on Linux even though macOS/Windows aren't, so renaming avoids
any ambiguity), and place it in the same models folder as the Whisper model
above.

**Smaller / faster — `qwen2.5-1.5b-instruct-q4_k_m.gguf` (986 MB):**

```
https://huggingface.co/bartowski/Qwen2.5-1.5B-Instruct-GGUF/resolve/main/Qwen2.5-1.5B-Instruct-Q4_K_M.gguf
```

**Better quality, needs more RAM — `qwen3-4b-instruct-2507-q4_k_m.gguf` (2.5 GB):**

```
https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF/resolve/main/Qwen3-4B-Instruct-2507-Q4_K_M.gguf
```

If both are present, WhimprFlow prefers the 4B one. Then in the Hub, set
**Settings → Cleanup Engine → Local**.

## Checking it worked

Launch WhimprFlow from a terminal (`./dev.sh` on macOS, or run the built
`.exe` from PowerShell on Windows) and hold your dictation key. You should see:

```
[whimpr] ASR model loaded — ready to transcribe
```

at startup. If instead you see `ASR model not found at <path>`, the filename
or folder doesn't match exactly what's listed above — re-check both.
