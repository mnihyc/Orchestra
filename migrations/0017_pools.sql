CREATE TABLE pools (
    id uuid PRIMARY KEY,
    org_id uuid NOT NULL REFERENCES nodes(id),
    home_id uuid NOT NULL REFERENCES nodes(id),
    owner_id uuid NOT NULL REFERENCES users(id),
    name text NOT NULL CHECK (length(btrim(name)) > 0),
    description text NOT NULL DEFAULT '',
    kind text NOT NULL CHECK (kind IN ('model','provider','environment','host')),
    definition jsonb NOT NULL CHECK (jsonb_typeof(definition)='object' AND definition->>'kind'=kind
        AND jsonb_typeof(definition->'members')='array' AND jsonb_typeof(definition->'config')='object'
        AND jsonb_typeof(definition->'enabled')='boolean'),
    revision bigint NOT NULL DEFAULT 1,
    home_overridden_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz
);
CREATE INDEX pools_home_idx ON pools(home_id,id) WHERE deleted_at IS NULL;
CREATE INDEX pools_root_kind_idx ON pools(org_id,kind,id) WHERE deleted_at IS NULL;
CREATE INDEX pools_owner_idx ON pools(owner_id,id) WHERE deleted_at IS NULL;

CREATE TABLE pool_edges (
    pool_id uuid NOT NULL REFERENCES pools(id) ON DELETE CASCADE,
    member_id uuid NOT NULL,
    child_id uuid NOT NULL REFERENCES pools(id),
    PRIMARY KEY(pool_id,member_id),
    CHECK (pool_id<>child_id)
);
CREATE INDEX pool_edges_child_idx ON pool_edges(child_id,pool_id);

CREATE TABLE pool_lending (
    id uuid PRIMARY KEY,
    resource_kind text NOT NULL CHECK (resource_kind IN ('provider','environment','host','pool','template')),
    resource_id uuid NOT NULL,
    is_defaults boolean NOT NULL DEFAULT false,
    giver_id uuid NOT NULL REFERENCES users(id),
    matcher jsonb NOT NULL CHECK (jsonb_typeof(matcher)='object'),
    options jsonb NOT NULL CHECK (jsonb_typeof(options)='object'),
    show_names boolean NOT NULL DEFAULT false,
    revision bigint NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    removed_at timestamptz,
    expires_at timestamptz,
    renewed_at timestamptz
);
CREATE INDEX pool_lending_member_idx ON pool_lending(resource_kind,resource_id,id) WHERE removed_at IS NULL;
CREATE INDEX pool_lending_giver_idx ON pool_lending(giver_id,id) WHERE removed_at IS NULL;

CREATE TRIGGER participation_pools_delete AFTER DELETE ON pools REFERENCING OLD TABLE AS participation_old_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();
CREATE TRIGGER participation_pools_insert AFTER INSERT ON pools REFERENCING NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();
CREATE TRIGGER participation_pools_update AFTER UPDATE ON pools REFERENCING OLD TABLE AS participation_old_objects NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();
CREATE TRIGGER resource_tags_pools_deleted AFTER DELETE ON pools FOR EACH ROW EXECUTE FUNCTION orch_resource_tags_deleted('pool');

CREATE TABLE pool_members (
    pool_id uuid NOT NULL REFERENCES pools(id) ON DELETE CASCADE,
    id uuid NOT NULL,
    position bigint NOT NULL,
    rule jsonb NOT NULL CHECK (jsonb_typeof(rule)='object'),
    enabled boolean NOT NULL,
    priority bigint CHECK (priority>=0),
    weight text,
    PRIMARY KEY(pool_id,id)
);
CREATE INDEX pool_members_order_idx ON pool_members(pool_id,position,id);

-- Hash narrows the lookup; full JSON equality still decides identity, including on a collision.
CREATE INDEX pool_members_exact_idx ON pool_members(pool_id,md5((rule->'resource')::text),position,id)
    WHERE enabled AND rule->>'type'='single';
CREATE INDEX pool_members_dynamic_idx ON pool_members(pool_id,position,id)
    WHERE enabled AND rule->>'type' NOT IN ('single','pool');
