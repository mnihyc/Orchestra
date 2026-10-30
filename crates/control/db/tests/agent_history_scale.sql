-- One million physical identities and turns across one thousand conversations, seeded in bulk.
CREATE TEMP TABLE segments (
    id uuid PRIMARY KEY, thread_id uuid NOT NULL, provider_id uuid NOT NULL,
    agent_id uuid NOT NULL, machine_id uuid, native_session_ref text,
    started_at timestamptz NOT NULL, import_report jsonb NOT NULL,
    agent_history_bytes bigint, agent_history_measured_at timestamptz,
    agent_history_removed_at timestamptz, agent_history_removed_by uuid,
    agent_history_owned boolean NOT NULL, agent_history_claim uuid,
    agent_history_claimed_at timestamptz, agent_history_error text,
    agent_history_measure_revision bigint NOT NULL,
    agent_history_measured_revision bigint NOT NULL
);
CREATE TEMP TABLE turns (
    id uuid PRIMARY KEY, thread_id uuid NOT NULL, created_at timestamptz NOT NULL,
    started_at timestamptz, completed_at timestamptz
);
CREATE TEMP TABLE agents (id uuid PRIMARY KEY, harness_kind text NOT NULL);
CREATE TEMP TABLE threads (node_id uuid PRIMARY KEY, current_turn_id uuid, state text NOT NULL);
CREATE TEMP TABLE thread_inputs (thread_id uuid NOT NULL, state text NOT NULL);
CREATE TEMP TABLE approvals (thread_id uuid NOT NULL, state text NOT NULL);
INSERT INTO agents SELECT md5('agent:'||i::text)::uuid,'scale-program' FROM generate_series(0,:threads-1) i;
INSERT INTO threads SELECT md5('thread:'||i::text)::uuid,NULL,'idle' FROM generate_series(0,:threads-1) i;
INSERT INTO segments
SELECT lpad(to_hex(i),32,'0')::uuid,md5('thread:'||(i % :threads)::text)::uuid,
    md5('provider:'||(i % :threads)::text)::uuid,md5('agent:'||(i % :threads)::text)::uuid,
    md5('machine:'||(i % :threads)::text)::uuid,md5('native:'||i::text),
    '2026-01-01T00:00Z'::timestamptz,'{}'::jsonb,4096,now(),NULL,NULL,true,NULL,NULL,NULL,1,1
FROM generate_series(1,:rows) i;
INSERT INTO turns
SELECT lpad(to_hex(i),32,'0')::uuid,md5('thread:'||(i % :threads)::text)::uuid,
    '2026-01-01T00:00Z'::timestamptz + i*interval '1 second',
    NULL,'2026-01-01T00:00Z'::timestamptz + i*interval '1 second' + interval '1 minute'
FROM generate_series(1,:rows) i;
