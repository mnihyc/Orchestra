__OWNERS__,visible AS NOT MATERIALIZED (
  SELECT i.id,i.thread_id,i.turn_id,i.stream,i.kind,i.author_id,i.segment_id,i.continuation_id,i.before_orch,i.seq_created,i.created_at,
    i.payload->>'phase' AS live_phase,i.payload #>> '{tool,kind}' AS live_tool_kind,
    (o.frozen_at IS NOT NULL AND i.seq_updated>o.frozen_at) AS from_revision,
    CASE WHEN o.frozen_at IS NULL OR i.seq_updated<=o.frozen_at THEN i.seq_updated ELSE r.seq_updated END AS seq_updated,
    CASE WHEN o.frozen_at IS NULL OR i.seq_updated<=o.frozen_at THEN i.status ELSE r.body->>'status' END AS status,
    CASE WHEN o.frozen_at IS NULL OR i.seq_updated<=o.frozen_at THEN i.payload ELSE r.body->'payload' END AS payload,
    CASE WHEN o.frozen_at IS NULL OR i.seq_updated<=o.frozen_at THEN i.search_text ELSE r.body->>'search_text' END AS search_text
  FROM owners o JOIN items i ON i.continuation_id=o.owner AND i.thread_id=$1
  LEFT JOIN LATERAL (SELECT body,seq_updated FROM thread_item_revisions r WHERE o.frozen_at IS NOT NULL
    AND i.seq_updated>o.frozen_at AND r.item_id=i.id AND r.seq_updated<=o.frozen_at ORDER BY r.seq_updated DESC LIMIT 1) r ON true
  WHERE i.seq_created BETWEEN o.first_seq AND o.last_seq AND (i.turn_id IS NULL OR EXISTS(
    SELECT 1 FROM turns t WHERE t.id=i.turn_id AND t.ordinal BETWEEN o.first_turn AND o.last_turn))
),candidates AS (
  SELECT selected.* FROM owners o JOIN LATERAL (
    SELECT i.id,i.seq_created,i.created_at,i.kind,i.turn_id,
      (i.turn_id IS NOT NULL AND i.kind<>'marker' AND NOT EXISTS(SELECT 1 FROM items prior
        WHERE prior.thread_id=i.thread_id AND prior.turn_id=i.turn_id AND prior.stream='main'
          AND prior.kind<>'marker' AND prior.seq_created<i.seq_created)) AS starts_turn
    FROM items i CROSS JOIN LATERAL (SELECT
      CASE WHEN o.frozen_at IS NULL OR i.seq_updated<=o.frozen_at THEN i.payload ELSE
        (SELECT r.body->'payload' FROM thread_item_revisions r WHERE r.item_id=i.id AND r.seq_updated<=o.frozen_at ORDER BY r.seq_updated DESC LIMIT 1) END AS payload,
      CASE WHEN o.frozen_at IS NULL OR i.seq_updated<=o.frozen_at THEN i.search_text ELSE
        (SELECT r.body->>'search_text' FROM thread_item_revisions r WHERE r.item_id=i.id AND r.seq_updated<=o.frozen_at ORDER BY r.seq_updated DESC LIMIT 1) END AS search_text) v
    WHERE i.thread_id=$1 AND i.continuation_id=o.owner AND __STREAM_FILTER__
      AND i.seq_created BETWEEN o.first_seq AND o.last_seq
      AND ($3::bigint IS NULL OR i.seq_created __COMPARISON__ $3)
      AND (i.turn_id IS NULL OR (SELECT t.ordinal BETWEEN o.first_turn AND o.last_turn FROM turns t WHERE t.id=i.turn_id))
      AND (__KIND_FILTER__)
      AND ($6::text IS NULL OR orch_conversation_text(i.kind,v.payload,v.search_text) ILIKE $6
        OR (i.kind='user_message' AND orch_conversation_text(i.kind,v.payload,NULL) ILIKE $6)
        OR v.payload #>> '{tool,name}' ILIKE $6
        OR EXISTS(SELECT 1 FROM users u WHERE u.id=i.author_id AND (u.username ILIKE $6 OR u.display_name ILIKE $6)))
    ORDER BY i.seq_created __ORDER__ LIMIT $4) selected ON true
  ORDER BY selected.seq_created __ORDER__ LIMIT $4
),turn_facts AS (
  SELECT needed.turn_id,
    CASE WHEN __ANSWER_FACTS__ THEN (SELECT MAX(last.seq_created) FROM owners o JOIN LATERAL (
      SELECT v.seq_created FROM visible v WHERE v.turn_id=needed.turn_id AND v.continuation_id=o.owner AND v.stream='main'
        AND v.seq_created BETWEEN o.first_seq AND o.last_seq AND v.kind='agent_message' AND orch_conversation_text(v.kind,v.payload,v.search_text) ~ '[^[:space:]]'
      ORDER BY v.seq_created DESC LIMIT 1) last ON true) END AS last_agent,
    CASE WHEN __ANSWER_FACTS__ THEN (SELECT MAX(last.seq_created) FROM owners o JOIN LATERAL (
      SELECT v.seq_created FROM visible v WHERE v.turn_id=needed.turn_id AND v.continuation_id=o.owner AND v.stream='main'
        AND v.seq_created BETWEEN o.first_seq AND o.last_seq AND v.kind='agent_message' AND (v.live_phase='final' OR v.from_revision)
        AND v.payload->>'phase'='final' AND orch_conversation_text(v.kind,v.payload,v.search_text) ~ '[^[:space:]]'
      ORDER BY v.seq_created DESC LIMIT 1) last ON true) END AS last_final,
    CASE WHEN __ANSWER_FACTS__ THEN (SELECT MAX(last.seq_created) FROM owners o JOIN LATERAL (
      SELECT v.seq_created FROM visible v WHERE v.turn_id=needed.turn_id AND v.continuation_id=o.owner AND v.stream='main'
        AND v.seq_created BETWEEN o.first_seq AND o.last_seq
        AND (v.kind IN('tool_call','reasoning','plan') OR v.kind='approval' AND v.status<>'pending')
      ORDER BY v.seq_created DESC LIMIT 1) last ON true) END AS last_work,
    CASE WHEN __COUNT_EDITS__ THEN (SELECT jsonb_build_object(
        'added',COALESCE(SUM((part->>'added')::numeric),0),'removed',COALESCE(SUM((part->>'removed')::numeric),0))
      FROM visible edit CROSS JOIN LATERAL jsonb_array_elements(CASE WHEN jsonb_typeof(edit.payload #> '{tool,changes}')='array' THEN edit.payload #> '{tool,changes}' ELSE '[]'::jsonb END) part
      WHERE edit.turn_id=needed.turn_id AND edit.stream='main' AND edit.kind='tool_call'
        AND (edit.live_tool_kind='file_change' OR edit.from_revision) AND edit.payload #>> '{tool,kind}'='file_change') END AS edits
  FROM (SELECT DISTINCT turn_id FROM candidates WHERE turn_id IS NOT NULL AND __MESSAGE_FACTS__) needed
)
SELECT c.id,c.seq_created,c.created_at,c.kind,c.starts_turn,t.ordinal AS turn_ordinal,i.author_id,i.turn_id,
  CASE c.kind
    WHEN 'user_message' THEN jsonb_build_object('source',i.payload->'source',
      'goal_condition',left(i.payload #>> '{goal,condition}',$5::int),
      'parts',CASE WHEN i.payload->>'source'='goal' THEN '[]'::jsonb ELSE jsonb_build_array(jsonb_build_object('type','text','text',
        (SELECT left(regexp_replace(line,'^[[:space:]]+|[[:space:]]+$','','g'),$5::int) FROM regexp_split_to_table(orch_conversation_text(i.kind,i.payload,i.search_text),E'\n') WITH ORDINALITY lines(line,ordinal) WHERE line ~ '[^[:space:]]' ORDER BY ordinal LIMIT 1))) END)
    WHEN 'agent_message' THEN jsonb_build_object('phase',i.payload->'phase','text',(SELECT left(regexp_replace(line,'^[[:space:]]+|[[:space:]]+$','','g'),$5::int)
      FROM regexp_split_to_table(orch_conversation_text(i.kind,i.payload,i.search_text),E'\n') WITH ORDINALITY lines(line,ordinal) WHERE line ~ '[^[:space:]]' ORDER BY ordinal LIMIT 1))
    WHEN 'announcement' THEN jsonb_build_object('title',left(i.payload->>'title',$5::int),'category',i.payload->'category')
    WHEN 'approval' THEN jsonb_build_object('approval_id',i.payload->'approval_id')
    WHEN 'tool_call' THEN CASE WHEN i.payload #>> '{tool,kind}'='subagent' THEN jsonb_build_object(
      'tool',jsonb_build_object('kind','subagent','child_stream',i.payload #> '{tool,child_stream}','name',i.payload #> '{tool,name}','state',i.payload #> '{tool,state}'),
      'started_at',i.payload->'started_at','duration_ms',i.payload->'duration_ms')
      WHEN i.payload #>> '{tool,kind}'='orch' AND i.payload #>> '{tool,tool}'='announce' AND i.payload #>> '{tool,arguments,category}'='milestone' THEN
        jsonb_build_object('tool',jsonb_build_object('tool','announce','arguments',jsonb_build_object('category','milestone','title',left(i.payload #>> '{tool,arguments,title}',$5::int))))
      ELSE '{}'::jsonb END
    ELSE '{}'::jsonb END AS payload,
  jsonb_build_object('ended',t.state NOT IN('starting','running'),'last_agent',facts.last_agent,'last_final',facts.last_final,'last_work',facts.last_work,
    'added',facts.edits->'added','removed',facts.edits->'removed','updated_seq',i.seq_updated,
    'recorded',i.before_orch OR s.ended_at IS NOT NULL OR t.state NOT IN('starting','running'),
    'result',(SELECT (SELECT left(regexp_replace(line,'^[[:space:]]+|[[:space:]]+$','','g'),$5::int) FROM regexp_split_to_table(orch_conversation_text(child.kind,child.payload,child.search_text),E'\n') WITH ORDINALITY lines(line,ordinal) WHERE line ~ '[^[:space:]]' ORDER BY ordinal LIMIT 1) FROM visible child
      WHERE c.kind='tool_call' AND i.payload #>> '{tool,kind}'='subagent' AND child.stream=i.payload #>> '{tool,child_stream}'
        AND child.kind='agent_message' AND orch_conversation_text(child.kind,child.payload,child.search_text) ~ '[^[:space:]]' ORDER BY child.seq_created DESC LIMIT 1),
    'tokens_out',(SELECT share #> '{tokens,output_tokens}' FROM jsonb_array_elements(CASE WHEN jsonb_typeof(t.usage_detail->'by_subagent')='array' THEN t.usage_detail->'by_subagent' ELSE '[]'::jsonb END) share
      WHERE c.kind='tool_call' AND i.payload #>> '{tool,kind}'='subagent' AND share->>'stream'=i.payload #>> '{tool,child_stream}' LIMIT 1)) AS metadata
FROM candidates c JOIN visible i ON i.id=c.id LEFT JOIN turns t ON t.id=c.turn_id AND t.thread_id=$1
LEFT JOIN segments s ON s.id=i.segment_id AND s.thread_id=$1 LEFT JOIN turn_facts facts ON facts.turn_id=c.turn_id
ORDER BY c.seq_created
