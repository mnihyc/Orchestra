SELECT position,notice FROM thread_pending_notices
WHERE thread_id=$1 AND position>$2 ORDER BY position LIMIT 64
