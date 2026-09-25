#!/bin/bash
# Run WhimprFlow in development: builds the local-LLM worker (tauri dev only
# builds the app crate), then starts the Vite UI server + the app with hot reload.
# The app loads its UI from the dev server, so the pill actually renders.
set -e
cd "$(dirname "$0")"
echo "[dev] building the local-LLM worker…"
cargo build -p whimpr-llm-worker
exec ui/node_modules/.bin/tauri dev "$@"
