-- Walk one native edge at a time instead of materializing and sorting the whole tree.
-- Ascending rows carry no text and never descend into the same children again.
WITH RECURSIVE walk AS (
    (SELECT b.id, b.parent_id, b."position", true AS entered,
            CASE WHEN btrim(b.text) <> '' THEN 1::bigint ELSE 0::bigint END AS seen, 1::bigint AS ordinal
       FROM document_blocks b
      WHERE b.document_id = $1 AND b.parent_id IS NULL AND b.removed_at IS NULL
        AND $2::bigint > 0
      ORDER BY b."position", b.id LIMIT 1)
    UNION ALL
    SELECT b.id, b.parent_id, b."position", next.entered,
           w.seen + CASE WHEN next.entered AND btrim(b.text) <> '' THEN 1 ELSE 0 END,
           w.ordinal + 1
      FROM walk w
      LEFT JOIN LATERAL (
          SELECT c.id FROM document_blocks c
           WHERE w.entered AND w.seen < $2 AND c.document_id = $1
             AND c.parent_id = w.id AND c.removed_at IS NULL
           ORDER BY c."position", c.id LIMIT 1
      ) child ON true
      LEFT JOIN LATERAL (
          -- Separate root and child entrances retain indexable parent equality.
          (SELECT s.id FROM document_blocks s
            WHERE child.id IS NULL AND w.seen < $2 AND w.parent_id IS NULL
              AND s.document_id = $1 AND s.parent_id IS NULL AND s.removed_at IS NULL
              AND (s."position", s.id) > (w."position", w.id)
            ORDER BY s."position", s.id LIMIT 1)
          UNION ALL
          (SELECT s.id FROM document_blocks s
            WHERE child.id IS NULL AND w.seen < $2 AND s.document_id = $1
              AND s.parent_id = w.parent_id AND s.removed_at IS NULL
              AND (s."position", s.id) > (w."position", w.id)
            ORDER BY s."position", s.id LIMIT 1)
      ) sibling ON true
      CROSS JOIN LATERAL (
          SELECT COALESCE(child.id, sibling.id, w.parent_id) AS id,
                 child.id IS NOT NULL OR sibling.id IS NOT NULL AS entered
      ) next
      CROSS JOIN LATERAL (
          SELECT b.id, b.parent_id, b."position", b.text FROM document_blocks b
           WHERE b.id = next.id AND b.document_id = $1 AND b.removed_at IS NULL
           LIMIT 1
      ) b
     WHERE w.seen < $2
)
SELECT b.id, b.seq_updated, b.text, b.updated_at AS at
  FROM walk w CROSS JOIN LATERAL (
      SELECT b.id, b.seq_updated, b.text, b.updated_at FROM document_blocks b
       WHERE w.entered AND b.id = w.id AND btrim(b.text) <> '' LIMIT 1
  ) b
 ORDER BY w.ordinal
