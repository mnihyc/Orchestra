#!/usr/bin/env python3
"""Owned QA binary inputs and finite process checkpoints for the existing WSS runner."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import time
import uuid
import zipfile

ROLES = ('api', 'gateway', 'connector')
PHASES = ('concurrent', 'cut', 'reconnect', 'floor')
REQUIRED_QUEUES = {
    'api': {'api.terminal_out', 'api.backplane', 'controller.down', 'controller.notices'},
    'gateway': {'gateway.backplane', 'link.control', 'link.data', 'session.inbound'},
    'connector': {'machine.inputs', 'link.control', 'link.data', 'session.inbound'},
}


def read(path):
    return json.loads(Path(path).read_text())


def sha(path):
    result = hashlib.sha256()
    with Path(path).open('rb') as source:
        for block in iter(lambda: source.read(1024 * 1024), b''):
            result.update(block)
    return result.hexdigest()


def write(path, value):
    path = Path(path)
    if path.is_symlink() or path.parent.resolve() != path.parent:
        raise ValueError('QA evidence path must be canonical and not a symlink')
    temporary = path.with_name(path.name + '.' + uuid.uuid4().hex)
    with temporary.open('x') as target:
        json.dump(value, target, indent=2)
        target.write('\n')
    os.replace(temporary, path)


def build_record(path, source, role=None, program=None):
    record = read(path)
    if record.get('source') != source or record.get('source_unchanged') is not True or record.get('exit_code') != 0:
        raise ValueError('build receipt does not attest the exact unchanged successful candidate')
    for boundary in ('source_before', 'source_after'):
        snapshot = record.get(boundary, {})
        if snapshot.get('head') != source or snapshot.get('diff_sha256') != hashlib.sha256(b'').hexdigest() \
                or snapshot.get('untracked_source') != {}:
            raise ValueError('unchanged dirty source is not a clean frozen build')
    command = record.get('command')
    if not isinstance(command, list) or not all(isinstance(part, str) for part in command):
        raise ValueError('build receipt has no actual command vector')
    if '--all-features' in command:
        raise ValueError('all-features output cannot stand in for selected normal or QA artifacts')
    selected = ' '.join(command)
    if not any(step in command for step in ('build', 'package')) or '--check' in command:
        raise ValueError('a check-only receipt is not an executable build receipt')
    if role is None and ('test-observers' in selected or '--features' in command):
        raise ValueError('normal package build selected QA observers')
    if role is not None and role + '/test-observers' not in selected:
        raise ValueError('QA build did not explicitly select this app observer feature')
    summary = {'sha256': sha(path), 'source': source, 'successful_unchanged_build': True, 'clean_source_verified': True,
        'selection': 'normal' if role is None else 'explicit_test_observers'}
    if role is not None:
        if program is None:
            raise ValueError('QA build validation requires the actual selected executable')
        from build_capture import artifact_events, build_target, selection
        if record.get('producer') != 'wss_qa_cargo_capture_v1' or record.get('successful_capture') is not True \
                or not isinstance(record.get('compiler'), str) or not record['compiler'].startswith('rustc '):
            raise ValueError('QA receipt was not emitted by the actual successful compiler invocation')
        packages = selection(command)
        if role not in packages:
            raise ValueError('this app was not selected by the captured invocation')
        item = record.get('artifacts', {}).get(role, {})
        event = item.get('compiler_artifact', {})
        manifest = Path(event.get('manifest_path', ''))
        if not manifest.is_absolute():
            raise ValueError('compiler did not report an absolute app manifest')
        invocation = record.get('execution', {})
        repo = Path(invocation.get('producer_worktree', ''))
        if not repo.is_absolute() or manifest != repo / 'apps' / role.removeprefix('orch-') / 'Cargo.toml':
            raise ValueError('compiler artifact is not from the captured source worktree')
        validated = artifact_events([event, record.get('build_finished', {})], [role], repo)[role]
        target = build_target(command, packages, record['compiler'])
        if target != record.get('build_target') or target != item.get('build_target'):
            raise ValueError('QA build target differs from the normal role-specific target')
        actual = program.resolve(strict=True)
        if item.get('source') != source or item.get('path') != validated['path'] \
                or item.get('sha256') != sha(actual):
            raise ValueError('selected executable SHA differs from the exact output captured during this successful build')
        summary.update(artifact_bound=True, artifact=item, compiler=record['compiler'],
            execution=invocation, build_target=target, selected_input={'path': str(actual), 'sha256': item['sha256'],
                'binding': 'byte_identical_to_captured_compiler_output'})
    return summary


def seal(args):
    manifest = read(args.release / 'version.json')
    source = manifest['commit']
    record = {'source': source, 'version': manifest['version'], 'tag': manifest['tag'], 'normal_manifest_sha256': sha(args.release / 'version.json'),
        'normal_build': build_record(args.normal_record, source), 'max_commands': args.max_commands,
        'programs': {}}
    if args.max_commands <= 0:
        raise ValueError('the reviewed run must supply a positive finite command budget')
    for role in ROLES:
        program = getattr(args, role).resolve(strict=True)
        receipt = getattr(args, role + '_record')
        record['programs'][role] = {'path': os.path.relpath(program, args.output.resolve().parent),
            'sha256': sha(program), 'build': build_record(receipt, source, 'orch-' + role, program)}
        expected_target = os.uname().machine + '-unknown-linux-' + ('musl' if role == 'connector' else 'gnu')
        if record['programs'][role]['build']['build_target'] != expected_target:
            raise ValueError('QA target differs from the normal package selected for this native WSS fixture')
    write(args.output.resolve(), record)


def prepare(args):
    supplied = read(args.manifest)
    normal = read(args.release / 'version.json')
    if supplied.get('source') != normal['commit'] or supplied.get('normal_manifest_sha256') != sha(args.release / 'version.json') \
            or supplied.get('version') != normal['version'] or supplied.get('tag') != normal['tag']:
        raise ValueError('QA inputs and normal package identify different candidates')
    if supplied.get('normal_build', {}).get('successful_unchanged_build') is not True or supplied['normal_build'].get('clean_source_verified') is not True \
            or type(supplied.get('max_commands')) is not int or supplied['max_commands'] <= 0:
        raise ValueError('QA inputs lack normal build evidence or a finite command budget')
    directory = args.work / 'qa'
    directory.mkdir(mode=0o700)
    (directory / 'bin').mkdir(mode=0o700)
    programs = {}
    for role in ROLES:
        item = supplied['programs'][role]
        source = (args.manifest.parent / item['path']).resolve(strict=True)
        if (item.get('build', {}).get('source') != normal['commit'] or item['build'].get('selection') != 'explicit_test_observers' \
                or item['build'].get('successful_unchanged_build') is not True or item['build'].get('clean_source_verified') is not True
                or item['build'].get('artifact_bound') is not True or item['build'].get('artifact', {}).get('sha256') != item['sha256']
                or item['build']['artifact'].get('source') != normal['commit'] or sha(source) != item['sha256']):
            raise ValueError('QA executable differs from its captured build input')
        destination = directory / 'bin' / ('orch-' + role)
        shutil.copyfile(source, destination)
        destination.chmod(0o755)
        if sha(destination) != item['sha256']:
            raise ValueError('the prelaunch copied QA executable differs from its sealed input')
        observations = directory / role
        observations.mkdir(mode=0o700)
        programs[role] = {**item, 'path': str(destination), 'output': str(observations)}
    write(args.evidence / 'qa-preflight.json', {**supplied, 'programs': programs,
        'input_manifest_sha256': sha(args.manifest), 'scope': 'separate_feature_qa_artifacts'})


def environment(preflight, run, machine):
    return {'ORCH_TEST_WSS_OBSERVER_DIR': '/qa-observers', 'ORCH_TEST_WSS_RUN': run,
        'ORCH_TEST_WSS_MACHINE': machine, 'ORCH_TEST_WSS_SOURCE': preflight['source'],
        'ORCH_TEST_WSS_MAX_COMMANDS': str(preflight['max_commands'])}


def docker(*arguments):
    return subprocess.check_output(['docker', *arguments], text=True).strip()


def normal_identity(args):
    manifest_path = args.release / 'version.json'
    manifest = read(manifest_path)
    source = manifest['commit']
    programs = {}
    server = args.release / ('orch-server-linux-' + os.uname().machine + '.zip')
    with zipfile.ZipFile(server) as archive:
        expected = {}
        for role in ('api', 'gateway'):
            with archive.open('bin/orch-' + role) as program:
                digest = hashlib.sha256()
                for block in iter(lambda: program.read(1024 * 1024), b''): digest.update(block)
                expected[role] = digest.hexdigest()
    expected['connector'] = sha(args.release / ('orch-connector-linux-' + os.uname().machine + '-musl'))
    for role in ROLES:
        container = getattr(args, role)
        labels = json.loads(docker('inspect', '--format', '{{json .Config.Labels}}', container)) or {}
        if (role == 'connector' and labels.get('org.orch.smoke.run') != args.run) or \
                (role != 'connector' and labels.get('com.docker.compose.project') != args.project):
            raise ValueError('normal binary identity was requested from another fixture')
        process = 1
        if role == 'connector':
            docker('cp', str(Path(__file__).parent / 'socket_facts.py'), container + ':/tmp/release-socket-facts.py')
            facts = json.loads(docker('exec', container, 'python3', '/tmp/release-socket-facts.py', '--gateway-port', str(args.gateway_port)))
            if len(facts.get('connector_processes', [])) != 1:
                raise ValueError('normal connector process identity is ambiguous')
            process = facts['connector_processes'][0]['pid']
        actual = docker('exec', container, 'sha256sum', f'/proc/{process}/exe').split()[0]
        if actual != expected[role]:
            raise ValueError('normal running executable differs from its package')
        programs[role] = {'sha256': actual, 'container_id': docker('inspect', '--format', '{{.Id}}', container), 'process': process}
    build = build_record(args.normal_record, source) if args.normal_record else None
    if args.qa_manifest:
        sealed = read(args.qa_manifest)
        if sealed.get('source') != source or sealed.get('normal_manifest_sha256') != sha(manifest_path):
            raise ValueError('QA inputs have another normal package')
        build = sealed.get('normal_build')
    repo = Path(__file__).resolve().parents[3]
    harness_source = subprocess.check_output(['git', '-C', str(repo), 'rev-parse', 'HEAD'], text=True).strip()
    harness_clean = not subprocess.check_output(['git', '-C', str(repo), 'status', '--porcelain'], text=True).strip()
    identity = {'mode': 'normal', 'source': source, 'version': manifest['version'], 'tag': manifest['tag'],
        'harness_source': harness_source, 'harness_clean': harness_clean,
        'manifest_sha256': sha(manifest_path), 'normal_build': build, 'programs': programs,
        'run': args.run, 'machine': args.machine, 'scope': 'actual_packaged_process_bytes_before_measurement'}
    write(args.evidence / 'normal-bootstrap-identity.json', identity)
    write(args.evidence / 'run-identity.json', identity)


def override(args):
    preflight = read(args.evidence / 'qa-preflight.json')
    world = read(args.work / 'world.json')
    services = {}
    for role in ('api', 'gateway'):
        item = preflight['programs'][role]
        services[role] = {'environment': environment(preflight, args.run, world['machine']),
            'volumes': [
                {'type': 'bind', 'source': item['path'], 'target': '/opt/orch/bin/orch-' + role, 'read_only': True},
                {'type': 'bind', 'source': item['output'], 'target': '/qa-observers'},
            ]}
    write(args.work / 'qa' / 'compose.json', {'services': services})
    normal = read(args.evidence / 'normal-bootstrap-identity.json')
    write(args.evidence / 'run-identity.json', {'mode': 'qa_starting', 'run': args.run,
        'source': preflight['source'], 'normal_bootstrap': normal,
        'scope': 'QA transition requested; no measurement proof yet'})


def validate_snapshot(record, preflight, run, machine, role, expected_instance=None, require_active=True):
    identity = record.get('identity', {})
    if record.get('scope') != 'explicit_test_observers_qa_process' or record.get('state') != 'running' or record.get('complete_wss_release_gate') is not False:
        raise ValueError('QA snapshot is absent, stopped, or falsely labels itself release qualified')
    if identity.get('run') != run or identity.get('component') != role or identity.get('machine') != machine \
            or identity.get('declared_source_commit') != preflight['source'] \
            or identity.get('executable_sha256') != preflight['programs'][role]['sha256']:
        raise ValueError('QA process identity differs from the selected copied executable or run')
    build = identity.get('build', {})
    if build.get('component') != role or build.get('version') != preflight['version'] or build.get('tag') != preflight['tag'] \
            or identity.get('max_commands') != preflight['max_commands']:
        raise ValueError('QA compiled build identity or observer budget differs from the selected candidate')
    if type(identity.get('process')) is not int or identity['process'] <= 0:
        raise ValueError('QA process did not report an actual PID')
    instance = identity.get('instance')
    uuid.UUID(instance)
    if record.get('clock_domain') != instance or (expected_instance is not None and instance != expected_instance):
        raise ValueError('QA process instance changed across the measured phase')
    if role == 'connector' and record.get('observations', {}).get('actual_connector_machine') != machine:
        raise ValueError('the QA connector did not observe the selected enrolled identity')
    if not REQUIRED_QUEUES[role] <= {row.get('label') for row in record.get('observations', {}).get('queues', [])
            if not require_active or row.get('receiver_closed') is False}:
        raise ValueError('the actual QA path has not constructed its required queue families')
    return instance


def await_ready(args):
    preflight = read(args.evidence / 'qa-preflight.json')
    machine = read(args.work / 'world.json')['machine']
    until = time.monotonic() + 240  # Existing native fixture readiness guard, not a performance SLA.
    last = None
    while time.monotonic() < until:
        try:
            instances, processes = {}, {}
            for role in ROLES:
                paths = list((args.work / 'qa' / role).glob(role + '-*.json'))
                if len(paths) != 1:
                    raise ValueError('QA process did not publish exactly one owned instance')
                record = read(paths[0])
                instances[role] = validate_snapshot(record, preflight, args.run, machine, role)
                processes[role] = record['identity']['process']
            write(args.evidence / 'qa-instances.json', {'run': args.run, 'machine': machine, 'instances': instances})
            normal = read(args.evidence / 'normal-bootstrap-identity.json')
            write(args.evidence / 'run-identity.json', {key: normal[key] for key in ('source', 'version', 'tag', 'manifest_sha256', 'run')} | {'mode': 'qa',
                'scope': 'separate_feature_qa_processes_not_normal_package_proof',
                'normal_bootstrap': normal,
                'qa_preflight_sha256': sha(args.evidence / 'qa-preflight.json'), 'instances': instances,
                'machine': machine, 'processes': processes})
            return
        except (ValueError, OSError) as error:
            last = str(error)
        time.sleep(0.25)  # Same existing fixture observation cadence.
    raise TimeoutError('QA processes were not ready: ' + str(last))


def checkpoint_run(args):
    preflight = read(args.evidence / 'qa-preflight.json')
    current = read(args.evidence / 'qa-instances.json')
    request = {'run': args.run, 'id': str(uuid.uuid4()), 'phase': args.phase, 'boundary': args.boundary}
    if (args.phase, args.boundary) not in [('idle', 'start'), ('shutdown', 'pre_stop')] + [(phase, boundary) for phase in PHASES for boundary in ('start', 'drained')]:
        raise ValueError('checkpoint is outside the existing finite phase sequence')
    for role in ROLES:
        write(args.work / 'qa' / role / 'checkpoint-request.json', request)
    until = time.monotonic() + 30  # Existing observation/PTY hang guard; no elapsed-time admission.
    while time.monotonic() < until:
        captured = {}
        for role in ROLES:
            instance = current['instances'][role]
            path = args.work / 'qa' / role / f'checkpoint-{request["id"]}-{role}-{instance}.json'
            if path.exists():
                record = read(path)
                validate_snapshot(record, preflight, args.run, current['machine'], role, instance)
                if record.get('checkpoint') != request or (args.boundary != 'start' and record.get('open_drained') is not True):
                    raise ValueError('QA checkpoint did not observe the requested open phase boundary')
                captured[role] = record
        if len(captured) == len(ROLES):
            write(args.evidence / f'qa-{args.phase}-{args.boundary}.json', {'request': request, 'processes': captured})
            return
        time.sleep(0.25)
    raise TimeoutError('the actual QA processes did not acknowledge this open checkpoint')


def fault(args):
    history_path = args.evidence / 'fault-history.json'
    history = read(history_path) if history_path.exists() else []
    if args.generation != len(history) or args.generation not in (0, 1, 2):
        raise ValueError('the deliberate fault must advance exactly once into and out of blackout')
    marker = {'run': args.run, 'generation': args.generation, 'active': args.generation == 1}
    write(history_path, history + [marker])
    write(args.work / 'client-fixture' / 'fault-state.json', marker)


def stop_connector(args):
    preflight = read(args.evidence / 'qa-preflight.json')
    current = read(args.evidence / 'qa-instances.json')
    instance = current['instances']['connector']
    record = read(args.work / 'qa' / 'connector' / ('connector-' + instance + '.json'))
    validate_snapshot(record, preflight, args.run, current['machine'], 'connector', instance, require_active=False)
    labels = json.loads(docker('inspect', '--format', '{{json .Config.Labels}}', args.container)) or {}
    if labels.get('org.orch.smoke.run') != args.run:
        raise ValueError('refusing to stop a connector outside this fixture')
    # Exact owned process and actual executable bytes are checked in its namespace.
    program = '''import hashlib,os,signal,sys,time
pid=int(sys.argv[1]); expected=sys.argv[2]
with open('/proc/%s/exe'%pid,'rb') as f:
 h=hashlib.file_digest(f,'sha256').hexdigest()
if h!=expected: raise SystemExit('owned connector executable changed')
os.kill(pid,signal.SIGTERM)
end=time.monotonic()+30
while os.path.exists('/proc/%s'%pid):
 if time.monotonic()>=end: raise SystemExit('owned connector did not stop')
 time.sleep(.25)
'''
    docker('exec', args.container, 'python3', '-c', program,
        str(record['identity']['process']), preflight['programs']['connector']['sha256'])


def collect_final(args):
    preflight = read(args.evidence / 'qa-preflight.json')
    current = read(args.evidence / 'qa-instances.json')
    captured, failures = {}, []
    for role in ROLES:
        instance = current['instances'][role]
        try:
            record = read(args.work / 'qa' / role / (role + '-' + instance + '.json'))
            state = record.get('state')
            # Reuse identity checks, while separately requiring a completed process lifecycle.
            validate_snapshot({**record, 'state': 'running'}, preflight, args.run, current['machine'], role, instance, require_active=False)
            if state != 'finished':
                failures.append(role + ': process did not finish its observed lifecycle')
            captured[role] = record
        except (OSError, ValueError) as error:
            failures.append(role + ': ' + type(error).__name__)
    write(args.evidence / 'qa-final.json', {'complete': not failures, 'processes': captured, 'failures': failures})
    if failures:
        raise ValueError('QA process shutdown observations are incomplete')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='action', required=True)
    sealing = sub.add_parser('seal')
    for name in ('release', 'normal-record', 'output'):
        sealing.add_argument('--' + name, type=Path, required=True)
    for role in ROLES:
        sealing.add_argument('--' + role, type=Path, required=True)
        sealing.add_argument('--' + role + '-record', type=Path, required=True)
    sealing.add_argument('--max-commands', type=int, required=True)
    for name in ('prepare', 'override', 'ready', 'checkpoint', 'stop-connector', 'final', 'fault'):
        command = sub.add_parser(name)
        command.add_argument('--work', type=Path, required=True)
        command.add_argument('--evidence', type=Path, required=True)
        command.add_argument('--run', required=True)
        if name == 'prepare':
            command.add_argument('--manifest', type=Path, required=True)
            command.add_argument('--release', type=Path, required=True)
        if name == 'checkpoint':
            command.add_argument('--phase', required=True)
            command.add_argument('--boundary', required=True)
        if name == 'stop-connector':
            command.add_argument('--container', required=True)
        if name == 'fault':
            command.add_argument('--generation', type=int, required=True)
    normal = sub.add_parser('normal')
    for name in ('release', 'evidence'):
        normal.add_argument('--' + name, type=Path, required=True)
    for name in ('run', 'project', 'machine', *ROLES):
        normal.add_argument('--' + name, required=True)
    normal.add_argument('--gateway-port', type=int, required=True)
    normal.add_argument('--normal-record', type=Path)
    normal.add_argument('--qa-manifest', type=Path)
    args = parser.parse_args()
    {'seal': seal, 'prepare': prepare, 'override': override, 'ready': await_ready, 'checkpoint': checkpoint_run,
        'normal': normal_identity, 'stop-connector': stop_connector, 'final': collect_final, 'fault': fault}[args.action](args)


if __name__ == '__main__':
    main()
