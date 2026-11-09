-- Each source family seeks its own native cursor before the bounded merge.
SELECT kind, id, place FROM (
    (SELECT 'thread'::text AS kind, t.node_id AS id, t.node_id AS place
       FROM threads t
      WHERE t.side_of IS NULL AND t.node_id >= $2
        AND (t.node_id, 'thread'::text) > ($2, $3::text)
        AND EXISTS (
            SELECT 1 FROM nodes n WHERE n.id = t.node_id AND $1 = ANY(n.path)
              AND NOT EXISTS (
                  SELECT 1 FROM nodes a WHERE a.id = ANY(n.path)
                    AND (a.deleted_at IS NOT NULL OR a.archived_at IS NOT NULL)))
      ORDER BY t.node_id LIMIT $4)
    UNION ALL
    (SELECT 'chat'::text, c.id, COALESCE(c.node_id, c.home_id)
       FROM chats c
      WHERE (c.kind = 'node' OR (c.kind = 'group' AND c.visibility = 'public'))
        AND c.last_message_seq > 0 AND c.id >= $2
        AND (c.id, 'chat'::text) > ($2, $3::text)
        AND EXISTS (
            SELECT 1 FROM nodes n
             WHERE n.id = COALESCE(c.node_id, c.home_id) AND $1 = ANY(n.path)
               AND NOT EXISTS (
                   SELECT 1 FROM nodes a WHERE a.id = ANY(n.path)
                     AND (a.deleted_at IS NOT NULL OR a.archived_at IS NOT NULL)))
      ORDER BY c.id LIMIT $4)
) sources
ORDER BY id, kind LIMIT $4
