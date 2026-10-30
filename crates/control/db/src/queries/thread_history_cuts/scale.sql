-- Connection-local replicas contain only columns used by the production queries.
CREATE TEMP TABLE threads(node_id uuid PRIMARY KEY,active_continuation_id uuid NOT NULL,last_activity_at timestamptz);
CREATE TEMP TABLE thread_continuations(id uuid PRIMARY KEY,thread_id uuid NOT NULL);
CREATE TEMP TABLE thread_continuation_spans(continuation_id uuid,owner_continuation_id uuid,first_turn integer,last_turn integer,first_seq bigint,last_seq bigint,frozen_at bigint);
CREATE INDEX ON thread_continuation_spans(continuation_id);
CREATE TEMP TABLE thread_inputs(id uuid PRIMARY KEY,thread_id uuid NOT NULL,state text NOT NULL);
CREATE TEMP TABLE turns(id uuid PRIMARY KEY,thread_id uuid NOT NULL,continuation_id uuid NOT NULL,input_id uuid NOT NULL,ordinal integer NOT NULL);
CREATE INDEX turns_continuation_ordinal_idx ON turns(thread_id,continuation_id,ordinal);
CREATE TEMP TABLE items(id uuid PRIMARY KEY,thread_id uuid NOT NULL,continuation_id uuid NOT NULL,turn_id uuid,kind text NOT NULL,payload jsonb NOT NULL,seq_created bigint NOT NULL,before_orch boolean NOT NULL DEFAULT false,stream text NOT NULL DEFAULT 'main');
CREATE INDEX items_prompt_input_idx ON items(thread_id,(payload->>'input_id')) WHERE kind='user_message';
INSERT INTO threads SELECT md5('thread'||n)::uuid,md5('view'||n)::uuid,'2026-01-01'::timestamptz FROM generate_series(1,1000)n;
INSERT INTO thread_continuations SELECT active_continuation_id,node_id FROM threads;
INSERT INTO thread_inputs SELECT md5('input'||n)::uuid,md5('thread'||((n-1)/1000+1))::uuid,'delivered' FROM generate_series(1,1000000)n;
INSERT INTO turns SELECT md5('turn'||n)::uuid,md5('thread'||((n-1)/1000+1))::uuid,md5('view'||((n-1)/1000+1))::uuid,md5('input'||n)::uuid,(n-1)%1000+1 FROM generate_series(1,1000000)n;
INSERT INTO items(id,thread_id,continuation_id,turn_id,kind,payload,seq_created) SELECT md5('item'||n)::uuid,md5('thread'||((n-1)/1000+1))::uuid,md5('view'||((n-1)/1000+1))::uuid,md5('turn'||n)::uuid,'agent_message','{}'::jsonb,(n-1)%1000+1 FROM generate_series(1,1000000)n;
INSERT INTO thread_inputs VALUES(md5('untouched')::uuid,md5('thread1')::uuid,'pending');
INSERT INTO thread_continuations VALUES(md5('selected1')::uuid,md5('thread1')::uuid);
INSERT INTO thread_continuation_spans VALUES(md5('selected1')::uuid,md5('view1')::uuid,1,500,0,500,500);
