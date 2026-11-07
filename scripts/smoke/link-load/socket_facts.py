#!/usr/bin/env python3
"""Non-secret socket/RSS evidence from the owned connector's network namespace."""
import argparse
import json
import os
import ipaddress
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
        def address(value):
            raw = bytes.fromhex(value)
            raw = raw[::-1] if family == 'ipv4' else b''.join(raw[start:start + 4][::-1] for start in range(0, 16, 4))
            ip = ipaddress.ip_address(raw)
            return str(getattr(ip, 'ipv4_mapped', None) or ip)
        found.append({'family': family, 'local_address_hex': local[0], 'local_port': int(local[1], 16),
            'remote_address_hex': remote[0], 'remote_port': int(remote[1], 16),
            'local_ip': address(local[0]), 'remote_ip': address(remote[0]),
            'inode': parts[9] if len(parts) > 9 else None,
            'tcp_send_queue_bytes': int(tx, 16), 'tcp_receive_queue_bytes': int(rx, 16)})
    return found


def collect(gateway_port, process_name='orch-connector'):
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
            if fields.get('Name', '').strip() == process_name:
                owned = set()
                for fd in (path / 'fd').iterdir():
                    try:
                        target = os.readlink(fd)
                        if target.startswith('socket:[') and target.endswith(']'):
                            owned.add(target[8:-1])
                    except OSError:
                        pass
                processes.append({'pid': int(path.name), 'rss_kib': int(fields['VmRSS'].split()[0]),
                    'peak_rss_kib': int(fields['VmHWM'].split()[0])})
                for row in found:
                    if row['inode'] in owned:
                        row.setdefault('process_ids', []).append(int(path.name))
        except (OSError, KeyError):
            continue
    return {'network_namespace': os.readlink('/proc/self/ns/net'), 'gateway_port': gateway_port,
        'gateway_tcp_connections': found, 'connector_processes': processes if process_name == 'orch-connector' else [],
        'processes': processes, 'observed_process_name': process_name,
        'queue_scope': 'kernel TCP socket queues only; application control/backplane queues unobserved'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--gateway-port', '--target-port', type=int, required=True)
    parser.add_argument('--process-name', choices=('orch-connector', 'orch-gateway'), default='orch-connector')
    args = parser.parse_args()
    if not 1 <= args.gateway_port <= 65535:
        parser.error('gateway port is invalid')
    print(json.dumps(collect(args.gateway_port, args.process_name)))


if __name__ == '__main__':
    main()
