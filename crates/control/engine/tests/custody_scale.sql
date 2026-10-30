-- A large retained world is made in set-based statements, with no per-item client loop.
CREATE TEMP TABLE custody_scale_threads ON COMMIT DROP AS
SELECT g AS number,md5('custody-scale-thread-'||g)::uuid AS id,
       md5('custody-scale-segment-'||g)::uuid AS segment
FROM custody_scale_seed c CROSS JOIN generate_series(c.thread_number,c.thread_number) g;

INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by,created_at,updated_at)
SELECT s.id,'thread',p.id,p.org_id,p.path||s.id,'Custody scale '||s.number,c.person,c.person,c.at,c.at
FROM custody_scale_threads s CROSS JOIN custody_scale_seed c JOIN nodes p ON p.id=c.topic;
INSERT INTO threads(node_id,topic_id,agent_id,provider_id,environment_id,arrangement)
SELECT s.id,c.topic,c.agent,c.provider,c.environment,'separated'
FROM custody_scale_threads s CROSS JOIN custody_scale_seed c;
INSERT INTO segments(id,thread_id,ordinal,agent_id,provider_id,environment_id,env_generation,arrangement,credential_path,seq_updated,started_at)
SELECT s.segment,s.id,1,c.agent,c.provider,c.environment,1,'separated','native',0,c.at
FROM custody_scale_threads s CROSS JOIN custody_scale_seed c;

CREATE TEMP TABLE custody_scale_work ON COMMIT DROP AS
SELECT s.id AS thread,s.segment,g AS ordinal,
       md5('custody-scale-input-'||s.number||'-'||g)::uuid AS input,
       md5('custody-scale-waiting-'||s.number||'-'||g)::uuid AS waiting,
       md5('custody-scale-turn-'||s.number||'-'||g)::uuid AS turn,
       md5('custody-scale-item-'||s.number||'-'||g)::uuid AS item,
       md5('custody-scale-usage-'||s.number||'-'||g)::uuid AS usage
FROM custody_scale_threads s CROSS JOIN custody_scale_seed c CROSS JOIN generate_series(1,c.turns_per_thread) g;
INSERT INTO thread_inputs(id,thread_id,author_id,source,content,delivery,state,position,seq_updated,created_at,delivered_at)
SELECT w.input,w.thread,c.person,'user','[]','queue','delivered',w.ordinal,0,c.at,c.at
FROM custody_scale_work w CROSS JOIN custody_scale_seed c;
INSERT INTO thread_inputs(id,thread_id,author_id,source,content,delivery,state,position,seq_updated,created_at)
SELECT w.waiting,w.thread,c.person,'user','[]','queue','pending',c.turns_per_thread+w.ordinal,0,c.at
FROM custody_scale_work w CROSS JOIN custody_scale_seed c;
INSERT INTO turns(id,thread_id,segment_id,ordinal,state,input_id,audit_for_user_id,model,usage,started_at,completed_at,created_at,seq_updated,native_end_confirmed,interrupt_cause)
SELECT w.turn,w.thread,w.segment,w.ordinal,
       CASE WHEN w.ordinal=c.turns_per_thread THEN 'interrupted' ELSE 'completed' END,
       w.input,c.person,'scale-model',jsonb_build_object('total_tokens',c.tokens_per_turn),c.at,c.at,c.at,0,true,
       CASE WHEN w.ordinal=c.turns_per_thread THEN 'resource_disabled' END
FROM custody_scale_work w CROSS JOIN custody_scale_seed c;
INSERT INTO items(id,thread_id,segment_id,turn_id,kind,status,payload,seq_created,seq_updated,created_at,updated_at)
SELECT w.item,w.thread,w.segment,w.turn,'agent_message','completed',
       '{"text":"retained scale history","text_len":22}',w.ordinal,w.ordinal,c.at,c.at
FROM custody_scale_work w CROSS JOIN custody_scale_seed c;
INSERT INTO items(id,thread_id,segment_id,turn_id,kind,status,payload,seq_created,seq_updated,created_at,updated_at)
SELECT md5('custody-scale-stopped-'||s.number)::uuid,s.id,s.segment,w.turn,'marker','completed',
       jsonb_build_object('marker','resource_stopped','data',jsonb_build_object(
         'object',jsonb_build_object('kind','provider','id',c.provider),'name','Scale provider',
         'by',jsonb_build_object('id',c.person,'kind','human','username','ada','display_name','Ada','avatar',jsonb_build_object('text','A','color',1)),
         'note',NULL,'model',NULL,'deleted',false)),
       c.turns_per_thread+1,c.turns_per_thread+1,c.at,c.at
FROM custody_scale_threads s CROSS JOIN custody_scale_seed c JOIN custody_scale_work w ON w.thread=s.id AND w.ordinal=c.turns_per_thread;
INSERT INTO usage_events(id,change_seq,at,origin,purpose,scope,counted,org_id,project_id,topic_id,thread_id,segment_id,turn_id,provider_id,user_id,agent_id,environment_id,model,input_tokens,total_tokens)
SELECT w.usage,0,c.at,'harness','turn','turn',true,n.org_id,n.parent_id,c.topic,
       w.thread,w.segment,w.turn,c.provider,c.person,c.agent,c.environment,'scale-model',c.tokens_per_turn,c.tokens_per_turn
FROM custody_scale_work w CROSS JOIN custody_scale_seed c JOIN nodes n ON n.id=c.topic;
UPDATE threads t SET next_seq=c.turns_per_thread+2 FROM custody_scale_seed c
WHERE t.node_id IN(SELECT id FROM custody_scale_threads);
