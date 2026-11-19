#!/usr/bin/env python3
"""A stand-in for publik's sign-in API on a runner (PUBLIK_ORIGIN=http://127.0.0.1:<port>), so a
live proof run never creates an install or a usage count in publik's database. It does NOT stand
in for the update feed: WhimprFlow reads that from https://publikhq.com, compiled in.

  fake-publik.py <port> unclaimed   the gate stays on "Sign in with publik" (nobody signs in)
  fake-publik.py <port> claimed     the gate lets the app through ("own key" chosen; the tray exists)

The same answers as the fake publik of scripts/verify-windows.ps1. No real key or token.
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

PORT = int(sys.argv[1])
STATE = sys.argv[2]
WALLET = {
    "claim_state": STATE,
    "balance_micros": 0,
    "plan": {"kind": "plan", "label": "CI plan"} if STATE == "claimed" else {"kind": "none", "label": None},
    "account": {"handle": "ci"} if STATE == "claimed" else None,
    "claim_url": "https://publikhq.com/claim/CI",
    "top_up_url": "https://publikhq.com/wallet",
}
INSTALL = {
    "key": "pk_ci_fake",
    "base_url": "http://127.0.0.1:%d/api/v1" % PORT,
    "install_id": "00000000-0000-4000-8000-000000000000",
    "claim_url": "https://publikhq.com/claim/CI",
}


class Handler(BaseHTTPRequestHandler):
    def _send(self, code, body=None):
        self.send_response(code)
        if body is None:
            self.end_headers()
            return
        data = json.dumps(body).encode()
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self._send(200, WALLET) if self.path == "/api/v1/wallet" else self._send(204)

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0))
        if self.path == "/api/v1/installs":
            # The running app names its own compiled-in version here. Only these fields are
            # logged: the request also carries the app token, which is never written anywhere.
            try:
                req = json.loads(body or b"{}")
            except ValueError:
                req = {}
            sys.stdout.write("install request: app_version=%s os=%s arch=%s\n" % (req.get("app_version"), req.get("os"), req.get("arch")))
            sys.stdout.flush()
            self._send(201, INSTALL)
        else:
            self._send(204)

    def log_message(self, fmt, *args):
        sys.stdout.write("fake publik: %s\n" % (fmt % args))
        sys.stdout.flush()


HTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
