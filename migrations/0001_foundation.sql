-- Extensions, aggregates and the functions the tables, triggers and the engine call.

-- Function bodies refer to tables made by the later files; they are checked when first called.

SET LOCAL check_function_bodies = false;

CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- A conversation's words are looked up together with its thread in one index (items_search_idx).
CREATE EXTENSION IF NOT EXISTS btree_gin;

CREATE SEQUENCE document_discussion_subscription_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

CREATE FUNCTION public.access_changed() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  INSERT INTO access_changes (xid) VALUES (pg_current_xact_id()) ON CONFLICT (xid) DO NOTHING;
  RETURN NULL;
END $$;

CREATE FUNCTION public.access_changes_prune(kept_for interval) RETURNS bigint
    LANGUAGE sql
    AS $$
  WITH gone AS (
    DELETE FROM access_changes WHERE at < now() - kept_for AND xid < pg_snapshot_xmin(pg_current_snapshot())
    RETURNING xid
  ), raised AS (
    INSERT INTO access_changes_floor (id, floor) SELECT true, max(xid) FROM gone HAVING count(*) > 0
    ON CONFLICT (id) DO UPDATE SET floor = greatest(access_changes_floor.floor, EXCLUDED.floor)
    RETURNING floor
  )
  SELECT count(*) FROM gone
$$;

CREATE FUNCTION public.access_holder_people(wanted_kind text, wanted_id uuid, minimum_role text, sampled_at timestamp with time zone, system_admin boolean DEFAULT false, candidates uuid[] DEFAULT NULL::uuid[]) RETURNS TABLE(user_id uuid)
    LANGUAGE sql STABLE
    SET jit TO 'off'
    AS $$
  SELECT h.user_id FROM access_holder_people_many(ARRAY[wanted_kind], ARRAY[wanted_id],
    ARRAY[minimum_role], sampled_at, system_admin, candidates) h
$$;

