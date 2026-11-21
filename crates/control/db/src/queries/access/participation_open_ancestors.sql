-- All descendants of each live target are in reach. One positive eligible bucket marks that target.
summary_requests AS MATERIALIZED (
 SELECT p.node_id,subject.kind AS subject_kind,subject.id AS subject_id,root.id AS scope_root
 FROM participation_paths p CROSS JOIN summary_subjects subject CROSS JOIN unnest($11::uuid[]) root(id)
 CROSS JOIN LATERAL (
  SELECT 1 FROM participation_node_counts c
   WHERE c.subject_kind=subject.kind AND c.subject_id=subject.id
    AND c.node_id=p.node_id AND c.scope_root=root.id AND c.total>0 LIMIT 1 OFFSET 0
 ) present
 WHERE p.node_id=ANY($5::uuid[]) AND p.live
), assigned_nodes AS (
 SELECT r.node_id AS marker FROM summary_requests r WHERE EXISTS (
  SELECT 1 FROM expiry_keys t CROSS JOIN LATERAL (
   SELECT 1 FROM participation_node_counts c
    WHERE c.subject_kind=r.subject_kind AND c.subject_id=r.subject_id
     AND c.node_id=r.node_id AND c.scope_root=r.scope_root
     AND c.time_shift=t.time_shift AND c.time_bucket=t.time_bucket
     AND (c.owner_org IS NULL OR c.owner_org=ANY($8::uuid[])) AND c.total>0 LIMIT 1 OFFSET 0
  ) held
 )
)
