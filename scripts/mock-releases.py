#!/usr/bin/env python3
"""Plays GitHub Releases for scripts/update-roundtrip.sh (test only, binds 127.0.0.1).

GET /repos/mafianacho/nachocombo-windows/releases/latest -> <dir>/latest.json
GET /files/<name>                                       -> <dir>/files/<name>
GET /                                                   -> 200 (readiness)
"""
import argparse
import http.server
import os

parser = argparse.ArgumentParser()
parser.add_argument("--port", type=int, default=8794)
parser.add_argument("--dir", required=True)
args = parser.parse_args()
root = os.path.abspath(args.dir)


class Handler(http.server.BaseHTTPRequestHandler):
    def send(self, status, body=b"", kind="application/octet-stream"):
        self.send_response(status)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        path = self.path.split("?")[0]
        if path == "/":
            return self.send(200, b"ok", "text/plain")
        if path == "/repos/mafianacho/nachocombo-windows/releases/latest":
            file, kind = os.path.join(root, "latest.json"), "application/json"
        elif path.startswith("/files/") and "/" not in path[7:] and ".." not in path:
            file, kind = os.path.join(root, "files", path[7:]), "application/octet-stream"
        else:
            return self.send(404, b"not found", "text/plain")
        if not os.path.isfile(file):
            return self.send(404, b"not found", "text/plain")
        with open(file, "rb") as f:
            self.send(200, f.read(), kind)

    def log_message(self, fmt, *a):
        print("%s %s" % (self.command, self.path), flush=True)


http.server.ThreadingHTTPServer(("127.0.0.1", args.port), Handler).serve_forever()