CREATE FUNCTION public.access_holder_people_many(wanted_kinds text[], wanted_ids uuid[], minimum_roles text[], sampled_at timestamp with time zone, system_admin boolean DEFAULT false, candidates uuid[] DEFAULT NULL::uuid[]) RETURNS TABLE(wanted bigint, user_id uuid)
    LANGUAGE plpgsql STABLE
    SET jit TO 'off'
    SET plan_cache_mode TO 'force_generic_plan'
    SET join_collapse_limit TO '1'
    SET from_collapse_limit TO '1'
    AS $$
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
  -- With candidates, only those people are expanded: a group or an org of any size is looked up by
  -- them rather than listed whole, and what holds for each of them is what holds without the list.
  ), leaves(idx, id, places, roots, only_user) AS MATERIALIZED (
    SELECT w.idx, w.id, w.places, w.roots, w.only_user FROM walk w WHERE w.kind = 'user'
       AND (candidates IS NULL OR w.id = ANY(candidates))
    UNION ALL
    SELECT w.idx, w.holder_id, w.places || w.owner_path,
           w.roots || CASE WHEN w.owner_root IS NULL THEN ARRAY[]::uuid[] ELSE ARRAY[w.owner_root] END,
           w.only_user
      FROM owner_sources w WHERE w.holder_kind = 'user'
       AND NOT ('user:' || w.holder_id::text || ':') = ANY(w.seen)
       AND (candidates IS NULL OR w.holder_id = ANY(candidates))
    UNION ALL
    SELECT w.idx, m.user_id, w.places || w.member_path, w.roots, w.only_user
      FROM group_sources w JOIN group_members m ON m.group_id = w.id
     WHERE candidates IS NULL AND m.user_id IS NOT NULL AND NOT ('user:' || m.user_id::text || ':') = ANY(w.seen)
    UNION ALL
    SELECT w.idx, m.user_id, w.places || w.member_path, w.roots, w.only_user
      FROM group_sources w JOIN group_members m ON m.group_id = w.id AND m.user_id = ANY(candidates)
     WHERE candidates IS NOT NULL AND NOT ('user:' || m.user_id::text || ':') = ANY(w.seen)
    UNION ALL
    SELECT w.idx, edge.id, w.places || edge.path,
           w.roots || CASE WHEN edge.root IS NULL THEN ARRAY[]::uuid[] ELSE ARRAY[edge.root] END,
           COALESCE(w.only_user, edge.only_user)
      FROM walk w CROSS JOIN LATERAL (
        SELECT 'user', m.user_id, '', d.path || o.path, NULL::uuid, NULL::uuid
          FROM nodes o JOIN nodes d ON d.path @> ARRAY[o.id]::uuid[]
          JOIN org_members m ON m.domain_id = d.id
         WHERE w.kind = 'members' AND o.id = w.id AND o.kind = 'org' AND candidates IS NULL
        UNION ALL
        SELECT 'user', m.user_id, '', d.path || o.path, NULL::uuid, NULL::uuid
          FROM nodes o JOIN org_members m ON m.user_id = ANY(candidates)
          JOIN nodes d ON d.id = m.domain_id AND d.path @> ARRAY[o.id]::uuid[]
         WHERE w.kind = 'members' AND o.id = w.id AND o.kind = 'org' AND candidates IS NOT NULL
        UNION ALL
        SELECT CASE WHEN r.subject_kind = 'role' AND r.subject_role = 'viewer' THEN 'visible' ELSE r.subject_kind END,
               r.subject_id, CASE WHEN r.subject_kind = 'role' AND r.subject_role <> 'viewer' THEN r.subject_role ELSE '' END,
               n.path, NULL, NULL
          FROM node_roles r JOIN nodes n ON n.id = r.node_id
         WHERE w.kind = 'role' AND r.node_id = w.id AND r.removed_at IS NULL
           AND (r.expires_at IS NULL OR r.expires_at > sampled_at)
           AND array_position(ARRAY['viewer','interactor','developer','admin'], r.role)
               >= array_position(ARRAY['viewer','interactor','developer','admin'], w.need) AND r.subject_kind = 'user'
           AND (candidates IS NULL OR r.subject_id = ANY(candidates))
        UNION ALL
        SELECT 'user', u.id, '', n.path, NULL, NULL
          FROM nodes n CROSS JOIN users u
         WHERE w.kind = 'role' AND n.id = w.id AND n.kind <> 'thread'
           AND system_admin AND u.is_system_admin AND u.status = 'active'
           AND (candidates IS NULL OR u.id = ANY(candidates))
        UNION ALL
        SELECT 'user', u.id, '', ARRAY[]::uuid[], NULL, NULL
          FROM users u WHERE w.kind = 'all' AND u.status = 'active' AND candidates IS NULL
        UNION ALL
        SELECT 'user', u.id, '', ARRAY[]::uuid[], NULL, NULL
          FROM users u WHERE w.kind = 'all' AND u.status = 'active' AND u.id = ANY(candidates)
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

CREATE FUNCTION public.access_holders(person uuid, sampled_at timestamp with time zone, system_admin boolean DEFAULT false) RETURNS TABLE(kind text, id uuid, role text)
    LANGUAGE sql STABLE
    SET jit TO 'off'
    AS $$
  WITH RECURSIVE open_nodes AS (
    SELECT n.* FROM nodes n JOIN users u ON u.id = person AND u.status = 'active'
     WHERE n.kind <> 'thread' AND n.deleted_at IS NULL
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
  ), edges(source_kind, source_id, source_role, target_kind, target_id, target_role) AS MATERIALIZED (
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

CREATE FUNCTION public.access_page_changed() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF TG_OP IN ('INSERT', 'UPDATE') THEN
    IF NEW.object_kind = 'system' OR NOT EXISTS (SELECT 1 FROM nodes n WHERE n.id = NEW.object_id AND n.kind = 'thread') THEN
      INSERT INTO access_changes (xid) VALUES (pg_current_xact_id()) ON CONFLICT (xid) DO NOTHING;
      RETURN NULL;
    END IF;
  END IF;
  IF TG_OP IN ('UPDATE', 'DELETE') THEN
    IF OLD.object_kind = 'system' OR NOT EXISTS (SELECT 1 FROM nodes n WHERE n.id = OLD.object_id AND n.kind = 'thread') THEN
      INSERT INTO access_changes (xid) VALUES (pg_current_xact_id()) ON CONFLICT (xid) DO NOTHING;
    END IF;
  END IF;
  RETURN NULL;
END $$;

CREATE FUNCTION public.access_role_people(places uuid[], roles text[], include_viewers boolean, sampled_at timestamp with time zone, system_admin boolean DEFAULT false, candidates uuid[] DEFAULT NULL::uuid[]) RETURNS TABLE(place uuid, user_id uuid)
    LANGUAGE sql STABLE
    SET jit TO 'off'
    AS $$
  SELECT p.id, h.user_id FROM unnest(places) AS p(id)
    CROSS JOIN LATERAL access_holder_people('visible', p.id, '', sampled_at, system_admin, candidates) h
   WHERE include_viewers AND 'viewer' = ANY(roles)
  UNION
  SELECT p.id, held.user_id FROM unnest(places) AS p(id)
    CROSS JOIN LATERAL (
      WITH grades AS MATERIALIZED (
        SELECT * FROM unnest(ARRAY['viewer','interactor','developer','admin']) WITH ORDINALITY AS g(role, rank)
      ), bands AS (
        SELECT g.role, (SELECT next.role FROM grades next
                        WHERE next.rank > g.rank AND NOT (next.role = ANY(roles))
                        ORDER BY next.rank LIMIT 1) AS stop_role
          FROM grades g WHERE g.role = ANY(roles)
           AND NOT EXISTS (SELECT 1 FROM grades before
                            WHERE before.rank = g.rank - 1 AND before.role = ANY(roles))
      )
      -- A contiguous requested band needs one lookup, plus the excluded higher
      -- threshold if there is one. A usual minimum-role audience needs only one.
      SELECT low.user_id FROM bands b CROSS JOIN LATERAL (
        SELECT user_id FROM access_holder_people('role', p.id, b.role, sampled_at, system_admin, candidates)
        EXCEPT
        SELECT user_id FROM access_holder_people('role', p.id, b.stop_role, sampled_at, system_admin, candidates)
         WHERE b.stop_role IS NOT NULL
      ) low
    ) held
   WHERE NOT (include_viewers AND 'viewer' = ANY(roles))
$$;

CREATE FUNCTION public.agent_history_closed() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    closed_thread uuid := (NEW.body->>'thread_id')::uuid;
    closed_segment uuid := (NEW.body->>'segment_id')::uuid;
    source_segment uuid := (NEW.body#>>'{source,segment_id}')::uuid;
BEGIN
    UPDATE segments s SET agent_history_measure_revision=agent_history_measure_revision+1
    WHERE s.native_session_ref IS NOT NULL AND s.machine_id=NEW.machine_id
      AND s.thread_id=closed_thread
      AND (NOT NEW.body ? 'segment_id' OR s.id=closed_segment)
      AND (source_segment IS NULL OR s.id=source_segment);
    RETURN NEW;
END
$$;

CREATE FUNCTION public.orch_agent_history_natives() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    was boolean := TG_OP <> 'INSERT' AND OLD.native_session_ref IS NOT NULL
        AND OLD.provider_id IS NOT NULL AND OLD.agent_id IS NOT NULL;
    named boolean := TG_OP <> 'DELETE' AND NEW.native_session_ref IS NOT NULL
        AND NEW.provider_id IS NOT NULL AND NEW.agent_id IS NOT NULL;
BEGIN
    -- One row per native identity keeps the closes of all its segments and the closes its last
    -- measurement covered, so that the unmeasured ones are found by index (agent_history::closed_page).
    IF was AND named AND OLD.machine_id IS NOT DISTINCT FROM NEW.machine_id
       AND OLD.provider_id = NEW.provider_id AND OLD.agent_id = NEW.agent_id
       AND OLD.native_session_ref = NEW.native_session_ref THEN
        UPDATE agent_history_natives n
           SET closes = n.closes + NEW.agent_history_measure_revision - OLD.agent_history_measure_revision,
               measured = GREATEST(n.measured, NEW.agent_history_measured_revision)
         WHERE n.machine_id IS NOT DISTINCT FROM NEW.machine_id AND n.provider_id = NEW.provider_id
           AND n.agent_id = NEW.agent_id AND n.native_session_ref = NEW.native_session_ref
           AND (NEW.agent_history_measure_revision <> OLD.agent_history_measure_revision
                OR NEW.agent_history_measured_revision > n.measured);
        RETURN NULL;
    END IF;
    IF was THEN
        UPDATE agent_history_natives n
           SET closes = n.closes - OLD.agent_history_measure_revision, segments = n.segments - 1
         WHERE n.machine_id IS NOT DISTINCT FROM OLD.machine_id AND n.provider_id = OLD.provider_id
           AND n.agent_id = OLD.agent_id AND n.native_session_ref = OLD.native_session_ref;
        DELETE FROM agent_history_natives n
         WHERE n.machine_id IS NOT DISTINCT FROM OLD.machine_id AND n.provider_id = OLD.provider_id
           AND n.agent_id = OLD.agent_id AND n.native_session_ref = OLD.native_session_ref AND n.segments = 0;
    END IF;
    IF named THEN
        -- A segment that joins an identity is its newest alias: like a fresh one, its history is
        -- measured again unless it brings a measurement of its own.
        INSERT INTO agent_history_natives AS n
               (machine_id, provider_id, agent_id, native_session_ref, closes, measured, segments)
        VALUES (NEW.machine_id, NEW.provider_id, NEW.agent_id, NEW.native_session_ref,
                NEW.agent_history_measure_revision, NEW.agent_history_measured_revision, 1)
        ON CONFLICT (machine_id, provider_id, agent_id, native_session_ref) DO UPDATE
           SET closes = n.closes + EXCLUDED.closes, measured = LEAST(n.measured, EXCLUDED.measured),
               segments = n.segments + 1;
    END IF;
    RETURN NULL;
END
$$;

CREATE FUNCTION public.audit_person_notify_count() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    NEW.person_notify_count := CASE WHEN NEW.action='person.notified'
        THEN (NEW.details->>'count')::numeric ELSE NULL END;
    RETURN NEW;
END $$;

CREATE FUNCTION public.document_basename(title text) RETURNS text
    LANGUAGE sql IMMUTABLE STRICT
    AS $$
  SELECT (CASE WHEN name IN ('','.','..') THEN '_' || name || '_' ELSE name END) || '.md'
    FROM (SELECT btrim(regexp_replace(replace(replace(btrim(title),'/','-'),chr(92),'-'),'[[:cntrl:]]','-','g')) AS name) canonical
$$;

CREATE FUNCTION public.document_comment_activity_changed() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE latest timestamptz;
BEGIN
    IF TG_OP='INSERT' THEN
        UPDATE document_discussions SET
            latest_comment_at=GREATEST(latest_comment_at,NEW.created_at),
            last_activity_at=GREATEST(latest_comment_at,NEW.created_at)
        WHERE id=NEW.discussion_id;
    ELSE
        -- Serialize repairs with additions to the same discussion. Soft deletion,
        -- reactions and text edits retain their original creation-time order.
        PERFORM 1 FROM document_discussions WHERE id=OLD.discussion_id FOR UPDATE;
        IF NOT FOUND THEN RETURN NULL; END IF;
        SELECT c.created_at INTO latest FROM document_comments c
            WHERE c.discussion_id=OLD.discussion_id
            ORDER BY c.created_at DESC,c.id DESC LIMIT 1;
        UPDATE document_discussions SET latest_comment_at=latest,
            last_activity_at=COALESCE(latest,created_at) WHERE id=OLD.discussion_id;
        IF TG_OP='UPDATE' THEN
            UPDATE document_discussions SET
                latest_comment_at=GREATEST(latest_comment_at,NEW.created_at),
                last_activity_at=GREATEST(latest_comment_at,NEW.created_at)
            WHERE id=NEW.discussion_id;
        END IF;
    END IF;
    RETURN NULL;
END $$;

CREATE FUNCTION public.document_discussion_activity_started() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP='INSERT' THEN NEW.latest_comment_at := NULL; END IF;
    NEW.last_activity_at := COALESCE(NEW.latest_comment_at,NEW.created_at);
    RETURN NEW;
END $$;

CREATE FUNCTION public.document_discussion_participant_added() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_TABLE_NAME='document_discussions' THEN
        INSERT INTO document_discussion_participants VALUES(NEW.id,NEW.created_by,NEW.seq_updated)
        ON CONFLICT(discussion_id,user_id) DO NOTHING;
    ELSE
        INSERT INTO document_discussion_participants VALUES(NEW.discussion_id,NEW.author_id,NEW.seq_updated)
        ON CONFLICT(discussion_id,user_id) DO NOTHING;
    END IF;
    RETURN NEW;
END $$;

CREATE FUNCTION public.document_discussion_subscription_changed() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF NEW.discussion_level IS DISTINCT FROM OLD.discussion_level THEN
        NEW.discussion_subscription_seq=nextval('document_discussion_subscription_seq');
    END IF;
    RETURN NEW;
END $$;

CREATE FUNCTION public.document_namespace_taken(area uuid, parent uuid, label text, kind text, identity uuid) RETURNS boolean
    LANGUAGE sql
    AS $$
  -- Separate the virtual root from a real folder so both cases can use all three
  -- live-name index keys. IS NOT DISTINCT FROM on the middle key scans siblings.
  SELECT CASE WHEN parent IS NULL THEN EXISTS(
    SELECT 1 FROM documents d WHERE d.node_id=area AND d.parent_container_id IS NULL
      AND d.deleted_at IS NULL AND (identity IS NULL OR kind<>'document' OR d.id<>identity)
      AND translate(d.basename,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz')=translate(label,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz')
    UNION ALL
    SELECT 1 FROM document_containers c WHERE c.node_id=area AND c.parent_container_id IS NULL
      AND c.deleted_at IS NULL AND (identity IS NULL OR kind<>'container' OR c.id<>identity)
      AND translate(c.name,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz')=translate(label,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz'))
  ELSE EXISTS(
    SELECT 1 FROM documents d WHERE d.node_id=area AND d.parent_container_id=parent
      AND d.deleted_at IS NULL AND (identity IS NULL OR kind<>'document' OR d.id<>identity)
      AND translate(d.basename,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz')=translate(label,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz')
    UNION ALL
    SELECT 1 FROM document_containers c WHERE c.node_id=area AND c.parent_container_id=parent
      AND c.deleted_at IS NULL AND (identity IS NULL OR kind<>'container' OR c.id<>identity)
      AND translate(c.name,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz')=translate(label,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz'))
  END
$$;

CREATE FUNCTION public.document_tree_lock(scopes uuid[]) RETURNS void
    LANGUAGE plpgsql
    AS $$
DECLARE scope uuid;
BEGIN
  FOR scope IN SELECT DISTINCT unnest(scopes) ORDER BY 1 LOOP
    INSERT INTO document_tree_scopes(node_id) SELECT id FROM nodes WHERE id=scope ON CONFLICT DO NOTHING;
    PERFORM node_id FROM document_tree_scopes WHERE node_id=scope FOR UPDATE;
  END LOOP;
END
$$;

CREATE FUNCTION public.document_tree_structure() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE old_node uuid; new_node uuid; parent uuid; entry_basename text; entry_kind text; cyclic boolean;
BEGIN
  IF TG_OP <> 'INSERT' THEN old_node := OLD.node_id; END IF;
  IF TG_OP <> 'DELETE' THEN new_node := NEW.node_id; END IF;
  PERFORM document_tree_lock(array_remove(ARRAY[old_node,new_node],NULL));
  IF TG_OP = 'DELETE' THEN
    UPDATE document_tree_scopes SET seq=seq+1 WHERE node_id=old_node;
    RETURN OLD;
  END IF;
  IF NOT EXISTS(SELECT 1 FROM nodes n WHERE n.id=NEW.node_id AND n.org_id=NEW.org_id) THEN
    RAISE EXCEPTION 'document anchor and root organization differ' USING ERRCODE='23514';
  END IF;
  IF TG_TABLE_NAME = 'documents' THEN
    IF TG_OP = 'INSERT' THEN
      IF NEW.basename='' THEN NEW.basename:=document_basename(NEW.title); END IF;
    ELSIF NEW.title IS DISTINCT FROM OLD.title AND NEW.basename=OLD.basename THEN
      NEW.basename:=document_basename(NEW.title);
    END IF;
    entry_basename:=NEW.basename;
    entry_kind:='document';
  ELSE
    entry_basename:=NEW.name;
    entry_kind:='container';
    IF TG_OP='UPDATE' AND NEW.revision=OLD.revision THEN NEW.revision:=OLD.revision+1; END IF;
  END IF;
  IF entry_basename='' OR entry_basename IN ('.','..') OR entry_basename<>btrim(entry_basename)
      OR strpos(entry_basename,'/')>0 OR strpos(entry_basename,chr(92))>0 OR entry_basename ~ '[[:cntrl:]]' THEN
    RAISE EXCEPTION 'invalid document basename' USING ERRCODE='23514';
  END IF;
  IF TG_TABLE_NAME='document_containers' AND (lower(entry_basename) IN ('docs','messages','conversation.md','chat.md')
      OR lower(entry_basename) LIKE '%.files' OR lower(entry_basename) LIKE '%.history') THEN
    RAISE EXCEPTION 'reserved document directory' USING ERRCODE='23514';
  END IF;
  parent:=NEW.parent_container_id;
  IF parent IS NOT NULL THEN
    IF NOT EXISTS(SELECT 1 FROM document_containers p WHERE p.id=parent
        AND p.node_id=NEW.node_id AND p.org_id=NEW.org_id) THEN
      RAISE EXCEPTION 'document parent is not a container in this scope' USING ERRCODE='23514';
    END IF;
    WITH RECURSIVE ancestry AS (
      SELECT id,parent_container_id,deleted_at,ARRAY[id] AS path,false AS cycle FROM document_containers WHERE id=parent
      UNION ALL SELECT p.id,p.parent_container_id,p.deleted_at,a.path||p.id,p.id=ANY(a.path)
        FROM document_containers p JOIN ancestry a ON p.id=a.parent_container_id WHERE NOT a.cycle
    ) SELECT COALESCE(bool_or(cycle OR (TG_TABLE_NAME='document_containers' AND id=NEW.id)
                 OR (NEW.deleted_at IS NULL AND deleted_at IS NOT NULL)),false) INTO cyclic FROM ancestry;
    IF cyclic THEN RAISE EXCEPTION 'document parent is deleted or cyclic' USING ERRCODE='23514'; END IF;
  END IF;
  IF NEW.deleted_at IS NULL THEN
    IF document_namespace_taken(NEW.node_id,parent,entry_basename,entry_kind,NEW.id) THEN
      RAISE EXCEPTION 'a document entry already uses this name' USING ERRCODE='23505';
    END IF;
  END IF;
  UPDATE document_tree_scopes SET seq=seq+1 WHERE node_id IN (old_node,new_node);
  RETURN NEW;
END
$$;

CREATE FUNCTION public.goal_note_retry_attempt() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF NEW.goal_id IS NOT NULL AND NEW.state IN ('cancelled', 'failed') THEN
        IF TG_OP = 'UPDATE' THEN
            IF OLD.goal_id IS NOT DISTINCT FROM NEW.goal_id AND OLD.state IN ('cancelled', 'failed') THEN
                RETURN NEW;
            END IF;
        END IF;
        UPDATE thread_goals SET retry_attempts = retry_attempts + 1 WHERE id = NEW.goal_id;
    END IF;
    RETURN NEW;
END
$$;

CREATE FUNCTION public.import_node_order_changed() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF TG_OP = 'DELETE' THEN
    UPDATE import_origins SET node_depth = NULL
      WHERE object_id = OLD.id AND node_depth IS NOT NULL;
  ELSE
    UPDATE import_origins
       SET node_depth = CASE WHEN NEW.deleted_at IS NULL THEN cardinality(NEW.path) END
     WHERE object_id = NEW.id AND created_by_import IS NOT NULL
       AND kind IN ('org','project','topic','thread')
       AND node_depth IS DISTINCT FROM
           CASE WHEN NEW.deleted_at IS NULL THEN cardinality(NEW.path) END;
  END IF;
  RETURN NULL;
END;
$$;

CREATE FUNCTION public.import_origin_node_depth() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  NEW.node_depth := NULL;
  IF NEW.created_by_import IS NOT NULL AND NEW.kind IN ('org','project','topic','thread') THEN
    -- The shared lock serializes a new origin with a simultaneous hierarchy move.
    SELECT CASE WHEN deleted_at IS NULL THEN cardinality(path) END
      INTO NEW.node_depth FROM nodes WHERE id = NEW.object_id FOR SHARE;
  END IF;
  RETURN NEW;
END;
$$;

CREATE FUNCTION public.job_views_notify() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE changed_id uuid; changed_batch uuid;
BEGIN
  IF TG_OP = 'UPDATE' AND NEW IS NOT DISTINCT FROM OLD THEN RETURN NULL; END IF;
  IF TG_OP = 'DELETE' THEN changed_id := OLD.id; changed_batch := OLD.batch_id;
  ELSE changed_id := NEW.id; changed_batch := NEW.batch_id;
  END IF;
  PERFORM pg_notify('orch_machine_views', 'job:' || changed_id::text);
  IF changed_batch IS NOT NULL THEN
    PERFORM pg_notify('orch_machine_views', 'fork:' || changed_batch::text);
  END IF;
  IF TG_OP = 'UPDATE' AND OLD.batch_id IS DISTINCT FROM NEW.batch_id AND OLD.batch_id IS NOT NULL THEN
    PERFORM pg_notify('orch_machine_views', 'fork:' || OLD.batch_id::text);
  END IF;
  RETURN NULL;
END; $$;

CREATE FUNCTION public.machine_views_notify() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF TG_OP = 'UPDATE' AND NEW IS NOT DISTINCT FROM OLD THEN RETURN NULL; END IF;
  IF TG_OP = 'DELETE' THEN
    PERFORM pg_notify('orch_machine_views', OLD.environment_id::text);
  ELSE
    PERFORM pg_notify('orch_machine_views', NEW.environment_id::text);
  END IF;
  RETURN NULL;
END; $$;

CREATE FUNCTION public.orch_authz_change(moves boolean) RETURNS bigint
    LANGUAGE plpgsql
    AS $$
DECLARE
  scope jsonb := orch_setting_json('orch.authz_scope', '[]');
BEGIN
  -- A scope of "grants" is a change that only adds authority (an invitation made or taken, a member, a token or a
  -- group added): no decision made before it can be wrong after it, so it moves no authorization epoch.
  IF moves AND scope <> '"grants"'::jsonb THEN
    IF jsonb_array_length(scope) = 0 THEN
      scope := '["00000000-0000-0000-0000-000000000000"]';
    END IF;
    PERFORM set_config('orch.authz_advance', (orch_setting_json('orch.authz_advance', '[]') || scope)::text, true);
  END IF;
  IF moves THEN
    PERFORM set_config('orch.guard_epoch', 'write', true);
  END IF;
  PERFORM set_config('orch.authz_bump', 'on', true);
  PERFORM orch_commit_arm();
  RETURN COALESCE((SELECT epoch FROM authz_state WHERE id), 0) + 1;
END
$$;

CREATE FUNCTION public.orch_authz_epochs(orgs uuid[]) RETURNS jsonb
    LANGUAGE sql STABLE
    AS $$
  SELECT COALESCE(jsonb_object_agg(w.o, COALESCE(e.epoch, 0)), '{}')
    FROM (SELECT DISTINCT o FROM unnest(array_append(orgs, '00000000-0000-0000-0000-000000000000'::uuid)) o
           WHERE o IS NOT NULL) w(o)
    LEFT JOIN authz_epochs e ON e.org_id = w.o
$$;

CREATE FUNCTION public.orch_authz_saw(seen jsonb) RETURNS boolean
    LANGUAGE plpgsql
    AS $$
BEGIN
  PERFORM set_config('orch.authz_seen', (COALESCE(seen, '{}') || orch_setting_json('orch.authz_seen', '{}'))::text, true);
  PERFORM orch_commit_arm();
  RETURN true;
END
$$;

CREATE FUNCTION public.orch_cache_program_name() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE wanted text; named jsonb;
BEGIN
    wanted=CASE WHEN TG_OP='DELETE' THEN OLD.kind ELSE NEW.kind END;
    SELECT r.manifest INTO named FROM agent_program_reports r WHERE r.kind=wanted AND NOT r.builtin
      AND jsonb_typeof(r.manifest)='object' ORDER BY reported_at,machine_id LIMIT 1;
    UPDATE agent_program_kinds SET manifest=named WHERE kind=wanted AND manifest IS DISTINCT FROM named;
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.orch_commit_arm() RETURNS boolean
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF COALESCE(current_setting('orch.commit_armed', true), '') = '' THEN
    INSERT INTO commit_work (xact) VALUES (pg_current_xact_id());
    PERFORM set_config('orch.commit_armed', 'on', true);
  END IF;
  RETURN true;
END
$$;

CREATE FUNCTION public.orch_commit_work() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
  seen    jsonb := orch_setting_json('orch.authz_seen', '{}');
  advance uuid[] := ARRAY(SELECT DISTINCT a::uuid FROM jsonb_array_elements_text(orch_setting_json('orch.authz_advance', '[]')) a ORDER BY 1);
  places  uuid[];
  locked_epochs jsonb := '{}';
  moved   uuid;
  n       bigint;
  expect  text := COALESCE(current_setting('orch.cache_expect', true), '');
  root    record;
BEGIN
  -- 1. Authorization: every epoch decided under and every one moved, locked in org order, compared, then moved.
  places := ARRAY(SELECT DISTINCT p FROM (SELECT k::uuid FROM jsonb_object_keys(seen) k UNION ALL SELECT unnest(advance)) x(p) ORDER BY 1);
  -- A watched zero is a real version too. Initialize all missing watched rows so
  -- the first competing authority change cannot insert past an absent-row lock.
  INSERT INTO authz_epochs (org_id) SELECT unnest(places) ORDER BY 1 ON CONFLICT (org_id) DO NOTHING;
  IF cardinality(advance) > 0 THEN
    -- Consume the complete locked set before comparing it. A mismatch filter or
    -- LIMIT in the locking query can skip equal rows and leave them unfenced.
    SELECT COALESCE(jsonb_object_agg(e.org_id::text,e.epoch),'{}') INTO locked_epochs
      FROM (SELECT org_id, epoch FROM authz_epochs WHERE org_id = ANY(places) ORDER BY org_id FOR NO KEY UPDATE) e;
  ELSIF cardinality(places) > 0 THEN
    SELECT COALESCE(jsonb_object_agg(e.org_id::text,e.epoch),'{}') INTO locked_epochs
      FROM (SELECT org_id, epoch FROM authz_epochs WHERE org_id = ANY(places) ORDER BY org_id FOR SHARE) e;
  END IF;
  SELECT e.key::uuid INTO moved FROM jsonb_each_text(locked_epochs) e
   WHERE seen ? e.key AND e.value::bigint <> (seen ->> e.key)::bigint LIMIT 1;
  IF moved IS NOT NULL THEN
    RAISE EXCEPTION 'who may do this changed while it was being done; try again'
      USING ERRCODE = '40001', CONSTRAINT = 'authz_epoch_moved', DETAIL = moved::text;
  END IF;
  IF cardinality(advance) > 0 THEN
    UPDATE authz_epochs SET epoch = epoch + 1 WHERE org_id = ANY(advance);
  END IF;
  -- 2. The cache epoch: moved once and announced with its new value, or checked for a background decision.
  IF COALESCE(current_setting('orch.authz_bump', true), '') <> '' THEN
    INSERT INTO authz_state (id, epoch) VALUES (true, 1)
      ON CONFLICT (id) DO UPDATE SET epoch = authz_state.epoch + 1 RETURNING epoch INTO n;
    IF expect <> '' AND n - 1 <> expect::bigint THEN
      RAISE EXCEPTION 'who may do this changed while it was being done; try again' USING ERRCODE = '40001', CONSTRAINT = 'authz_epoch_moved';
    END IF;
    PERFORM pg_notify('orch_authz', n::text);
  ELSIF expect <> '' THEN
    SELECT COALESCE((SELECT s.epoch FROM authz_state s WHERE s.id FOR SHARE), 0) INTO n;
    IF n <> expect::bigint THEN
      RAISE EXCEPTION 'who may do this changed while it was being done; try again' USING ERRCODE = '40001', CONSTRAINT = 'authz_epoch_moved';
    END IF;
  END IF;
  -- 3. Trees: each root org's next number, for the nodes stamped in it (already locked by this transaction).
  FOR root IN SELECT key::uuid AS org, value AS stamps FROM jsonb_each(orch_setting_json('orch.tree_stamps', '{}')) ORDER BY 1 LOOP
    UPDATE orgs SET tree_seq = tree_seq + 1,
                    tree_changed_at = GREATEST(tree_changed_at, COALESCE((root.stamps ->> 'at')::timestamptz, now()))
     WHERE node_id = root.org RETURNING tree_seq INTO n;
    IF n IS NOT NULL THEN
      UPDATE nodes SET tree_seq = n
       WHERE id IN (SELECT jsonb_array_elements_text(COALESCE(root.stamps -> 'n', '[]'))::uuid);
      IF jsonb_array_length(COALESCE(root.stamps -> 's', '[]')) > 0 THEN
        UPDATE nodes SET tree_seq = n
         WHERE path && ARRAY(SELECT jsonb_array_elements_text(root.stamps -> 's')::uuid);
      END IF;
      PERFORM pg_notify('orch_tree', root.org::text || ':' || n::text);
    END IF;
  END LOOP;
  -- 4. Number each changed row once, in its first-write order within this commit.
  -- Replay of an already committed immutable event keeps its original cursor.
  WITH touched AS MATERIALIZED (
    SELECT table_name, id, min(ordinal) AS first_ordinal
      FROM usage_commit_rows WHERE xact = NEW.xact
     GROUP BY table_name, id
  ), eligible AS MATERIALIZED (
    SELECT touched.* FROM touched JOIN usage_events target ON target.id=touched.id
     WHERE touched.table_name='usage_events' AND target.change_seq=0
    UNION ALL
    SELECT touched.* FROM touched JOIN usage_running target ON target.id=touched.id
     WHERE touched.table_name='usage_running' AND target.change_seq=0
  ), numbered AS MATERIALIZED (
    SELECT table_name, id, row_number() OVER (ORDER BY first_ordinal) AS cursor_offset
      FROM eligible
  ), reserved AS (
    UPDATE usage_change_state SET revision=revision+(SELECT count(*) FROM numbered)
     WHERE singleton AND EXISTS(SELECT 1 FROM numbered)
     RETURNING revision-(SELECT count(*) FROM numbered) AS cursor_base
  ), stamped_events AS (
    UPDATE usage_events target SET change_seq=reserved.cursor_base+numbered.cursor_offset
      FROM numbered CROSS JOIN reserved
     WHERE numbered.table_name='usage_events' AND target.id=numbered.id
     RETURNING target.id
  )
  UPDATE usage_running target SET change_seq=reserved.cursor_base+numbered.cursor_offset
    FROM numbered CROSS JOIN reserved
   WHERE numbered.table_name='usage_running' AND target.id=numbered.id;
  DELETE FROM usage_commit_rows WHERE xact=NEW.xact;
  DELETE FROM commit_work WHERE xact = NEW.xact;
  RETURN NULL;
END
$$;

CREATE FUNCTION public.orch_document_comment_reference_refresh() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF TG_OP='DELETE' THEN
    DELETE FROM related_references WHERE source_kind='comment' AND part_id=OLD.id;
    RETURN OLD;
  END IF;
  DELETE FROM related_references WHERE source_kind='comment' AND part_id=NEW.id;
  IF NEW.deleted_at IS NULL THEN
    INSERT INTO related_references (source_kind,source_id,part_id,target_kind,target_id,target_path_key)
    SELECT 'comment',NEW.document_id,NEW.id,kind,id,path_key FROM orch_related_targets(NEW.content)
    ON CONFLICT DO NOTHING;
  END IF;
  RETURN NEW;
END
$$;

CREATE FUNCTION public.orch_document_reference_moments() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF TG_TABLE_NAME='document_blocks' THEN
    UPDATE related_references SET source_moment=NEW.updated_at
      WHERE source_kind='document' AND part_id=NEW.id AND source_moment IS DISTINCT FROM NEW.updated_at;
  ELSE
    UPDATE related_references SET source_moment=NEW.created_at
      WHERE source_kind='comment' AND part_id=NEW.id AND source_moment IS DISTINCT FROM NEW.created_at;
  END IF;
  RETURN NEW;
END
$$;

CREATE FUNCTION public.orch_document_summary_due() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    INSERT INTO document_summary_state (document_id, due_at, next_at)
        SELECT d.id, NEW.updated_at, NEW.updated_at FROM documents d
        WHERE d.id = NEW.document_id AND d.description_source = 'auto' AND d.deleted_at IS NULL
        ON CONFLICT (document_id) DO UPDATE
            SET due_at = LEAST(COALESCE(document_summary_state.due_at, EXCLUDED.due_at), EXCLUDED.due_at)
            WHERE document_summary_state.due_at IS NULL OR EXCLUDED.due_at < document_summary_state.due_at;
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.orch_item_internal_marker() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
  written text := NEW.payload::text;
BEGIN
  -- The body is written out once: for Orch's internal marker, and for the length a reader decodes,
  -- which a plan and a native proof compare with their limits (the stored size is compressed).
  NEW.contains_internal_marker:=COALESCE(NEW.search_text LIKE '%<orch>%',false)
    OR written LIKE '%<orch>%';
  NEW.payload_bytes:=octet_length(written);
  RETURN NEW;
END
$$;

CREATE FUNCTION public.orch_machine_resources_unavailable() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF NEW.lost_at IS NULL AND NEW.deleted_at IS NULL THEN RETURN NEW; END IF;
  UPDATE mcp_server_offerings SET state='lost',lost_at=COALESCE(NEW.lost_at,NEW.deleted_at,NEW.updated_at),reason='No machine'
    WHERE machine_id=NEW.id AND state<>'lost';
  UPDATE mcp_servers s SET machine_id=(SELECT o.machine_id FROM mcp_server_offerings o JOIN machines m ON m.id=o.machine_id
      WHERE o.server_id=s.id AND o.state='ready' AND m.status='active' AND m.deleted_at IS NULL AND m.lost_at IS NULL ORDER BY o.offered_at,o.machine_id LIMIT 1),
      revision=revision+1,updated_at=NEW.updated_at
    WHERE s.imported AND (s.machine_id=NEW.id OR EXISTS(SELECT 1 FROM mcp_server_offerings o WHERE o.server_id=s.id AND o.machine_id=NEW.id));
  INSERT INTO mount_lost_locations(id,mount_id,machine_id,machine_name,host_name,storage,total_bytes,size_at,lost_at)
    SELECT gen_random_uuid(),l.mount_id,NEW.id,NEW.name,h.name,l.storage,l.last_bytes,l.size_at,COALESCE(NEW.lost_at,NEW.deleted_at,NEW.updated_at)
      FROM mount_locations l LEFT JOIN hosts h ON h.machine_id=NEW.id WHERE l.machine_id=NEW.id AND l.kind='volume';
  DELETE FROM mount_locations WHERE machine_id=NEW.id;
  INSERT INTO mount_lost_locations(id,mount_id,machine_id,machine_name,host_name,storage,total_bytes,size_at,lost_at)
    SELECT k.id,k.mount_id,NEW.id,NEW.name,h.name,k.storage,k.total_bytes,k.kept_at,COALESCE(NEW.lost_at,NEW.deleted_at,NEW.updated_at)
      FROM mount_kept_data k LEFT JOIN hosts h ON h.machine_id=NEW.id WHERE k.machine_id=NEW.id AND k.kind='volume' AND k.lost_at IS NULL
      ON CONFLICT(id) DO NOTHING;
  UPDATE mount_kept_data SET lost_at=COALESCE(NEW.lost_at,NEW.deleted_at,NEW.updated_at) WHERE machine_id=NEW.id AND lost_at IS NULL;
  RETURN NEW;
END $$;

CREATE FUNCTION public.orch_mcp_offerings_discovered() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE found jsonb; server uuid; candidates bigint; imported_job uuid; existing_state text; existing_source jsonb; changed integer;
BEGIN
  IF NEW.machine_id IS NULL OR NEW.deleted_at IS NOT NULL OR NEW.lost_at IS NOT NULL THEN RETURN NEW; END IF;
  IF NOT EXISTS(SELECT 1 FROM machines WHERE id=NEW.machine_id AND status='active' AND deleted_at IS NULL AND lost_at IS NULL) THEN RETURN NEW; END IF;
  FOR found IN SELECT value FROM jsonb_array_elements(CASE WHEN jsonb_typeof(NEW.facts->'mcp_found')='array' THEN NEW.facts->'mcp_found' ELSE '[]'::jsonb END) ORDER BY value->>'name' LOOP
    SELECT count(*),(array_agg(id ORDER BY id))[1] INTO candidates,server FROM mcp_servers
      WHERE imported AND deleted_at IS NULL AND home_id=NEW.org_id AND COALESCE(spec->'imported'->>'name',name)=found->>'name';
    IF candidates<>1 THEN CONTINUE; END IF;
    PERFORM 1 FROM mcp_servers WHERE id=server FOR UPDATE;
    SELECT state,source INTO existing_state,existing_source FROM mcp_server_offerings WHERE server_id=server AND machine_id=NEW.machine_id;
    IF existing_state='ready' THEN
      IF NOT EXISTS(SELECT 1 FROM mcp_servers WHERE id=server AND spec->'imported'->>'definition_fingerprint'=found->>'definition_fingerprint' AND spec->>'transport'=found->>'transport') THEN
        UPDATE mcp_server_offerings SET proposal=jsonb_build_object('environment',NEW.id,'source',found,
          'reason',CASE WHEN found->>'definition_fingerprint' IS NULL OR (SELECT spec->'imported'->>'definition_fingerprint' FROM mcp_servers WHERE id=server) IS NULL THEN 'definition_unknown' ELSE 'different_definition' END),seen_at=NEW.updated_at
          WHERE server_id=server AND machine_id=NEW.machine_id AND proposal IS DISTINCT FROM jsonb_build_object('environment',NEW.id,'source',found,
          'reason',CASE WHEN found->>'definition_fingerprint' IS NULL OR (SELECT spec->'imported'->>'definition_fingerprint' FROM mcp_servers WHERE id=server) IS NULL THEN 'definition_unknown' ELSE 'different_definition' END);
        GET DIAGNOSTICS changed=ROW_COUNT;
        IF changed>0 THEN UPDATE mcp_servers SET revision=revision+1,updated_at=NEW.updated_at WHERE id=server; END IF;
      ELSE UPDATE mcp_server_offerings SET seen_at=NEW.updated_at WHERE server_id=server AND machine_id=NEW.machine_id; END IF;
      CONTINUE;
    END IF;
    IF (existing_state IN ('ready','pending') AND existing_source->>'digest'=found->>'digest'
        AND existing_source->>'definition_fingerprint' IS NOT DISTINCT FROM found->>'definition_fingerprint')
        OR (existing_state='failed' AND existing_source->>'digest'=found->>'digest') THEN
      UPDATE mcp_server_offerings SET seen_at=NEW.updated_at WHERE server_id=server AND machine_id=NEW.machine_id;
      CONTINUE;
    END IF;
    IF existing_state='found' AND existing_source=found AND NOT EXISTS(SELECT 1 FROM mcp_servers WHERE id=server
         AND spec->'imported'->>'definition_fingerprint'=found->>'definition_fingerprint' AND spec->>'transport'=found->>'transport') THEN
      UPDATE mcp_server_offerings SET seen_at=NEW.updated_at WHERE server_id=server AND machine_id=NEW.machine_id;
      CONTINUE;
    END IF;
    IF found->>'definition_fingerprint' IS NULL OR NOT EXISTS(SELECT 1 FROM mcp_servers WHERE id=server
        AND spec->'imported'->>'definition_fingerprint'=found->>'definition_fingerprint' AND spec->>'transport'=found->>'transport') THEN
      INSERT INTO mcp_server_offerings(server_id,machine_id,environment_id,state,source,seen_at,reason)
        VALUES(server,NEW.machine_id,NEW.id,'found',found,NEW.updated_at,
          CASE WHEN found->>'definition_fingerprint' IS NULL OR (SELECT spec->'imported'->>'definition_fingerprint' FROM mcp_servers WHERE id=server) IS NULL THEN 'definition_unknown' ELSE 'different_definition' END)
        ON CONFLICT(server_id,machine_id) DO UPDATE SET environment_id=EXCLUDED.environment_id,state='found',source=EXCLUDED.source,
          seen_at=EXCLUDED.seen_at,reason=EXCLUDED.reason,lost_at=NULL,job_id=NULL;
      UPDATE mcp_servers SET revision=revision+1,updated_at=NEW.updated_at WHERE id=server;
      CONTINUE;
    END IF;
    imported_job:=gen_random_uuid();
    INSERT INTO jobs(id,kind,environment_id,state,plan,progress,created_at,updated_at)
      VALUES(imported_job,'mcp_offering',NEW.id,'queued',jsonb_build_object('server',server,'machine',NEW.machine_id,'environment',NEW.id,'source',found),
        jsonb_build_object('phase','import','phases',jsonb_build_array(jsonb_build_object('name','import','weight',100,'state','pending')),
          'percent',0,'files_done',0,'files_total',NULL,'bytes_done',0,'bytes_total',NULL,'rate_bps',NULL,'eta_seconds',NULL,'message',NULL),NEW.updated_at,NEW.updated_at);
    INSERT INTO mcp_server_offerings(server_id,machine_id,environment_id,state,source,seen_at,job_id)
      VALUES(server,NEW.machine_id,NEW.id,'pending',found,NEW.updated_at,imported_job)
      ON CONFLICT(server_id,machine_id) DO UPDATE SET environment_id=EXCLUDED.environment_id,state='pending',source=EXCLUDED.source,
        seen_at=EXCLUDED.seen_at,job_id=EXCLUDED.job_id,lost_at=NULL,reason=NULL;
  END LOOP;
  RETURN NEW;
END $$;

CREATE FUNCTION public.orch_participation_after_keys(at timestamp with time zone) RETURNS TABLE(time_shift smallint, time_bucket bigint)
    LANGUAGE sql IMMUTABLE PARALLEL SAFE
    AS $$
 SELECT 64::smallint,0::bigint
 UNION ALL SELECT s::smallint,(orch_participation_micros(at)>>s)+1 FROM generate_series(0,62) s
   WHERE ((orch_participation_micros(at)>>s)&1)=0
 UNION ALL SELECT 63::smallint,0::bigint WHERE orch_participation_micros(at)<0
$$;

CREATE FUNCTION public.orch_participation_entries_changed() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 DECLARE refresh_keys text := CASE TG_OP WHEN 'INSERT' THEN 'SELECT id FROM participation_new_entries' WHEN 'DELETE' THEN 'SELECT id FROM participation_old_entries' ELSE 'SELECT id FROM participation_new_entries UNION SELECT id FROM participation_old_entries' END; refresh_kind text := NULL; entry_scope text; source_scope text;
BEGIN
 IF refresh_kind IS NULL THEN
  entry_scope:='e.id IN(SELECT id FROM changed)'; source_scope:='s.entry_id IN(SELECT id FROM changed)';
 ELSE
  entry_scope:=format('e.object_kind=%L AND e.object_id IN(SELECT id FROM changed)',refresh_kind);
  source_scope:=format('s.object_kind=%L AND s.object_id IN(SELECT id FROM changed)',refresh_kind);
 END IF;
 -- Entry changes lock their objects before reading eligibility. Object triggers already hold these locks.
 IF refresh_kind IS NULL THEN
 EXECUTE 'WITH changed AS ('||refresh_keys||') SELECT f.object_kind,f.object_id FROM participation_object_facts f
  WHERE EXISTS(SELECT 1 FROM permission_entries e WHERE '||entry_scope||'
   AND e.object_kind=f.object_kind AND e.object_id=f.object_id)
  ORDER BY f.object_kind,f.object_id FOR SHARE OF f';
 END IF;
 EXECUTE 'WITH changed AS ('||refresh_keys||') DELETE FROM participation_entries s WHERE ('||source_scope||')
  AND NOT EXISTS(SELECT 1 FROM permission_entries e JOIN participation_object_facts f
   ON f.object_kind=e.object_kind AND f.object_id=e.object_id AND f.live
   WHERE e.id=s.entry_id AND e.removed_at IS NULL AND e.holder_kind IN(''user'',''group'') AND e.holder_id IS NOT NULL
    AND (e.permission<>''access.own'' OR f.org_id IS NOT NULL))';
 EXECUTE 'WITH changed AS ('||refresh_keys||') MERGE INTO participation_entries s USING (
  SELECT e.id entry_id,e.object_kind,e.object_id,f.home_id,e.holder_kind subject_kind,e.holder_id subject_id,
   CASE WHEN e.permission=''access.own'' THEN f.org_id END owner_org,e.expires_at
  FROM permission_entries e JOIN participation_object_facts f
   ON f.object_kind=e.object_kind AND f.object_id=e.object_id AND f.live
  WHERE ('||entry_scope||') AND e.removed_at IS NULL AND e.holder_kind IN(''user'',''group'') AND e.holder_id IS NOT NULL
   AND (e.permission<>''access.own'' OR f.org_id IS NOT NULL)
 ) a ON s.entry_id=a.entry_id
 WHEN MATCHED AND (s.object_kind,s.object_id,s.home_id,s.subject_kind,s.subject_id,s.owner_org,s.expires_at)
  IS DISTINCT FROM (a.object_kind,a.object_id,a.home_id,a.subject_kind,a.subject_id,a.owner_org,a.expires_at)
 THEN UPDATE SET object_kind=a.object_kind,object_id=a.object_id,home_id=a.home_id,
  subject_kind=a.subject_kind,subject_id=a.subject_id,owner_org=a.owner_org,expires_at=a.expires_at
 WHEN NOT MATCHED BY TARGET THEN INSERT(entry_id,object_kind,object_id,home_id,subject_kind,subject_id,owner_org,expires_at)
  VALUES(a.entry_id,a.object_kind,a.object_id,a.home_id,a.subject_kind,a.subject_id,a.owner_org,a.expires_at)';
END;
 RETURN NULL;
END $$;

CREATE FUNCTION public.orch_participation_micros(at timestamp with time zone) RETURNS bigint
    LANGUAGE sql IMMUTABLE PARALLEL SAFE
    AS $$
 SELECT CASE WHEN at='infinity'::timestamptz THEN 9223372036854775807::bigint
             WHEN at='-infinity'::timestamptz THEN '-9223372036854775808'::bigint
             ELSE ((extract(epoch FROM at)-946684800)*1000000)::bigint END
$$;

CREATE FUNCTION public.orch_participation_node_count(marker uuid, person uuid, groups uuid[], members uuid[], roots uuid[], at timestamp with time zone) RETURNS bigint
    LANGUAGE sql STABLE
    AS $$
 SELECT coalesce(sum(held.total),0)::bigint FROM (
  SELECT 'user'::text kind,person id UNION SELECT 'group',id FROM unnest(groups) g(id)
 ) subject CROSS JOIN unnest(roots) root(id) CROSS JOIN orch_participation_after_keys(at) t
 CROSS JOIN LATERAL (
  SELECT c.total FROM participation_node_counts c
  WHERE c.subject_kind=subject.kind AND c.subject_id=subject.id AND c.node_id=marker AND c.scope_root=root.id
   AND c.time_shift=t.time_shift AND c.time_bucket=t.time_bucket
   AND (c.owner_org IS NULL OR c.owner_org=ANY(members)) AND c.total>0 OFFSET 0
 ) held
$$;

CREATE FUNCTION public.orch_participation_objects_changed() RETURNS trigger
    LANGUAGE plpgsql
    AS $_$
DECLARE kind text; home_column text; live_sql text; joins text:=''; keys_sql text;
BEGIN
 keys_sql:=CASE TG_OP WHEN 'INSERT' THEN 'SELECT id FROM participation_new_objects'
  WHEN 'DELETE' THEN 'SELECT id FROM participation_old_objects'
  ELSE 'SELECT id FROM participation_new_objects UNION SELECT id FROM participation_old_objects' END;
 SELECT x.kind,x.home_column,x.live_sql INTO kind,home_column,live_sql FROM (VALUES
  ('nodes','node','id','o.deleted_at IS NULL'),
  ('documents','document','node_id','o.deleted_at IS NULL'),
  ('providers','provider','home_id','o.deleted_at IS NULL'),
  ('agents','agent','home_id','o.deleted_at IS NULL'),
  ('environments','environment','home_node_id','o.deleted_at IS NULL'),
  ('hosts','host','home_id','m.id IS NOT NULL AND m.deleted_at IS NULL'),
  ('machines','machine','home_id','o.deleted_at IS NULL'),
  ('env_templates','template','home_id','o.deleted_at IS NULL'),
  ('mcp_servers','mcp_server','home_id','o.deleted_at IS NULL'),
  ('mounts','mount','home_id','o.deleted_at IS NULL'),
  ('skills','skill','home_id','o.deleted_at IS NULL')
 ) x(table_name,kind,home_column,live_sql) WHERE x.table_name=TG_TABLE_NAME;
 IF kind='host' THEN joins:=' LEFT JOIN machines m ON m.id=o.machine_id '; END IF;
 EXECUTE format('WITH changed AS (%s) SELECT f.object_id FROM participation_object_facts f
  WHERE f.object_kind=%L AND f.object_id IN(SELECT id FROM changed) ORDER BY f.object_id FOR UPDATE OF f',keys_sql,kind);
 IF kind='node' THEN DECLARE path_keys text := keys_sql; prefix text; fresh_live_paths boolean:=false;
BEGIN
 -- Live inserts cannot change an existing path's liveness. Existing homes reference existing nodes,
 -- and new node assignments are refreshed after their placement has been added.
 IF TG_OP='INSERT' THEN
  fresh_live_paths:=NOT EXISTS(SELECT 1 FROM participation_new_objects WHERE deleted_at IS NOT NULL);
  IF fresh_live_paths THEN
   INSERT INTO participation_paths(node_id,path,live)
   SELECT n.id,n.path,NOT EXISTS(SELECT 1 FROM nodes gone WHERE gone.id=ANY(n.path) AND gone.deleted_at IS NOT NULL)
   FROM participation_new_objects n;
  END IF;
 END IF;
 IF NOT fresh_live_paths THEN
 prefix:='WITH changed AS (
   SELECT c.id FROM ('||path_keys||') c
   LEFT JOIN nodes n ON n.id=c.id LEFT JOIN participation_paths p ON p.node_id=c.id
   LEFT JOIN participation_object_facts f ON f.object_kind=''node'' AND f.object_id=c.id
   WHERE n.id IS NULL OR p.node_id IS NULL OR p.path IS DISTINCT FROM n.path
    OR f.live IS DISTINCT FROM (n.deleted_at IS NULL)
  ), affected AS (
   SELECT p.node_id FROM changed c JOIN participation_paths p ON p.path @> ARRAY[c.id]
   UNION SELECT n.id FROM changed c JOIN nodes n ON n.path @> ARRAY[c.id]
   UNION SELECT id FROM changed), paths AS (
   SELECT a.node_id,p.path old_path,p.live old_live,n.path new_path,
    n.id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM nodes gone WHERE gone.id=ANY(n.path) AND gone.deleted_at IS NOT NULL) new_live
   FROM affected a LEFT JOIN participation_paths p ON p.node_id=a.node_id LEFT JOIN nodes n ON n.id=a.node_id) ';
 EXECUTE prefix||'SELECT p.node_id FROM participation_paths p JOIN paths x USING(node_id)
  WHERE (x.old_path,x.old_live) IS DISTINCT FROM (x.new_path,x.new_live)
  ORDER BY p.node_id FOR UPDATE OF p';
 EXECUTE prefix||$query$, changes AS (
 SELECT h.*,p.old_path path,-h.total delta FROM participation_home_counts h JOIN paths p ON p.node_id=h.home_id
 WHERE p.old_live AND (p.old_path,p.old_live) IS DISTINCT FROM (p.new_path,p.new_live)
 UNION ALL
 SELECT h.*,p.new_path path,h.total delta FROM participation_home_counts h JOIN paths p ON p.node_id=h.home_id
 WHERE p.new_live AND (p.old_path,p.old_live) IS DISTINCT FROM (p.new_path,p.new_live)
), counts AS (
 SELECT subject_kind,subject_id,a.node_id,path[1] scope_root,time_shift,time_bucket,owner_org,sum(delta)::bigint total
 FROM changes CROSS JOIN LATERAL unnest(path) a(node_id)
 GROUP BY subject_kind,subject_id,a.node_id,path[1],time_shift,time_bucket,owner_org HAVING sum(delta)<>0)
$query$||$query$INSERT INTO participation_node_counts AS held(subject_kind,subject_id,node_id,scope_root,time_shift,time_bucket,owner_org,total)
SELECT subject_kind,subject_id,node_id,scope_root,time_shift,time_bucket,owner_org,total FROM counts ORDER BY subject_kind,subject_id,node_id,scope_root,time_shift,time_bucket,owner_org
ON CONFLICT(subject_kind,subject_id,node_id,scope_root,time_shift,time_bucket,owner_org) DO UPDATE SET total=held.total+EXCLUDED.total$query$;

DELETE FROM participation_node_counts WHERE total=0;
IF EXISTS(SELECT 1 FROM participation_node_counts WHERE total<0)
THEN RAISE EXCEPTION 'negative participation count'; END IF;
 EXECUTE prefix||'INSERT INTO participation_paths(node_id,path,live)
 SELECT node_id,new_path,new_live FROM paths WHERE new_path IS NOT NULL
 ON CONFLICT(node_id) DO UPDATE SET path=EXCLUDED.path,live=EXCLUDED.live
 WHERE (participation_paths.path,participation_paths.live) IS DISTINCT FROM (EXCLUDED.path,EXCLUDED.live)';
 EXECUTE 'WITH changed AS ('||path_keys||') DELETE FROM participation_paths p
 WHERE p.node_id IN(SELECT id FROM changed) AND NOT EXISTS(SELECT 1 FROM nodes n WHERE n.id=p.node_id)';
 END IF;
END; END IF;
 EXECUTE format('WITH changed AS (%s) INSERT INTO participation_object_facts AS held(object_kind,object_id,home_id,org_id,live)
  SELECT %L,o.id,o.%I,o.org_id,(%s) FROM %I o %s JOIN changed c ON c.id=o.id
  ON CONFLICT(object_kind,object_id) DO UPDATE SET home_id=EXCLUDED.home_id,org_id=EXCLUDED.org_id,live=EXCLUDED.live
  WHERE (held.home_id,held.org_id,held.live) IS DISTINCT FROM (EXCLUDED.home_id,EXCLUDED.org_id,EXCLUDED.live)',
  keys_sql,kind,home_column,live_sql,TG_TABLE_NAME,joins);
 EXECUTE format('WITH changed AS (%s) DELETE FROM participation_object_facts f WHERE f.object_kind=%L
  AND f.object_id IN(SELECT id FROM changed) AND NOT EXISTS(SELECT 1 FROM %I o WHERE o.id=f.object_id)',keys_sql,kind,TG_TABLE_NAME);
 DECLARE refresh_keys text := keys_sql; refresh_kind text := kind; entry_scope text; source_scope text;
BEGIN
 IF refresh_kind IS NULL THEN
  entry_scope:='e.id IN(SELECT id FROM changed)'; source_scope:='s.entry_id IN(SELECT id FROM changed)';
 ELSE
  entry_scope:=format('e.object_kind=%L AND e.object_id IN(SELECT id FROM changed)',refresh_kind);
  source_scope:=format('s.object_kind=%L AND s.object_id IN(SELECT id FROM changed)',refresh_kind);
 END IF;
 -- Entry changes lock their objects before reading eligibility. Object triggers already hold these locks.
 IF refresh_kind IS NULL THEN
 EXECUTE 'WITH changed AS ('||refresh_keys||') SELECT f.object_kind,f.object_id FROM participation_object_facts f
  WHERE EXISTS(SELECT 1 FROM permission_entries e WHERE '||entry_scope||'
   AND e.object_kind=f.object_kind AND e.object_id=f.object_id)
  ORDER BY f.object_kind,f.object_id FOR SHARE OF f';
 END IF;
 EXECUTE 'WITH changed AS ('||refresh_keys||') DELETE FROM participation_entries s WHERE ('||source_scope||')
  AND NOT EXISTS(SELECT 1 FROM permission_entries e JOIN participation_object_facts f
   ON f.object_kind=e.object_kind AND f.object_id=e.object_id AND f.live
   WHERE e.id=s.entry_id AND e.removed_at IS NULL AND e.holder_kind IN(''user'',''group'') AND e.holder_id IS NOT NULL
    AND (e.permission<>''access.own'' OR f.org_id IS NOT NULL))';
 EXECUTE 'WITH changed AS ('||refresh_keys||') MERGE INTO participation_entries s USING (
  SELECT e.id entry_id,e.object_kind,e.object_id,f.home_id,e.holder_kind subject_kind,e.holder_id subject_id,
   CASE WHEN e.permission=''access.own'' THEN f.org_id END owner_org,e.expires_at
  FROM permission_entries e JOIN participation_object_facts f
   ON f.object_kind=e.object_kind AND f.object_id=e.object_id AND f.live
  WHERE ('||entry_scope||') AND e.removed_at IS NULL AND e.holder_kind IN(''user'',''group'') AND e.holder_id IS NOT NULL
   AND (e.permission<>''access.own'' OR f.org_id IS NOT NULL)
 ) a ON s.entry_id=a.entry_id
 WHEN MATCHED AND (s.object_kind,s.object_id,s.home_id,s.subject_kind,s.subject_id,s.owner_org,s.expires_at)
  IS DISTINCT FROM (a.object_kind,a.object_id,a.home_id,a.subject_kind,a.subject_id,a.owner_org,a.expires_at)
 THEN UPDATE SET object_kind=a.object_kind,object_id=a.object_id,home_id=a.home_id,
  subject_kind=a.subject_kind,subject_id=a.subject_id,owner_org=a.owner_org,expires_at=a.expires_at
 WHEN NOT MATCHED BY TARGET THEN INSERT(entry_id,object_kind,object_id,home_id,subject_kind,subject_id,owner_org,expires_at)
  VALUES(a.entry_id,a.object_kind,a.object_id,a.home_id,a.subject_kind,a.subject_id,a.owner_org,a.expires_at)';
END;
 IF kind='machine' THEN
  -- Host participation follows the machine's liveness, not an independent host flag.
  EXECUTE 'WITH changed AS ('||keys_sql||') UPDATE participation_object_facts f SET live=m.deleted_at IS NULL
   FROM hosts h JOIN machines m ON m.id=h.machine_id WHERE f.object_kind=''host'' AND f.object_id=h.id
    AND m.id IN(SELECT id FROM changed) AND f.live IS DISTINCT FROM (m.deleted_at IS NULL)';
  DECLARE refresh_keys text := 'SELECT h.id FROM hosts h WHERE h.machine_id IN('||keys_sql||')'; refresh_kind text := 'host'; entry_scope text; source_scope text;
BEGIN
 IF refresh_kind IS NULL THEN
  entry_scope:='e.id IN(SELECT id FROM changed)'; source_scope:='s.entry_id IN(SELECT id FROM changed)';
 ELSE
  entry_scope:=format('e.object_kind=%L AND e.object_id IN(SELECT id FROM changed)',refresh_kind);
  source_scope:=format('s.object_kind=%L AND s.object_id IN(SELECT id FROM changed)',refresh_kind);
 END IF;
 -- Entry changes lock their objects before reading eligibility. Object triggers already hold these locks.
 IF refresh_kind IS NULL THEN
 EXECUTE 'WITH changed AS ('||refresh_keys||') SELECT f.object_kind,f.object_id FROM participation_object_facts f
  WHERE EXISTS(SELECT 1 FROM permission_entries e WHERE '||entry_scope||'
   AND e.object_kind=f.object_kind AND e.object_id=f.object_id)
  ORDER BY f.object_kind,f.object_id FOR SHARE OF f';
 END IF;
 EXECUTE 'WITH changed AS ('||refresh_keys||') DELETE FROM participation_entries s WHERE ('||source_scope||')
  AND NOT EXISTS(SELECT 1 FROM permission_entries e JOIN participation_object_facts f
   ON f.object_kind=e.object_kind AND f.object_id=e.object_id AND f.live
   WHERE e.id=s.entry_id AND e.removed_at IS NULL AND e.holder_kind IN(''user'',''group'') AND e.holder_id IS NOT NULL
    AND (e.permission<>''access.own'' OR f.org_id IS NOT NULL))';
 EXECUTE 'WITH changed AS ('||refresh_keys||') MERGE INTO participation_entries s USING (
  SELECT e.id entry_id,e.object_kind,e.object_id,f.home_id,e.holder_kind subject_kind,e.holder_id subject_id,
   CASE WHEN e.permission=''access.own'' THEN f.org_id END owner_org,e.expires_at
  FROM permission_entries e JOIN participation_object_facts f
   ON f.object_kind=e.object_kind AND f.object_id=e.object_id AND f.live
  WHERE ('||entry_scope||') AND e.removed_at IS NULL AND e.holder_kind IN(''user'',''group'') AND e.holder_id IS NOT NULL
   AND (e.permission<>''access.own'' OR f.org_id IS NOT NULL)
 ) a ON s.entry_id=a.entry_id
 WHEN MATCHED AND (s.object_kind,s.object_id,s.home_id,s.subject_kind,s.subject_id,s.owner_org,s.expires_at)
  IS DISTINCT FROM (a.object_kind,a.object_id,a.home_id,a.subject_kind,a.subject_id,a.owner_org,a.expires_at)
 THEN UPDATE SET object_kind=a.object_kind,object_id=a.object_id,home_id=a.home_id,
  subject_kind=a.subject_kind,subject_id=a.subject_id,owner_org=a.owner_org,expires_at=a.expires_at
 WHEN NOT MATCHED BY TARGET THEN INSERT(entry_id,object_kind,object_id,home_id,subject_kind,subject_id,owner_org,expires_at)
  VALUES(a.entry_id,a.object_kind,a.object_id,a.home_id,a.subject_kind,a.subject_id,a.owner_org,a.expires_at)';
END;
 END IF;
 RETURN NULL;
END $_$;

CREATE FUNCTION public.orch_participation_source_counts() RETURNS trigger
    LANGUAGE plpgsql
    AS $_$
DECLARE changes text; prefix text;
BEGIN
 changes:=CASE TG_OP
  WHEN 'INSERT' THEN 'SELECT n.*,1::bigint delta FROM participation_new_sources n'
  WHEN 'DELETE' THEN 'SELECT o.*,-1::bigint delta FROM participation_old_sources o'
  ELSE 'SELECT n.*,1::bigint delta FROM participation_new_sources n UNION ALL SELECT o.*,-1::bigint delta FROM participation_old_sources o' END;
 -- Hold the kept placement while adjusting its counters; a concurrent move waits for this transaction.
 EXECUTE 'WITH changes AS ('||changes||') SELECT p.node_id FROM participation_paths p
   WHERE p.node_id IN(SELECT home_id FROM changes) ORDER BY p.node_id FOR SHARE OF p';
 prefix:='WITH changes AS ('||changes||'), timed AS (
   SELECT c.*,t.time_shift,t.time_bucket FROM changes c
   CROSS JOIN LATERAL orch_participation_time_keys(c.expires_at) t) ';
 EXECUTE prefix||$query$INSERT INTO participation_direct_counts AS held(subject_kind,subject_id,object_kind,object_id,time_shift,time_bucket,owner_org,total)
