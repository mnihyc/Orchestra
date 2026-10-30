"""Assert related-query row and index bounds on an isolated million-part synthetic world; never build Rust."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import uuid


def identity(label):
    return str(uuid.UUID(hashlib.md5(('search-probe/' + label).encode()).hexdigest()))


def literal(value):
    return "'" + str(value).replace("'", "''") + "'"


def constant(source, name):
    found = re.search(r'pub const ' + name + r': &str = r#"(.*?)"#;', source, re.S)
    if found is None:
        raise ValueError('Missing production SQL constant: ' + name)
    return found.group(1)


def nodes(value):
    yield value
    for child in value.get('Plans', []):
        yield from nodes(child)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--container', required=True)
    parser.add_argument('--database', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--rows', type=int, choices=[100_000, 1_000_000], default=1_000_000)
    parser.add_argument('--candidates', type=int, default=200, help='the configured reference/candidate window')
    args = parser.parse_args()
    if not args.container.startswith('orchprobe-related-'):
        parser.error('use an owned disposable probe container')
    if not args.database.startswith('orchprobe_search_') or args.candidates <= 0:
        parser.error('use the isolated search corpus and a positive configured window')
    root = Path(__file__).resolve().parents[2]
    source = (root / 'crates/control/db/src/queries/related.rs').read_text()
    shares = (root / 'crates/control/db/src/queries/search.rs').read_text()
    members = (root / 'crates/control/db/src/queries/chats.rs').read_text()
    candidates = constant(source, 'REFERENCE_CANDIDATES_SQL')
    scores = constant(source, 'REFERENCE_SCORES_SQL')
    filenames = constant(source, 'SOURCE_DOCUMENTS_SQL').replace('__PARENT__', 'parent_container_id IS NULL AND $2::uuid IS NULL')
    entrances = re.search(r'pub async fn agent_entrances.*?r#"(.*?)"#', source, re.S).group(1)
    roster = re.search(r'pub async fn members_for_label.*?r#"(.*?)"#', members, re.S).group(1)
    access = constant(shares, 'CANDIDATE_ACCESS')
    for before, after in [('__ACCESS__', '$3'), ('__PATH__', 'n.path'), ('__OWNER__', 'n.owner_id'),
                          ('__KIND__', 'entry.kind'), ('__SOURCE__', 'entry.source_id')]:
        access = access.replace(before, after)
    block=source.split('fn access_for(',1)[1].split('/// Reads only selected',1)[0]
    formats=re.findall(r'format!\(\s*"(.*?)"\s*\)',block,re.S)
    holder=next(value for value in formats if value.startswith("e.holder_kind")).format(readable='$3')
    area=next(value for value in formats if value.startswith("e.permission")).format(kind='entry.kind')
    actual=next(value for value in formats if value.startswith('(({ordinary})')).format(
        ordinary=access,readable='$3',kind='entry.kind',source='entry.source_id',holder=holder,area=area)
    place=constant(source,'PLACE_CANDIDATES_SQL').replace('__FOLDER__','$4::uuid IS NULL').replace('__DOCUMENTS__',"ARRAY(SELECT jsonb_array_elements_text($3->'documents')::uuid)").replace('__CHATS__',"ARRAY(SELECT jsonb_array_elements_text($3->'chats')::uuid)").replace('__ACCESS__',actual)
    meaning = re.search(r'pub async fn meaning_scores.*?sqlx::query_as\(\s*"(.*?)",', source, re.S).group(1)
    source_id, peer, target = [identity('document/' + str(number)) for number in [1, 2, 3]]
    generation, org, topic = [identity(label) for label in ['generation', 'root', 'topic/0']]
    path = literal(json.dumps({'scopes': [org], 'doc_scopes': [org], 'thread_scopes': [org],
                               'chat_scopes': [org], 'documents': [], 'chats': [], 'groups': []}))
    budget, count = args.candidates, args.rows
    program = ["BEGIN;SET LOCAL statement_timeout='10min';SET LOCAL work_mem='32MB';SET LOCAL jit=off;",
               f"DO $$ BEGIN IF (SELECT count(*) FROM documents)<{count} THEN RAISE EXCEPTION 'native corpus is too small'; END IF; END $$;",
               '''CREATE TEMP TABLE scale_world AS SELECT
                 CASE WHEN n<=10000 THEN 0 ELSE 1+(n-10001)%999 END thread,
                 count(*) items,count(*)*10000 tokens FROM generate_series(1,''' + str(count) + ''') n
                 GROUP BY 1;''',
               '''CREATE FUNCTION pg_temp.capture(label text, statement text, bound bigint) RETURNS void LANGUAGE plpgsql AS $$
                 DECLARE plan jsonb;
                 BEGIN EXECUTE 'EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) '||statement INTO plan;
                 RAISE NOTICE 'ORCH_RELATED_BOUNDS %',jsonb_build_object('label',label,'bound',bound,'plan',plan);
                 END $$;''',
               f'PREPARE related_names(uuid,uuid,text,text) AS {filenames};',
               f'PREPARE related_place(uuid,bigint,jsonb,uuid,uuid) AS {place};',
               f'PREPARE related_meaning(uuid,text,uuid,bigint,uuid[]) AS {meaning};',
               f"SELECT pg_temp.capture('place-root', {literal(f'EXECUTE related_place({literal(org)},{budget},{path},NULL,{literal(source_id)})')},{budget});",
               f"SELECT pg_temp.capture('place-topic', {literal(f'EXECUTE related_place({literal(topic)},{budget},{path},NULL,{literal(source_id)})')},{budget});",
               f"SELECT pg_temp.capture('filename', {literal(f'EXECUTE related_names({literal(topic)},NULL,\'absent.md\',\'absent\')')},2);",
               f"SELECT pg_temp.capture('meaning-score', {literal(f'EXECUTE related_meaning({literal(generation)},\'document\',{literal(source_id)},1,ARRAY[{literal(peer)}::uuid])')},1);",
               'CREATE TEMP TABLE related_references (LIKE public.related_references INCLUDING ALL);',
               'CREATE TEMP TABLE related_reference_targets (LIKE public.related_reference_targets INCLUDING ALL);',
               '''CREATE TRIGGER target_added AFTER INSERT ON related_references REFERENCING NEW TABLE AS added
                 FOR EACH STATEMENT EXECUTE FUNCTION orch_related_reference_targets_added();
                 CREATE TRIGGER target_removed AFTER DELETE ON related_references REFERENCING OLD TABLE AS removed
                 FOR EACH STATEMENT EXECUTE FUNCTION orch_related_reference_targets_removed();
                 CREATE TRIGGER target_updated_1_removed AFTER UPDATE ON related_references REFERENCING OLD TABLE AS removed
                 FOR EACH STATEMENT EXECUTE FUNCTION orch_related_reference_targets_removed();
                 CREATE TRIGGER target_updated_2_added AFTER UPDATE ON related_references REFERENCING NEW TABLE AS added
                 FOR EACH STATEMENT EXECUTE FUNCTION orch_related_reference_targets_added();''',
               f'PREPARE related_neighbors(text,uuid,bigint) AS {candidates};',
               f'PREPARE related_scores(text,uuid,uuid[],bigint) AS {scores};']
    for phase in ['duplicates', 'distinct', 'popular']:
        program.append('SET LOCAL statement_timeout=\'10min\';TRUNCATE related_references;TRUNCATE related_reference_targets;')
        from_id = "search_probe_id('document/'||n)" if phase == 'popular' else literal(source_id) + '::uuid'
        to_id = "search_probe_id('document/'||n)" if phase == 'distinct' else literal(target) + '::uuid'
        program.append(f'''INSERT INTO related_references SELECT 'document',{from_id},search_probe_id('bounds/part/'||n),
                    'document',{to_id},''::bytea FROM generate_series(1,{count}) n;''')
        expected = 1 if phase == 'duplicates' else count
        program.append(f'''DO $$ BEGIN
          IF (SELECT count(*) FROM related_reference_targets)<>{expected}
             OR (SELECT sum(links) FROM related_reference_targets)<>{count}
          THEN RAISE EXCEPTION 'distinct target projection differs from native parts'; END IF; END $$;''')
        if phase == 'duplicates':
            program.append(f'''DELETE FROM related_references WHERE part_id=search_probe_id('bounds/part/1');
              DO $$ BEGIN IF (SELECT links FROM related_reference_targets)<>{count - 1}
                THEN RAISE EXCEPTION 'deleting one repeat lost another part'; END IF; END $$;
              UPDATE related_references SET target_id=search_probe_id('document/4') WHERE part_id=search_probe_id('bounds/part/2');
              DO $$ BEGIN IF (SELECT count(*) FROM related_reference_targets)<>2 OR
                 (SELECT sum(links) FROM related_reference_targets)<>{count - 1}
                THEN RAISE EXCEPTION 'replacement target counters are not atomic'; END IF; END $$;''')
        program.append('ANALYZE related_references;ANALYZE related_reference_targets;SET LOCAL statement_timeout=\'20s\';')
        for label, statement, bound in [
            (phase + '-neighbors', f"EXECUTE related_neighbors('document',{literal(source_id)},{budget})", budget),
            (phase + '-score', f"EXECUTE related_scores('document',{literal(source_id)},ARRAY[{literal(peer)}::uuid],{budget})", 1),
        ]:
            program.append(f'SELECT pg_temp.capture({literal(label)},{literal(statement)},{bound});')
    program.extend(['''CREATE TEMP TABLE permission_entries(holder_kind text,holder_id uuid,object_kind text,
       object_id uuid,permission text,removed_at timestamptz,expires_at timestamptz);
       CREATE INDEX ON permission_entries(holder_id,expires_at,object_kind,object_id,permission)
         WHERE holder_kind='agents' AND removed_at IS NULL;''',
        f'''SET LOCAL statement_timeout='10min';INSERT INTO permission_entries SELECT 'agents','{org}',
          'document',search_probe_id('document/'||n),'doc.view',NULL,NULL FROM generate_series(1,{count}) n;
          ANALYZE permission_entries;SET LOCAL statement_timeout='20s';''',
        f'PREPARE related_grants(uuid[],timestamptz,bigint) AS {entrances};',
        f"SELECT pg_temp.capture('agent-grants',{literal(f'EXECUTE related_grants(ARRAY[{literal(org)}::uuid],\'2026-10-29\',{budget + 1})')},{budget + 1});",
        '''CREATE TEMP TABLE chat_members(chat_id uuid,user_id uuid,added_by uuid,joined_at timestamptz,
          left_at timestamptz,guest boolean,seq_updated bigint,expires_at timestamptz);
          CREATE INDEX ON chat_members(chat_id,expires_at,user_id) WHERE left_at IS NULL;''',
        f'''SET LOCAL statement_timeout='10min';INSERT INTO chat_members SELECT '{org}',search_probe_id('member/'||n),
          NULL,'2020-01-01',NULL,false,0,'2020-02-01' FROM generate_series(1,{count}) n;
          INSERT INTO chat_members SELECT '{org}',search_probe_id('active/'||n),NULL,'2026-01-01',NULL,false,0,
            CASE WHEN n=3 THEN timestamptz '2027-01-01' ELSE NULL END FROM generate_series(1,3)n;
          ANALYZE chat_members;SET LOCAL statement_timeout='20s';''',
        f'PREPARE related_roster(uuid,uuid,timestamptz,bigint) AS {roster};',
        f"SELECT pg_temp.capture('chat-label',{literal(f'EXECUTE related_roster({literal(org)},{literal(source_id)},\'2026-10-29\',3)')},3);",
        '''DO $$ BEGIN RAISE NOTICE 'ORCH_RELATED_WORLD %',(SELECT jsonb_build_object('threads',count(*),
          'parts',sum(items),'tokens',sum(tokens),'largest_thread_tokens',max(tokens)) FROM scale_world);END $$;ROLLBACK;'''])
    args.output.mkdir(parents=True, exist_ok=True)
    sql = '\n'.join(program)
    (args.output / 'program.sql').write_text(sql)
    result = subprocess.run(['docker', 'exec', '-i', args.container, 'psql', '-X', '-qAt', '-U', 'orch',
                             '-d', args.database, '-v', 'ON_ERROR_STOP=1'], input=sql, text=True,
                            capture_output=True, timeout=1500)
    (args.output / 'native.log').write_text(result.stdout + result.stderr)
    plans, failures, world = [], [], None
    for line in result.stderr.splitlines():
        if 'ORCH_RELATED_BOUNDS ' in line:
            row = json.loads(line.split('ORCH_RELATED_BOUNDS ', 1)[1])
            plan = row['plan'][0]['Plan']
            if plan['Actual Rows'] > row['bound']:
                failures.append(row['label'] + ': page bound exceeded')
            for node in nodes(plan):
                relation = node.get('Relation Name', '')
                if relation in ['items', 'chat_messages', 'document_blocks', 'related_references']:
                    failures.append(row['label'] + ': interactive read reached history/parts: ' + relation)
                large = relation in ['documents', 'search_item_vectors']
                large |= relation == 'permission_entries' and row['label'] == 'agent-grants'
                large |= relation == 'chat_members' and row['label'] == 'chat-label'
                large |= relation == 'related_reference_targets' and not row['label'].startswith('duplicates')
                if large and node.get('Actual Loops',0)>0 and 'Seq Scan' in node['Node Type']:
                    failures.append(row['label'] + ': sequential scan of ' + relation)
                if large and node['Actual Rows'] > budget * budget:
                    failures.append(row['label'] + ': an indexed window retained too many rows')
            plans.append(row)
        if 'ORCH_RELATED_WORLD ' in line:
            world = json.loads(line.split('ORCH_RELATED_WORLD ', 1)[1])
    if result.returncode or len(plans) != 12 or world is None:
        failures.append('native proof did not finish all twelve plans and the world receipt')
    if world is not None and (world['threads'] != 1000 or world['parts'] != count or world['tokens'] != count * 10000):
        failures.append('synthetic world cardinalities differ')
    report = {'source_sql_sha256': hashlib.sha256(source.encode()).hexdigest(), 'rows': count,
              'candidates': budget, 'world': world, 'plans': plans, 'failures': failures,
              'profile': 'native SQL; no Rust build', 'passed': not failures}
    (args.output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'passed': not failures, 'plans': len(plans), 'world': world, 'failures': failures}))
    return 1 if failures else 0


if __name__ == '__main__':
    raise SystemExit(main())
