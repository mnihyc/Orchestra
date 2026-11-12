#!/usr/bin/env python3
"""Opt-in QA build receipt producer. Never infer a build from an existing executable.

Run on the host under its existing resource/process supervisor; the optional builder
mode uses the normal packaging mounts and image without requiring Python in it.
The normal package builder does not import or invoke this module.
"""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import os
import re
import subprocess
import sys
import time

from qa import ROLES, sha, write


def source_state(repo):
    def git(*args):
        return subprocess.check_output(['git', '-C', str(repo), *args])
    return {'head': git('rev-parse', 'HEAD').decode().strip(),
        'diff_sha256': hashlib.sha256(git('diff', '--binary', 'HEAD')).hexdigest(),
        'untracked_source': {name: sha(repo / name) for name in
            git('ls-files', '--others', '--exclude-standard', '-z').decode().split('\0')
            if name and (repo / name).is_file()}}


def clean(snapshot, source):
    return snapshot == {'head': source, 'diff_sha256': hashlib.sha256(b'').hexdigest(), 'untracked_source': {}}


def selection(command):
    # This explicit QA entrypoint accepts Cargo build only, never shell commands,
    # check-only/package receipts or a late attachment to a different invocation.
    if command[:2] != ['cargo', 'build'] or '--locked' not in command or '--release' not in command or '--all-features' in command \
            or any(part == '--profile' or part.startswith('--profile=') for part in command) \
            or '--message-format=json' not in command or '--message-format' in command \
            or sum(part.startswith('--message-format=') for part in command) != 1:
        raise ValueError('capture requires cargo build --release --locked --message-format=json with explicit QA features')
    packages, features = [], []
    index = 2
    while index < len(command):
        part = command[index]
        if part in ('-p', '--package', '--features', '--target'):
            if index + 1 == len(command): raise ValueError('missing Cargo selection')
            if part != '--target':
                (features if part == '--features' else packages).extend(command[index + 1].split(','))
            index += 2
        elif part in ('--release', '--locked', '--message-format=json', '--offline', '--frozen'):
            index += 1
        else:
            raise ValueError('use the existing normal builder selection; unsupported Cargo override: ' + part)
    if not packages or len(set(packages)) != len(packages) or any(package not in ['orch-' + role for role in ROLES] for package in packages) \
            or set(features) != {package + '/test-observers' for package in packages}:
        raise ValueError('capture only the explicitly selected WSS QA applications')
    return packages


def artifact_events(events, packages, repo):
    """Validate actual compiler output; path/hash comes from this invocation, not a CLI binary."""
    finished = [event for event in events if event.get('reason') == 'build-finished']
    if len(finished) != 1 or finished[0].get('success') is not True:
        raise ValueError('Cargo did not report one successful build completion')
    artifacts = {}
    for package in packages:
        rows = [event for event in events if event.get('reason') == 'compiler-artifact'
            and event.get('target', {}).get('name') == package and event.get('executable')]
        if len(rows) != 1:
            raise ValueError('Cargo did not emit one executable artifact for ' + package)
        row = rows[0]
        expected_manifest = repo / 'apps' / package.removeprefix('orch-') / 'Cargo.toml'
        if row.get('target', {}).get('kind') != ['bin'] or row.get('profile', {}).get('test') is not False \
                or row['profile'].get('opt_level') != '3' or row['profile'].get('debug_assertions') is not False \
                or row['profile'].get('overflow_checks') is not False \
                or PurePosixPath(row.get('manifest_path', '')) != PurePosixPath(expected_manifest) \
                or not isinstance(row.get('package_id'), str) or not row['package_id'] \
                or set(row.get('features', [])) - {'default', 'test-observers'} \
                or 'test-observers' not in row.get('features', []) \
                or type(row.get('fresh')) is not bool:
            raise ValueError('compiler artifact does not identify the actual selected app/features')
        path = PurePosixPath(row['executable'])
        if not path.is_absolute() or '..' in path.parts or str(path) != row['executable']:
            raise ValueError('compiler executable path must be absolute and canonical')
        artifacts[package] = {'path': str(path), 'compiler_artifact': row}
    return artifacts


def build_target(command, packages, compiler):
    hosts = [line[6:] for line in compiler.splitlines() if line.startswith('host: ')]
    if len(hosts) != 1 or hosts[0] not in ('x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu'):
        raise ValueError('this WSS gate needs the existing native Linux service builder')
    targets = [command[index + 1] for index, part in enumerate(command[:-1]) if part == '--target']
    if any(part.startswith('--target=') for part in command) or command[-1] == '--target' or len(targets) > 1:
        raise ValueError('target selection must be one explicit normal-builder target')
    wanted = hosts[0].replace('-gnu', '-musl') if packages == ['orch-connector'] else hosts[0]
    if ('orch-connector' in packages and packages != ['orch-connector']) or targets != ([wanted] if packages == ['orch-connector'] else []):
        raise ValueError('QA must use normal server host target or the same-architecture connector musl target')
    return wanted


