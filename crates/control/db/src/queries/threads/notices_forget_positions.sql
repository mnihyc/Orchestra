DELETE FROM thread_pending_notices
WHERE thread_id=$1 AND position=ANY($2::bigint[])
