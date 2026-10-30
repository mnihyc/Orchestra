#!/usr/bin/env python3
"""A measured-duration candidate soak, with accepted-work and database-ledger reconciliation."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import sys
import time
import urllib.error
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'load'))
from common import append, check_work, docker, input_body, load_world, resources, settled, thread, until


def duration_result(seconds, actual):
    return {'requested_seconds': seconds, 'observed_seconds': actual,
        'full_24_hours': seconds >= 86400 and actual >= 86400,
        'owed_24_hours': seconds < 86400 or actual < 86400}


def ledger_check(expected, actual):
    if len(expected) != len({row['key'] for row in expected}):
        raise ValueError('the scenario reused an idempotency key')
    indexed = {row['key']: row for row in actual}
    if len(indexed) != len(actual) or set(indexed) != {row['key'] for row in expected}:
        raise ValueError('the final database lost or duplicated an accepted input')
    for row in expected:
        stored = indexed[row['key']]
        if stored['input'] != row['input'] or stored['turns'] != 1:
            raise ValueError('an idempotent resend made another input or turn')
        if stored['state'] not in ('completed', 'failed', 'interrupted'):
            raise ValueError('work remained silently pending after recovery')
        if stored['outputs'] > 1 or (stored['state'] == 'completed' and stored['outputs'] != 1):
            raise ValueError('completed output was lost or duplicated')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--world', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--budgets', type=Path, required=True)
    parser.add_argument('--stack', type=Path, required=True)
    parser.add_argument('--project', required=True)
    parser.add_argument('--lab', required=True)
    parser.add_argument('--run', required=True)
    parser.add_argument('--seconds', type=int, default=1800)
    args = parser.parse_args()
    if args.seconds < 60 or not re.fullmatch('[a-z0-9]+', args.run):
        parser.error('a unique alphanumeric run and a duration of at least 60 seconds are required')
    budgets = json.loads(args.budgets.read_text())
    world, api = load_world(args.world)
    args.out.mkdir(parents=True, exist_ok=True)
    prefix = 'soak' + args.run
    ids = [thread(api, world, f'Soak {index:02}') for index in range(20)]

    def compose(*command):
        return subprocess.check_output(['docker', 'compose', *command], cwd=args.stack, timeout=180, text=True)

    def queues():
        return api.call('GET', '/admin/system')['queues']

    def fault(kind):
        record = {'kind': kind, 'started': time.time()}
        if kind in ('api', 'worker', 'gateway'):
            compose('restart', kind)
        elif kind == 'connector':
            docker('restart', args.lab)
            # This fixture's PID 1 is sleep, so explicitly resume the released connector after restart.
            docker('exec', '-d', args.lab, 'sh', '-c', 'exec orch-connector --data-dir /data run >> /data/run.log 2>&1')
        else:
            network = args.project + '_control'
            docker('network', 'disconnect', network, args.lab)
            try:
                time.sleep(3)  # Intentional network outage with exact owned endpoints.
            finally:
                docker('network', 'connect', network, args.lab)
        record['returned'] = time.time()
        append(args.out / 'faults.jsonl', record)

    # Warm the finite set of sessions before comparing post-work resources.
    for index, value in enumerate(ids):
        key, marker = prefix + 'warm' + str(index), 'warm' + uuid.uuid4().hex
        reply = api.call('POST', f'/threads/{value}/inputs', input_body(key, marker))
        sync = settled(api, value, reply['input']['id'], budgets['turn_guard_seconds'])
        check_work(sync, reply['input']['id'], marker)
    baseline_queues = queues()
    baseline = resources(args.project, args.lab)
    append(args.out / 'resources.jsonl', {'phase': 'warm', **baseline})
    started = time.monotonic()
    expected, failures = [], []
    kinds = ['api', 'worker', 'gateway', 'connector', 'network']
    cycle = 0
    try:
        while time.monotonic() - started < args.seconds:
            value = ids[cycle % len(ids)]
            marker = 'result' + uuid.uuid4().hex
            key = prefix + 'work' + str(cycle)
            body = input_body(key, marker, delay_ms=2000)
            reply = api.call('POST', f'/threads/{value}/inputs', body)
            input_id = reply['input']['id']
            until('the accepted work never started before the fault', lambda: api.call('GET', f'/threads/{value}/sync'),
                lambda row: any(turn['input_id'] == input_id and turn['state'] == 'running' for turn in row['turns']),
                seconds=budgets['turn_guard_seconds'])
            kind = kinds[cycle % len(kinds)]
            fault(kind)
            # Replay the SAME request after an uncertain response/reconnect; never invent another key.
            def resend():
                try:
                    return api.call('POST', f'/threads/{value}/inputs', body)
                except (urllib.error.URLError, TimeoutError, ConnectionError):
                    return None
            replay = until('the system did not recover for an idempotent resend', resend, lambda row: row is not None,
                seconds=budgets['turn_guard_seconds'])
            if replay['input']['id'] != input_id:
                raise ValueError('an acknowledged input was accepted twice after a restart')
            sync = settled(api, value, input_id, budgets['turn_guard_seconds'])
            record = {**check_work(sync, input_id, marker, allow_interruption=True), 'key': key,
                'thread': value, 'marker': marker, 'fault': kind, 'cycle': cycle}
            expected.append(record)
            append(args.out / 'work.jsonl', record)
            if record['state'] != 'completed':
                # A named terminal fault is evidence, never silently rewritten as a successful turn.
                failures.append(record)
            snapshot = resources(args.project, args.lab)
            append(args.out / 'resources.jsonl', {'phase': 'fault-recovered', 'cycle': cycle, **snapshot})
            if snapshot['bytes'] > budgets['base_stack_peak_bytes']:
                raise ValueError('the stack exceeded its declared allocation')
            cycle += 1
        elapsed = time.monotonic() - started
        # No accepted work can stay silently queued once faults stop.
        until('queues did not drain back to the warm baseline', queues,
            lambda row: all(row[key] <= baseline_queues[key] for key in baseline_queues),
            seconds=budgets['turn_guard_seconds'])
        after = resources(args.project, args.lab)
        append(args.out / 'resources.jsonl', {'phase': 'quiet', **after})
        if after['bytes'] > baseline['bytes'] + budgets['quiet_memory_growth_bytes']:
            raise ValueError('quiet memory growth exceeded the declared fixture allowance')
        if after['fds'] > baseline['fds'] + budgets['quiet_fd_growth']:
            raise ValueError('quiet descriptor growth exceeded the declared fixture allowance')
        # Read the release's own PostgreSQL ledger, not the helper's reconstruction of its API replies.
        query = f"""SELECT json_build_object('key', i.client_id, 'input', i.id,
          'turns', (SELECT count(*) FROM turns t WHERE t.input_id=i.id),
          'state', (SELECT t.state FROM turns t WHERE t.input_id=i.id ORDER BY t.ordinal DESC LIMIT 1),
          'outputs', (SELECT count(*) FROM turns t JOIN items m ON m.turn_id=t.id
            WHERE t.input_id=i.id AND m.kind='agent_message' AND m.payload->>'text' LIKE 'result%'))
          FROM thread_inputs i WHERE i.client_id LIKE '{prefix}work%' ORDER BY i.created_at"""
        raw = compose('exec', '-T', 'postgres', 'psql', '-U', 'orch', '-d', 'orch', '-At', '-c', query)
        actual = [json.loads(line) for line in raw.splitlines() if line]
        (args.out / 'database-work.json').write_text(json.dumps(actual, indent=2))
        ledger_check(expected, actual)
        result = {**duration_result(args.seconds, elapsed), 'cycles': cycle, 'fault_kinds': kinds,
            'accepted_inputs': len(expected), 'explicit_interrupted_or_failed': len(failures),
            'lost_or_duplicate': 0, 'queues_baseline': baseline_queues, 'queues_after': queues(),
            'quiet_memory_growth_bytes': after['bytes'] - baseline['bytes'],
            'quiet_fd_growth': after['fds'] - baseline['fds'], 'passed': not failures,
            'durability_reconciled': True,
            'all_turns_completed': not failures}
        (args.out / 'soak-result.json').write_text(json.dumps(result, indent=2))
        if failures:
            # The durability audit can pass while an unqualified "no work failed" release claim cannot.
            raise ValueError('explicit failed/interrupted work occurred; see work.jsonl, never count it as completed')
    except BaseException as error:
        (args.out / 'soak-failure.json').write_text(json.dumps({**duration_result(args.seconds, time.monotonic() - started),
            'passed': False, 'failure': type(error).__name__, 'cycles': cycle}))
        raise


if __name__ == '__main__':
    main()