def builder_invocation(args, repo, command):
    image = getattr(args, 'builder_image', None)
    if not image:
        return {'command': command, 'compiler_command': ['rustc', '--version', '--verbose'],
            'producer_worktree': str(repo), 'target_mount': None, 'host_target': None, 'image_id': None}
    if not re.fullmatch(r'sha256:[0-9a-f]{64}', image):
        raise ValueError('use the actual normal builder image ID, not a mutable tag')
    paths = {}
    for name in ('target_dir', 'cargo_home', 'counters'):
        value = getattr(args, name, None)
        if value is None: raise ValueError('builder capture needs the existing owned ' + name)
        paths[name] = value.resolve(strict=True)
        if not paths[name].is_dir(): raise ValueError('builder mount is not a directory')
    if not getattr(args, 'release_version', None) or not getattr(args, 'release_tag', None):
        raise ValueError('use the normal package release identity')
    # Mirrors container::Builder::container and package::build; no Dockerfile or
    # normal builder mutation. Host Git/receipt operations need no in-image Python.
    prefix = ['docker', 'run', '--rm', '--user', str(os.getuid()) + ':' + str(os.getgid()),
        '--volume', str(repo) + ':/work:ro', '--volume', str(paths['target_dir']) + ':/target',
        '--volume', str(paths['cargo_home']) + ':/cargo-home', '--volume', str(paths['counters']) + ':/counters',
        '--workdir', '/work', '--env', 'HOME=/tmp', '--env', 'CARGO_HOME=/cargo-home',
        '--env', 'CARGO_TARGET_DIR=/target', '--env', 'ORCH_BUILD_NUMBER_DIR=/counters',
        '--env', 'CARGO_INCREMENTAL=0', '--env', 'CARGO_BUILD_JOBS', '--env', 'RUST_MIN_STACK',
        '--env', 'ORCH_RELEASE_VERSION=' + args.release_version, '--env', 'ORCH_RELEASE_TAG=' + args.release_tag]
    if 'orch-connector' not in selection(command):
        architecture = os.uname().machine
        if architecture not in ('x86_64', 'aarch64'): raise ValueError('unsupported normal Linux server architecture')
        prefix += ['--env', 'OPENSSL_STATIC=1', '--env', 'OPENSSL_LIB_DIR=/usr/lib/' + architecture + '-linux-gnu',
            '--env', 'OPENSSL_INCLUDE_DIR=/usr/include']
    return {'command': [*prefix, image, *command],
        'compiler_command': [*prefix, image, 'rustc', '--version', '--verbose'],
        'producer_worktree': '/work', 'target_mount': '/target', 'host_target': str(paths['target_dir']), 'image_id': image}


def produced_path(artifact, invocation, target):
    if invocation['target_mount'] is None:
        return Path(artifact['path'])
    producer = PurePosixPath(artifact['path'])
    expected = PurePosixPath('/target')
    if target.endswith('-musl'): expected /= target
    expected = expected / 'release' / artifact['compiler_artifact']['target']['name']
    if producer != expected:
        raise ValueError('compiler executable is outside the exact normal builder output path')
    return Path(invocation['host_target']).joinpath(*producer.relative_to('/target').parts)


def capture(args):
    repo = args.repo.resolve(strict=True)
    output = args.output.resolve()
    if output.exists(): raise ValueError('capture output must be fresh')
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    packages = selection(command)
    before = source_state(repo)
    if not clean(before, args.source): raise ValueError('build source is not the exact frozen clean candidate')
    invocation = builder_invocation(args, repo, command)
    compiler = subprocess.check_output(invocation['compiler_command'], cwd=repo, text=True).strip()
    target = build_target(command, packages, compiler)
    events, artifacts, failure, code = [], {}, None, None
    started = time.monotonic()
    # stdout is the actual Cargo JSON stream. Diagnostics remain on stderr;
    # no credentials, environment dump or payload is copied into the receipt.
    with subprocess.Popen(invocation['command'], cwd=repo, stdout=subprocess.PIPE, text=True) as child:
        try:
            for line in child.stdout:
                row = json.loads(line)
                if row.get('reason') in ('compiler-artifact', 'build-finished'):
                    events.append(row)
                elif row.get('reason') == 'compiler-message':
                    rendered = row.get('message', {}).get('rendered')
                    if rendered: print(rendered, file=sys.stderr, end='')
            code = child.wait()
            if code != 0: raise ValueError('Cargo build failed')
            artifacts = artifact_events(events, packages, Path(invocation['producer_worktree']))
            for artifact in artifacts.values():
                host_path = produced_path(artifact, invocation, target).resolve(strict=True)
                artifact['sha256'] = sha(host_path)
                artifact['host_path'] = str(host_path)
                artifact['build_target'] = target
                artifact['source'] = args.source
        except (ValueError, OSError) as error:
            failure = str(error)
            if child.poll() is None: child.terminate()
            code = child.wait()
    after = source_state(repo)
    success = failure is None and code == 0 and before == after and clean(after, args.source)
    record = {'producer': 'wss_qa_cargo_capture_v1', 'source': args.source, 'worktree': str(repo),
        'source_before': before, 'source_after': after, 'source_unchanged': before == after,
        'command': command, 'execution': invocation, 'build_target': target, 'exit_code': code, 'compiler': compiler,
        'elapsed_seconds': time.monotonic() - started, 'successful_capture': success,
        'build_finished': next((row for row in events if row.get('reason') == 'build-finished'), None),
        'artifacts': artifacts, 'failure': failure, 'complete_wss_release_gate': False}
    write(output, record)
    if not success: raise ValueError('QA build/artifact capture failed; inspect retained receipt')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', type=Path, required=True)
    parser.add_argument('--source', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--builder-image', help='exact image ID used by the paired normal package build')
    for name in ('target-dir', 'cargo-home', 'counters'):
        parser.add_argument('--' + name, type=Path)
    parser.add_argument('--release-version')
    parser.add_argument('--release-tag')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    capture(parser.parse_args())


if __name__ == '__main__':
    main()
