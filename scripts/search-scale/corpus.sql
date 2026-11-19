-- An isolated synthetic search fixture; ordinary application constraints and native indexes stay enabled.
CREATE TEMP TABLE corpus (
 id bigint PRIMARY KEY, kind text NOT NULL, scope int NOT NULL,
 title text NOT NULL, description text NOT NULL, body text NOT NULL, updated_at timestamptz NOT NULL
);
-- A million documents, half English and half Chinese; chat messages and thread items each add 20%.
-- Four different operational topics, with unique names/identifiers in each paragraph, keep stored text realistic.
INSERT INTO corpus
 SELECT n,CASE WHEN n<=current_setting('orch_probe.documents')::bigint THEN 'document' WHEN n<=current_setting('orch_probe.documents')::bigint*6/5 THEN 'chat' ELSE 'conversation' END,
   (n%100)::int,
   CASE WHEN n%2=0 THEN 'Release notes ' ELSE '发布记录 ' END || n,
   CASE WHEN n%4=0 THEN 'Deployment checks and rollback procedures' ELSE 'Daily engineering notes and decisions' END,
   CASE WHEN n%2=0 THEN
     'The release coordinator reviews the deployment plan, checks the migration and records the outcome. ' || md5(n::text) ||
     ' Before switching traffic, compare the health checks with the previous release and keep the stable version available. ' || md5((n+1)::text) ||
     ' The worker takes the next task after the transaction commits. A failed request is retried with the same identifier. ' || md5((n+2)::text) ||
     ' Each project has a separate resource policy. Engineers can inspect the logs and update the document after testing. ' || md5((n+3)::text) ||
     ' Index maintenance happens in bounded batches. The report records memory, storage, duration and the current state. ' || md5((n+4)::text) ||
     ' Readers see the changes once a revision is saved. A private conversation remains visible only to its participants. ' || md5((n+5)::text) ||
     ' The previous release stays available until the replacement passes its checks. Search finds the procedure and its owner. ' || md5((n+6)::text) ||
     CASE WHEN n%least(current_setting('orch_probe.documents')::bigint,10000)=0 THEN ' A zephyrfax incident requires manual recovery. ' ELSE ' Follow the agreed procedure and verify the resulting records. ' END || md5((n+7)::text)
   ELSE
     '本次发布由值班同事检查计划和迁移结果，确认服务正常后逐步切换流量。每个项目都有独立的访问规则。' || md5(n::text) ||
     '如果发现错误，先保留日志，再回滚到上一版本。事务提交以后，工作进程才开始下一项任务。' || md5((n+1)::text) ||
     '查询应当先返回已经找到的内容，后台继续补充其他结果，阅读中的列表不能突然移动。' || md5((n+2)::text) ||
     '工程师完成测试以后更新文档，记录资源、磁盘、持续时间以及当前状态。私密对话只对成员可见。' || md5((n+3)::text) ||
     '管理员可以修改索引使用的模型。新索引构建期间，原有的索引继续回答查询。' || md5((n+4)::text) ||
     '涉及发布的问题应当结合历史记录进行排查，确认变更已经生效，并且没有遗漏需要通知的人员。' || md5((n+5)::text) ||
     '当磁盘达到限制时，后台暂停处理，页面保留当前进度，等待管理员选择下一步。' || md5((n+6)::text)
   END, timestamptz '2026-01-01' + n * interval '1 second'
 FROM generate_series(1,current_setting('orch_probe.documents')::bigint*7/5) n;

CREATE FUNCTION search_probe_id(label text) RETURNS uuid LANGUAGE sql IMMUTABLE STRICT AS $$SELECT md5('search-probe/'||label)::uuid$$;
INSERT INTO users(id,username,display_name,avatar_color) VALUES
 (search_probe_id('owner'),'probe_owner','Corpus owner',0),
 (search_probe_id('all'),'probe_all','All reader',1),
 (search_probe_id('small'),'probe_small','One percent reader',2);
