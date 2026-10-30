-- Progress needs aggregate memberships, not a full loss inventory and its thread history.
WITH subject AS (
  SELECT h.name,h.machine_id,m.online FROM hosts h JOIN machines m ON m.id=h.machine_id
  WHERE h.id=$1 AND m.deleted_at IS NULL
), live AS (
  SELECT e.id FROM environments e
    WHERE e.machine_id=(SELECT machine_id FROM subject) AND e.purged_at IS NULL AND e.status NOT IN ('removed','lost')
  UNION
  SELECT e.id FROM runtimes r JOIN environments e ON e.id=r.environment_id
    WHERE r.host_id=$1 AND r.successor_of IS NULL AND r.lost_at IS NULL AND r.state NOT IN ('retired','removed')
      AND e.purged_at IS NULL AND e.status NOT IN ('removed','lost')
  UNION
  SELECT e.id FROM environments e
    WHERE e.purged_at IS NULL AND e.status NOT IN ('removed','lost')
      AND e.removal_phase IN ('kept','restoring','purging','lost')
      AND e.retained_volumes @> jsonb_build_array(jsonb_build_object('host_id',$1::uuid::text))
), retention AS (
  SELECT count(*) FILTER(WHERE e.removal_phase IN ('kept','restoring','purging')) AS kept,
    min(e.volumes_purge_at) FILTER(WHERE e.volumes_purge_at>$2) AS until
  FROM environments e
  WHERE e.removal_phase IN ('removing','kept','restoring','purging')
    AND e.retained_volumes @> jsonb_build_array(jsonb_build_object('host_id',$1::uuid::text))
), runtime AS (
  SELECT count(*) AS runtimes,min(remove_at) FILTER(WHERE state='retired' AND remove_at>$2) AS until
  FROM runtimes WHERE host_id=$1 AND lost_at IS NULL AND state<>'removed'
)
SELECT (SELECT count(*) FROM live) AS environments,retention.kept,runtime.runtimes,
       LEAST(retention.until,runtime.until) AS until,subject.online,subject.name
FROM subject CROSS JOIN retention CROSS JOIN runtime
