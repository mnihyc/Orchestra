-- A growing installation with a fixed reader's three memberships and fixed page sizes.
-- Set-based writes retain all production triggers; no cache or access projection is fabricated.
CREATE FUNCTION pg_temp.page_person(n bigint) RETURNS uuid LANGUAGE sql IMMUTABLE AS $$
 SELECT CASE WHEN n=0 THEN current_setting('orch.fixture_reader')::uuid ELSE md5('page-person/'||n)::uuid END
$$;
CREATE FUNCTION pg_temp.page_id(kind text,n bigint) RETURNS uuid LANGUAGE sql IMMUTABLE AS $$
 SELECT md5('page-'||kind||'/'||n)::uuid
$$;
CREATE FUNCTION pg_temp.page_size(n integer) RETURNS integer LANGUAGE sql STABLE AS $$
 SELECT n*current_setting('orch.fixture_scale')::integer
$$;

INSERT INTO users(id,username,display_name,avatar_color)
 SELECT pg_temp.page_person(n),'page-person-'||n,'Page person '||n,0
 FROM generate_series(1,pg_temp.page_size(20)-1) n;
INSERT INTO nodes(id,kind,org_id,path,name,owner_id,created_by)
 SELECT id,'org',id,ARRAY[id],'Scale org '||n,pg_temp.page_person(1),pg_temp.page_person(1)
 FROM (SELECT n,pg_temp.page_id('org',n) id FROM generate_series(1,pg_temp.page_size(10)) n) made;
INSERT INTO orgs(node_id,slug,org_kind)
 SELECT pg_temp.page_id('org',n),'scale-org-'||n,'team' FROM generate_series(1,pg_temp.page_size(10)) n;
INSERT INTO org_members(org_id,user_id,domain_id)
 SELECT pg_temp.page_id('org',1+(n+offsets)%pg_temp.page_size(10)),pg_temp.page_person(n),
        pg_temp.page_id('org',1+(n+offsets)%pg_temp.page_size(10))
 FROM generate_series(0,pg_temp.page_size(20)-1) n CROSS JOIN generate_series(0,2) offsets;
INSERT INTO node_roles(id,node_id,subject_kind,subject_id,role,created_by)
 SELECT gen_random_uuid(),org_id,'user',user_id,'developer',pg_temp.page_person(1)
 FROM org_members WHERE user_id IN (SELECT pg_temp.page_person(n) FROM generate_series(0,pg_temp.page_size(20)-1) n)
 AND org_id IN (SELECT pg_temp.page_id('org',n) FROM generate_series(1,pg_temp.page_size(10)) n);
INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by)
 SELECT id,'project',root,root,ARRAY[root,id],'Scale project '||n,pg_temp.page_person(1),pg_temp.page_person(1)
 FROM (SELECT n,pg_temp.page_id('project',n) id,pg_temp.page_id('org',1+(n-1)/10) root
       FROM generate_series(1,pg_temp.page_size(100)) n) made;
INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by)
 SELECT pg_temp.page_id('topic',n),'topic',p.id,p.org_id,p.path||pg_temp.page_id('topic',n),
        'Scale topic '||n,pg_temp.page_person(1),pg_temp.page_person(1)
 FROM generate_series(1,pg_temp.page_size(100)) n JOIN nodes p ON p.id=pg_temp.page_id('project',n);
INSERT INTO topics(node_id) SELECT pg_temp.page_id('topic',n) FROM generate_series(1,pg_temp.page_size(100)) n;
INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by)
 SELECT pg_temp.page_id('thread',n),'thread',p.id,p.org_id,p.path||pg_temp.page_id('thread',n),
        'Scale thread '||n,pg_temp.page_person(1),pg_temp.page_person(1)
 FROM generate_series(1,pg_temp.page_size(100)) n JOIN nodes p ON p.id=pg_temp.page_id('topic',n);
INSERT INTO threads(node_id,topic_id,next_seq,seq_state)
 SELECT pg_temp.page_id('thread',n),pg_temp.page_id('topic',n),201,200 FROM generate_series(1,pg_temp.page_size(100)) n;