INSERT INTO nodes(id,kind,org_id,path,name,owner_id,created_by) VALUES
 (search_probe_id('root'),'org',search_probe_id('root'),ARRAY[search_probe_id('root')],'Corpus',search_probe_id('owner'),search_probe_id('owner'));
INSERT INTO orgs(node_id,slug,org_kind) VALUES(search_probe_id('root'),'search-probe','team');
INSERT INTO org_members(org_id,user_id,domain_id) SELECT search_probe_id('root'),id,search_probe_id('root') FROM users;
INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by)
 VALUES(search_probe_id('project'),'project',search_probe_id('root'),search_probe_id('root'),
 ARRAY[search_probe_id('root'),search_probe_id('project')],'Corpus project',search_probe_id('owner'),search_probe_id('owner'));
INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by)
 SELECT search_probe_id('topic/'||n),'topic',search_probe_id('project'),search_probe_id('root'),
  ARRAY[search_probe_id('root'),search_probe_id('project'),search_probe_id('topic/'||n)],'Scope '||n,search_probe_id('owner'),search_probe_id('owner') FROM generate_series(0,99)n;
INSERT INTO topics(node_id) SELECT id FROM nodes WHERE kind='topic';
INSERT INTO node_roles(id,node_id,subject_kind,subject_id,role,created_by) VALUES
 (search_probe_id('role/all'),search_probe_id('root'),'user',search_probe_id('all'),'developer',search_probe_id('owner')),
 (search_probe_id('role/small'),search_probe_id('topic/0'),'user',search_probe_id('small'),'developer',search_probe_id('owner'));

-- @documents
INSERT INTO documents(id,node_id,org_id,owner_id,created_by,title,description,description_source,doc_seq,block_count,bytes,updated_by,created_at,updated_at)
   SELECT search_probe_id('document/'||id),search_probe_id('topic/'||((id/2)%100)),search_probe_id('root'),search_probe_id('owner'),search_probe_id('owner'),
     title,description,'user',1,1,octet_length(body),search_probe_id('owner'),updated_at,updated_at
   FROM corpus WHERE kind='document' AND id BETWEEN $1 AND $2;

-- @content
INSERT INTO document_blocks(id,document_id,position,type,content,text,seq_created,seq_updated,created_by,updated_by,created_at,updated_at)
 SELECT search_probe_id('part/'||id),search_probe_id('document/'||id),1,'paragraph',jsonb_build_array(jsonb_build_object('type','text','text',body)),body,1,1,
 search_probe_id('owner'),search_probe_id('owner'),updated_at,updated_at FROM corpus WHERE kind='document';
-- @phase
INSERT INTO chats(id,kind,node_id,org_id,next_seq)
 SELECT search_probe_id('chat/'||n),'node',search_probe_id('topic/'||n),search_probe_id('root'),current_setting('orch_probe.documents')::bigint/500+1 FROM generate_series(0,99)n;
INSERT INTO chat_messages(id,chat_id,author_id,content,search_text,seq_created,seq_updated,created_at)
 SELECT search_probe_id('part/'||id),search_probe_id('chat/'||((id/2)%100)),search_probe_id('owner'),
 jsonb_build_array(jsonb_build_object('type','text','text',body)),body,
 row_number() OVER(PARTITION BY ((id/2)%100) ORDER BY id),row_number() OVER(PARTITION BY ((id/2)%100) ORDER BY id),updated_at FROM corpus WHERE kind='chat';
-- @phase
INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by)
 SELECT search_probe_id('thread/'||n),'thread',search_probe_id('topic/'||n),search_probe_id('root'),
  ARRAY[search_probe_id('root'),search_probe_id('project'),search_probe_id('topic/'||n),search_probe_id('thread/'||n)],'Thread '||n,search_probe_id('owner'),search_probe_id('owner') FROM generate_series(0,99)n;
