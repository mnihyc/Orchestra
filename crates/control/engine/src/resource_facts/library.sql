WITH wanted AS MATERIALIZED (
 SELECT * FROM jsonb_to_recordset($1) AS w(ordinal bigint,kind text,id uuid,version uuid,unit text)
), counts AS (
 SELECT w.ordinal, COALESCE(max(r.uses) FILTER (WHERE r.day IS NULL),0)::bigint all_time,
   COALESCE(sum(r.uses) FILTER (WHERE r.day BETWEEN ($2::timestamptz AT TIME ZONE 'UTC')::date-29 AND ($2::timestamptz AT TIME ZONE 'UTC')::date),0)::bigint recent,
   max(r.last_at) FILTER (WHERE r.day IS NULL) last_at
   ,COALESCE(max(r.unattributed) FILTER (WHERE r.day IS NULL),0)>0 unknown_people
   ,COALESCE(sum(r.unattributed) FILTER (WHERE r.day BETWEEN ($2::timestamptz AT TIME ZONE 'UTC')::date-29 AND ($2::timestamptz AT TIME ZONE 'UTC')::date),0)>0 unknown_recent_people
 FROM wanted w LEFT JOIN resource_use_rollups r ON (r.subject_kind,r.subject_id,r.use_kind)=(w.kind,w.id,w.unit) AND r.version_id IS NOT DISTINCT FROM w.version
 GROUP BY w.ordinal
), people AS (
 SELECT w.ordinal,count(p.person_id)::bigint people,
   count(p.person_id) FILTER (WHERE p.last_at >= (($2::timestamptz AT TIME ZONE 'UTC')::date-29)::timestamp AT TIME ZONE 'UTC')::bigint recent_people
 FROM wanted w LEFT JOIN resource_use_people p ON (p.subject_kind,p.subject_id,p.use_kind)=(w.kind,w.id,w.unit) AND p.version_id IS NOT DISTINCT FROM w.version
 GROUP BY w.ordinal
)
SELECT c.*,p.people,p.recent_people,s.since FROM counts c JOIN people p USING(ordinal) CROSS JOIN resource_use_recording s
