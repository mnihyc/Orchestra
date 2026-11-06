#!/usr/bin/env python3
"""Reject a shaping label unsupported by actual namespace, socket and qdisc evidence."""
import argparse
import json
from pathlib import Path


def verify(placement, namespace, connector, upload, download):
    selected = placement.get('placement')
    if selected not in ('client', 'connector') or not isinstance(namespace, str):
        raise ValueError('missing shaping placement or network namespace')
    if selected == 'connector' and namespace != connector.get('network_namespace'):
        raise ValueError('the shaped namespace is not the actual connector namespace')
    if selected == 'client' and namespace == connector.get('network_namespace'):
        raise ValueError('client and connector attribution is ambiguous')
    if not any(row.get('remote_port') == placement.get('gateway_port')
            for row in connector.get('gateway_tcp_connections', [])):
        raise ValueError('no actual connector session to the pinned gateway was observed')
    for direction in (upload, download):
        rows = [row for row in direction if row.get('kind') == 'netem']
        if len(rows) != 1 or not all(type(rows[0].get(field)) is int and rows[0][field] > 0
                for field in ('bytes', 'packets', 'drops')):
            raise ValueError('actual bidirectional netem traffic/loss counters are missing')
    return {'placement': selected, 'namespace_and_gateway_socket_verified': True,
        'bidirectional_packet_loss_observed': True, 'complete_wss_release_gate': False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('evidence', type=Path)
    args = parser.parse_args()
    read = lambda name: json.loads((args.evidence / name).read_text())
    result = verify(read('shaping-placement.json'), read('shaped-netns.json'),
        read('connector-socket-before.json'), read('profile-upload-qdisc-final.json'), read('profile-download-qdisc-final.json'))
    (args.evidence / 'placement-qualification.json').write_text(json.dumps(result, indent=2) + '\n')


if __name__ == '__main__':
    main()