-- Imported history requires no executor or model, and retains real segments and revision pins.
INSERT INTO segments(id,thread_id,ordinal,env_generation,arrangement,credential_path,ended_at,import_fidelity,seq_updated)
 SELECT pg_temp.page_id('segment',n),pg_temp.page_id('thread',n),1,0,'together','native',now(),'structured',200
 FROM generate_series(1,pg_temp.page_size(100)) n;
INSERT INTO items(id,thread_id,segment_id,kind,status,author_id,payload,search_text,seq_created,seq_updated)
 SELECT pg_temp.page_id('item',(n-1)*200+part),pg_temp.page_id('thread',n),pg_temp.page_id('segment',n),
        'agent_message','completed',pg_temp.page_person(1),
        jsonb_build_object('text','Scale fixture message','text_len',21),'Scale fixture message',part,part
 FROM generate_series(1,pg_temp.page_size(100)) n CROSS JOIN generate_series(1,200) part;
INSERT INTO providers(id,name,owner_id,org_id,home_id,kind,arrangements,upstreams)
 SELECT pg_temp.page_id('provider',n),'Scale provider '||n,pg_temp.page_person(1),
        pg_temp.page_id('org',n),pg_temp.page_id('org',n),'api',ARRAY['separated'],
        '[{"protocol":"openai-responses","base_url":"https://models.example.test/v1","options":{}}]'::jsonb
 FROM generate_series(1,pg_temp.page_size(10)) n;
INSERT INTO agents(id,name,owner_id,org_id,home_id,harness_kind)
 SELECT pg_temp.page_id('agent',n),'Scale agent '||n,pg_temp.page_person(1),
        pg_temp.page_id('org',n),pg_temp.page_id('org',n),'fake'
 FROM generate_series(1,pg_temp.page_size(10)) n;
INSERT INTO documents(id,node_id,org_id,owner_id,created_by,title,version)
 SELECT pg_temp.page_id('document',n),p.id,p.org_id,pg_temp.page_person(1),pg_temp.page_person(1),'Scale document '||n,2
 FROM generate_series(1,pg_temp.page_size(1000)) n JOIN nodes p ON p.id=pg_temp.page_id('project',1+(n-1)/10);
INSERT INTO document_versions(document_id,version,title,whole,stored_bytes,whole_bytes,bytes,cause,from_at,to_at,block_count,top_blocks)
 SELECT id,v,title,'{"blocks":[]}'::jsonb,13,13,0,'rewrite',created_at,created_at,0,0
 FROM documents CROSS JOIN generate_series(1,2) v;
INSERT INTO document_discussions(id,document_id,created_by,seq_updated,last_activity_at)
 SELECT pg_temp.page_id('discussion',n),pg_temp.page_id('document',n),pg_temp.page_person(1),0,now()
 FROM generate_series(1,pg_temp.page_size(1000)) n WHERE n%4=1;
INSERT INTO document_comments(id,discussion_id,document_id,author_id,content,seq_updated)
 SELECT gen_random_uuid(),id,document_id,pg_temp.page_person(1),'[{"type":"text","text":"Scale reply"}]'::jsonb,0
 FROM document_discussions CROSS JOIN generate_series(1,2) reply;
INSERT INTO chats(id,kind,node_id,org_id,created_by)
 SELECT gen_random_uuid(),'node',id,org_id,pg_temp.page_person(1) FROM nodes n
 WHERE NOT EXISTS(SELECT 1 FROM chats c WHERE c.node_id=n.id);
INSERT INTO permission_entries(id,object_kind,object_id,page,profile,permission,holder_kind,holder_id,created_by)
 SELECT gen_random_uuid(),kind,id,'live','any','access.own','user',pg_temp.page_person(1),pg_temp.page_person(1)
 FROM (SELECT 'node' kind,id FROM nodes WHERE id<>current_setting('orch.fixture_personal')::uuid
       UNION ALL SELECT 'document',id FROM documents
       UNION ALL SELECT 'provider',id FROM providers
       UNION ALL SELECT 'agent',id FROM agents) objects;
ANALYZE;
