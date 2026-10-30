#!/usr/bin/env python3
"""Disposable HTTP caller verifier or TCP/UDP echo for a joined environment port walk."""
import argparse
import base64
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import socket
import struct
import time

# These are the public caller assertion's lifetime and verifier clock allowance.
ASSERTION_SECONDS = 30
SKEW_SECONDS = 5
KEY_ID_HEX_DIGITS = 16
# A bounded socket chunk; datagrams fit the maximum UDP payload and TCP streams
# are echoed chunk by chunk without collecting an unbounded conversation.
CHUNK_BYTES = 65536
PROXY_V2_SIGNATURE = b"\r\n\r\n\x00\r\nQUIT\n"
PROXY_V2_HEADER_BYTES = 16
PROXY_TCP_IPV4 = 0x11
PROXY_TCP_IPV6 = 0x21


def decoded(text):
    return base64.b64decode(text + "=" * (-len(text) % 4), altchars=b"-_", validate=True)


def verify(assertion, keys, issuer, environment, port, generation, at):
    """Accept only the installation's exact target-bound, live Ed25519 statement."""
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
    header_part, claims_part, signature_part = assertion.split(".")
    header = json.loads(decoded(header_part))
    if header.get("alg") != "Ed25519" or header.get("typ") != "orch-port+jwt":
        raise ValueError("wrong assertion algorithm or type")
    selected = next((key for key in keys["keys"] if key.get("kid") == header.get("kid")), None)
    if selected is None or selected.get("kty") != "OKP" or selected.get("crv") != "Ed25519":
        raise ValueError("unknown assertion verification key")
    public = decoded(selected["x"])
    if hashlib.sha256(public).hexdigest()[:KEY_ID_HEX_DIGITS] != header["kid"]:
        raise ValueError("key identity does not match its bytes")
    Ed25519PublicKey.from_public_bytes(public).verify(decoded(signature_part), (header_part + "." + claims_part).encode())
    claims = json.loads(decoded(claims_part))
    if claims.get("iss") != issuer or claims.get("aud") != f"orch-port:{environment}:{port}":
        raise ValueError("another issuer or target")
    if claims.get("environment_id") != environment or claims.get("port") != port or claims.get("generation") != generation:
        raise ValueError("another environment generation or port")
    issued, expires = claims.get("iat"), claims.get("exp")
    if type(issued) is not int or type(expires) is not int or expires <= issued or expires - issued > ASSERTION_SECONDS:
        raise ValueError("invalid assertion lifetime")
    if issued > at + SKEW_SECONDS or expires < at - SKEW_SECONDS:
        raise ValueError("assertion is not valid now")
    if not claims.get("connection_id") or bool(claims.get("person")) == bool(claims.get("link")):
        raise ValueError("no unique caller or connection")
    # Print safe attribution, never the short-lived signed assertion itself.
    return {name: claims.get(name) for name in ["environment_id", "port", "generation", "connection_id", "opening_id", "transport", "outside", "person", "link"]}


def exact(stream, length):
    data = bytearray()
    while len(data) < length:
        piece = stream.recv(length - len(data))
        if not piece:
            raise ValueError("connection ended before its PROXY frame")
        data.extend(piece)
    return bytes(data)


def proxy_peer(stream):
    header = exact(stream, PROXY_V2_HEADER_BYTES)
    if header[:len(PROXY_V2_SIGNATURE)] != PROXY_V2_SIGNATURE or header[12] != 0x21:
        raise ValueError("expected a PROXY v2 caller frame")
    payload = exact(stream, struct.unpack("!H", header[14:16])[0])
    if header[13] == PROXY_TCP_IPV4:
        source = socket.inet_ntop(socket.AF_INET, payload[:4])
        target = socket.inet_ntop(socket.AF_INET, payload[4:8])
        source_port, target_port = struct.unpack("!HH", payload[8:12])
    elif header[13] == PROXY_TCP_IPV6:
        source = socket.inet_ntop(socket.AF_INET6, payload[:16])
        target = socket.inet_ntop(socket.AF_INET6, payload[16:32])
        source_port, target_port = struct.unpack("!HH", payload[32:36])
    else:
        raise ValueError("expected IPv4 or IPv6 TCP caller framing")
    return {"source": [source, source_port], "target": [target, target_port]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=["http", "tcp", "udp"], required=True)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--bind", default="127.0.0.1")
    parser.add_argument("--proxy-v2", action="store_true")
    parser.add_argument("--keys", type=Path, help="public /ports/assertion-keys response, never an installation secret")
    parser.add_argument("--issuer")
    parser.add_argument("--environment")
    parser.add_argument("--generation", type=int)
    parser.add_argument("--assertion-header", help="the caller assertion header declared by this candidate")
    args = parser.parse_args()
    if not 1 <= args.port <= 65535:
        parser.error("port must be 1 through 65535")
    if args.proxy_v2 and args.mode != "tcp":
        parser.error("PROXY framing belongs only to the dedicated TCP probe")
    if args.mode == "http":
        if not all([args.keys, args.issuer, args.environment, args.generation is not None, args.assertion_header]):
            parser.error("HTTP verification requires public keys, issuer, environment, generation and the candidate's assertion header")
        keys = json.loads(args.keys.read_text())

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                try:
                    caller = verify(self.headers.get(args.assertion_header, ""), keys, args.issuer,
                                    args.environment, args.port, args.generation, int(time.time()))
                except Exception as error:
                    self.send_response(403)
                    self.end_headers()
                    self.wfile.write(("caller assertion refused: " + type(error).__name__).encode())
                    return
                reply = {"verified": True, "peer": list(self.client_address), "caller": caller,
                         "header_names": sorted(self.headers.keys())}
                print(json.dumps(reply), flush=True)
                body = json.dumps(reply).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *_):
                pass

        ThreadingHTTPServer((args.bind, args.port), Handler).serve_forever()
        return
    family = socket.AF_INET6 if ":" in args.bind else socket.AF_INET
    kind = socket.SOCK_DGRAM if args.mode == "udp" else socket.SOCK_STREAM
    with socket.socket(family, kind) as listener:
        listener.bind((args.bind, args.port))
        print(json.dumps({"listening": [args.bind, args.port], "mode": args.mode}), flush=True)
        if args.mode == "udp":
            while True:
                data, peer = listener.recvfrom(CHUNK_BYTES)
                print(json.dumps({"peer": list(peer), "bytes": len(data)}), flush=True)
                listener.sendto(data, peer)
        else:
            listener.listen()
            while True:
                stream, peer = listener.accept()
                with stream:
                    caller = proxy_peer(stream) if args.proxy_v2 else None
                    print(json.dumps({"peer": list(peer), "proxy": caller}), flush=True)
                    while data := stream.recv(CHUNK_BYTES):
                        stream.sendall(data)


if __name__ == "__main__":
    main()
