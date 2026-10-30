(__SELECTION__->>'file_type' IS NULL OR
  orch_search_file_types(__PAYLOAD__) @> ARRAY[lower(__SELECTION__->>'file_type')])
