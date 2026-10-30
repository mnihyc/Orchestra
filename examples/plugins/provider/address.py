#!/usr/bin/env python3
"""The same made-up Acme account over the provider address binding, with bearer authorization.

Loopback HTTP for an explicitly allowed internal-network example. No credential presenter is exposed.
The token is read from --token-file, never from the command line or an access log.
"""

import argparse
from http.server import BaseHTTPRequestHandler, HTTPServer
import hmac
import json
import os
from pathlib import Path
import sys

from plan import Acme, CONTRACT_VERSION, MAX_REQUEST_BYTES, Refusal, encoded


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", required=True)
    parser.add_argument("--token-file", required=True)
    parser.add_argument("--port", type=int, default=0, help="zero asks the kernel for an available loopback port")
    parser.add_argument("--ready-file", required=True, help="receives the actual URL after binding; contains no token")
    parser.add_argument("--at", help="fixed UTC clock for reproducible tests")
    args = parser.parse_args()
    token = Path(args.token_file).read_text().rstrip("\r\n")
    if not token or not token.isascii() or any(character.isspace() for character in token):
        parser.error("the token file needs one nonempty ASCII bearer token")
    authorization = ("Bearer " + token).encode()
    account = Acme(args.state_dir, args.at)

    class Handler(BaseHTTPRequestHandler):
        # Closing every response matches the binding and avoids a second request sharing authorization state.
        protocol_version = "HTTP/1.1"

        def log_message(self, _format, *args):
            pass  # No request headers, bodies, token or key enter an access log.

        def answer(self, status, value):
            data = encoded(value).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(data)
            self.close_connection = True

        def do_POST(self):
            if self.path != "/provider":
                self.answer(404, {"error": Refusal("unsupported", "use the final /provider address").wire()})
                return
            if not hmac.compare_digest(self.headers.get("Authorization", "").encode(), authorization):
                self.answer(401, {"error": Refusal("unauthorized", "this address refused its authorization").wire()})
                return
            try:
                length = int(self.headers.get("Content-Length", "-1"))
                if not 0 <= length <= MAX_REQUEST_BYTES:
                    self.answer(413, {"error": Refusal("too_large", "the example needs a bounded JSON request").wire()})
                    return
                request = json.loads(self.rfile.read(length))
                if type(request["contract"]) is not int or request["contract"] != CONTRACT_VERSION:
                    raise Refusal("unsupported", "this address speaks provider contract 1")
                result = account.call(request["method"], request.get("params") or {}, address=True)
                self.answer(200, {"result": result})
            except Refusal as error:
                self.answer(200, {"error": error.wire()})
            except (ValueError, KeyError, TypeError, AttributeError):
                self.answer(200, {"error": Refusal("malformed", "the example received malformed parameters").wire()})
            except OSError:
                self.answer(200, {"error": Refusal("unavailable", "the example account state is unavailable", True).wire()})

    # A serial server keeps this example's one account/reset transaction in one owner process.
    with HTTPServer(("127.0.0.1", args.port), Handler) as server:
        ready = Path(args.ready_file)
        temporary = ready.with_suffix(".new")
        with temporary.open("w") as output:
            os.chmod(temporary, 0o600)
            output.write(encoded({"url": "http://127.0.0.1:%d/provider" % server.server_port}))
        os.replace(temporary, ready)
        # A readiness signal, not an access log; only the nonsecret bound URL is published.
        sys.stdout.write(encoded({"url": "http://127.0.0.1:%d/provider" % server.server_port}) + "\n")
        sys.stdout.flush()
        server.serve_forever()


if __name__ == "__main__":
    main()
