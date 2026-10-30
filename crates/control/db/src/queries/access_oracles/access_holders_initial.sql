-- The first definition of access_holders, kept as an independent oracle for the tests in access.rs.
CREATE FUNCTION access_holders(person uuid, sampled_at timestamptz, system_admin boolean DEFAULT false)
RETURNS TABLE(kind text, id uuid, role text) LANGUAGE sql STABLE SET jit = off AS $$
  WITH RECURSIVE open_nodes AS (
    SELECT n.* FROM nodes n JOIN users u ON u.id = person AND u.status = 'active'
     WHERE n.deleted_at IS NULL
       AND NOT EXISTS (SELECT 1 FROM nodes p WHERE p.id = ANY(n.path) AND p.deleted_at IS NOT NULL)
       AND NOT EXISTS (
       SELECT 1 FROM org_suspensions s WHERE s.user_id = person AND s.org_id = ANY(n.path))
  ), view_scopes AS (
    SELECT n.id AS place, r.permission,
           CASE WHEN r.object_kind = 'system' THEN 0 ELSE array_position(n.path, r.object_id) END AS rank,
           r.inherit, r.locked AND r.page = 'defaults' AS locked
      FROM open_nodes n JOIN permission_rows r ON r.permission IN (n.kind || '.view', n.kind || '.interact', n.kind || '.manage', n.kind || '.archive',
        'access.manage','access.external','settings.manage','members.manage','audit.view','usage.view','usage.manage',
        'chat.view','chat.interact','chat.manage','chat.export','doc.view','doc.edit','doc.manage','doc.create','doc.comment','doc.export')
       AND r.profile = 'any' AND (
         (r.object_kind = 'system' AND r.page = 'defaults')
         OR (r.object_kind = 'node' AND r.object_id = ANY(n.path)
             AND ((r.page = 'live' AND r.object_id = n.id) OR (r.page = 'defaults' AND r.object_id <> n.id))))
     WHERE n.kind <> 'thread'
  ), view_sources AS (
    SELECT n.id AS place, COALESCE(n.parent_id, n.id) AS parent_place, n.kind AS place_kind, e.*,
           CASE WHEN e.object_kind = 'system' THEN 0 ELSE array_position(n.path, e.object_id) END AS rank
      FROM open_nodes n JOIN permission_entries e
        ON e.permission IN (n.kind || '.view', n.kind || '.interact', n.kind || '.manage', n.kind || '.archive',
          'access.manage','access.external','settings.manage','members.manage','audit.view','usage.view','usage.manage',
          'chat.view','chat.interact','chat.manage','chat.export','doc.view','doc.edit','doc.manage','doc.create','doc.comment','doc.export') AND e.profile = 'any'
       AND ((e.object_kind = 'node' AND e.page = 'live' AND e.object_id = n.id)
         OR (e.page = 'defaults' AND (e.object_kind = 'system'
           OR (e.object_kind = 'node' AND e.object_id = ANY(n.path) AND e.object_id <> n.id))))
     WHERE n.kind <> 'thread' AND e.removed_at IS NULL AND (e.expires_at IS NULL OR e.expires_at > sampled_at)
       AND e.holder_kind IN ('user','group','members','role','everyone')
       AND (e.source <> 'creation' OR (e.holder_kind = 'user' AND e.holder_id = person))
  ), counted_views AS (
    SELECT e.* FROM view_sources e
     WHERE e.rank <= COALESCE((SELECT min(r.rank) FROM view_scopes r
       WHERE r.place = e.place AND r.permission = e.permission AND r.locked), 2147483647)
       AND NOT EXISTS (SELECT 1 FROM view_scopes r
         WHERE r.place = e.place AND r.permission = e.permission AND NOT r.inherit AND r.rank > e.rank
           AND r.rank <= COALESCE((SELECT min(l.rank) FROM view_scopes l
             WHERE l.place = e.place AND l.permission = e.permission AND l.locked), 2147483647))
  ), edges(source_kind, source_id, source_role, target_kind, target_id, target_role) AS (
    SELECT 'user', m.user_id, ''::text, 'members', o.id, ''::text
      FROM org_members m JOIN open_nodes d ON d.id = m.domain_id
      JOIN open_nodes o ON o.kind = 'org' AND o.id = ANY(d.path) WHERE m.user_id = person
    UNION ALL
    SELECT CASE WHEN m.user_id IS NOT NULL THEN 'user' WHEN m.member_group_id IS NOT NULL THEN 'group'
                WHEN m.member_org_id IS NOT NULL THEN 'members' ELSE 'role' END,
           COALESCE(m.user_id, m.member_group_id, m.member_org_id, m.member_node_id),
           COALESCE(m.member_role, ''), 'group', m.group_id, ''
      FROM group_members m JOIN groups g ON g.id = m.group_id JOIN open_nodes n ON n.id = g.org_id
    UNION ALL
    SELECT r.subject_kind, r.subject_id, COALESCE(r.subject_role, ''), 'role', r.node_id, r.role
      FROM node_roles r JOIN open_nodes n ON n.id = r.node_id
     WHERE r.removed_at IS NULL AND (r.expires_at IS NULL OR r.expires_at > sampled_at)
    UNION ALL
    SELECT 'role', n.parent_id, '', 'role', n.id, NULL::text
      FROM open_nodes n WHERE n.kind <> 'thread' AND n.parent_id IS NOT NULL
    UNION ALL
    -- Page-only viewing is a separate identity: it never propagates a role to children.
    SELECT CASE WHEN e.source = 'creation' THEN 'reachable'
                WHEN e.holder_kind = 'role' AND e.holder_id IS NULL THEN 'page_role'
                WHEN e.holder_kind = 'members' THEN 'members' WHEN e.holder_kind = 'everyone' THEN 'user'
                ELSE e.holder_kind END,
           CASE WHEN e.source = 'creation' THEN e.parent_place
                WHEN e.holder_kind = 'everyone' THEN person ELSE COALESCE(e.holder_id, e.place) END,
           COALESCE(e.holder_role, ''),
           CASE WHEN e.permission IN (e.place_kind || '.view', e.place_kind || '.interact', e.place_kind || '.manage')
                THEN 'visible' ELSE 'reachable' END, e.place, 'viewer'
      FROM counted_views e
    UNION ALL
    SELECT 'visible', n.id, '', 'reachable', n.id, '' FROM open_nodes n WHERE n.kind <> 'thread'
    UNION ALL
    SELECT 'role', n.id, 'admin', 'reachable', n.id, '' FROM open_nodes n WHERE n.kind <> 'thread'
    UNION ALL
    SELECT e.holder_kind, e.holder_id, '', 'role', n.id, 'admin'
      FROM permission_entries e JOIN open_nodes n ON n.id = e.object_id AND n.kind <> 'thread'
      JOIN org_members m ON m.org_id = n.org_id AND m.user_id = person
     WHERE e.object_kind = 'node' AND e.page = 'live' AND e.permission = 'access.own'
       AND e.holder_kind IN ('user','group') AND e.removed_at IS NULL
       AND (e.expires_at IS NULL OR e.expires_at > sampled_at)
    UNION ALL
    SELECT e.holder_kind, e.holder_id, '', 'visible', n.id, 'viewer'
      FROM permission_entries e JOIN open_nodes n ON n.id = e.object_id AND n.kind <> 'thread'
      JOIN org_members m ON m.org_id = n.org_id AND m.user_id = person
     WHERE e.object_kind = 'node' AND e.page = 'live' AND e.permission = 'access.own'
       AND e.holder_kind IN ('user','group') AND e.removed_at IS NULL
       AND (e.expires_at IS NULL OR e.expires_at > sampled_at)
    UNION ALL
    SELECT 'user', person, '', 'role', n.id, 'admin'
      FROM open_nodes n JOIN users u ON u.id = person
     WHERE system_admin AND u.is_system_admin AND n.kind <> 'thread'
  ), held(kind, id, role) AS (
    SELECT 'user'::text, person, ''::text
    UNION
    SELECT e.target_kind, e.target_id, COALESCE(e.target_role, h.role) FROM held h JOIN edges e
      ON (e.source_kind = h.kind OR (e.source_kind = 'role' AND h.kind = 'visible' AND e.source_role = 'viewer')
          OR (e.source_kind = 'page_role' AND h.kind = 'role'))
     AND e.source_id = h.id
     AND NOT (e.source_kind = 'role' AND e.source_role = 'viewer' AND h.kind = 'role')
     AND (e.source_role = '' OR array_position(ARRAY['viewer','interactor','developer','admin'], h.role)
                              >= array_position(ARRAY['viewer','interactor','developer','admin'], e.source_role))
  )
  SELECT h.kind, h.id,
         CASE WHEN h.kind = 'role' THEN (ARRAY['viewer','interactor','developer','admin'])[
           max(array_position(ARRAY['viewer','interactor','developer','admin'], h.role))]
           WHEN h.kind = 'visible' THEN 'viewer'::text ELSE ''::text END
    FROM held h GROUP BY h.kind, h.id
$$;