BEGIN;
INSERT INTO threads(node_id,topic_id,next_seq,active_continuation_id)
 SELECT search_probe_id('thread/'||n),search_probe_id('topic/'||n),current_setting('orch_probe.documents')::bigint/500+1,search_probe_id('thread/'||n) FROM generate_series(0,99)n;
COMMIT;

-- @phase
INSERT INTO machines(id,name,placement,status,online,roles,platform_os,platform_arch,connector_version,protocol_version,cert_serial,cert_fingerprint,cert_not_after) VALUES(search_probe_id('machine'),'Search probe','connected','active',true,ARRAY['agent','environment','provider'],'linux','x86_64','0.1.0',1,'search-probe',decode(md5('search-probe')||md5('search-probe'),'hex'),timestamptz '2028-01-01');
INSERT INTO env_templates(id,name,driver,spec) VALUES(search_probe_id('template'),'Probe','docker','{}');
INSERT INTO agents(id,name,owner_id,org_id,home_id,harness_kind,profile,descriptor)
 VALUES(search_probe_id('agent'),'Probe agent',search_probe_id('owner'),search_probe_id('root'),search_probe_id('root'),'fake','{}','{}');
INSERT INTO providers(id,name,owner_id,org_id,home_id,kind,machine_id,key_machine_id,upstreams,protocols,arrangements,status)
 VALUES(search_probe_id('provider'),'Probe embeddings',search_probe_id('owner'),search_probe_id('root'),search_probe_id('root'),'api',search_probe_id('machine'),search_probe_id('machine'),
 '[{"protocol":"openai-embeddings","base_url":"https://embeddings.invalid/v1","options":{"auth":"none"}}]',ARRAY['openai-embeddings'],ARRAY['key_separated'],'ready');
INSERT INTO provider_models(id,provider_id,model_name,display_name,enabled,source,created_by)
 VALUES(search_probe_id('model'),search_probe_id('provider'),'probe-256','Probe',true,'manual',search_probe_id('owner'));
INSERT INTO environments(id,name,owner_id,org_id,placement,home_node_id,workspace_root,machine_id,template_id,generation,status)
 VALUES(search_probe_id('environment'),'Probe',search_probe_id('owner'),search_probe_id('root'),'managed',search_probe_id('topic/0'),'/workspace',search_probe_id('machine'),search_probe_id('template'),1,'running');
INSERT INTO segments(id,thread_id,ordinal,agent_id,provider_id,environment_id,machine_id,env_generation,arrangement,credential_path,native_session_ref,seq_updated,continuation_id)
 SELECT search_probe_id('segment/'||n),search_probe_id('thread/'||n),1,search_probe_id('agent'),search_probe_id('provider'),search_probe_id('environment'),search_probe_id('machine'),1,
 'key_separated','forwarded','probe-'||n,0,search_probe_id('thread/'||n) FROM generate_series(0,99)n;
UPDATE threads t SET agent_id=search_probe_id('agent'),provider_id=search_probe_id('provider'),environment_id=search_probe_id('environment'),arrangement='key_separated',current_segment_id=search_probe_id('segment/'||substring(n.name from 8)) FROM nodes n WHERE n.id=t.node_id;
UPDATE thread_continuations c SET current_segment_id=t.current_segment_id FROM threads t WHERE t.node_id=c.thread_id;
INSERT INTO items(id,thread_id,segment_id,continuation_id,kind,status,author_id,payload,search_text,seq_created,seq_updated,created_at)
 SELECT search_probe_id('part/'||id),search_probe_id('thread/'||((id/2)%100)),search_probe_id('segment/'||((id/2)%100)),search_probe_id('thread/'||((id/2)%100)),
 'user_message','completed',search_probe_id('owner'),jsonb_build_object('parts',jsonb_build_array(jsonb_build_object('type','text','text',body))),body,
 row_number() OVER(PARTITION BY ((id/2)%100) ORDER BY id),row_number() OVER(PARTITION BY ((id/2)%100) ORDER BY id),updated_at
 FROM corpus WHERE kind='conversation';

