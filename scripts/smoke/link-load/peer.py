#!/usr/bin/env python3
"""Full-duplex, stream-specific byte fixture, running beside the released connector."""
import argparse
import hashlib
import json
from pathlib import Path
import socket
import socketserver
import struct
import threading
import time

CHUNK = 64 * 1024


def chunks(seed, direction, count):
    """Different streams, offsets and directions have different reproducible bytes."""
    offset = 0
    while offset < count:
        size = min(CHUNK, count - offset)
        yield hashlib.shake_256(f'{seed}/{direction}/{offset}'.encode()).digest(size)
        offset += size


def digest(seed, direction, count):
    value = hashlib.sha256()
    for part in chunks(seed, direction, count):
        value.update(part)
    return value.hexdigest()


def exact(sock, count):
    result = bytearray()
    while len(result) < count:
        part = sock.recv(count - len(result))
        if not part:
            raise EOFError('the stream ended before its header')
        result.extend(part)
    return result


class Peer(socketserver.BaseRequestHandler):
    def handle(self):
        started = time.monotonic()
        size = struct.unpack('!I', exact(self.request, 4))[0]
        if size > 1024:
            raise ValueError('an unbounded fixture header')
        header = json.loads(exact(self.request, size))
        seed, count = header['seed'], header['bytes']
        if not isinstance(count, int) or not 0 < count <= 1024 ** 3 or not isinstance(seed, str) or not seed.isalnum():
            raise ValueError('invalid fixture request')
        result = {'seed': seed, 'expected_bytes': count, 'upload_bytes': 0, 'download_bytes': 0, 'closed': False}
        sent_error = []

        def send():
            try:
                for part in chunks(seed, 'download', count):
                    self.request.sendall(part)
                    result['download_bytes'] += len(part)
                self.request.shutdown(socket.SHUT_WR)
            except OSError as error:
                sent_error.append(type(error).__name__)

        self.request.settimeout(7200)  # Hang guard for a 100-MiB run at the declared shaped rates.
        sender = threading.Thread(target=send)
        sender.start()
        received = hashlib.sha256()
        try:
            while True:
                part = self.request.recv(CHUNK)
                if not part:
                    break
                received.update(part)
                result['upload_bytes'] += len(part)
                if result['upload_bytes'] > count:
                    raise ValueError('bytes crossed a stream boundary')
        except (OSError, ValueError) as error:
            result['receive_error'] = type(error).__name__
        finally:
            sender.join(timeout=300)  # A cut must unblock the producer as well as the consumer.
            if sender.is_alive():
                self.request.close()
                sender.join(timeout=5)
                result['send_stuck'] = sender.is_alive()
            result.update(upload_sha256=received.hexdigest(), closed=True,
                send_errors=sent_error, seconds=time.monotonic() - started)
            target = self.server.output / (seed + '.json')
            temporary = target.with_suffix('.partial')
            temporary.write_text(json.dumps(result, sort_keys=True))
            temporary.replace(target)


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    with Server(('0.0.0.0', args.port), Peer) as server:
        server.output = args.out
        (args.out / 'ready').write_text(str(args.port))
        server.serve_forever()


if __name__ == '__main__':
    main()
