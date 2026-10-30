WITH wanted AS (
 SELECT * FROM jsonb_to_recordset($1) AS w(ordinal bigint,kind text,id uuid)
)
SELECT w.ordinal,r.uses FROM wanted w
LEFT JOIN env_templates t ON w.kind='template' AND t.id=w.id
JOIN resource_use_rollups r ON (r.subject_kind,r.subject_id)=(w.kind,w.id)
 AND r.version_id IS NULL AND r.day IS NULL
 AND r.use_kind=CASE w.kind
   WHEN 'mcp_server' THEN 'mcp_call'
   WHEN 'skill' THEN 'skill_invocation'
   WHEN 'host' THEN 'environment_made'
   WHEN 'template' THEN CASE WHEN t.kind='provider' THEN 'provider_made' ELSE 'environment_made' END
 END
