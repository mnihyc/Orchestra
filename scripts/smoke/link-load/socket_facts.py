#!/usr/bin/env python3
"""Non-secret socket/RSS evidence from the owned connector's network namespace."""
import argparse
import json
import os
from pathlib import Path


def sockets(text, family, gateway_port):
    found = []
    for line in text.splitlines()[1:]:
        parts = line.split()
        if len(parts) < 5 or parts[3] != '01':  # TCP_ESTABLISHED
            continue
        local, remote = (endpoint.rsplit(':', 1) for endpoint in parts[1:3])
        if int(remote[1], 16) != gateway_port:
            continue
        tx, rx = parts[4].split(':')
        found.append({'family': family, 'local_address_hex': local[0], 'local_port': int(local[1], 16),
            'remote_address_hex': remote[0], 'remote_port': int(remote[1], 16),
            'tcp_send_queue_bytes': int(tx, 16), 'tcp_receive_queue_bytes': int(rx, 16)})
    return found


def collect(gateway_port):
    found = []
    for table, family in [('tcp', 'ipv4'), ('tcp6', 'ipv6')]:
        path = Path('/proc/net') / table
        if path.exists():
            found.extend(sockets(path.read_text(), family, gateway_port))
    processes = []
    for path in Path('/proc').iterdir():
        if not path.name.isdigit():
            continue
        try:
            fields = dict(line.split(':', 1) for line in (path / 'status').read_text().splitlines() if ':' in line)
            if fields.get('Name', '').strip() == 'orch-connector':
                processes.append({'pid': int(path.name), 'rss_kib': int(fields['VmRSS'].split()[0]),
                    'peak_rss_kib': int(fields['VmHWM'].split()[0])})
        except (OSError, KeyError):
            continue
    return {'network_namespace': os.readlink('/proc/self/ns/net'), 'gateway_port': gateway_port,
        'gateway_tcp_connections': found, 'connector_processes': processes,
        'queue_scope': 'kernel TCP socket queues only; application control/backplane queues unobserved'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--gateway-port', type=int, required=True)
    args = parser.parse_args()
    if not 1 <= args.gateway_port <= 65535:
        parser.error('gateway port is invalid')
    print(json.dumps(collect(args.gateway_port)))


if __name__ == '__main__':
    main()