SELECT subject_kind,subject_id,object_kind,object_id,time_shift,time_bucket,owner_org,sum(delta)::bigint total FROM timed GROUP BY subject_kind,subject_id,object_kind,object_id,time_shift,time_bucket,owner_org HAVING sum(delta)<>0 ORDER BY subject_kind,subject_id,object_kind,object_id,time_shift,time_bucket,owner_org
ON CONFLICT(subject_kind,subject_id,object_kind,object_id,time_shift,time_bucket,owner_org) DO UPDATE SET total=held.total+EXCLUDED.total$query$;

DELETE FROM participation_direct_counts WHERE total=0;
IF EXISTS(SELECT 1 FROM participation_direct_counts WHERE total<0)
THEN RAISE EXCEPTION 'negative participation count'; END IF;
 EXECUTE prefix||$query$INSERT INTO participation_home_counts AS held(subject_kind,subject_id,home_id,time_shift,time_bucket,owner_org,total)
SELECT subject_kind,subject_id,home_id,time_shift,time_bucket,owner_org,sum(delta)::bigint total FROM timed WHERE home_id IS NOT NULL GROUP BY subject_kind,subject_id,home_id,time_shift,time_bucket,owner_org HAVING sum(delta)<>0 ORDER BY subject_kind,subject_id,home_id,time_shift,time_bucket,owner_org
ON CONFLICT(subject_kind,subject_id,home_id,time_shift,time_bucket,owner_org) DO UPDATE SET total=held.total+EXCLUDED.total$query$;

