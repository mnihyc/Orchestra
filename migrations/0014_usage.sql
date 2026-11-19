-- The usage ledger, its rollups, caps and rate catalogue, and resource use.

CREATE TABLE rate_catalogue (
    id boolean DEFAULT true NOT NULL,
    doc jsonb NOT NULL,
    as_of date NOT NULL,
    origin text NOT NULL,
    source text,
    loaded_at timestamp with time zone DEFAULT now() NOT NULL,
    loaded_by uuid,
    CONSTRAINT rate_catalogue_id_check CHECK (id),
    CONSTRAINT rate_catalogue_origin_check CHECK ((origin = ANY (ARRAY['shipped'::text, 'uploaded'::text, 'fetched'::text]))),
    CONSTRAINT rate_catalogue_pkey PRIMARY KEY (id),
    CONSTRAINT rate_catalogue_loaded_by_fkey FOREIGN KEY (loaded_by) REFERENCES users(id)
);

CREATE TABLE resource_use_people (
    subject_kind text NOT NULL,
    subject_id uuid NOT NULL,
    version_id uuid,
    use_kind text NOT NULL,
    person_id uuid NOT NULL,
    last_at timestamp with time zone NOT NULL,
    CONSTRAINT resource_use_people_subject_kind_subject_id_version_id_use__key UNIQUE NULLS NOT DISTINCT (subject_kind, subject_id, version_id, use_kind, person_id)
);

CREATE TABLE resource_use_receipts (
    subject_kind text NOT NULL,
    subject_id uuid NOT NULL,
    version_id uuid,
    use_kind text NOT NULL,
    source text NOT NULL,
    turn_id uuid,
    at timestamp with time zone NOT NULL,
    person_id uuid,
    CONSTRAINT resource_use_receipts_check CHECK (((use_kind = ANY (ARRAY['mcp_call'::text, 'skill_invocation'::text])) = (turn_id IS NOT NULL))),
    CONSTRAINT resource_use_receipts_use_kind_check CHECK ((use_kind = ANY (ARRAY['mcp_call'::text, 'skill_invocation'::text, 'environment_made'::text, 'provider_made'::text]))),
    CONSTRAINT resource_use_receipts_subject_kind_subject_id_version_id_us_key UNIQUE NULLS NOT DISTINCT (subject_kind, subject_id, version_id, use_kind, source),
    CONSTRAINT resource_use_receipts_turn_id_fkey FOREIGN KEY (turn_id) REFERENCES turns(id) ON DELETE CASCADE
);

CREATE INDEX resource_use_receipts_turn_idx ON resource_use_receipts USING btree (turn_id) WHERE (turn_id IS NOT NULL);

CREATE TABLE resource_use_recording (
    singleton boolean DEFAULT true NOT NULL,
    since timestamp with time zone NOT NULL,
    CONSTRAINT resource_use_recording_singleton_check CHECK (singleton),
    CONSTRAINT resource_use_recording_pkey PRIMARY KEY (singleton)
);

INSERT INTO resource_use_recording (since) VALUES (transaction_timestamp());

CREATE TABLE resource_use_rollups (
    subject_kind text NOT NULL,
    subject_id uuid NOT NULL,
    version_id uuid,
    use_kind text NOT NULL,
    day date,
    uses bigint NOT NULL,
    unattributed bigint NOT NULL,
    last_at timestamp with time zone NOT NULL,
    CONSTRAINT resource_use_rollups_unattributed_check CHECK ((unattributed >= 0)),
    CONSTRAINT resource_use_rollups_uses_check CHECK ((uses >= 0)),
    CONSTRAINT resource_use_rollups_subject_kind_subject_id_version_id_use_key UNIQUE NULLS NOT DISTINCT (subject_kind, subject_id, version_id, use_kind, day)
);

