-- Seek the native content cursor before checking each candidate's ancestor chain.
-- A removed ancestor, a foreign document parent or a rootless cycle contributes no text.
SELECT b.id, b.seq_updated, b.text, b.updated_at AS at, b.seq_updated AS ordinal
  FROM document_blocks b
 WHERE b.document_id = $1 AND b.removed_at IS NULL AND btrim(b.text) <> ''
   AND (b.seq_updated, b.id) <= ($2, $3)
   AND ($5::boolean OR (b.seq_updated, b.id) <> ($2, $3))
   AND (b.parent_id IS NULL OR (
       WITH RECURSIVE ancestors AS (
           SELECT p.id, p.parent_id, p.removed_at FROM document_blocks p
            WHERE p.id = b.parent_id AND p.document_id = $1
           UNION
           SELECT p.id, p.parent_id, p.removed_at FROM document_blocks p
             JOIN ancestors a ON p.id = a.parent_id
            WHERE p.document_id = $1
       )
       SELECT COALESCE(bool_and(removed_at IS NULL) AND bool_or(parent_id IS NULL), false)
         FROM ancestors
   ))
 ORDER BY b.seq_updated DESC, b.id DESC LIMIT $4
