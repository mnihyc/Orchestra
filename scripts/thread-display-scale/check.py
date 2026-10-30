"""Explain the actual display-facts SQL against isolated million-row temporary relations."""
from pathlib import Path
import argparse
import hashlib
import json
import os
import re
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument('--tree', required=True, type=Path)
parser.add_argument('--output', required=True, type=Path)
parser.add_argument('--rows', type=int, default=1_000_000)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
source = (args.tree / 'crates/control/db/src/queries/source.rs').read_text()
function = source.split('pub async fn thread_display_facts(', 1)[1].split('/// Contained threads', 1)[0]
query = re.search(r'"(WITH pending AS MATERIALIZED .*?WHERE th.node_id=ANY\(\$1\))"', function, re.S)[1]
jobs = (args.tree / 'crates/control/db/src/queries/jobs.rs').read_text().split('fn thread_hold_predicate(', 1)[1]
hold = re.search(r'"(\(j.state IN .*?)"\s*\.replace', jobs, re.S)[1].replace('__THREAD__', 'th.node_id')
query = query.replace('__RECOVERY_HOLD__', hold)
pages = source.split('pub async fn topic_threads_page(', 1)[1].split('/// SQL totals', 1)[0]
page_queries = re.findall(r'"(SELECT th.topic_id.*?LIMIT \$[24])"', pages, re.S)
assert len(page_queries) == 2
attention = re.search(r'pub const ATTENTION_SQL: &str = "(.*?)";', source, re.S)[1]
counts = source.split('pub async fn topic_attention_counts(', 1)[1]
count_query = re.search(r'"(WITH flags AS MATERIALIZED .*?FROM flags GROUP BY topic_id)"', counts, re.S)[1].replace('{ATTENTION_SQL}', attention)
common = (args.tree / 'crates/contracts/api/src/common.rs').read_text()
page_size = int(re.search(r'const MAX_LIMIT: u32 = ([\d_]+)', common)[1].replace('_', ''))
rows = args.rows
thread = "md5('thread'||1)::uuid"
reader = "md5('reader')::uuid"
one = f'ARRAY[{thread}]'
many = 'ARRAY[' + ','.join("md5('thread'||"+str(index)+")::uuid" for index in range(1,1001)) + ']'
topic = "md5('topic')::uuid"
ordered = sorted(hashlib.md5(('thread'+str(index)).encode(), usedforsecurity=False).hexdigest() for index in range(1,1001))
cursor = ordered[min(page_size, 1000)-1]

def explain(label, identifiers):
    path = str(args.output / (label + '.json')).replace("'", "''")
    return f"\\o '{path}'\nEXPLAIN(ANALYZE,BUFFERS,FORMAT JSON) EXECUTE facts({identifiers},{reader},'2026-10-28T00:00:00Z');\n\\o\n"

def explain_execute(label, call):
    path = str(args.output / (label + '.json')).replace("'", "''")
    return f"\\o '{path}'\nEXPLAIN(ANALYZE,BUFFERS,FORMAT JSON) EXECUTE {call};\n\\o\n"

def capture(label):
    path = str(args.output / (label + '.json')).replace("'", "''")
    return f"\\o '{path}'\nEXECUTE fact_json({one},{reader},'2026-10-28T00:00:00Z');\n\\o\n"

