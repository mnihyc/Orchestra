-- Existing private drafts retain only available file references, with one pin per distinct blob.
WITH cleaned AS (
  SELECT s.user_id, s.chat_id,
    COALESCE(jsonb_agg(p.part ORDER BY p.ordinal) FILTER (
      WHERE p.part->>'type' NOT IN ('image', 'attachment')
        OR EXISTS (SELECT 1 FROM blobs b WHERE b.hash = decode(p.part->>'blob', 'hex'))
    ), '[]'::jsonb) AS parts
  FROM chat_states s
  CROSS JOIN LATERAL jsonb_array_elements(s.draft) WITH ORDINALITY p(part, ordinal)
  WHERE s.draft IS NOT NULL
  GROUP BY s.user_id, s.chat_id
)
UPDATE chat_states s SET draft = cleaned.parts
FROM cleaned WHERE s.user_id = cleaned.user_id AND s.chat_id = cleaned.chat_id;

WITH draft_blobs AS (
  SELECT DISTINCT s.user_id, s.chat_id, decode(p->>'blob', 'hex') AS hash
  FROM chat_states s CROSS JOIN LATERAL jsonb_array_elements(s.draft) p
  WHERE p->>'type' IN ('image', 'attachment')
), pins AS (
  SELECT hash, count(*)::integer AS n FROM draft_blobs GROUP BY hash
)
UPDATE blobs b SET pin_count = b.pin_count + pins.n
FROM pins WHERE b.hash = pins.hash;
