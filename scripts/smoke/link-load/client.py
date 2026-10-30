#!/usr/bin/env python3
"""Transfer bytes through an actual public PortUse WebSocket; no product seam is doubled."""
import argparse
import asyncio
import base64
import hashlib
import json
import os
from pathlib import Path
import ssl
import struct
import time
import urllib.parse
import uuid

from peer import CHUNK, chunks, digest
from world import API


class WebSocket:
    def __init__(self, reader, writer):
        self.reader, self.writer = reader, writer
        self.lock = asyncio.Lock()

    @classmethod
    async def connect(cls, url, ca, origin):
        target = urllib.parse.urlsplit(url)
        context = ssl.create_default_context(cafile=ca)
        reader, writer = await asyncio.open_connection(target.hostname, target.port or 443, ssl=context,
            server_hostname=target.hostname, limit=128 * 1024)
        key = base64.b64encode(os.urandom(16)).decode()
        request = (f'GET {target.path}?{target.query} HTTP/1.1\r\nHost: {target.netloc}\r\nUpgrade: websocket\r\n'
            f'Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n'
            f'Sec-WebSocket-Protocol: orch.port.v1\r\nOrigin: {origin}\r\n\r\n')
        writer.write(request.encode())
        await writer.drain()
        headers = (await reader.readuntil(b'\r\n\r\n')).decode('ascii')
        fields = dict(line.split(': ', 1) for line in headers.split('\r\n')[1:] if ': ' in line)
        fields = {key.lower(): value for key, value in fields.items()}
        expected = base64.b64encode(hashlib.sha1((key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()).decode()
        if not headers.startswith('HTTP/1.1 101 ') or fields.get('sec-websocket-accept') != expected or fields.get('sec-websocket-protocol') != 'orch.port.v1':
            writer.close()
            raise ValueError('the actual port socket refused its upgrade or subprotocol')
        return cls(reader, writer)

    async def send(self, opcode, data):
        async with self.lock:
            mask = os.urandom(4)
            size = len(data)
            length = bytes([0x80 | size]) if size < 126 else b'\xfe' + struct.pack('!H', size) if size <= 65535 else b'\xff' + struct.pack('!Q', size)
            self.writer.write(bytes([0x80 | opcode]) + length + mask + bytes(value ^ mask[index % 4] for index, value in enumerate(data)))
            await self.writer.drain()

    async def frames(self):
        partial, kind = bytearray(), None
        while True:
            first, second = await self.reader.readexactly(2)
            if first & 0x70 or second & 0x80:
                raise ValueError('an invalid server frame')
            size = second & 127
            if size == 126:
                size = struct.unpack('!H', await self.reader.readexactly(2))[0]
            elif size == 127:
                size = struct.unpack('!Q', await self.reader.readexactly(8))[0]
            if size > 2 * 1024 * 1024:
                raise ValueError('the socket exceeded the fixture frame bound')
            data = await self.reader.readexactly(size)
            opcode = first & 15
            if opcode == 8:
                return
            if opcode == 9:
                await self.send(10, data)
                continue
            if opcode == 10:
                continue
            if opcode in (1, 2):
                if kind is not None:
                    raise ValueError('a message was interrupted')
                kind = opcode
            elif opcode != 0 or kind is None:
                raise ValueError('an invalid continuation')
            partial.extend(data)
            if len(partial) > 2 * 1024 * 1024:
                raise ValueError('the socket exceeded the fixture message bound')
            if first & 0x80:
                yield kind, bytes(partial)
                partial.clear()
                kind = None

    async def close(self):
        self.writer.close()
        try:
            await self.writer.wait_closed()
        except (ConnectionError, ssl.SSLError):
            pass


async def transfer(world, count, port, pause_reader=False, cut=False, ready=None):
    seed = uuid.uuid4().hex
    token = Path(world['token_file']).read_text()
    use = await asyncio.to_thread(API(world['edge'], token).call, 'POST',
        f'/environments/{world["environment"]}/ports/{port}/use', {'transport': 'tcp'})
    query = urllib.parse.urlencode({'ticket': use['ticket']})
    url = world['system'].replace('https://', 'wss://') + f'/api/v1/ws/ports/{use["id"]}?{query}'
    websocket = await WebSocket.connect(url, world['ca'], world['system'])
    header = json.dumps({'seed': seed, 'bytes': count}).encode()
    await websocket.send(2, struct.pack('!I', len(header)) + header)
    if ready is not None:
        ready(seed)
    result = {'seed': seed, 'expected_bytes': count, 'cut': cut, 'paused_reader': pause_reader,
        'upload_bytes': 0, 'download_bytes': 0, 'max_write_wait_ms': 0}
    started = time.monotonic()

    async def upload():
        for part in chunks(seed, 'upload', count):
            before = time.monotonic()
            await websocket.send(2, part)
            result['max_write_wait_ms'] = max(result['max_write_wait_ms'], (time.monotonic() - before) * 1000)
            result['upload_bytes'] += len(part)
        await websocket.send(1, b'eof')

    async def download():
        received = hashlib.sha256()
        if pause_reader:
            await asyncio.sleep(5)  # An intentional stalled reader, not a wait for a presumed event.
        async for kind, part in websocket.frames():
            if kind == 1:
                if part != b'eof':
                    raise ValueError('an unexpected port control message')
                break
            received.update(part)
            result['download_bytes'] += len(part)
            if result['download_bytes'] > count:
                raise ValueError('bytes crossed a stream boundary')
            if cut and result['download_bytes'] >= 1024 * 1024:
                result['download_sha256'] = received.hexdigest()
                return
        result['download_sha256'] = received.hexdigest()
        if not cut and (result['download_bytes'] != count or result['download_sha256'] != digest(seed, 'download', count)):
            raise ValueError('the complete download differs from this stream and direction')

    writer = asyncio.create_task(upload())
    try:
        await download()
        if not cut:
            await writer
            result['upload_sha256'] = digest(seed, 'upload', count)
            result['expected_download_sha256'] = digest(seed, 'download', count)
    finally:
        if not writer.done():
            writer.cancel()
        await asyncio.gather(writer, return_exceptions=True)
        await websocket.close()
    result['seconds'] = time.monotonic() - started
    return result


async def run(args):
    world = json.loads(args.world.read_text())
    # Paths inside this isolated client; credentials never enter retained result files.
    world.update(edge='http://frontend', ca='/fixture/ca.pem', token_file='/fixture/access-token')
    started = []

    def ready(seed):
        started.append(seed)
        if len(started) == args.streams:
            args.out.with_suffix('.started').write_text(json.dumps(started))

    results = await asyncio.gather(*(transfer(world, args.bytes, args.port,
        pause_reader=index == 0 and args.stall, cut=args.cut, ready=ready) for index in range(args.streams)))
    args.out.write_text(json.dumps({'profile': args.profile, 'streams': results, 'passed': True}, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--world', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--port', type=int, default=41325)
    parser.add_argument('--bytes', type=int, default=100 * 1024 * 1024)
    parser.add_argument('--streams', type=int, default=3)
    parser.add_argument('--profile', choices=['profile', 'floor'], default='profile')
    parser.add_argument('--stall', action='store_true')
    parser.add_argument('--cut', action='store_true')
    args = parser.parse_args()
    if not 1 <= args.streams <= 16 or not 0 < args.bytes <= 1024 ** 3:
        parser.error('1..16 bounded streams and 1..1GiB per direction are required')
    try:
        asyncio.run(asyncio.wait_for(run(args), 7200))
    except BaseException as error:
        args.out.write_text(json.dumps({'passed': False, 'failure': type(error).__name__}))
        raise


if __name__ == '__main__':
    main()
