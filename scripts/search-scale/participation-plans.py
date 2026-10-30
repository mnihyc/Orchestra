#!/usr/bin/env python3
"""Check bounded participation reads and transactional rollups on an isolated native database.

The database must contain the current native schema, including its participation
tables and triggers. All fixture tables and data are rolled back. Use a disk
database for the million-row case.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--database-url', default=os.environ.get('ORCH_TEST_DATABASE_URL'))
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--sizes', default='1000,10000,100000,1000000')
parser.add_argument('--query-sql', type=Path, help='an earlier participation template for the deciding regression')
parser.add_argument('--explain-moves', action='store_true', help='include nested move plans using auto_explain on an owned database')
parser.add_argument('--wide', action='store_true', help='put all but the selected document in distinct unrelated homes')
args = parser.parse_args()
if not args.database_url:
    parser.error('--database-url or ORCH_TEST_DATABASE_URL is required')
sizes = [int(s) for s in args.sizes.split(',')]
assert sizes == sorted(set(sizes)) and min(sizes) > 0
out = args.output.resolve()
out.mkdir(parents=True, exist_ok=False)
tree = Path(__file__).resolve().parents[2]
user = "'00000000-0000-4000-8000-000000000001'::uuid"
root = "'00000000-0000-4000-8000-000000000002'::uuid"
branch = "'00000000-0000-4000-8000-000000000003'::uuid"
deep = "'00000000-0000-4000-8000-000000000004'::uuid"
destination = "'00000000-0000-4000-8000-000000000005'::uuid"
at = "timestamptz '2026-10-30 00:00:00Z'"
source = (args.query_sql or (tree / 'crates/control/db/src/queries/access/participation.sql')).read_text()
# These fixtures have no roles or follows. The unchanged native access suite exercises those sources.
source = source.replace('__ROLE_SUBJECT__', 'false').replace('__NODE_FOLLOW_LEVEL__', "'off'").replace('__PEOPLE_FOLLOW_LEVEL__', "'off'")
bindings = {
    1: user, 2: "'{}'::uuid[]", 3: f'ARRAY[{root}]', 4: at, 5: f'ARRAY[{root}]',
    6: "ARRAY['node','document','document','document','document','document']::text[]",
    7: f"ARRAY[{root},md5('inventory/document/1')::uuid,md5('missing/1')::uuid,md5('missing/2')::uuid,md5('missing/3')::uuid,md5('missing/4')::uuid]",
    8: f'ARRAY[{root}]', 9: "'{}'::uuid[]", 10: 'NULL::uuid', 11: f'ARRAY[{root}]',
}
def statement(overrides=None):
    values = bindings | (overrides or {})
    return re.sub(r'\$(\d+)\b', lambda m: values[int(m[1])], source)
tables = ['nodes', 'documents', 'permission_entries', 'permission_rows', 'node_roles',
          'providers', 'agents', 'environments', 'hosts', 'machines', 'env_templates',
          'mcp_servers', 'mounts', 'skills', 'subscriptions', 'chats', 'chat_states',
          'notification_prefs', 'chat_members', 'threads']
summary_tables = ['participation_object_facts', 'participation_paths', 'participation_entries',
                  'participation_direct_counts', 'participation_home_counts', 'participation_node_counts']
sql = ["BEGIN; SET statement_timeout='20min'; SET max_parallel_maintenance_workers=0;"]
sql.extend(f'CREATE TEMP TABLE {t}(LIKE public.{t} INCLUDING ALL);' for t in tables)
sql.extend(f'CREATE TEMP TABLE {t}(LIKE public.{t} INCLUDING ALL);' for t in summary_tables)
# Clone current trigger definitions; no historical migration file survives the initial schema fold.
sql.append("""DO $clone$ DECLARE held record; BEGIN
 FOR held IN SELECT c.relname,pg_get_triggerdef(t.oid) definition
  FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid
  JOIN pg_namespace n ON n.oid=c.relnamespace
  WHERE n.nspname='public' AND NOT t.tgisinternal AND t.tgname LIKE 'participation_%'
 LOOP EXECUTE replace(held.definition,format(' ON public.%I ',held.relname),
  format(' ON pg_temp.%I ',held.relname)); END LOOP;
