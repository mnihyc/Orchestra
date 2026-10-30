#!/usr/bin/env python3
"""100 real threads through the released stack, with the fake adapter explicitly named."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import threading
import time
import uuid

from common import append, check_work, input_body, load_world, percentile, resources, settled, thread, until


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--world', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--project', required=True)
    parser.add_argument('--lab', required=True)
    parser.add_argument('--budgets', type=Path, required=True)
    args = parser.parse_args()
    world, api = load_world(args.world)
    budgets = json.loads(args.budgets.read_text())
    args.out.mkdir(parents=True, exist_ok=True)
    samples = args.out / 'resources.jsonl'
    stopped = threading.Event()

    def sample():
        while not stopped.is_set():
            append(samples, resources(args.project, args.lab))
            stopped.wait(2)  # Resource observations are independent of correctness fences.

    # Create in small batches, then release all 100 inputs at the same barrier.
    with ThreadPoolExecutor(max_workers=5) as pool:
        ids = list(pool.map(lambda index: thread(api, world, f'Load {index:03}'), range(100)))
    (args.out / 'threads.json').write_text(json.dumps(ids))
    (args.out / 'ready').write_text('100 threads created')
    # The browser runner authenticates and warms all 30 processes before this start signal.
    until('the thirty-browser observer did not become ready', lambda: (args.out / 'browsers-ready').exists(), bool, seconds=300)
    barrier = threading.Barrier(100)
    sampler = threading.Thread(target=sample)
    sampler.start()
    try:
        def send(index):
            marker, key = 'load' + uuid.uuid4().hex, str(uuid.uuid4())
            body = input_body(key, marker, delay_ms=30_000)  # Hold a real overlap window; excluded from latency claims.
            barrier.wait(timeout=60)
            before = time.monotonic()
            reply = api.call('POST', f'/threads/{ids[index]}/inputs', body)
            return {'thread': ids[index], 'input': reply['input']['id'], 'key': key,
                'marker': marker, 'ack_ms': (time.monotonic() - before) * 1000}

        with ThreadPoolExecutor(max_workers=100) as pool:
            pending = [pool.submit(send, index) for index in range(100)]
            work = [future.result() for future in pending]
            def current():
                return list(pool.map(lambda value: api.call('GET', f'/threads/{value}/sync'), ids))
            overlap = until('100 actual running turns never overlapped', current,
                lambda rows: all(row['thread']['state'] == 'running' for row in rows), seconds=60)
            (args.out / 'overlap.json').write_text(json.dumps({'at': time.time(),
                'threads': [{'id': row['thread']['id'], 'state': row['thread']['state'], 'seq': row['seq']} for row in overlap]}))
            (args.out / 'streaming').write_text('all 100 turns observed running')
            for row in work:
                sync = settled(api, row['thread'], row['input'], budgets['turn_guard_seconds'])
                append(args.out / 'work.jsonl', {**row, **check_work(sync, row['input'], row['marker'])})
        ack = percentile([row['ack_ms'] for row in work], .95)
        result = {'adapter': 'fake (scripted test double)', 'thread_count': 100, 'overlap_observed': True,
            'input_ack_p95_ms': ack, 'budget_ms': budgets['input_ack_p95_ms'], 'passed': ack <= budgets['input_ack_p95_ms']}
        (args.out / 'threads-result.json').write_text(json.dumps(result, indent=2))
        if not result['passed']:
            raise ValueError('input acknowledgement exceeded the declared load budget')
    finally:
        stopped.set()
        sampler.join(timeout=120)
    rows = [json.loads(line) for line in samples.read_text().splitlines()]
    if not rows or max(row['bytes'] for row in rows) > budgets['base_stack_peak_bytes']:
        raise ValueError('the measured stack exceeded its declared memory allocation')


if __name__ == '__main__':
    main()
