"""Check native-history SQL page and index bounds on an isolated large PostgreSQL world."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time

# This is the release world's minimum size, not a production retention limit.
MINIMUM_ROWS = 1_000_000
WORLD_THREADS = 1_000
PAGE = 200
PORT = 41748
ROOT = Path(__file__).resolve().parents[4]


def run(command, *, body=None, seconds=60):
    result = subprocess.run(command, input=body, text=True, capture_output=True, timeout=seconds)
    if result.returncode:
        raise RuntimeError(f'{command[:3]} failed: {result.stderr[-4000:]}')
    return result.stdout


def literal(source, name):
    match = re.search(r'const ' + name + r': &str = "((?:[^"\\]|\\.)*)";', source)
    if not match:
        raise RuntimeError(f'the actual SQL supplier {name} is unavailable')
    return bytes(match[1], 'utf-8').decode('unicode_escape')


def inventory_sql(source):
    body = source.split('pub async fn page(', 1)[1].split('\n}\n', 1)[0]
    match = re.search(r'format!\(\s*"((?:[^"\\]|\\.)*)"', body)
    if not match:
        raise RuntimeError('the actual native-history page SQL is unavailable')
    query = bytes(match[1], 'utf-8').decode('unicode_escape')
    return query.replace('{SELECT}', literal(source, 'SELECT')).replace('{LATEST}', literal(source, 'LATEST'))


def statement_sql(source, function, operation):
    body = source.split('pub async fn ' + function + '(', 1)[1].split('\n}\n', 1)[0]
    match = re.search(operation + r'\(\s*"((?:[^"\\]|\\.)*)"', body)
    if not match:
        raise RuntimeError(f'the actual native-history SQL for {function} is unavailable')
    return bytes(match[1], 'utf-8').decode('unicode_escape')


def nodes(plan):
    yield plan
    for child in plan.get('Plans', []):
        yield from nodes(child)


def bootstrap_current_schema(sql, directory):
    paths = sorted(directory.glob('*.sql'))
    if not paths:
        raise RuntimeError('the current schema directory contains no SQL files')
    sql('CREATE EXTENSION IF NOT EXISTS pg_trgm;')
    manifest = []
    for path in paths:
        body = path.read_text()
        sql('BEGIN;\n' + body + '\nCOMMIT;')
        manifest.append({'file': path.name, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()})
    return manifest


def catalogue_indexes(sql, wanted):
    names = ','.join("'" + name.replace("'", "''") + "'" for name in wanted)
    rows = json.loads(sql("SELECT COALESCE(json_agg(json_build_object('name',indexname,'sql',indexdef)), '[]'::json) "
                          "FROM pg_indexes WHERE schemaname='public' AND indexname IN (" + names + ');'))
    found = {row['name']: row['sql'] for row in rows}
    if set(found) != set(wanted):
        raise RuntimeError('the current schema is missing supporting indexes: ' + ','.join(sorted(set(wanted)-set(found))))
    # The scale fixture shadows these same relations with temporary rows in its one connection.
    return [found[name].replace(' ON public.', ' ON ') + ';' for name in wanted]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--data-root', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--image', default='postgres:17-trixie')
    parser.add_argument('--schema-dir', type=Path, default=ROOT / 'migrations')
    parser.add_argument('--rows', type=int, default=MINIMUM_ROWS)
    args = parser.parse_args()
    if args.rows < MINIMUM_ROWS:
        parser.error('a release scale proof needs at least one million rows')
    args.data_root.mkdir(parents=True, exist_ok=False, mode=0o700)
    args.out.mkdir(parents=True, exist_ok=True)
    source = (ROOT / 'crates/control/db/src/queries/agent_history.rs').read_text()
    query = inventory_sql(source)
    homes = statement_sql(source, 'home_page', 'sqlx::query_as')
    observations = statement_sql(source, 'measured_batch', 'sqlx::query')
    wanted = ('segments_agent_history_native_idx', 'segments_agent_history_page_idx',
              'turns_agent_history_activity_idx', 'segments_agent_history_homes_idx',
              'inputs_agent_history_waiting_idx', 'approvals_agent_history_waiting_idx',
              'segments_agent_history_membership_idx')
    fixture = (Path(__file__).with_suffix('.sql')).read_text()
    fixture = fixture.replace(':threads', str(WORLD_THREADS)).replace(':rows', str(args.rows))
    name = f'orch-agent-history-scale-{os.getpid()}'
    started = False
    try:
        run(['docker', 'run', '-d', '--rm', '--name', name, '--network', 'none',
             '--user', f'{os.getuid()}:{os.getgid()}', '--memory', '1g', '--cpus', '2',
             '-e', 'POSTGRES_USER=orch', '-e', 'POSTGRES_PASSWORD=scale-proof',
             '-e', 'PGDATA=/var/lib/postgresql/data',
             '-v', f'{args.data_root.resolve()}:/var/lib/postgresql/data', args.image,
             'postgres', '-p', str(PORT), '-c', 'shared_buffers=64MB', '-c', 'max_connections=10'])
        started = True
        ready = False
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            result = subprocess.run(['docker', 'exec', name, 'pg_isready', '-p', str(PORT), '-U', 'orch'],
                                    capture_output=True, text=True, timeout=5)
            if result.returncode == 0:
                ready = True
                break
            time.sleep(0.2)
        if not ready:
            raise RuntimeError('the isolated PostgreSQL did not become ready within one minute')

        def sql(body):
            return run(['docker', 'exec', '-i', name, 'psql', '-X', '-q', '-A', '-t',
                        '-v', 'ON_ERROR_STOP=1', '-U', 'orch', '-p', str(PORT), '-d', 'orch'],
                       body=body, seconds=600)

        schema = bootstrap_current_schema(sql, args.schema_dir)
        indexes = catalogue_indexes(sql, wanted)
        (args.out / 'schema.json').write_text(json.dumps(schema, indent=2) + '\n')
        # One connection owns the temporary synthetic tables. EXPLAIN runs the SQL read from the real supplier.
        statements = [fixture, *indexes, 'ANALYZE segments; ANALYZE turns;',
                      'ANALYZE agents; ANALYZE threads; ANALYZE thread_inputs; ANALYZE approvals;',
                      'SET statement_timeout=120000;',
                      'PREPARE history_page(uuid,bigint) AS ' + query + ';',
                      'PREPARE history_homes(text[],uuid,uuid,uuid,bigint) AS ' + homes + ';',
                      'PREPARE home_counts(text[],uuid,uuid,uuid,bigint) AS WITH observed AS (' + homes + ") SELECT json_build_object('rows',count(*),'eligible',count(*) FILTER(WHERE eligible)) FROM observed;",
                      'PREPARE history_observations(jsonb,timestamptz) AS ' + observations + ';']
        cases = [('first', 'NULL'), ('middle', "'" + f'{args.rows // 2:032x}' + "'"),
                 ('last', "'" + f'{args.rows - PAGE:032x}' + "'")]
        for label, after in cases:
            statements += [f"SELECT '{label}';", f'EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) EXECUTE history_page({after},{PAGE});']
        statements += ["SELECT 'homes';", "EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) EXECUTE history_homes('{scale-program}',NULL,NULL,NULL,200);",
                       "SELECT jsonb_agg(jsonb_build_object('segment',id,'native',native_session_ref,'bytes',8192,'removed',false,'error',NULL,'revision',5)) AS observations FROM (SELECT id,native_session_ref FROM segments ORDER BY id LIMIT 200) batch \\gset",
                       "SELECT 'observations';", "EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) EXECUTE history_observations(:'observations','2026-10-29T00:00Z');"]
        statements += ["SELECT machine_id AS after_machine,provider_id AS after_provider,agent_id AS after_agent FROM (SELECT DISTINCT machine_id,provider_id,agent_id FROM segments ORDER BY machine_id,provider_id,agent_id LIMIT 200) page ORDER BY machine_id DESC,provider_id DESC,agent_id DESC LIMIT 1 \\gset",
                       "UPDATE agents SET harness_kind='unsupported-program' WHERE id IN (SELECT DISTINCT agent_id FROM segments WHERE (machine_id,provider_id,agent_id)<=(:'after_machine'::uuid,:'after_provider'::uuid,:'after_agent'::uuid));",
                       "SELECT 'blocked_homes';", "EXECUTE home_counts('{scale-program}',NULL,NULL,NULL,200);",
                       "SELECT 'later_homes';", "EXECUTE home_counts('{scale-program}',:'after_machine',:'after_provider',:'after_agent',200);"]
        statements.append("SELECT 'SCALE_SQL_DONE';")
        output = run(['docker', 'exec', '-i', name, 'psql', '-X', '-q', '-A', '-t',
                      '-v', 'ON_ERROR_STOP=1', '-U', 'orch', '-p', str(PORT), '-d', 'orch'],
                     body='\n'.join(statements), seconds=600)
        (args.out / 'explain.log').write_text(output)
        reports = []
        for index, (label, _) in enumerate(cases):
            text = output.split(label + '\n', 1)[1]
            next_label = cases[index + 1][0] if index + 1 < len(cases) else 'homes'
            plan = json.loads(text.split('\n' + next_label, 1)[0])[0]['Plan']
            all_nodes = list(nodes(plan))
            assert plan['Actual Rows'] == PAGE, (label, plan)
            assert not any(n['Node Type'] == 'Seq Scan' and n.get('Relation Name') in ('segments', 'turns')
                           for n in all_nodes), (label, plan)
            activity = [n for n in all_nodes if n.get('Index Name') == 'turns_agent_history_activity_idx']
            assert activity and all(n['Actual Rows'] <= 1 and n['Actual Loops'] <= PAGE for n in activity), (label, plan)
            reports.append({'case': label, 'rows_returned': plan['Actual Rows'],
                            'activity_rows_per_lookup': max(n['Actual Rows'] for n in activity),
                            'activity_lookups': max(n['Actual Loops'] for n in activity),
                            'indexes': sorted({n['Index Name'] for n in all_nodes if 'Index Name' in n})})
        for label, end in [('homes', 'observations'), ('observations', 'blocked_homes')]:
            plan = json.loads(output.split(label + '\n', 1)[1].split('\n' + end, 1)[0])[0]['Plan']
            all_nodes = list(nodes(plan))
            assert not any(n['Node Type'] == 'Seq Scan' and n.get('Relation Name') == 'segments'
                           for n in all_nodes), (label, plan)
            if label == 'homes':
                assert plan['Actual Rows'] == PAGE, (label, plan)
            else:
                selected = [n for n in all_nodes if n.get('Function Name') == 'jsonb_to_recordset']
                assert selected and selected[0]['Actual Rows'] == PAGE, (label, plan)
            reports.append({'case': label, 'rows_returned': plan['Actual Rows'],
                            'indexes': sorted({n['Index Name'] for n in all_nodes if 'Index Name' in n})})
        blocked = json.loads(output.split('blocked_homes\n', 1)[1].split('\nlater_homes', 1)[0])
        later = json.loads(output.split('later_homes\n', 1)[1].split('\nSCALE_SQL_DONE', 1)[0])
        assert blocked == {'rows': PAGE, 'eligible': 0}, blocked
        assert later == {'rows': PAGE, 'eligible': PAGE}, later
        reports.append({'case': 'blocked_pages_do_not_starve_later_homes', 'first': blocked, 'next': later})
        report = {'rows_per_table': args.rows, 'threads': WORLD_THREADS, 'page': PAGE,
                  'no_rust_compilation': True, 'cases': reports}
        (args.out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report), flush=True)
        print('AGENT HISTORY SCALE SQL PASSED', flush=True)
    finally:
        if started:
            diagnostics = subprocess.run(['docker', 'logs', name], capture_output=True,
                                         text=True, timeout=30)
            (args.out / 'postgres.log').write_text(diagnostics.stdout + diagnostics.stderr)
            run(['docker', 'stop', '--timeout', '5', name])


if __name__ == '__main__':
    main()
