, subjects AS (
 SELECT 'user'::text AS kind,$1::uuid AS id UNION ALL SELECT 'group',id FROM unnest($2::uuid[]) g(id)
), candidates AS MATERIALIZED (
 -- Probe the subject/node/root prefix before asking which expiry buckets are current.
 SELECT p.node_id,subject.kind,subject.id,root.id AS scope_root
 FROM participation_paths p CROSS JOIN subjects subject CROSS JOIN unnest($11::uuid[]) root(id)
 CROSS JOIN LATERAL (
  SELECT 1 FROM participation_node_counts c
  WHERE c.subject_kind=subject.kind AND c.subject_id=subject.id AND c.node_id=p.node_id
   AND c.scope_root=root.id AND c.total>0 LIMIT 1
 ) present
 WHERE p.node_id=ANY($5::uuid[]) AND p.live
), expiry_keys AS MATERIALIZED (
 SELECT * FROM orch_participation_after_keys($4)
)
SELECT 'node' AS kind,p.node_id AS id FROM candidates p WHERE EXISTS (
 SELECT 1 FROM expiry_keys t JOIN participation_node_counts c
 ON c.subject_kind=p.kind AND c.subject_id=p.id AND c.node_id=p.node_id AND c.scope_root=p.scope_root
 AND c.time_shift=t.time_shift AND c.time_bucket=t.time_bucket
 WHERE (c.owner_org IS NULL OR c.owner_org=ANY($8::uuid[])) AND c.total>0
)
