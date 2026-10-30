-- Whether an item `i` of the history owner `o` is a match: the same test for every read of conversation items.
i.kind=ANY($9::text[]) AND i.kind<>'marker' AND i.search_text IS NOT NULL AND __ITEM_MATCH__
     AND __ITEM_FILE_TYPE__
     AND NOT COALESCE(($5::jsonb->'search_advanced'->>'title_only')::boolean,false)
     AND i.seq_created BETWEEN o.first_seq AND o.last_seq AND (o.frozen_at IS NULL OR i.seq_updated<=o.frozen_at)
     AND (i.turn_id IS NULL OR EXISTS(SELECT 1 FROM turns t WHERE t.id=i.turn_id AND t.ordinal BETWEEN o.first_turn AND o.last_turn))
     AND ($32::uuid[] IS NULL OR i.id=ANY($32))
     AND i.search_text NOT LIKE '%<orch>%' -- old projections stay private until their startup repair commits
     AND ($12::uuid IS NULL OR i.author_id=$12)
     AND (($5::jsonb->'search_advanced'->>'date_field') IS NOT NULL OR $13::timestamptz IS NULL OR i.created_at >= $13) AND (($5::jsonb->'search_advanced'->>'date_field') IS NOT NULL OR $14::timestamptz IS NULL OR i.created_at <= $14)
     AND (NOT $21 OR orch_search_file_types(i.payload) @> ARRAY['*/*'])
     AND ($22::uuid IS NULL OR jsonb_path_exists(i.payload,'$.** ? (@.type == "mention" && @.user_id == $person)',jsonb_build_object('person',$22::text)))
