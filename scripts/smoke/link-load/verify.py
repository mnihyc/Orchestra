#!/usr/bin/env python3
"""Join the two ends' independent byte evidence, including the deliberately abandoned stream."""
import argparse
import json
import math
import hashlib
from pathlib import Path
from peer import digest
from observations import verify_phase, verify_final
from placement import verify as verify_placement, verify_profile, verify_identity
from faults import classify, verify_history
from workload import phase as workload_phase

PHASES = ('concurrent', 'cut', 'reconnect', 'floor')


def release_qualification(phases, placement, release_mode):
    """The existing release-proof mode requires complete evidence, never byte-only success."""
    missing = [name for name in PHASES if not isinstance(phases.get(name), dict)
        or phases[name].get('qa_phase_observed') is not True
        or phases[name].get('remaining_observers') != []]
    if not isinstance(placement, dict) or placement.get('namespace_and_gateway_socket_verified') is not True \
            or placement.get('bidirectional_packet_loss_observed') is not True:
        missing.append('placement')
    result = {'wss_release_gate_qualified': False, 'placement_case_observed': not missing,
        'release_mode': release_mode == '1', 'scope': 'one placement; normal and QA three-leg index still required',
        'missing_or_incomplete': missing + ['normal_and_qa_three_leg_index']}
    return result, 2 if result['release_mode'] else 0


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
    if not isinstance(record, dict) or record.get('terminal_created') is not True or record.get('terminal_closed') is not True:
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
    if not probes or any(sample.get('completed') is not True for sample in probes):
        raise ValueError('a public command did not complete')
    if not terminal or any(sample.get('observed') is not True for sample in terminal):
        raise ValueError('a terminal marker was not observed')
    if [sample.get('sequence') for sample in terminal] != list(range(len(terminal))):
        raise ValueError('terminal markers were duplicated, omitted or reordered')
    for samples in (probes, terminal):
        if not any(sample.get('active_bulk_streams', 0) > 0 for sample in samples):
            raise ValueError('the observations did not overlap actual bulk streams')
    loaded = [sample for sample in probes if sample.get('active_bulk_streams', 0) > 0]
    steady = [sample for sample in loaded if classify(sample) != 'intentional_fault']
    proven = bool(steady) and all(sample.get('ack_within_budget_proven') and
        type(sample.get('completion_ms')) in (int, float) and math.isfinite(sample['completion_ms']) and
        0 <= sample['completion_ms'] <= 1000 for sample in steady)
    return {'ordered_terminal_markers': True, 'commands_complete_during_bulk': True,
        'profile': profile,
        'command_ack_budget': 'not_required_at_floor' if profile == 'floor' else
            'proven_by_completion_upper_bound' if proven else 'exact_ack_not_observed',
        'whole_session_queue_bound': 'not_observed',
        'complete_wss_release_gate': False,
        'remaining_observers': (['whole_session_application_queue_bounds'] +
            ([] if profile == 'floor' or proven else ['exact_command_ack_timestamps']))}


def verify(client, peer, require_mixed=False):
    if client.get('passed') is not True or not client.get('streams'):
        raise ValueError('the actual client did not complete')
    seeds = [row['seed'] for row in client['streams']]
    if any(not isinstance(seed, str) or not seed.isalnum() for seed in seeds) or len(set(seeds)) != len(seeds):
        raise ValueError('two connections reused a stream identity')
    for row in client['streams']:
        other = peer[row['seed']]
        if other.get('expected_bytes') != row.get('expected_bytes'):
            raise ValueError('the peer and client did not select the same actual byte count')
        if other.get('closed') is not True or other.get('send_stuck') not in (None, False):
            raise ValueError('a cut or half-close left its peer stuck')
        if row['cut']:
            if not 0 < row['download_bytes'] < row['expected_bytes']:
                raise ValueError('the deliberate cut was not an interrupted stream')
            if other['upload_bytes'] > row['upload_bytes'] or other['download_bytes'] < row['download_bytes']:
                raise ValueError('the abandoned stream has impossible byte counts')
            if row.get('download_sha256') != digest(row['seed'], 'download', row['download_bytes']) \
                    or other.get('upload_sha256') != digest(row['seed'], 'upload', other['upload_bytes']):
                raise ValueError('the abandoned stream corrupted its actually received byte prefix')
            continue
        count, seed = row['expected_bytes'], row['seed']
        if other.get('receive_error') is not None or other.get('send_errors', []) != []:
            raise ValueError('a completed stream reported a producer or receiver failure')
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


