#!/usr/bin/env python3
"""Check indexed message/revision MIME filters at two selectable synthetic row counts."""
import argparse
import hashlib
import json
import re
import subprocess
from pathlib import Path
from urllib.parse import urlsplit


def walk(node):
    yield node
    for child in node.get('Plans', []):
        yield from walk(child)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--database-url', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--rows', type=int, choices=[100_000, 1_000_000], default=1_000_000)
    args = parser.parse_args()
    address = urlsplit(args.database_url)
    if (address.hostname != '127.0.0.1' or not 41000 <= (address.port or 0) <= 41999
            or address.port == 41532 or not address.path.startswith('/orchprobe_search_')):
        parser.error('use an isolated loopback orchprobe_search_ database')
    args.output.mkdir(parents=True, exist_ok=False)
    tree = Path(__file__).resolve().parents[2]
    migration = (tree / 'migrations/0040_search_file_types.sql').read_text()
    function = re.search(r'CREATE FUNCTION orch_search_file_types.*?\$media\$;', migration, re.S).group(0)
    indexes = [re.search(r'CREATE INDEX ' + name + r'.*?;', migration, re.S).group(0)
               for name in ['search_items_media_idx', 'search_revision_items_recent_idx', 'search_revisions_media_idx', 'search_messages_media_idx']]
    template = (tree / 'crates/control/db/src/queries/search_payload_file.sql').read_text()
    sample = (tree / 'crates/control/db/src/queries/search_media_sample.sql').read_text()
    revision_sample = (tree / 'crates/control/db/src/queries/search_revision_sample.sql').read_text().replace('$17','200')
    commands = [f"""
BEGIN;
SET statement_timeout='15min';
CREATE TEMP TABLE nodes(id uuid PRIMARY KEY,created_by uuid,owner_id uuid);
INSERT INTO nodes SELECT md5('media-thread/'||i)::uuid,md5('media-owner')::uuid,md5('media-owner')::uuid FROM generate_series(0,999)i;
CREATE TEMP TABLE items(id uuid PRIMARY KEY,thread_id uuid,payload jsonb,kind text,search_text text,created_at timestamptz);
INSERT INTO items SELECT md5('media-scale/'||i)::uuid,md5('media-thread/'||(i%1000))::uuid,
 CASE i
 WHEN 1 THEN '{{"parts":[{{"type":"image","media_type":"image/png"}}]}}'::jsonb
 WHEN 2 THEN '{{"parts":[{{"type":"file","blob":null}}]}}'::jsonb
 WHEN 3 THEN '{{"parts":[{{"type":"file","media_type":"application/pdf"}}]}}'::jsonb
 WHEN 4 THEN '{{"parts":[{{"type":"file","media_type":"image"}}]}}'::jsonb
 WHEN 5 THEN '{{"tool":{{"kind":"image","media_type":"image/webp"}}}}'::jsonb
 ELSE CASE WHEN i%2=0 THEN '{{"parts":[{{"type":"file","media_type":"text/plain"}}]}}'::jsonb
 ELSE '{{"parts":[{{"type":"text","text":"release","media_type":"image/png","kind":"image"}}]}}'::jsonb END END,
 CASE WHEN i=5 THEN 'tool_call' ELSE 'user_message' END,'release',timestamptz '2026-01-01'+i*interval '1 second'
 FROM generate_series(1,{args.rows})i;
CREATE INDEX search_items_scope_recent_idx ON items(thread_id,created_at DESC,id DESC) INCLUDE(kind)
 WHERE search_text IS NOT NULL AND kind<>'marker' AND search_text NOT LIKE '%<orch>%';
CREATE TEMP TABLE chat_messages AS SELECT id,coalesce(payload->'parts','[{{"type":"text","text":"release"}}]'::jsonb) content,search_text,created_at,NULL::timestamptz deleted_at FROM items;
CREATE INDEX media_messages_recent ON chat_messages(created_at DESC,id DESC);
CREATE TEMP TABLE thread_item_revisions AS SELECT id item_id,1::bigint seq_updated,jsonb_build_object('payload',payload,'search_text',search_text) body FROM items;
CREATE UNIQUE INDEX media_revisions_item ON thread_item_revisions(item_id,seq_updated DESC);
{function.replace('CREATE FUNCTION ', 'CREATE OR REPLACE FUNCTION ')}
{' '.join(indexes)}
ANALYZE nodes;ANALYZE items;ANALYZE chat_messages;ANALYZE thread_item_revisions;
SET statement_timeout='30s';
SET enable_seqscan=off;SET enable_indexscan=on;SET enable_bitmapscan=on;
"""]
    cases = []

    def record(label, source, expression, condition, order, kind, count, ids=None, index=None, relations=None, sampled=False):
        selection = "('" + json.dumps({'file_type':kind}) + "'::jsonb)"
        predicate = template.replace('__SELECTION__', selection).replace('__PAYLOAD__', expression)
        if sampled:
            if 'thread_item_revisions' in source:
                source = revision_sample
            inner = f'SELECT i.id,{predicate} AS media_matches FROM {source} WHERE {condition} ORDER BY {order} LIMIT 200'
            statement = sample.replace('__OUTER_COLUMNS__','s.id').replace('__INNER__',inner).replace('__TEXT_LOOKUP__','').replace('__TEXT_MATCH__','true')
        else:
            # Sparse density samples enter the production probe: no recency sort, at most probe+1 rows.
            statement = f'SELECT i.id FROM {source} WHERE {condition} AND {predicate} LIMIT 20001'
        commands.extend([
            f"\\o '{args.output / (label+'.estimate.json')}'\nEXPLAIN(FORMAT JSON) {statement};\n\\o\n",
            f"\\o '{args.output / (label+'.plan.json')}'\nEXPLAIN(ANALYZE,BUFFERS,FORMAT JSON) {statement};\n\\o\n",
            f"\\o '{args.output / (label+'.result.json')}'\nSELECT jsonb_build_object('count',count(*),'ids',coalesce(jsonb_agg(id),'[]'::jsonb)) FROM ({statement}) selected;\n\\o\n",
        ])
        cases.append((label,count,ids,index,relations or ['items'],sampled))

    item_live = "i.search_text IS NOT NULL AND i.kind<>'marker' AND i.search_text NOT LIKE '%<orch>%' AND i.search_text ILIKE '%release%'"
    record('items-exact','items i','i.payload',item_live,'i.created_at DESC,i.id DESC','image/png',1,[1],'search_items_media_idx')
    record('items-family','items i','i.payload',item_live,'i.created_at DESC,i.id DESC','image/*',2,[1,5],'search_items_media_idx')
    record('items-common-sample','items i','i.payload',item_live,'i.created_at DESC,i.id DESC','text/*',100,sampled=True)
    record('items-any-sample','items i','i.payload',item_live,'i.created_at DESC,i.id DESC','*/*',100,sampled=True)
    record('items-rare-sample','items i','i.payload',item_live,'i.created_at DESC,i.id DESC','image/*',0,sampled=True)
    record('source-creator-empty','items i JOIN nodes n ON n.id=i.thread_id','i.payload',item_live+" AND n.created_by='00000000-0000-4000-8000-00000000ffff'::uuid",'i.created_at DESC,i.id DESC',None,0)
    chat_live = "i.deleted_at IS NULL AND i.search_text IS NOT NULL AND i.search_text ILIKE '%release%'"
    record('messages-family','chat_messages i','i.content',chat_live,'i.created_at DESC,i.id DESC','image/*',1,[1],'search_messages_media_idx',['chat_messages'])
    revision_live = "i.kind<>'marker' AND r.body->>'search_text' IS NOT NULL AND r.body->>'search_text' NOT LIKE '%<orch>%' AND r.body->>'search_text' ILIKE '%release%'"
    record('revisions-family','items i JOIN thread_item_revisions r ON r.item_id=i.id',"r.body->'payload'",revision_live,'i.created_at DESC,i.id DESC','image/*',2,[1,5],'search_revisions_media_idx',['items','thread_item_revisions'])
    record('revisions-common-sample','items i JOIN thread_item_revisions r ON r.item_id=i.id',"r.body->'payload'",revision_live,'i.created_at DESC,i.id DESC','text/*',100,relations=['items','thread_item_revisions'],sampled=True)
    commands.append(f"""
CREATE TEMP TABLE long_revisions AS SELECT md5('media-scale/{args.rows}')::uuid item_id,i::bigint seq_updated,
 jsonb_build_object('payload','{{"parts":[{{"type":"file","media_type":"text/plain"}}]}}'::jsonb,'search_text','release') body
 FROM generate_series(1,{args.rows})i;
CREATE UNIQUE INDEX media_long_revisions_item ON long_revisions(item_id,seq_updated DESC);
ANALYZE long_revisions;
""")
    long_source = revision_sample.replace('thread_item_revisions','long_revisions')
    record('long-item-revisions-sample',long_source,"r.body->'payload'",revision_live,'i.created_at DESC,i.id DESC','text/*',200,[args.rows],relations=['items','long_revisions'],sampled=True)
    commands.append(f"\\o '{args.output / 'indexes.json'}'\nSELECT jsonb_object_agg(indexrelname,pg_relation_size(indexrelid)) FROM pg_stat_user_indexes WHERE indexrelname IN ('search_items_media_idx','search_revisions_media_idx','search_messages_media_idx','search_revision_items_recent_idx');\n\\o\nROLLBACK;\n")
    with (args.output/'output.log').open('x') as log:
        result = subprocess.run(['psql',args.database_url,'-X','-Atq','-v','ON_ERROR_STOP=1'],input='\n'.join(commands),text=True,stdout=log,stderr=subprocess.STDOUT,timeout=1200)
    if result.returncode:
        raise SystemExit(result.returncode)
    report=[]
    identity=lambda number: hashlib.md5(f'media-scale/{number}'.encode()).hexdigest()
    for label,count,ids,index,relations,sampled in cases:
        found=json.loads((args.output/(label+'.result.json')).read_text())
        plan=json.loads((args.output/(label+'.plan.json')).read_text())[0]
        nodes=list(walk(plan['Plan']))
        scans=[node for node in nodes if node.get('Relation Name') in relations]
        assert found['count']==count,(label,found)
        if ids is not None:
            assert {value.replace('-','') for value in found['ids']}=={identity(number) for number in ids},(label,found)
        assert all(node['Node Type']!='Seq Scan' for node in scans),(label,scans)
        examined=sum((node.get('Actual Rows',0)+node.get('Rows Removed by Filter',0)+node.get('Rows Removed by Index Recheck',0))*node.get('Actual Loops',0) for node in scans)
        budget=200 if sampled else 2*max(count,1)
        assert examined<=budget*len(relations),(label,examined,scans)
        assert max(node.get('Actual Rows',0) for node in nodes)<=budget,(label,nodes)
        if index:
            assert any(node.get('Index Name')==index for node in nodes),(label,nodes)
        report.append({'case':label,'fixture_rows':args.rows,'returned_rows':count,'rows_examined':examined,'execution_ms':plan['Execution Time']})
    (args.output/'report.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report),flush=True)
    print('DONE bounded media plans',flush=True)


if __name__=='__main__':
    main()
