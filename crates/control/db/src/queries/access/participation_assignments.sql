, subjects AS MATERIALIZED (
 SELECT 'user'::text kind,$1::uuid id UNION SELECT 'group',id FROM unnest($2::uuid[]) g(id)
), direct_subjects AS MATERIALIZED (
 -- Test a subject's indexed prefix once before expanding absent object/expiry keys.
 SELECT subject.* FROM subjects subject CROSS JOIN LATERAL (
  SELECT 1 FROM participation_direct_counts c
   WHERE c.subject_kind=subject.kind AND c.subject_id=subject.id AND c.total>0 LIMIT 1 OFFSET 0
 ) present
), summary_subjects AS MATERIALIZED (
 SELECT subject.* FROM subjects subject CROSS JOIN LATERAL (
  SELECT 1 FROM participation_node_counts c
   WHERE c.subject_kind=subject.kind AND c.subject_id=subject.id AND c.total>0 LIMIT 1 OFFSET 0
 ) present
), expiry_keys AS MATERIALIZED (
 SELECT * FROM orch_participation_after_keys($4)
), direct_requests AS MATERIALIZED (
 -- An assignment elsewhere must not cause an expiry-key probe for every requested page.
 SELECT w.kind,w.id,subject.kind AS subject_kind,subject.id AS subject_id
 FROM wanted w CROSS JOIN direct_subjects subject CROSS JOIN LATERAL (
  SELECT 1 FROM participation_direct_counts c
   WHERE c.subject_kind=subject.kind AND c.subject_id=subject.id
    AND c.object_kind=w.kind AND c.object_id=w.id AND c.total>0 LIMIT 1 OFFSET 0
 ) present
), direct AS (
 SELECT w.kind,w.id FROM direct_requests w
 CROSS JOIN LATERAL (
  SELECT o.* FROM participation_object_facts o WHERE o.object_kind=w.kind AND o.object_id=w.id AND o.live OFFSET 0
 ) o LEFT JOIN participation_paths p ON p.node_id=o.home_id
 WHERE (o.home_id IS NULL OR p.live)
  AND NOT COALESCE(p.path,'{}'::uuid[]) && $9::uuid[]
  AND ($10::uuid IS NULL OR $10=ANY(p.path))
  AND (cardinality(COALESCE(p.path,'{}'::uuid[]))=0 OR p.path[1]=ANY($11::uuid[]))
  AND EXISTS (
   SELECT 1 FROM expiry_keys t CROSS JOIN LATERAL (
    SELECT 1 FROM participation_direct_counts c
    WHERE c.subject_kind=w.subject_kind AND c.subject_id=w.subject_id
     AND c.object_kind=w.kind AND c.object_id=w.id AND c.time_shift=t.time_shift AND c.time_bucket=t.time_bucket
     AND (c.owner_org IS NULL OR c.owner_org=ANY($8::uuid[])) AND c.total>0 LIMIT 1 OFFSET 0
   ) held
  )
), intersections AS (
 SELECT a.id marker,CASE
  WHEN $10::uuid IS NULL OR p.path @> ARRAY[$10::uuid] THEN a.id
  WHEN token.path @> ARRAY[a.id] THEN token.node_id END target
 FROM unnest($5::uuid[]) a(id) JOIN participation_paths p ON p.node_id=a.id AND p.live
 LEFT JOIN participation_paths token ON token.node_id=$10::uuid AND token.live
), targets AS MATERIALIZED (
 SELECT DISTINCT i.marker,i.target FROM intersections i JOIN participation_paths p ON p.node_id=i.target AND p.live
 WHERE NOT p.path && $9::uuid[]
), blockers AS MATERIALIZED (
 SELECT DISTINCT t.marker,p.node_id FROM targets t JOIN participation_paths p
  ON p.node_id=ANY($9::uuid[]) AND p.path @> ARRAY[t.target] AND p.live
 WHERE NOT EXISTS(SELECT 1 FROM unnest($9::uuid[]) other(id)
  WHERE other.id<>p.node_id AND p.path @> ARRAY[other.id])
), count_requests AS MATERIALIZED (
 SELECT target id FROM targets UNION SELECT node_id FROM blockers
), summary_requests AS MATERIALIZED (
 SELECT r.id,subject.kind AS subject_kind,subject.id AS subject_id,root.id AS scope_root
 FROM count_requests r CROSS JOIN summary_subjects subject CROSS JOIN unnest($11::uuid[]) root(id)
 CROSS JOIN LATERAL (
  SELECT 1 FROM participation_node_counts c
   WHERE c.subject_kind=subject.kind AND c.subject_id=subject.id
    AND c.node_id=r.id AND c.scope_root=root.id AND c.total>0 LIMIT 1 OFFSET 0
 ) present
), summary_cache_capacity AS (
 -- A tiny relation fits in one bounded cache and avoids repeated scans for absent expiry keys.
 SELECT 64 AS rows
), summary_sample AS MATERIALIZED (
 SELECT * FROM participation_node_counts LIMIT (SELECT rows+1 FROM summary_cache_capacity)
), summary_cache_mode AS MATERIALIZED (
 SELECT count(*)<=(SELECT rows FROM summary_cache_capacity) cached FROM summary_sample
), totals AS MATERIALIZED (
 SELECT r.id,coalesce(sum(held.total),0)::bigint total FROM summary_requests r CROSS JOIN expiry_keys t
 LEFT JOIN LATERAL (
  SELECT c.total FROM summary_sample c
  WHERE (SELECT cached FROM summary_cache_mode)
   AND c.subject_kind=r.subject_kind AND c.subject_id=r.subject_id AND c.node_id=r.id AND c.scope_root=r.scope_root
   AND c.time_shift=t.time_shift AND c.time_bucket=t.time_bucket
   AND (c.owner_org IS NULL OR c.owner_org=ANY($8::uuid[])) AND c.total>0
  UNION ALL
  SELECT c.total FROM participation_node_counts c
  WHERE NOT (SELECT cached FROM summary_cache_mode)
   AND c.subject_kind=r.subject_kind AND c.subject_id=r.subject_id AND c.node_id=r.id AND c.scope_root=r.scope_root
   AND c.time_shift=t.time_shift AND c.time_bucket=t.time_bucket
   AND (c.owner_org IS NULL OR c.owner_org=ANY($8::uuid[])) AND c.total>0
   -- Keep each complete summary-key lookup correlated instead of scanning unrelated homes.
   OFFSET 0
 ) held ON true GROUP BY r.id
), assigned_nodes AS (
 SELECT t.marker FROM targets t JOIN totals all_sources ON all_sources.id=t.target
 LEFT JOIN blockers b ON b.marker=t.marker LEFT JOIN totals blocked ON blocked.id=b.node_id
 GROUP BY t.marker,all_sources.total HAVING all_sources.total-coalesce(sum(blocked.total),0)>0
)
SELECT kind,id FROM direct
UNION SELECT 'node',marker FROM assigned_nodes
