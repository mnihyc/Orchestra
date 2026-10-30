"""Exercise the updater's actual uninstall command against isolated PostgreSQL 17 Compose stacks.

Arguments: orch-node executable, orchctl executable, artifact directory. The GNU test executables run
on Trixie stock images; released static executables work there too. No model service is started.
All fixture directories remain for review. Every created Docker object is removed by its exact name
or its private Compose project. The foreign sentinel must keep its data throughout all cases.
"""
from pathlib import Path
import hashlib
import ipaddress
import json
import os
import secrets
import shutil
import subprocess
import sys
import time
import uuid

REPO = Path(__file__).resolve().parents[3]
NODE, CTL, ARTIFACTS = (Path(value).resolve() for value in sys.argv[1:])
ARTIFACTS.mkdir(parents=True, exist_ok=True)
POSTGRES = 'postgres:17-trixie'
DEBIAN = 'debian:trixie-slim'
BUSYBOX = 'busybox:latest'
NGINX = 'nginx:1.30-alpine'
# Sixty seconds is the updater's ordinary service-stop budget, ample for fixture initialization.
READY_SECONDS = 60
# A fixture command includes the stop budget, one tiny local dump and exact native removals.
COMMAND_SECONDS = 300
# Six services and one short-lived backup client fit in one private fourteen-address network.
FIXTURE_NETWORKS = ipaddress.ip_network('10.254.0.0/16')


def unused_subnet(env, log):
    identities = checked(['docker', 'network', 'ls', '-q'], env=env, log=log).decode().split()
    occupied = []
    if identities:
        configurations = checked(['docker', 'network', 'inspect', '--format', '{{json .IPAM.Config}}',
                                  *identities], env=env, log=log)
        for line in configurations.decode().splitlines():
            for item in json.loads(line) or []:
                if item.get('Subnet'):
                    occupied.append(ipaddress.ip_network(item['Subnet']))
    for candidate in FIXTURE_NETWORKS.subnets(new_prefix=28):
        if not any(candidate.overlaps(network) for network in occupied if network.version == 4):
            return str(candidate)
    raise RuntimeError('no unused private subnet for the disposable fixture')


def checked(arguments, *, env, log, data=None, timeout=COMMAND_SECONDS):
    log.write('RUN ' + ' '.join(map(str, arguments)) + '\n')
    log.flush()
    result = subprocess.run(list(map(str, arguments)), input=data, env=env, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, timeout=timeout)
    log.write(result.stdout.decode(errors='replace'))
    log.flush()
    if result.returncode:
        raise RuntimeError(f'{arguments[0]} exited {result.returncode}; see {log.name}')
    return result.stdout


