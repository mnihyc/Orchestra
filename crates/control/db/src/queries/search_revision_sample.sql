(SELECT * FROM items WHERE kind<>'marker' ORDER BY created_at DESC,id DESC LIMIT $17) i
CROSS JOIN LATERAL (
  SELECT * FROM thread_item_revisions r WHERE r.item_id=i.id
  ORDER BY r.seq_updated DESC LIMIT $17 OFFSET 0
) r
