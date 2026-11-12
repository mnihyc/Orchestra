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
from workload import within_budget, failure


class WebSocket:
    def __init__(self, reader, writer):
        self.reader, self.writer = reader, writer
        self.lock = asyncio.Lock()

    @classmethod
    async def connect(cls, url, ca, origin, subprotocol='orch.port.v1'):
        if subprotocol not in ('orch.port.v1', 'orch.terminal.v1'):
            raise ValueError('the fixture has no framing for this subprotocol')
        target = urllib.parse.urlsplit(url)
        context = ssl.create_default_context(cafile=ca)
        reader, writer = await asyncio.open_connection(target.hostname, target.port or 443, ssl=context,
            server_hostname=target.hostname, limit=128 * 1024)
        try:
            key = base64.b64encode(os.urandom(16)).decode()
            request = (f'GET {target.path}?{target.query} HTTP/1.1\r\nHost: {target.netloc}\r\nUpgrade: websocket\r\n'
                f'Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n'
                f'Sec-WebSocket-Protocol: {subprotocol}\r\nOrigin: {origin}\r\n\r\n')
            writer.write(request.encode())
            await writer.drain()
            headers = (await reader.readuntil(b'\r\n\r\n')).decode('ascii')
            fields = dict(line.split(': ', 1) for line in headers.split('\r\n')[1:] if ': ' in line)
            fields = {key.lower(): value for key, value in fields.items()}
            expected = base64.b64encode(hashlib.sha1((key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()).decode()
            if not headers.startswith('HTTP/1.1 101 ') or fields.get('sec-websocket-accept') != expected or fields.get('sec-websocket-protocol') != subprotocol:
                writer.close()
                raise ValueError('the actual socket refused its upgrade or subprotocol')
            return cls(reader, writer)
        except BaseException:
            writer.close()
            writer.transport.abort()
            raise

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
            await asyncio.wait_for(self.writer.wait_closed(), 30)
        except (ConnectionError, ssl.SSLError):
            pass
        except BaseException:
            self.writer.transport.abort()
            raise


async def transfer(world, count, port, pause_reader=False, cut=False, ready=None, payload=None, observation=None):
    seed = uuid.uuid4().hex
    token = Path(world['token_file']).read_text()
    use = await asyncio.to_thread(API(world['edge'], token).call, 'POST',
        f'/environments/{world["environment"]}/ports/{port}/use', {'transport': 'tcp'})
    query = urllib.parse.urlencode({'ticket': use['ticket']})
    url = world['system'].replace('https://', 'wss://') + f'/api/v1/ws/ports/{use["id"]}?{query}'
    websocket = await WebSocket.connect(url, world['ca'], world['system'])
    header = json.dumps({'seed': seed, 'bytes': count}).encode()
    await websocket.send(2, struct.pack('!I', len(header)) + header)
    result = {'seed': seed, 'expected_bytes': count, 'cut': cut, 'paused_reader': pause_reader,
        'upload_bytes': 0, 'download_bytes': 0, 'max_write_wait_ms': 0,
        'progress': []}
    started = time.monotonic()
    # One checkpoint per source chunk preserves ordered monotonic progress without
    # retaining any transferred content. CHUNK is the existing 64-KiB payload unit.
    checkpoints = {'upload': 0, 'download': 0}
    announced = {'ready': False, 'payload': False}

    def progress(direction):
        if observation is not None:
            observation(seed, {'seed': seed, 'upload_bytes': result['upload_bytes'], 'download_bytes': result['download_bytes'],
                'last_progress_seconds': time.monotonic() - started})
        offset = result[direction + '_bytes']
        # A received byte proves actual payload, rather than only an opened socket
        # or a local write buffer. Other streams can supply it while one reader stalls.
        if result['download_bytes'] > 0 and not announced['payload']:
            announced['payload'] = True
            if payload is not None:
                payload(seed)
        # The deliberate outage waits for nonzero payload in BOTH directions on
        # every stream; sending the initial JSON header is not sufficient evidence.
        if result['upload_bytes'] > 0 and result['download_bytes'] > 0 and not announced['ready']:
            announced['ready'] = True
            if ready is not None:
                ready(seed)
        # Keep at most one checkpoint per MiB (the existing deliberate-cut unit),
        # plus each final offset; three100-MiB streams remain bounded evidence.
        if offset - checkpoints[direction] >= 1024 * 1024 or offset == count:
            result['progress'].append({'direction': direction, 'offset': offset,
                'seconds': time.monotonic() - started})
            checkpoints[direction] = offset

    async def upload():
        for part in chunks(seed, 'upload', count):
            before = time.monotonic()
            await websocket.send(2, part)
            result['max_write_wait_ms'] = max(result['max_write_wait_ms'], (time.monotonic() - before) * 1000)
            result['upload_bytes'] += len(part)
            progress('upload')
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
            progress('download')
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
    world.update(edge='http://frontend', ca='/fixture/ca.pem', token_file='/fixture/access-token',
        fault_state_file='/fixture/fault-state.json')
    started = []
    active = set()
    bulk_ready, bulk_done = asyncio.Event(), asyncio.Event()

    def ready(seed):
        started.append(seed)
        if len(started) == args.streams:
            args.out.with_suffix('.started').write_text(json.dumps(started))

    def payload(seed):
        active.add(seed)
        bulk_ready.set()

    async def one(index):
        seen = []
        def receiving(seed):
            seen.append(seed)
            payload(seed)
        try:
            return await transfer(world, args.bytes, args.port,
                pause_reader=index == 0 and args.stall, cut=args.cut, ready=ready, payload=receiving,
                observation=lambda seed, item: args.payload_observations.update({seed: item}))
        finally:
            active.difference_update(seen)

    mixed = None
    watching = None
    transfers = []
    results = []
    if args.mixed:
        from mixed import Mixed
        def changed(record):
            args.out.with_suffix('.mixed.json').write_text(json.dumps(record, indent=2))
        mixed = Mixed(world, WebSocket, lambda: len(active), changed, profile=args.profile)
    try:
        if mixed:
            await mixed.start()
            await asyncio.gather(mixed.probe(), mixed.echo())  # Idle comparison, not bulk evidence.
            watching = asyncio.create_task(mixed.watch(bulk_ready, bulk_done))
        transfers = [asyncio.create_task(one(index)) for index in range(args.streams)]
        results = await asyncio.gather(*transfers)
    finally:
        bulk_done.set()
        try:
            for transfer_task in transfers:
                if not transfer_task.done():
                    transfer_task.cancel()
            await asyncio.gather(*transfers, return_exceptions=True)
            if watching:
                try:
                    await asyncio.wait_for(asyncio.shield(watching), 30)
                except asyncio.TimeoutError:
                    mixed.record['watcher_cleanup_failure'] = 'TimeoutError'
                    watching.cancel()
                    await asyncio.gather(watching, return_exceptions=True)
            if mixed and results and mixed.record['terminal_fault'] is None:
                await asyncio.gather(mixed.probe(), mixed.echo())  # Post-drain comparison.
        finally:
            if watching and not watching.done():
                watching.cancel()
                await asyncio.gather(watching, return_exceptions=True)
            if mixed:
                await mixed.close()
    args.out.write_text(json.dumps({'profile': args.profile, 'streams': results, 'passed': True,
        'mixed': None if mixed is None else mixed.record, 'harness_budget_seconds': args.harness_budget_seconds}, indent=2))


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
    parser.add_argument('--harness-budget-seconds', type=int, default=7200, help='QA work budget only; expiry is incomplete evidence, never a floor performance verdict')
    parser.add_argument('--mixed', action='store_true', help='Observe owned terminal and public command completion during bulk traffic')
    args = parser.parse_args()
    if not 1 <= args.streams <= 16 or not 0 < args.bytes <= 1024 ** 3:
        parser.error('1..16 bounded streams and 1..1GiB per direction are required')
    if args.harness_budget_seconds <= 0:
        parser.error('harness budget must be positive')
    args.payload_observations = {}
    try:
        asyncio.run(within_budget(run(args), args.harness_budget_seconds))
    except BaseException as error:
        record = failure(error, args.harness_budget_seconds, args.payload_observations)
        observed = args.out.with_suffix('.mixed.json')
        if observed.exists():
            record['mixed'] = json.loads(observed.read_text())
        args.out.write_text(json.dumps(record, indent=2))
        raise


if __name__ == '__main__':
    main()
