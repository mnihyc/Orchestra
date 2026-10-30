"""Run the exact Storage commands in a private PostgreSQL 17 Compose stack.

Arguments: orch-node executable, orchctl executable, artifact directory. Reuses the real uninstall
stack fixture; no model service starts. Directories and logs remain for review. Only the exact
private Compose project is removed. The copied commands themselves are split as POSIX shell text,
without interpolation, so quoted deployment paths are exercised as shown.
"""
import importlib.util
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import uuid

FIXTURES = Path(__file__).with_name('uninstall-stack.py')
spec = importlib.util.spec_from_file_location('custody_stack', FIXTURES)
if spec is None or spec.loader is None:
    raise RuntimeError('the shared disposable stack fixture is missing')
stack = importlib.util.module_from_spec(spec)
spec.loader.exec_module(stack)

ARTIFACTS = Path(sys.argv[3]).resolve()
ARTIFACTS.mkdir(parents=True, exist_ok=True)


def sql(prefix, text, env, log):
    return stack.checked([*prefix, 'exec', '-T', 'postgres', 'psql', '-U', 'orch', '-d', 'orch',
                          '-v', 'ON_ERROR_STOP=1', '-q', '-t', '-A'], env=env, log=log, data=text.encode())


def policy(prefix, value, env, log):
    sql(prefix, "INSERT INTO settings(node_id,key,value) VALUES(NULL,'cleanup.release_keep_count'," +
        str(value) + "::text::jsonb) ON CONFLICT(node_id,key) DO UPDATE SET value=excluded.value;", env, log)


def report(root):
    return json.loads((root / 'runtime/update/storage.json').read_text())


def release_names(root):
    return {payload: {path.name for path in (root / f'runtime/{payload}/releases').iterdir()}
            for payload in ['server', 'frontend', 'cli']}


def run_command(command, env, log, *, data=None, succeeds=True, extra=()):
    arguments = [*shlex.split(command), *extra]
    if succeeds:
        return stack.checked(arguments, env=env, log=log, data=data)
    log.write('RUN expecting refusal ' + ' '.join(arguments) + '\n')
    result = subprocess.run(arguments, env=env, input=data, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, timeout=stack.COMMAND_SECONDS)
    log.write(result.stdout.decode(errors='replace')); log.flush()
    assert result.returncode != 0, 'the unconfirmed or changed plan must be refused'
    return result.stdout


def backup(root, env, log):
    directory = root / 'runtime/update-backups'
    before = set(directory.glob('*.dump'))
    stack.checked([root / 'update.sh', 'backup'], env=env, log=log)
    made = set(directory.glob('*.dump')) - before
    assert len(made) == 1
    dump = made.pop()
    original = json.loads(dump.with_suffix('.manifest.json').read_text())
    row = next(item['record'] for item in report(root)['items'] if item['record']['id'] == 'dump:' + dump.name)
    assert row['blocker'] == 'paired_copy_unrecorded' and not row.get('operator_command')
    assert row['bytes'] is None
    copy = directory / (dump.name + '.data')
    shutil.copytree(root / 'runtime/data', copy)
    stack.checked([root / 'update.sh', 'backup', '--repair-record', dump.name,
                   '--paired-blobs', copy.name], env=env, log=log)
    paired = json.loads(dump.with_suffix('.manifest.json').read_text())
    for key in ['database_at', 'completed_at', 'cause', 'dump']:
        assert paired[key] == original[key], 'pairing must preserve the original snapshot identity'
    row = next(item['record'] for item in report(root)['items'] if item['record']['id'] == 'dump:' + dump.name)
    assert row['operator_command'] and row['bytes'] is not None
    return dump, copy, row['operator_command']