END $clone$;""")
sql.append(f"""\\o '{out / 'schema-functions.json'}'
SELECT json_agg(pg_get_functiondef(p.oid) ORDER BY p.proname)
 FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace
 WHERE n.nspname='public' AND p.proname LIKE 'orch_participation_%';
\\o""")
sql.append(f"""INSERT INTO nodes(id,kind,org_id,path,name,owner_id,created_by,parent_id) VALUES
 ({root},'org',{root},ARRAY[{root}],'Root',{user},{user},NULL),
 ({branch},'topic',{root},ARRAY[{root},{branch}],'Branch',{user},{user},{root}),
 ({destination},'topic',{root},ARRAY[{root},{destination}],'Destination',{user},{user},{root}),
 ({deep},'topic',{root},ARRAY[{root},{branch},{deep}],'Deep',{user},{user},{branch});""")
if args.explain_moves:
    sql.extend(["LOAD 'auto_explain';","SET client_min_messages=notice;","SET auto_explain.log_level=notice;","SET auto_explain.log_format=json;","SET auto_explain.log_analyze=on;","SET auto_explain.log_buffers=on;","SET auto_explain.log_nested_statements=on;"])
checks = []
def plan(label, query, expected):
    if args.explain_moves and label.startswith('move-'):
        sql.append('SET auto_explain.log_min_duration=0;')
    sql.append(f"\\o '{out / (label+'.plan.json')}'\nEXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) {query};\n\\o\n\\echo DONE {label}")
    if args.explain_moves and label.startswith('move-'):
        sql.append('SET auto_explain.log_min_duration=-1;')
    checks.append((label, expected))
previous = 0
for size in sizes:
    if args.wide:
        sql.append(f"""INSERT INTO nodes(id,kind,org_id,path,name,owner_id,created_by,parent_id)
 SELECT md5('inventory/home/'||i)::uuid,'topic',{root},
 ARRAY[{root},{destination},md5('inventory/home/'||i)::uuid],
 'Unrelated home '||i,{user},{user},{destination}
 FROM generate_series({max(previous+1,2)},{size})i;""")
    home = f"CASE WHEN i=1 THEN {deep} ELSE md5('inventory/home/'||i)::uuid END" if args.wide else deep
    sql.append(f"""INSERT INTO documents(id,node_id,org_id,owner_id,created_by,title,basename)
 SELECT md5('inventory/document/'||i)::uuid,{home},{root},{user},{user},
 'Inventory document '||i,'inventory-'||i||'.md' FROM generate_series({previous+1},{size})i;
