(__SELECTION__->>'owner_id' IS NULL OR __OWNER__=(__SELECTION__->>'owner_id')::uuid)
AND (__SELECTION__->>'created_by_id' IS NULL OR __CREATOR__=(__SELECTION__->>'created_by_id')::uuid)
AND (__SELECTION__->>'edited_by_id' IS NULL OR __EDITOR__=(__SELECTION__->>'edited_by_id')::uuid)
AND (__SELECTION__->>'date_field' IS NULL OR (
  (__SELECTION__->>'from' IS NULL OR
    CASE __SELECTION__->>'date_field' WHEN 'created' THEN __CREATED__ WHEN 'edited' THEN __EDITED__ END >= (__SELECTION__->>'from')::timestamptz)
  AND (__SELECTION__->>'to' IS NULL OR
    CASE __SELECTION__->>'date_field' WHEN 'created' THEN __CREATED__ WHEN 'edited' THEN __EDITED__ END <= (__SELECTION__->>'to')::timestamptz)
))