def main():
    profile = ARTIFACTS / ('docker-profile-' + uuid.uuid4().hex)
    profile.mkdir(); (profile / 'config.json').write_text('{"auths":{}}')
    env = {**os.environ, 'DOCKER_CONFIG': str(profile), 'LANG': 'C.UTF-8', 'LC_ALL': 'C.UTF-8', 'TMPDIR': str(ARTIFACTS)}
    socket = os.environ.get('ORCH_TEST_DOCKER_SOCKET', '/var/run/docker.sock')
    env['DOCKER_HOST'] = socket if '://' in socket else 'unix://' + socket
    project = 'orchcustody-operator-' + uuid.uuid4().hex
    # Spaces must survive the exact producer's quoting and the copied command.
    root = ARTIFACTS / (project + ' files')
    prefix = ['docker', 'compose', '-p', project, '-f', str(root / 'compose.yml')]
    with (ARTIFACTS / 'operator-storage.log').open('w') as log:
        try:
            prefix = stack.setup(root, project, env, log)
            for payload in ['server', 'frontend', 'cli']:
                directory = root / f'runtime/{payload}/releases'
                for number in range(5):
                    path = directory / f'unused-{number}'
                    path.mkdir(); (path / '.complete').write_text('a' * 64 + '\n')
                    (path / 'payload').write_text(str(number)); os.utime(path, (number + 1, number + 1))
                (directory.parent / 'previous').symlink_to('releases/unused-0')
            policy(prefix, 3, env, log)
            before = release_names(root)
            stack.checked([root / 'update.sh', 'prune'], env=env, log=log)
            assert release_names(root) == before, 'the advertised default is a dry run'
            commands = {item['record']['operator_command'] for item in report(root)['items']
                        if item['record']['kind'] == 'release' and item['record'].get('operator_command')}
            assert len(commands) == 1
            command = commands.pop()
            run_command(command, env, log)
            assert release_names(root) == before
            run_command(command, env, log, extra=['--yes'])
            for names in release_names(root).values():
                assert names == {'fixture', 'unused-0', 'unused-2', 'unused-3', 'unused-4'}
            policy(prefix, 0, env, log)
            before = release_names(root)
            stack.checked([root / 'update.sh', 'prune', '--yes'], env=env, log=log)
            assert release_names(root) == before, 'zero keeps every release'
            assert not any(item['record'].get('operator_command') for item in report(root)['items'] if item['record']['kind'] == 'release')
            print('PASS exact prune command, protected links and zero', flush=True)
            dump, copy, command = backup(root, env, log)
            run_command(command, env, log, data=b'wrong name\n', succeeds=False)
            assert dump.is_file() and dump.with_suffix('.manifest.json').is_file() and copy.is_dir()
            # A real rollback reference is a counted source and is withdrawn before erasing the pair.
            state = root / 'runtime/update/state'
            state.write_text('current_tag=v0.1.0\nprevious_tag=v0.0.9\nprevious_dump=runtime/update-backups/' + dump.name + '\n')
            run_command(command, env, log, data=(dump.name + '\n').encode())
            assert not dump.exists() and not dump.with_suffix('.manifest.json').exists() and not copy.exists()
            assert 'previous_dump=\n' in state.read_text()
            assert (root / 'runtime/data/kept-fixture').read_bytes() == b'paired blob-store fixture'
            print('PASS exact typed dump-pair command and live-store preservation', flush=True)
            dump, copy, command = backup(root, env, log)
            parked = copy.with_name(copy.name + '.original')
            copy.rename(parked); copy.mkdir(); (copy / 'foreign').write_bytes(b'keep replaced data')
            run_command(command, env, log, extra=['--yes'], succeeds=False)
            assert dump.is_file() and parked.is_dir() and (copy / 'foreign').read_bytes() == b'keep replaced data'
            changed = next(item['record'] for item in report(root)['items'] if item['record']['id'] == 'dump:' + dump.name)
            assert changed['blocker'] == 'paired_copy_changed' and not changed.get('operator_command')
            print('PASS replacement refusal with no guessed data deletion', flush=True)
        finally:
            subprocess.run([*map(str, prefix), 'down', '--volumes', '--timeout', '10'], env=env, stdout=log, stderr=log)


if __name__ == '__main__':
    main()
