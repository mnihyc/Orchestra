#!/usr/bin/env python3
"""Assert bounded search-file lookups using production SQL and temporary native-shaped metadata."""
import argparse
import json
import re
import subprocess
from pathlib import Path
from urllib.parse import urlsplit


def literal(value):
    return 'NULL' if value is None else "'" + value.replace("'", "''") + "'"


def walk(plan):
    yield plan
    for child in plan.get('Plans', []):
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
        parser.error('use an isolated loopback orchprobe_search_ database, never the shared test database')
    args.output.mkdir(parents=True, exist_ok=False)
    command = ['psql', args.database_url, '-X', '-Atq', '-v', 'ON_ERROR_STOP=1']
    tree = Path(__file__).resolve().parents[2]
    hydration = (tree / 'crates/control/db/src/queries/search_files.sql').read_text()
    media = (tree / 'crates/control/db/src/queries/search_file_type.sql').read_text()
    facts = (tree / 'crates/control/db/src/queries/search_source_facts.sql').read_text()
    migration = (tree / 'migrations/0040_search_file_types.sql').read_text()
    new_indexes = set(re.findall(r'CREATE INDEX (\w+)', migration))
    catalog = subprocess.run([*command, '-c', "SET statement_timeout='25s'; SELECT coalesce(jsonb_agg(jsonb_build_object('table',t.relname,'name',i.relname,'sql',pg_get_indexdef(i.oid))),'[]'::jsonb) FROM pg_index x JOIN pg_class t ON t.oid=x.indrelid JOIN pg_class i ON i.oid=x.indexrelid WHERE t.oid IN ('public.documents'::regclass,'public.document_files'::regclass)"], text=True, capture_output=True, check=True, timeout=30)
    baseline = []
    for index in json.loads(catalog.stdout):
        if index['name'] not in new_indexes:
            statement = index['sql'].replace(' ON public.' + index['table'] + ' ', ' ON ' + index['table'] + ' ')
            baseline.append(statement + ';')
    owner = '00000000-0000-4000-8000-000000000001'
    # One hot document has nearly every file. Another 999 documents exercise a page of distinct sources.
    hot = args.rows - 999
    filename = lambda row: f'file-{row:07d}'
    doc = lambda number: f"md5('document/{number}')::uuid"
    commands = [f"""
BEGIN;
SET statement_timeout='10min';
SET max_parallel_maintenance_workers=0;
CREATE TEMP TABLE documents (LIKE public.documents INCLUDING DEFAULTS INCLUDING CONSTRAINTS);
CREATE TEMP TABLE document_files (LIKE public.document_files INCLUDING DEFAULTS INCLUDING CONSTRAINTS);
{' '.join(baseline)}
INSERT INTO documents(id,node_id,org_id,owner_id,created_by,updated_by,title,basename,created_at,updated_at)
SELECT md5('document/'||i)::uuid,'00000000-0000-4000-8000-000000000002'::uuid,
 '00000000-0000-4000-8000-000000000002'::uuid,'{owner}'::uuid,'{owner}'::uuid,'{owner}'::uuid,
 CASE WHEN i%2=0 THEN 'Release notes ' ELSE '发布记录 ' END||i,'document-'||i||'.md',
 timestamptz '2026-01-01'+(i*1000000.0/{args.rows})*interval '1 second',
 timestamptz '2026-01-01'+(i*1000000.0/{args.rows})*interval '1 second'
 FROM generate_series(1,{args.rows})i;
CREATE TEMP TABLE document_blocks(id uuid PRIMARY KEY,document_id uuid,updated_at timestamptz,removed_at timestamptz);
CREATE INDEX filter_blocks_scope ON document_blocks(document_id,updated_at DESC,id DESC);
INSERT INTO document_blocks SELECT md5('block/'||id)::uuid,id,updated_at,NULL FROM documents;
CREATE TEMP TABLE items (LIKE public.items INCLUDING DEFAULTS INCLUDING CONSTRAINTS);
CREATE TEMP TABLE chat_messages (LIKE public.chat_messages INCLUDING DEFAULTS INCLUDING CONSTRAINTS);
CREATE TEMP TABLE thread_item_revisions (LIKE public.thread_item_revisions INCLUDING DEFAULTS INCLUDING CONSTRAINTS);
INSERT INTO document_files(id,document_id,blob,name,media_type,size,is_text,added_at)
SELECT md5('file-scale/'||i)::uuid,
 md5('document/'||CASE WHEN i<={hot} THEN 1 ELSE i-{hot}+1 END)::uuid,
 decode(repeat('ab',32),'hex'),'file-'||lpad(i::text,7,'0'),
 CASE i WHEN {hot - 1} THEN 'application/pdf' WHEN {hot} THEN 'image/png' WHEN {args.rows} THEN 'image' ELSE 'text/plain' END,
 1,true,timestamptz '2026-01-01' FROM generate_series(1,{args.rows})i;
{migration.replace('CREATE INDEX ', 'CREATE INDEX IF NOT EXISTS ').replace('CREATE FUNCTION ', 'CREATE OR REPLACE FUNCTION ').replace('ADD COLUMN search_file_count', 'ADD COLUMN IF NOT EXISTS search_file_count')}
ANALYZE documents;ANALYZE document_blocks;ANALYZE document_files;
SET statement_timeout='30s';
DO $size$ BEGIN
 IF (SELECT count(*) FROM documents)<>{args.rows} OR (SELECT count(*) FROM document_files)<>{args.rows}
 THEN RAISE EXCEPTION 'fixture row count'; END IF;
END $size$;
"""]
    cases = []

    def record(label, statement, count, names=None, types=None, requests=1, index=None, relation='document_files'):
        plan = args.output / f'{label}.plan.json'
        result = args.output / f'{label}.result.json'
        commands.extend([
            f"\\o '{plan}'\nEXPLAIN(ANALYZE,BUFFERS,FORMAT JSON) {statement};\n\\o\n",
            f"\\o '{result}'\nSELECT jsonb_build_object('count',count(*),'names',coalesce(jsonb_agg(name ORDER BY name),'[]'::jsonb),'types',coalesce(jsonb_agg(media_type ORDER BY name),'[]'::jsonb)) FROM ({statement}) selected;\n\\o\n",
        ])
        cases.append((label, count, names, types, requests, index, relation))

    def select(label, count, names, kind, focused, documents=1, picked=None):
        if kind is None:
            predicate, value = '$4::text IS NULL', None
        elif kind.endswith('/*'):
            predicate, value = "position('/' in f.media_type)>0 AND split_part(lower(f.media_type),'/',1)=$4", kind[:-2].lower()
        else:
            predicate, value = 'lower(f.media_type)=$4', kind.lower()
        ids = f"ARRAY(SELECT md5('document/'||i)::uuid FROM generate_series(1,{documents})i)"
        titles = f"ARRAY(SELECT {literal(picked or 'the document title')} FROM generate_series(1,{documents}))"
        statement = hydration.replace('__FILE_TYPE__', predicate)
        for key, replacement in [('$1', ids), ('$2', titles), ('$3', str(focused).lower()), ('$4', literal(value))]:
            statement = statement.replace(key, replacement)
        record(label, statement, count, names=names, requests=documents)

    select('one-any', 1, [filename(1)], None, True)
    select('one-pdf', 1, [filename(hot - 1)], 'application/pdf', True)
    select('one-family', 1, [filename(hot)], 'image/*', True)
    select('named', 1, [filename(hot // 2)], None, False, picked=filename(hot // 2))
    select('missing-name', 0, [], None, False)
    select('wrong-type', 0, [], 'image/*', False, picked=filename(hot - 1))
    select('page-any', 200, [filename(1)] + [filename(hot + i) for i in range(1, 200)], None, True, documents=200)
    for label, kind, expected, index in [
        ('predicate-pdf', 'application/pdf', 'application/pdf', 'search_files_media_name_idx'),
        ('predicate-family', 'image/*', 'image/png', 'search_files_family_name_idx'),
    ]:
        predicate = media.replace('__SELECTION__', f"({literal(json.dumps({'file_type': kind}))}::jsonb)").replace('__MEDIA_TYPE__', 'f.media_type')
        record(label, f'SELECT f.name,f.media_type FROM document_files f WHERE f.document_id={doc(1)} AND {predicate} ORDER BY f.name LIMIT 1', 1, types=[expected], index=index)
    malformed = media.replace('__SELECTION__', f"({literal(json.dumps({'file_type':'image/*'}))}::jsonb)").replace('__MEDIA_TYPE__','f.media_type')
    record('predicate-malformed', f'SELECT f.name,f.media_type FROM document_files f WHERE f.document_id={doc(1000)} AND {malformed} ORDER BY f.name LIMIT 1', 0, names=[])
    # Exercise the same native document facts predicate used before the lexical candidate limit.
    selected = literal(json.dumps({'owner_id':owner,'created_by_id':owner,'edited_by_id':owner,
                                  'date_field':'edited','from':'2026-01-03T00:00:00Z','to':'2026-01-10T00:00:00Z'})) + '::jsonb'
    predicate = facts
    for key, value in [('__SELECTION__',f'({selected})'),('__OWNER__','d.owner_id'),('__CREATOR__','d.created_by'),('__EDITOR__','d.updated_by'),('__CREATED__','d.created_at'),('__EDITED__','d.updated_at')]:
        predicate = predicate.replace(key, value)
    record('owner-edited-facts', f"SELECT d.title name,NULL::text media_type FROM documents d WHERE d.deleted_at IS NULL AND d.title ILIKE '%release%' AND {predicate} ORDER BY d.updated_at DESC,d.id DESC LIMIT 50", 50, requests=50, relation='documents')
    record('global-file-presence', "SELECT d.title name,NULL::text media_type FROM documents d WHERE d.deleted_at IS NULL AND d.search_file_count>0 AND d.title ILIKE '%release%' ORDER BY d.updated_at DESC,d.id DESC LIMIT 50", 50, requests=50, index='search_documents_files_recent_idx', relation='documents')
    for label, selection, index in [
        ('creator-empty', {'created_by_id':'00000000-0000-4000-8000-00000000ffff'}, 'search_documents_creator_recent_idx'),
        ('editor-empty', {'edited_by_id':'00000000-0000-4000-8000-00000000ffff'}, 'search_documents_editor_recent_idx'),
        ('created-empty', {'date_field':'created','to':'2025-01-01T00:00:00Z'}, 'search_documents_created_idx'),
    ]:
        predicate = facts
        for key, value in [('__SELECTION__',f'({literal(json.dumps(selection))}::jsonb)'),('__OWNER__','d.owner_id'),('__CREATOR__','d.created_by'),('__EDITOR__','d.updated_by'),('__CREATED__','d.created_at'),('__EDITED__','d.updated_at')]:
            predicate = predicate.replace(key, value)
        record(label, f'SELECT d.title name,NULL::text media_type FROM documents d WHERE d.deleted_at IS NULL AND {predicate} ORDER BY d.updated_at DESC,d.id DESC LIMIT 50', 0, names=[], index=index, relation='documents')
        if label == 'creator-empty':
            record('block-creator-empty', f'SELECT b.id::text name,NULL::text media_type FROM document_blocks b JOIN documents d ON d.id=b.document_id WHERE b.removed_at IS NULL AND d.deleted_at IS NULL AND {predicate} ORDER BY b.updated_at DESC,b.id DESC LIMIT 50', 0, names=[], index=index, relation='documents')
    commands.append(f"""
DO $check$ BEGIN
 IF (SELECT search_file_count FROM documents WHERE id={doc(1)})<>{hot} THEN RAISE EXCEPTION 'initial file count'; END IF;
END $check$;
UPDATE document_files SET document_id={doc(2)} WHERE id=md5('file-scale/1')::uuid;
DO $check$ BEGIN
 IF (SELECT search_file_count FROM documents WHERE id={doc(1)})<>{hot-1} OR
    (SELECT search_file_count FROM documents WHERE id={doc(2)})<>2 THEN RAISE EXCEPTION 'moved file count'; END IF;
END $check$;
DELETE FROM document_files WHERE id=md5('file-scale/1')::uuid;
INSERT INTO document_files(id,document_id,blob,name,media_type,size,is_text)
 VALUES(md5('added-scale-file')::uuid,{doc(1001)},decode(repeat('ab',32),'hex'),'new.txt','text/plain',1,true);
UPDATE document_files SET name='renamed.txt' WHERE id=md5('added-scale-file')::uuid;
DO $check$ BEGIN
 IF (SELECT search_file_count FROM documents WHERE id={doc(2)})<>1 OR
    (SELECT search_file_count FROM documents WHERE id={doc(1001)})<>1 THEN RAISE EXCEPTION 'added/removed file count'; END IF;
END $check$;
DELETE FROM document_files WHERE id=md5('added-scale-file')::uuid;
DO $check$ BEGIN
 IF (SELECT search_file_count FROM documents WHERE id={doc(1001)})<>0 THEN RAISE EXCEPTION 'last file count'; END IF;
END $check$;
\\o '{args.output / 'indexes.json'}'
SELECT jsonb_object_agg(indexrelname,pg_relation_size(indexrelid)) FROM pg_stat_user_indexes WHERE relid IN ('document_files'::regclass,'documents'::regclass);
\\o
ROLLBACK;
""")
    with (args.output / 'output.log').open('x') as log:
        result = subprocess.run(command,
                                input='\n'.join(commands), text=True, stdout=log, stderr=subprocess.STDOUT, timeout=900)
    if result.returncode:
        raise SystemExit(result.returncode)
    report = []
    for label, count, names, types, requests, index, relation in cases:
        result = json.loads((args.output / f'{label}.result.json').read_text())
        plan = json.loads((args.output / f'{label}.plan.json').read_text())[0]
        nodes = list(walk(plan['Plan']))
        scans = [node for node in nodes if node.get('Relation Name') == relation]
        examined = sum((node.get('Actual Rows', 0) + node.get('Rows Removed by Filter', 0)
                        + node.get('Rows Removed by Index Recheck', 0)) * node.get('Actual Loops', 0) for node in scans)
        assert result['count'] == count, (label, result)
        if names is not None:
            assert result['names'] == sorted(names), (label, result)
        if types is not None:
            assert result['types'] == types, (label, result)
        assert count <= requests, (label, count, requests)
        assert all(node['Node Type'] != 'Seq Scan' for node in scans), (label, scans)
        assert examined <= 2 * requests, (label, examined, scans)
        assert max(node.get('Actual Rows', 0) for node in nodes) <= max(requests, 10), (label, nodes)
        if index:
            assert any(node.get('Index Name') == index for node in scans), (label, scans)
        report.append({'case': label, 'fixture_rows': args.rows, 'relation':relation,'requested_documents': requests,
                       'returned_rows': count, 'rows_examined': examined, 'execution_ms': plan['Execution Time']})
    (args.output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report), flush=True)
    print('DONE bounded file plans', flush=True)


if __name__ == '__main__':
    main()
