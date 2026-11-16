-- An earlier turn may have a later-created item. Ordinal order alone is not a sequence boundary.
INSERT INTO items(id,thread_id,continuation_id,turn_id,kind,payload,seq_created)
  VALUES(md5('late-first-turn-item')::uuid,md5('thread1')::uuid,md5('view1')::uuid,
    md5('turn1')::uuid,'agent_message','{}'::jsonb,200001);
INSERT INTO thread_history_item_edges(item_id,thread_id,continuation_id,turn_id,ordinal,seq_created)
  VALUES(md5('late-first-turn-item')::uuid,md5('thread1')::uuid,md5('view1')::uuid,md5('turn1')::uuid,1,200001);