DELETE FROM participation_home_counts WHERE total=0;
IF EXISTS(SELECT 1 FROM participation_home_counts WHERE total<0)
THEN RAISE EXCEPTION 'negative participation count'; END IF;
 EXECUTE prefix||$query$INSERT INTO participation_node_counts AS held(subject_kind,subject_id,node_id,scope_root,time_shift,time_bucket,owner_org,total)
SELECT subject_kind,subject_id,a.node_id,p.path[1] scope_root,time_shift,time_bucket,owner_org,
 sum(delta)::bigint total FROM timed c JOIN participation_paths p ON p.node_id=c.home_id AND p.live
 CROSS JOIN LATERAL unnest(p.path) a(node_id)
 GROUP BY subject_kind,subject_id,a.node_id,p.path[1],time_shift,time_bucket,owner_org HAVING sum(delta)<>0 ORDER BY subject_kind,subject_id,node_id,scope_root,time_shift,time_bucket,owner_org
ON CONFLICT(subject_kind,subject_id,node_id,scope_root,time_shift,time_bucket,owner_org) DO UPDATE SET total=held.total+EXCLUDED.total$query$;

DELETE FROM participation_node_counts WHERE total=0;
IF EXISTS(SELECT 1 FROM participation_node_counts WHERE total<0)
THEN RAISE EXCEPTION 'negative participation count'; END IF;
 RETURN NULL;
