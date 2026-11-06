#!/usr/bin/env python3
"""CI check for the publik usage counts (telemetry v1) in a built WhimprFlow.

It starts the app two times against a local listener, never against publikhq.com:
  1. with DO_NOT_TRACK=1: no request arrives and no telemetry.json is made;
  2. with the defaults: exactly one correct `app_opened` arrives.
The app must stay up both times. Standard library only.

Usage: python3 .github/scripts/publik-telemetry-smoke.py <app executable> [result.json]
"""
import json
import os
import re
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

SLUG = "whimprflow"
ROOT = Path(__file__).resolve().parents[2]
VERSION = re.search(r'^version = "([^"]+)"', (ROOT / "Cargo.toml").read_text(encoding="utf-8"), re.M).group(1)
OS_NAME = {"darwin": "macos", "win32": "windows"}.get(sys.platform, "linux")
ALLOWED = {"v", "app", "install_id", "event", "app_version", "os", "arch"}
UUID4 = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")
UP_SECONDS = 12  # how long the app must stay up in each run

seen = []
lock = threading.Lock()


class Handler(BaseHTTPRequestHandler):
    def _handle(self):
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length).decode("utf-8", "replace") if length else ""
        with lock:
            seen.append({
                "method": self.command,
                "path": self.path,
                "content_type": self.headers.get("Content-Type"),
                "origin": self.headers.get("Origin"),
                "has_authorization": self.headers.get("Authorization") is not None,  # the value is never kept
                "body": body,
            })
        self.send_response(204)
        self.send_header("Content-Length", "0")
        self.end_headers()

    do_POST = do_GET = do_PUT = do_OPTIONS = do_HEAD = _handle

    def log_message(self, *args):
        pass


def publik_dir():
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Application Support" / "publik"
    if sys.platform == "win32":
        return Path(os.environ["LOCALAPPDATA"]) / "publik"
    return Path(os.environ.get("XDG_CONFIG_HOME") or Path.home() / ".config") / "publik"


def stop(proc):
    if proc.poll() is not None:
        return
    if sys.platform == "win32":
        subprocess.run(["taskkill", "/PID", str(proc.pid), "/T", "/F"], capture_output=True)
    else:
        proc.terminate()
    try:
        proc.wait(10)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(10)


def run(exe, url, extra_env):
    """Runs the app for UP_SECONDS. Returns (stayed up, requests, log text)."""
    with lock:
        del seen[:]
    drop = ("DO_NOT_TRACK", "PUBLIK_TELEMETRY", "PUBLIK_API_KEY", "PUBLIK_TELEMETRY_INTERVAL_SECONDS")
    env = {name: value for name, value in os.environ.items() if name not in drop}
    env.update({"PUBLIK_TELEMETRY_URL": url, "PUBLIK_TELEMETRY_DEBUG": "1"})
    env.update(extra_env)
    with tempfile.TemporaryFile() as log:
        proc = subprocess.Popen([exe], env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT)
        deadline = time.time() + UP_SECONDS
        while time.time() < deadline and proc.poll() is None:
            time.sleep(0.2)
        stayed_up = proc.poll() is None
        stop(proc)
        time.sleep(3)  # Windows: let the single-instance lock and the web view go
        log.seek(0)
        text = log.read().decode("utf-8", "replace")
    with lock:
        return stayed_up, list(seen), text


def problems_of(request):
    out = []
    if request["method"] != "POST":
        out.append("method is %s" % request["method"])
    if request["path"] != "/api/v1/telemetry":
        out.append("path is %s" % request["path"])
    if (request["content_type"] or "").split(";")[0].strip().lower() != "application/json":
        out.append("Content-Type is %r" % request["content_type"])
    if request["origin"] is not None:
        out.append("an Origin header came with it (the server drops such an event)")
    if request["has_authorization"]:
        out.append("an Authorization header came with it, and this computer has no publik key")
    try:
        body = json.loads(request["body"])
    except ValueError:
        return out + ["the body is not JSON"]
    if not isinstance(body, dict):
        return out + ["the body is not a JSON object"]
    want = {"v": 1, "app": SLUG, "event": "app_opened", "app_version": VERSION, "os": OS_NAME}
    for name, value in want.items():
        if body.get(name) != value or type(body.get(name)) is not type(value):
            out.append("%s is %r, expected %r" % (name, body.get(name), value))
    if set(body) - ALLOWED:
        out.append("extra fields: %s" % sorted(set(body) - ALLOWED))
    if not UUID4.match(str(body.get("install_id", ""))):
        out.append("install_id is not a lowercase UUID v4")
    if "arch" in body and body["arch"] not in ("arm64", "x64"):
        out.append("arch is %r" % body["arch"])
    return out


def main():
    exe = sys.argv[1]
    result_file = sys.argv[2] if len(sys.argv) > 2 else None
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    url = "http://127.0.0.1:%d/api/v1/telemetry" % server.server_address[1]
    id_file = publik_dir() / "telemetry.json"
    had_id_file = id_file.exists()
    failures = []
    print("app: %s\nexpected: app=%s app_version=%s os=%s\nlistener: %s" % (exe, SLUG, VERSION, OS_NAME, url))

    print("\n1. DO_NOT_TRACK=1")
    up, got, text = run(exe, url, {"DO_NOT_TRACK": "1"})
    off_line = "publik-telemetry: app_opened -> off (opt-out)" in text
    if not up:
        failures.append("opt-out run: the app did not stay up for %d s" % UP_SECONDS)
    if got:
        failures.append("opt-out run: %d request(s) arrived" % len(got))
    if not had_id_file and id_file.exists():
        failures.append("opt-out run: telemetry.json was made")
    print("   stayed up: %s, requests: %d, telemetry.json made: %s, debug line 'off (opt-out)': %s" % (up, len(got), (not had_id_file) and id_file.exists(), off_line))
    opt_out = {"stayed_up": up, "requests": len(got), "debug_line_off": off_line}

    print("\n2. defaults")
    up, got, text = run(exe, url, {})
    if not up:
        failures.append("normal run: the app did not stay up for %d s" % UP_SECONDS)
    if len(got) != 1:
        failures.append("normal run: %d request(s) arrived, expected exactly 1" % len(got))
    for request in got:
        failures.extend("normal run: " + problem for problem in problems_of(request))
        print("   request: %s %s %s" % (request["method"], request["path"], request["body"]))
    debug_lines = [line.strip() for line in text.splitlines() if "publik-telemetry:" in line]
    print("   stayed up: %s, requests: %d, debug lines: %s" % (up, len(got), debug_lines))
    try:
        saved = json.loads(id_file.read_text(encoding="utf-8")).get("install_id")
    except (OSError, ValueError):
        saved = None
    if got and saved != json.loads(got[0]["body"]).get("install_id"):
        failures.append("normal run: the install_id sent is not the one in telemetry.json")
    if failures:
        print("\n--- app output (first 40 lines) ---")
        print("\n".join(text.splitlines()[:40]))

    if result_file:
        Path(result_file).write_text(json.dumps({
            "app": SLUG, "expected_version": VERSION, "os": OS_NAME, "opt_out_run": opt_out,
            "normal_run": {"stayed_up": up, "requests": got, "debug_lines": debug_lines},
            "failures": failures,
        }, indent=2) + "\n", encoding="utf-8")
    print()
    for failure in failures:
        print("FAIL  " + failure)
    print("RESULT: %s" % ("FAIL" if failures else "PASS"))
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
