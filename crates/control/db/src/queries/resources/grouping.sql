WITH wanted(kind,id) AS (SELECT DISTINCT * FROM unnest($1::text[],$2::uuid[]))
SELECT w.kind,e.id,e.home_node_id home_id,e.home_overridden_at,e.machine_id origin_machine_id,
       e.name,e.revision,NULL::uuid container_environment_id,e.owner_id owner_id
  FROM wanted w JOIN environments e ON w.kind='environment' AND e.id=w.id WHERE e.deleted_at IS NULL
UNION ALL
SELECT w.kind,p.id,p.home_id,p.home_overridden_at,COALESCE(p.key_machine_id,p.machine_id),p.name,p.revision,NULL::uuid,p.owner_id
  FROM wanted w JOIN providers p ON w.kind='provider' AND p.id=w.id WHERE p.deleted_at IS NULL
UNION ALL
SELECT w.kind,a.id,a.home_id,a.home_overridden_at,a.machine_id,a.name,a.revision,NULL::uuid,a.owner_id
  FROM wanted w JOIN agents a ON w.kind='agent' AND a.id=w.id WHERE a.deleted_at IS NULL
UNION ALL
SELECT w.kind,s.id,s.home_id,s.home_overridden_at,s.machine_id,s.name,s.revision,s.environment_id,s.owner_id
  FROM wanted w JOIN mcp_servers s ON w.kind='mcp_server' AND s.id=w.id WHERE s.deleted_at IS NULL
UNION ALL
SELECT w.kind,h.id,h.home_id,h.home_overridden_at,h.machine_id,h.name,h.revision,NULL::uuid,h.owner_id
  FROM wanted w JOIN hosts h ON w.kind='host' AND h.id=w.id JOIN machines m ON m.id=h.machine_id
 WHERE m.deleted_at IS NULL
UNION ALL
SELECT w.kind,m.id,CASE WHEN h.id IS NULL THEN m.home_id ELSE h.home_id END,
       CASE WHEN h.id IS NULL THEN m.home_overridden_at ELSE h.home_overridden_at END,NULL::uuid,
       m.name,m.revision,NULL::uuid,m.owner_id
  FROM wanted w JOIN machines m ON w.kind='machine' AND m.id=w.id LEFT JOIN hosts h ON h.machine_id=m.id
 WHERE m.runtime_id IS NULL AND m.deleted_at IS NULL
UNION ALL
SELECT w.kind,t.id,t.home_id,t.home_overridden_at,NULL::uuid,t.name,t.revision,NULL::uuid,t.owner_id
  FROM wanted w JOIN env_templates t ON w.kind='template' AND t.id=w.id WHERE t.deleted_at IS NULL
UNION ALL
SELECT w.kind,m.id,m.home_id,m.home_overridden_at,NULL::uuid,m.name,m.revision,NULL::uuid,m.owner_id
  FROM wanted w JOIN mounts m ON w.kind='mount' AND m.id=w.id WHERE m.deleted_at IS NULL
UNION ALL
SELECT w.kind,s.id,s.home_id,s.home_overridden_at,NULL::uuid,s.name,s.revision,NULL::uuid,s.owner_id
  FROM wanted w JOIN skills s ON w.kind='skill' AND s.id=w.id WHERE s.deleted_at IS NULL
