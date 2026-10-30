"""Probe real holder and document-page SQL on million-row disposable relations; compile no Rust."""
from pathlib import Path
import argparse,hashlib,json,os,re,subprocess,time
from urllib.parse import urlparse,unquote

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output',type=Path,required=True)
parser.add_argument('--holder-candidate',type=Path,required=True)
options=parser.parse_args()
notes=options.output.resolve();notes.mkdir(parents=True,exist_ok=True)
tree=Path(__file__).resolve().parents[3]
url=urlparse(os.environ['ORCH_TEST_DATABASE_URL'])
database='orch_budgets_scale_'+str(os.getpid())
assert re.fullmatch(r'orch_budgets_scale_[0-9]+',database)
env=dict(os.environ,PGPASSWORD=unquote(url.password or ''),PGCONNECT_TIMEOUT='5')
base=['psql','-X','-q','-A','-t','-v','ON_ERROR_STOP=1','-h',url.hostname or '127.0.0.1','-p',str(url.port or 5432),'-U',unquote(url.username or '')]
output=notes;output.mkdir(exist_ok=True)
def sql(text,db=database,seconds=300):
    result=subprocess.run([*base,'-d',db],input=text,text=True,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=seconds)
    if result.returncode:raise RuntimeError(result.stderr[-6000:])
    return result.stdout.strip()
def plan(label,query,types,args):
    statement="BEGIN READ ONLY; SET LOCAL statement_timeout='60s'; SET LOCAL jit=off; SET LOCAL plan_cache_mode=force_generic_plan; PREPARE measured("+types+") AS "+query+"; EXPLAIN(ANALYZE,BUFFERS,SETTINGS,FORMAT JSON) EXECUTE measured("+args+"); DEALLOCATE measured; ROLLBACK;"
    raw=sql(statement);x=json.loads(raw)
    (output/(label+'.json')).write_text(json.dumps(x,indent=2))
    p=x[0];print(label,json.dumps({k:p.get(k) for k in ['Planning Time','Execution Time']}),flush=True)
    return p
