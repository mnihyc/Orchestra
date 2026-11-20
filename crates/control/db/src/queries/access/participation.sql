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
    FROM chat_states s JOIN chats c ON c.id=s.chat_id JOIN nodes n ON n.id=COALESCE(c.node_id,c.home_id,c.org_id)
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
     AND NOT EXISTS (SELECT 1 FROM unnest(s.path) ancestor(id)
       WHERE (SELECT gone.deleted_at FROM nodes gone WHERE gone.id=ancestor.id) IS NOT NULL)
     AND (cardinality(s.path)=0 OR s.path[1]=ANY($11::uuid[]))
), subjects AS MATERIALIZED (
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
), __ANCESTOR_MARKS__, marks AS (
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
