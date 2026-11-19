WITH selected_nodes AS (
 (SELECT n.id FROM nodes n WHERE n.kind<>'thread' AND n.deleted_at IS NULL
   AND ($1::uuid IS NULL OR n.path @> ARRAY[$1::uuid])
   AND cardinality($2::uuid[])=0 AND cardinality($3::uuid[])=0 AND cardinality($4::uuid[])=0
 ORDER BY n.id LIMIT $5)
 UNION SELECT n.id FROM nodes n WHERE n.id=$1 OR n.id=ANY($4::uuid[])
 UNION SELECT d.node_id FROM documents d WHERE d.id=ANY($2::uuid[])
 UNION SELECT COALESCE(c.node_id,c.org_id) FROM chats c WHERE c.id=ANY($3::uuid[])
 UNION (SELECT DISTINCT n.id FROM permission_rows p JOIN nodes n ON n.id=p.object_id
 WHERE p.object_kind='node' AND p.page='live' AND n.kind='thread' AND n.deleted_at IS NULL
   AND ($1::uuid IS NULL OR n.path @> ARRAY[$1::uuid])
   AND cardinality($2::uuid[])=0 AND cardinality($3::uuid[])=0 AND cardinality($4::uuid[])=0
 ORDER BY n.id LIMIT $5)
 UNION (SELECT DISTINCT n.id FROM permission_entries e JOIN nodes n ON n.id=e.object_id
 WHERE e.object_kind='node' AND e.page='live' AND e.permission<>'access.own' AND e.removed_at IS NULL
   AND n.kind='thread' AND n.deleted_at IS NULL AND ($1::uuid IS NULL OR n.path @> ARRAY[$1::uuid])
   AND cardinality($2::uuid[])=0 AND cardinality($3::uuid[])=0 AND cardinality($4::uuid[])=0
 ORDER BY n.id LIMIT $5)
)
SELECT 'node'::text kind,id FROM selected_nodes
UNION SELECT 'document',id FROM documents WHERE id=ANY($2::uuid[]) AND deleted_at IS NULL
UNION SELECT 'chat',id FROM chats WHERE id=ANY($3::uuid[]) AND node_id IS NULL
UNION SELECT 'environment',e.id FROM environments e JOIN threads t ON t.environment_id=e.id
 WHERE t.node_id IN(SELECT id FROM selected_nodes) AND e.deleted_at IS NULL
LIMIT $5