-- @phase
BEGIN;
INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by)
 SELECT search_probe_id('reader/'||label),'thread',search_probe_id('topic/0'),search_probe_id('root'),
 ARRAY[search_probe_id('root'),search_probe_id('project'),search_probe_id('topic/0'),search_probe_id('reader/'||label)],'Reader '||label,search_probe_id(label),search_probe_id(label)
 FROM unnest(ARRAY['all','small'])label;
INSERT INTO threads(node_id,topic_id,active_continuation_id,agent_id,provider_id,environment_id,arrangement,state)
 SELECT search_probe_id('reader/'||label),search_probe_id('topic/0'),search_probe_id('reader/'||label),search_probe_id('agent'),search_probe_id('provider'),search_probe_id('environment'),'key_separated','running'
 FROM unnest(ARRAY['all','small'])label;
COMMIT;
INSERT INTO segments(id,thread_id,ordinal,agent_id,provider_id,environment_id,machine_id,env_generation,arrangement,credential_path,native_session_ref,seq_updated,continuation_id)
 SELECT search_probe_id('reader-segment/'||label),search_probe_id('reader/'||label),1,search_probe_id('agent'),search_probe_id('provider'),search_probe_id('environment'),search_probe_id('machine'),1,
 'key_separated','forwarded','reader-'||label,0,search_probe_id('reader/'||label) FROM unnest(ARRAY['all','small'])label;
INSERT INTO thread_inputs(id,thread_id,author_id,source,content,delivery,state,position,seq_updated,continuation_id)
 SELECT search_probe_id('reader-input/'||label),search_probe_id('reader/'||label),search_probe_id(label),'user','[{"type":"text","text":"Search the corpus"}]','queue','delivered',1,0,search_probe_id('reader/'||label) FROM unnest(ARRAY['all','small'])label;
INSERT INTO turns(id,thread_id,segment_id,ordinal,state,input_id,model,seq_updated,continuation_id)
 SELECT search_probe_id('reader-turn/'||label),search_probe_id('reader/'||label),search_probe_id('reader-segment/'||label),1,'running',search_probe_id('reader-input/'||label),'probe-256',0,search_probe_id('reader/'||label) FROM unnest(ARRAY['all','small'])label;
UPDATE threads t SET current_turn_id=search_probe_id('reader-turn/'||label),current_segment_id=search_probe_id('reader-segment/'||label)
 FROM unnest(ARRAY['all','small'])label WHERE t.node_id=search_probe_id('reader/'||label);
UPDATE thread_continuations c SET current_segment_id=t.current_segment_id FROM threads t WHERE t.node_id=c.thread_id;
INSERT INTO permission_entries(id,object_kind,object_id,page,profile,permission,holder_kind,holder_id,created_by,created_at,share_inside)
 SELECT search_probe_id('grant/'||label||'/'||permission||'/'||profile),'node',
 CASE label WHEN 'all' THEN search_probe_id('root') ELSE search_probe_id('topic/0') END,
 'defaults',profile,permission,'agents',search_probe_id('reader/'||label),search_probe_id('owner'),timestamptz '2026-01-01',true
 FROM unnest(ARRAY['all','small'])label CROSS JOIN
 (VALUES ('doc.view','any'),('chat.view','any'),('thread.view','managed'),('thread.view','connected')) row(permission,profile);
INSERT INTO permission_entries(id,object_kind,object_id,page,permission,holder_kind,holder_id,created_by)
 SELECT search_probe_id('grant-own/'||label),'node',search_probe_id('reader/'||label),'live','access.own','user',search_probe_id(label),search_probe_id('owner')
 FROM unnest(ARRAY['all','small'])label;

