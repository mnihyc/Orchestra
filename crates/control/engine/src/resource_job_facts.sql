-- One accepted job is one current dependency, regardless of its direct and recipe memberships.
WITH wanted AS (
  SELECT 'provider'::text AS kind,id,name FROM providers WHERE id=ANY($1)
  UNION ALL SELECT 'agent',id,name FROM agents WHERE id=ANY($1)
  UNION ALL SELECT 'environment',id,name FROM environments WHERE id=ANY($1)
  UNION ALL SELECT 'host',id,name FROM hosts WHERE id=ANY($1)
  UNION ALL SELECT 'template',id,name FROM env_templates WHERE id=ANY($1)
  UNION ALL SELECT 'mount',id,name FROM mounts WHERE id=ANY($1)
  UNION ALL SELECT 'skill',id,name FROM skills WHERE id=ANY($1)
), active AS MATERIALIZED (
  SELECT id,kind,provider_id,environment_id,plan FROM jobs WHERE state IN ('queued','running')
    OR (kind='workspace_restore' AND state='failed' AND plan->>'restore_recovery_required'='true')
), memberships AS (
  SELECT j.id AS job_id,r.kind,r.id FROM active j CROSS JOIN LATERAL (VALUES
    ('provider'::text,j.provider_id::text),('environment',j.environment_id::text),
    ('host',j.plan->>'host'),('host',j.plan#>>'{cascade,host}'),('template',j.plan->>'template'),
    (j.plan#>>'{quiesce,target,object,kind}',j.plan#>>'{quiesce,target,object,id}')
  ) r(kind,id)
  UNION ALL SELECT j.id,
    CASE w.value#>>'{target,kind}' WHEN 'native' THEN 'host' WHEN 'environment' THEN 'environment'
      WHEN 'skill_version' THEN 'skill' WHEN 'mount_data' THEN 'mount' END,
    CASE w.value#>>'{target,kind}' WHEN 'native' THEN w.value#>>'{target,host}'
      WHEN 'environment' THEN w.value#>>'{target,environment}' WHEN 'skill_version' THEN w.value#>>'{target,skill}'
      WHEN 'mount_data' THEN w.value#>>'{target,mount}' END
    FROM active j CROSS JOIN LATERAL jsonb_array_elements(CASE WHEN jsonb_typeof(j.plan#>'{recipe,work}')='array' THEN j.plan#>'{recipe,work}' ELSE '[]'::jsonb END) w(value)
    WHERE j.kind='storage_cleanup' AND w.value->>'choice'<>'keep'
  UNION ALL SELECT j.id,'mount',w.value#>>'{location,mount_id}'
    FROM active j CROSS JOIN LATERAL jsonb_array_elements(CASE WHEN jsonb_typeof(j.plan#>'{cascade,custody,mounts}')='array' THEN j.plan#>'{cascade,custody,mounts}' ELSE '[]'::jsonb END) w(value)
    WHERE j.kind='host_delete' AND w.value->>'fate' IN ('move','delete')
  UNION ALL SELECT j.id,'host',w.value#>>'{row,host,id}'
    FROM active j CROSS JOIN LATERAL jsonb_array_elements(CASE WHEN jsonb_typeof(j.plan#>'{cascade,custody,mounts}')='array' THEN j.plan#>'{cascade,custody,mounts}' ELSE '[]'::jsonb END) w(value)
    WHERE j.kind='host_delete' AND w.value->>'fate'='move'
)
SELECT DISTINCT w.kind AS subject_kind,w.id AS subject_id,w.kind,w.id,w.name,
  'operation'::text AS relation,NULL::timestamptz AS until,'job:'||m.job_id::text AS reference_key
  FROM memberships m JOIN wanted w ON m.kind=w.kind AND m.id=w.id::text
