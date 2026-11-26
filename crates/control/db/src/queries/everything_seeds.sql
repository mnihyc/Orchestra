-- $2 is every kind (NULL) or only the resource kinds a resource list shows: then neither places, documents nor chats are
-- seeds; the homes of those resources and the places above them come with the resources, and the places of their tags here.
SELECT 'node'::text kind, n.id FROM nodes n WHERE $2::text[] IS NULL AND n.kind <> 'thread' AND n.deleted_at IS NULL
  AND ($1::uuid IS NULL OR n.path @> ARRAY[$1::uuid])
UNION
SELECT 'node', id FROM nodes WHERE $2::text[] IS NULL AND id = $1 AND deleted_at IS NULL
UNION
SELECT 'node', n.id FROM (
  SELECT object_id FROM permission_rows WHERE object_kind = 'node' AND page = 'live'
  UNION SELECT object_id FROM permission_entries WHERE object_kind = 'node' AND page = 'live'
    AND permission <> 'access.own' AND removed_at IS NULL
) custom JOIN nodes n ON n.id = custom.object_id WHERE $2::text[] IS NULL AND n.kind = 'thread' AND n.deleted_at IS NULL
  AND ($1::uuid IS NULL OR n.path @> ARRAY[$1::uuid])
UNION
SELECT seed.kind, seed.id FROM documents d JOIN nodes n ON n.id = d.node_id
  CROSS JOIN LATERAL (VALUES ('node', d.node_id), ('document', d.id)) seed(kind,id)
  WHERE $2::text[] IS NULL AND d.deleted_at IS NULL AND ($1::uuid IS NULL OR n.path @> ARRAY[$1::uuid])
UNION
SELECT 'node', n.id FROM resource_tags rt JOIN tags tag ON tag.id = rt.tag_id JOIN nodes n ON n.id = tag.node_id
  WHERE rt.resource_kind = ANY($2::text[]) AND n.deleted_at IS NULL
UNION
SELECT 'environment', e.id FROM environments e WHERE ($2::text[] IS NULL OR 'environment' = ANY($2::text[]))
  AND e.deleted_at IS NULL AND ($1::uuid IS NULL
  OR EXISTS (SELECT 1 FROM threads t JOIN nodes n ON n.id=t.node_id
      WHERE t.environment_id=e.id AND n.path @> ARRAY[$1::uuid] AND n.deleted_at IS NULL))
UNION
SELECT 'provider', id FROM providers WHERE ($2::text[] IS NULL OR 'provider' = ANY($2::text[])) AND $1::uuid IS NULL AND deleted_at IS NULL
UNION
SELECT 'host', h.id FROM hosts h JOIN machines m ON m.id = h.machine_id WHERE ($2::text[] IS NULL OR 'host' = ANY($2::text[]))
  AND $1::uuid IS NULL AND m.deleted_at IS NULL
UNION
SELECT 'template', id FROM env_templates WHERE ($2::text[] IS NULL OR 'template' = ANY($2::text[])) AND $1::uuid IS NULL AND deleted_at IS NULL
UNION
SELECT 'mount', id FROM mounts WHERE ($2::text[] IS NULL OR 'mount' = ANY($2::text[])) AND $1::uuid IS NULL AND deleted_at IS NULL
UNION
SELECT 'machine', id FROM machines WHERE ($2::text[] IS NULL OR 'machine' = ANY($2::text[])) AND $1::uuid IS NULL AND deleted_at IS NULL
UNION
SELECT 'agent', id FROM agents WHERE ($2::text[] IS NULL OR 'agent' = ANY($2::text[])) AND $1::uuid IS NULL AND deleted_at IS NULL
UNION
SELECT 'mcp_server', id FROM mcp_servers WHERE ($2::text[] IS NULL OR 'mcp_server' = ANY($2::text[])) AND $1::uuid IS NULL
  AND deleted_at IS NULL AND environment_id IS NULL
UNION
SELECT 'skill', id FROM skills WHERE ($2::text[] IS NULL OR 'skill' = ANY($2::text[])) AND $1::uuid IS NULL AND deleted_at IS NULL
UNION
SELECT 'chat', c.id FROM chats c JOIN nodes home ON home.id=COALESCE(c.home_id,c.org_id) WHERE $2::text[] IS NULL
  AND c.node_id IS NULL AND ($1::uuid IS NULL OR home.path @> ARRAY[$1::uuid])

UNION
SELECT 'pool', id FROM pools WHERE ($2::text[] IS NULL OR 'pool' = ANY($2::text[])) AND $1::uuid IS NULL AND deleted_at IS NULL
