"""Prove fresh-schema native close and claim work stays bounded as history grows."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import time

ROOT = Path(__file__).resolve().parents[4]
# Two corpus sizes ten times apart include the million-row release world.
SIZES = (100_000, 1_000_000)
PAGE = 200
# Keep foreign-key pending events and dirty transaction state bounded: a million rows need 100 SQL batches.
SEED_BATCH = 10_000
# This probe has its own networkless PostgreSQL in the reserved service range.
PORT = 41749


def interrupted(_signal, _frame):
    raise SystemExit('the native-history bounds proof was stopped')


def call(command, body=None, seconds=600):
    result = subprocess.run(command, input=body, text=True, capture_output=True, timeout=seconds)
    if result.returncode:
        raise RuntimeError(f'{command[:3]} failed: {result.stderr[-6000:]}')
    return result.stdout.strip()


def literal(source, name):
    match = re.search(r'const\s+' + re.escape(name) + r':\s*&str\s*=\s*"((?:[^"\\]|\\.)*)";', source)
    if not match:
        raise RuntimeError(f'the actual SQL supplier {name} is unavailable')
    return bytes(match[1], 'utf-8').decode('unicode_escape')


def main():
    signal.signal(signal.SIGTERM, interrupted)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--data-root', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--image', default='postgres:17-trixie')
    parser.add_argument('--schema-dir', type=Path, default=ROOT / 'migrations')
    parser.add_argument('--database', default='orch', help='a fresh database name inside the disposable cluster')
    parser.add_argument('--prepared', action='store_true',
                        help='use an already initialized empty disposable PostgreSQL 17 cluster')
    args = parser.parse_args()
    if not re.fullmatch('[a-z][a-z0-9_]{0,62}', args.database):
        parser.error('the disposable database needs a simple lowercase SQL name')
    if args.prepared:
        if not args.data_root.is_dir() or (args.data_root / 'PG_VERSION').read_text().strip() != '17':
            parser.error('prepared data must be an existing disposable PostgreSQL 17 cluster')
    else:
        args.data_root.mkdir(parents=True, exist_ok=False, mode=0o700)
    args.out.mkdir(parents=True, exist_ok=True)
    spec = importlib.util.spec_from_file_location('history_scale', Path(__file__).with_name('agent_history_scale.py'))
    helpers = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(helpers)
    source = (ROOT / 'crates/control/db/src/queries/agent_history.rs').read_text()
    select = literal(source, 'SELECT')
    first = select + literal(source, 'CLAIMS_FIRST')
    after = select + literal(source, 'CLAIMS_AFTER')
    fixture = Path(__file__).with_name('agent_history_upgrade_fixture.sql').read_text()
    name = f'orch-history-bounds-{os.getpid()}'
    (args.out / 'container.json').write_text(json.dumps({'name': name, 'data': str(args.data_root.resolve()),
                                                       'database': args.database, 'prepared': args.prepared}) + '\n')
    started = False

    def sql(body, seconds=600):
        return call(['docker', 'exec', '-i', name, 'psql', '-X', '-q', '-A', '-t', '-v', 'ON_ERROR_STOP=1',
                     '-U', 'orch', '-p', str(PORT), '-d', args.database], body, seconds)

    def seed_segments(start, end):
        statement = 'INSERT INTO segments(' + fixture.split('INSERT INTO segments(', 1)[1].split('INSERT INTO commands(', 1)[0]
        for first_row in range(start, end + 1, SEED_BATCH):
            last_row = min(end, first_row + SEED_BATCH - 1)
            sql(statement.replace('generate_series(1,:rows)', f'generate_series({first_row},{last_row})'))
            print(f'SEEDED_SEGMENTS through={last_row}', flush=True)

    def explain(query, types, values):
        text = sql('BEGIN; SET LOCAL statement_timeout=120000; SET LOCAL plan_cache_mode=force_generic_plan;\n'
                   f'PREPARE measured({types}) AS {query};\n'
                   f'EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) EXECUTE measured({values}); ROLLBACK;')
        plan = json.loads(text)[0]['Plan']
        nodes = list(helpers.nodes(plan))
        scans = [node for node in nodes if node.get('Relation Name') == 'segments' and 'Scan' in node['Node Type']]
        assert scans and not any(node['Node Type'] == 'Seq Scan' for node in scans), plan
        examined = sum((node['Actual Rows'] + node.get('Rows Removed by Filter', 0)
                        + node.get('Rows Removed by Index Recheck', 0)) * node['Actual Loops']
                       for node in scans)
        return {'rows': plan['Actual Rows'], 'examined': examined,
                'indexes': sorted({node['Index Name'] for node in nodes if 'Index Name' in node}), 'plan': plan}

    try:
        call(['docker', 'run', '-d', '--rm', '--name', name, '--network', 'none',
              '--user', f'{os.getuid()}:{os.getgid()}', '--memory', '1g', '--cpus', '2',
              '-e', 'POSTGRES_USER=orch', '-e', 'POSTGRES_PASSWORD=bounds-proof',
              '-e', f'POSTGRES_DB={args.database}',
              # Disposable bootstrap skips its initial whole-cluster sync; normal transaction durability
              # stays enabled and an explicit checkpoint precedes the bounded-work checks.
              '-e', 'POSTGRES_INITDB_ARGS=--no-sync',
              '-e', 'PGDATA=/var/lib/postgresql/data', '-v', f'{args.data_root.resolve()}:/var/lib/postgresql/data',
              args.image, 'postgres', '-p', str(PORT), '-c', 'shared_buffers=64MB',
              '-c', 'maintenance_work_mem=64MB', '-c', 'max_connections=10'])
        started = True
        deadline = time.monotonic() + 60
        while True:
            ready = subprocess.run(['docker', 'exec', name, 'pg_isready', '-p', str(PORT), '-U', 'orch'],
                                   capture_output=True, timeout=5)
            if ready.returncode == 0:
                break
            if time.monotonic() >= deadline:
                raise RuntimeError('the isolated PostgreSQL did not become ready within one minute')
            time.sleep(0.2)
        if args.prepared and args.database != 'orch':
            call(['docker', 'exec', name, 'createdb', '-U', 'orch', '-p', str(PORT), '--', args.database])
        assert sql('SHOW fsync; SHOW synchronous_commit;') == 'on\non'
        assert sql("SELECT count(*) FROM pg_tables WHERE schemaname='public';") == '0', 'the proof refuses a database containing existing application tables'
        schema = helpers.bootstrap_current_schema(sql, args.schema_dir)
        definition = sql("SELECT pg_get_functiondef('agent_history_closed()'::regprocedure);")
        match = re.search(r'UPDATE\s+(?:public\.)?segments\s+s\s+SET.*?;', definition, re.S | re.I)
        if not match:
            raise RuntimeError('the current close function no longer supplies its measured update')
        close = match.group(0)
        for variable, parameter in [('NEW.machine_id', '$1'), ('closed_thread', '$2'), ('NEW.body', '$3'),
                                    ('closed_segment', '$4'), ('source_segment', '$5')]:
            close = close.replace(variable, parameter)
        (args.out / 'schema.json').write_text(json.dumps({'files': schema,
            'close_function_sha256': hashlib.sha256(definition.encode()).hexdigest()}, indent=2) + '\n')
        sql(fixture.split('INSERT INTO segments(', 1)[0])
        seed_segments(1, SIZES[0])
        sql('INSERT INTO commands(' + fixture.split('INSERT INTO commands(', 1)[1])
        sql('CHECKPOINT;')
        reports = []
        for size in SIZES:
            if size > SIZES[0]:
                seed_segments(SIZES[0] + 1, size)
            sql('ANALYZE segments; ANALYZE turns;')
            body = "'" + json.dumps({'thread_id': hashlib.md5(b'history-proof-thread-0').hexdigest()}) + "'::jsonb"
            machine = "md5('history-proof-machine')::uuid"
            thread = "md5('history-proof-thread-0')::uuid"
            closed = explain(close, 'uuid,uuid,jsonb,uuid,uuid', f'{machine},{thread},{body},NULL,NULL')
            assert closed['examined'] <= 4, closed
            cases = {'close': closed}
            ceiling = f"'{size:032x}'::uuid"
            for label, cursor in [('first', None), ('middle', size // 2), ('last', size - PAGE * 10)]:
                query = first if cursor is None else after
                types = 'uuid,bigint' if cursor is None else 'uuid,uuid,bigint'
                values = f'{ceiling},{PAGE}' if cursor is None else f"'{cursor:032x}'::uuid,{ceiling},{PAGE}"
                observed = explain(query, types, values)
                assert observed['rows'] == PAGE and observed['examined'] <= PAGE * 8, observed
                assert 'segments_agent_history_claim_page_idx' in observed['indexes'], observed
                cases['claims_' + label] = observed
            top = explain('SELECT id FROM segments WHERE agent_history_claim IS NOT NULL ORDER BY id DESC LIMIT $1',
                          'bigint', '1')
            assert top['rows'] == 1 and top['examined'] <= 1, top
            cases['ceiling'] = top
            query = after.replace('$1', "'00000000-0000-0000-0000-0000000000c8'::uuid").replace('$2', ceiling).replace('$3', str(PAGE))
            progress = json.loads(sql("SELECT json_build_object('rows',count(*),'first',min(id::text)) FROM (" + query + ') later;'))
            assert progress == {'rows': PAGE, 'first': '00000000-0000-0000-0000-0000000000c9'}, progress
            unknown = int(sql("SELECT count(*) FROM segments WHERE agent_history_error='history.command_unconfirmed' AND agent_history_claim IS NOT NULL;"))
            assert unknown == PAGE, unknown
            cases['unknown_receipts_preserved'] = unknown
            added = f"'{size+1:032x}'::uuid"
            old_page = after.replace('$1', ceiling).replace('$2', ceiling).replace('$3', str(PAGE))
            next_page = after.replace('$1', ceiling).replace('$2', added).replace('$3', str(PAGE))
            admission = sql("BEGIN; INSERT INTO segments(id,thread_id,continuation_id,ordinal,agent_id,provider_id,environment_id,machine_id,env_generation,arrangement,credential_path,seq_updated,native_session_ref,agent_history_claim,agent_history_claimed_at) "
                            f"SELECT {added},thread_id,continuation_id,3,agent_id,provider_id,environment_id,machine_id,env_generation,arrangement,credential_path,seq_updated,'new-after-fence',md5('new-after-fence')::uuid,now() FROM segments WHERE id='00000000-0000-0000-0000-000000000001'::uuid; "
                            "SELECT json_build_object('old_pass',(SELECT count(*) FROM (" + old_page + ") old_pass),'next_pass',(SELECT count(*) FROM (" + next_page + ') next_pass)); ROLLBACK;')
            assert json.loads(admission) == {'old_pass': 0, 'next_pass': 1}, admission
            cases['arrivals_wait_for_next_pass'] = True
            reports.append({'segments': size, 'cases': cases})
            (args.out / 'plans.json').write_text(json.dumps(reports, indent=2) + '\n')
        # Real trigger behavior uses the fresh schema and retains all exact identity fences.
        sql("BEGIN; CREATE TEMP TABLE old_revisions AS SELECT id,agent_history_measure_revision FROM segments; UPDATE commands SET state='succeeded',result='{}' WHERE id=md5('history-proof-command-1')::uuid; DO $$ BEGIN IF EXISTS(SELECT 1 FROM segments s JOIN old_revisions o USING(id) WHERE s.agent_history_measure_revision<>o.agent_history_measure_revision+CASE WHEN s.id='00000000-0000-0000-0000-000000000001'::uuid THEN 1 ELSE 0 END) THEN RAISE EXCEPTION 'close changed another identity'; END IF; END $$; ROLLBACK;")
        print(json.dumps({'sizes': list(SIZES), 'fresh_schema': str(args.out / 'schema.json'),
                          'plans': str(args.out / 'plans.json'), 'unknown_receipts_do_not_starve_later_claims': True}), flush=True)
        print('AGENT HISTORY BOUNDS SQL PASSED', flush=True)
    finally:
        if started:
            logs = subprocess.run(['docker', 'logs', name], capture_output=True, text=True, timeout=30)
            (args.out / 'postgres.log').write_text(logs.stdout + logs.stderr)
            call(['docker', 'stop', '--timeout', '5', name], seconds=30)


if __name__ == '__main__':
    main()
