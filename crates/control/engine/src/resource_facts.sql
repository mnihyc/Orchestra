-- Reverse references are batched by subject; explicit giving is counted once, never expanded by inheritance.
WITH units AS (
  SELECT 'host'::text AS subject_kind,r.host_id AS subject_id,'environment'::text AS kind,e.id,e.name,
    CASE WHEN e.deleted_at IS NULL THEN 'attachment' ELSE 'restore' END AS relation,e.volumes_purge_at AS until,
    CASE WHEN e.deleted_at IS NULL THEN 'unit:'||r.id::text ELSE NULL::text END AS reference_key
    FROM runtimes r JOIN environments e ON e.id=r.environment_id
    WHERE r.host_id=ANY($1) AND r.state NOT IN ('retired','removed') AND r.lost_at IS NULL AND e.lost_at IS NULL
  UNION ALL SELECT 'host',r.host_id,'provider',p.id,p.name,'attachment',NULL::timestamptz,'unit:'||r.id::text
    FROM runtimes r JOIN providers p ON p.id=r.provider_id
    WHERE r.host_id=ANY($1) AND r.state NOT IN ('retired','removed') AND r.lost_at IS NULL AND p.deleted_at IS NULL
), refs AS (
  SELECT 'provider'::text AS subject_kind,t.provider_id AS subject_id,'thread'::text AS kind,t.node_id AS id,n.name,'binding'::text AS relation,NULL::timestamptz AS until
    FROM threads t JOIN nodes n ON n.id=t.node_id WHERE t.provider_id=ANY($1)
  UNION ALL SELECT 'agent',t.agent_id,'thread',t.node_id,n.name,'binding',NULL
    FROM threads t JOIN nodes n ON n.id=t.node_id WHERE t.agent_id=ANY($1)
  UNION ALL SELECT 'environment',t.environment_id,'thread',t.node_id,n.name,'binding',NULL
    FROM threads t JOIN nodes n ON n.id=t.node_id WHERE t.environment_id=ANY($1)
  UNION ALL SELECT 'template',e.template_id,'environment',e.id,e.name,'attachment',e.volumes_purge_at
    FROM environments e WHERE e.template_id=ANY($1) AND (e.deleted_at IS NULL OR e.purged_at IS NULL)
  UNION ALL SELECT 'host',h.id,'environment',e.id,e.name,'attachment',NULL
    FROM hosts h JOIN environments e ON e.machine_id=h.machine_id
    WHERE h.id=ANY($1) AND e.deleted_at IS NULL AND e.lost_at IS NULL AND e.status NOT IN ('removed','lost')
  UNION ALL SELECT 'host',h.id,'environment',e.id,e.name,'restore',e.volumes_purge_at
    FROM hosts h JOIN environments e ON e.retained_volumes @> jsonb_build_array(jsonb_build_object('host_id',h.id))
    WHERE h.id=ANY($1) AND e.removal_phase IN ('kept','restoring','purging') AND e.purged_at IS NULL AND e.lost_at IS NULL
  UNION ALL SELECT 'host',h.id,'mcp_server',s.id,s.name,'attachment',NULL
    FROM hosts h JOIN mcp_server_offerings o ON o.machine_id=h.machine_id JOIN mcp_servers s ON s.id=o.server_id
    WHERE h.id=ANY($1) AND o.state<>'lost' AND s.deleted_at IS NULL
  UNION ALL SELECT 'host',h.id,'provider',p.id,p.name,'attachment',NULL
    FROM hosts h JOIN providers p ON p.machine_id=h.machine_id WHERE h.id=ANY($1) AND p.deleted_at IS NULL
  UNION ALL SELECT 'template',r.template_id,'provider',p.id,p.name,'attachment',NULL
    FROM runtimes r JOIN providers p ON p.id=r.provider_id WHERE r.template_id=ANY($1) AND r.state NOT IN ('retired','removed')
  UNION ALL SELECT 'machine',e.machine_id,'environment',e.id,e.name,'attachment',e.volumes_purge_at
    FROM environments e WHERE e.machine_id=ANY($1) AND (e.deleted_at IS NULL OR e.purged_at IS NULL)
  UNION ALL SELECT 'machine',p.machine_id,'provider',p.id,p.name,'attachment',NULL FROM providers p WHERE p.machine_id=ANY($1) AND p.deleted_at IS NULL
  UNION ALL SELECT 'machine',a.machine_id,'agent',a.id,a.name,'attachment',NULL FROM agents a WHERE a.machine_id=ANY($1) AND a.deleted_at IS NULL
  UNION ALL SELECT 'machine',m.machine_id,'mcp_server',m.id,m.name,'attachment',NULL FROM mcp_servers m WHERE m.machine_id=ANY($1) AND m.deleted_at IS NULL
  UNION ALL SELECT 'mount',l.mount_id,'machine',m.id,m.name,'attachment',NULL FROM mount_locations l JOIN machines m ON m.id=l.machine_id WHERE l.mount_id=ANY($1)
  UNION ALL SELECT 'mount',l.mount_id,'machine',m.id,m.name,'restore',NULL FROM mount_kept_data l JOIN machines m ON m.id=l.machine_id WHERE l.mount_id=ANY($1) AND l.lost_at IS NULL
  UNION ALL SELECT 'environment',m.environment_id,'mcp_server',m.id,m.name,'attachment',NULL FROM mcp_servers m WHERE m.environment_id=ANY($1) AND m.deleted_at IS NULL
  UNION ALL SELECT 'mcp_server',w.id,'thread',h.node_id,n.name,'attachment',NULL FROM unnest($1::uuid[]) w(id)
    JOIN segments s ON s.delivered_resources @> jsonb_build_object('mcp',jsonb_build_array(w.id))
    JOIN threads h ON h.current_segment_id=s.id JOIN nodes n ON n.id=h.node_id WHERE n.deleted_at IS NULL AND s.delivered_resources->>'state'='open'
  UNION ALL SELECT 'mcp_server',w.id,'environment',e.id,e.name,'attachment',e.volumes_purge_at FROM unnest($1::uuid[]) w(id)
    JOIN environments e ON e.facts->'mcp_servers' @> jsonb_build_array(jsonb_build_object('server_id',w.id)) WHERE e.deleted_at IS NULL
  UNION ALL SELECT 'agent',p.agent_id,'provider',p.id,p.name,'attachment',NULL FROM providers p WHERE p.agent_id=ANY($1) AND p.deleted_at IS NULL
  UNION ALL SELECT 'environment',p.held_by_environment_id,'provider',p.id,p.name,'attachment',NULL FROM providers p WHERE p.held_by_environment_id=ANY($1) AND p.deleted_at IS NULL
  UNION ALL SELECT 'machine',h.machine_id,'host',h.id,h.name,'attachment',NULL FROM hosts h WHERE h.machine_id=ANY($1)
  UNION ALL SELECT 'machine',l.machine_id,'mount',m.id,m.name,'attachment',NULL FROM mount_locations l JOIN mounts m ON m.id=l.mount_id WHERE l.machine_id=ANY($1)
  UNION ALL SELECT 'machine',l.machine_id,'mount',m.id,m.name,'restore',NULL FROM mount_kept_data l JOIN mounts m ON m.id=l.mount_id WHERE l.machine_id=ANY($1) AND l.lost_at IS NULL
  UNION ALL SELECT 'host',h.id,'mount',m.id,m.name,'attachment',NULL FROM hosts h JOIN mount_locations l ON l.machine_id=h.machine_id JOIN mounts m ON m.id=l.mount_id WHERE h.id=ANY($1)
  UNION ALL SELECT 'host',h.id,'mount',m.id,m.name,'restore',NULL FROM hosts h JOIN mount_kept_data l ON l.machine_id=h.machine_id JOIN mounts m ON m.id=l.mount_id WHERE h.id=ANY($1) AND l.lost_at IS NULL
  UNION ALL SELECT 'template',r.template_id,'environment',e.id,e.name,'restore',e.volumes_purge_at FROM runtimes r JOIN environments e ON e.id=r.environment_id
    WHERE r.template_id=ANY($1) AND r.state<>'retired' AND (e.deleted_at IS NULL OR e.purged_at IS NULL)
  UNION ALL SELECT 'mount',w.id,'environment',e.id,e.name,'attachment',e.volumes_purge_at FROM unnest($1::uuid[]) w(id)
    JOIN runtimes r ON r.volumes ? ('over:' || w.id::text) JOIN environments e ON e.id=r.environment_id
    WHERE r.state<>'retired' AND (e.deleted_at IS NULL OR e.purged_at IS NULL)
  UNION ALL SELECT 'mount',w.id,'environment',e.id,e.name,'attachment',NULL FROM unnest($1::uuid[]) w(id)
    JOIN environments e ON e.facts->'mounts' @> jsonb_build_array(jsonb_build_object('mount_id',w.id,'state','attached'))
    WHERE e.deleted_at IS NULL AND e.lost_at IS NULL
  UNION ALL SELECT DISTINCT 'skill',v.skill_id,'thread',h.node_id,n.name,'attachment',NULL::timestamptz FROM skill_versions v
    JOIN segments s ON s.delivered_resources @> jsonb_build_object('skills',jsonb_build_array(v.id)) JOIN threads h ON h.current_segment_id=s.id JOIN nodes n ON n.id=h.node_id
    WHERE v.skill_id=ANY($1) AND n.deleted_at IS NULL AND s.delivered_resources->>'state'='open'
  UNION ALL SELECT DISTINCT 'skill',v.skill_id,'thread',t.thread_id,n.name,'restore',NULL::timestamptz FROM skill_versions v
    JOIN turns t ON t.skill_versions @> ARRAY[v.id] JOIN nodes n ON n.id=t.thread_id WHERE v.skill_id=ANY($1)
), scalar_keys(subject_kind,key) AS (VALUES
  ('provider','agent.default_provider_id'),('agent','agent.default_agent_id'),('environment','env.default_id'),
  ('template','env.template_id'),('template','runtime.template_id'),('host','env.host_id'),('host','runtime.host_id')
), explicit AS (
  SELECT k.subject_kind,w.id AS subject_id,n.kind,n.id,COALESCE(n.name,'System') AS name,'explicit'::text AS relation,NULL::timestamptz AS until
    FROM unnest($1::uuid[]) w(id) CROSS JOIN scalar_keys k JOIN settings s ON s.key=k.key AND s.value @> to_jsonb(w.id::text)
    LEFT JOIN nodes n ON n.id=s.node_id WHERE n.deleted_at IS NULL
  UNION ALL SELECT 'mount',w.id,n.kind,n.id,COALESCE(n.name,'System'),'explicit',NULL FROM unnest($1::uuid[]) w(id)
    JOIN settings s ON s.key='env.mounts' AND s.value @> jsonb_build_object(w.id::text,true)
      OR s.key='env.mounts' AND s.value @> jsonb_build_object(w.id::text,'read_only')
      OR s.key='env.mounts' AND s.value @> jsonb_build_object(w.id::text,'private')
    LEFT JOIN nodes n ON n.id=s.node_id WHERE n.deleted_at IS NULL
  UNION ALL SELECT 'mcp_server',w.id,n.kind,n.id,COALESCE(n.name,'System'),'explicit',NULL FROM unnest($1::uuid[]) w(id)
    JOIN settings s ON s.key='mcp.enabled' AND s.value @> jsonb_build_object(w.id::text,true)
    LEFT JOIN nodes n ON n.id=s.node_id WHERE n.deleted_at IS NULL
  UNION ALL SELECT 'skill',w.id,n.kind,n.id,COALESCE(n.name,'System'),'explicit',NULL FROM unnest($1::uuid[]) w(id)
    JOIN settings s ON s.key='agents.skills' AND s.value @> jsonb_build_object(w.id::text,'latest')
    LEFT JOIN nodes n ON n.id=s.node_id WHERE n.deleted_at IS NULL
  UNION ALL SELECT 'skill',v.skill_id,n.kind,n.id,COALESCE(n.name,'System'),'explicit',NULL FROM skill_versions v
    JOIN settings s ON s.key='agents.skills' AND s.value @> jsonb_build_object(v.skill_id::text,v.id::text)
    LEFT JOIN nodes n ON n.id=s.node_id WHERE v.skill_id=ANY($1) AND n.deleted_at IS NULL
), own AS (
  SELECT k.subject_kind,w.id AS subject_id,'environment'::text AS kind,e.id,e.name,'explicit'::text AS relation,NULL::timestamptz AS until
    FROM unnest($1::uuid[]) w(id) CROSS JOIN scalar_keys k JOIN environments e ON e.config @> jsonb_build_object(k.key,w.id::text)
    WHERE e.deleted_at IS NULL
  UNION ALL SELECT k.subject_kind,w.id,'provider',p.id,p.name,'explicit',NULL
    FROM unnest($1::uuid[]) w(id) CROSS JOIN scalar_keys k JOIN providers p ON p.runtime_config @> jsonb_build_object(k.key,w.id::text)
    WHERE p.deleted_at IS NULL
  UNION ALL SELECT 'mount',w.id,'environment',e.id,e.name,'explicit',NULL FROM unnest($1::uuid[]) w(id)
    JOIN environments e ON e.config @> jsonb_build_object('env.mounts',jsonb_build_object(w.id::text,true))
      OR e.config @> jsonb_build_object('env.mounts',jsonb_build_object(w.id::text,'read_only'))
      OR e.config @> jsonb_build_object('env.mounts',jsonb_build_object(w.id::text,'private')) WHERE e.deleted_at IS NULL
  UNION ALL SELECT 'mcp_server',w.id,'environment',e.id,e.name,'explicit',NULL FROM unnest($1::uuid[]) w(id)
    JOIN environments e ON e.config @> jsonb_build_object('mcp.enabled',jsonb_build_object(w.id::text,true)) WHERE e.deleted_at IS NULL
)
SELECT refs.*,NULL::text AS reference_key FROM refs
UNION ALL SELECT explicit.*,NULL::text FROM explicit
UNION ALL SELECT own.*,NULL::text FROM own
UNION ALL SELECT * FROM units
