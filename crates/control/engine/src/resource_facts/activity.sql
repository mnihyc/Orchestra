-- Live dependencies use immutable turn attribution; pending messages follow their current binding.
WITH model AS (
  SELECT model_name FROM provider_models WHERE id=$3 AND provider_id=$2
), bound AS (
  SELECT t.node_id,t.active_continuation_id,t.queue_held FROM threads t WHERE
    (($1='provider' AND t.provider_id=$2) OR ($1='agent' AND t.agent_id=$2) OR ($1='environment' AND t.environment_id=$2))
), running AS (
  SELECT 'running'::text AS kind,t.id,1::bigint AS count,t.thread_id AS thread,t.segment_id AS segment,i.author_id AS author,n.name,t.created_at AS since
  FROM segments s JOIN turns t ON t.segment_id=s.id JOIN nodes n ON n.id=t.thread_id JOIN thread_inputs i ON i.id=t.input_id
  WHERE (($1='provider' AND s.provider_id=$2) OR ($1='agent' AND s.agent_id=$2) OR ($1='environment' AND s.environment_id=$2))
    AND ($3::uuid IS NULL OR t.model=(SELECT model_name FROM model)) AND t.state IN ('starting','running')
), waiting AS (
  SELECT 'waiting'::text,min(i.id::text)::uuid AS id,count(*)::bigint AS count,b.node_id AS thread,NULL::uuid AS segment,NULL::uuid AS author,n.name,min(i.created_at) AS since
  FROM bound b JOIN thread_inputs i ON i.thread_id=b.node_id AND i.continuation_id=b.active_continuation_id JOIN nodes n ON n.id=b.node_id
  WHERE i.state='pending' AND NOT i.queue_held AND NOT b.queue_held AND n.deleted_at IS NULL
  GROUP BY b.node_id,n.name
), stopped AS (
  SELECT 'stopped'::text,t.id,1::bigint AS count,t.thread_id AS thread,t.segment_id AS segment,i.author_id AS author,n.name,COALESCE(t.completed_at,t.created_at) AS since
  FROM bound b JOIN LATERAL (SELECT latest.* FROM turns latest WHERE latest.thread_id=b.node_id AND latest.continuation_id=b.active_continuation_id ORDER BY latest.ordinal DESC LIMIT 1) t ON true
    JOIN segments s ON s.id=t.segment_id JOIN thread_inputs i ON i.id=t.input_id JOIN nodes n ON n.id=b.node_id
  WHERE t.state='interrupted' AND t.interrupt_cause='resource_disabled' AND s.continuation_id=b.active_continuation_id AND NOT b.queue_held
    AND EXISTS(SELECT 1 FROM items marker WHERE marker.turn_id=t.id AND marker.kind='marker' AND marker.payload->>'marker'='resource_stopped'
      AND marker.payload#>>'{data,object,id}'=$2::text AND marker.payload#>>'{data,object,kind}'=$1
      AND (($3::uuid IS NULL AND marker.payload#>>'{data,model}' IS NULL) OR marker.payload#>>'{data,model}'=(SELECT model_name FROM model)))
    AND NOT EXISTS(SELECT 1 FROM items resumed WHERE resumed.turn_id=t.id AND resumed.kind='marker'
      AND resumed.payload->>'marker'='resource_resumed' AND resumed.payload#>>'{data,input_id}' IS NOT NULL)
)
SELECT * FROM running UNION ALL SELECT * FROM waiting UNION ALL SELECT * FROM stopped