CREATE TABLE usage_caps (
    id uuid NOT NULL,
    node_id uuid,
    provider_id uuid,
    subject_kind text DEFAULT 'everyone'::text NOT NULL,
    subject_id uuid,
    subject_role text,
    split text DEFAULT 'shared'::text NOT NULL,
    models text[] DEFAULT '{}'::text[] NOT NULL,
    tiers text[] DEFAULT '{}'::text[] NOT NULL,
    harness_kinds text[] DEFAULT '{}'::text[] NOT NULL,
    meter text NOT NULL,
    period text,
    counted_from date,
    amount numeric,
    at_limit text DEFAULT 'stop'::text NOT NULL,
    warn_percent smallint,
    uncounted text DEFAULT 'allow'::text NOT NULL,
    note text DEFAULT ''::text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    created_by uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT usage_caps_amount_check CHECK ((amount >= (0)::numeric)),
    CONSTRAINT usage_caps_at_limit_check CHECK ((at_limit = ANY (ARRAY['stop'::text, 'finish_turn'::text]))),
    CONSTRAINT usage_caps_check CHECK (((subject_kind <> ALL (ARRAY['user'::text, 'group'::text, 'members'::text])) OR (subject_id IS NOT NULL))),
    CONSTRAINT usage_caps_check1 CHECK (((subject_kind <> 'everyone'::text) OR (subject_id IS NULL))),
    CONSTRAINT usage_caps_check2 CHECK (((subject_kind = 'role'::text) = (subject_role IS NOT NULL))),
    CONSTRAINT usage_caps_check3 CHECK (((subject_kind <> 'user'::text) OR (split = 'shared'::text))),
    CONSTRAINT usage_caps_check4 CHECK (((subject_kind <> 'role'::text) OR ((split = 'each'::text) AND ((provider_id IS NULL) OR (subject_id IS NOT NULL))))),
    CONSTRAINT usage_caps_check5 CHECK (((meter = 'running'::text) = (period IS NULL))),
    CONSTRAINT usage_caps_check6 CHECK (((NOT (period IS DISTINCT FROM 'total'::text)) OR (counted_from IS NULL))),
    CONSTRAINT usage_caps_check7 CHECK (((meter <> 'running'::text) OR ((warn_percent IS NULL) AND ((amount IS NULL) OR (amount = trunc(amount)))))),
    CONSTRAINT usage_caps_meter_check CHECK ((meter = ANY (ARRAY['tokens'::text, 'credits'::text, 'running'::text]))),
    CONSTRAINT usage_caps_note_check CHECK ((char_length(note) <= 500)),
    CONSTRAINT usage_caps_period_check CHECK ((period = ANY (ARRAY['day'::text, 'week'::text, 'month'::text, 'total'::text]))),
    CONSTRAINT usage_caps_split_check CHECK ((split = ANY (ARRAY['shared'::text, 'each'::text]))),
    CONSTRAINT usage_caps_subject_kind_check CHECK ((subject_kind = ANY (ARRAY['everyone'::text, 'user'::text, 'group'::text, 'role'::text, 'members'::text]))),
    CONSTRAINT usage_caps_subject_role_check CHECK ((subject_role = ANY (ARRAY['viewer'::text, 'interactor'::text, 'developer'::text, 'admin'::text]))),
    CONSTRAINT usage_caps_uncounted_check CHECK ((uncounted = ANY (ARRAY['allow'::text, 'block'::text]))),
    CONSTRAINT usage_caps_warn_percent_check CHECK (((warn_percent >= 1) AND (warn_percent <= 99))),
    CONSTRAINT usage_caps_pkey PRIMARY KEY (id),
    CONSTRAINT usage_caps_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id),
    CONSTRAINT usage_caps_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE,
    CONSTRAINT usage_caps_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES providers(id) ON DELETE CASCADE
);

CREATE INDEX usage_caps_node_idx ON usage_caps USING btree (node_id) WHERE enabled;

CREATE INDEX usage_caps_provider_idx ON usage_caps USING btree (provider_id) WHERE enabled;

CREATE TABLE usage_change_state (
    singleton boolean NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    CONSTRAINT usage_change_state_singleton_check CHECK (singleton),
    CONSTRAINT usage_change_state_pkey PRIMARY KEY (singleton)
);

INSERT INTO usage_change_state (singleton) VALUES (true);

CREATE UNLOGGED TABLE usage_commit_rows (
    xact xid8 NOT NULL,
    ordinal bigint GENERATED ALWAYS AS IDENTITY (SEQUENCE NAME usage_commit_rows_ordinal_seq START WITH 1 INCREMENT BY 1 NO MINVALUE NO MAXVALUE CACHE 1),
    table_name text NOT NULL,
    id uuid NOT NULL,
    CONSTRAINT usage_commit_rows_table_name_check CHECK ((table_name = ANY (ARRAY['usage_events'::text, 'usage_running'::text]))),
    CONSTRAINT usage_commit_rows_pkey PRIMARY KEY (xact, ordinal)
);

