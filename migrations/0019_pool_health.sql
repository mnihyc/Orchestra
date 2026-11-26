CREATE TABLE pool_failure_scopes (
    id uuid PRIMARY KEY,
    identity jsonb NOT NULL,
    identity_hash text GENERATED ALWAYS AS (md5(identity::text)) STORED,
    failure_epoch bigint NOT NULL DEFAULT 0,
    succeeded_epoch bigint NOT NULL DEFAULT 0
);
CREATE INDEX pool_failure_scopes_identity_idx ON pool_failure_scopes(identity_hash);
CREATE TABLE pool_failures (
    scope_id uuid NOT NULL REFERENCES pool_failure_scopes(id),
    attempt_id uuid NOT NULL,
    epoch bigint NOT NULL,
    occurred_at timestamptz NOT NULL,
    reset_at timestamptz,
    classification jsonb NOT NULL,
    PRIMARY KEY(scope_id,attempt_id),
    UNIQUE(scope_id,epoch)
);
CREATE INDEX pool_failures_window_idx ON pool_failures(scope_id,occurred_at,epoch);
CREATE TABLE pool_failure_clears (
    pool_id uuid NOT NULL REFERENCES pools(id),
    scope_id uuid NOT NULL REFERENCES pool_failure_scopes(id),
    epoch bigint NOT NULL,
    cleared_at timestamptz NOT NULL,
    PRIMARY KEY(pool_id,scope_id)
);
CREATE TABLE pool_scope_observations (
    pool_id uuid NOT NULL REFERENCES pools(id),
    scope_id uuid NOT NULL REFERENCES pool_failure_scopes(id),
    PRIMARY KEY(pool_id,scope_id)
);
CREATE TABLE pool_recovery_trials (
    scope_id uuid NOT NULL REFERENCES pool_failure_scopes(id),
    failure_epoch bigint NOT NULL,
    attempt_id uuid NOT NULL,
    pool_id uuid NOT NULL REFERENCES pools(id),
    slot bigint NOT NULL,
    created_at timestamptz NOT NULL,
    settled_at timestamptz,
    PRIMARY KEY(scope_id,failure_epoch,attempt_id)
);
CREATE UNIQUE INDEX pool_recovery_trial_slot_idx ON pool_recovery_trials(scope_id,failure_epoch,slot) WHERE settled_at IS NULL;
CREATE TABLE pool_attempt_scopes (
    attempt_id uuid NOT NULL,
    scope_id uuid NOT NULL REFERENCES pool_failure_scopes(id),
    choice_id uuid NOT NULL REFERENCES pool_choices(id),
    failure_epoch bigint NOT NULL,
    credential boolean NOT NULL,
    settled_at timestamptz,
    PRIMARY KEY(attempt_id,scope_id)
);
CREATE INDEX pool_attempt_scopes_choice_idx ON pool_attempt_scopes(choice_id);
