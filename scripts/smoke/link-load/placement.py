#!/usr/bin/env python3
"""Reject a shaping label unsupported by actual namespace, socket and qdisc evidence."""
import argparse
import json
import ipaddress
import re
from decimal import Decimal
from pathlib import Path


def verify_identity(placement, namespace, connector, backplane=None):
    selected = placement.get('placement')
    if selected not in ('client', 'connector', 'backplane') or not isinstance(namespace, str):
        raise ValueError('missing shaping placement or network namespace')
    if selected == 'connector' and namespace != connector.get('network_namespace'):
        raise ValueError('the shaped namespace is not the actual connector namespace')
    if selected == 'client' and namespace == connector.get('network_namespace'):
        raise ValueError('client and connector attribution is ambiguous')
    if selected == 'backplane':
        if not isinstance(backplane, dict) or namespace != backplane.get('network_namespace') or namespace == connector.get('network_namespace'):
            raise ValueError('the shaped namespace is not the actual separate gateway backplane namespace')
        target = str(ipaddress.IPv4Address(placement['backplane_ip']))
        if placement.get('backplane_port') != 8444 or backplane.get('observed_process_name') != 'orch-gateway' or not any(
                row.get('remote_ip') == target and row.get('remote_port') == 8444 and row.get('process_ids')
                for row in backplane.get('gateway_tcp_connections', [])):
            raise ValueError('no actual owned gateway socket to the selected API backplane was observed')
    process = placement.get('connector_process')
    if type(process) is not int or process <= 0 or connector.get('observed_process_name') != 'orch-connector' \
            or [row.get('pid') for row in connector.get('connector_processes', [])] != [process] \
            or not any(row.get('remote_port') == placement.get('gateway_port')
                and isinstance(row.get('inode'), str) and row['inode'].isdigit() and int(row['inode']) > 0
                and row.get('process_ids') == [process]
                for row in connector.get('gateway_tcp_connections', [])):
        raise ValueError('no socket inode is owned by the exact enrolled connector PID at the pinned gateway')
    return selected


def verify(placement, namespace, connector, upload, download, backplane=None, egress=None, ingress=None):
    selected = verify_identity(placement, namespace, connector, backplane)
    if selected == 'backplane':
        verify_backplane_filters(placement['backplane_ip'], upload, egress, ingress)
    for direction in (upload, download):
        rows = [row for row in direction if row.get('kind') == 'netem']
        if len(rows) != 1 or not all(type(rows[0].get(field)) is int and rows[0][field] > 0
                for field in ('bytes', 'packets', 'drops')):
            raise ValueError('actual bidirectional netem traffic/loss counters are missing')
    return {'placement': selected, 'namespace_and_gateway_socket_verified': True,
        'bidirectional_packet_loss_observed': True, 'complete_wss_release_gate': False}


def verify_backplane_filters(target, qdiscs, egress, ingress):
    roots = [row for row in qdiscs if row.get('kind') == 'prio' and row.get('handle') == '1:']
    if len(roots) != 1 or roots[0].get('options', {}).get('bands') != 3 or roots[0]['options'].get('priomap') != [0] * 16:
        raise ValueError('unclassified gateway traffic is not proven to stay in the unshaped band')
    def selection(rows, direction):
        filters = [row for row in (rows or []) if row.get('kind') == 'flower' and row.get('options', {}).get('keys')]
        if len(filters) != 1:
            raise ValueError('backplane needs exactly one actual tuple filter per direction')
        options = filters[0]['options']; keys = options['keys']
        wanted = 'dst' if direction == 'egress' else 'src'
        network = ipaddress.ip_network(str(keys.get(wanted + '_ip')), strict=False)
        if network.version != 4 or network.prefixlen != 32 or str(network.network_address) != target \
                or keys.get('ip_proto') not in ('tcp', 6) or str(keys.get(wanted + '_port')) != '8444':
            raise ValueError('backplane filter does not select the exact API address and TCP port')
        if direction == 'egress':
            if options.get('classid', options.get('flowid')) != '1:3':
                raise ValueError('backplane egress does not enter its selected netem band')
        else:
            actions = options.get('actions', [])
            if not any(action.get('kind') == 'mirred' and action.get('to_dev', action.get('dev')) == 'ifb0'
                    and (action.get('eaction') == 'egress_redirect' or
                        (action.get('eaction') == 'redirect' and action.get('direction') == 'egress')) for action in actions):
                raise ValueError('backplane ingress is not redirected to its owned IFB')
    selection(egress, 'egress'); selection(ingress, 'ingress')


