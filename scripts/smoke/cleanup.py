#!/usr/bin/env python3
"""Preflight one owned smoke directory and render its literal absolute removal command."""
import argparse
import json
from pathlib import Path
import re
import shlex
import subprocess


def checked_path(repo, base, run, name):
    if not re.fullmatch(r'[a-z0-9]+', run) or not re.fullmatch(r'[a-z0-9-]+', name):
        raise ValueError('invalid owned smoke run or scenario')
    repo = repo.resolve(strict=True)
    common = Path(subprocess.check_output(
        ['git', '-C', str(repo), 'rev-parse', '--path-format=absolute', '--git-common-dir'],
        text=True).strip()).resolve(strict=True)
    project = common.parent
    if not repo.is_relative_to(project):
        raise ValueError('the checkout must belong to this project')
    base = base.absolute()
    if not base.is_relative_to(project) or base == project:
        raise ValueError('smoke files must stay below this project')
    work = base / 'runs' / run / name
    evidence = base / 'evidence' / run / name
    for target in (work, evidence):
        for path in [target, *target.parents]:
            if path.is_symlink():
                raise ValueError('a smoke work or evidence path must not traverse a symlink')
            if path == project:
                break
        if target.resolve() != target:
            raise ValueError('smoke work or evidence path is not canonical')
    for filename in ('cleanup-files.sh', 'cleanup-files.json', 'cleanup.json', 'runner.json',
            'netem-image-cleanup.json', 'netem-built-image-id.txt', 'harness-final.json'):
        if (evidence / filename).is_symlink():
            raise ValueError('a smoke cleanup evidence file must not be a symlink')
    return work


def removal_script(work):
    if not work.is_absolute():
        raise ValueError('cleanup requires a literal absolute path')
    # This is the only file deletion command. Its argument is literal in the
    # reviewed script, never a shell variable or command substitution.
    return '#!/usr/bin/env bash\nset -euo pipefail\nrm -r -- ' + shlex.quote(str(work)) + '\n'


def prepare(repo, base, run, name):
    work = checked_path(repo, base, run, name)
    evidence = base.absolute() / 'evidence' / run / name
    if work.exists() or evidence.exists():
        raise ValueError('the smoke run already has files or evidence; choose a fresh run identity')
    work.parent.mkdir(parents=True, exist_ok=True)
    work.mkdir(mode=0o700)
    evidence.mkdir(parents=True)
    checked_path(repo, base, run, name)
    stat = work.stat()
    script = evidence / 'cleanup-files.sh'
    script.write_text(removal_script(work))
    script.chmod(0o600)
    record = {'work': str(work), 'device': stat.st_dev, 'inode': stat.st_ino,
        'script': str(script), 'preflight_before_launch': True, 'files_removed': False}
    (evidence / 'cleanup-files.json').write_text(json.dumps(record, indent=2) + '\n')
    print('cleanup preflight: ' + str(script))


def execute(repo, base, run, name):
    work = checked_path(repo, base, run, name)
    evidence = base.absolute() / 'evidence' / run / name
    record_path = evidence / 'cleanup-files.json'
    if record_path.is_symlink():
        raise ValueError('the cleanup receipt must not be a symlink')
    record = json.loads(record_path.read_text())
    script = evidence / 'cleanup-files.sh'
    if record.get('work') != str(work) or record.get('script') != str(script) \
            or script.is_symlink() or script.read_text() != removal_script(work):
        raise ValueError('cleanup command differs from the prelaunch preflight')
    if not work.is_dir() or (work.stat().st_dev, work.stat().st_ino) != (record['device'], record['inode']):
        raise ValueError('the owned work directory was replaced or already removed')
    # Revalidate immediately before the single literal command, after callers
    # have verified owned container/network/volume cleanup.
    subprocess.run(['bash', '-n', str(script)], check=True)
    subprocess.run(['bash', str(script)], check=True)
    if work.exists():
        raise ValueError('the owned work directory remains after cleanup')
    checked_path(repo, base, run, name)
    record['files_removed'] = True
    record_path.write_text(json.dumps(record, indent=2) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('prepare', 'execute', 'validate'))
    parser.add_argument('--repo', type=Path, required=True)
    parser.add_argument('--base', type=Path, required=True)
    parser.add_argument('--run', required=True)
    parser.add_argument('--name', required=True)
    args = parser.parse_args()
    try:
        action = {'prepare': prepare, 'execute': execute, 'validate': checked_path}[args.action]
        action(args.repo, args.base, args.run, args.name)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        parser.exit(2, 'smoke cleanup: ' + str(error) + '\n')


if __name__ == '__main__':
    main()
