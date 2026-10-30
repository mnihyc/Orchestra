WITH candidates AS (
  SELECT candidate.position FROM unnest($2::text[]) requested(id)
  CROSS JOIN LATERAL (
    SELECT position FROM thread_pending_notices
    WHERE thread_id=$1 AND notice_id=requested.id ORDER BY position LIMIT 64
  ) candidate
), delivered AS (SELECT position FROM candidates ORDER BY position LIMIT 64)
DELETE FROM thread_pending_notices WHERE thread_id=$1 AND position IN(SELECT position FROM delivered)
