"""Prove current-schema notice behavior and indexed storage bounds at two corpus sizes on disk."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time

ROOT = Path(__file__).resolve().parents[4]
PORT = 41676
PAGE = 64
SIZES = (100_000, 1_000_000)
NAME = 'orch-isolation-storage-scale-final'
THREAD = "md5('storage-thread')::uuid"
RUNTIME = '10000000-0000-4000-8000-000000000001'


def run(args, body=None, seconds=600):
    result = subprocess.run(args, input=body, text=True, capture_output=True, timeout=seconds)
    if result.returncode:
        raise RuntimeError(f'{args[:4]} exit={result.returncode}: {result.stderr[-8000:]}')
    return result.stdout


def sql(body):
    return run(['docker','exec','-i',NAME,'psql','-X','-q','-A','-t','-v','ON_ERROR_STOP=1',
                '-U','orch','-p',str(PORT),'-d','postgres'], "SET statement_timeout='10min';\n"+body)


def walk(plan):
    yield plan
    for child in plan.get('Plans', []):
        yield from walk(child)


def checked_plan(out, name, query, table, maximum, required):
    plan = json.loads(sql('EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) '+query))[0]['Plan']
    (out/(name+'.json')).write_text(json.dumps(plan,indent=2)+'\n')
    scans = [node for node in walk(plan) if node.get('Relation Name')==table]
    assert scans, (name,plan)
    assert all(node['Node Type']!='Seq Scan' for node in scans), (name,plan)
    examined = sum((node.get('Actual Rows',0)+node.get('Rows Removed by Filter',0)+node.get('Rows Removed by Index Recheck',0))*node.get('Actual Loops',1) for node in scans)
    assert examined<=maximum, (name,examined,plan)
    indexes = {node.get('Index Name') for node in scans}
    assert required in indexes, (name,indexes,plan)
    return {'case':name,'rows_examined':examined,'indexes':sorted(indexes-{None})}



def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--data-root',type=Path,required=True)
    parser.add_argument('--out',type=Path,required=True)
    args=parser.parse_args()
    args.data_root.mkdir(parents=True,exist_ok=True)
    args.out.mkdir(parents=True,exist_ok=True)
    assert not run(['docker','ps','-aq','--filter','name=^/'+NAME+'$']).strip(), 'owned container already exists'
    started=False
    try:
        run(['docker','run','-d','--name',NAME,'--network','none','--memory','1g','--cpus','2',
             '--user',f'{os.getuid()}:{os.getgid()}',
             '-e','POSTGRES_USER=orch','-e','POSTGRES_PASSWORD=storage-proof',
             '-e','PGDATA=/var/lib/postgresql/data','-v',str(args.data_root.resolve())+':/var/lib/postgresql/data',
             'postgres:17@sha256:31ba9c94a03ada9988f2bb493a22a75eef3a185884533e5cdbcd498ea835b08f','postgres','-p',str(PORT),'-c','shared_buffers=32MB','-c','max_connections=10'])
        started=True
        deadline=time.monotonic()+60
        while time.monotonic()<deadline:
            ready=subprocess.run(['docker','exec',NAME,'pg_isready','-U','orch','-p',str(PORT)],capture_output=True,timeout=10)
            if ready.returncode==0:break
            time.sleep(1)
        else:raise TimeoutError('sixty-second PostgreSQL startup')
        # A fresh database in the owned on-disk server allows a repeat without deleting fixture files.
        sql('DROP SCHEMA public CASCADE; CREATE SCHEMA public;')
        migrations=sorted((ROOT/'migrations').glob('*.sql'))
        assert migrations, 'the current schema must be present'
        applied=[]
        for file in migrations:
            source=file.read_text()
            sql('BEGIN;\n'+source+'\nCOMMIT;')
            applied.append({'file':file.name,'sha256':hashlib.sha256(source.encode()).hexdigest()})
        sql("""BEGIN;
          INSERT INTO users(id,username,display_name,avatar_color) VALUES(md5('storage-owner')::uuid,'storage-reader','Storage reader',0);
          INSERT INTO nodes(id,kind,org_id,path,name,owner_id,created_by)
            VALUES(md5('storage-org')::uuid,'org',md5('storage-org')::uuid,ARRAY[md5('storage-org')::uuid],'Office',md5('storage-owner')::uuid,md5('storage-owner')::uuid);
          INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by)
            VALUES(md5('storage-topic')::uuid,'topic',md5('storage-org')::uuid,md5('storage-org')::uuid,ARRAY[md5('storage-org')::uuid,md5('storage-topic')::uuid],'Topic',md5('storage-owner')::uuid,md5('storage-owner')::uuid);
          INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by)
            VALUES(md5('storage-thread')::uuid,'thread',md5('storage-topic')::uuid,md5('storage-org')::uuid,
              ARRAY[md5('storage-org')::uuid,md5('storage-topic')::uuid,md5('storage-thread')::uuid],'Conversation',md5('storage-owner')::uuid,md5('storage-owner')::uuid);
          INSERT INTO threads(node_id,topic_id) VALUES(md5('storage-thread')::uuid,md5('storage-topic')::uuid);
          CREATE TEMP TABLE storage_notice_expected AS
            SELECT jsonb_agg(CASE WHEN n%11=0 THEN 'null'::jsonb
              WHEN n%7=0 THEN jsonb_build_object('text','No identifier','values',jsonb_build_array(n,'你好'))
              ELSE jsonb_build_object('id','duplicate-'||(n%5),'values',jsonb_build_array(n,'Complete value',jsonb_build_object('kept',true))) END ORDER BY n)
              AS notices FROM generate_series(1,513) n;
          UPDATE threads SET pending_notices=(SELECT notices FROM storage_notice_expected) WHERE node_id=md5('storage-thread')::uuid;
          CREATE TABLE storage_notice_behavior AS SELECT
            (SELECT notices FROM storage_notice_expected)=(SELECT jsonb_agg(notice ORDER BY position) FROM thread_pending_notices) AS preserved,
            (SELECT count(*) FROM thread_pending_notices) AS notice_rows,
            (SELECT jsonb_array_length(pending_notices) FROM threads WHERE node_id=md5('storage-thread')::uuid) AS preview;
          INSERT INTO jobs(id,kind,plan,created_at) VALUES
            ('20000000-0000-4000-8000-000000000001','environment_provision',jsonb_build_object('runtime','10000000-0000-4000-8000-000000000001','created_spec',jsonb_build_object('spec',jsonb_build_object('runtime','runc'))),'2026-01-01'),
            ('20000000-0000-4000-8000-000000000002','environment_provision',jsonb_build_object('runtime','10000000-0000-4000-8000-000000000001','created_spec',jsonb_build_object('spec',jsonb_build_object('runtime','kata'))),'2026-01-01');
          COMMIT;""")
        behavior=json.loads(sql('SELECT row_to_json(result) FROM storage_notice_behavior result;'))
        assert behavior=={'preserved':True,'notice_rows':513,'preview':PAGE},behavior
        (args.out/'schema-behavior.json').write_text(json.dumps({'applied':applied,'result':behavior},indent=2)+'\n')
        print('CURRENT SCHEMA NOTICE VALUES PRESERVED',behavior,flush=True)
        sql("TRUNCATE thread_pending_notices; UPDATE threads SET pending_notices='[]'::jsonb;")
        suppliers=ROOT/'crates/control/db/src/queries/threads'
        queries={name:(suppliers/('notices_'+name+'.sql')).read_text() for name in ['append','page','forget','segment','delivery','forget_positions']}
        resources=(ROOT/'crates/control/db/src/queries/resources.rs').read_text()
        match=re.search(r"SELECT plan->'created_spec'->'spec' AS spec FROM jobs\s+WHERE plan->>'runtime'=r.id::text AND plan->'created_spec'->'spec' IS NOT NULL\s+ORDER BY created_at DESC,id DESC LIMIT 1",resources)
        assert match,'actual capacity supplier must match the index proof'
        capacity=match[0].replace('r.id::text',"'"+RUNTIME+"'")
        # Exercise the actual projection with different current values and frozen limits.
        projection=resources.split('pub async fn loads(',1)[1].split('e.last_activity_at',1)[0]
        projection=projection.split('r.environment_id,',1)[1].rstrip().rstrip(',')
        facts=[]
        for kind in ['environment','provider']:
            fixture=f"""WITH r(kind) AS(VALUES('{kind}'::text)),
              frozen(spec) AS(VALUES('{{"needs":{{"disk_gb":7}},"resources":{{"memory_mb":512}},"resources_max":{{"memory_mb":1024}},"runtime":"kata","image":"frozen-image","unrelated":"Large unused definition"}}'::jsonb)),
              t(spec) AS(VALUES('{{"resources":{{"memory_mb":1}},"runtime":"runc"}}'::jsonb)),
              e(config) AS(VALUES('{{"env.needs":{{"memory_mb":256}},"env.resources":{{"memory_mb":768}},"env.runtime":"runsc","unrelated":"Unused settings"}}'::jsonb)),
              p(runtime_config) AS(VALUES('{{"runtime.needs":{{"memory_mb":128}},"runtime.resources":{{"memory_mb":384}},"env.runtime":"kata","unrelated":"Unused provider settings"}}'::jsonb))
              SELECT row_to_json(projected) FROM(SELECT {projection} FROM r CROSS JOIN frozen CROSS JOIN t CROSS JOIN e CROSS JOIN p) projected;"""
            value=json.loads(sql(fixture));facts.append(value)
            assert value['spec']=={'needs':{'disk_gb':7},'resources':{'memory_mb':512},'resources_max':{'memory_mb':1024},'runtime':'kata'},value
            prefix='runtime' if kind=='provider' else 'env'
            assert value['config']=={prefix+'.needs':{'memory_mb':128 if kind=='provider' else 256},prefix+'.resources':{'memory_mb':384 if kind=='provider' else 768},'env.runtime':'kata' if kind=='provider' else 'runsc'},value
        (args.out/'capacity-projections.json').write_text(json.dumps(facts,indent=2)+'\n')
        reports=[];previous=0
        for size in SIZES:
            sql(f"INSERT INTO jobs(id,kind,plan) SELECT md5('capacity-job-'||n)::uuid,'environment_provision',jsonb_build_object('runtime',md5('unrelated-runtime-'||n)::uuid,'created_spec',jsonb_build_object('spec',jsonb_build_object('runtime','runc'))) FROM generate_series({previous+1},{size}) n;")
            sql(f"INSERT INTO thread_pending_notices(thread_id,notice) SELECT {THREAD},jsonb_build_object('id','corpus-'||n,'text','Queued value '||n) FROM generate_series({previous+1},{size}) n ORDER BY n;")
            sql('ANALYZE jobs; ANALYZE thread_pending_notices;')
            report=checked_plan(args.out,f'capacity-{size}',capacity,'jobs',1,'jobs_runtime_created_spec_idx')
            reports.append(report)
            preview=sql("SELECT prosrc FROM pg_proc WHERE oid='orch_pending_notice_preview(uuid)'::regprocedure")
            preview=re.sub(r'\bthread\b',THREAD,preview).strip()
            reports.append(checked_plan(args.out,f'notice-preview-{size}',preview,'thread_pending_notices',PAGE,'thread_pending_notices_pkey'))
            assert json.loads(sql(capacity))['runtime']=='kata'
            first=int(sql(f'SELECT min(position) FROM thread_pending_notices WHERE thread_id={THREAD};'))
            for label,cursor in [('first',0),('middle',first+size//2),('last',first+size-65)]:
                statement=queries['page'].replace('$1',THREAD).replace('$2',str(cursor))
                reports.append(checked_plan(args.out,f'notices-{label}-{size}',statement,'thread_pending_notices',PAGE,'thread_pending_notices_pkey'))
            identity=f"SELECT EXISTS(SELECT 1 FROM thread_pending_notices WHERE thread_id={THREAD} AND notice_id='corpus-{size}')"
            reports.append(checked_plan(args.out,f'notice-dedup-{size}',identity,'thread_pending_notices',1,'thread_pending_notices_identity_idx'))
            statement=queries['append'].replace('$1',THREAD).replace('$2',f"'{{\"id\":\"new-{size}\",\"text\":\"One appended value\"}}'")
            reports.append(checked_plan(args.out,f'notice-append-{size}',statement,'thread_pending_notices',1,'thread_pending_notices_identity_idx'))
            statement=queries['segment'].replace('$1',THREAD).replace('$2',"md5('absent-segment')::uuid")
            reports.append(checked_plan(args.out,f'notice-lanes-{size}',statement,'thread_pending_notices',PAGE,'thread_pending_notices_lane_idx'))
            statement=queries['forget'].replace('$1',THREAD).replace('$2',f"ARRAY(SELECT 'corpus-'||n FROM generate_series({size-63},{size}) n)")
            reports.append(checked_plan(args.out,f'notice-forget-{size}',statement,'thread_pending_notices',2*PAGE,'thread_pending_notices_identity_idx'))
            assert int(sql(f"SELECT count(*) FROM thread_pending_notices WHERE thread_id={THREAD} AND notice_id=ANY(ARRAY(SELECT 'corpus-'||n FROM generate_series({size-63},{size}) n))"))==0
            assert int(sql(f"SELECT count(*) FROM thread_pending_notices WHERE thread_id={THREAD} AND notice_id='new-{size}'"))==1
            positions=f"ARRAY(SELECT position FROM thread_pending_notices WHERE thread_id={THREAD} ORDER BY position LIMIT {PAGE})"
            pinned=json.loads(sql(f"SELECT array_to_json({positions})"))
            pinned_sql="ARRAY["+','.join(str(position) for position in pinned)+"]::bigint[]"
            statement=queries['delivery'].replace('$1',THREAD).replace('$2',pinned_sql)
            reports.append(checked_plan(args.out,f'notice-delivery-{size}',statement,'thread_pending_notices',PAGE,'thread_pending_notices_pkey'))
            statement=queries['forget_positions'].replace('$1',THREAD).replace('$2',pinned_sql)
            reports.append(checked_plan(args.out,f'notice-ack-{size}',statement,'thread_pending_notices',PAGE,'thread_pending_notices_pkey'))
            assert int(sql(f"SELECT count(*) FROM thread_pending_notices WHERE thread_id={THREAD} AND position=ANY({pinned_sql})"))==0
            # Repeating the exact receipt never consumes a later page or a new arrival.
            sql(statement)
            assert int(sql(f"SELECT count(*) FROM thread_pending_notices WHERE thread_id={THREAD} AND notice_id='new-{size}'"))==1
            previous=size
        for case in ['capacity','notice-preview','notices-first','notices-middle','notices-last','notice-dedup','notice-append','notice-lanes','notice-forget','notice-delivery','notice-ack']:
            a,b=[next(row for row in reports if row['case']==f'{case}-{size}') for size in SIZES]
            assert b['rows_examined']<=a['rows_examined'],(a,b)
        (args.out/'report.json').write_text(json.dumps({'sizes':SIZES,'page':PAGE,'behavior':behavior,'cases':reports},indent=2)+'\n')
        print('STORAGE SCALE PASSED',json.dumps(reports),flush=True)
    finally:
        if started:
            diagnostic=subprocess.run(['docker','logs','--tail','80',NAME],capture_output=True,text=True,timeout=30)
            (args.out/'postgres.log').write_text(diagnostic.stdout+diagnostic.stderr)
            run(['docker','inspect','--format','{{.Name}} {{.State.Status}}',NAME])
            run(['docker','rm','-f',NAME],seconds=60)


if __name__=='__main__':
    main()
