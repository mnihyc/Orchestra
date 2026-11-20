-- The permanent-cut selector runs against its real membership shape and captured turn metadata.
ALTER TABLE items ADD COLUMN seq_updated bigint NOT NULL DEFAULT 1;
CREATE TEMP TABLE thread_history_cuts(id uuid PRIMARY KEY,captured_seq bigint NOT NULL);
CREATE TEMP TABLE thread_history_cut_spans(cut_id uuid,thread_id uuid,owner_continuation_id uuid,
  first_seq bigint,last_seq bigint,frozen_at bigint,seq_offset bigint,loose_before_orch boolean,
  loose_low bigint,loose_high bigint,PRIMARY KEY(cut_id,thread_id,owner_continuation_id));
CREATE TEMP TABLE thread_history_cut_rows(cut_id uuid,kind text,row_id uuid,ordinal integer,body jsonb,
  item_high bigint,PRIMARY KEY(cut_id,kind,row_id));
CREATE TEMP TABLE thread_history_cut_items(cut_id uuid,item_id uuid,seq_created bigint,turn_id uuid,
  stream text,kind text,parent_cut_id uuid,revision_seq bigint,native_alias text,body jsonb,
  PRIMARY KEY(cut_id,item_id));
CREATE TEMP TABLE thread_native_item_aliases(item_id uuid PRIMARY KEY,native_ref text);
CREATE TEMP TABLE thread_item_revisions(item_id uuid,seq_updated bigint,body jsonb,PRIMARY KEY(item_id,seq_updated));
INSERT INTO thread_history_cuts VALUES(md5('full-edge-cut')::uuid,200001);
INSERT INTO thread_history_cut_spans VALUES(md5('full-edge-cut')::uuid,md5('thread1')::uuid,
  md5('view1')::uuid,0,200001,200001,0,true,0,200001);
INSERT INTO thread_history_cut_rows(cut_id,kind,row_id,ordinal,body,item_high)
  SELECT md5('full-edge-cut')::uuid,'turn',t.id,t.ordinal,'{}'::jsonb,max(i.seq_created)
  FROM turns t LEFT JOIN items i ON i.turn_id=t.id AND i.stream='main'
  WHERE t.thread_id=md5('thread1')::uuid GROUP BY t.id,t.ordinal;
CREATE INDEX history_cut_boundaries_idx ON thread_history_cut_rows
  USING gist(cut_id,ordinal,item_high) WHERE kind='turn' AND item_high IS NOT NULL;
ANALYZE thread_history_cuts;
ANALYZE thread_history_cut_rows;
ANALYZE thread_history_cut_spans;
ANALYZE thread_history_cut_items;
ANALYZE thread_native_item_aliases;
ANALYZE thread_item_revisions;
