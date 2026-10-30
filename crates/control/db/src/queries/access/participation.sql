-- Ordinary assignment summaries and current role/follow facts; never authorization.
WITH wanted AS MATERIALIZED (
 SELECT * FROM unnest($6::text[],$7::uuid[]) w(kind,id)
), sources AS (
  SELECT 'node'::text kind,n.id,n.path,false AS followed,false AS owner
    FROM node_roles r JOIN nodes n ON n.id=r.node_id
   WHERE r.removed_at IS NULL AND (r.expires_at IS NULL OR r.expires_at>$4)
     AND __ROLE_SUBJECT__
  UNION ALL
  SELECT 'node',n.id,n.path,true,false FROM subscriptions s JOIN nodes n ON n.id=s.node_id
   WHERE s.user_id=$1 AND s.level<>'off'
  UNION ALL
  SELECT 'chat',n.id,n.path,true,false
    FROM subscriptions sub JOIN nodes n ON n.id=sub.node_id AND n.kind='thread'
    JOIN chats c ON c.node_id=n.id
    LEFT JOIN chat_states s ON s.chat_id=c.id AND s.user_id=sub.user_id
    LEFT JOIN notification_prefs p ON p.user_id=sub.user_id
   WHERE sub.user_id=$1 AND sub.level='all' AND __PEOPLE_FOLLOW_LEVEL__<>'off'
  UNION ALL
  SELECT 'chat',COALESCE(c.node_id,c.id),n.path,true,false
    FROM chat_states s JOIN chats c ON c.id=s.chat_id JOIN nodes n ON n.id=COALESCE(c.node_id,c.org_id)
    LEFT JOIN notification_prefs p ON p.user_id=s.user_id
   WHERE s.user_id=$1 AND c.kind='node' AND __NODE_FOLLOW_LEVEL__<>'off'
  UNION ALL
  SELECT 'chat',c.id,n.path,true,false
    FROM chat_members m JOIN chats c ON c.id=m.chat_id JOIN nodes n ON n.id=c.org_id
    LEFT JOIN chat_states s ON s.chat_id=c.id AND s.user_id=m.user_id
    LEFT JOIN notification_prefs p ON p.user_id=m.user_id
   WHERE m.user_id=$1 AND m.left_at IS NULL AND (c.org_id=ANY($8::uuid[]) OR m.guest)
     AND __PEOPLE_FOLLOW_LEVEL__<>'off'
), live AS MATERIALIZED (
  SELECT s.* FROM sources s
   WHERE NOT s.path && $9::uuid[]
     AND ($10::uuid IS NULL OR $10=ANY(s.path))
     AND NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id=ANY(s.path) AND gone.deleted_at IS NOT NULL)
     AND (cardinality(s.path)=0 OR s.path[1]=ANY($11::uuid[]))
), subjects AS MATERIALIZED (
 SELECT 'user'::text kind,$1::uuid id UNION SELECT 'group',id FROM unnest($2::uuid[]) g(id)
), expiry_keys AS MATERIALIZED (
 SELECT * FROM orch_participation_after_keys($4)
), direct AS (
 SELECT w.kind,w.id FROM wanted w
 CROSS JOIN LATERAL (
  SELECT o.* FROM participation_object_facts o WHERE o.object_kind=w.kind AND o.object_id=w.id AND o.live OFFSET 0
 ) o LEFT JOIN participation_paths p ON p.node_id=o.home_id
 WHERE (o.home_id IS NULL OR p.live)
  AND NOT COALESCE(p.path,'{}'::uuid[]) && $9::uuid[]
  AND ($10::uuid IS NULL OR $10=ANY(p.path))
  AND (cardinality(COALESCE(p.path,'{}'::uuid[]))=0 OR p.path[1]=ANY($11::uuid[]))
  AND EXISTS (
   SELECT 1 FROM subjects subject CROSS JOIN expiry_keys t CROSS JOIN LATERAL (
    SELECT 1 FROM participation_direct_counts c
    WHERE c.subject_kind=subject.kind AND c.subject_id=subject.id
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
), summary_cache_capacity AS (
 -- A tiny relation fits in one bounded cache and avoids repeated scans for absent expiry keys.
 SELECT 64 AS rows
), summary_sample AS MATERIALIZED (
 SELECT * FROM participation_node_counts LIMIT (SELECT rows+1 FROM summary_cache_capacity)
), summary_cache_mode AS MATERIALIZED (
 SELECT count(*)<=(SELECT rows FROM summary_cache_capacity) cached FROM summary_sample
), totals AS MATERIALIZED (
 SELECT r.id,coalesce(sum(held.total),0)::bigint total FROM count_requests r
 CROSS JOIN subjects subject CROSS JOIN unnest($11::uuid[]) root(id) CROSS JOIN expiry_keys t
 LEFT JOIN LATERAL (
  SELECT c.total FROM summary_sample c
  WHERE (SELECT cached FROM summary_cache_mode)
   AND c.subject_kind=subject.kind AND c.subject_id=subject.id AND c.node_id=r.id AND c.scope_root=root.id
   AND c.time_shift=t.time_shift AND c.time_bucket=t.time_bucket
   AND (c.owner_org IS NULL OR c.owner_org=ANY($8::uuid[])) AND c.total>0
  UNION ALL
  SELECT c.total FROM participation_node_counts c
  WHERE NOT (SELECT cached FROM summary_cache_mode)
   AND c.subject_kind=subject.kind AND c.subject_id=subject.id AND c.node_id=r.id AND c.scope_root=root.id
   AND c.time_shift=t.time_shift AND c.time_bucket=t.time_bucket
   AND (c.owner_org IS NULL OR c.owner_org=ANY($8::uuid[])) AND c.total>0
   -- Keep each complete summary-key lookup correlated instead of scanning unrelated homes.
   OFFSET 0
 ) held ON true GROUP BY r.id
), assigned_nodes AS (
 SELECT t.marker FROM targets t JOIN totals all_sources ON all_sources.id=t.target
 LEFT JOIN blockers b ON b.marker=t.marker LEFT JOIN totals blocked ON blocked.id=b.node_id
 GROUP BY t.marker,all_sources.total HAVING all_sources.total-coalesce(sum(blocked.total),0)>0
), marks AS (
 SELECT kind,id FROM direct
 UNION SELECT 'node',marker FROM assigned_nodes
 UNION
  SELECT s.kind,s.id FROM live s
   WHERE EXISTS (SELECT 1 FROM wanted w WHERE w.kind=s.kind AND w.id=s.id)
      OR (s.kind IN ('node','chat') AND s.followed)
  UNION
  SELECT 'node',ancestor.id FROM live s CROSS JOIN LATERAL unnest(s.path) ancestor(id)
   WHERE ancestor.id=ANY($5::uuid[])
)
SELECT kind,id FROM marks
