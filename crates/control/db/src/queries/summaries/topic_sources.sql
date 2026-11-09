-- Each family seeks its activity cursor and limits before the small final merge.
-- Split the thread fallback so an expression spanning nodes and threads needs no full sort.
SELECT kind, id, place, at FROM (
    (SELECT 'thread'::text AS kind, t.node_id AS id, t.node_id AS place, t.last_activity_at AS at
       FROM threads t
      WHERE t.side_of IS NULL AND t.last_activity_at IS NOT NULL
        AND (t.last_activity_at, t.node_id) <=
            (COALESCE($2::timestamptz, 'infinity'::timestamptz),
             COALESCE($3::uuid, 'ffffffff-ffff-ffff-ffff-ffffffffffff'::uuid))
        AND ($2::timestamptz IS NULL OR (t.last_activity_at, t.node_id, 'thread'::text) < ($2, $3, $4::text))
        AND EXISTS (
            SELECT 1 FROM nodes n WHERE n.id = t.node_id AND $1 = ANY(n.path)
              AND n.deleted_at IS NULL AND n.archived_at IS NULL
              AND NOT EXISTS (
                  SELECT 1 FROM nodes a WHERE a.id = ANY(n.path)
                    AND (a.deleted_at IS NOT NULL OR a.archived_at IS NOT NULL)))
      ORDER BY t.last_activity_at DESC, t.node_id DESC LIMIT $5)
    UNION ALL
    (SELECT 'thread'::text, n.id, n.id, n.updated_at
       FROM nodes n JOIN threads t ON t.node_id = n.id
      WHERE n.kind = 'thread' AND n.deleted_at IS NULL AND n.archived_at IS NULL
        AND t.side_of IS NULL AND t.last_activity_at IS NULL AND $1 = ANY(n.path)
        AND (n.updated_at, n.id) <=
            (COALESCE($2::timestamptz, 'infinity'::timestamptz),
             COALESCE($3::uuid, 'ffffffff-ffff-ffff-ffff-ffffffffffff'::uuid))
        AND ($2::timestamptz IS NULL OR (n.updated_at, n.id, 'thread'::text) < ($2, $3, $4::text))
        AND NOT EXISTS (
            SELECT 1 FROM nodes a WHERE a.id = ANY(n.path)
              AND (a.deleted_at IS NOT NULL OR a.archived_at IS NOT NULL))
      ORDER BY n.updated_at DESC, n.id DESC LIMIT $5)
    UNION ALL
    (SELECT 'chat'::text, c.id, COALESCE(c.node_id, c.home_id),
            COALESCE(c.last_message_at, c.created_at)
       FROM chats c
      WHERE (c.kind = 'node' OR (c.kind = 'group' AND c.visibility = 'public'))
        AND c.last_message_seq > 0
        AND (COALESCE(c.last_message_at, c.created_at), c.id) <=
            (COALESCE($2::timestamptz, 'infinity'::timestamptz),
             COALESCE($3::uuid, 'ffffffff-ffff-ffff-ffff-ffffffffffff'::uuid))
        AND ($2::timestamptz IS NULL OR
             (COALESCE(c.last_message_at, c.created_at), c.id, 'chat'::text) < ($2, $3, $4::text))
        AND EXISTS (
            SELECT 1 FROM nodes n
             WHERE n.id = COALESCE(c.node_id, c.home_id) AND $1 = ANY(n.path)
               AND n.deleted_at IS NULL AND n.archived_at IS NULL
               AND NOT EXISTS (
                   SELECT 1 FROM nodes a WHERE a.id = ANY(n.path)
                     AND (a.deleted_at IS NOT NULL OR a.archived_at IS NOT NULL)))
      ORDER BY COALESCE(c.last_message_at, c.created_at) DESC, c.id DESC LIMIT $5)
) sources
ORDER BY at DESC, id DESC, kind DESC LIMIT $5
