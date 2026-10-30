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
