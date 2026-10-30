-- Valid retained identities share a busy machine; unrelated history grows while the closed thread stays small.
INSERT INTO users(id,username,display_name,avatar_color)
VALUES (md5('history-proof-person')::uuid,'history_proof','History proof',0);
INSERT INTO nodes(id,kind,org_id,path,name,owner_id,created_by)
SELECT id,'org',id,ARRAY[id],'History proof',person,person
FROM (SELECT md5('history-proof-org')::uuid id,md5('history-proof-person')::uuid person) seed;
INSERT INTO orgs(node_id,slug,org_kind)
VALUES (md5('history-proof-org')::uuid,'history-proof','team');
INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by)
SELECT md5('history-proof-project')::uuid,'project',id,id,ARRAY[id,md5('history-proof-project')::uuid],
       'History project',person,person
FROM (SELECT md5('history-proof-org')::uuid id,md5('history-proof-person')::uuid person) seed;
INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by)
SELECT md5('history-proof-topic')::uuid,'topic',md5('history-proof-project')::uuid,id,
       ARRAY[id,md5('history-proof-project')::uuid,md5('history-proof-topic')::uuid],
       'History topic',person,person
FROM (SELECT md5('history-proof-org')::uuid id,md5('history-proof-person')::uuid person) seed;
INSERT INTO topics(node_id) VALUES (md5('history-proof-topic')::uuid);
INSERT INTO machines(id,name,owner_id,org_id,home_id,placement,platform_os,platform_arch,
                     connector_version,protocol_version,cert_serial,cert_fingerprint,cert_not_after)
VALUES (md5('history-proof-machine')::uuid,'History machine',md5('history-proof-person')::uuid,
        md5('history-proof-org')::uuid,md5('history-proof-org')::uuid,'connected','linux','x86_64',
        'test',1,'history-proof',decode(repeat('01',32),'hex'),'2030-01-01');
INSERT INTO agents(id,name,owner_id,org_id,home_id,harness_kind)
VALUES (md5('history-proof-agent')::uuid,'History profile',md5('history-proof-person')::uuid,
        md5('history-proof-org')::uuid,md5('history-proof-org')::uuid,'scale-program');
INSERT INTO providers(id,name,owner_id,org_id,home_id,kind,arrangements,upstreams,protocols)
VALUES (md5('history-proof-provider')::uuid,'History provider',md5('history-proof-person')::uuid,
        md5('history-proof-org')::uuid,md5('history-proof-org')::uuid,'api',ARRAY['key_separated'],
        '[{"protocol":"openai-responses","base_url":"http://127.0.0.1:41800","options":{}}]'::jsonb,
        ARRAY['openai-responses']);
INSERT INTO environments(id,name,owner_id,org_id,home_node_id,placement,machine_id,generation)
VALUES (md5('history-proof-environment')::uuid,'History environment',md5('history-proof-person')::uuid,
        md5('history-proof-org')::uuid,md5('history-proof-org')::uuid,'connected',md5('history-proof-machine')::uuid,1);
INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by)
SELECT md5('history-proof-thread-'||g)::uuid,'thread',md5('history-proof-topic')::uuid,
       md5('history-proof-org')::uuid,
       ARRAY[md5('history-proof-org')::uuid,md5('history-proof-project')::uuid,
             md5('history-proof-topic')::uuid,md5('history-proof-thread-'||g)::uuid],
       'History thread '||g,md5('history-proof-person')::uuid,md5('history-proof-person')::uuid
FROM generate_series(0,999) g;
INSERT INTO threads(node_id,topic_id,agent_id,provider_id,environment_id,arrangement)
SELECT md5('history-proof-thread-'||g)::uuid,md5('history-proof-topic')::uuid,
       md5('history-proof-agent')::uuid,md5('history-proof-provider')::uuid,
       md5('history-proof-environment')::uuid,'key_separated'
FROM generate_series(0,999) g;
INSERT INTO segments(id,thread_id,ordinal,agent_id,provider_id,environment_id,machine_id,
                     env_generation,arrangement,credential_path,seq_updated,native_session_ref,
                     import_report,agent_history_bytes,agent_history_owned,agent_history_claim,
                     agent_history_claimed_at,agent_history_error,agent_history_measure_revision,
                     agent_history_measured_revision,started_by,started_at)
SELECT lpad(to_hex(g),32,'0')::uuid,
       md5('history-proof-thread-'||CASE WHEN g<=2 THEN 0 ELSE 1+(g-3)%999 END)::uuid,
       CASE WHEN g<=2 THEN g ELSE 1+(g-3)/999 END,
       md5('history-proof-agent')::uuid,md5('history-proof-provider')::uuid,
       md5('history-proof-environment')::uuid,md5('history-proof-machine')::uuid,
       1,'key_separated','forwarded',g,'history-'||g,
       jsonb_build_object('proof',g),4096+g,true,
       CASE WHEN g<=201 OR g%10=0 THEN md5('history-proof-claim-'||g)::uuid END,
       CASE WHEN g<=201 OR g%10=0 THEN '2026-10-01'::timestamptz END,
       CASE WHEN g<=200 THEN 'history.command_unconfirmed' END,g%7,g%4,
       md5('history-proof-person')::uuid,'2026-01-01'::timestamptz
FROM generate_series(1,:rows) g;
INSERT INTO commands(id,machine_id,kind,body,deadline_at)
SELECT md5('history-proof-command-'||g)::uuid,md5('history-proof-machine')::uuid,'session.close',
       jsonb_build_object('thread_id',md5('history-proof-thread-0')::uuid,
                         'segment_id',lpad(to_hex(g),32,'0')::uuid),
       '2030-01-01'::timestamptz
FROM generate_series(1,2) g;
