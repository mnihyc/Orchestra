#!/usr/bin/env python3
"""Sample only recorded fixture containers; RSS is supporting evidence, not a queue bound."""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import signal
import subprocess
import threading
import time


def sample(connector, containers, gateway_port, phase):
    row = {'at': datetime.now(timezone.utc).isoformat(), 'monotonic_seconds': time.monotonic(), 'phase': phase,
        'whole_session_queue_bound': 'not_observed'}
    try:
        # Only label-owned fixture names/IDs supplied by the caller reach Docker.
        raw = subprocess.check_output(['docker', 'exec', connector, 'python3', '/tmp/release-socket-facts.py',
            '--gateway-port', str(gateway_port)], text=True, timeout=30)
        row['connector'] = json.loads(raw)
        raw = subprocess.check_output(['docker', 'stats', '--no-stream', '--format', '{{json .}}', *containers],
            text=True, timeout=30)
        # Docker working-set memory is not labeled as process RSS.
        row['containers'] = [json.loads(line) for line in raw.splitlines() if line]
        row['complete'] = len(row['containers']) == len(containers)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        row.update(complete=False, failure=type(error).__name__)
    return row


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--connector', required=True)
    parser.add_argument('--containers', nargs='+', required=True)
    parser.add_argument('--gateway-port', type=int, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--phase-file', type=Path, required=True)
    parser.add_argument('--once', action='store_true')
    args = parser.parse_args()
    stopped = threading.Event()
    for number in (signal.SIGTERM, signal.SIGINT):
        signal.signal(number, lambda *_: stopped.set())
    while True:
        phase = args.phase_file.read_text().strip()
        row = sample(args.connector, args.containers, args.gateway_port, phase)
        with args.out.open('a') as target:
            target.write(json.dumps(row) + '\n')
        if args.once or stopped.wait(20):  # Existing heartbeat cadence; measurement density only.
            return


if __name__ == '__main__':
    main()
