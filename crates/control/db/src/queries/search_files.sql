WITH requested AS MATERIALIZED (
  SELECT document_id,array_agg(name) names
  FROM unnest($1::uuid[],$2::text[]) h(document_id,name) GROUP BY document_id
), named AS MATERIALIZED (
  SELECT f.document_id,f.id,f.name,f.media_type,f.size,f.is_text,f.added_by,f.added_by_thread,f.added_at
  FROM requested h CROSS JOIN LATERAL (
    SELECT f.document_id,f.id,f.name,f.media_type,f.size,f.is_text,f.added_by,f.added_by_thread,f.added_at
    FROM unnest(h.names) wanted(name) CROSS JOIN LATERAL (
      SELECT f.document_id,f.id,f.name,f.media_type,f.size,f.is_text,f.added_by,f.added_by_thread,f.added_at
      FROM document_files f WHERE f.document_id=h.document_id AND f.name=wanted.name LIMIT 1
    ) f
    WHERE __FILE_TYPE__ ORDER BY f.name LIMIT 1
  ) f
)
SELECT chosen.* FROM requested h CROSS JOIN LATERAL (
  SELECT n.* FROM named n WHERE n.document_id=h.document_id
  UNION ALL
  SELECT fallback.* FROM (
    SELECT f.document_id,f.id,f.name,f.media_type,f.size,f.is_text,f.added_by,f.added_by_thread,f.added_at
    FROM document_files f WHERE f.document_id=h.document_id AND $3
      AND NOT EXISTS(SELECT 1 FROM named n WHERE n.document_id=h.document_id)
      AND __FILE_TYPE__ ORDER BY f.name LIMIT 1
  ) fallback
) chosen
