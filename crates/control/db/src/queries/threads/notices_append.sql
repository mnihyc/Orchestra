WITH parent AS MATERIALIZED (SELECT node_id FROM threads WHERE node_id=$1 FOR UPDATE)
INSERT INTO thread_pending_notices(thread_id,notice,dedup_key)
SELECT parent.node_id,$2::jsonb,$2::jsonb->>'id' FROM parent
WHERE $2::jsonb->>'id' IS NULL OR NOT EXISTS(
  SELECT 1 FROM thread_pending_notices WHERE thread_id=$1 AND notice_id=$2::jsonb->>'id')
ON CONFLICT(thread_id,dedup_key) DO NOTHING
