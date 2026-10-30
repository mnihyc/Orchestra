-- The previous definition of access_holder_people_many, kept as an independent oracle for the tests in access.rs.
-- Retain all holder expansion, suspension and ownership eligibility semantics.
-- Membership is unique by (org_id,user_id). A non-null scalar probe expresses
-- each required root's exact lookup and avoids rescanning all memberships per person.
CREATE OR REPLACE FUNCTION access_holder_people_many(wanted_kinds text[], wanted_ids uuid[], minimum_roles text[],
                                    sampled_at timestamptz, system_admin boolean DEFAULT false)
RETURNS TABLE(wanted bigint, user_id uuid) LANGUAGE plpgsql STABLE
SET jit = off SET plan_cache_mode = force_generic_plan AS $$
BEGIN
  RETURN QUERY
  WITH RECURSIVE requested AS (
    SELECT * FROM unnest(wanted_kinds, wanted_ids, minimum_roles) WITH ORDINALITY AS r(wanted_kind, wanted_id, minimum_role, idx)
  ), walk(idx, kind, id, need, places, roots, only_user, seen) AS (
    SELECT seed.idx, seed.kind, seed.id, seed.need, ARRAY[]::uuid[], ARRAY[]::uuid[], NULL::uuid,
           ARRAY[seed.kind || ':' || seed.id::text || ':' || seed.need]
      FROM (
        SELECT idx, wanted_kind AS kind, wanted_id AS id,
               CASE WHEN wanted_kind = 'role' THEN minimum_role ELSE '' END AS need
          FROM requested
         WHERE wanted_id IS NOT NULL AND (wanted_kind <> 'role'
           OR minimum_role = ANY(ARRAY['viewer','interactor','developer','admin']))
        UNION ALL
        SELECT r.idx, wanted_kind, n.id, CASE WHEN wanted_kind = 'role' THEN minimum_role ELSE '' END
          FROM requested r CROSS JOIN nodes n WHERE wanted_id IS NULL AND wanted_kind IN ('role','visible') AND n.kind <> 'thread'
           AND (wanted_kind <> 'role' OR minimum_role = ANY(ARRAY['viewer','interactor','developer','admin']))
      ) seed
    UNION ALL
    SELECT w.idx, edge.kind, edge.id, edge.need, w.places || edge.path,
           w.roots || CASE WHEN edge.root IS NULL THEN ARRAY[]::uuid[] ELSE ARRAY[edge.root] END,
           COALESCE(w.only_user, edge.only_user), w.seen || (edge.kind || ':' || edge.id::text || ':' || edge.need)
      FROM walk w CROSS JOIN LATERAL (
        SELECT CASE WHEN m.user_id IS NOT NULL THEN 'user' WHEN m.member_group_id IS NOT NULL THEN 'group'
                    WHEN m.member_org_id IS NOT NULL THEN 'members'
                    WHEN m.member_role = 'viewer' THEN 'visible' ELSE 'role' END AS kind,
               COALESCE(m.user_id, m.member_group_id, m.member_org_id, m.member_node_id) AS id,
               CASE WHEN m.member_node_id IS NOT NULL AND m.member_role <> 'viewer' THEN m.member_role ELSE '' END AS need,
               n.path, NULL::uuid AS root, NULL::uuid AS only_user
          FROM group_members m JOIN groups g ON g.id = m.group_id JOIN nodes n ON n.id = g.org_id
         WHERE w.kind = 'group' AND m.group_id = w.id
        UNION ALL
        SELECT 'user', m.user_id, '', d.path || o.path, NULL, NULL
          FROM nodes o JOIN nodes d ON d.path @> ARRAY[o.id]::uuid[]
          JOIN org_members m ON m.domain_id = d.id
         WHERE w.kind = 'members' AND o.id = w.id AND o.kind = 'org'
        UNION ALL
        SELECT CASE WHEN r.subject_kind = 'role' AND r.subject_role = 'viewer' THEN 'visible' ELSE r.subject_kind END,
               r.subject_id, CASE WHEN r.subject_kind = 'role' AND r.subject_role <> 'viewer' THEN r.subject_role ELSE '' END,
               n.path, NULL, NULL
          FROM node_roles r JOIN nodes n ON n.id = r.node_id
         WHERE w.kind = 'role' AND r.node_id = w.id AND r.removed_at IS NULL
           AND (r.expires_at IS NULL OR r.expires_at > sampled_at)
           AND array_position(ARRAY['viewer','interactor','developer','admin'], r.role)
               >= array_position(ARRAY['viewer','interactor','developer','admin'], w.need)
        UNION ALL
        SELECT 'role', n.parent_id, w.need, n.path, NULL, NULL
          FROM nodes n WHERE w.kind = 'role' AND n.id = w.id AND n.kind <> 'thread' AND n.parent_id IS NOT NULL
        UNION ALL
        SELECT e.holder_kind, e.holder_id, '', n.path, n.org_id, NULL
          FROM nodes n JOIN permission_entries e ON e.object_kind = 'node' AND e.object_id = n.id
           AND e.page = 'live' AND e.permission = 'access.own'
         WHERE w.kind IN ('role','visible') AND n.id = w.id AND n.kind <> 'thread'
           AND e.holder_kind IN ('user','group') AND e.removed_at IS NULL
           AND (e.expires_at IS NULL OR e.expires_at > sampled_at)
        UNION ALL
        SELECT 'user', u.id, '', n.path, NULL, NULL
          FROM nodes n CROSS JOIN users u
         WHERE w.kind = 'role' AND n.id = w.id AND n.kind <> 'thread'
           AND system_admin AND u.is_system_admin AND u.status = 'active'
        UNION ALL
        SELECT 'visible', n.id, '', n.path, NULL, NULL
          FROM nodes n WHERE w.kind = 'reachable' AND n.id = w.id AND n.kind <> 'thread'
        UNION ALL
        SELECT 'role', n.id, 'admin', n.path, NULL, NULL
          FROM nodes n WHERE w.kind = 'reachable' AND n.id = w.id AND n.kind <> 'thread'
        UNION ALL
        SELECT CASE WHEN e.source = 'creation' THEN 'reachable'
                    WHEN e.holder_kind = 'members' THEN 'members'
                    WHEN e.holder_kind = 'everyone' THEN 'all'
                    WHEN e.holder_kind = 'role' AND e.holder_id IS NOT NULL AND e.holder_role = 'viewer' THEN 'visible'
                    ELSE e.holder_kind END,
               CASE WHEN e.source = 'creation' THEN COALESCE(n.parent_id, n.id)
                    WHEN e.holder_kind = 'everyone' THEN n.id ELSE COALESCE(e.holder_id, n.id) END,
               CASE WHEN e.source <> 'creation' AND e.holder_kind = 'role'
                          AND NOT (e.holder_id IS NOT NULL AND e.holder_role = 'viewer')
                    THEN e.holder_role ELSE '' END,
               n.path, NULL, CASE WHEN e.source = 'creation' THEN e.holder_id ELSE NULL END
          FROM nodes n CROSS JOIN LATERAL (
            WITH scopes AS MATERIALIZED (
              SELECT r.permission,
                     CASE WHEN r.object_kind = 'system' THEN 0 ELSE array_position(n.path, r.object_id) END AS rank,
                     r.inherit, r.locked AND r.page = 'defaults' AS locked
                FROM permission_rows r
               WHERE r.profile = 'any'
                 AND r.permission IN (n.kind || '.view', n.kind || '.interact', n.kind || '.manage', n.kind || '.archive',
                   'access.manage','access.external','settings.manage','members.manage','audit.view','usage.view','usage.manage',
                   'chat.view','chat.interact','chat.manage','chat.export','doc.view','doc.edit','doc.manage','doc.create','doc.comment','doc.export')
                 AND ((r.object_kind = 'system' AND r.page = 'defaults')
                   OR (r.object_kind = 'node' AND r.object_id = ANY(n.path)
                     AND ((r.page = 'live' AND r.object_id = n.id) OR (r.page = 'defaults' AND r.object_id <> n.id))))
            ), sources AS (
              SELECT p.*, CASE WHEN p.object_kind = 'system' THEN 0 ELSE array_position(n.path, p.object_id) END AS rank
                FROM permission_entries p
               WHERE p.profile = 'any' AND p.removed_at IS NULL AND (p.expires_at IS NULL OR p.expires_at > sampled_at)
                 AND p.holder_kind IN ('user','group','members','role','everyone')
                 AND (p.source <> 'creation' OR p.holder_kind = 'user')
                 AND p.permission IN (n.kind || '.view', n.kind || '.interact', n.kind || '.manage', n.kind || '.archive',
                   'access.manage','access.external','settings.manage','members.manage','audit.view','usage.view','usage.manage',
                   'chat.view','chat.interact','chat.manage','chat.export','doc.view','doc.edit','doc.manage','doc.create','doc.comment','doc.export')
                 AND ((p.object_kind = 'node' AND p.page = 'live' AND p.object_id = n.id)
                   OR (p.page = 'defaults' AND (p.object_kind = 'system'
                     OR (p.object_kind = 'node' AND p.object_id = ANY(n.path) AND p.object_id <> n.id))))
            )
            SELECT p.* FROM sources p
             WHERE p.rank <= COALESCE((SELECT min(s.rank) FROM scopes s WHERE s.permission = p.permission AND s.locked), 2147483647)
               AND NOT EXISTS (SELECT 1 FROM scopes s WHERE s.permission = p.permission AND NOT s.inherit AND s.rank > p.rank
                 AND s.rank <= COALESCE((SELECT min(l.rank) FROM scopes l WHERE l.permission = p.permission AND l.locked), 2147483647))
          ) e
         WHERE w.kind IN ('visible','reachable') AND n.id = w.id AND n.kind <> 'thread'
           AND (CASE WHEN e.permission IN (n.kind || '.view', n.kind || '.interact', n.kind || '.manage')
                     THEN 'visible' ELSE 'reachable' END) = w.kind
        UNION ALL
        SELECT 'user', u.id, '', ARRAY[]::uuid[], NULL, NULL
          FROM users u WHERE w.kind = 'all' AND u.status = 'active'
      ) edge
     WHERE NOT (edge.kind || ':' || edge.id::text || ':' || edge.need) = ANY(w.seen)
       AND (w.only_user IS NULL OR edge.only_user IS NULL OR w.only_user = edge.only_user)
  )
  SELECT DISTINCT w.idx, u.id FROM walk w JOIN users u ON w.kind = 'user' AND u.id = w.id AND u.status = 'active'
   WHERE (w.only_user IS NULL OR w.only_user = u.id)
     AND NOT EXISTS (SELECT 1 FROM nodes n WHERE n.id = ANY(w.places) AND n.deleted_at IS NOT NULL)
     AND NOT EXISTS (SELECT 1 FROM org_suspensions s WHERE s.user_id = u.id AND s.org_id = ANY(w.places))
     AND NOT EXISTS (SELECT 1 FROM unnest(w.roots) AS r(id)
                      WHERE (SELECT 1 FROM org_members m WHERE m.org_id = r.id AND m.user_id = u.id) IS NULL);
END
$$;
