-- Accepted auxiliary work retains its source even before its native command is dispatched.
WITH open_requests AS (
  SELECT c.id,p.id AS provider_id,c.body->>'model_name' AS model_name,p.agent_id,p.held_by_environment_id AS environment_id
    FROM commands c JOIN providers p ON c.body->>'provider_id'=p.id::text
    WHERE c.kind IN ('provider.complete','provider.embed') AND c.state IN ('queued','sent','accepted')
      AND NOT EXISTS(SELECT 1 FROM summary_calls accepted WHERE accepted.id=c.id)
  UNION ALL
  SELECT accepted.id,accepted.provider_id,accepted.model_name,
    COALESCE(accepted.agent_id,s.agent_id,t.agent_id,p.agent_id),
    COALESCE(accepted.environment_id,s.environment_id,t.environment_id,p.held_by_environment_id)
    FROM summary_calls accepted JOIN providers p ON p.id=accepted.provider_id
      LEFT JOIN commands c ON c.id=accepted.id
      LEFT JOIN threads t ON t.node_id=accepted.thread_id
      LEFT JOIN segments s ON s.id=t.current_segment_id
    WHERE accepted.cancel_reason IS NULL AND (c.id IS NULL OR c.state IN ('queued','sent','accepted'))
)
SELECT DISTINCT id FROM open_requests
WHERE (($1='provider' AND provider_id=$2) OR ($1='agent' AND agent_id=$2) OR ($1='environment' AND environment_id=$2))
  AND ($3::uuid IS NULL OR model_name=(SELECT model_name FROM provider_models WHERE id=$3 AND provider_id=$2))
