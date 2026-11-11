"""Bounded observations of the ordinary command and terminal paths during bulk load.

Public MachineProbe completion upper-bounds its preceding ACK; it does not expose
the exact ACK timestamp. Terminal latency is recorded without an invented SLA.
"""
import asyncio
import json
from pathlib import Path
import re
import time
import urllib.parse
import uuid

from world import API
from faults import classify

# Sample at the existing machine heartbeat cadence. This is observation density,
# not a performance condition. Each observer has at most one outstanding request.
SAMPLE_SECONDS = 20
# Existing MachineProbe command/open-PTY budget, used only as a harness hang guard.
OBSERVATION_SECONDS = 30
ACK_BUDGET_MS = 1000  # the specified acknowledgement budget; the simulator's 1500 ms does not amend it.


def marker_command(marker):
    """The shell's input echo cannot contain the output marker in plain text."""
    if not re.fullmatch(r'ORCHWSS_[a-f0-9]{32}_[0-9]+', marker):
        raise ValueError('invalid owned marker')
    encoded = ''.join('\\%03o' % value for value in (marker + '\n').encode())
    return ("printf '" + encoded + "'\r").encode()


class Mixed:
    def __init__(self, world, websocket_type, active, changed=lambda record: None, profile='profile'):
        if profile not in ('profile', 'floor'):
            raise ValueError('unknown measured profile')
        self.world, self.websocket_type, self.active, self.changed = world, websocket_type, active, changed
        self.api = API(world['edge'], Path(world['token_file']).read_text())
        self.prefix = 'ORCHWSS_' + uuid.uuid4().hex + '_'
        self.started = time.monotonic()
        self.record = {'sampling_seconds': SAMPLE_SECONDS, 'profile': profile,
            'command_timing': 'public_completion_upper_bound_on_ack',
            'ack_budget_ms': ACK_BUDGET_MS if profile == 'profile' else None,
            'probes': [], 'terminal': [], 'terminal_created': False, 'terminal_closed': False,
            'terminal_fault': None, 'whole_session_queue_bound': 'not_observed'}
        self.socket = None
        self.reader = None
        self.terminal_id = None
        self.ready = asyncio.get_running_loop().create_future()
        self.pending = None
        self.next_marker = 0
        self.received_markers = 0

    async def start(self):
        reply = await asyncio.to_thread(self.api.call, 'POST',
            f'/environments/{self.world["environment"]}/terminals', {'cols': 120, 'rows': 32},
            timeout=OBSERVATION_SECONDS)
        session = reply['session']
        if session['environment_id'] != self.world['environment']:
            raise ValueError('terminal belongs to another fixture environment')
        self.terminal_id = session['id']
        self.record['terminal_created'] = True
        self.changed(self.record)
        query = urllib.parse.urlencode({'ticket': reply['ticket']})
        url = self.world['system'].replace('https://', 'wss://') + f'/api/v1/ws/terminals/{self.terminal_id}?{query}'
        self.socket = await asyncio.wait_for(self.websocket_type.connect(
            url, self.world['ca'], self.world['system'], 'orch.terminal.v1'), OBSERVATION_SECONDS)
        self.reader = asyncio.create_task(self.read_terminal())
        await asyncio.wait_for(asyncio.shield(self.ready), OBSERVATION_SECONDS)

    async def read_terminal(self):
        buffer = bytearray()
        replay = 0
        pattern = re.compile(re.escape(self.prefix.encode()) + rb'([0-9]+)\r?\n')
        try:
            async for kind, data in self.socket.frames():
                if kind == 1:
                    frame = json.loads(data)
                    if frame.get('op') == 'ready' and not self.ready.done():
                        replay = frame['replay_bytes']
                        if type(replay) is not int or not 0 <= replay <= 256 * 1024:
                            raise ValueError('terminal replay exceeds its declared bound')
                        self.ready.set_result(True)
                    elif frame.get('op') in ('closed', 'exit'):
                        raise ConnectionError('owned terminal ended during the load observation')
                    elif frame.get('op') != 'pong':
                        raise ValueError('unexpected terminal control frame')
                    continue
                if not self.ready.done():
                    raise ValueError('terminal bytes preceded ready')
                omitted = min(replay, len(data))
                replay -= omitted
                buffer.extend(data[omitted:])
                consumed = 0
                for match in pattern.finditer(buffer):
                    number = int(match[1])
                    if number != self.received_markers or self.pending is None:
                        raise ValueError('terminal marker was duplicated, reordered or unsolicited')
                    self.received_markers += 1
                    self.pending.set_result(time.monotonic())
                    self.pending = None
                    consumed = match.end()
                if consumed:
                    del buffer[:consumed]
                # No shell output is retained. Keep only enough tail for one split marker.
                buffer[:] = buffer[-(len(self.prefix) + 32):]
            raise ConnectionError('owned terminal socket closed')
        except asyncio.CancelledError:
            raise
        except Exception as error:
            self.record['terminal_fault'] = type(error).__name__
            if not self.ready.done():
                self.ready.set_exception(error)
            if self.pending is not None and not self.pending.done():
                self.pending.set_exception(error)
            self.changed(self.record)

    async def probe(self):
        before = time.monotonic()
        sample = {'at_seconds': before - self.started, 'active_bulk_streams': self.active(),
            'completed': False, 'ack_within_budget_proven': False if self.record['ack_budget_ms'] is not None else None}
        self.record['probes'].append(sample)
        def fault_marker():
            path = self.world.get('fault_state_file')
            return json.loads(Path(path).read_text()) if path else None
        try:
            sample['fault_before'] = fault_marker()
            reply, request_id = await asyncio.to_thread(self.api.call, 'POST',
                f'/machines/{self.world["machine"]}/probe', timeout=OBSERVATION_SECONDS, include_request_id=True)
            sample['request_id'] = request_id
            sample['completion_ms'] = (time.monotonic() - before) * 1000
            sample['completed'] = reply.get('id') == self.world['machine']
            sample['fault_after'] = fault_marker()
            sample['fault_status'] = classify(sample)
            budget = self.record['ack_budget_ms']
            # A slower terminal answer says nothing conclusive about the earlier ACK.
            if budget is None:
                sample['ack_within_budget_proven'] = None
                sample['ack_status'] = 'exact_ack_not_observed_no_floor_latency_promise'
            elif sample['fault_status'] == 'intentional_fault':
                sample['ack_within_budget_proven'] = False
                sample['ack_status'] = 'intentional_blackout_overlap_not_steady_profile_latency'
            else:
                sample['ack_within_budget_proven'] = sample['completed'] and sample['completion_ms'] <= budget
                sample['ack_status'] = 'proven_by_completion_bound' if sample['ack_within_budget_proven'] else 'exact_ack_not_observed'
        except Exception as error:
            sample['failure'] = type(error).__name__
        self.changed(self.record)

    async def echo(self):
        number = self.next_marker
        before = time.monotonic()
        sample = {'sequence': number, 'at_seconds': before - self.started,
            'active_bulk_streams': self.active(), 'observed': False}
        self.record['terminal'].append(sample)
        try:
            if self.record['terminal_fault'] is not None or self.pending is not None:
                raise ConnectionError('terminal observer is unavailable')
            self.pending = asyncio.get_running_loop().create_future()
            future = self.pending
            async def send_and_observe():
                await self.socket.send(2, marker_command(self.prefix + str(number)))
                return await asyncio.shield(future)
            at = await asyncio.wait_for(send_and_observe(), OBSERVATION_SECONDS)
            sample.update(observed=True, echo_ms=(at - before) * 1000)
            self.next_marker += 1
        except Exception as error:
            sample['failure'] = type(error).__name__
            # Do not send another marker after uncertain acceptance of the previous one.
            self.record['terminal_fault'] = self.record['terminal_fault'] or type(error).__name__
        self.changed(self.record)

    async def watch(self, ready, done):
        start = asyncio.create_task(ready.wait())
        finish = asyncio.create_task(done.wait())
        try:
            await asyncio.wait((start, finish), return_when=asyncio.FIRST_COMPLETED)
            if not ready.is_set():
                return
            async def observe(one):
                while not done.is_set():
                    await one()
                    try:
                        await asyncio.wait_for(done.wait(), SAMPLE_SECONDS)
                    except asyncio.TimeoutError:
                        pass
            await asyncio.gather(observe(self.probe), observe(self.echo))
        finally:
            start.cancel()
            finish.cancel()
            await asyncio.gather(start, finish, return_exceptions=True)

    async def close(self):
        if self.reader is not None:
            self.reader.cancel()
            await asyncio.gather(self.reader, return_exceptions=True)
        if self.socket is not None:
            try:
                await asyncio.wait_for(self.socket.close(), OBSERVATION_SECONDS)
            except (OSError, asyncio.TimeoutError) as error:
                self.record['terminal_socket_cleanup_failure'] = type(error).__name__
        if self.terminal_id is not None:
            try:
                reply = await asyncio.to_thread(self.api.call, 'DELETE', f'/terminals/{self.terminal_id}',
                    timeout=OBSERVATION_SECONDS)
                self.record['terminal_closed'] = reply.get('id') == self.terminal_id and reply.get('state') == 'closed'
            except Exception as error:
                self.record['terminal_cleanup_failure'] = type(error).__name__
        self.changed(self.record)
