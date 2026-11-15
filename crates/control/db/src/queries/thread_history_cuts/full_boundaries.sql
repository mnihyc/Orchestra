-- Extend one thread to the real long-thread size after the million-row plan world's earlier assertions.
-- These connection-local item keys match the maintained production history boundary index.
CREATE EXTENSION IF NOT EXISTS btree_gist;
INSERT INTO turns(id,thread_id,continuation_id,input_id,ordinal)
  SELECT md5('full-window-turn'||n)::uuid,md5('thread1')::uuid,md5('view1')::uuid,md5('input1')::uuid,n
  FROM generate_series(1001,200000)n;
INSERT INTO items(id,thread_id,continuation_id,turn_id,kind,payload,seq_created)
  SELECT md5('full-window-item'||n)::uuid,md5('thread1')::uuid,md5('view1')::uuid,
    md5('full-window-turn'||n)::uuid,'agent_message','{}'::jsonb,n
  FROM generate_series(1001,200000)n;
CREATE INDEX items_continuation_created_idx ON items(thread_id,continuation_id,seq_created);
CREATE TEMP TABLE thread_history_item_edges(item_id uuid PRIMARY KEY,thread_id uuid,continuation_id uuid,
  turn_id uuid,ordinal integer,seq_created bigint);
INSERT INTO thread_history_item_edges SELECT i.id,i.thread_id,i.continuation_id,i.turn_id,t.ordinal,i.seq_created
  FROM items i JOIN turns t ON t.id=i.turn_id WHERE i.thread_id=md5('thread1')::uuid
    AND i.stream='main' AND i.kind<>'reasoning';
CREATE INDEX history_item_boundaries_idx ON thread_history_item_edges
  USING gist(thread_id,continuation_id,ordinal,(seq_created::double precision));
ANALYZE thread_history_item_edges;
ANALYZE threads;
ANALYZE turns;
ANALYZE items;
