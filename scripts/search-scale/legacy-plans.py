#!/usr/bin/env python3
"""Check legacy ordered match and selected-access pages without materializing the document inventory.

Run on an isolated disk database with the current native schema. Temporary tables
copy native indexes; all data is generated in SQL and rolled back. The rendered
statement uses the production fragments for an unfiltered chronological document
search, the same match/cursor contract, and its maximum raw identity page.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--database-url', default=os.environ.get('ORCH_TEST_DATABASE_URL'))
p.add_argument('--output',type=Path,required=True)
p.add_argument('--sizes',default='1000,10000,100000,1000000')
args=p.parse_args()
if not args.database_url:p.error('--database-url or ORCH_TEST_DATABASE_URL is required')
sizes=[int(x) for x in args.sizes.split(',')]
assert sizes==sorted(set(sizes)) and min(sizes)>0
out=args.output.resolve();out.mkdir(parents=True,exist_ok=False)
tree=Path(__file__).resolve().parents[2]
queries=tree/'crates/control/db/src/queries'
query=(queries/'search_content.sql').read_text()
match=(queries/'search_match.sql').read_text()
replacements={
 '__NODE_VISIBILITY__':(queries/'search_candidate_visibility.sql').read_text(),
 '__CONTENT_COLUMNS__':(queries/'search_candidate_columns.sql').read_text(),
 '__CONTENT_CANDIDATES__':'true','__TITLE__':'3','__DESCRIPTION__':'2','__HEADING__':'1',
 '__ADVANCED_SOURCE__':'true','__ADVANCED_SHARED__':'true','__DOCUMENT_FILE_TYPE__':'true',
 '__HEADER_FILE_TYPE__':'true','__ITEM_FILE_TYPE__':'true','__REVISION_FILE_TYPE__':'true','__CHAT_FILE_TYPE__':'true',
 '__OCCURRENCES__':"(SELECT COALESCE(sum((char_length(lower(m.text))-char_length(replace(lower(m.text),lower(term),'')))/char_length(term)),0)::bigint FROM unnest($28::text[]) words(term) WHERE char_length(term)>0)",
}
for token,field in [('__HEADERS_MATCH__','text'),('__ITEM_MATCH__','i.search_text'),('__OLD_ITEM_MATCH__',"r.body->>'search_text'"),('__CHAT_MATCH__','m.search_text'),('__BLOCK_MATCH__','b.text'),('__NODE_MATCH__','x.text')]:
 replacements[token]=match.replace('{text}',field).replace('{patterns}','$1')
for token,value in replacements.items():query=query.replace(token,value)
assert not re.search(r'__[A-Z_]+__',query)
user="'00000000-0000-4000-8000-000000000001'::uuid"
root="'00000000-0000-4000-8000-000000000002'::uuid"
bindings={1:"ARRAY['%boundedneedle%']::text[]",2:user,3:"'{}'::uuid[]",4:f'ARRAY[{root}]',5:"'{}'::jsonb",6:'NULL::uuid',7:'NULL::uuid',8:'NULL::uuid',9:"'{}'::text[]",10:'false',11:'true',12:'NULL::uuid',13:'NULL::timestamptz',14:'NULL::timestamptz',15:'NULL::timestamptz',16:'NULL::uuid',17:'NULL::text',18:'NULL::boolean',19:'201',20:"'boundedneedle'::text",21:'false',22:'NULL::uuid',23:'false',24:"timestamptz '2026-10-30'",25:'false',26:'0',27:'false',28:"ARRAY['boundedneedle']::text[]",29:'0',30:'NULL::uuid'}
bindings.update({i:'NULL::uuid[]' for i in range(31,37)})
query=re.sub(r'\$(\d+)\b',lambda m:bindings[int(m[1])],query)
seed=(queries/'search_access_seeds.sql').read_text()
seed_values={1:'NULL::uuid',2:"ARRAY[md5('legacy/document/1')::uuid]",3:"'{}'::uuid[]",4:"'{}'::uuid[]",5:'NULL::bigint'}
seed=re.sub(r'\$(\d+)\b',lambda m:seed_values[int(m[1])],seed)
tables=['nodes','documents','permission_entries','permission_rows','node_roles','providers','agents','environments','hosts','machines','env_templates','mcp_servers','mounts','skills','subscriptions','chats','chat_states','notification_prefs','chat_members','threads','document_files','document_blocks','thread_continuations','thread_continuation_spans','items','thread_item_revisions','chat_messages','turns']
sql=["BEGIN;SET statement_timeout='15min';SET max_parallel_maintenance_workers=0;"]
sql.extend(f'CREATE TEMP TABLE {t}(LIKE public.{t} INCLUDING ALL);' for t in tables)
sql.append(f"INSERT INTO nodes(id,kind,org_id,path,name,owner_id,created_by) VALUES({root},'org',{root},ARRAY[{root}],'Root',{user},{user});")
previous=0
for size in sizes:
 sql.append(f"""INSERT INTO documents(id,node_id,org_id,owner_id,created_by,title,basename)
 SELECT md5('legacy/document/'||i)::uuid,{root},{root},{user},{user},
 CASE WHEN i=1 THEN 'boundedneedle' ELSE 'Unrelated '||i END,'legacy-'||i||'.md' FROM generate_series({previous+1},{size})i;
 INSERT INTO permission_entries(id,object_kind,object_id,page,permission,holder_kind,holder_id,source)
 SELECT md5('legacy/owner/'||i)::uuid,'document',md5('legacy/document/'||i)::uuid,'live','access.own','user',{user},'creation' FROM generate_series({previous+1},{size})i;""")
 sql.extend(f'ANALYZE {t};' for t in tables)
 for label,statement in [('matches',query),('seeds',seed)]:
  sql.append(f"\\o '{out/f'{label}-{size}.plan.json'}'\nEXPLAIN(ANALYZE,BUFFERS,FORMAT JSON) {statement};\n\\o\n\\echo DONE {label}-{size}")
 previous=size
sql.append('ROLLBACK;')
(out/'run.sql').write_text('\n'.join(sql))
(out/'inputs.json').write_text(json.dumps({'head':subprocess.check_output(['git','-C',str(tree),'rev-parse','HEAD'],text=True).strip(),'match_sha256':hashlib.sha256(query.encode()).hexdigest(),'seeds_sha256':hashlib.sha256(seed.encode()).hexdigest(),'sizes':sizes},indent=2)+'\n')
with (out/'psql.log').open('x') as log:
 r=subprocess.run(['psql',args.database_url,'-X','-Atq','-v','ON_ERROR_STOP=1','-f',str(out/'run.sql')],stdout=log,stderr=subprocess.STDOUT,timeout=21600)
if r.returncode:raise SystemExit(r.returncode)
def walk(n):
 yield n
 for child in n.get('Plans',[]):yield from walk(child)
report=[]
for size in sizes:
 for label,expected in [('matches',1),('seeds',2)]:
  p=json.loads((out/f'{label}-{size}.plan.json').read_text())[0]
  rows={}
  for n in walk(p['Plan']):
   if 'Relation Name' in n:
    name=n['Relation Name'];rows[name]=rows.get(name,0)+(n.get('Actual Rows',0)+n.get('Rows Removed by Filter',0)+n.get('Rows Removed by Index Recheck',0))*n.get('Actual Loops',0)
  assert p['Plan']['Actual Rows']==expected,(size,label,p['Plan']['Actual Rows'])
  assert rows.get('documents',0)<=8,(size,label,rows)
  assert rows.get('permission_entries',0)==0,(size,label,rows)
  report.append({'size':size,'case':label,'returned_rows':expected,'execution_ms':p['Execution Time'],'rows_examined':rows})
for label in ['matches','seeds']:
 matrix=[r['rows_examined'] for r in report if r['case']==label]
 assert all(m==matrix[0] for m in matrix),(label,matrix)
(out/'report.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps(report),flush=True)
print('DONE legacy proof',flush=True)