END $_$;

CREATE FUNCTION public.orch_participation_time_keys(at timestamp with time zone) RETURNS TABLE(time_shift smallint, time_bucket bigint)
    LANGUAGE sql IMMUTABLE PARALLEL SAFE
    AS $$
 SELECT 64::smallint,0::bigint WHERE at IS NULL
 UNION ALL SELECT s::smallint,orch_participation_micros(at)>>s FROM generate_series(0,62) s
   WHERE at IS NOT NULL AND ((orch_participation_micros(at)>>s)&1)=1
 UNION ALL SELECT 63::smallint,0::bigint WHERE at IS NOT NULL AND orch_participation_micros(at)>=0
$$;

CREATE FUNCTION public.orch_pending_notice_lane_changed() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF orch_pending_notice_lane(OLD) IS DISTINCT FROM orch_pending_notice_lane(NEW) THEN
    UPDATE thread_pending_notices SET lane_key=orch_pending_notice_lane(NEW) WHERE recipient_segment_id=NEW.id;
  END IF;
  RETURN NEW;
END $$;

CREATE FUNCTION public.orch_pending_notice_legacy_write() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE source jsonb; offset_at integer:=0;
BEGIN
  IF pg_trigger_depth()>1 THEN RETURN NEW; END IF;
  source:=NEW.pending_notices;
  IF jsonb_typeof(source)<>'array' THEN source:=jsonb_build_array(source); END IF;
  DELETE FROM thread_pending_notices WHERE thread_id=NEW.node_id;
  WHILE offset_at<jsonb_array_length(source) LOOP
    INSERT INTO thread_pending_notices(thread_id,notice)
      SELECT NEW.node_id,source->i FROM generate_series(offset_at,LEAST(offset_at+255,jsonb_array_length(source)-1)) i ORDER BY i;
    offset_at:=offset_at+256;
  END LOOP;
  NEW.pending_notices:=orch_pending_notice_preview(NEW.node_id);
  RETURN NEW;
END $$;

CREATE FUNCTION public.orch_pending_notice_preview(thread uuid) RETURNS jsonb
    LANGUAGE sql STABLE
    AS $$
  SELECT COALESCE(jsonb_agg(notice ORDER BY position),'[]'::jsonb) FROM
    (SELECT notice,position FROM thread_pending_notices WHERE thread_id=thread ORDER BY position LIMIT 64) head
$$;

CREATE FUNCTION public.orch_pending_notice_preview_added() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF pg_trigger_depth()>1 THEN RETURN NULL; END IF;
  UPDATE threads t SET pending_notices=orch_pending_notice_preview(t.node_id)
    FROM(SELECT DISTINCT thread_id FROM added_notices) affected
    WHERE t.node_id=affected.thread_id AND jsonb_array_length(t.pending_notices)<64;
  RETURN NULL;
END $$;

CREATE FUNCTION public.orch_pending_notice_preview_removed() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF pg_trigger_depth()>1 THEN RETURN NULL; END IF;
  UPDATE threads t SET pending_notices=orch_pending_notice_preview(t.node_id)
    FROM(SELECT DISTINCT thread_id FROM removed_notices) affected WHERE t.node_id=affected.thread_id;
  RETURN NULL;
END $$;

CREATE FUNCTION public.orch_pending_notice_recipient() RETURNS trigger
    LANGUAGE plpgsql
    AS $_$
BEGIN
  NEW.recipient_segment_id:=NULL; NEW.lane_key:=''::bytea;
  IF NEW.notice->>'id' LIKE 'agent-history:%' THEN
    IF NEW.notice->>'id' ~ '^agent-history:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$' THEN
      NEW.recipient_segment_id:=substring(NEW.notice->>'id' FROM 15)::uuid;
    END IF;
    SELECT orch_pending_notice_lane(s) INTO NEW.lane_key FROM segments s WHERE s.id=NEW.recipient_segment_id;
    NEW.lane_key:=COALESCE(NEW.lane_key,sha256(convert_to(NEW.notice->>'id','UTF8')));
  END IF;
  RETURN NEW;
END $_$;

CREATE FUNCTION public.orch_program_build_page(wanted text, shipped jsonb, trust_mirror boolean, expected text[], cutoff timestamp with time zone, wanted_os text, wanted_arch text, wanted_status text, after_order text, after_version text, after_os text, after_arch text, take bigint) RETURNS TABLE(version text, os text, arch text, version_sort text, payload jsonb)
    LANGUAGE sql STABLE
    AS $$
    WITH stored AS (
        (SELECT b.version,b.os,b.arch,b.version_sort,b.payload
         FROM orch_program_stored_build_rows(wanted,shipped,trust_mirror,expected,cutoff) b
         WHERE wanted_status IS NULL AND (wanted_os IS NULL OR b.os=wanted_os)
           AND (wanted_arch IS NULL OR b.arch=wanted_arch)
           AND (after_order IS NULL OR b.version_sort COLLATE "C"<=after_order COLLATE "C")
           AND (after_order IS NULL OR b.version_sort COLLATE "C"<after_order COLLATE "C"
             OR (b.version_sort=after_order AND b.version<after_version)
             OR (b.version_sort=after_order AND b.version=after_version AND (b.os,b.arch)>(after_os,after_arch)))
         ORDER BY b.version_sort COLLATE "C" DESC,b.version DESC,b.os,b.arch LIMIT take)
        UNION ALL
        SELECT b.version,b.os,b.arch,b.version_sort,b.payload
        FROM unnest(CASE WHEN wanted_status='untested' THEN ARRAY['untested','checked'] ELSE ARRAY[wanted_status] END) state
        CROSS JOIN LATERAL (
            SELECT b.version,b.os,b.arch,b.version_sort,b.payload
            FROM orch_program_stored_build_rows(wanted,shipped,trust_mirror,expected,cutoff) b
            WHERE wanted_status IS NOT NULL AND b.raw_status=state AND b.payload->>'status'=wanted_status
              AND (wanted_os IS NULL OR b.os=wanted_os) AND (wanted_arch IS NULL OR b.arch=wanted_arch)
              AND (after_order IS NULL OR b.version_sort COLLATE "C"<=after_order COLLATE "C")
              AND (after_order IS NULL OR b.version_sort COLLATE "C"<after_order COLLATE "C"
                OR (b.version_sort=after_order AND b.version<after_version)
                OR (b.version_sort=after_order AND b.version=after_version AND (b.os,b.arch)>(after_os,after_arch)))
            ORDER BY b.version_sort COLLATE "C" DESC,b.version DESC,b.os,b.arch LIMIT take
        ) b
    ), released AS (
        SELECT * FROM orch_program_release_build_rows(wanted,shipped,trust_mirror,cutoff)
    ), candidates AS (
        SELECT * FROM stored
        UNION ALL
        SELECT * FROM released r WHERE (wanted_os IS NULL OR r.os=wanted_os)
          AND (wanted_arch IS NULL OR r.arch=wanted_arch) AND (wanted_status IS NULL OR r.payload->>'status'=wanted_status)
          AND (after_order IS NULL OR r.version_sort COLLATE "C"<after_order COLLATE "C"
            OR (r.version_sort=after_order AND r.version<after_version)
            OR (r.version_sort=after_order AND r.version=after_version AND (r.os,r.arch)>(after_os,after_arch)))
    ) SELECT * FROM candidates ORDER BY version_sort COLLATE "C" DESC,version DESC,os,arch LIMIT take
$$;

CREATE FUNCTION public.orch_program_build_rows(wanted text, shipped jsonb, trust_mirror boolean, expected text[], cutoff timestamp with time zone) RETURNS TABLE(version text, os text, arch text, version_sort text, payload jsonb)
    LANGUAGE sql STABLE
    AS $$
    SELECT version,os,arch,version_sort,payload
    FROM orch_program_stored_build_rows(wanted,shipped,trust_mirror,expected,cutoff)
    UNION ALL
    SELECT * FROM orch_program_release_build_rows(wanted,shipped,trust_mirror,cutoff)
$$;

