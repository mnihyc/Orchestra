CREATE TABLE pool_choices (
    id uuid PRIMARY KEY,
    actor_id uuid NOT NULL,
    source jsonb NOT NULL,
    target jsonb NOT NULL,
    state text NOT NULL CHECK (state IN ('building','selected','exhausted')),
    leaf jsonb,
    path jsonb,
    policies jsonb NOT NULL DEFAULT '[]'::jsonb,
    reason text,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    details_expired_at timestamptz
);
CREATE INDEX pool_choices_details_idx ON pool_choices(id) WHERE state<>'building' AND details_expired_at IS NULL;
CREATE INDEX pool_choices_path_idx ON pool_choices USING gin(path jsonb_path_ops) WHERE state='selected';
CREATE INDEX jobs_pool_choice_active_idx ON jobs((plan->>'pool_choice')) WHERE state IN ('queued','running') AND plan->>'pool_choice' IS NOT NULL;
CREATE INDEX pool_choices_chain_idx ON pool_choices((source->>'chain')) WHERE state='selected';
CREATE TABLE pool_choice_nodes (
    choice_id uuid NOT NULL REFERENCES pool_choices(id) ON DELETE CASCADE,
    id uuid NOT NULL,
    pool_id uuid NOT NULL,
    parent_id uuid,
    member_id uuid,
    priority numeric NOT NULL,
    weight text NOT NULL,
    ordinal bigint NOT NULL,
    policy jsonb NOT NULL,
    revision bigint NOT NULL,
    definition jsonb NOT NULL,
    defaults jsonb NOT NULL,
    feasible boolean NOT NULL DEFAULT false,
    proposal bigint,
    cost numeric,
    loads jsonb NOT NULL DEFAULT '{}'::jsonb,
    PRIMARY KEY(choice_id,id)
);
CREATE INDEX pool_choice_nodes_parent_idx ON pool_choice_nodes(choice_id,parent_id,ordinal);
CREATE TABLE pool_choice_members (
    choice_id uuid NOT NULL REFERENCES pool_choices(id) ON DELETE CASCADE,
    ordinal bigint NOT NULL,
    parent_id uuid NOT NULL,
    member_id uuid NOT NULL,
    priority numeric NOT NULL,
    weight text NOT NULL,
    leaf jsonb NOT NULL,
    path jsonb NOT NULL,
    reason text,
    leaf_hash text GENERATED ALWAYS AS (md5(leaf::text)) STORED,
    cost numeric,
    load double precision,
    measurement jsonb NOT NULL DEFAULT '{}'::jsonb,
    loads jsonb NOT NULL DEFAULT '{}'::jsonb,
    PRIMARY KEY(choice_id,ordinal)
);
CREATE INDEX pool_choice_members_parent_idx ON pool_choice_members(choice_id,parent_id,priority,ordinal);
CREATE INDEX pool_choice_members_leaf_idx ON pool_choice_members(choice_id,leaf_hash) WHERE reason IS NULL;
CREATE TABLE pool_rotation (
    id uuid PRIMARY KEY,
    pool_id uuid NOT NULL,
    identity jsonb NOT NULL,
    identity_hash text GENERATED ALWAYS AS (md5(identity::text)) STORED,
    weight text NOT NULL,
    current_weight text NOT NULL,
    updated_at timestamptz NOT NULL
);
CREATE INDEX pool_rotation_member_idx ON pool_rotation(pool_id,identity_hash);
CREATE TABLE pool_choice_skips (
    choice_id uuid NOT NULL REFERENCES pool_choices(id) ON DELETE CASCADE,
    ordinal bigint NOT NULL,
    path jsonb NOT NULL,
    reason text NOT NULL,
    PRIMARY KEY(choice_id,ordinal)
);