-- @phase
INSERT INTO search_generations(id,org_id,provider_id,model_name,asked_dimensions,dimensions,chunking,state,created_at,activated_at)
 VALUES(search_probe_id('generation'),search_probe_id('root'),search_probe_id('provider'),'probe-256',256,256,'parts-v1:400','active',timestamptz '2026-01-01',timestamptz '2026-01-01');
INSERT INTO search_sources(generation_id,kind,source_id,org_id,title,marker,changed_at,chunks,tokens,bytes,embedded_for)
 SELECT search_probe_id('generation'),'document',d.id,d.org_id,d.title,d.doc_seq,d.updated_at,1,300,d.bytes,search_probe_id('generation') FROM documents d
 UNION ALL SELECT search_probe_id('generation'),'chat',c.id,c.org_id,n.name,c.next_seq,timestamptz '2026-01-01',(c.next_seq-1)::int,300*(c.next_seq-1),1100*(c.next_seq-1),search_probe_id('generation') FROM chats c JOIN nodes n ON n.id=c.node_id
 UNION ALL SELECT search_probe_id('generation'),'conversation',t.node_id,n.org_id,n.name,t.next_seq,timestamptz '2026-01-01',(t.next_seq-1)::int,300*(t.next_seq-1),1100*(t.next_seq-1),search_probe_id('generation') FROM threads t JOIN nodes n ON n.id=t.node_id;
-- @phase
INSERT INTO search_chunks(id,generation_id,node_id,kind,source_id,org_id,ordinal,first_part,text_hash,tokens,bytes)
 SELECT search_probe_id('chunk/'||id),search_probe_id('generation'),
 CASE kind WHEN 'conversation' THEN search_probe_id('thread/'||((id/2)%100)) ELSE search_probe_id('topic/'||((id/2)%100)) END,
 kind,CASE kind WHEN 'document' THEN search_probe_id('document/'||id) WHEN 'chat' THEN search_probe_id('chat/'||((id/2)%100)) ELSE search_probe_id('thread/'||((id/2)%100)) END,
 search_probe_id('root'),CASE kind WHEN 'document' THEN 0 ELSE row_number() OVER(PARTITION BY kind,(id/2)%100 ORDER BY id)::int-1 END,
 search_probe_id('part/'||id),decode(md5(id::text),'hex'),300,octet_length(body) FROM corpus;


-- Dense deterministic anchors and independent perturbations keep neighborhoods unrelated to access scopes.
-- The anchor/jitter pair has over 500 million combinations for 1.4 million chunks.
-- @phase
CREATE TABLE corpus_vector_prototypes AS
 SELECT i,l2_normalize(ARRAY(SELECT CASE WHEN d=(i%8)+1 THEN 1::real ELSE (sin(i*17+d*13)*0.02)::real END
 FROM generate_series(1,256)d)::halfvec(256)) embedding FROM generate_series(0,8191)i;
ALTER TABLE corpus_vector_prototypes ADD PRIMARY KEY(i);
CREATE TEMP TABLE corpus_vector_jitter AS
 SELECT i,array_agg((sin((i::float8+1)*(d+3)*0.7548776662)*0.002)::real ORDER BY d)::vector(256) embedding
 FROM generate_series(0,65535)i CROSS JOIN generate_series(0,255)d GROUP BY i;
ALTER TABLE corpus_vector_jitter ADD PRIMARY KEY(i);
-- @phase
INSERT INTO search_vectors(generation_id,chunk_id,text_hash,embedding)
 SELECT search_probe_id('generation'),search_probe_id('chunk/'||c.id),decode(md5(c.id::text),'hex'),
 l2_normalize(p.embedding::vector+j.embedding)::halfvec
 FROM corpus c JOIN corpus_vector_prototypes p
 ON p.i=get_byte(decode(md5(c.id::text),'hex'),0)*32+get_byte(decode(md5(c.id::text),'hex'),1)%32
 JOIN corpus_vector_jitter j ON j.i=(hashint8(c.id)::bigint & 65535);
-- @phase
ANALYZE;