sql = f"""\\set ON_ERROR_STOP on
\\pset tuples_only on
\\pset format unaligned
BEGIN;
SET LOCAL statement_timeout='60s';
SET LOCAL lock_timeout='5s';
SET LOCAL work_mem='16MB';
SET LOCAL maintenance_work_mem='64MB';
CREATE TEMP TABLE nodes(id uuid PRIMARY KEY,path uuid[],archived_at timestamptz,deleted_at timestamptz);
CREATE TEMP TABLE threads(node_id uuid PRIMARY KEY,topic_id uuid,side_of uuid,active_continuation_id uuid,environment_id uuid,provider_id uuid,next_seq bigint,state text,blocked_reason text,blocked_detail text,queue_held boolean,work_observed jsonb);
CREATE INDEX threads_topic_idx ON threads(topic_id);
CREATE TEMP TABLE environments(id uuid PRIMARY KEY,status text,deleted_at timestamptz,machine_id uuid);
CREATE TEMP TABLE machines(id uuid PRIMARY KEY,online boolean,status text,deleted_at timestamptz);
CREATE TEMP TABLE turns(id uuid PRIMARY KEY,thread_id uuid,continuation_id uuid,ordinal integer,state text,completed_seq bigint,seq_updated bigint,completed_at timestamptz,UNIQUE(thread_id,ordinal));
CREATE INDEX turns_continuation_ordinal_idx ON turns(thread_id,continuation_id,ordinal);
CREATE INDEX turns_continuation_updated_idx ON turns(thread_id,continuation_id,seq_updated);
CREATE TEMP TABLE thread_inputs(id uuid PRIMARY KEY,thread_id uuid,continuation_id uuid,author_id uuid,client_id text,position numeric,state text,state_detail text,queue_held boolean,seq_created bigint,seq_updated bigint,created_at timestamptz,UNIQUE(thread_id,author_id,client_id));
CREATE INDEX thread_inputs_pending_idx ON thread_inputs(thread_id,position) WHERE state='pending';
CREATE INDEX inputs_continuation_pending_idx ON thread_inputs(thread_id,continuation_id,position) WHERE state='pending';
CREATE INDEX inputs_continuation_updated_idx ON thread_inputs(thread_id,continuation_id,seq_updated);
CREATE TEMP TABLE approvals(id uuid PRIMARY KEY,kind text,node_id uuid,thread_id uuid,turn_id uuid,state text,expires_at timestamptz,request jsonb);
CREATE INDEX approvals_pending_idx ON approvals(kind,node_id) WHERE state='pending';
CREATE TEMP TABLE input_schedules(id uuid PRIMARY KEY,state text,deleted_at timestamptz,target jsonb,next_run_at timestamptz);
CREATE INDEX input_schedules_due_idx ON input_schedules(next_run_at) WHERE state='active';
CREATE TEMP TABLE jobs(id uuid PRIMARY KEY,kind text,node_id uuid,environment_id uuid,provider_id uuid,batch_id uuid,state text,plan jsonb,created_at timestamptz);
CREATE INDEX jobs_batch_idx ON jobs(batch_id) WHERE batch_id IS NOT NULL;
CREATE INDEX jobs_active_environment_idx ON jobs(environment_id) WHERE environment_id IS NOT NULL AND state IN('queued','running');
CREATE INDEX jobs_environment_host_idx ON jobs(environment_id,created_at DESC) WHERE plan?'host';
CREATE INDEX jobs_provider_facts_idx ON jobs(provider_id) WHERE state IN('queued','running');
CREATE TEMP TABLE thread_reads(user_id uuid,thread_id uuid,continuation_id uuid,last_read_seq bigint,PRIMARY KEY(user_id,thread_id,continuation_id));
CREATE TEMP TABLE model_call_recovery(node_id uuid,purpose text,call_key uuid,state text,action jsonb,due_at timestamptz,rule jsonb,context jsonb,failure jsonb,updated_at timestamptz,PRIMARY KEY(node_id,purpose,call_key));
CREATE INDEX model_call_recovery_workflow_idx ON model_call_recovery(node_id,purpose,(context->>'workflow_id'),updated_at DESC) WHERE state<>'dismissed';
CREATE TEMP TABLE items(id bigint PRIMARY KEY,thread_id uuid,tokens bigint,payload text);
INSERT INTO nodes SELECT md5('thread'||n)::uuid,ARRAY[md5('thread'||n)::uuid],NULL,NULL FROM generate_series(1,1000)n;
INSERT INTO threads SELECT md5('thread'||n)::uuid,md5('topic')::uuid,NULL,md5('continuation'||n)::uuid,md5('environment'||n)::uuid,md5('provider')::uuid,{3*rows+1},'idle',NULL,NULL,false,NULL FROM generate_series(1,1000)n;
INSERT INTO nodes SELECT md5('unrelated'||n)::uuid,ARRAY[md5('unrelated'||n)::uuid],NULL,NULL FROM generate_series(1,{rows})n;
INSERT INTO threads SELECT md5('unrelated'||n)::uuid,md5('unrelated-topic')::uuid,NULL,md5('unrelated-continuation'||n)::uuid,NULL,NULL,1,'idle',NULL,NULL,false,NULL FROM generate_series(1,{rows})n;
INSERT INTO turns SELECT md5('turn'||n)::uuid,{thread},md5('continuation'||1)::uuid,n,CASE WHEN n=1 THEN 'completed' ELSE 'interrupted' END,2*n,2*n,'2026-01-01T00:00:00Z'::timestamptz+interval '1second'*n FROM generate_series(1,{rows})n;
INSERT INTO thread_inputs SELECT md5('input'||n)::uuid,{thread},md5('continuation'||1)::uuid,NULL,NULL,n,'delivered',NULL,false,CASE WHEN n%2=0 THEN n ELSE 0 END,n,'2026-01-01T00:00:00Z'::timestamptz+interval '1second'*n FROM generate_series(1,{rows})n;
INSERT INTO approvals SELECT md5('approval'||n)::uuid,'harness',md5('thread'||2)::uuid,md5('thread'||2)::uuid,NULL,'pending',NULL,'{{}}' FROM generate_series(1,{rows})n;
INSERT INTO input_schedules SELECT md5('schedule'||n)::uuid,'active',NULL,jsonb_build_object('kind','thread','thread_id',md5('thread'||2)::uuid),'2026-11-01T00:00:00Z' FROM generate_series(1,{rows})n;
INSERT INTO jobs SELECT md5('job'||n)::uuid,'export',md5('thread'||2)::uuid,md5('environment'||2)::uuid,md5('provider')::uuid,NULL,'succeeded','{{}}','2026-01-01T00:00:00Z' FROM generate_series(1,{rows})n;
INSERT INTO model_call_recovery SELECT md5('thread'||((n%1000)+1))::uuid,'summary',md5('recovery'||n)::uuid,'completed','{{}}',NULL,'{{}}','{{}}','{{}}','2026-01-01T00:00:00Z' FROM generate_series(1,{rows})n;
INSERT INTO model_call_recovery VALUES({thread},'agent',{thread},'scheduled','{{"attempt":2,"step":{{"type":"retry","times":3}}}}','2026-10-28T00:00:55Z','{{"id":"busy"}}','{{}}','{{"code":"provider_busy"}}','2026-10-28T00:00:00Z');
INSERT INTO thread_reads SELECT md5('other-reader'||((n-1)/1000))::uuid,md5('thread'||(((n-1)%1000)+1))::uuid,md5('continuation'||(((n-1)%1000)+1))::uuid,n FROM generate_series(1,{rows})n;
INSERT INTO items SELECT n,{thread},10000,'synthetic token weight; no history body is read by the icon query' FROM generate_series(1,{rows})n;
ANALYZE nodes; ANALYZE threads; ANALYZE turns; ANALYZE thread_inputs; ANALYZE approvals; ANALYZE input_schedules; ANALYZE jobs; ANALYZE thread_reads; ANALYZE model_call_recovery; ANALYZE items;
PREPARE facts(uuid[],uuid,timestamptz) AS {query};
PREPARE fact_json(uuid[],uuid,timestamptz) AS SELECT row_to_json(f) FROM ({query})f;
PREPARE first_topic(uuid[],bigint) AS {page_queries[1]};
PREPARE next_topic(uuid[],uuid,uuid,bigint) AS {page_queries[0]};
PREPARE attention_counts(uuid[]) AS {count_query};
"""
sql += explain('baseline-idle', one)
sql += "UPDATE threads SET state='blocked',blocked_reason='error',blocked_detail='call_recovery' WHERE node_id="+thread+";\n"
sql += explain('baseline-recovery', one)
sql += (args.tree / 'migrations/0042_thread_display_indexes.sql').read_text() + '\n'
sql += 'ANALYZE nodes; ANALYZE threads; ANALYZE turns; ANALYZE thread_inputs; ANALYZE approvals; ANALYZE input_schedules; ANALYZE jobs; ANALYZE thread_reads; ANALYZE model_call_recovery; ANALYZE items;\n'
sql += explain('indexed-recovery', one)
sql += explain('indexed-thousand', many)
sql += "UPDATE threads SET state='idle',blocked_reason=NULL,blocked_detail=NULL WHERE node_id="+thread+";\n"
sql += "UPDATE turns SET state='failed' WHERE ordinal="+str(rows)+";\n"
sql += explain('indexed-failed', one)
sql += capture('failed-no-later-input')
sql += f"INSERT INTO thread_inputs VALUES(md5('later-input')::uuid,{thread},md5('continuation'||1)::uuid,NULL,NULL,{rows+1},'cancelled',NULL,false,{rows*4},{rows*4},'2026-10-27T00:00:00Z');\n"
sql += capture('failed-later-accepted')
sql += "UPDATE thread_inputs SET seq_created=0 WHERE id=md5('later-input')::uuid;\n"
sql += capture('failed-later-imported')
sql += "UPDATE thread_inputs SET created_at='2025-01-01T00:00:00Z' WHERE id=md5('later-input')::uuid;\n"
sql += capture('failed-older-imported')
sql += explain_execute('indexed-topic-first', f'first_topic(ARRAY[{topic}],{page_size})')
sql += explain_execute('indexed-topic-next', f"next_topic(ARRAY[{topic}],{topic},'{cursor}'::uuid,{page_size})")
sql += explain_execute('indexed-topic-counts', f'attention_counts({one})')
metrics_path = str(args.output / 'corpus.json').replace("'", "''")
sql += f"\\o '{metrics_path}'\nSELECT json_build_object('items',count(*),'logical_tokens',sum(tokens),'read_positions',(SELECT count(*) FROM thread_reads),'thread_metadata',(SELECT count(*) FROM threads)) FROM items;\n\\o\nROLLBACK;\n"
script = args.output / 'probe.sql'
script.write_text(sql)
environment = dict(os.environ, PGAPPNAME='orch-thread-display-scale')
result = subprocess.run(['psql', os.environ['ORCH_TEST_DATABASE_URL'], '-X', '-v', 'ON_ERROR_STOP=1', '-f', str(script)], env=environment, timeout=900)
if result.returncode:
    raise SystemExit(result.returncode)

