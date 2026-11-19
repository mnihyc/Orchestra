-- Stored metric samples by day and the hours they fold into.

CREATE TABLE metric_hours (
    at timestamp with time zone NOT NULL,
    name text NOT NULL,
    labels jsonb DEFAULT '{}'::jsonb NOT NULL,
    mean double precision NOT NULL,
    peak double precision NOT NULL,
    total double precision NOT NULL,
    samples smallint NOT NULL,
    CONSTRAINT metric_hours_pkey PRIMARY KEY (name, labels, at)
)
PARTITION BY RANGE (at);

CREATE TABLE metric_samples (
    at timestamp with time zone NOT NULL,
    name text NOT NULL,
    labels jsonb DEFAULT '{}'::jsonb NOT NULL,
    value double precision NOT NULL,
    CONSTRAINT metric_samples_pkey PRIMARY KEY (name, labels, at)
)
PARTITION BY RANGE (at);

CREATE INDEX metric_samples_at_idx ON ONLY metric_samples USING btree (at);

-- Exact vendor observations use the existing metrics retention job, without float rollups.
ALTER TABLE providers ADD COLUMN credit_source_revision bigint NOT NULL DEFAULT 0
    CHECK (credit_source_revision >= 0);

CREATE TABLE provider_credit_observations (
    provider_id uuid NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
    source_revision bigint NOT NULL CHECK (source_revision >= 0),
    member text NOT NULL CHECK (member IN ('balances', 'spend', 'extra_usage')),
    item_id text NOT NULL CHECK (char_length(item_id) <= 200),
    quantity text NOT NULL CHECK (quantity IN ('remaining', 'total', 'used', 'limit')),
    sample_at timestamptz NOT NULL,
    observed_at timestamptz NOT NULL,
    label text CHECK (char_length(label) <= 200),
    unit text CHECK (char_length(unit) <= 200),
    decimal_places bigint CHECK (decimal_places BETWEEN 0 AND 4294967295),
    amount text CHECK (length(amount) <= 40 AND amount ~ '^-?[0-9]+([.][0-9]+)?$'),
    unlimited boolean,
    enabled boolean,
    cleared boolean NOT NULL DEFAULT false,
    had_gap boolean NOT NULL DEFAULT false,
    PRIMARY KEY (provider_id, source_revision, member, item_id, quantity, sample_at)
);
CREATE INDEX provider_credit_observations_at_idx ON provider_credit_observations(sample_at);
CREATE INDEX provider_credit_observations_read_idx ON provider_credit_observations(provider_id, observed_at);

-- Bounded operational history for authenticated keys. Business audits remain separate.
CREATE TABLE api_token_activity (
    id uuid PRIMARY KEY,
    sequence bigint GENERATED ALWAYS AS IDENTITY UNIQUE,
    token_id uuid NOT NULL REFERENCES api_tokens(id) ON DELETE CASCADE,
    at timestamp with time zone NOT NULL,
    operation text NOT NULL,
    selection text,
    facets jsonb NOT NULL CHECK (jsonb_typeof(facets) = 'array'),
    org_ids uuid[] NOT NULL DEFAULT '{}',
    method text NOT NULL,
    route text NOT NULL,
    status integer NOT NULL CHECK (status >= 0 AND status <= 65535),
    error_code text,
    replayed boolean NOT NULL DEFAULT false,
    duration_ms bigint NOT NULL CHECK (duration_ms >= 0),
    request_id text NOT NULL,
    ip text
);
CREATE INDEX api_token_activity_recent_idx ON api_token_activity(token_id, at DESC, id DESC);
CREATE INDEX api_token_activity_age_idx ON api_token_activity(at, id);