def setup(root, project, env, log):
    root.mkdir()
    (root / 'config').mkdir()
    for directory in ['runtime/postgres/data', 'runtime/data', 'runtime/node/identity', 'runtime/update', 'runtime/update-backups']:
        (root / directory).mkdir(parents=True, exist_ok=True)
    for name in ['node.orch-instance-lock', 'update/lock']:
        (root / 'runtime' / name).touch()
    (root / 'update.sh').write_bytes((REPO / 'deploy/update.sh').read_bytes())
    os.chmod(root / 'update.sh', 0o755)
    for payload in ['server', 'cli', 'frontend']:
        release = root / f'runtime/{payload}/releases/fixture'
        release.mkdir(parents=True)
        (release / '.complete').write_text('b' * 64 + '\n')
        (root / f'runtime/{payload}/current').symlink_to('releases/fixture')
    (root / 'runtime/server/current/bin').mkdir()
    shutil.copy2(NODE, root / 'runtime/server/current/bin/orch-node')
    shutil.copy2(CTL, root / 'runtime/cli/current/orchctl')
    (root / 'runtime/update/state').write_text('current_tag=v0.1.0\nprevious_tag=\nprevious_dump=\n')
    (root / 'runtime/data/kept-fixture').write_bytes(b'paired blob-store fixture')
    (root / 'runtime/node/identity/enrollment.json').write_text(json.dumps({
        'machine_id': str(uuid.uuid4()), 'name': 'Bundled fixture node',
        'system': {'name': project, 'public_url': 'http://127.0.0.1:41760'},
        'pin': 'public-fixture-pin', 'protocol_version': 1, 'managed': True,
        'enrolled_at': '2026-10-26T00:00:00.000Z'}))
    password = 'Fixture-Only-Pg-17_' + secrets.token_hex(24)
    docker_socket = os.environ.get('ORCH_TEST_DOCKER_SOCKET', '/var/run/docker.sock')
    docker_socket = docker_socket.removeprefix('unix://')
    docker_gid = os.stat(docker_socket).st_gid
    (root / '.env').write_text(f'POSTGRES_PASSWORD={password}\nORCH_HOME={root}\nORCH_UID={os.getuid()}\nORCH_GID={os.getgid()}\nORCH_DOCKER_GID={docker_gid}\n')
    os.chmod(root / '.env', 0o600)
    configuration = f'version=1\n[system]\nname={json.dumps(project)}\n[node]\ndata_dir={json.dumps(str(root / "runtime/node"))}\ndrivers=["docker"]\n[node.docker]\nsocket="/var/run/docker.sock"\nbuild_cache_gb=0\n'
    (root / 'config/orch.toml').write_text(configuration)
    checked([CTL, 'compose-secrets', '--config', root / 'config/orch.toml', '--secrets-dir', root / 'config/secrets'], env=env, log=log)
    (root / 'runtime/postgres/passwd').write_text(f'root:x:0:0:root:/root:/bin/sh\norch:x:{os.getuid()}:{os.getgid()}:Orch:/tmp:/bin/sh\n')
    compose = {'name': project, 'services': {
        'postgres': {'image': POSTGRES, 'user': f'{os.getuid()}:{os.getgid()}',
                     'environment': {'POSTGRES_USER': 'orch', 'POSTGRES_DB': 'orch', 'POSTGRES_PASSWORD': '${POSTGRES_PASSWORD}'},
                     'volumes': ['./runtime/postgres/data:/var/lib/postgresql/data', './runtime/postgres/passwd:/etc/passwd:ro']},
        'frontend': {'image': NGINX, 'command': ['sleep', '3600']},
    }}
    for service in ['api', 'gateway', 'worker', 'node']:
        compose['services'][service] = {'image': DEBIAN, 'command': ['sleep', '3600'], 'user': f'{os.getuid()}:{os.getgid()}'}
    compose['services']['node'].update({
        'group_add': [str(docker_gid)],
        'environment': {'ORCH_CONFIG': '/etc/orch/orch.toml', 'ORCH_NODE_DATA_DIR': str(root / 'runtime/node')},
        'volumes': ['./runtime/server/current:/opt/orch:ro', './config/orch.toml:/etc/orch/orch.toml:ro',
                    f'{root}/runtime/node:{root}/runtime/node',
                    f'{root}/runtime/node.orch-instance-lock:{root}/runtime/node.orch-instance-lock',
                    f'{docker_socket}:/var/run/docker.sock']})
    # JSON is YAML too. Scalar image lines let the updater identify its stock toolbox/runtime images.
    # The updater's image reader uses ordinary YAML image fields; use a minimal explicit rendering.
    lines = ['name: ' + project, 'services:']
    for service, fields in compose['services'].items():
        lines.append('  ' + service + ':')
        for key, value in fields.items():
            lines.append('    ' + key + ': ' + (value if key == 'image' else json.dumps(value)))
    # Explicit IPAM avoids Docker's exhausted default pools without changing another network.
    lines.extend(['networks:', '  default:', '    ipam:', '      config:',
                  '        - subnet: ' + unused_subnet(env, log)])
    (root / 'compose.yml').write_text('\n'.join(lines) + '\n')
    prefix = ['docker', 'compose', '-p', project, '-f', root / 'compose.yml']
    checked([*prefix, 'up', '-d'], env=env, log=log)
    until = time.monotonic() + READY_SECONDS
    while True:
        # The image's temporary Unix-only server precedes database creation. Require the final
        # TCP database that the separate backup client uses, inside the same readiness budget.
        remaining = max(0.1, until - time.monotonic())
        try:
            ready = subprocess.run([*map(str, prefix), 'exec', '-T', 'postgres', 'sh', '-c',
                'export PGPASSWORD="$POSTGRES_PASSWORD"; exec psql -h 127.0.0.1 -U orch -d orch -Atqc "SELECT 1"'],
                env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=min(5, remaining))
            ready_output = ready.stdout[-4096:]
        except subprocess.TimeoutExpired:
            ready = None
            ready_output = b'the bounded PostgreSQL readiness call timed out\n'
        if ready is not None and ready.returncode == 0:
            break
        if time.monotonic() >= until:
            log.write(ready_output.decode(errors='replace'))
            try:
                diagnostic = subprocess.run([*map(str, prefix), 'logs', '--no-color', '--tail', '80', 'postgres'],
                                            env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=15)
                log.write(diagnostic.stdout.decode(errors='replace'))
            except subprocess.TimeoutExpired:
                log.write('the bounded PostgreSQL diagnostic call timed out\n')
            log.flush()
            raise RuntimeError('the disposable PostgreSQL did not become ready')
        time.sleep(1)
    schema = b'CREATE TABLE _sqlx_migrations(version bigint PRIMARY KEY,description text NOT NULL,installed_on timestamptz NOT NULL DEFAULT now(),success boolean NOT NULL,checksum bytea NOT NULL,execution_time bigint NOT NULL);\n'
    for migration in sorted((REPO / 'migrations').glob('*.sql')):
        body = migration.read_bytes()
        version = int(migration.name.split('_', 1)[0])
        checksum = hashlib.sha384(body).hexdigest()
        schema += body + f"\nINSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time) VALUES({version},'fixture',true,decode('{checksum}','hex'),0);\n".encode()
    checked([*prefix, 'exec', '-T', 'postgres', 'psql', '-U', 'orch', '-d', 'orch', '-v', 'ON_ERROR_STOP=1'], env=env, log=log, data=schema)
    return prefix


