-- The same resource identities, physical-machine facade and deleted-home exclusion as Everything.
-- Grant.listed comes from the access engine; a name-only permission page counts, private unseen rows do not.
WITH resources AS (
  SELECT 'environment'::text kind, id, home_node_id home, id grant_id FROM environments WHERE deleted_at IS NULL
  UNION ALL SELECT 'provider', id, home_id, id FROM providers WHERE deleted_at IS NULL
  UNION ALL SELECT 'agent', id, home_id, id FROM agents WHERE deleted_at IS NULL
  UNION ALL SELECT 'template', id, home_id, id FROM env_templates WHERE deleted_at IS NULL
  UNION ALL SELECT 'mcp_server', id, home_id, id FROM mcp_servers WHERE deleted_at IS NULL AND environment_id IS NULL
  UNION ALL SELECT 'mount', id, home_id, id FROM mounts WHERE deleted_at IS NULL
  UNION ALL SELECT 'skill', id, home_id, id FROM skills WHERE deleted_at IS NULL
  UNION ALL SELECT 'host', h.id, h.home_id, h.id FROM hosts h JOIN machines m ON m.id=h.machine_id WHERE m.deleted_at IS NULL
  UNION ALL SELECT 'machine', m.id, CASE WHEN h.id IS NULL THEN m.home_id ELSE h.home_id END, COALESCE(h.id,m.id)
    FROM machines m LEFT JOIN hosts h ON h.machine_id=m.id WHERE m.deleted_at IS NULL AND m.runtime_id IS NULL
)
SELECT r.kind, count(*)::bigint
  FROM resources r LEFT JOIN nodes n ON n.id=r.home
 WHERE COALESCE(($1::jsonb->'objects'->r.grant_id::text->>'listed')::boolean,false)
   AND (n.id IS NULL OR NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id=ANY(n.path) AND gone.deleted_at IS NOT NULL))
 GROUP BY r.kind