def read_case(directory, source, placement, mode):
    def read(name):
        path = directory / name
        if path.is_symlink() or path.resolve().parent != directory.resolve():
            raise ValueError('case evidence leaves its owned directory')
        return json.loads(path.read_text())
    identity, runner, cleanup = read('run-identity.json'), read('runner.json'), read('cleanup.json')
    harness_final = read('harness-final.json')
    if harness_final.get('source') != source or harness_final.get('clean') is not True:
        raise ValueError('the harness changed or became dirty during the measured case')
    if identity.get('mode') != mode or identity.get('source') != source or not identity.get('run') \
            or runner.get('executed') is not True or runner.get('process_exit') != 0 or runner.get('final_exit') != 0 \
            or runner.get('failures') != 0 or cleanup.get('complete') is not True or cleanup.get('retained_by_request') is not False:
        raise ValueError('case identity, actual execution or final cleanup is incomplete')
    netem_cleanup = read('netem-image-cleanup.json')
    if netem_cleanup.get('complete') is not True or netem_cleanup.get('outcome') != 'owned_tag_removed':
        raise ValueError('the actual run-owned shaping image was not retired')
    image = read('netem-image.json')
    built_image = (directory / 'netem-built-image-id.txt').read_text().strip()
    if len(image) != 1 or image[0].get('Id') != built_image or netem_cleanup.get('image_id') != built_image \
            or netem_cleanup.get('run') != identity['run'] \
            or (image[0].get('Config', {}).get('Labels') or {}).get('org.orch.smoke.run') != identity['run']:
        raise ValueError('image cleanup is not bound to this run and its actual built image')
    selected = read('shaping-placement.json')
    expected_process = identity.get('programs', {}).get('connector', {}).get('process') if mode == 'normal' else identity.get('processes', {}).get('connector')
    if selected.get('connector_process') != expected_process or selected.get('machine') != identity.get('machine') \
            or selected.get('run') != identity.get('run'):
        raise ValueError('socket attribution differs from the actual enrolled process/run')
    if selected.get('placement') != placement:
        raise ValueError('one shaped leg cannot stand in for a different leg')
    extra = (read('backplane-socket-before.json'), read('backplane-egress-filter.json'), read('backplane-ingress-filter.json')) \
        if placement == 'backplane' else ()
    verified_path = verify_placement(selected, read('shaped-netns.json'), read('connector-socket-before.json'),
        read('profile-upload-qdisc-final.json'), read('profile-download-qdisc-final.json'), *extra)
    verify_identity(selected, read('shaped-netns.json'), read('connector-socket-final.json'),
        read('backplane-socket-final.json') if placement == 'backplane' else None)
    for profile, rates in [('profile', (750000, 1600000)), ('floor', (75000, 160000))]:
        for direction, rate in zip(('upload', 'download'), rates):
            verify_profile((directory / f'{profile}-{direction}-qdisc-final.txt').read_text(), rate)
    exits = [json.loads(line) for line in (directory / 'exits.jsonl').read_text().splitlines()]
    if len(exits) != len(PHASES) or {row.get('phase') for row in exits} != set(PHASES) or any(row.get('exit') != 0 for row in exits):
        raise ValueError('all four actual phases must finish once without retries or missing exits')
    normal = identity if mode == 'normal' else identity.get('normal_bootstrap', {})
    build = normal.get('normal_build') or {}
    if normal.get('harness_source') != source or normal.get('harness_clean') is not True:
        raise ValueError('the measured harness was not the same clean frozen candidate')
    if build.get('source') != source or build.get('selection') != 'normal' or build.get('successful_unchanged_build') is not True or build.get('clean_source_verified') is not True:
        raise ValueError('the normal package has no matching unchanged normal-build receipt')
    if set(normal.get('programs', {})) != {'api', 'gateway', 'connector'}:
        raise ValueError('normal running executable identities are incomplete')
    preflight = read('qa-preflight.json') if mode == 'qa' else None
    pinned = read('qa-instances.json') if mode == 'qa' else None
    if mode == 'qa' and (preflight.get('source') != source or preflight.get('normal_manifest_sha256') != normal.get('manifest_sha256')
            or preflight.get('version') != normal.get('version') or preflight.get('tag') != normal.get('tag')):
        raise ValueError('the feature-QA candidate differs from its normal-package source')
    if mode == 'qa':
        if hashlib.sha256((directory / 'qa-preflight.json').read_bytes()).hexdigest() != identity.get('qa_preflight_sha256'):
            raise ValueError('QA prelaunch inputs changed after the process instances were selected')
        if set(preflight.get('programs', {})) != {'api', 'gateway', 'connector'} or any(
                item.get('build', {}).get('source') != source or item['build'].get('selection') != 'explicit_test_observers'
                or item['build'].get('successful_unchanged_build') is not True or item['build'].get('clean_source_verified') is not True
                or item['build'].get('artifact_bound') is not True or item['build'].get('artifact', {}).get('source') != source
                or item['build']['artifact'].get('sha256') != item.get('sha256') for item in preflight['programs'].values()):
            raise ValueError('QA binaries lack exact successful explicit-feature build provenance')
    phases, seeds = {}, set()
    history = read('fault-history.json')
    verify_history(history, identity['run'], blackout=placement == 'connector')
    for phase in PHASES:
        client = read(phase + '.json')
        for sample in client.get('mixed', {}).get('probes', []):
            if classify(sample, history) == 'unobserved':
                raise ValueError('actual probes lack bounded steady/fault generation observations')
        if not any(sample.get('active_bulk_streams', 0) > 0 and classify(sample, history) == 'steady'
                for sample in client.get('mixed', {}).get('probes', [])):
            raise ValueError('this positive phase has no measured steady loaded probe')
        workload = workload_phase(placement, phase)
        expected_count, expected_bytes = workload['streams'], workload['bytes']
        if client.get('harness_budget_seconds') != workload['budget_seconds']:
            raise ValueError('the measured phase does not identify its reviewed harness budget')
        rows = client.get('streams', [])
        if len(rows) != expected_count or client.get('profile') != ('floor' if phase == 'floor' else 'profile') \
                or any(row.get('expected_bytes') != expected_bytes or row.get('cut') is not (phase == 'cut') for row in rows) \
                or (phase == 'concurrent' and sum(row.get('paused_reader') is True for row in rows) != 1):
            raise ValueError('the actual workload differs from the existing streams, bytes, stall or cut case')
        current = {row['seed'] for row in rows}
        if any(not isinstance(seed, str) or not seed.isalnum() for seed in current):
            raise ValueError('invalid payload identity in case evidence')
        if seeds & current:
            raise ValueError('another phase reused its payload identity')
        seeds |= current
        peer = {row['seed']: read(row['seed'] + '.json') for row in rows}
        result = verify(client, peer, True)
        if mode == 'qa':
            result.update(verify_phase(client, read(f'qa-{phase}-start.json'), read(f'qa-{phase}-drained.json'), preflight, pinned, phase))
        phases[phase] = result
    if mode == 'qa':
        verify_final(read('qa-shutdown-pre_stop.json'), read('qa-final.json'), preflight, pinned)
    return {'run': identity['run'], 'mode': mode, 'source': source, 'manifest_sha256': normal.get('manifest_sha256'),
        'placement': placement, 'path': verified_path, 'phases': phases,
        'normal_programs': normal['programs'],
        'qa_programs': None if mode == 'normal' else {role: item['sha256'] for role, item in preflight['programs'].items()},
        'complete_wss_release_gate': False}


