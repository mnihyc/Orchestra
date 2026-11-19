-- One selected turn in a long source with a different input and segment for every turn.
CREATE TEMP TABLE segments(id uuid PRIMARY KEY,thread_id uuid NOT NULL,ordinal integer NOT NULL,agent_id uuid);
CREATE INDEX segments_capture_cursor_idx ON segments(thread_id,id);
CREATE TEMP TABLE thread_inputs(id uuid PRIMARY KEY,thread_id uuid NOT NULL,turn_id uuid,state text NOT NULL);
CREATE INDEX inputs_turn_history_idx ON thread_inputs(thread_id,turn_id,id) WHERE turn_id IS NOT NULL;
CREATE INDEX inputs_capture_cursor_idx ON thread_inputs(thread_id,id);
CREATE TEMP TABLE thread_history_cut_rows(cut_id uuid,kind text,row_id uuid,ordinal integer DEFAULT 0,
  body jsonb NOT NULL,PRIMARY KEY(cut_id,kind,row_id));
INSERT INTO segments SELECT md5('metadata-segment'||n)::uuid,md5('metadata-thread')::uuid,n,NULL
  FROM generate_series(1,200000)n;
INSERT INTO thread_inputs SELECT md5('metadata-input'||n)::uuid,md5('metadata-thread')::uuid,
  md5('metadata-turn'||n)::uuid,'delivered' FROM generate_series(1,200000)n;
INSERT INTO thread_history_cut_rows(cut_id,kind,row_id,ordinal,body)
  VALUES(md5('metadata-cut')::uuid,'turn',md5('metadata-turn199999')::uuid,199999,
    jsonb_build_object('id',md5('metadata-turn199999')::uuid,'thread_id',md5('metadata-thread')::uuid,
      'segment_id',md5('metadata-segment199999')::uuid,'input_id',md5('metadata-input199999')::uuid));
ANALYZE segments;
ANALYZE thread_inputs;
ANALYZE thread_history_cut_rows;
