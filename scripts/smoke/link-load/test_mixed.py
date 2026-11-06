"""Pure evidence/protocol controls. These do not qualify real WSS or kernel loss."""
import asyncio
from copy import deepcopy
import json
import unittest
from unittest.mock import patch

from client import transfer
from mixed import Mixed, marker_command
from placement import verify as verify_placement
from socket_facts import sockets
from verify import PHASES, release_guard, release_qualification, verify_mixed


def observation():
    return {'terminal_created': True, 'terminal_closed': True, 'terminal_fault': None,
        'profile': 'profile', 'command_timing': 'public_completion_upper_bound_on_ack', 'ack_budget_ms': 1000,
        'probes': [{'completed': True, 'active_bulk_streams': 3, 'completion_ms': 800,
            'ack_within_budget_proven': True}],
        'terminal': [{'sequence': 0, 'observed': True, 'active_bulk_streams': 3, 'echo_ms': 950}]}


class Evidence(unittest.TestCase):
    def test_floor_records_slow_observations_without_creating_a_latency_requirement(self):
        row = observation()
        row.update(profile='floor', ack_budget_ms=None)
        row['probes'][0].update(completion_ms=4000, ack_within_budget_proven=None)
        row['terminal'][0]['echo_ms'] = 5000
        proof = verify_mixed(row, 'floor')
        self.assertEqual(proof['command_ack_budget'], 'not_required_at_floor')
        self.assertNotIn('exact_command_ack_timestamps', proof['remaining_observers'])
        self.assertEqual(proof['whole_session_queue_bound'], 'not_observed')
        self.assertFalse(proof['complete_wss_release_gate'])
        bad = deepcopy(row)
        bad['terminal_closed'] = False
        with self.assertRaises(ValueError):
            verify_mixed(bad, 'floor')
        bad = deepcopy(row)
        bad['terminal'][0]['observed'] = False
        with self.assertRaises(ValueError):
            verify_mixed(bad, 'floor')
        phases = {name: {'complete_wss_release_gate': True, 'remaining_observers': []} for name in PHASES}
        phases['floor'] = proof
        placement = {'namespace_and_gateway_socket_verified': True, 'bidirectional_packet_loss_observed': True}
        self.assertNotEqual(release_qualification(phases, placement, '1')[1], 0)
        row.update(profile='profile', ack_budget_ms=1000)
        recommended = verify_mixed(row, 'profile')
        self.assertEqual(recommended['command_ack_budget'], 'exact_ack_not_observed')
        self.assertIn('exact_command_ack_timestamps', recommended['remaining_observers'])
        with self.assertRaises(ValueError):
            verify_mixed(row, 'floor')

    def test_release_guard_saves_incomplete_receipt_before_returning_failure(self):
        files = {'placement-qualification.json': json.dumps({
            'namespace_and_gateway_socket_verified': True, 'bidirectional_packet_loss_observed': True})}
        class MemoryFile:
            def __init__(self, name):
                self.name = name
            def read_text(self):
                if self.name not in files:
                    raise FileNotFoundError(self.name)
                return files[self.name]
            def write_text(self, text):
                files[self.name] = text
        class EvidenceDirectory:
            def __truediv__(self, name):
                return MemoryFile(name)
        self.assertEqual(release_guard(EvidenceDirectory(), '1'), 2)
        saved = json.loads(files['wss-release-qualification.json'])
        self.assertFalse(saved['wss_release_gate_qualified'])
        self.assertEqual(saved['missing_or_incomplete'], list(PHASES))

    def test_release_mode_rejects_false_missing_or_incomplete_gate_evidence(self):
        complete = {name: {'complete_wss_release_gate': True, 'remaining_observers': []} for name in PHASES}
        placement = {'namespace_and_gateway_socket_verified': True, 'bidirectional_packet_loss_observed': True}
        for mode in ('0', '1'):
            result, status = release_qualification(complete, placement, mode)
            self.assertTrue(result['wss_release_gate_qualified'])
            self.assertEqual(status, 0)
        for change in (None, {'complete_wss_release_gate': False, 'remaining_observers': []},
                {'complete_wss_release_gate': True, 'remaining_observers': ['exact_command_ack_timestamps']},
                {'complete_wss_release_gate': True}):
            phases = {**complete, 'concurrent': change}
            developer, developer_status = release_qualification(phases, placement, '0')
            release, release_status = release_qualification(phases, placement, '1')
            self.assertFalse(developer['wss_release_gate_qualified'])
            self.assertEqual(developer_status, 0)
            self.assertFalse(release['wss_release_gate_qualified'])
            self.assertNotEqual(release_status, 0)
            self.assertIn('concurrent', release['missing_or_incomplete'])
        self.assertNotEqual(release_qualification(complete, None, '1')[1], 0)

    def test_public_completion_can_prove_ack_upper_bound_but_never_whole_queue_bound(self):
        proof = verify_mixed(observation())
        self.assertEqual(proof['command_ack_budget'], 'proven_by_completion_upper_bound')
        self.assertEqual(proof['whole_session_queue_bound'], 'not_observed')
        self.assertFalse(proof['complete_wss_release_gate'])
        self.assertEqual(proof['remaining_observers'], ['whole_session_application_queue_bounds'])

    def test_slow_public_completion_is_unknown_ack_not_a_proven_ack_failure(self):
        for duration in (1001, 1500, float('inf'), float('nan')):
            row = observation()
            row['probes'][0]['completion_ms'] = duration
            proof = verify_mixed(row)
            self.assertEqual(proof['command_ack_budget'], 'exact_ack_not_observed')
            self.assertIn('exact_command_ack_timestamps', proof['remaining_observers'])

    def test_missing_load_cleanup_or_order_cannot_claim_mixed_progress(self):
        cases = []
        row = observation(); row['terminal_closed'] = False; cases.append(row)
        row = observation(); row['probes'][0]['active_bulk_streams'] = 0; cases.append(row)
        row = observation(); row['terminal'][0]['active_bulk_streams'] = 0; cases.append(row)
        row = observation(); row['terminal'][0]['sequence'] = 1; cases.append(row)
        row = observation(); row['terminal'].append(deepcopy(row['terminal'][0])); cases.append(row)
        row = observation(); row['ack_budget_ms'] = 1500; cases.append(row)
        row = observation(); row['watcher_cleanup_failure'] = 'TimeoutError'; cases.append(row)
        for row in cases:
            with self.assertRaises(ValueError):
                verify_mixed(row)

    def test_marker_cannot_be_forged_by_command_echo_or_shell_syntax(self):
        marker = 'ORCHWSS_' + 'a' * 32 + '_12'
        command = marker_command(marker)
        self.assertNotIn(marker.encode(), command)
        self.assertTrue(command.startswith(b"printf '\\117\\122"))
        self.assertTrue(command.endswith(b"'\r"))
        with self.assertRaises(ValueError):
            marker_command('$(touch owner-file)')

    def test_socket_attribution_requires_established_exact_gateway_port(self):
        header = 'sl local_address rem_address st tx_queue:rx_queue\n'
        table = header + '\n'.join([
            '0: 0100007F:ABCD 0200007F:A1B1 01 00000010:00000020',
            '1: 0100007F:ABCD 0200007F:A1B2 01 00000030:00000040',
            '2: 0100007F:ABCD 0200007F:A1B1 0A 00000050:00000060'])
        rows = sockets(table, 'ipv4', int('A1B1', 16))
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]['tcp_send_queue_bytes'], 16)
        self.assertEqual(rows[0]['tcp_receive_queue_bytes'], 32)

    def test_connector_label_requires_its_own_namespace_socket_and_actual_loss(self):
        placement = {'placement': 'connector', 'gateway_port': 41401}
        connector = {'network_namespace': 'net:[42]', 'gateway_tcp_connections': [{'remote_port': 41401}]}
        qdisc = [{'kind': 'netem', 'bytes': 10000, 'packets': 100, 'drops': 1}]
        proof = verify_placement(placement, 'net:[42]', connector, qdisc, qdisc)
        self.assertTrue(proof['bidirectional_packet_loss_observed'])
        with self.assertRaisesRegex(ValueError, 'namespace'):
            verify_placement(placement, 'net:[43]', connector, qdisc, qdisc)
        with self.assertRaisesRegex(ValueError, 'socket|session'):
            verify_placement(placement, 'net:[42]', {**connector, 'gateway_tcp_connections': []}, qdisc, qdisc)
        with self.assertRaisesRegex(ValueError, 'counters'):
            verify_placement(placement, 'net:[42]', connector, qdisc, [{**qdisc[0], 'drops': 0}])