CREATE FUNCTION public.orch_program_has_native_proof(p jsonb, expected text[]) RETURNS boolean
    LANGUAGE sql IMMUTABLE
    AS $_$
    SELECT COALESCE(p->>'source'='release' OR expected IS NULL OR
        (p#>>'{check,stage}'='complete'
        AND jsonb_typeof(p#>'{check,cases}')='array'
        AND jsonb_array_length(CASE WHEN jsonb_typeof(p#>'{check,cases}')='array' THEN p#>'{check,cases}' ELSE '[]'::jsonb END)=cardinality(expected)
        AND NOT EXISTS (SELECT 1 FROM unnest(expected) e WHERE
            (SELECT count(*) FROM jsonb_array_elements(CASE WHEN jsonb_typeof(p#>'{check,cases}')='array' THEN p#>'{check,cases}' ELSE '[]'::jsonb END) c WHERE c->>'name'=e)<>1)
        AND NOT EXISTS (SELECT 1 FROM jsonb_array_elements(CASE WHEN jsonb_typeof(p#>'{check,cases}')='array' THEN p#>'{check,cases}' ELSE '[]'::jsonb END) c WHERE
            c->'passed' IS DISTINCT FROM 'true'::jsonb OR c->>'error' IS NOT NULL
            OR CASE WHEN jsonb_typeof(c->'elapsed_ms')='number' AND c->>'elapsed_ms' ~ '^[0-9]+$' THEN (c->>'elapsed_ms')::numeric>9223372036854775807 ELSE true END
            OR (c->>'unsupported' IS NOT NULL AND
                (c->>'name'<>'history.native_import' OR jsonb_typeof(c->'unsupported')<>'string' OR btrim(c->>'unsupported')='')))), false)
$_$;

CREATE FUNCTION public.orch_program_release_build_rows(wanted text, shipped jsonb, trust_mirror boolean, cutoff timestamp with time zone) RETURNS TABLE(version text, os text, arch text, version_sort text, payload jsonb)
    LANGUAGE sql STABLE
    AS $$
    SELECT r->>'version',r->>'os',r->>'arch',orch_program_version_order(r->>'version'),
        r || jsonb_build_object('is_default',COALESCE(r->>'is_default'='true',false) AND NOT EXISTS (
            SELECT 1 FROM agent_program_builds a WHERE a.kind=wanted AND a.os=r->>'os' AND a.arch=r->>'arch'
            AND a.updated_at<=cutoff AND a.payload->>'is_default'='true' AND a.payload->>'source' IN ('admin','mirror')
            AND (a.payload->>'checksum_origin' IN ('admin','vendor') OR (trust_mirror AND a.payload->>'checksum_origin'='mirror'))))
    FROM jsonb_array_elements(shipped) r WHERE r->>'kind'=wanted
    AND NOT EXISTS (SELECT 1 FROM agent_program_builds a WHERE a.kind=wanted
        AND a.version=r->>'version' AND a.os=r->>'os' AND a.arch=r->>'arch' AND a.priority>2 AND a.updated_at<=cutoff
        AND ((a.payload->>'source'='admin' AND a.payload->>'checksum_origin' IN ('admin','vendor')) OR
             (trust_mirror AND a.payload->>'source'='mirror' AND a.payload->>'checksum_origin'='mirror')))
$$;

CREATE FUNCTION public.orch_program_search_text(program_kind text, names jsonb) RETURNS text
    LANGUAGE sql IMMUTABLE
    AS $$
    SELECT lower(program_kind||' '||COALESCE(string_agg(value,' ' ORDER BY key),''))
    FROM jsonb_each_text(CASE WHEN jsonb_typeof(names)='object' THEN names ELSE '{}'::jsonb END);
$$;

CREATE FUNCTION public.orch_program_stored_build_rows(wanted text, shipped jsonb, trust_mirror boolean, expected text[], cutoff timestamp with time zone) RETURNS TABLE(version text, os text, arch text, version_sort text, raw_status text, payload jsonb)
    LANGUAGE sql STABLE
    AS $$
    SELECT b.version,b.os,b.arch,b.version_sort,b.payload->>'status',
        b.payload || jsonb_build_object('status',CASE WHEN b.payload->>'status'='checked'
            AND NOT orch_program_has_native_proof(b.payload,expected) THEN 'untested'
            ELSE COALESCE(b.payload->>'status','untested') END,
            'is_default',b.priority>1 AND COALESCE(b.payload->>'is_default'='true',false))
    FROM agent_program_builds b
    WHERE b.kind=wanted AND b.updated_at<=cutoff AND
        ((b.payload->>'source'='admin' AND b.payload->>'checksum_origin' IN ('admin','vendor')) OR
         (b.payload->>'source'='channel' AND b.payload->>'checksum_origin'='vendor') OR
         (trust_mirror AND b.payload->>'source'='mirror' AND b.payload->>'checksum_origin'='mirror'))
    AND NOT EXISTS (
        SELECT 1 FROM agent_program_builds a WHERE a.kind=b.kind AND a.version=b.version
        AND a.os=b.os AND a.arch=b.arch AND a.priority>b.priority AND a.updated_at<=cutoff
        AND ((a.payload->>'source'='admin' AND a.payload->>'checksum_origin' IN ('admin','vendor')) OR
             (trust_mirror AND a.payload->>'source'='mirror' AND a.payload->>'checksum_origin'='mirror')))
    AND (b.priority>1 OR NOT EXISTS (SELECT 1 FROM jsonb_array_elements(shipped) r
        WHERE r->>'kind'=wanted AND r->>'version'=b.version AND r->>'os'=b.os AND r->>'arch'=b.arch))
$$;

CREATE FUNCTION public.orch_program_version_order(version text) RETURNS text
    LANGUAGE plpgsql IMMUTABLE STRICT
    AS $_$
DECLARE m text[]; part text; result text;
BEGIN
    m := regexp_match(version, '^v?([0-9]{1,20})\.([0-9]{1,20})\.([0-9]{1,20})(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$');
    IF m IS NULL OR m[1] !~ '^(0|[1-9][0-9]*)$' OR m[2] !~ '^(0|[1-9][0-9]*)$'
       OR m[3] !~ '^(0|[1-9][0-9]*)$' THEN RETURN '0!' || version; END IF;
    IF m[1]::numeric>18446744073709551615 OR m[2]::numeric>18446744073709551615
       OR m[3]::numeric>18446744073709551615 THEN RETURN '0!' || version; END IF;
    IF m[4] IS NOT NULL AND EXISTS (
        SELECT 1 FROM unnest(string_to_array(m[4],'.')) mark
        WHERE mark='' OR (mark ~ '^[0-9]+$' AND mark !~ '^(0|[1-9][0-9]*)$')
    ) THEN RETURN '0!' || version; END IF;
    result := '1!' || lpad(m[1],20,'0') || '!' || lpad(m[2],20,'0') || '!' || lpad(m[3],20,'0') || '!';
    IF m[4] IS NULL THEN RETURN result || '2'; END IF;
    result := result || '1!';
    FOREACH part IN ARRAY string_to_array(m[4],'.') LOOP
        -- Numeric prerelease identifiers have arbitrary length. Compare their decimal length first.
        IF part ~ '^[0-9]+$' THEN result := result || '0' || lpad(length(part)::text,20,'0') || '!' || part || '!';
        ELSE result := result || '1' || part || '!'; END IF;
    END LOOP;
    RETURN result;
END;
$_$;

CREATE FUNCTION public.orch_project_program_build() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        IF OLD.node_id IS NULL THEN DELETE FROM agent_program_builds WHERE setting_key=OLD.key; END IF;
        RETURN OLD;
    END IF;
    IF TG_OP='UPDATE' AND OLD.node_id IS NULL THEN
        DELETE FROM agent_program_builds WHERE setting_key=OLD.key;
    END IF;
    IF NEW.node_id IS NULL AND (starts_with(NEW.key,'harness_build:') OR starts_with(NEW.key,'harness_channel:'))
       AND jsonb_typeof(NEW.value)='object'
       AND NEW.value ?& ARRAY['kind','version','os','arch']
       AND NEW.value->>'os' IN ('linux','macos','windows')
       AND NEW.value->>'arch' IN ('x86_64','aarch64')
       AND NEW.key=(CASE WHEN starts_with(NEW.key,'harness_build:') THEN 'harness_build:' ELSE 'harness_channel:' END)
            || (NEW.value->>'kind') || ':' || (NEW.value->>'version') || ':' || (NEW.value->>'os') || ':' || (NEW.value->>'arch') THEN
        INSERT INTO agent_program_builds
        VALUES (NEW.key, NEW.value->>'kind', NEW.value->>'version', NEW.value->>'os', NEW.value->>'arch',
                CASE WHEN starts_with(NEW.key,'harness_build:') THEN 3 ELSE 1 END,
                orch_program_version_order(NEW.value->>'version'), NEW.value, clock_timestamp())
        ON CONFLICT (setting_key) DO UPDATE SET
            kind=EXCLUDED.kind, version=EXCLUDED.version, os=EXCLUDED.os, arch=EXCLUDED.arch,
            priority=EXCLUDED.priority, version_sort=EXCLUDED.version_sort,
            payload=EXCLUDED.payload, updated_at=EXCLUDED.updated_at;
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.orch_project_program_reports() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    DELETE FROM agent_program_reports WHERE machine_id = NEW.id;
    IF NEW.deleted_at IS NULL AND NEW.status = 'active' THEN
        INSERT INTO agent_program_reports (machine_id, kind, manifest, builtin, reported_at)
        SELECT NEW.id, a->>'kind', a->'manifest', COALESCE(a->>'builtin','false')='true', NEW.created_at
        FROM jsonb_array_elements(CASE WHEN jsonb_typeof(NEW.facts->'adapters')='array'
                                     THEN NEW.facts->'adapters' ELSE '[]'::jsonb END) a
        WHERE COALESCE(a->>'kind','') <> ''
        ON CONFLICT (machine_id, kind) DO UPDATE SET manifest=EXCLUDED.manifest, builtin=EXCLUDED.builtin;
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.orch_public_name_claim(wanted text, person uuid, organization uuid) RETURNS void
    LANGUAGE plpgsql
    AS $$
DECLARE
  affected integer;
BEGIN
  IF (person IS NULL) = (organization IS NULL) THEN
    RAISE EXCEPTION 'a public name claim needs one object'
      USING ERRCODE='23514',CONSTRAINT='public_name_owner_check';
  END IF;
  INSERT INTO public_names AS held(name,user_id,org_id) VALUES(wanted,person,organization)
  ON CONFLICT(name) DO UPDATE SET
    user_id=COALESCE(held.user_id,EXCLUDED.user_id),
    org_id=COALESCE(held.org_id,EXCLUDED.org_id)
  WHERE (EXCLUDED.user_id IS NULL OR held.user_id IS NULL OR held.user_id=EXCLUDED.user_id)
    AND (EXCLUDED.org_id IS NULL OR held.org_id IS NULL OR held.org_id=EXCLUDED.org_id)
    AND (COALESCE(held.user_id,EXCLUDED.user_id) IS NULL
      OR COALESCE(held.org_id,EXCLUDED.org_id) IS NULL
      OR EXISTS(SELECT 1 FROM users u JOIN orgs o ON o.node_id=u.personal_org_id
          WHERE u.id=COALESCE(held.user_id,EXCLUDED.user_id)
            AND o.node_id=COALESCE(held.org_id,EXCLUDED.org_id)
            AND o.parent_id IS NULL AND o.org_kind='personal'
            AND u.username=wanted AND o.slug=wanted));
  GET DIAGNOSTICS affected=ROW_COUNT;
  IF affected=0 THEN
    RAISE EXCEPTION 'the public name is already allocated'
      USING ERRCODE='23505',CONSTRAINT=CASE WHEN person IS NOT NULL
        THEN 'users_username_key' ELSE 'orgs_slug_key' END;
  END IF;
END
$$;

CREATE FUNCTION public.orch_public_name_cleanup() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF NEW.user_id IS NULL AND NEW.org_id IS NULL THEN
    DELETE FROM public_names WHERE name=NEW.name;
  END IF;
  RETURN NEW;
END
$$;

CREATE FUNCTION public.orch_public_name_org() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF TG_OP='UPDATE' THEN
    IF NEW.slug=OLD.slug AND NEW.parent_id IS NOT DISTINCT FROM OLD.parent_id
      AND NEW.org_kind=OLD.org_kind THEN RETURN NEW; END IF;
    UPDATE public_names SET org_id=NULL WHERE org_id=OLD.node_id;
  END IF;
  IF NEW.parent_id IS NULL THEN
    PERFORM orch_public_name_claim(NEW.slug,NULL,NEW.node_id);
  END IF;
  RETURN NEW;
END
$$;

CREATE FUNCTION public.orch_public_name_user() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF TG_OP='UPDATE' THEN
    IF NEW.username=OLD.username THEN RETURN NEW; END IF;
    UPDATE public_names SET user_id=NULL WHERE user_id=OLD.id;
  END IF;
  PERFORM orch_public_name_claim(NEW.username,NEW.id,NULL);
  RETURN NEW;
END
$$;

CREATE FUNCTION public.orch_reference_source_moment() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  NEW.source_moment:=NULL;
  IF NEW.source_kind='document' THEN
    SELECT updated_at INTO NEW.source_moment FROM document_blocks
      WHERE id=NEW.part_id AND document_id=NEW.source_id AND removed_at IS NULL;
  ELSIF NEW.source_kind='comment' THEN
    SELECT created_at INTO NEW.source_moment FROM document_comments
      WHERE id=NEW.part_id AND document_id=NEW.source_id AND deleted_at IS NULL;
  END IF;
  RETURN NEW;
END
$$;

CREATE FUNCTION public.orch_related_name_key(value text) RETURNS bytea
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
    AS $$ SELECT sha256(convert_to(value,'UTF8')) $$;

CREATE FUNCTION public.orch_related_path_key(path text, encoded boolean) RETURNS bytea
    LANGUAGE plpgsql IMMUTABLE STRICT PARALLEL SAFE
    AS $_$
DECLARE value text:=path; bytes bytea; invalid boolean;
BEGIN
  IF encoded THEN
    SELECT bool_or(part[1]='%'),string_agg(CASE
      WHEN left(part[1],1)='%' AND length(part[1])=3 THEN decode(substr(part[1],2),'hex')
      ELSE convert_to(part[1],'UTF8') END,''::bytea ORDER BY ordinal)
      INTO invalid,bytes FROM regexp_matches(path,'(%[0-9A-Fa-f]{2}|[^%]+|%)','g')
        WITH ORDINALITY AS parts(part,ordinal);
    IF invalid THEN RETURN NULL; END IF;
    value:=convert_from(bytes,'UTF8');
  END IF;
  IF value='' OR value LIKE '/%' OR value ~ '(^|/)\.{1,2}(/|$)|//|/$' THEN RETURN NULL; END IF;
  RETURN sha256(convert_to(value,'UTF8'));
EXCEPTION WHEN character_not_in_repertoire THEN RETURN NULL;
END
$_$;

CREATE FUNCTION public.orch_related_reference_refresh() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE from_kind text; from_source_id uuid; body jsonb;
BEGIN
  from_kind:=CASE TG_TABLE_NAME WHEN 'document_blocks' THEN 'document'
               WHEN 'chat_messages' THEN 'chat' ELSE 'conversation' END;
  IF TG_OP='DELETE' THEN
    DELETE FROM related_references WHERE source_kind=from_kind AND part_id=OLD.id;
    RETURN OLD;
  END IF;
  DELETE FROM related_references WHERE source_kind=from_kind AND part_id=NEW.id;
  IF TG_TABLE_NAME='document_blocks' THEN
    IF NEW.removed_at IS NOT NULL THEN RETURN NEW; END IF;
    from_source_id:=NEW.document_id; body:=NEW.content;
  ELSIF TG_TABLE_NAME='chat_messages' THEN
    IF NEW.deleted_at IS NOT NULL THEN RETURN NEW; END IF;
    from_source_id:=NEW.chat_id; body:=NEW.content;
  ELSE
    from_source_id:=NEW.thread_id; body:=NEW.payload;
  END IF;
  INSERT INTO related_references
  SELECT from_kind,from_source_id,NEW.id,kind,id,path_key FROM orch_related_targets(body)
  ON CONFLICT DO NOTHING;
  RETURN NEW;
END
$$;

CREATE FUNCTION public.orch_related_reference_targets_added() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  INSERT INTO related_reference_targets
  SELECT source_kind,source_id,target_kind,target_id,target_path_key,count(*) FROM added
    GROUP BY source_kind,source_id,target_kind,target_id,target_path_key
    ORDER BY source_kind,source_id,target_kind,target_id,target_path_key
  ON CONFLICT (source_kind,source_id,target_kind,target_id,target_path_key)
    DO UPDATE SET links=related_reference_targets.links+EXCLUDED.links;
  RETURN NULL;
END
$$;

CREATE FUNCTION public.orch_related_reference_targets_removed() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  UPDATE related_reference_targets t SET links=t.links-r.links FROM (
    SELECT source_kind,source_id,target_kind,target_id,target_path_key,count(*) links FROM removed
    GROUP BY source_kind,source_id,target_kind,target_id,target_path_key
  ) r WHERE (t.source_kind,t.source_id,t.target_kind,t.target_id,t.target_path_key)=
            (r.source_kind,r.source_id,r.target_kind,r.target_id,r.target_path_key);
  DELETE FROM related_reference_targets t USING (
    SELECT DISTINCT source_kind,source_id,target_kind,target_id,target_path_key FROM removed
  ) r WHERE t.links=0 AND (t.source_kind,t.source_id,t.target_kind,t.target_id,t.target_path_key)=
                         (r.source_kind,r.source_id,r.target_kind,r.target_id,r.target_path_key);
  RETURN NULL;
END
$$;

CREATE FUNCTION public.orch_related_targets(body jsonb) RETURNS TABLE(kind text, id uuid, path_key bytea)
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
    AS $_$
  SELECT DISTINCT value->>'kind',(value->>'id')::uuid,''::bytea
  FROM jsonb_path_query(body,'strict $.**') value
  WHERE value->>'type'='ref' AND value->>'kind' IN ('document','node','chat','file')
    AND value->>'id' ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'
  UNION
  SELECT CASE matched[1] WHEN 'documents' THEN 'document' WHEN 'nodes' THEN 'node'
             WHEN 'chats' THEN 'chat' ELSE 'file' END,matched[2]::uuid,''::bytea
  FROM jsonb_path_query(body,'strict $.** ? (@.type() == "string")') value
  CROSS JOIN LATERAL regexp_matches(value #>> '{}',
    $pattern$(?:^|\]\(<?)/(documents|nodes|chats|files)/([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})(?:[#>)]|$)$pattern$,'g') matched
  UNION
  SELECT 'node',(value->>'thread_id')::uuid,''::bytea
  FROM jsonb_path_query(body,'strict $.**') value
  WHERE value->>'type'='item_ref'
    AND value->>'thread_id' ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'
  UNION
  SELECT 'environment_file',(value->>'id')::uuid,orch_related_path_key(value->>'path',false)
  FROM jsonb_path_query(body,'strict $.**') value
  WHERE value->>'type'='ref' AND value->>'kind'='environment_file'
    AND value->>'id' ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'
    AND orch_related_path_key(value->>'path',false) IS NOT NULL
  UNION
  SELECT 'environment_file',matched[1]::uuid,orch_related_path_key(matched[2],true)
  FROM jsonb_path_query(body,'strict $.** ? (@.type() == "string")') value
  CROSS JOIN LATERAL regexp_matches(value #>> '{}',
    $pattern$(?:^|\]\(<?)/environments/([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})/files/([^#>\s)]+)(?:[#>)]|$)$pattern$,'g') matched
  WHERE orch_related_path_key(matched[2],true) IS NOT NULL
$_$;

CREATE FUNCTION public.orch_related_tree_name(value text) RETURNS text
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
    AS $$
  WITH trimmed AS (SELECT btrim(replace(value,'/','-'),
    E' \t\n\r\013\014'||U&'\0085\00a0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200a\2028\2029\202f\205f\3000') value)
  SELECT CASE WHEN value IN ('','.','..') THEN '_'||value||'_' ELSE value END FROM trimmed
$$;

CREATE FUNCTION public.orch_remember_program_kind() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_TABLE_NAME='agents' THEN
        INSERT INTO agent_program_kinds(kind) VALUES (NEW.harness_kind) ON CONFLICT DO NOTHING;
    ELSE
        INSERT INTO agent_program_kinds(kind) VALUES (NEW.kind) ON CONFLICT DO NOTHING;
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.orch_search_chunk_vectors_setup() RETURNS text
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF to_regclass('search_vectors') IS NOT NULL THEN
    RETURN 'ready';
  END IF;
  IF NOT EXISTS (SELECT 1 FROM pg_available_extensions WHERE name = 'vector') THEN
    RETURN 'absent';
  END IF;
  -- Two servers that start together set it up once.
  PERFORM pg_advisory_xact_lock(7454014637211743281);
  IF to_regclass('search_vectors') IS NOT NULL THEN
    RETURN 'ready';
  END IF;
  BEGIN
    CREATE EXTENSION IF NOT EXISTS vector;
  EXCEPTION WHEN insufficient_privilege THEN
    RETURN 'not_permitted';
  END;
  -- One generation's vector of one piece, in half precision, of the generation's length. Each generation's
  -- HNSW index (by cosine) is made with the generation, partial on its id, since its length is its own.
  CREATE TABLE search_vectors (
    generation_id uuid NOT NULL REFERENCES search_generations (id) ON DELETE CASCADE,
    chunk_id      uuid NOT NULL REFERENCES search_chunks (id) ON DELETE CASCADE,
    text_hash     bytea NOT NULL,                   -- the text it was made of; a piece cut anew makes it stale
    embedding     halfvec NOT NULL,
    PRIMARY KEY (generation_id, chunk_id)
  );
  CREATE INDEX search_vectors_chunk_idx ON search_vectors (chunk_id);
  RETURN 'ready';
END
$$;

CREATE FUNCTION public.orch_search_file_types(value jsonb) RETURNS text[]
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
    AS $_$
  WITH types AS (
    SELECT lower(media #>> '{}') media FROM (
      SELECT jsonb_path_query(value,'$.** ? (@.type == "file" || @.type == "image" || @.type == "attachment").media_type') media
      UNION ALL SELECT jsonb_path_query(value,'$.**.attachments[*].media_type')
      UNION ALL SELECT jsonb_path_query(value,'$.**.images[*].media_type')
      UNION ALL SELECT jsonb_path_query(value,'$.tool ? (@.kind == "image").media_type')
    ) recorded WHERE jsonb_typeof(media)='string'
  ), tokens AS (
    SELECT media token FROM types
    UNION ALL SELECT split_part(media,'/',1)||'/*' FROM types WHERE position('/' in media)>0
    UNION ALL SELECT '*/*' WHERE jsonb_path_exists(value,'$.** ? (@.type == "file" || @.type == "image" || @.type == "attachment" || exists(@.attachments[0]) || exists(@.images[0]))')
      OR value->'tool'->>'kind'='image'
  )
  SELECT coalesce(array_agg(DISTINCT token),'{}'::text[]) FROM tokens WHERE token IS NOT NULL
$_$;

CREATE FUNCTION public.orch_search_files_added() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  UPDATE documents d SET search_file_count=d.search_file_count+counts.total FROM (
    SELECT document_id,count(*) total FROM added_files GROUP BY document_id
  ) counts WHERE counts.document_id=d.id;
  RETURN NULL;
END
$$;

CREATE FUNCTION public.orch_search_files_moved() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  UPDATE documents d SET search_file_count=d.search_file_count+counts.total FROM (
    SELECT document_id,sum(delta) total FROM (
      SELECT document_id,count(*) delta FROM added_files GROUP BY document_id
      UNION ALL SELECT document_id,-count(*) FROM removed_files GROUP BY document_id
    ) changes GROUP BY document_id HAVING sum(delta)<>0
  ) counts WHERE counts.document_id=d.id;
  RETURN NULL;
END
$$;

CREATE FUNCTION public.orch_search_files_removed() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  UPDATE documents d SET search_file_count=d.search_file_count-counts.total FROM (
    SELECT document_id,count(*) total FROM removed_files GROUP BY document_id
  ) counts WHERE counts.document_id=d.id;
  RETURN NULL;
END
$$;

CREATE FUNCTION public.orch_search_grams(value text) RETURNS tsvector
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
    AS $$
 WITH chars AS MATERIALIZED (
  SELECT c,lead(c) OVER(ORDER BY n) AS next
  FROM unnest(string_to_array(lower(value),NULL)) WITH ORDINALITY p(c,n)
 )
 SELECT array_to_tsvector(ARRAY(SELECT 'u'||c FROM chars UNION SELECT 'b'||c||next FROM chars WHERE next IS NOT NULL))
$$;

CREATE FUNCTION public.orch_search_item_vector_refresh() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF to_regclass('search_item_vectors') IS NULL THEN RETURN NEW; END IF;
  DELETE FROM search_item_vectors
    WHERE generation_id=NEW.generation_id AND kind=NEW.kind AND source_id=NEW.source_id;
  IF NEW.embedded_for=NEW.generation_id AND NEW.chunks>0 THEN
    INSERT INTO search_item_vectors (generation_id,kind,source_id,marker,embedding)
    SELECT NEW.generation_id,NEW.kind,NEW.source_id,NEW.marker,
           l2_normalize(avg(l2_normalize(v.embedding::vector)))::halfvec
    FROM search_chunks c JOIN search_vectors v ON v.chunk_id=c.id
      AND v.generation_id=c.generation_id AND v.text_hash=c.text_hash
    WHERE c.generation_id=NEW.generation_id AND c.kind=NEW.kind AND c.source_id=NEW.source_id
    HAVING count(*)=NEW.chunks;
  END IF;
  RETURN NEW;
END
$$;

CREATE FUNCTION public.orch_search_vectors_setup() RETURNS text
    LANGUAGE plpgsql
    AS $$
DECLARE result text; generation record;
BEGIN
  result := orch_search_chunk_vectors_setup();
  IF result <> 'ready' THEN RETURN result; END IF;
  PERFORM pg_advisory_xact_lock(7454014637211743281);
  IF to_regclass('search_item_vectors') IS NULL THEN
    CREATE TABLE search_item_vectors (
      generation_id uuid NOT NULL,
      kind text NOT NULL,
      source_id uuid NOT NULL,
      marker bigint NOT NULL,
      embedding halfvec NOT NULL,
      PRIMARY KEY (generation_id, kind, source_id),
      FOREIGN KEY (generation_id, kind, source_id)
        REFERENCES search_sources (generation_id, kind, source_id) ON DELETE CASCADE
    );
    FOR generation IN SELECT id,dimensions FROM search_generations WHERE dimensions IS NOT NULL LOOP
      EXECUTE format('CREATE INDEX %I ON search_item_vectors USING hnsw
        ((subvector(embedding,1,%s)::halfvec(%s)) halfvec_cosine_ops) WHERE generation_id=%L',
        'search_item_vectors_'||replace(generation.id::text,'-',''),
        generation.dimensions,generation.dimensions,generation.id);
    END LOOP;
  END IF;
  RETURN 'ready';
END
$$;

CREATE FUNCTION public.orch_setting_json(name text, fallback jsonb) RETURNS jsonb
    LANGUAGE sql STABLE
    AS $$
  SELECT COALESCE(NULLIF(current_setting(name, true), '')::jsonb, fallback)
$$;

CREATE FUNCTION public.orch_tree_mark(org uuid, nodes uuid[], subtrees uuid[], at timestamp with time zone) RETURNS boolean
    LANGUAGE plpgsql
    AS $$
DECLARE
  stamps jsonb := orch_setting_json('orch.tree_stamps', '{}');
  mine   jsonb := COALESCE(stamps -> org::text, '{"n":[],"s":[]}');
BEGIN
  mine := jsonb_build_object(
    'n', COALESCE(mine -> 'n', '[]') || to_jsonb(COALESCE(nodes, '{}'::uuid[])),
    's', COALESCE(mine -> 's', '[]') || to_jsonb(COALESCE(subtrees, '{}'::uuid[])),
    'at', GREATEST((mine ->> 'at')::timestamptz, at));
  PERFORM set_config('orch.tree_stamps', jsonb_set(stamps, ARRAY[org::text], mine)::text, true);
  PERFORM orch_commit_arm();
  RETURN true;
END
$$;

CREATE FUNCTION public.pin_private_attachment_upload() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP <> 'INSERT' THEN
        IF OLD.blob IS NOT NULL AND OLD.expires_at IS NOT NULL THEN
            UPDATE blobs SET pin_count = pin_count - 1 WHERE hash = OLD.blob;
        END IF;
    END IF;
    IF TG_OP <> 'DELETE' THEN
        IF NEW.blob IS NOT NULL AND NEW.expires_at IS NOT NULL THEN
            UPDATE blobs SET pin_count = pin_count + 1 WHERE hash = NEW.blob;
        END IF;
        RETURN NEW;
    END IF;
    RETURN OLD;
END;
$$;

CREATE FUNCTION public.port_opening_publisher() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE publisher_machine uuid;
BEGIN
  IF NEW.host_id IS NOT NULL THEN
    SELECT machine_id INTO publisher_machine FROM hosts WHERE id=NEW.host_id;
    IF NEW.publish_machine_id IS NULL THEN NEW.publish_machine_id=publisher_machine;
    ELSIF NEW.publish_machine_id IS DISTINCT FROM publisher_machine THEN
      RAISE EXCEPTION 'the publishing machine differs from its host facade' USING ERRCODE='23514',CONSTRAINT='port_openings_publisher';
    END IF;
  END IF;
  IF NEW.host_allocated AND NEW.publish_machine_id IS NULL THEN
    RAISE EXCEPTION 'a public allocation needs its physical publisher' USING ERRCODE='23514',CONSTRAINT='port_openings_publisher';
  END IF;
  RETURN NEW;
END;
$$;

CREATE FUNCTION public.private_thread_draft_pins() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    DELETE FROM thread_draft_blob_refs r WHERE r.user_id=NEW.user_id AND r.thread_id=NEW.thread_id
        AND NOT EXISTS (SELECT 1 FROM thread_payload_blobs(NEW.content) p WHERE p.blob=r.blob);
    INSERT INTO thread_draft_blob_refs(user_id, thread_id, blob)
        SELECT NEW.user_id, NEW.thread_id, b.hash
        FROM thread_payload_blobs(NEW.content) p JOIN blobs b ON b.hash=p.blob
        ON CONFLICT DO NOTHING;
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.provider_placement(runtime_machine uuid) RETURNS text
    LANGUAGE sql STABLE
    AS $$
    SELECT CASE WHEN runtime_machine IS NULL OR EXISTS (
        SELECT 1 FROM machines m WHERE m.id=runtime_machine AND m.runtime_id IS NOT NULL
    ) THEN 'managed' ELSE 'connected' END;
$$;

CREATE FUNCTION public.provider_status_time() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    -- Establish the default keeper once, rather than guessing it at every read. An explicitly
    -- selected other machine stays selected; a keeper in this provider's old unit follows its successor.
    IF NEW.machine_id IS NOT NULL THEN
        IF NEW.key_machine_id IS NULL THEN
            NEW.key_machine_id := NEW.machine_id;
        ELSIF TG_OP = 'UPDATE' AND NEW.key_machine_id = OLD.key_machine_id THEN
            IF EXISTS (
                SELECT 1 FROM machines m JOIN runtimes r ON r.id=m.runtime_id
                 WHERE m.id=OLD.key_machine_id AND r.provider_id=NEW.id
            ) THEN
                NEW.key_machine_id := NEW.machine_id;
            END IF;
        END IF;
    ELSIF TG_OP = 'UPDATE' AND NEW.key_machine_id = OLD.key_machine_id AND EXISTS (
        SELECT 1 FROM machines m JOIN runtimes r ON r.id=m.runtime_id
         WHERE m.id=OLD.key_machine_id AND r.provider_id=NEW.id
    ) THEN
        NEW.key_machine_id := NULL;
    END IF;
    IF TG_OP = 'INSERT' THEN
        NEW.status_at := NEW.updated_at;
    ELSIF NEW.status IS DISTINCT FROM OLD.status THEN
        NEW.status_at := NEW.updated_at;
    ELSE
        NEW.status_at := OLD.status_at;
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.provider_trial_ended() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    UPDATE providers SET credential_trial_input_id = NULL, credential_trial_turn_id = NULL
      WHERE credential_trial_turn_id = NEW.id;
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.provider_trial_recovered() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF NEW.status <> 'error' THEN
        NEW.credential_trial_input_id := NULL;
        NEW.credential_trial_turn_id := NULL;
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.skill_blob_pin() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
  added bytea;
  removed bytea;
BEGIN
  IF TG_TABLE_NAME = 'skill_versions' THEN
    added := CASE WHEN TG_OP = 'INSERT' THEN NEW.archive END;
    removed := CASE WHEN TG_OP = 'DELETE' THEN OLD.archive END;
  ELSE
    added := CASE WHEN TG_OP IN ('INSERT','UPDATE') THEN NEW.blob END;
    removed := CASE WHEN TG_OP IN ('DELETE','UPDATE') THEN OLD.blob END;
  END IF;
  IF added IS NOT NULL THEN
    UPDATE blobs SET pin_count = pin_count + 1 WHERE hash = added;
  END IF;
  IF removed IS NOT NULL THEN
    UPDATE blobs SET pin_count = pin_count - 1, last_access_at = GREATEST(last_access_at, now())
     WHERE hash = removed AND pin_count > 0;
    IF NOT FOUND THEN RAISE EXCEPTION 'a skill blob pin is missing' USING ERRCODE = '23514'; END IF;
  END IF;
  RETURN NULL;
END
$$;

CREATE FUNCTION public.skill_source_secrets_release() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  DELETE FROM secrets WHERE owner_kind = 'skill' AND owner_id = OLD.id;
  RETURN NULL;
END
$$;

CREATE FUNCTION public.thread_continuation_definition() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF TG_OP='UPDATE' THEN
    IF OLD.native_creation IS NOT NULL AND NEW.native_creation IS DISTINCT FROM OLD.native_creation
       OR OLD.native_creation_result IS NOT NULL AND NEW.native_creation_result IS DISTINCT FROM OLD.native_creation_result THEN
      RAISE EXCEPTION 'a retained native creation identity and result are immutable' USING ERRCODE='23514';
    END IF;
    IF (NEW.id,NEW.thread_id,NEW.parent_id,NEW.before_input_id,NEW.created_at,NEW.created_seq)
        IS DISTINCT FROM (OLD.id,OLD.thread_id,OLD.parent_id,OLD.before_input_id,OLD.created_at,OLD.created_seq) THEN
      RAISE EXCEPTION 'a retained continuation definition is immutable' USING ERRCODE='23514';
    END IF;
  ELSIF NEW.parent_id IS NULL THEN
    IF NEW.id<>NEW.thread_id OR NEW.before_input_id IS NOT NULL THEN
      RAISE EXCEPTION 'only the original thread continuation has no parent' USING ERRCODE='23514';
    END IF;
  ELSIF NEW.before_input_id IS NULL OR NOT EXISTS(SELECT 1 FROM thread_continuations p
      WHERE p.id=NEW.parent_id AND p.thread_id=NEW.thread_id) THEN
    RAISE EXCEPTION 'a new continuation needs an existing same-thread parent and actual input boundary' USING ERRCODE='23514';
  END IF;
  IF NEW.native_creation_result IS NOT NULL AND NEW.native_creation IS NULL THEN
    RAISE EXCEPTION 'a native creation result needs its original command identity' USING ERRCODE='23514';
  END IF;
  RETURN NEW;
END
$$;

CREATE FUNCTION public.thread_continuation_job_snapshot() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  UPDATE thread_continuation_operations SET job_snapshot=jsonb_build_object('id',NEW.id,'kind',NEW.kind,
    'state',NEW.state,'phase',COALESCE(NEW.progress->>'phase','preparing'),
    'percent',COALESCE((NEW.progress->>'percent')::int,0)) WHERE id=NEW.id;
  RETURN NEW;
END
$$;

CREATE FUNCTION public.thread_continuation_owner() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE actual uuid; input_owner uuid;
BEGIN
  IF TG_TABLE_NAME IN ('segments','thread_inputs') THEN
    SELECT active_continuation_id INTO actual FROM threads WHERE node_id=NEW.thread_id;
    IF NEW.continuation_id IS NOT NULL THEN actual:=NEW.continuation_id; END IF;
  ELSE
    SELECT continuation_id INTO actual FROM segments WHERE id=NEW.segment_id AND thread_id=NEW.thread_id;
    IF actual IS NULL OR (NEW.continuation_id IS NOT NULL AND NEW.continuation_id<>actual) THEN
      RAISE EXCEPTION 'history ownership must match its actual segment' USING ERRCODE='23514';
    END IF;
    IF TG_TABLE_NAME='turns' THEN
      SELECT continuation_id INTO input_owner FROM thread_inputs WHERE id=NEW.input_id AND thread_id=NEW.thread_id;
      IF input_owner IS DISTINCT FROM actual THEN
        RAISE EXCEPTION 'an input cannot migrate to another continuation' USING ERRCODE='23514';
      END IF;
    ELSIF NEW.turn_id IS NOT NULL THEN
      SELECT continuation_id INTO input_owner FROM turns WHERE id=NEW.turn_id AND thread_id=NEW.thread_id;
      IF input_owner IS DISTINCT FROM actual THEN
        RAISE EXCEPTION 'an item cannot migrate to another continuation' USING ERRCODE='23514';
      END IF;
    END IF;
  END IF;
  IF TG_OP='UPDATE' AND (OLD.continuation_id IS DISTINCT FROM actual OR OLD.thread_id<>NEW.thread_id) THEN
    RAISE EXCEPTION 'continuation ownership is immutable' USING ERRCODE='23514';
  END IF;
  NEW.continuation_id:=actual;
  RETURN NEW;
END
$$;

CREATE FUNCTION public.thread_continuation_queue() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  UPDATE thread_continuations SET queue_held=OLD.queue_held,queue_held_by=OLD.queue_held_by,
    queue_held_at=OLD.queue_held_at,queue_revision=OLD.queue_revision,current_segment_id=OLD.current_segment_id
    WHERE id=OLD.active_continuation_id;
  IF NEW.active_continuation_id<>OLD.active_continuation_id THEN
    SELECT queue_held,queue_held_by,queue_held_at,queue_revision INTO NEW.queue_held,NEW.queue_held_by,
      NEW.queue_held_at,NEW.queue_revision FROM thread_continuations WHERE id=NEW.active_continuation_id;
    -- The existing Resume revision is also a selection fence: another queue may have the same local number.
    NEW.queue_revision:=GREATEST(OLD.queue_revision,NEW.queue_revision)+1;
  END IF;
  UPDATE thread_continuations SET queue_held=NEW.queue_held,queue_held_by=NEW.queue_held_by,
    queue_held_at=NEW.queue_held_at,queue_revision=NEW.queue_revision,current_segment_id=NEW.current_segment_id
    WHERE id=NEW.active_continuation_id;
  RETURN NEW;
END
$$;

CREATE FUNCTION public.thread_continuation_root() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF NEW.active_continuation_id IS NOT NULL AND NEW.active_continuation_id<>NEW.node_id THEN
    RAISE EXCEPTION 'a new thread starts with its own root continuation' USING ERRCODE='23514';
  END IF;
  NEW.active_continuation_id := NEW.node_id;
  INSERT INTO thread_continuations(id,thread_id,status,created_by,created_at,created_seq,
    queue_held,queue_held_by,queue_held_at,queue_revision)
  SELECT NEW.node_id,NEW.node_id,'ready',n.owner_id,n.created_at,0,
    NEW.queue_held,NEW.queue_held_by,NEW.queue_held_at,NEW.queue_revision FROM nodes n WHERE n.id=NEW.node_id;
  RETURN NEW;
END
$$;

CREATE FUNCTION public.thread_history_blob_pin() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF TG_OP='INSERT' THEN
    UPDATE blobs SET pin_count=pin_count+1 WHERE hash=NEW.blob;
    RETURN NEW;
  END IF;
  UPDATE blobs SET pin_count=pin_count-1,last_access_at=GREATEST(last_access_at,now())
    WHERE hash=OLD.blob AND pin_count>0;
  IF NOT FOUND THEN RAISE EXCEPTION 'a retained history blob pin is missing' USING ERRCODE='23514'; END IF;
  RETURN OLD;
END
$$;

CREATE FUNCTION public.thread_history_cut_freeze() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  -- OLD is captured before current-body blob owners can be released. Pin/quote
  -- changes also freeze metadata, but never copy an unchanged item at admission.
  -- A cut holds an item by its spans; the first change after the cut was made
  -- keeps the item as it stood, in the cut, with a pin on the bytes it names.
  WITH holding AS (
    SELECT s.cut_id, s.seq_offset FROM thread_history_cut_spans s
     WHERE s.thread_id=OLD.thread_id AND s.owner_continuation_id=OLD.continuation_id
       AND OLD.seq_created BETWEEN s.first_seq AND s.last_seq AND OLD.seq_updated<=s.frozen_at
       AND ((OLD.turn_id IS NOT NULL AND EXISTS(SELECT 1 FROM thread_history_cut_rows t
               WHERE t.cut_id=s.cut_id AND t.kind='turn' AND t.row_id=OLD.turn_id))
         OR (OLD.turn_id IS NULL AND ((s.loose_before_orch AND OLD.before_orch)
               OR OLD.seq_created BETWEEN s.loose_low AND s.loose_high)))
  ), kept AS (
    INSERT INTO thread_history_cut_items AS c (cut_id,item_id,seq_created,turn_id,stream,kind,native_alias,body)
    SELECT h.cut_id,OLD.id,OLD.seq_created+h.seq_offset,OLD.turn_id,OLD.stream,OLD.kind,
      (SELECT a.native_ref FROM thread_native_item_aliases a WHERE a.item_id=OLD.id),to_jsonb(OLD)
      FROM holding h
    ON CONFLICT (cut_id,item_id) DO UPDATE SET body=EXCLUDED.body
      WHERE c.body IS NULL AND c.revision_seq IS NULL
    RETURNING c.cut_id
  )
  INSERT INTO thread_history_cut_blobs(cut_id,blob)
    SELECT k.cut_id,b.hash FROM kept k CROSS JOIN thread_payload_blobs(OLD.payload) p JOIN blobs b ON b.hash=p.blob
    ON CONFLICT DO NOTHING;
  IF TG_OP='DELETE' THEN RETURN OLD; END IF;
  RETURN NEW;
END
$$;

CREATE FUNCTION public.thread_history_cut_members(cut uuid) RETURNS TABLE(cut_id uuid, item_id uuid, seq_created bigint, turn_id uuid, stream text, kind text, parent_cut_id uuid, revision_seq bigint, native_alias text, body jsonb)
    LANGUAGE sql STABLE
    AS $_$
  -- What the cut holds, as rows of thread_history_cut_items read: the items its spans hold, at the revision they
  -- had when it was made, and the copies it keeps of those changed or removed since.
  SELECT s.cut_id,i.id,i.seq_created+s.seq_offset,i.turn_id,i.stream,i.kind,NULL::uuid,
    CASE WHEN i.seq_updated>s.frozen_at THEN (SELECT r.seq_updated FROM thread_item_revisions r
      WHERE r.item_id=i.id AND r.seq_updated<=s.frozen_at ORDER BY r.seq_updated DESC LIMIT 1) END,
    COALESCE(x.native_alias,a.native_ref),NULL::jsonb
    FROM thread_history_cut_spans s
    JOIN items i ON i.thread_id=s.thread_id AND i.continuation_id=s.owner_continuation_id
      AND i.seq_created BETWEEN s.first_seq AND s.last_seq
    LEFT JOIN thread_history_cut_items x ON x.cut_id=s.cut_id AND x.item_id=i.id
    LEFT JOIN thread_native_item_aliases a ON a.item_id=i.id
   WHERE s.cut_id=$1 AND x.body IS NULL
     AND ((i.turn_id IS NOT NULL AND EXISTS(SELECT 1 FROM thread_history_cut_rows t
             WHERE t.cut_id=s.cut_id AND t.kind='turn' AND t.row_id=i.turn_id))
       OR (i.turn_id IS NULL AND ((s.loose_before_orch AND i.before_orch)
             OR i.seq_created BETWEEN s.loose_low AND s.loose_high)))
  UNION ALL
  SELECT x.cut_id,x.item_id,x.seq_created,x.turn_id,x.stream,x.kind,x.parent_cut_id,x.revision_seq,x.native_alias,x.body
    FROM thread_history_cut_items x WHERE x.cut_id=$1 AND x.body IS NOT NULL
$_$;

CREATE FUNCTION public.thread_history_cut_identity() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF (NEW.id,NEW.thread_id,NEW.continuation_id,NEW.job_id,NEW.owner_thread_id,NEW.source_segment_id,
      NEW.source_native_ref,NEW.execution_revision,NEW.first_turn,NEW.last_turn,NEW.captured_seq,NEW.captured_at,NEW.head)
    IS DISTINCT FROM (OLD.id,OLD.thread_id,OLD.continuation_id,OLD.job_id,OLD.owner_thread_id,OLD.source_segment_id,
      OLD.source_native_ref,OLD.execution_revision,OLD.first_turn,OLD.last_turn,OLD.captured_seq,OLD.captured_at,OLD.head) THEN
    RAISE EXCEPTION 'a confirmed history cut cannot change its source or boundary' USING ERRCODE='23514';
  END IF;
  IF EXISTS(SELECT 1 FROM jsonb_each(OLD.artifacts) previous
      WHERE NEW.artifacts->previous.key IS DISTINCT FROM previous.value) THEN
    RAISE EXCEPTION 'a confirmed history artifact cannot be replaced' USING ERRCODE='23514';
  END IF;
  RETURN NEW;
END
$$;

CREATE FUNCTION public.thread_history_cut_parent() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF NEW.parent_cut_id IS NOT NULL AND NOT EXISTS(
      SELECT 1 FROM thread_history_cuts own JOIN threads side ON side.node_id=own.thread_id
        JOIN thread_history_cuts parent ON parent.id=NEW.parent_cut_id
        JOIN thread_history_cut_items item ON item.cut_id=parent.id AND item.item_id=NEW.item_id
      WHERE own.id=NEW.cut_id AND parent.owner_thread_id=side.node_id
        AND side.side_of=parent.thread_id AND item.parent_cut_id IS NULL
        AND NEW.body IS NULL AND NEW.revision_seq IS NULL) THEN
    RAISE EXCEPTION 'a retained side prefix has another source or depth' USING ERRCODE='23514';
  END IF;
  RETURN NEW;
END
$$;

CREATE FUNCTION public.thread_history_cut_release() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF EXISTS(SELECT 1 FROM thread_history_cut_commands WHERE cut_id=OLD.id AND NOT settled) THEN
    RAISE EXCEPTION 'unconfirmed native history inputs are still in use' USING ERRCODE='23514';
  END IF;
  RETURN OLD;
END
$$;

CREATE FUNCTION public.thread_history_native_receipt() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF NEW.state='succeeded' OR (NEW.state='failed' AND NEW.error->>'code'='unsupported'
      AND NEW.kind IN ('session.open','session.open.fenced','session.continuation.resume')) THEN
    UPDATE thread_history_cut_commands SET settled=true WHERE command_id=NEW.id;
  END IF;
  IF NEW.state='succeeded' AND NEW.kind='machine.probe' THEN
    UPDATE thread_history_cut_commands h SET settled=true FROM commands original
      WHERE h.fence_id=NEW.id AND original.id=h.command_id AND original.machine_id=NEW.machine_id
        AND original.serial_key=NEW.serial_key AND original.state IN ('failed','expired','cancelled');
  END IF;
  IF NEW.state='succeeded' AND NEW.kind='node.runtime.remove' THEN
    UPDATE thread_history_cut_commands SET settled=true WHERE runtime_id::text=NEW.body->>'runtime_id'
      AND retirement_machine_id=NEW.machine_id;
  END IF;
  IF NEW.kind='session.close.fenced' AND (NEW.state='succeeded'
      OR NEW.state='failed' AND NEW.error->>'code'='session.segment_changed') THEN
    UPDATE thread_history_cut_commands h SET settled=true FROM commands original
      WHERE (h.fence_id=NEW.id OR (h.fence_id IS NULL AND original.updated_at<=NEW.created_at
          AND EXISTS(SELECT 1 FROM thread_history_cuts cut WHERE cut.id=h.cut_id
            AND cut.owner_thread_id::text=original.body->>'thread_id')))
        AND original.id=h.command_id AND original.machine_id=NEW.machine_id
        AND original.serial_key=NEW.serial_key AND original.state IN ('succeeded','failed','expired','cancelled')
        AND original.body->>'thread_id'=NEW.body->>'thread_id'
        AND COALESCE(original.body#>>'{session,segment_id}',original.body->>'segment_id')=NEW.body->>'segment_id';
  END IF;
  RETURN NEW;
END
$$;

CREATE FUNCTION public.thread_item_current_pins() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  DELETE FROM thread_item_blob_refs r WHERE r.item_id=NEW.id
    AND NOT EXISTS(SELECT 1 FROM thread_payload_blobs(NEW.payload) p WHERE p.blob=r.blob);
  INSERT INTO thread_item_blob_refs(item_id,blob)
    SELECT NEW.id,b.hash FROM thread_payload_blobs(NEW.payload) p JOIN blobs b ON b.hash=p.blob
    ON CONFLICT DO NOTHING;
  RETURN NEW;
END
$$;

CREATE FUNCTION public.thread_item_freeze_revision() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE ordinal int;
BEGIN
  IF NEW.id<>OLD.id OR NEW.seq_created<>OLD.seq_created THEN
    RAISE EXCEPTION 'retained item identity and creation sequence are immutable' USING ERRCODE='23514';
  END IF;
  IF OLD.turn_id IS NOT NULL THEN SELECT t.ordinal INTO ordinal FROM turns t WHERE t.id=OLD.turn_id; END IF;
  IF (NEW.segment_id IS DISTINCT FROM OLD.segment_id OR NEW.turn_id IS DISTINCT FROM OLD.turn_id
    OR NEW.stream IS DISTINCT FROM OLD.stream OR NEW.parent_item_id IS DISTINCT FROM OLD.parent_item_id
    OR NEW.kind IS DISTINCT FROM OLD.kind OR NEW.before_orch IS DISTINCT FROM OLD.before_orch)
    AND EXISTS(SELECT 1 FROM thread_continuation_spans s WHERE s.owner_continuation_id=OLD.continuation_id
      AND OLD.seq_created BETWEEN s.first_seq AND s.last_seq
      AND (OLD.turn_id IS NULL OR ordinal BETWEEN s.first_turn AND s.last_turn)) THEN
    RAISE EXCEPTION 'a retained history item cannot change its structural identity' USING ERRCODE='23514';
  END IF;
  IF EXISTS(SELECT 1 FROM thread_continuation_spans s WHERE s.owner_continuation_id=OLD.continuation_id
    AND OLD.seq_created BETWEEN s.first_seq AND s.last_seq AND s.frozen_at>=OLD.seq_updated
    AND (OLD.turn_id IS NULL OR ordinal BETWEEN s.first_turn AND s.last_turn)) THEN
    IF NEW.seq_updated<OLD.seq_updated OR (NEW.seq_updated=OLD.seq_updated AND
      (NEW.payload IS DISTINCT FROM OLD.payload OR NEW.status IS DISTINCT FROM OLD.status
        OR NEW.model_input_id IS DISTINCT FROM OLD.model_input_id OR NEW.search_text IS DISTINCT FROM OLD.search_text)) THEN
      RAISE EXCEPTION 'a frozen item change needs a later sequence' USING ERRCODE='23514';
    END IF;
    IF NEW.seq_updated>OLD.seq_updated THEN
      INSERT INTO thread_item_revisions(item_id,seq_updated,body) VALUES(OLD.id,OLD.seq_updated,to_jsonb(OLD)) ON CONFLICT DO NOTHING;
    END IF;
  END IF;
  RETURN NEW;
END
$$;

CREATE FUNCTION public.thread_item_revision_pins() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  INSERT INTO thread_item_revision_blob_refs(item_id,seq_updated,blob)
    SELECT NEW.item_id,NEW.seq_updated,b.hash FROM thread_payload_blobs(NEW.body->'payload') p JOIN blobs b ON b.hash=p.blob
    ON CONFLICT DO NOTHING;
  RETURN NEW;
END
$$;

CREATE FUNCTION public.thread_native_alias_identity() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF NEW IS DISTINCT FROM OLD THEN
    RAISE EXCEPTION 'a copied native item identity cannot be replaced' USING ERRCODE='23514';
  END IF;
  RETURN NEW;
END
$$;

CREATE FUNCTION public.thread_payload_blobs(payload jsonb) RETURNS TABLE(blob bytea)
    LANGUAGE sql IMMUTABLE STRICT
    AS $_$
  SELECT DISTINCT decode(value #>> '{}','hex') FROM jsonb_path_query(payload,'lax $.**.blob') value
  WHERE jsonb_typeof(value)='string' AND (value #>> '{}') ~ '^[0-9a-f]{64}$'
$_$;

CREATE FUNCTION public.thread_read_continuation() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF NEW.continuation_id IS NULL THEN
    SELECT active_continuation_id INTO NEW.continuation_id FROM threads WHERE node_id=NEW.thread_id;
  END IF;
  RETURN NEW;
END
$$;

CREATE FUNCTION public.usage_count_add(state jsonb, label text) RETURNS jsonb
    LANGUAGE sql IMMUTABLE STRICT
    AS $$
  SELECT CASE WHEN label IS NULL THEN state ELSE
    state || jsonb_build_object(label, COALESCE((state->>label)::bigint,0)+1) END
$$;

CREATE FUNCTION public.usage_counts_merge(state jsonb, counts jsonb) RETURNS jsonb
    LANGUAGE sql IMMUTABLE
    AS $$
  SELECT COALESCE(jsonb_object_agg(key,n),'{}'::jsonb) FROM (
    SELECT key,sum(value::bigint) AS n FROM (
      SELECT * FROM jsonb_each_text(state) UNION ALL
      SELECT * FROM jsonb_each_text(COALESCE(counts,'{}'::jsonb))
    ) entries GROUP BY key
  ) merged
$$;

CREATE FUNCTION public.usage_hist_add(state jsonb, value double precision) RETURNS jsonb
    LANGUAGE sql IMMUTABLE STRICT
    AS $$
  SELECT usage_count_add(state, CASE WHEN value IS NULL OR value<0 THEN NULL
    WHEN value=0 THEN 'zero' WHEN value>1048576 THEN 'overflow'
    ELSE GREATEST(-80,ceil(log(2,value::numeric)*8)::int)::text END)
$$;

CREATE FUNCTION public.usage_mark_change() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE before_stamp record;
BEGIN
  IF TG_OP='UPDATE' THEN
    -- Identical images make cursor assignment cheap. Other representations
    -- retain the original semantic comparison, including equal numeric scales.
    before_stamp := OLD;
    before_stamp.change_seq := NEW.change_seq;
    IF pg_catalog.record_image_eq(NEW, before_stamp) THEN RETURN NEW; END IF;
    IF (to_jsonb(NEW)-'change_seq') IS NOT DISTINCT FROM (to_jsonb(OLD)-'change_seq') THEN RETURN NEW; END IF;
  END IF;
  NEW.change_seq := 0;
  INSERT INTO usage_commit_rows(xact,table_name,id)
    VALUES(pg_current_xact_id(),TG_TABLE_NAME,NEW.id);
  PERFORM orch_commit_arm();
  RETURN NEW;
END
$$;

CREATE AGGREGATE usage_counts(text) (
    SFUNC = usage_count_add,
    STYPE = jsonb,
    INITCOND = '{}'
);

CREATE AGGREGATE usage_histogram(double precision) (
    SFUNC = usage_hist_add,
    STYPE = jsonb,
    INITCOND = '{}'
);

CREATE AGGREGATE usage_merge_counts(jsonb) (
    SFUNC = usage_counts_merge,
    STYPE = jsonb,
    INITCOND = '{}'
);