CREATE TABLE usage_daily (
    day date NOT NULL,
    org_id uuid NOT NULL,
    project_id uuid NOT NULL,
    topic_id uuid NOT NULL,
    thread_id uuid,
    purpose text DEFAULT 'turn'::text NOT NULL,
    provider_id uuid NOT NULL,
    entry_id uuid,
    user_id uuid,
    model text NOT NULL,
    tier text,
    harness_kind text,
    environment_kind text,
    effort text,
    access_program text,
    requests bigint DEFAULT 0 NOT NULL,
    hidden_requests bigint DEFAULT 0 NOT NULL,
    short_requests bigint DEFAULT 0 NOT NULL,
    errors jsonb DEFAULT '{}'::jsonb NOT NULL,
    speed_tokens bigint DEFAULT 0 NOT NULL,
    generation_ms_sum bigint DEFAULT 0 NOT NULL,
    speed_buckets jsonb DEFAULT '{}'::jsonb NOT NULL,
    first_token_buckets jsonb DEFAULT '{}'::jsonb NOT NULL,
    turns integer DEFAULT 0 NOT NULL,
    input_tokens bigint DEFAULT 0 NOT NULL,
    cached_input_tokens bigint DEFAULT 0 NOT NULL,
    output_tokens bigint DEFAULT 0 NOT NULL,
    reasoning_tokens bigint DEFAULT 0 NOT NULL,
    total_tokens bigint DEFAULT 0 NOT NULL,
    credits numeric DEFAULT 0 NOT NULL,
    estimated_credits numeric DEFAULT 0 NOT NULL,
    unrated_tokens bigint DEFAULT 0 NOT NULL,
    cache_write_tokens bigint DEFAULT 0 NOT NULL,
    rated_rows bigint DEFAULT 0 NOT NULL,
    CONSTRAINT usage_daily_day_topic_id_thread_id_purpose_provider_id_entr_key UNIQUE NULLS NOT DISTINCT (day, topic_id, thread_id, purpose, provider_id, entry_id, model, tier, user_id, harness_kind, environment_kind, effort, access_program)
);

CREATE INDEX usage_daily_org_idx ON usage_daily USING btree (org_id, day);

CREATE INDEX usage_daily_project_idx ON usage_daily USING btree (project_id, day);

CREATE INDEX usage_daily_provider_idx ON usage_daily USING btree (provider_id, day);

CREATE INDEX usage_daily_thread_idx ON usage_daily USING btree (thread_id, day);

CREATE INDEX usage_daily_user_idx ON usage_daily USING btree (user_id, day);

COMMENT ON COLUMN usage_daily.rated_rows IS 'How many of the counted rows had a rate: where none had, the credits are unknown rather than zero.';

CREATE TABLE usage_events (
    id uuid NOT NULL,
    change_seq bigint NOT NULL,
    at timestamp with time zone NOT NULL,
    origin text NOT NULL,
    purpose text DEFAULT 'turn'::text NOT NULL,
    scope text NOT NULL,
    counted boolean NOT NULL,
    org_id uuid NOT NULL,
    project_id uuid NOT NULL,
    topic_id uuid NOT NULL,
    thread_id uuid,
    segment_id uuid,
    turn_id uuid,
    provider_id uuid NOT NULL,
    entry_id uuid,
    user_id uuid,
    agent_id uuid,
    harness_kind text,
    environment_id uuid,
    environment_kind text,
    arrangement text,
    model text NOT NULL,
    effort text,
    tier text,
    input_tokens bigint DEFAULT 0 NOT NULL,
    cached_input_tokens bigint DEFAULT 0 NOT NULL,
    cache_write_tokens bigint DEFAULT 0 NOT NULL,
    output_tokens bigint DEFAULT 0 NOT NULL,
    reasoning_tokens bigint DEFAULT 0 NOT NULL,
    total_tokens bigint DEFAULT 0 NOT NULL,
    context_tokens bigint,
    first_token_ms bigint,
    streamed_output_tokens bigint,
    speed_milli bigint,
    generation_ms bigint,
    request_ms bigint,
    outcome text,
    access_program text,
    measurement_states jsonb DEFAULT '{}'::jsonb NOT NULL,
    request_ref text,
    rate jsonb,
    rate_source text,
    credits numeric,
    CONSTRAINT usage_events_environment_kind_check CHECK ((environment_kind = ANY (ARRAY['managed'::text, 'connected'::text]))),
    CONSTRAINT usage_events_first_token_ms_check CHECK ((first_token_ms >= 0)),
    CONSTRAINT usage_events_generation_ms_check CHECK ((generation_ms >= 0)),
    CONSTRAINT usage_events_origin_check CHECK ((origin = ANY (ARRAY['harness'::text, 'forwarder'::text, 'orch'::text]))),
    CONSTRAINT usage_events_purpose_check CHECK ((purpose = ANY (ARRAY['turn'::text, 'summary'::text, 'review'::text, 'embeddings'::text]))),
    CONSTRAINT usage_events_rate_source_check CHECK ((rate_source = ANY (ARRAY['provider'::text, 'rate_card'::text, 'catalogue'::text]))),
    CONSTRAINT usage_events_request_ms_check CHECK ((request_ms >= 0)),
    CONSTRAINT usage_events_scope_check CHECK ((scope = ANY (ARRAY['turn'::text, 'request'::text]))),
    CONSTRAINT usage_events_speed_milli_check CHECK ((speed_milli >= 0)),
    CONSTRAINT usage_events_streamed_output_tokens_check CHECK ((streamed_output_tokens >= 0)),
    CONSTRAINT usage_events_pkey PRIMARY KEY (id)
);