def main():
    profile = ARTIFACTS / ('docker-profile-' + uuid.uuid4().hex)
    profile.mkdir()
    (profile / 'config.json').write_text('{"auths":{}}')
    env = {**os.environ, 'DOCKER_CONFIG': str(profile), 'LANG': 'C.UTF-8', 'LC_ALL': 'C.UTF-8', 'TMPDIR': str(ARTIFACTS)}
    socket = os.environ.get('ORCH_TEST_DOCKER_SOCKET', '/var/run/docker.sock')
    env['DOCKER_HOST'] = socket if '://' in socket else 'unix://' + socket
    sentinel = 'orchcustody-sentinel-' + uuid.uuid4().hex
    sentinel_volume = sentinel + '-data'
    with (ARTIFACTS / 'uninstall-stack.log').open('w') as log:
        checked(['docker', 'volume', 'create', sentinel_volume], env=env, log=log)
        try:
            checked(['docker', 'run', '-d', '--name', sentinel, '--label', 'orch.system=' + sentinel,
                     '-v', sentinel_volume + ':/data', BUSYBOX, 'sh', '-c', 'echo untouched > /data/keep; exec sleep 3600'], env=env, log=log)
            for mode in ['keep', 'purge', 'purge-without-backup']:
                project = 'orchcustody-' + uuid.uuid4().hex
                root = ARTIFACTS / project
                prefix = ['docker', 'compose', '-p', project, '-f', root / 'compose.yml']
                execution = project + '-execution'
                volume = project + '-data'
                try:
                    prefix = setup(root, project, env, log)
                    checked(['docker', 'volume', 'create', '--label', 'orch.system=' + project, '--label', 'orch.driver=docker', volume], env=env, log=log)
                    checked(['docker', 'run', '-d', '--name', execution, '--label', 'orch.system=' + project, '--label', 'orch.driver=docker', '-v', volume + ':/data', BUSYBOX, 'sleep', '3600'], env=env, log=log)
                    arguments = [root / 'update.sh', 'uninstall']
                    if mode != 'keep': arguments.append('--purge')
                    if mode == 'purge-without-backup': arguments.append('--no-backup')
                    checked([*arguments, '--plan'], env=env, log=log)
                    assert checked(['docker', 'inspect', '--format', '{{.State.Running}}', execution], env=env, log=log).strip() == b'true'
                    checked([*arguments, '--yes'], env=env, log=log)
                    assert (root / 'runtime').exists() == (mode == 'keep')
                    assert (root / 'config/secrets').exists() == (mode == 'keep')
                    assert (root / 'config/orch.toml').is_file() and (root / '.env').is_file()
                    own_units = checked(['docker', 'ps', '-aq', '--filter', 'label=orch.system=' + project], env=env, log=log)
                    own_volumes = checked(['docker', 'volume', 'ls', '-q', '--filter', 'label=orch.system=' + project], env=env, log=log)
                    assert bool(own_units.strip()) == (mode == 'keep')
                    assert bool(own_volumes.strip()) == (mode == 'keep')
                    if mode == 'keep':
                        assert checked(['docker', 'inspect', '--format', '{{.State.Running}}', execution], env=env, log=log).strip() == b'true'
                    if mode == 'purge':
                        pairs = list((root / 'backups').glob('uninstall-*'))
                        assert len(pairs) == 1
                        pair = pairs[0]
                        manifest = json.loads((pair / 'orch-final.manifest.json').read_text())
                        assert (pair / 'data/kept-fixture').read_bytes() == b'paired blob-store fixture'
                        assert (pair / 'config/secrets/orch_master_key').is_file()
                        digest = checked([CTL, 'checksum', pair / 'orch-final.dump'], env=env, log=log).decode().split()[0]
                        assert manifest['dump']['hash'] == digest
                        checked(['docker', 'run', '--rm', '--network', 'none', '-v', str(pair) + ':/saved:ro', POSTGRES, 'pg_restore', '--list', '/saved/orch-final.dump'], env=env, log=log)
                    if mode == 'purge-without-backup': assert not (root / 'backups').exists()
                    assert checked(['docker', 'exec', sentinel, 'cat', '/data/keep'], env=env, log=log) == b'untouched\n'
                    print('PASS stack uninstall ' + mode, flush=True)
                finally:
                    subprocess.run([*map(str, prefix), 'down', '--volumes', '--timeout', '10'], env=env, stdout=log, stderr=log)
                    subprocess.run(['docker', 'stop', '--time', '10', execution], env=env, stdout=log, stderr=log)
                    subprocess.run(['docker', 'rm', execution], env=env, stdout=log, stderr=log)
                    subprocess.run(['docker', 'volume', 'rm', volume], env=env, stdout=log, stderr=log)
        finally:
            subprocess.run(['docker', 'stop', '--time', '10', sentinel], env=env, stdout=log, stderr=log)
            subprocess.run(['docker', 'rm', sentinel], env=env, stdout=log, stderr=log)
            subprocess.run(['docker', 'volume', 'rm', sentinel_volume], env=env, stdout=log, stderr=log)


if __name__ == '__main__':
    main()