root="md5('root')::uuid";department="md5('department')::uuid";project="md5('project')::uuid";outer="md5('outer')::uuid";inner="md5('inner')::uuid"
created=False
try:
    sql('CREATE DATABASE '+database,unquote(url.path.lstrip('/')) or 'postgres');created=True
    sql("""
CREATE TABLE users(id uuid PRIMARY KEY,status text NOT NULL DEFAULT 'active',is_system_admin boolean NOT NULL DEFAULT false,padding text DEFAULT repeat('x',120));
CREATE INDEX users_system_admin_idx ON users(id) WHERE is_system_admin AND status='active';
CREATE TABLE nodes(id uuid PRIMARY KEY,kind text,parent_id uuid,org_id uuid,path uuid[],deleted_at timestamptz);
CREATE INDEX nodes_path_idx ON nodes USING gin(path);
CREATE INDEX nodes_org_id_idx ON nodes(org_id,id);
CREATE TABLE groups(id uuid PRIMARY KEY,org_id uuid);
CREATE TABLE group_members(group_id uuid,user_id uuid,member_group_id uuid,member_node_id uuid,member_org_id uuid,member_role text,UNIQUE(group_id,user_id));
CREATE UNIQUE INDEX group_members_full_key ON group_members(group_id,user_id,member_group_id,member_node_id,member_org_id,member_role) NULLS NOT DISTINCT;
CREATE INDEX group_members_nested_idx ON group_members(member_group_id) WHERE member_group_id IS NOT NULL;
CREATE TABLE org_members(org_id uuid,user_id uuid,domain_id uuid,PRIMARY KEY(org_id,user_id));
CREATE INDEX org_members_domain_idx ON org_members(domain_id);
CREATE INDEX org_members_user_idx ON org_members(user_id);
CREATE TABLE org_suspensions(org_id uuid,user_id uuid,PRIMARY KEY(org_id,user_id));
CREATE TABLE node_roles(id uuid PRIMARY KEY,node_id uuid,subject_kind text,subject_id uuid,subject_role text,role text,removed_at timestamptz,expires_at timestamptz);
CREATE UNIQUE INDEX node_roles_active_key ON node_roles(node_id,subject_kind,subject_id,subject_role) NULLS NOT DISTINCT WHERE removed_at IS NULL;
CREATE INDEX node_roles_subject_idx ON node_roles(subject_kind,subject_id) WHERE removed_at IS NULL;
CREATE TABLE permission_rows(object_kind text,object_id uuid,page text,profile text,permission text,inherit boolean,locked boolean,PRIMARY KEY(object_kind,object_id,page,profile,permission));
CREATE TABLE permission_entries(id uuid PRIMARY KEY,object_kind text,object_id uuid,page text,profile text,permission text,holder_kind text,holder_id uuid,holder_role text,source text,removed_at timestamptz,expires_at timestamptz);
CREATE UNIQUE INDEX permission_entries_active_key ON permission_entries(object_kind,object_id,page,profile,permission,holder_kind,holder_id,holder_role) NULLS NOT DISTINCT WHERE removed_at IS NULL;
CREATE INDEX permission_entries_holder_idx ON permission_entries(holder_kind,holder_id) WHERE removed_at IS NULL;
CREATE TABLE documents(id uuid PRIMARY KEY,node_id uuid,org_id uuid,owner_id uuid,created_by uuid,created_by_thread uuid,created_by_agent text,title text,description text DEFAULT '',description_source jsonb,doc_seq bigint DEFAULT 0,seq_updated bigint DEFAULT 0,purged_seq bigint DEFAULT 0,block_count integer DEFAULT 0,bytes bigint DEFAULT 0,files_bytes bigint DEFAULT 0,version integer DEFAULT 0,version_at timestamptz,span jsonb DEFAULT '{}',told uuid[] DEFAULT '{}',created_at timestamptz DEFAULT '2026-01-01Z',updated_at timestamptz DEFAULT '2026-01-01Z',updated_by uuid,deleted_at timestamptz,deleted_by uuid);
CREATE INDEX documents_node_idx ON documents(node_id,updated_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX documents_org_idx ON documents(org_id) WHERE deleted_at IS NULL;
CREATE TABLE scale_threads(id uuid PRIMARY KEY,total_tokens bigint NOT NULL);
CREATE TABLE scale_items(id uuid PRIMARY KEY,thread_id uuid,seq bigint,body text);
CREATE INDEX scale_items_tail_idx ON scale_items(thread_id,seq DESC);
""")
    fn=(tree/'migrations/0032_expand_holder_leaves.sql').read_text();sql(fn)
    started=time.monotonic()
    sql("INSERT INTO users(id) SELECT md5('user'||i)::uuid FROM generate_series(1,1000000)i; INSERT INTO nodes SELECT md5('unrelated'||i)::uuid,'org',NULL,md5('unrelated'||i)::uuid,ARRAY[md5('unrelated'||i)::uuid],NULL FROM generate_series(1,1000000)i; INSERT INTO groups SELECT md5('group'||i)::uuid,md5('unrelated'||i)::uuid FROM generate_series(1,1000000)i; INSERT INTO group_members(group_id,user_id) SELECT md5('group'||i)::uuid,md5('user'||i)::uuid FROM generate_series(1,1000000)i;")
    sql(f"INSERT INTO nodes VALUES({root},'org',NULL,{root},ARRAY[{root}],NULL),({department},'org',{root},{root},ARRAY[{root},{department}],NULL),({project},'project',{department},{root},ARRAY[{root},{department},{project}],NULL); INSERT INTO groups VALUES({outer},{root}),({inner},{root}); INSERT INTO group_members(group_id,user_id) SELECT {inner},md5('user'||i)::uuid FROM generate_series(1,1000)i; INSERT INTO group_members(group_id,member_group_id) VALUES({outer},{inner}); INSERT INTO org_members SELECT {root},md5('user'||i)::uuid,{department} FROM generate_series(1,1000)i; INSERT INTO node_roles VALUES(md5('role')::uuid,{department},'group',{outer},NULL,'developer',NULL,NULL); INSERT INTO permission_entries VALUES(md5('owner')::uuid,'node',{project},'live','any','access.own','group',{outer},NULL,'manual',NULL,NULL), (md5('view')::uuid,'system','00000000-0000-0000-0000-000000000000','defaults','any','project.view','role',NULL,'viewer','manual',NULL,NULL);")
    sql(f"INSERT INTO documents(id,node_id,org_id,owner_id,created_by,title) SELECT md5('doc'||i)::uuid,{project},{root},md5('user1')::uuid,md5('user1')::uuid,'Document '||i FROM generate_series(1,1000000)i; INSERT INTO scale_threads SELECT md5('thread'||i)::uuid,CASE WHEN i=1 THEN 100000000 WHEN i BETWEEN 2 AND 10 THEN 0 ELSE 10000000 END FROM generate_series(1,1000)i; INSERT INTO scale_items SELECT md5('item'||i)::uuid,md5('thread'||(1+(i-1)%1000))::uuid,1+(i-1)/1000,repeat('word ',32) FROM generate_series(1,1000000)i;")
    sql('ANALYZE;');print('load_seconds',round(time.monotonic()-started,3),flush=True)
    counts=sql('SELECT (SELECT count(*) FROM users),(SELECT count(*) FROM group_members),(SELECT count(*) FROM documents),(SELECT count(*) FROM scale_threads),(SELECT sum(total_tokens) FROM scale_threads),(SELECT count(*) FROM scale_items);')
    print('world_counts',counts,flush=True);assert counts.split('|')==['1000000','1001001','1000000','1000','10000000000','1000000']
    access=(tree/'crates/control/db/src/queries/access.rs').read_text()
    group=re.search(r'"(WITH requested AS MATERIALIZED[\s\S]*?ORDER BY g.id, h.user_id LIMIT \$2)"',access).group(1)
    for label in ['group-first','group-warm']:
        p=plan(label,group,'uuid[],bigint,timestamptz',f"ARRAY[{outer}],2000,'2026-10-08T14:00Z'");assert p['Plan']['Actual Rows']==1000
    def holder_body(definition):
        body=definition.split('RETURN QUERY'+chr(10),1)[1].rsplit(chr(10)+'END',1)[0].strip().rstrip(';')
        for source,target in [('wanted_kinds','$1'),('wanted_ids','$2'),('minimum_roles','$3'),('sampled_at','$4'),('AND system_admin AND','AND $5 AND')]:body=body.replace(source,target)
        return body
    original_inner=holder_body(fn)
    # These local options are the actual function's settings, not global server settings.
    def inner_plan(label,body):
        args=f"ARRAY['group'],ARRAY[{outer}],ARRAY[''],'2026-10-08T14:00Z',true"
        statement="BEGIN READ ONLY; SET LOCAL statement_timeout='60s'; SET LOCAL jit=off; SET LOCAL plan_cache_mode=force_generic_plan; SET LOCAL join_collapse_limit=1; SET LOCAL from_collapse_limit=1; PREPARE measured(text[],uuid[],text[],timestamptz,boolean) AS "+body+"; EXPLAIN(ANALYZE,BUFFERS,SETTINGS,FORMAT JSON) EXECUTE measured("+args+"); DEALLOCATE measured; ROLLBACK;"
        result=json.loads(sql(statement));(output/(label+'.json')).write_text(json.dumps(result,indent=2));print(label,result[0]['Execution Time'],flush=True)
    inner_plan('holder-inner-before',original_inner)
    candidate_definition=options.holder_candidate.read_text()
    sql(candidate_definition)
    inner_plan('holder-inner-after',holder_body(candidate_definition))
    checked=json.loads((output/'holder-inner-after.json').read_text())[0]
    def check_holder(node):
        if node.get('Relation Name') in ['users','groups','group_members','nodes'] and node.get('Actual Loops',0)>0:
            assert node['Node Type'] not in ['Seq Scan','Parallel Seq Scan'],node
            assert node.get('Actual Rows',0)*node.get('Actual Loops',0)<=5000,node
        for child in node.get('Plans',[]):check_holder(child)
    check_holder(checked['Plan'])

    for label in ['group-after-first','group-after-warm']:
        measured=plan(label,group,'uuid[],bigint,timestamptz',f"ARRAY[{outer}],2000,'2026-10-08T14:00Z'");assert measured['Plan']['Actual Rows']==1000
    counts_query=re.search(r'"(WITH chips AS MATERIALIZED[\s\S]*?)"',access).group(1)
    for label in ['role-counts-first','role-counts-warm']:
        measured=plan(label,counts_query,'text[],uuid[],timestamptz,boolean[]',f"ARRAY['viewer','admin'],ARRAY[{project},{project}],'2026-10-08T14:00Z',ARRAY[true,false]");assert measured['Plan']['Actual Rows']==2
    assert sql(f"SELECT count(*) FROM access_holder_people_many(ARRAY['group'],ARRAY[{outer}],ARRAY[''],'2026-10-08Z',true)")=='1000'
    sql(f"INSERT INTO org_suspensions VALUES({root},md5('user1')::uuid); UPDATE users SET status='suspended' WHERE id=md5('user2')::uuid;")
    assert sql(f"SELECT count(*) FROM access_holder_people_many(ARRAY['group'],ARRAY[{outer}],ARRAY[''],'2026-10-08Z',true)")=='998'
    sql(f"UPDATE nodes SET deleted_at='2026-01-01Z' WHERE id={root};")
    assert sql(f"SELECT count(*) FROM access_holder_people_many(ARRAY['group'],ARRAY[{outer}],ARRAY[''],'2026-10-08Z',true)")=='0'
    sql(f"UPDATE nodes SET deleted_at=NULL WHERE id={root}; UPDATE users SET status='active' WHERE id=md5('user2')::uuid; DELETE FROM org_suspensions;")
    docs=(tree/'crates/control/db/src/queries/documents.rs').read_text()
    columns=re.search(r'const DOCUMENT_COLUMNS: &str = "([\s\S]*?)";',docs).group(1)
    section=docs[docs.index('pub async fn list_beside('):docs.index('pub async fn names_beside(')]
    query=re.search(r'"(SELECT \{DOCUMENT_COLUMNS\}[\s\S]*?LIMIT \$6 OFFSET \$7)"',section).group(1).replace('{DOCUMENT_COLUMNS}',columns)
    query=query.replace('{reach}','($3 AND $1 = ANY(n.path) OR NOT $3 AND d.node_id = $1)')
    args=f"{project},false,false,NULL,NULL,31,0";types='uuid,boolean,boolean,uuid,timestamptz,bigint,bigint'
    before=plan('documents-before',query,types,args);assert before['Plan']['Actual Rows']==31
    sql('CREATE INDEX documents_page_candidate ON documents(node_id,(deleted_at IS NOT NULL),updated_at DESC,id DESC); ANALYZE documents;')
    candidate=query.replace('($3 AND $1 = ANY(n.path) OR NOT $3 AND d.node_id = $1)','(NOT $3 AND d.node_id = $1)')
    after=plan('documents-after',candidate,types,args);assert after['Plan']['Actual Rows']==31
    def scans(root,relation):
        values=[]
        def visit(node):
            if node.get('Relation Name')==relation:values.append(node)
            for child in node.get('Plans',[]):visit(child)
        visit(root['Plan']);return values
    before_scans=scans(before,'documents');after_scans=scans(after,'documents')
    before_rows=sum(node['Actual Rows']*node['Actual Loops'] for node in before_scans)
    after_rows=sum(node['Actual Rows']*node['Actual Loops'] for node in after_scans)
    # PostgreSQL reports rounded per-worker averages for parallel scans.
    assert before_rows>=1000000-sum(node['Actual Loops'] for node in before_scans)
    assert after_rows<=31 and any(node.get('Index Name')=='documents_page_candidate' for node in after_scans)
    print('document_rows_read',before_rows,after_rows,flush=True)
    tail=plan('synthetic-world-tail','SELECT id,seq,body FROM scale_items WHERE thread_id=$1 ORDER BY seq DESC LIMIT $2','uuid,bigint',"md5('thread1')::uuid,100");assert tail['Plan']['Actual Rows']==100
    summary={'counts':counts,'function_sha256':hashlib.sha256(fn.encode()).hexdigest(),'docs_before_ms':before['Execution Time'],'docs_after_ms':after['Execution Time'],'description':'exact production holder/document SQL on isolated matching query-column/index relations; token totals are logical counters, not 10 billion materialized token strings; synthetic tail is fixture sanity, not production timeline proof'}
    (output/'summary.json').write_text(json.dumps(summary,indent=2));print('SQL scale assertions PASS',flush=True)
finally:
    if created:sql('DROP DATABASE '+database+' WITH (FORCE)',unquote(url.path.lstrip('/')) or 'postgres')
    print('DONE scale-million',flush=True)