def release_index(index_path, expected_source):
    supplied = json.loads(index_path.read_text())
    if supplied.get('source') != expected_source:
        raise ValueError('the proof index is not the current frozen candidate')
    legs = supplied.get('placements', {})
    if set(legs) != {'client', 'connector', 'backplane'}:
        raise ValueError('all three independently shaped WSS legs are required')
    cases, paths, runs, manifest, normal_hashes, qa_hashes = {}, set(), set(), None, None, None
    root = index_path.resolve().parent
    for leg in ('client', 'connector', 'backplane'):
        if set(legs[leg]) != {'normal', 'qa'}:
            raise ValueError('each shaped leg needs distinct normal and QA evidence')
        cases[leg] = {}
        for mode in ('normal', 'qa'):
            path = (root / legs[leg][mode]).resolve(strict=True)
            if not path.is_relative_to(root) or path in paths:
                raise ValueError('proof directories must be distinct and stay within this owned evidence set')
            paths.add(path)
            record = read_case(path, expected_source, leg, mode)
            if record['run'] in runs or not record.get('manifest_sha256'):
                raise ValueError('run identity was reused or normal package identity is absent')
            runs.add(record['run'])
            if manifest is None: manifest = record['manifest_sha256']
            if record['manifest_sha256'] != manifest:
                raise ValueError('shaped cases use different normal packages')
            binaries = {role: item['sha256'] for role, item in record['normal_programs'].items()}
            if normal_hashes is None: normal_hashes = binaries
            if normal_hashes != binaries: raise ValueError('normal executable bytes changed between shaped legs')
            if mode == 'qa':
                if qa_hashes is None: qa_hashes = record['qa_programs']
                if qa_hashes != record['qa_programs']: raise ValueError('QA executable bytes changed between shaped legs')
            cases[leg][mode] = record
        if {role: item['sha256'] for role, item in cases[leg]['normal']['normal_programs'].items()} != \
                {role: item['sha256'] for role, item in cases[leg]['qa']['normal_programs'].items()}:
            raise ValueError('normal and QA bootstrap executable bytes do not identify the same package')
    return {'wss_release_gate_qualified': False, 'source': expected_source, 'manifest_sha256': manifest,
        'attributed_mixed_cases_observed': True,
        'scope': 'three actual shaped legs; separate normal functional and feature-QA observation identities', 'placements': cases,
        'missing_or_incomplete': ['combined_64_live_16_waiting_on_actual_impaired_wss'],
        'complementary_control_requirement': 'same-candidate native window/resume/capacity controls are required separately; they do not prove this combined-concurrency gap'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--client', type=Path)
    parser.add_argument('--peer', type=Path)
    parser.add_argument('--require-mixed', action='store_true')
    parser.add_argument('--qualification', type=Path)
    parser.add_argument('--release-guard', type=Path)
    parser.add_argument('--release-mode', default='0')
    parser.add_argument('--qa-evidence', type=Path)
    parser.add_argument('--phase', choices=PHASES)
    parser.add_argument('--release-index', type=Path)
    parser.add_argument('--expect-source')
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    if args.release_index is not None:
        if not args.expect_source or args.output is None:
            parser.error('release index needs the frozen expected source and an owned output path')
        if args.output.is_symlink() or args.output.resolve().parent != args.release_index.resolve().parent:
            parser.error('qualification output must remain beside the owned index')
        status = 0
        args.output.write_text(json.dumps({'wss_release_gate_qualified': False, 'source': args.expect_source,
            'state': 'verification_in_progress'}, indent=2) + '\n')
        try:
            result = release_index(args.release_index, args.expect_source)
            if result.get('wss_release_gate_qualified') is not True: status = 2
        except Exception as error:
            result = {'wss_release_gate_qualified': False, 'source': args.expect_source,
                'failure': type(error).__name__, 'detail': str(error)}
            status = 2
        args.output.write_text(json.dumps(result, indent=2) + '\n')
        return status
    if args.release_guard is not None:
        return release_guard(args.release_guard, args.release_mode)
    if args.client is None or args.peer is None:
        parser.error('--client and --peer are required for byte verification')
    client = json.loads(args.client.read_text())
    if any(not isinstance(row.get('seed'), str) or not row['seed'].isalnum() for row in client.get('streams', [])):
        raise ValueError('invalid payload identity')
    peer = {row['seed']: json.loads((args.peer / (row['seed'] + '.json')).read_text()) for row in client['streams']}
    qualification = verify(client, peer, args.require_mixed)
    if args.qa_evidence:
        if args.phase is None: parser.error('QA phase evidence needs --phase')
        read = lambda name: json.loads((args.qa_evidence / name).read_text())
        qualification.update(verify_phase(client, read(f'qa-{args.phase}-start.json'), read(f'qa-{args.phase}-drained.json'),
            read('qa-preflight.json'), read('qa-instances.json'), args.phase))
    if args.qualification:
        args.qualification.write_text(json.dumps(qualification, indent=2) + '\n')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
