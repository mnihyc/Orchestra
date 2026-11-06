-- Direct participation for selected resource pages; permission evaluation stays in the caller's frame.
WITH wanted AS MATERIALIZED (
 SELECT * FROM unnest($4::text[],$5::uuid[]) w(kind,id)
), subjects AS MATERIALIZED (
 SELECT 'user'::text kind,$1::uuid id UNION SELECT 'group',id FROM unnest($2::uuid[]) g(id)
), expiry_keys AS MATERIALIZED (
 SELECT * FROM orch_participation_after_keys($3)
), direct AS (
 SELECT w.kind,w.id FROM wanted w
 CROSS JOIN LATERAL (
  SELECT o.* FROM participation_object_facts o WHERE o.object_kind=w.kind AND o.object_id=w.id AND o.live OFFSET 0
 ) o LEFT JOIN participation_paths p ON p.node_id=o.home_id
 WHERE (o.home_id IS NULL OR p.live)
  AND NOT COALESCE(p.path,'{}'::uuid[]) && $7::uuid[]
  AND ($8::uuid IS NULL OR $8=ANY(p.path))
  AND (cardinality(COALESCE(p.path,'{}'::uuid[]))=0 OR p.path[1]=ANY($9::uuid[]))
  AND EXISTS (
   SELECT 1 FROM subjects subject CROSS JOIN expiry_keys t CROSS JOIN LATERAL (
    SELECT 1 FROM participation_direct_counts c
    WHERE c.subject_kind=subject.kind AND c.subject_id=subject.id
     AND c.object_kind=w.kind AND c.object_id=w.id AND c.time_shift=t.time_shift AND c.time_bucket=t.time_bucket
     AND (c.owner_org IS NULL OR c.owner_org=ANY($6::uuid[])) AND c.total>0 LIMIT 1 OFFSET 0
   ) held
  )
)
SELECT DISTINCT kind,id FROM direct
