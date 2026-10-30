SELECT position,notice FROM (
  (SELECT position,notice,1 AS priority FROM thread_pending_notices
   WHERE thread_id=$1 AND lane_key=''::bytea ORDER BY position LIMIT 64)
  UNION ALL
  (SELECT position,notice,0 AS priority FROM thread_pending_notices
   WHERE thread_id=$1 AND lane_key=(
     SELECT orch_pending_notice_lane(s) FROM segments s WHERE s.id=$2 AND s.thread_id=$1)
   ORDER BY position LIMIT 64)
) lanes ORDER BY priority,position LIMIT 64