CREATE INDEX usage_agent_facts_idx ON usage_events USING btree (agent_id, at) WHERE (turn_id IS NOT NULL);

CREATE INDEX usage_environment_facts_idx ON usage_events USING btree (environment_id, at) WHERE (turn_id IS NOT NULL);

CREATE INDEX usage_events_change_idx ON usage_events USING btree (change_seq);

CREATE INDEX usage_events_org_idx ON usage_events USING btree (org_id, at);

CREATE INDEX usage_events_project_idx ON usage_events USING btree (project_id, at);

CREATE INDEX usage_events_provider_idx ON usage_events USING btree (provider_id, at);

CREATE INDEX usage_events_thread_idx ON usage_events USING btree (thread_id, at);

CREATE INDEX usage_events_turn_idx ON usage_events USING btree (turn_id);

CREATE INDEX usage_events_user_idx ON usage_events USING btree (user_id, at);

CREATE TRIGGER usage_events_change BEFORE INSERT OR UPDATE ON usage_events FOR EACH ROW EXECUTE FUNCTION usage_mark_change();

CREATE TABLE usage_limit_bypasses (
    id uuid NOT NULL,
    cap_id uuid,
    provider_id uuid,
    thread_id uuid NOT NULL,
    turn_id uuid NOT NULL,
    author_id uuid,
    by_user uuid NOT NULL,
    meter text NOT NULL,
    used numeric,
    amount numeric,
    period_start date,
    at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT usage_limit_bypasses_meter_check CHECK ((meter = ANY (ARRAY['tokens'::text, 'credits'::text, 'running'::text, 'provider_limit'::text]))),
    CONSTRAINT usage_limit_bypasses_pkey PRIMARY KEY (id),
    CONSTRAINT usage_limit_bypasses_author_id_fkey FOREIGN KEY (author_id) REFERENCES users(id),
    CONSTRAINT usage_limit_bypasses_by_user_fkey FOREIGN KEY (by_user) REFERENCES users(id),
    CONSTRAINT usage_limit_bypasses_cap_id_fkey FOREIGN KEY (cap_id) REFERENCES usage_caps(id) ON DELETE SET NULL,
    CONSTRAINT usage_limit_bypasses_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES providers(id) ON DELETE SET NULL,
    CONSTRAINT usage_limit_bypasses_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE CASCADE,
    CONSTRAINT usage_limit_bypasses_turn_id_fkey FOREIGN KEY (turn_id) REFERENCES turns(id) ON DELETE CASCADE
);

CREATE INDEX usage_limit_bypasses_cap_idx ON usage_limit_bypasses USING btree (cap_id, at);

CREATE INDEX usage_limit_bypasses_provider_idx ON usage_limit_bypasses USING btree (provider_id, at);

CREATE TABLE usage_monthly (
    month date NOT NULL,
    org_id uuid NOT NULL,
    project_id uuid NOT NULL,
    topic_id uuid NOT NULL,
    user_id uuid,
    purpose text DEFAULT 'turn'::text NOT NULL,
    provider_id uuid NOT NULL,
    model text NOT NULL,
    tier text,
    harness_kind text,
    environment_kind text,
    turns integer DEFAULT 0 NOT NULL,
    input_tokens bigint DEFAULT 0 NOT NULL,
    cached_input_tokens bigint DEFAULT 0 NOT NULL,
    output_tokens bigint DEFAULT 0 NOT NULL,
    reasoning_tokens bigint DEFAULT 0 NOT NULL,
    total_tokens bigint DEFAULT 0 NOT NULL,
    credits numeric DEFAULT 0 NOT NULL,
    estimated_credits numeric DEFAULT 0 NOT NULL,
    unrated_tokens bigint DEFAULT 0 NOT NULL,
    cache_write_tokens bigint DEFAULT 0 NOT NULL,
    CONSTRAINT usage_monthly_month_topic_id_user_id_purpose_provider_id_mo_key UNIQUE NULLS NOT DISTINCT (month, topic_id, user_id, purpose, provider_id, model, tier, harness_kind, environment_kind)
);