class FakeSocket:
    def __init__(self, frames):
        self.given = frames
        self.stop = asyncio.Event()

    async def frames(self):
        for row in self.given:
            yield row
            await asyncio.sleep(0)
        await self.stop.wait()


class Terminal(unittest.IsolatedAsyncioTestCase):
    def fixture(self, frames):
        with patch('mixed.Path.read_text', return_value='synthetic-test-token'):
            result = Mixed({'edge': 'unused', 'token_file': 'unused'}, None, lambda: 1)
        result.prefix = 'ORCHWSS_' + 'b' * 32 + '_'
        result.socket = FakeSocket(frames)
        result.pending = asyncio.get_running_loop().create_future()
        return result

    async def test_split_marker_ignores_announced_replay_and_is_not_counted_twice(self):
        marker = ('ORCHWSS_' + 'b' * 32 + '_0\r\n').encode()
        result = self.fixture([(1, json.dumps({'op': 'ready', 'replay_bytes': len(marker)}).encode()),
            (2, marker), (2, marker[:9]), (2, marker[9:]), (2, b'next prompt')])
        accepted = result.pending
        task = asyncio.create_task(result.read_terminal())
        try:
            await asyncio.wait_for(asyncio.shield(accepted), 1)
            await asyncio.sleep(0)
            await asyncio.sleep(0)
            self.assertEqual(result.received_markers, 1)
            self.assertIsNone(result.record['terminal_fault'])
        finally:
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)

    async def test_duplicate_terminal_marker_is_a_protocol_failure(self):
        marker = ('ORCHWSS_' + 'b' * 32 + '_0\r\n').encode()
        result = self.fixture([(1, b'{"op":"ready","replay_bytes":0}'), (2, marker + marker)])
        await result.read_terminal()
        self.assertEqual(result.received_markers, 1)
        self.assertEqual(result.record['terminal_fault'], 'ValueError')

    async def test_blocked_send_is_inside_the_existing_observation_guard(self):
        class Blocked:
            async def send(self, *_):
                await asyncio.Event().wait()
        result = self.fixture([])
        result.socket = Blocked()
        result.pending = None
        # Accelerate only the unit fixture's existing guard; no production/SLA
        # value changes and no transport, model, shell or network is executed.
        with patch('mixed.OBSERVATION_SECONDS', .001):
            await asyncio.wait_for(result.echo(), 1)
        self.assertEqual(result.record['terminal_fault'], 'TimeoutError')
        self.assertFalse(result.record['terminal'][0]['observed'])

    async def test_failed_bulk_setup_does_not_leave_watcher_waiting_for_ready(self):
        result = self.fixture([])
        ready, done = asyncio.Event(), asyncio.Event()
        done.set()
        await asyncio.wait_for(result.watch(ready, done), 1)
        self.assertEqual(result.record['probes'], [])
        self.assertEqual(result.record['terminal'], [])

    async def test_blackout_ready_requires_real_payload_in_both_directions(self):
        from peer import chunks
        class Stream:
            seed = None
            uploaded = False
            downloaded = False
            async def send(self, opcode, data):
                if self.seed is None:
                    self.seed = json.loads(data[4:])['seed']
                elif opcode == 2:
                    self.uploaded = True
            async def frames(self):
                self.downloaded = True
                yield 2, next(chunks(self.seed, 'download', 64))
                yield 1, b'eof'
            async def close(self):
                pass
        stream, ready, receiving = Stream(), [], []
        async def connect(*_):
            return stream
        def payload(_):
            self.assertTrue(stream.downloaded)
            receiving.append(True)
        def started(_):
            self.assertTrue(stream.uploaded and stream.downloaded)
            ready.append(True)
        world = {'edge': 'http://unused', 'token_file': 'unused', 'system': 'https://owned.invalid',
            'environment': 'owned', 'ca': 'unused'}
        with patch('client.API') as api, patch('client.Path.read_text', return_value='synthetic'), \
                patch('client.WebSocket.connect', side_effect=connect):
            api.return_value.call.return_value = {'ticket': 'synthetic', 'id': 'owned'}
            result = await transfer(world, 64, 41325, ready=started, payload=payload)
        self.assertEqual(ready, [True])
        self.assertEqual(receiving, [True])
        self.assertEqual((result['upload_bytes'], result['download_bytes']), (64, 64))


if __name__ == '__main__':
    unittest.main()
