SELECT 'node'::text kind, n.id FROM nodes n WHERE n.kind <> 'thread' AND n.deleted_at IS NULL
  AND ($1::uuid IS NULL OR n.path @> ARRAY[$1::uuid])
UNION
SELECT 'node', id FROM nodes WHERE id = $1 AND deleted_at IS NULL
UNION
SELECT 'node', n.id FROM (
  SELECT object_id FROM permission_rows WHERE object_kind = 'node' AND page = 'live'
  UNION SELECT object_id FROM permission_entries WHERE object_kind = 'node' AND page = 'live'
    AND permission <> 'access.own' AND removed_at IS NULL
) custom JOIN nodes n ON n.id = custom.object_id WHERE n.kind = 'thread' AND n.deleted_at IS NULL
  AND ($1::uuid IS NULL OR n.path @> ARRAY[$1::uuid])
UNION
SELECT seed.kind, seed.id FROM documents d JOIN nodes n ON n.id = d.node_id
  CROSS JOIN LATERAL (VALUES ('node', d.node_id), ('document', d.id)) seed(kind,id)
  WHERE d.deleted_at IS NULL AND ($1::uuid IS NULL OR n.path @> ARRAY[$1::uuid])
UNION
SELECT 'environment', e.id FROM environments e WHERE e.deleted_at IS NULL AND ($1::uuid IS NULL
  OR EXISTS (SELECT 1 FROM threads t JOIN nodes n ON n.id=t.node_id
      WHERE t.environment_id=e.id AND n.path @> ARRAY[$1::uuid] AND n.deleted_at IS NULL))
UNION
SELECT 'provider', id FROM providers WHERE $1::uuid IS NULL AND deleted_at IS NULL
UNION
SELECT 'host', h.id FROM hosts h JOIN machines m ON m.id = h.machine_id WHERE $1::uuid IS NULL AND m.deleted_at IS NULL
UNION
SELECT 'template', id FROM env_templates WHERE $1::uuid IS NULL AND deleted_at IS NULL
UNION
SELECT 'mount', id FROM mounts WHERE $1::uuid IS NULL AND deleted_at IS NULL
UNION
SELECT 'machine', id FROM machines WHERE $1::uuid IS NULL AND deleted_at IS NULL
UNION
SELECT 'agent', id FROM agents WHERE $1::uuid IS NULL AND deleted_at IS NULL
UNION
SELECT 'mcp_server', id FROM mcp_servers WHERE $1::uuid IS NULL AND deleted_at IS NULL AND environment_id IS NULL
UNION
SELECT 'skill', id FROM skills WHERE $1::uuid IS NULL AND deleted_at IS NULL
UNION
SELECT 'chat', id FROM chats WHERE node_id IS NULL AND ($1::uuid IS NULL OR org_id = $1)