def walk(plan):
    yield plan
    for child in plan.get('Plans', []):
        yield from walk(child)

summaries = {}
for label in ['baseline-idle', 'baseline-recovery', 'indexed-recovery', 'indexed-thousand', 'indexed-failed', 'indexed-topic-first', 'indexed-topic-next', 'indexed-topic-counts']:
    path = args.output / (label + '.json')
    data = json.loads(path.read_text())[0]
    plan = data['Plan']
    summaries[path.stem] = {'rows_returned': plan['Actual Rows'], 'execution_ms': data['Execution Time'],
        'scans': [{key: node[key] for key in ['Node Type','Relation Name','Index Name','Actual Rows','Actual Loops','Rows Removed by Filter'] if key in node}
                  for node in walk(plan) if 'Relation Name' in node or 'Index Name' in node]}
    assert all(node.get('Relation Name') != 'items' for node in walk(plan)), 'icons never scan history bodies'
assert summaries['indexed-thousand']['rows_returned'] == 1000
assert summaries['indexed-recovery']['rows_returned'] == 1
assert summaries['indexed-topic-first']['rows_returned'] == min(page_size, 1000)
assert summaries['indexed-topic-next']['rows_returned'] == min(page_size, max(0, 1000-page_size))
assert summaries['indexed-topic-counts']['rows_returned'] == 1
corpus = json.loads((args.output/'corpus.json').read_text())
assert corpus == {'items':rows,'logical_tokens':rows*10_000,'read_positions':rows,'thread_metadata':rows+1000}
for label, failed in [('failed-no-later-input',True),('failed-later-accepted',False),('failed-later-imported',False),('failed-older-imported',True)]:
    assert json.loads((args.output/(label+'.json')).read_text())['failed'] is failed, label
for label in ['indexed-recovery', 'indexed-failed']:
    large = {'turns','thread_inputs','approvals','input_schedules','jobs','model_call_recovery','thread_reads','nodes','threads'}
    for scan in summaries[label]['scans']:
        if scan.get('Relation Name') in large and scan.get('Actual Loops',0)>0:
            assert scan['Node Type'] != 'Seq Scan', (label, scan)
            assert scan.get('Rows Removed by Filter',0)<1000, (label, scan)
(args.output / 'summary.json').write_text(json.dumps(summaries, indent=2)+'\n')
print(json.dumps(summaries, indent=2))
print('SCALE_PROBE_DONE')
