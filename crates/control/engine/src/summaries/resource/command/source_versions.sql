WITH RECURSIVE wanted AS (
    SELECT DISTINCT kind,id FROM jsonb_to_recordset($1) AS t(kind text,id uuid)
), places AS (
    SELECT id AS node FROM wanted WHERE kind IN ('thread','topic')
    UNION SELECT d.node_id FROM documents d JOIN wanted w ON w.kind='document' AND w.id=d.id
    UNION SELECT COALESCE(c.node_id,c.home_id,c.org_id) FROM chats c
        JOIN wanted w ON w.kind IN ('chat','chat_title') AND w.id=c.id
), mask_environments AS (
    SELECT DISTINCT e.id,e.home_node_id,e.xmin::text AS version FROM environments e
        JOIN threads t ON t.environment_id=e.id WHERE t.node_id IN (SELECT node FROM places)
), ancestors AS (
    SELECT DISTINCT unnest(n.path) AS id FROM nodes n
        WHERE n.id IN (SELECT node FROM places UNION SELECT home_node_id FROM mask_environments)
), folders AS (
    SELECT d.parent_container_id AS id FROM documents d JOIN wanted w ON w.kind='document' AND w.id=d.id
    UNION SELECT c.parent_container_id FROM document_containers c JOIN folders f ON f.id=c.id
), versions AS (
    SELECT 'node' AS kind,n.id::text AS id,n.xmin::text AS version FROM nodes n JOIN ancestors a ON a.id=n.id
    UNION ALL SELECT 'thread',t.node_id::text,t.xmin::text FROM threads t WHERE t.node_id IN (SELECT node FROM places)
    UNION ALL SELECT 'document',d.id::text,d.xmin::text FROM documents d JOIN wanted w ON w.kind='document' AND w.id=d.id
    UNION ALL SELECT 'folder',c.id::text,c.xmin::text FROM document_containers c JOIN folders f ON f.id=c.id
    UNION ALL SELECT 'chat',c.id::text,c.xmin::text FROM chats c JOIN wanted w ON w.kind IN ('chat','chat_title') AND w.id=c.id
    UNION ALL SELECT 'document_scope',d.node_id::text,COALESCE(s.seq,0)::text FROM documents d
        JOIN wanted w ON w.kind='document' AND w.id=d.id LEFT JOIN document_tree_scopes s ON s.node_id=d.node_id
    UNION ALL SELECT 'setting',COALESCE(s.node_id::text,'system') || ':' || s.key,s.xmin::text
        FROM settings s WHERE s.node_id IS NULL OR s.node_id IN (SELECT id FROM ancestors)
    UNION ALL SELECT 'mask_environment',e.id::text,e.version FROM mask_environments e
    -- Only row identity/version is needed to repeat masking; no name or value enters this record.
    UNION ALL SELECT 'mask_input',s.id::text,s.xmin::text FROM secrets s
        WHERE (s.owner_kind='setting' AND s.owner_id IN (SELECT id FROM ancestors))
           OR (s.owner_kind='environment' AND s.owner_id IN (SELECT id FROM mask_environments))
    UNION ALL SELECT 'request',r.id::text,r.xmin::text FROM resource_summary_requests r WHERE r.id=$2
    UNION ALL SELECT 'paragraph',s.id::text,s.xmin::text FROM resource_summaries s
        JOIN resource_summary_requests r ON r.summary_id=s.id WHERE r.id=$2
    UNION ALL SELECT 'provider',p.id::text,p.xmin::text FROM providers p WHERE p.id=$3
)
SELECT COALESCE(jsonb_agg(jsonb_build_array(kind,id,version) ORDER BY kind,id,version),'[]'::jsonb)
FROM versions