def verify_profile(text, rate):
    rows = [line for line in text.splitlines() if re.search(r'\bnetem\b', line)]
    if len(rows) != 1:
        raise ValueError('the selected direction has no unique actual netem configuration')
    line = rows[0]
    delay = re.search(r'\bdelay\s+([0-9.]+)([num]?s)\b', line)
    loss = re.search(r'\bloss(?:\s+random)?\s+([0-9.]+)%', line)
    bandwidth = re.search(r'\brate\s+([0-9.]+)([KkMmGg]?)bit\b', line)
    units = {'s': Decimal(1), 'ms': Decimal('0.001'), 'us': Decimal('0.000001'), 'ns': Decimal('0.000000001')}
    powers = {'': 1, 'K': 1000, 'M': 1000000, 'G': 1000000000}
    if not delay or Decimal(delay[1]) * units[delay[2]] != Decimal('0.150') \
            or not loss or Decimal(loss[1]) != 1 \
            or not bandwidth or Decimal(bandwidth[1]) * powers[bandwidth[2].upper()] != rate \
            or not re.search(r'\blimit\s+1024\b', line):
        raise ValueError('actual netem delay, independent loss, rate or existing queue limit differs from the selected profile')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('evidence', type=Path)
    parser.add_argument('--preflight', action='store_true')
    args = parser.parse_args()
    read = lambda name: json.loads((args.evidence / name).read_text())
    placement = read('shaping-placement.json')
    if args.preflight:
        verify_identity(placement, read('shaped-netns.json'), read('connector-socket-before.json'),
            read('backplane-socket-before.json') if placement.get('placement') == 'backplane' else None)
        for direction, rate in [('upload', 750000), ('download', 1600000)]:
            verify_profile((args.evidence / f'{direction}-qdisc.txt').read_text(), rate)
        if placement.get('placement') == 'backplane':
            verify_backplane_filters(placement['backplane_ip'], read('upload-qdisc.json'),
                read('backplane-egress-filter.json'), read('backplane-ingress-filter.json'))
        (args.evidence / 'configuration-preflight.json').write_text(json.dumps({
            'configured_profile_verified': True, 'traffic_and_loss_proven': False,
            'complete_wss_release_gate': False}, indent=2) + '\n')
        return
    extra = (read('backplane-socket-before.json'), read('backplane-egress-filter.json'), read('backplane-ingress-filter.json')) \
        if placement.get('placement') == 'backplane' else ()
    result = verify(placement, read('shaped-netns.json'),
        read('connector-socket-before.json'), read('profile-upload-qdisc-final.json'), read('profile-download-qdisc-final.json'), *extra)
    verify_identity(placement, read('shaped-netns.json'), read('connector-socket-final.json'),
        read('backplane-socket-final.json') if placement.get('placement') == 'backplane' else None)
    for profile, rates in [('profile', (750000, 1600000)), ('floor', (75000, 160000))]:
        for direction, rate in zip(('upload', 'download'), rates):
            verify_profile((args.evidence / f'{profile}-{direction}-qdisc-final.txt').read_text(), rate)
    result['configured_profiles_verified'] = True
    (args.evidence / 'placement-qualification.json').write_text(json.dumps(result, indent=2) + '\n')


if __name__ == '__main__':
    main()
