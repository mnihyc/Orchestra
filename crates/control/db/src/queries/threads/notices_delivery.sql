SELECT queued.position,queued.notice
FROM unnest($2::bigint[]) WITH ORDINALITY delivered(position,ordinal)
JOIN thread_pending_notices queued ON queued.thread_id=$1 AND queued.position=delivered.position
ORDER BY delivered.ordinal
