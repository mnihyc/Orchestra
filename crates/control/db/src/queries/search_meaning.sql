WITH __NODE_VISIBILITY__, scoped_nodes AS NOT MATERIALIZED (
  SELECT * FROM node_access n WHERE $6::uuid IS NULL OR n.path @> ARRAY[$6::uuid]
), readable_threads AS NOT MATERIALIZED (
  SELECT id FROM scoped_nodes n WHERE kind='thread'
    AND COALESCE((n."grant"->>'view')::boolean,false)
    AND (NOT $7 OR COALESCE((n."grant"->>'participating')::boolean,false))
), conversation_histories AS NOT MATERIALIZED (
  SELECT t.node_id AS thread_id,c.id AS view FROM readable_threads r JOIN threads t ON t.node_id=r.id
    JOIN thread_continuations c ON c.thread_id=t.node_id AND c.id=t.active_continuation_id
), conversation_owners AS NOT MATERIALIZED (
  SELECT h.thread_id,h.view AS owner,1 AS first_turn,2147483647 AS last_turn,
    0::bigint AS first_seq,9223372036854775807::bigint AS last_seq,NULL::bigint AS frozen_at
    FROM conversation_histories h
  UNION ALL SELECT h.thread_id,s.owner_continuation_id,s.first_turn,s.last_turn,s.first_seq,s.last_seq,s.frozen_at
    FROM conversation_histories h JOIN thread_continuation_spans s ON s.continuation_id=h.view
), readable_chats AS NOT MATERIALIZED (
  SELECT c.id FROM chats c JOIN nodes root ON root.id=c.org_id LEFT JOIN scoped_nodes n ON n.id=c.node_id
    CROSS JOIN LATERAL (SELECT CASE WHEN c.node_id IS NULL THEN $4::jsonb->'objects'->c.id::text ELSE n."grant"->'chat' END value) page
   WHERE ((c.node_id IS NOT NULL AND n.id IS NOT NULL) OR (c.node_id IS NULL AND ($6::uuid IS NULL OR c.org_id=$6)))
     AND NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id=ANY(root.path) AND gone.deleted_at IS NOT NULL)
     AND COALESCE((page.value->>'view')::boolean,false)
     AND (NOT $7 OR COALESCE((page.value->>'participating')::boolean,false))
), readable_documents AS NOT MATERIALIZED (
  SELECT d.* FROM documents d JOIN nodes n ON n.id=d.node_id LEFT JOIN threads t ON t.node_id=n.id
   WHERE d.deleted_at IS NULL AND ($6::uuid IS NULL OR n.path @> ARRAY[$6::uuid]) AND t.side_of IS NULL
     AND NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id=ANY(n.path) AND gone.deleted_at IS NOT NULL)
     AND COALESCE(($4::jsonb->'objects'->d.id::text->>'view')::boolean,false)
     AND (NOT $7 OR COALESCE(($4::jsonb->'objects'->d.id::text->>'participating')::boolean,false))
), found AS (
  -- The parts that pieces found by meaning begin with, each only where its reader may read it now.
  SELECT 'item'::text kind,i.thread_id object_id,i.id,NULL::uuid block_id,i.search_text text,i.created_at at,
         i.author_id,NULL::uuid author_thread_id,NULL::text author_agent,0::int relevance,0::bigint matches
    FROM items i JOIN conversation_owners o ON o.thread_id=i.thread_id AND o.owner=i.continuation_id
   WHERE i.id=ANY($8::uuid[]) AND i.kind=ANY($12::text[]) AND i.kind<>'marker' AND i.search_text IS NOT NULL
     AND i.seq_created BETWEEN o.first_seq AND o.last_seq AND (o.frozen_at IS NULL OR i.seq_updated<=o.frozen_at)
     AND (i.turn_id IS NULL OR EXISTS(SELECT 1 FROM turns t WHERE t.id=i.turn_id AND t.ordinal BETWEEN o.first_turn AND o.last_turn))
     AND i.search_text NOT LIKE '%<orch>%' AND $17::uuid IS NULL AND $18::uuid IS NULL
     AND ($19::uuid IS NULL OR i.author_id=$19)
     AND (NOT $20 OR orch_search_file_types(i.payload) @> ARRAY['*/*'])
     AND ($21::uuid IS NULL OR jsonb_path_exists(i.payload,'$.** ? (@.type == "mention" && @.user_id == $person)',jsonb_build_object('person',$21::text)))
  UNION ALL
  SELECT 'chat_message',m.chat_id,m.id,NULL::uuid,m.search_text,m.created_at,m.author_id,m.author_thread_id,m.author_agent,0,0
    FROM chat_messages m JOIN readable_chats r ON r.id=m.chat_id
   WHERE $13 AND m.id=ANY($9::uuid[]) AND m.deleted_at IS NULL AND m.search_text IS NOT NULL AND $18::uuid IS NULL AND ($17::uuid IS NULL OR m.chat_id=$17)
     AND ($19::uuid IS NULL OR m.author_id=$19)
     AND (NOT $20 OR orch_search_file_types(m.content) @> ARRAY['*/*'])
     AND ($21::uuid IS NULL OR $21=ANY(m.mentions))
  UNION ALL
  SELECT 'document',b.document_id,b.id,b.id,b.text,b.updated_at,b.updated_by,b.updated_by_thread,b.updated_by_agent,0,0
    FROM document_blocks b JOIN readable_documents d ON d.id=b.document_id
   WHERE $14 AND b.id=ANY($10::uuid[]) AND b.removed_at IS NULL AND $17::uuid IS NULL AND ($18::uuid IS NULL OR b.document_id=$18)
     AND ($19::uuid IS NULL OR b.updated_by=$19)
     AND (NOT $20 OR d.search_file_count>0)
     AND ($21::uuid IS NULL OR jsonb_path_exists(b.content,'$.** ? (@.type == "mention" && @.user_id == $person)',jsonb_build_object('person',$21::text)))
)
SELECT *,lower(text) folded_text,lower($11::text) folded_query FROM found
 WHERE (($4::jsonb->'search_advanced'->>'date_field') IS NOT NULL OR $15::timestamptz IS NULL OR at >= $15)
   AND (($4::jsonb->'search_advanced'->>'date_field') IS NOT NULL OR $16::timestamptz IS NULL OR at <= $16)
   AND __ADVANCED_SOURCE__ AND __ADVANCED_FILE__ AND __ADVANCED_SHARED__
