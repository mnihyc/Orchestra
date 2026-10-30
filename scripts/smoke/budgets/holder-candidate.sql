-- Query-scale candidate only; the production upgrade requires its reserved forward migration.
-- Expand group and owner leaves in batches and share their path and eligibility reads.
-- Every leaf retains its source slot, creation restriction, traversed places and required owner roots.
CREATE OR REPLACE FUNCTION access_holder_people_many(wanted_kinds text[], wanted_ids uuid[], minimum_roles text[],
                                    sampled_at timestamptz, system_admin boolean DEFAULT false)
RETURNS TABLE(wanted bigint, user_id uuid) LANGUAGE plpgsql STABLE
-- The written joins bind each lookup to its source; avoid repeating join-order search on a new backend.
SET jit = off SET plan_cache_mode = force_generic_plan
SET join_collapse_limit = 1 SET from_collapse_limit = 1 AS $$
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
         WHERE w.kind = 'group' AND m.group_id = w.id AND m.user_id IS NULL
        UNION ALL
        SELECT CASE WHEN r.subject_kind = 'role' AND r.subject_role = 'viewer' THEN 'visible' ELSE r.subject_kind END,
               r.subject_id, CASE WHEN r.subject_kind = 'role' AND r.subject_role <> 'viewer' THEN r.subject_role ELSE '' END,
               n.path, NULL, NULL
          FROM node_roles r JOIN nodes n ON n.id = r.node_id
         WHERE w.kind = 'role' AND r.node_id = w.id AND r.removed_at IS NULL
           AND (r.expires_at IS NULL OR r.expires_at > sampled_at)
           AND array_position(ARRAY['viewer','interactor','developer','admin'], r.role)
               >= array_position(ARRAY['viewer','interactor','developer','admin'], w.need) AND r.subject_kind <> 'user'
        UNION ALL
        SELECT 'role', n.parent_id, w.need, n.path, NULL, NULL
          FROM nodes n WHERE w.kind = 'role' AND n.id = w.id AND n.kind <> 'thread' AND n.parent_id IS NOT NULL
        UNION ALL
        SELECT e.holder_kind, e.holder_id, '', n.path, n.org_id, NULL
          FROM nodes n JOIN permission_entries e ON e.object_kind = 'node' AND e.object_id = n.id
           AND e.page = 'live' AND e.permission = 'access.own'
         WHERE w.kind IN ('role','visible') AND n.id = w.id AND n.kind <> 'thread'
           AND e.holder_kind = 'group' AND e.removed_at IS NULL
           AND (e.expires_at IS NULL OR e.expires_at > sampled_at)
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
      ) edge
     WHERE w.kind <> 'user' AND NOT (edge.kind || ':' || edge.id::text || ':' || edge.need) = ANY(w.seen)
       AND (w.only_user IS NULL OR edge.only_user IS NULL OR w.only_user = edge.only_user)
  -- People are terminal. Group and owner members expand after the structural frontier.
  ), group_sources AS MATERIALIZED (
    SELECT w.*, membership.member_path FROM walk w CROSS JOIN LATERAL (
      SELECT n.path AS member_path FROM groups g JOIN nodes n ON n.id = g.org_id
       WHERE g.id = w.id OFFSET 0
    ) membership WHERE w.kind = 'group'
  -- Keep the owner lookup bound to one place before separating user and group holders.
  -- Otherwise a user-only estimate can choose a broad holder index for every place.
  ), owner_sources AS MATERIALIZED (
    SELECT w.*, owner.* FROM walk w CROSS JOIN LATERAL (
      SELECT n.path AS owner_path, n.org_id AS owner_root, e.holder_kind, e.holder_id
        FROM nodes n JOIN permission_entries e ON e.object_kind = 'node' AND e.object_id = n.id
         AND e.page = 'live' AND e.permission = 'access.own'
       WHERE n.id = w.id AND n.kind <> 'thread' AND e.holder_kind IN ('user','group')
         AND e.removed_at IS NULL AND (e.expires_at IS NULL OR e.expires_at > sampled_at)
       OFFSET 0
    ) owner WHERE w.kind IN ('role','visible')
  ), leaves(idx, id, places, roots, only_user) AS MATERIALIZED (
    SELECT w.idx, w.id, w.places, w.roots, w.only_user FROM walk w WHERE w.kind = 'user'
    UNION ALL
    SELECT w.idx, w.holder_id, w.places || w.owner_path,
           w.roots || CASE WHEN w.owner_root IS NULL THEN ARRAY[]::uuid[] ELSE ARRAY[w.owner_root] END,
           w.only_user
      FROM owner_sources w WHERE w.holder_kind = 'user'
       AND NOT ('user:' || w.holder_id::text || ':') = ANY(w.seen)
    UNION ALL
    SELECT w.idx, m.user_id, w.places || w.member_path, w.roots, w.only_user
      FROM group_sources w JOIN group_members m ON m.group_id = w.id
     WHERE m.user_id IS NOT NULL AND NOT ('user:' || m.user_id::text || ':') = ANY(w.seen)
    UNION ALL
    SELECT w.idx, edge.id, w.places || edge.path,
           w.roots || CASE WHEN edge.root IS NULL THEN ARRAY[]::uuid[] ELSE ARRAY[edge.root] END,
           COALESCE(w.only_user, edge.only_user)
      FROM walk w CROSS JOIN LATERAL (
        SELECT 'user', m.user_id, '', d.path || o.path, NULL::uuid, NULL::uuid
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
               >= array_position(ARRAY['viewer','interactor','developer','admin'], w.need) AND r.subject_kind = 'user'
        UNION ALL
        SELECT 'user', u.id, '', n.path, NULL, NULL
          FROM nodes n CROSS JOIN users u
         WHERE w.kind = 'role' AND n.id = w.id AND n.kind <> 'thread'
           AND system_admin AND u.is_system_admin AND u.status = 'active'
        UNION ALL
        SELECT 'user', u.id, '', ARRAY[]::uuid[], NULL, NULL
          FROM users u WHERE w.kind = 'all' AND u.status = 'active'
      ) edge(kind, id, need, path, root, only_user)
     WHERE w.kind <> 'user' AND edge.kind = 'user'
       AND NOT (edge.kind || ':' || edge.id::text || ':' || edge.need) = ANY(w.seen)
       AND (w.only_user IS NULL OR edge.only_user IS NULL OR w.only_user = edge.only_user)
  -- Shared person and path checks do not repeat for each source naming the same people.
  ), leaf_people AS MATERIALIZED (
    SELECT DISTINCT id FROM leaves
  ), leaf_paths AS MATERIALIZED (
    SELECT DISTINCT places FROM leaves
  ), deleted_paths AS MATERIALIZED (
    SELECT p.places FROM leaf_paths p
     WHERE EXISTS (SELECT 1 FROM nodes n WHERE n.id = ANY(p.places) AND n.deleted_at IS NOT NULL)
  )
  SELECT DISTINCT w.idx, w.id FROM leaves w
   -- Preserve a hashed active-person set rather than a poorly estimated user join.
   WHERE (w.id IN (SELECT p.id FROM leaf_people p
     WHERE (SELECT u.status FROM users u WHERE u.id = p.id) = 'active')) IS TRUE AND (w.only_user IS NULL OR w.only_user = w.id)
     AND NOT EXISTS (SELECT 1 FROM deleted_paths p WHERE p.places IS NOT DISTINCT FROM w.places)
     AND NOT EXISTS (SELECT 1 FROM org_suspensions s WHERE s.user_id = w.id AND s.org_id = ANY(w.places))
     AND NOT EXISTS (SELECT 1 FROM unnest(w.roots) AS r(id)
                      WHERE (SELECT 1 FROM org_members m WHERE m.org_id = r.id AND m.user_id = w.id) IS NULL);
END
$$;
