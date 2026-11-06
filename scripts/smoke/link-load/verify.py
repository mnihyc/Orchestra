#!/usr/bin/env python3
"""Join the two ends' independent byte evidence, including the deliberately abandoned stream."""
import argparse
import json
import math
from pathlib import Path
from peer import digest

PHASES = ('concurrent', 'cut', 'reconnect', 'floor')


def release_qualification(phases, placement, release_mode):
    """The existing release-proof mode requires complete evidence, never byte-only success."""
    missing = [name for name in PHASES if not isinstance(phases.get(name), dict)
        or phases[name].get('complete_wss_release_gate') is not True
        or phases[name].get('remaining_observers') != []]
    if not isinstance(placement, dict) or placement.get('namespace_and_gateway_socket_verified') is not True \
            or placement.get('bidirectional_packet_loss_observed') is not True:
        missing.append('placement')
    result = {'wss_release_gate_qualified': not missing, 'release_mode': release_mode == '1',
        'missing_or_incomplete': missing}
    return result, 2 if result['release_mode'] and missing else 0


def release_guard(evidence, release_mode):
    def read(name):
        try:
            return json.loads((evidence / name).read_text())
        except (OSError, ValueError):
            return None
    phases = {name: read(name + '-qualification.json') for name in PHASES}
    result, status = release_qualification(phases, read('placement-qualification.json'), release_mode)
    (evidence / 'wss-release-qualification.json').write_text(json.dumps(result, indent=2) + '\n')
    return status


def verify_mixed(record, profile='profile'):
    if not isinstance(record, dict) or not record.get('terminal_created') or not record.get('terminal_closed'):
        raise ValueError('owned terminal observation or cleanup is missing')
    if profile not in ('profile', 'floor') or record.get('profile') != profile:
        raise ValueError('mixed observations do not identify the actual measured profile')
    budget = 1000 if profile == 'profile' else None
    if record.get('command_timing') != 'public_completion_upper_bound_on_ack' \
            or 'ack_budget_ms' not in record or record['ack_budget_ms'] != budget:
        raise ValueError('command timing evidence changed its meaning or budget')
    if record.get('terminal_fault') is not None:
        raise ValueError('the owned terminal failed during observation')
    if any(record.get(field) is not None for field in (
            'watcher_cleanup_failure', 'terminal_socket_cleanup_failure', 'terminal_cleanup_failure')):
        raise ValueError('an observer or owned socket did not clean up')
    probes, terminal = record.get('probes', []), record.get('terminal', [])
    if not probes or any(not sample.get('completed') for sample in probes):
        raise ValueError('a public command did not complete')
    if not terminal or any(not sample.get('observed') for sample in terminal):
        raise ValueError('a terminal marker was not observed')
    if [sample.get('sequence') for sample in terminal] != list(range(len(terminal))):
        raise ValueError('terminal markers were duplicated, omitted or reordered')
    for samples in (probes, terminal):
        if not any(sample.get('active_bulk_streams', 0) > 0 for sample in samples):
            raise ValueError('the observations did not overlap actual bulk streams')
    loaded = [sample for sample in probes if sample.get('active_bulk_streams', 0) > 0]
    proven = all(sample.get('ack_within_budget_proven') and
        type(sample.get('completion_ms')) in (int, float) and math.isfinite(sample['completion_ms']) and
        0 <= sample['completion_ms'] <= 1000 for sample in loaded)
    return {'ordered_terminal_markers': True, 'commands_complete_during_bulk': True,
        'profile': profile,
        'command_ack_budget': 'not_required_at_floor' if profile == 'floor' else
            'proven_by_completion_upper_bound' if proven else 'exact_ack_not_observed',
        'whole_session_queue_bound': 'not_observed',
        'complete_wss_release_gate': False,
        'remaining_observers': (['whole_session_application_queue_bounds'] +
            ([] if profile == 'floor' or proven else ['exact_command_ack_timestamps']))}


def verify(client, peer, require_mixed=False):
    if not client.get('passed') or not client.get('streams'):
        raise ValueError('the actual client did not complete')
    seeds = [row['seed'] for row in client['streams']]
    if len(set(seeds)) != len(seeds):
        raise ValueError('two connections reused a stream identity')
    for row in client['streams']:
        other = peer[row['seed']]
        if not other['closed'] or other.get('send_stuck'):
            raise ValueError('a cut or half-close left its peer stuck')
        if row['cut']:
            if not 0 < row['download_bytes'] < row['expected_bytes']:
                raise ValueError('the deliberate cut was not an interrupted stream')
            if other['upload_bytes'] > row['upload_bytes'] or other['download_bytes'] < row['download_bytes']:
                raise ValueError('the abandoned stream has impossible byte counts')
            continue
        count, seed = row['expected_bytes'], row['seed']
        if row['upload_bytes'] != count or row['download_bytes'] != count or other['upload_bytes'] != count or other['download_bytes'] != count:
            raise ValueError('a direction was shortened or duplicated')
        if other['upload_sha256'] != digest(seed, 'upload', count) or row['download_sha256'] != digest(seed, 'download', count):
            raise ValueError('cross-stream, cross-direction or changed bytes')
        if require_mixed:
            for direction in ('upload', 'download'):
                points = [point for point in row.get('progress', []) if point.get('direction') == direction]
                offsets = [point.get('offset') for point in points]
                if not points or offsets[-1] != count or any(
                        not isinstance(offset, int) or not 0 < offset <= count for offset in offsets) or any(
                        right <= left for left, right in zip(offsets, offsets[1:])):
                    raise ValueError('ordered progress checkpoints are missing or invalid')
    if require_mixed:
        return verify_mixed(client.get('mixed'), client.get('profile'))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--client', type=Path)
    parser.add_argument('--peer', type=Path)
    parser.add_argument('--require-mixed', action='store_true')
    parser.add_argument('--qualification', type=Path)
    parser.add_argument('--release-guard', type=Path)
    parser.add_argument('--release-mode', default='0')
    args = parser.parse_args()
    if args.release_guard is not None:
        return release_guard(args.release_guard, args.release_mode)
    if args.client is None or args.peer is None:
        parser.error('--client and --peer are required for byte verification')
    client = json.loads(args.client.read_text())
    peer = {row['seed']: json.loads((args.peer / (row['seed'] + '.json')).read_text()) for row in client['streams']}
    qualification = verify(client, peer, args.require_mixed)
    if args.qualification:
        args.qualification.write_text(json.dumps(qualification, indent=2) + '\n')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
