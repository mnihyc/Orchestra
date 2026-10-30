"""A program accepts only its live scoped caller signature, and PROXY framing never changes the following payload."""
import base64
import hashlib
import json
import socket
import struct
import unittest

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat
from ports_probe import KEY_ID_HEX_DIGITS, PROXY_V2_SIGNATURE, proxy_peer, verify


def encoded(value):
    return base64.urlsafe_b64encode(value).decode().rstrip("=")


class Caller(unittest.TestCase):
    def setUp(self):
        self.key = Ed25519PrivateKey.generate()
        public = self.key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
        kid = hashlib.sha256(public).hexdigest()[:KEY_ID_HEX_DIGITS]
        self.keys = {"keys": [{"kty": "OKP", "crv": "Ed25519", "kid": kid, "x": encoded(public)}]}
        self.header = {"alg": "Ed25519", "typ": "orch-port+jwt", "kid": kid}
        self.claims = {"iss": "https://orch.example", "aud": "orch-port:environment:41884", "environment_id": "environment",
                       "port": 41884, "generation": 1, "connection_id": "connection", "person": {"id": "person-a"},
                       "iat": 1000, "exp": 1030}

    def token(self, claims):
        content = encoded(json.dumps(self.header).encode()) + "." + encoded(json.dumps(claims).encode())
        return content + "." + encoded(self.key.sign(content.encode()))

    def test_the_intended_live_target_gets_the_safe_caller_without_an_assertion(self):
        caller = verify(self.token(self.claims), self.keys, "https://orch.example", "environment", 41884, 1, 1010)
        self.assertEqual(caller["person"], {"id": "person-a"})
        self.assertNotIn("assertion", caller)

    def test_another_target_generation_or_expired_signature_is_refused(self):
        for audience, generation, time in [("other", 1, 1010), ("environment", 2, 1010), ("environment", 1, 1040)]:
            with self.assertRaises(ValueError):
                verify(self.token(self.claims), self.keys, "https://orch.example", audience, 41884, generation, time)

    def test_a_signed_statement_for_another_issuer_is_refused(self):
        claims = dict(self.claims, iss="https://another.example")
        with self.assertRaises(ValueError):
            verify(self.token(claims), self.keys, "https://orch.example", "environment", 41884, 1, 1010)

    def test_proxy_metadata_is_read_first_and_the_callers_payload_is_untouched(self):
        sender, receiver = socket.socketpair()
        with sender, receiver:
            peer = socket.inet_aton("192.0.2.1") + socket.inet_aton("192.0.2.2") + struct.pack("!HH", 40000, 41883)
            sender.sendall(PROXY_V2_SIGNATURE + bytes([0x21, 0x11]) + struct.pack("!H", len(peer)) + peer + b"raw caller bytes")
            self.assertEqual(proxy_peer(receiver), {"source": ["192.0.2.1", 40000], "target": ["192.0.2.2", 41883]})
            self.assertEqual(receiver.recv(1024), b"raw caller bytes")


if __name__ == "__main__":
    unittest.main()
