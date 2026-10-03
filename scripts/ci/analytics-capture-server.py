#!/usr/bin/env python3
"""A loopback stand-in for the OpenPanel collector, for CI.

Logs every request as one JSON line (method, path, lower-cased headers, body)
to --log and answers 200 {"deviceId":"x","sessionId":"y"}, the shape the real
`POST /track` returns. `scripts/ci/assert-desktop-analytics.sh` points a built
binary at it (OPENCOMPANY_ANALYTICS_ENDPOINT) and asserts on what arrived.

Binds 127.0.0.1 only. `--port 0` picks a free ephemeral port; the chosen port is
written to --port-file once the socket is listening, so the caller never races.
"""
import argparse
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--log", required=True)
    parser.add_argument("--port-file", required=True)
    args = parser.parse_args()

    class Handler(BaseHTTPRequestHandler):
        def _record(self) -> None:
            length = int(self.headers.get("content-length") or 0)
            body = self.rfile.read(length).decode("utf-8", "replace") if length else ""
            entry = {
                "method": self.command,
                "path": self.path,
                "headers": {k.lower(): v for k, v in self.headers.items()},
                "body": body,
            }
            with open(args.log, "a", encoding="utf-8") as log:
                log.write(json.dumps(entry) + "\n")
            payload = b'{"deviceId":"x","sessionId":"y"}'
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        do_POST = do_GET = do_PUT = do_OPTIONS = _record

        def log_message(self, *_args) -> None:  # keep CI output quiet
            pass

    server = HTTPServer(("127.0.0.1", args.port), Handler)
    with open(args.port_file, "w", encoding="utf-8") as handle:
        handle.write(str(server.server_address[1]))
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