CREATE INDEX usage_monthly_org_idx ON usage_monthly USING btree (org_id, month);

CREATE INDEX usage_monthly_project_idx ON usage_monthly USING btree (project_id, month);

CREATE INDEX usage_monthly_user_idx ON usage_monthly USING btree (user_id, month);

CREATE TABLE usage_running (
    id uuid NOT NULL,
    change_seq bigint NOT NULL,
    at timestamp with time zone NOT NULL,
    origin text NOT NULL,
    purpose text DEFAULT 'turn'::text NOT NULL,
    scope text NOT NULL,
    counted boolean NOT NULL,
    org_id uuid NOT NULL,
    project_id uuid NOT NULL,
    topic_id uuid NOT NULL,
    thread_id uuid,
    segment_id uuid,
    turn_id uuid,
    provider_id uuid NOT NULL,
    entry_id uuid,
    user_id uuid,
    agent_id uuid,
    harness_kind text,
    environment_id uuid,
    environment_kind text,
    arrangement text,
    model text NOT NULL,
    effort text,
    tier text,
    input_tokens bigint DEFAULT 0 NOT NULL,
    cached_input_tokens bigint DEFAULT 0 NOT NULL,
    cache_write_tokens bigint DEFAULT 0 NOT NULL,
    output_tokens bigint DEFAULT 0 NOT NULL,
    reasoning_tokens bigint DEFAULT 0 NOT NULL,
    total_tokens bigint DEFAULT 0 NOT NULL,
    context_tokens bigint,
    first_token_ms bigint,
    streamed_output_tokens bigint,
    speed_milli bigint,
    generation_ms bigint,
    request_ms bigint,
    outcome text,
    access_program text,
    measurement_states jsonb DEFAULT '{}'::jsonb NOT NULL,
    request_ref text,
    rate jsonb,
    rate_source text,
    credits numeric,
    CONSTRAINT usage_events_environment_kind_check CHECK ((environment_kind = ANY (ARRAY['managed'::text, 'connected'::text]))),
    CONSTRAINT usage_events_first_token_ms_check CHECK ((first_token_ms >= 0)),
    CONSTRAINT usage_events_generation_ms_check CHECK ((generation_ms >= 0)),
    CONSTRAINT usage_events_origin_check CHECK ((origin = ANY (ARRAY['harness'::text, 'forwarder'::text, 'orch'::text]))),
    CONSTRAINT usage_events_purpose_check CHECK ((purpose = ANY (ARRAY['turn'::text, 'summary'::text, 'review'::text, 'embeddings'::text]))),
    CONSTRAINT usage_events_rate_source_check CHECK ((rate_source = ANY (ARRAY['provider'::text, 'rate_card'::text, 'catalogue'::text]))),
    CONSTRAINT usage_events_request_ms_check CHECK ((request_ms >= 0)),
    CONSTRAINT usage_events_scope_check CHECK ((scope = ANY (ARRAY['turn'::text, 'request'::text]))),
    CONSTRAINT usage_events_speed_milli_check CHECK ((speed_milli >= 0)),
    CONSTRAINT usage_events_streamed_output_tokens_check CHECK ((streamed_output_tokens >= 0)),
    CONSTRAINT usage_running_pkey PRIMARY KEY (id)
);

CREATE INDEX usage_running_at_idx ON usage_running USING btree (at, id);

CREATE INDEX usage_running_change_idx ON usage_running USING btree (change_seq);

CREATE TRIGGER usage_running_change BEFORE INSERT OR UPDATE ON usage_running FOR EACH ROW EXECUTE FUNCTION usage_mark_change();

CREATE TABLE usage_cap_notices (
    cap_id uuid NOT NULL,
    user_id uuid,
    period_start date NOT NULL,
    level text NOT NULL,
    sent_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT usage_cap_notices_level_check CHECK ((level = ANY (ARRAY['warning'::text, 'reached'::text]))),
    CONSTRAINT usage_cap_notices_cap_id_user_id_period_start_level_key UNIQUE NULLS NOT DISTINCT (cap_id, user_id, period_start, level),
    CONSTRAINT usage_cap_notices_cap_id_fkey FOREIGN KEY (cap_id) REFERENCES usage_caps(id) ON DELETE CASCADE
);
