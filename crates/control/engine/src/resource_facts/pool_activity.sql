-- Only actual admitted attempts carry pool attribution; direct use of the same leaf is independent.
SELECT 'running'::text AS kind,t.id,1::bigint AS count,t.thread_id AS thread,
       t.segment_id AS segment,i.author_id AS author,n.name,t.created_at AS since
FROM turns t JOIN thread_inputs i ON i.id=t.input_id JOIN nodes n ON n.id=t.thread_id
WHERE t.state IN ('starting','running')
  AND EXISTS(SELECT 1 FROM pool_attempt_scopes a JOIN pool_choices c ON c.id=a.choice_id
             WHERE a.attempt_id=t.id AND c.path @> jsonb_build_array(jsonb_build_object('pool_id',$1::uuid)))
UNION ALL
SELECT 'stopped'::text,t.id,1::bigint,t.thread_id,t.segment_id,i.author_id,n.name,COALESCE(t.completed_at,t.created_at)
FROM threads th JOIN turns t ON t.thread_id=th.node_id AND t.continuation_id=th.active_continuation_id
  JOIN thread_inputs i ON i.id=t.input_id JOIN nodes n ON n.id=t.thread_id
WHERE t.state='interrupted' AND t.interrupt_cause='resource_disabled' AND NOT th.queue_held
  AND NOT EXISTS(SELECT 1 FROM turns later WHERE later.thread_id=t.thread_id AND later.continuation_id=t.continuation_id AND later.ordinal>t.ordinal)
  AND EXISTS(SELECT 1 FROM items marker WHERE marker.turn_id=t.id AND marker.kind='marker'
             AND marker.payload->>'marker'='resource_stopped'
             AND marker.payload#>>'{data,object,id}'=$1::text AND marker.payload#>>'{data,object,kind}'='pool')
  AND NOT EXISTS(SELECT 1 FROM items resumed WHERE resumed.turn_id=t.id AND resumed.kind='marker'
                 AND resumed.payload->>'marker'='resource_resumed' AND resumed.payload#>>'{data,input_id}' IS NOT NULL)
