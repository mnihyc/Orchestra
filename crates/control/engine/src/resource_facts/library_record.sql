WITH eligible AS MATERIALIZED (
 SELECT id FROM turns WHERE id=$8 AND started_at IS NOT NULL AND state IN ('starting','running')
   AND NOT native_end_confirmed AND (native_turn_ref IS NULL OR native_turn_ref NOT LIKE 'fork-copy:%')
 FOR UPDATE
), receipt AS (
 INSERT INTO resource_use_receipts(subject_kind,subject_id,version_id,use_kind,source,at,person_id,turn_id)
 SELECT $1,$2,$3,$4,$5,$6,$7,$8
 WHERE EXISTS(SELECT 1 FROM eligible) OR ($8::uuid IS NULL AND $4 IN ('environment_made','provider_made'))
 ON CONFLICT DO NOTHING RETURNING *
), versions AS (
 SELECT r.*,v.version FROM receipt r CROSS JOIN LATERAL (
   SELECT r.version_id version UNION SELECT NULL::uuid
 ) v
), buckets AS (
 SELECT v.*,d.day FROM versions v CROSS JOIN LATERAL (
   SELECT (v.at AT TIME ZONE 'UTC')::date AS day UNION ALL SELECT NULL::date
 ) d
), rolled AS (
 INSERT INTO resource_use_rollups(subject_kind,subject_id,version_id,use_kind,day,uses,unattributed,last_at)
 SELECT subject_kind,subject_id,version,use_kind,day,1,CASE WHEN person_id IS NULL THEN 1 ELSE 0 END,at FROM buckets
 ON CONFLICT (subject_kind,subject_id,version_id,use_kind,day) DO UPDATE
 SET uses=resource_use_rollups.uses+1,unattributed=resource_use_rollups.unattributed+EXCLUDED.unattributed,last_at=GREATEST(resource_use_rollups.last_at,EXCLUDED.last_at)
), expired_days AS (
 DELETE FROM resource_use_rollups old USING versions v
 WHERE (old.subject_kind,old.subject_id,old.use_kind)=(v.subject_kind,v.subject_id,v.use_kind)
   AND old.version_id IS NOT DISTINCT FROM v.version AND old.day<(v.at AT TIME ZONE 'UTC')::date-29
)
INSERT INTO resource_use_people(subject_kind,subject_id,version_id,use_kind,person_id,last_at)
SELECT subject_kind,subject_id,version,use_kind,person_id,at FROM versions WHERE person_id IS NOT NULL
ON CONFLICT (subject_kind,subject_id,version_id,use_kind,person_id) DO UPDATE SET last_at=GREATEST(resource_use_people.last_at,EXCLUDED.last_at)