INSERT INTO permission_entries(id,object_kind,object_id,page,permission,holder_kind,holder_id,source)
 SELECT md5('inventory/owner/'||i)::uuid,'document',md5('inventory/document/'||i)::uuid,
 'live','access.own','user',{user},'creation' FROM generate_series({previous+1},{size})i;""")
    sql.extend(f'ANALYZE {t};' for t in tables + summary_tables)
    plan(f'markers-{size}', statement(), 2)
    plan(f'ancestors-{size}', statement({5:f'ARRAY[{root},{branch}]'}), 3)
    plan(f'negative-{size}', statement({1:"md5('absent/person')::uuid"}), 0)
    plan(f'closed-{size}', statement({9:f'ARRAY[{branch}]'}), int(args.wide and size>1))
    plan(f'token-{size}', statement({10:destination}), int(args.wide and size>1))
    sql.append(f"\\o '{out / f'storage-{size}.json'}'\nSELECT json_build_object('entries',(SELECT count(*) FROM participation_entries),'direct_keys',(SELECT count(*) FROM participation_direct_counts),'home_keys',(SELECT count(*) FROM participation_home_counts),'ancestor_keys',(SELECT count(*) FROM participation_node_counts),'bytes',(SELECT sum(pg_total_relation_size(x::regclass)) FROM unnest(ARRAY[{','.join(repr(t) for t in summary_tables)}])x));\n\\o")
    # Updating a home path rolls up the home's aggregate; no descendant object or grant needs a rewrite.
    plan(f'move-{size}', f"UPDATE nodes SET parent_id={destination},path=ARRAY[{root},{destination},{deep}] WHERE id={deep}", 1)
    plan(f'moved-{size}', statement({5:f'ARRAY[{root},{branch},{destination}]'}), 3)
    sql.append(f"""DO $test$ BEGIN
 IF (SELECT total FROM participation_node_counts WHERE node_id={branch}) IS NOT NULL
 OR (SELECT sum(total) FROM participation_node_counts WHERE node_id={destination})<>{size}
 OR (SELECT count(*) FROM participation_entries)<>{size}
 THEN RAISE EXCEPTION 'move rollup mismatch'; END IF; END $test$;
 UPDATE nodes SET parent_id={branch},path=ARRAY[{root},{branch},{deep}] WHERE id={deep};""")
    previous = size
# The only assigned object is deep below a requested marker, not among the requested objects.
sql.append(f"INSERT INTO permission_entries(id,object_kind,object_id,page,permission,holder_kind,holder_id,source) VALUES(md5('deep/witness')::uuid,'document',md5('inventory/document/1')::uuid,'live','access.view','user',md5('deep/person')::uuid,'creation');")
plan('deep-only', statement({1:"md5('deep/person')::uuid",5:f'ARRAY[{branch}]',6:"ARRAY['node']::text[]",7:f'ARRAY[{branch}]'}), 1)
sql.append("ROLLBACK;\n\\echo DONE participation proof")
(out / 'run.sql').write_text('\n'.join(sql))
(out / 'inputs.json').write_text(json.dumps({
    'head': subprocess.check_output(['git','-C',str(tree),'rev-parse','HEAD'],text=True).strip(),
    'query_sha256': hashlib.sha256(source.encode()).hexdigest(),
    'sizes':sizes,'wide':args.wide,
},indent=2)+'\n')
with (out / 'psql.log').open('x') as log:
    result = subprocess.run(['psql', args.database_url, '-X', '-Atq', '-v', 'ON_ERROR_STOP=1', '-f', str(out/'run.sql')], stdout=log, stderr=subprocess.STDOUT, timeout=21600)
if result.returncode:
    raise SystemExit(result.returncode)
def walk(node):
    yield node
    for child in node.get('Plans', []):
        yield from walk(child)
report = []
nested_moves = {}
if args.explain_moves:
    log = (out/'psql.log').read_text()
    decoder = json.JSONDecoder()
    start = 0
    for size in sizes:
        end = log.index(f'DONE move-{size}', start)
        section = log[start:end]
        scans = {}
        for match in re.finditer(r'\n\{\n', section):
            try:
                nested, _ = decoder.raw_decode(section[match.start()+1:])
            except ValueError:
                continue
            for n in walk(nested.get('Plan',{})):
                if 'Relation Name' in n:
                    name = n['Relation Name']
                    scans[name] = scans.get(name,0)+(n.get('Actual Rows',0)+n.get('Rows Removed by Filter',0)+n.get('Rows Removed by Index Recheck',0))*n.get('Actual Loops',0)
        assert scans.get('permission_entries',0)<=8, (size, scans)
        assert scans.get('participation_entries',0)<=8, (size, scans)
        assert scans.get('documents',0)==0, (size, scans)
        nested_moves[size]=scans
        start=end
for label, expected in checks:
    p = json.loads((out / (label+'.plan.json')).read_text())[0]
    rows = {}
    for n in walk(p['Plan']):
        if 'Relation Name' in n:
            name=n['Relation Name']
            rows[name]=rows.get(name,0)+(n.get('Actual Rows',0)+n.get('Rows Removed by Filter',0)+n.get('Rows Removed by Index Recheck',0))*n.get('Actual Loops',0)
    if label.startswith('move-'):
        assert p['Plan']['Actual Rows']==0, (label,p['Plan'])
    else:
        assert p['Plan']['Actual Rows']==expected,(label,p['Plan']['Actual Rows'],expected)
        assert rows.get('permission_entries',0)==0 and rows.get('documents',0)==0,(label,rows)
        assert sum(rows.values())<=128,(label,rows)
    report.append({'case':label,'execution_ms':p['Execution Time'],'rows_examined':rows,'triggers':p.get('Triggers',[]), 'nested_move_rows':nested_moves.get(int(label.split('-')[1])) if label.startswith('move-') else None})
for prefix in ['markers','ancestors','negative','closed','token']:
    matrix=[r['rows_examined'] for r in report if r['case'].startswith(prefix+'-')]
    assert all(m==matrix[0] for m in matrix),(prefix,matrix)
(out/'report.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps(report),flush=True)
print('DONE participation proof',flush=True)
