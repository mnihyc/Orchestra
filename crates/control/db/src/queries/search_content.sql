WITH __NODE_VISIBILITY__, __HISTORY__, readable_chats AS NOT MATERIALIZED (
  SELECT c.id FROM chats c JOIN nodes root ON root.id=c.org_id LEFT JOIN scoped_nodes n ON n.id=c.node_id
    CROSS JOIN LATERAL (SELECT CASE WHEN c.node_id IS NULL THEN $5::jsonb->'objects'->c.id::text ELSE n."grant"->'chat' END value) page
   WHERE $8::uuid IS NULL AND ($7::uuid IS NULL OR c.id=$7)
     AND ((c.node_id IS NOT NULL AND n.id IS NOT NULL) OR (c.node_id IS NULL AND ($6::uuid IS NULL OR c.org_id=$6)))
     AND NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id=ANY(root.path) AND gone.deleted_at IS NOT NULL)
     AND (__CONTENT_CANDIDATES__ OR COALESCE((page.value->>'view')::boolean,false))
     -- A direct or group chat is searched by the people in it: reading one by inspection is an audited
     -- opening of that chat, and never a hit.
     AND (c.node_id IS NOT NULL OR EXISTS (SELECT 1 FROM chat_members cm
           WHERE cm.chat_id=c.id AND cm.user_id=$2 AND cm.left_at IS NULL))
     AND (NOT $23 OR COALESCE((page.value->>'participating')::boolean,false))
), readable_documents AS NOT MATERIALIZED (
  SELECT d.* FROM documents d JOIN nodes n ON n.id=d.node_id LEFT JOIN threads t ON t.node_id=n.id
   WHERE $7::uuid IS NULL AND ($8::uuid IS NULL OR d.id=$8) AND d.deleted_at IS NULL
     AND ($6::uuid IS NULL OR n.path @> ARRAY[$6::uuid])
     AND ($31::uuid[] IS NULL OR d.id=ANY($31))
     AND (($8 IS NOT NULL AND $6 IS NULL) OR t.side_of IS NULL)
     AND NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id=ANY(n.path) AND gone.deleted_at IS NOT NULL)
     AND (__CONTENT_CANDIDATES__ OR COALESCE(($5::jsonb->'objects'->d.id::text->>'view')::boolean,false))
     AND (NOT $23 OR COALESCE(($5::jsonb->'objects'->d.id::text->>'participating')::boolean,false))
), headers AS (
  -- A document's own fields, each with its field tier: its title, its description, the names of its files.
  SELECT d.id object_id, d.title text, d.updated_at at, d.updated_by author,
         NULL::uuid author_thread_id, NULL::text author_agent, __TITLE__ tier
    FROM readable_documents d WHERE $11 AND $22::uuid IS NULL
     AND __DOCUMENT_FILE_TYPE__
     AND (NOT $21 OR d.search_file_count>0)
  UNION ALL
  SELECT d.id, d.description, d.updated_at, d.updated_by, NULL::uuid, NULL::text, __DESCRIPTION__
    FROM readable_documents d WHERE $11 AND $22::uuid IS NULL AND d.description<>''
     AND __DOCUMENT_FILE_TYPE__
     AND (NOT $21 OR d.search_file_count>0)
  UNION ALL
  SELECT f.document_id, f.name, f.added_at, f.added_by, f.added_by_thread, NULL::text, __HEADING__
    FROM document_files f JOIN readable_documents d ON d.id=f.document_id WHERE $11 AND $22::uuid IS NULL
     AND __HEADER_FILE_TYPE__
), matching_headers AS (
  -- One hit for a document's own fields: the most telling one that matches.
  SELECT DISTINCT ON (object_id) 'document'::text kind, object_id, object_id id, NULL::uuid block_id,
         text, at, author author_id, author_thread_id, author_agent, CASE WHEN $25 THEN tier ELSE 0 END relevance
    FROM headers WHERE ($35::uuid[] IS NULL OR object_id=ANY($35)) AND __HEADERS_MATCH__ AND ($12::uuid IS NULL OR author=$12)
     AND (NOT COALESCE(($5::jsonb->'search_advanced'->>'title_only')::boolean,false) OR tier=__TITLE__)
     AND (($5::jsonb->'search_advanced'->>'date_field') IS NOT NULL OR $13::timestamptz IS NULL OR at >= $13) AND (($5::jsonb->'search_advanced'->>'date_field') IS NOT NULL OR $14::timestamptz IS NULL OR at <= $14)
   ORDER BY object_id,tier DESC,at DESC,text
), matches AS (
  SELECT 'item'::text kind,i.thread_id object_id,i.id,NULL::uuid block_id,i.search_text text,i.created_at at,
         i.author_id,NULL::uuid author_thread_id,NULL::text author_agent,0::int relevance
    FROM items i JOIN conversation_owners o ON o.thread_id=i.thread_id AND o.owner=i.continuation_id
   WHERE __ITEM_FILTER__
  UNION ALL
  SELECT 'item'::text,i.thread_id,i.id,NULL::uuid,r.body->>'search_text',i.created_at,
         i.author_id,NULL::uuid,NULL::text,0::int
    FROM thread_item_revisions r JOIN items i ON i.id=r.item_id
    JOIN conversation_owners o ON o.thread_id=i.thread_id AND o.owner=i.continuation_id
   WHERE o.frozen_at IS NOT NULL AND i.seq_updated>o.frozen_at AND r.seq_updated<=o.frozen_at
     AND __REVISION_FILE_TYPE__
     AND NOT COALESCE(($5::jsonb->'search_advanced'->>'title_only')::boolean,false)
     AND r.seq_updated=(SELECT max(latest.seq_updated) FROM thread_item_revisions latest
       WHERE latest.item_id=i.id AND latest.seq_updated<=o.frozen_at)
     AND i.seq_created BETWEEN o.first_seq AND o.last_seq
     AND (i.turn_id IS NULL OR EXISTS(SELECT 1 FROM turns t WHERE t.id=i.turn_id AND t.ordinal BETWEEN o.first_turn AND o.last_turn))
     AND i.kind=ANY($9::text[]) AND i.kind<>'marker' AND r.body->>'search_text' IS NOT NULL AND __OLD_ITEM_MATCH__
     AND ($32::uuid[] IS NULL OR i.id=ANY($32))
     AND r.body->>'search_text' NOT LIKE '%<orch>%'
     AND ($12::uuid IS NULL OR i.author_id=$12)
     AND (($5::jsonb->'search_advanced'->>'date_field') IS NOT NULL OR $13::timestamptz IS NULL OR i.created_at >= $13) AND (($5::jsonb->'search_advanced'->>'date_field') IS NOT NULL OR $14::timestamptz IS NULL OR i.created_at <= $14)
     AND (NOT $21 OR orch_search_file_types(r.body->'payload') @> ARRAY['*/*'])
     AND ($22::uuid IS NULL OR jsonb_path_exists(r.body->'payload','$.** ? (@.type == "mention" && @.user_id == $person)',jsonb_build_object('person',$22::text)))
  UNION ALL
  SELECT 'chat_message',m.chat_id,m.id,NULL::uuid,m.search_text,m.created_at,m.author_id,m.author_thread_id,m.author_agent,0
    FROM chat_messages m JOIN readable_chats r ON r.id=m.chat_id WHERE $10 AND m.deleted_at IS NULL
     AND __CHAT_FILE_TYPE__
     AND NOT COALESCE(($5::jsonb->'search_advanced'->>'title_only')::boolean,false)
     AND ($33::uuid[] IS NULL OR m.id=ANY($33))
     AND m.search_text IS NOT NULL AND __CHAT_MATCH__ AND ($12::uuid IS NULL OR m.author_id=$12)
     AND (($5::jsonb->'search_advanced'->>'date_field') IS NOT NULL OR $13::timestamptz IS NULL OR m.created_at >= $13) AND (($5::jsonb->'search_advanced'->>'date_field') IS NOT NULL OR $14::timestamptz IS NULL OR m.created_at <= $14)
     AND (NOT $21 OR orch_search_file_types(m.content) @> ARRAY['*/*'])
     AND ($22::uuid IS NULL OR $22=ANY(m.mentions))
  UNION ALL
  SELECT 'document',b.document_id,b.id,b.id,b.text,b.updated_at,b.updated_by,b.updated_by_thread,b.updated_by_agent,
         CASE WHEN $25 AND b.type='heading' THEN __HEADING__ ELSE 0 END
    FROM document_blocks b JOIN readable_documents d ON d.id=b.document_id WHERE $11 AND b.removed_at IS NULL
     AND __DOCUMENT_FILE_TYPE__
     AND NOT COALESCE(($5::jsonb->'search_advanced'->>'title_only')::boolean,false)
     AND ($34::uuid[] IS NULL OR b.id=ANY($34))
     AND __BLOCK_MATCH__ AND ($12::uuid IS NULL OR b.updated_by=$12)
     AND (($5::jsonb->'search_advanced'->>'date_field') IS NOT NULL OR $13::timestamptz IS NULL OR b.updated_at >= $13) AND (($5::jsonb->'search_advanced'->>'date_field') IS NOT NULL OR $14::timestamptz IS NULL OR b.updated_at <= $14)
     AND (NOT $21 OR d.search_file_count>0)
     AND ($22::uuid IS NULL OR jsonb_path_exists(b.content,'$.** ? (@.type == "mention" && @.user_id == $person)',jsonb_build_object('person',$22::text)))
  UNION ALL SELECT * FROM matching_headers
  UNION ALL
  -- A readable place's name or description, the most telling one that matches.
  SELECT * FROM (SELECT DISTINCT ON (n.id) 'name'::text kind,n.id object_id,n.id id,NULL::uuid block_id,x.text,
         GREATEST(n.updated_at,n.activity_at,n.thread_activity_at) at,NULL::uuid author_id,NULL::uuid author_thread_id,
         NULL::text author_agent,CASE WHEN $25 THEN x.tier ELSE 0 END relevance
    FROM scoped_nodes n CROSS JOIN LATERAL (VALUES (n.name,__TITLE__),(n.description,__DESCRIPTION__)) x(text,tier)
   WHERE $27 AND ($36::uuid[] IS NULL OR n.id=ANY($36)) AND $7::uuid IS NULL AND $8::uuid IS NULL AND $12::uuid IS NULL AND NOT $21 AND $22::uuid IS NULL
     AND $5::jsonb->'search_advanced'->>'file_type' IS NULL
     AND COALESCE((n."grant"->>'view')::boolean,false)
     AND (NOT $23 OR COALESCE((n."grant"->>'participating')::boolean,false)) AND x.text<>'' AND __NODE_MATCH__
     AND (NOT COALESCE(($5::jsonb->'search_advanced'->>'title_only')::boolean,false) OR x.tier=__TITLE__)
     AND ($5::jsonb->'search_advanced'->>'kinds' IS NULL OR n.kind='thread')
     AND (($5::jsonb->'search_advanced'->>'date_field') IS NOT NULL OR $13::timestamptz IS NULL OR GREATEST(n.updated_at,n.activity_at,n.thread_activity_at) >= $13)
     AND (($5::jsonb->'search_advanced'->>'date_field') IS NOT NULL OR $14::timestamptz IS NULL OR GREATEST(n.updated_at,n.activity_at,n.thread_activity_at) <= $14)
   ORDER BY n.id,x.tier DESC) named
), ranked AS (
  -- Best matches: the field, then how often the words occur, then the newest. Newest first: both are zero.
  SELECT m.*, CASE WHEN $25 THEN __OCCURRENCES__ ELSE 0::bigint END AS matches FROM matches m
   WHERE __ADVANCED_SOURCE__ AND __ADVANCED_SHARED__
)
SELECT __CONTENT_COLUMNS__ FROM ranked
 WHERE $15::timestamptz IS NULL OR (relevance,matches,at,id,kind,block_id IS NOT NULL)<($26::int,$29::bigint,$15,$16::uuid,$17::text,$18::boolean)
 ORDER BY relevance DESC,matches DESC,at DESC,id DESC,kind DESC,(block_id IS NOT NULL) DESC LIMIT $19
