-- The searched conversations and, for each, the continuations whose items make up its viewed history.
scoped_nodes AS NOT MATERIALIZED (
  SELECT * FROM node_access n WHERE $6::uuid IS NULL OR n.path @> ARRAY[$6::uuid]
), readable_threads AS NOT MATERIALIZED (
  SELECT id FROM scoped_nodes n WHERE kind='thread' AND $7::uuid IS NULL AND $8::uuid IS NULL
    AND COALESCE((n."grant"->>'view')::boolean,false)
    AND (NOT $23 OR COALESCE((n."grant"->>'participating')::boolean,false))
), conversation_histories AS NOT MATERIALIZED (
  SELECT t.node_id AS thread_id,c.id AS view FROM readable_threads r JOIN threads t ON t.node_id=r.id
    JOIN thread_continuations c ON c.thread_id=t.node_id AND c.id=COALESCE($30::uuid,t.active_continuation_id)
), conversation_owners AS NOT MATERIALIZED (
  SELECT h.thread_id,h.view AS owner,1 AS first_turn,2147483647 AS last_turn,
    0::bigint AS first_seq,9223372036854775807::bigint AS last_seq,NULL::bigint AS frozen_at
    FROM conversation_histories h
  UNION ALL SELECT h.thread_id,s.owner_continuation_id,s.first_turn,s.last_turn,s.first_seq,s.last_seq,s.frozen_at
    FROM conversation_histories h JOIN thread_continuation_spans s ON s.continuation_id=h.view
)
