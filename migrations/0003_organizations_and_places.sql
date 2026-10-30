-- Organizations, the tree of places (projects, topics, threads), roles, groups, tags.

CREATE TABLE favorites (
    user_id uuid NOT NULL,
    kind text NOT NULL,
    id uuid NOT NULL,
    "position" numeric NOT NULL,
    view jsonb,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT favorites_kind_check CHECK ((kind = ANY (ARRAY['org'::text, 'project'::text, 'topic'::text, 'thread'::text, 'document'::text, 'document_container'::text, 'chat'::text, 'tag'::text, 'user'::text, 'group'::text, 'provider'::text, 'agent'::text, 'environment'::text, 'host'::text, 'template'::text, 'mcp_server'::text, 'mount'::text, 'machine'::text, 'skill'::text]))),
    CONSTRAINT favorites_pkey PRIMARY KEY (user_id, kind, id),
    CONSTRAINT favorites_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX favorites_position_idx ON favorites USING btree (user_id, "position", kind, id);

CREATE TABLE nodes (
    id uuid NOT NULL,
    kind text NOT NULL,
    parent_id uuid,
    org_id uuid NOT NULL,
    path uuid[] NOT NULL,
    name text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    title_source text DEFAULT 'user'::text NOT NULL,
    description_source text DEFAULT 'auto'::text NOT NULL,
    tags_source text DEFAULT 'auto'::text NOT NULL,
    auto_updated_at timestamp with time zone,
    color smallint,
    custom_color text,
    readme_document_id uuid,
    owner_id uuid NOT NULL,
    archived_at timestamp with time zone,
    archived_by uuid,
    deleted_at timestamp with time zone,
    deleted_by uuid,
    deletion_id uuid,
    archived_before_delete timestamp with time zone,
    archived_by_before_delete uuid,
    tree_seq bigint DEFAULT 0 NOT NULL,
    activity_at timestamp with time zone,
    revision bigint DEFAULT 1 NOT NULL,
    created_by uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT nodes_check CHECK (((kind = 'org'::text) OR (parent_id IS NOT NULL))),
    CONSTRAINT nodes_check1 CHECK (((parent_id IS NULL) = (org_id = id))),
    CONSTRAINT nodes_check2 CHECK ((path[array_upper(path, 1)] = id)),
    CONSTRAINT nodes_check3 CHECK (((color IS NULL) OR (custom_color IS NULL))),
    CONSTRAINT nodes_color_check CHECK (((color >= 1) AND (color <= 12))),
    CONSTRAINT nodes_custom_color_check CHECK ((custom_color ~ '^#[0-9a-f]{6}$'::text)),
    CONSTRAINT nodes_description_check CHECK ((char_length(description) <= 4000)),
    CONSTRAINT nodes_description_source_check CHECK ((description_source = ANY (ARRAY['user'::text, 'auto'::text]))),
    CONSTRAINT nodes_kind_check CHECK ((kind = ANY (ARRAY['org'::text, 'project'::text, 'topic'::text, 'thread'::text]))),
    CONSTRAINT nodes_name_check CHECK (((char_length(name) >= 1) AND (char_length(name) <= 120))),
    CONSTRAINT nodes_tags_source_check CHECK ((tags_source = ANY (ARRAY['user'::text, 'auto'::text]))),
    CONSTRAINT nodes_title_source_check CHECK ((title_source = ANY (ARRAY['user'::text, 'auto'::text]))),
    CONSTRAINT nodes_pkey PRIMARY KEY (id),
    CONSTRAINT nodes_archived_by_before_delete_fkey FOREIGN KEY (archived_by_before_delete) REFERENCES users(id),
    CONSTRAINT nodes_archived_by_fkey FOREIGN KEY (archived_by) REFERENCES users(id),
    CONSTRAINT nodes_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id),
    CONSTRAINT nodes_deleted_by_fkey FOREIGN KEY (deleted_by) REFERENCES users(id),
    CONSTRAINT nodes_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id),
    CONSTRAINT nodes_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES users(id),
    CONSTRAINT nodes_parent_id_fkey FOREIGN KEY (parent_id) REFERENCES nodes(id)
);

CREATE INDEX nodes_everything_created_idx ON nodes USING btree (created_at, id) WHERE (deleted_at IS NULL);

CREATE INDEX nodes_everything_owner_idx ON nodes USING btree (owner_id, created_at, id) WHERE (deleted_at IS NULL);

CREATE INDEX nodes_everything_parent_idx ON nodes USING btree (((parent_id)::text), lower(name), id) WHERE (deleted_at IS NULL);

CREATE INDEX nodes_name_trgm_idx ON nodes USING gin (name gin_trgm_ops) WHERE (deleted_at IS NULL);

CREATE INDEX nodes_org_id_idx ON nodes USING btree (org_id, id);

CREATE INDEX nodes_org_tree_idx ON nodes USING btree (org_id, tree_seq);

CREATE INDEX nodes_parent_idx ON nodes USING btree (parent_id) WHERE (deleted_at IS NULL);

CREATE INDEX nodes_path_idx ON nodes USING gin (path);

CREATE INDEX nodes_places_idx ON nodes USING btree (id) WHERE ((kind <> 'thread'::text) AND (deleted_at IS NULL));

CREATE INDEX related_node_folded_name_idx ON nodes USING btree (parent_id, orch_related_name_key(translate(orch_related_tree_name(name), 'ABCDEFGHIJKLMNOPQRSTUVWXYZ'::text, 'abcdefghijklmnopqrstuvwxyz'::text))) WHERE (deleted_at IS NULL);

CREATE INDEX related_node_name_idx ON nodes USING btree (parent_id, orch_related_name_key(orch_related_tree_name(name))) WHERE (deleted_at IS NULL);

CREATE INDEX related_threads_recent_idx ON nodes USING btree (GREATEST(updated_at, activity_at) DESC, id) WHERE (deleted_at IS NULL);

CREATE INDEX search_nodes_description_trgm_idx ON nodes USING gin (description gin_trgm_ops) WHERE ((deleted_at IS NULL) AND (description <> ''::text));

CREATE TRIGGER access_changed_delete AFTER DELETE ON nodes FOR EACH ROW WHEN ((old.kind <> 'thread'::text)) EXECUTE FUNCTION access_changed();

CREATE TRIGGER access_changed_insert AFTER INSERT ON nodes FOR EACH ROW WHEN ((new.kind <> 'thread'::text)) EXECUTE FUNCTION access_changed();

CREATE TRIGGER access_changed_update AFTER UPDATE OF kind, parent_id, org_id, path, deleted_at ON nodes FOR EACH ROW WHEN ((((old.kind <> 'thread'::text) OR (new.kind <> 'thread'::text)) AND (((((old.kind IS DISTINCT FROM new.kind) OR (old.parent_id IS DISTINCT FROM new.parent_id)) OR (old.org_id IS DISTINCT FROM new.org_id)) OR (old.path IS DISTINCT FROM new.path)) OR (old.deleted_at IS DISTINCT FROM new.deleted_at)))) EXECUTE FUNCTION access_changed();

CREATE TRIGGER import_node_order_deleted AFTER DELETE ON nodes FOR EACH ROW EXECUTE FUNCTION import_node_order_changed();

CREATE TRIGGER import_node_order_inserted AFTER INSERT ON nodes FOR EACH ROW EXECUTE FUNCTION import_node_order_changed();

CREATE TRIGGER import_node_order_updated AFTER UPDATE OF path, deleted_at ON nodes FOR EACH ROW WHEN (((old.path IS DISTINCT FROM new.path) OR (old.deleted_at IS DISTINCT FROM new.deleted_at))) EXECUTE FUNCTION import_node_order_changed();

CREATE TRIGGER participation_nodes_delete AFTER DELETE ON nodes REFERENCING OLD TABLE AS participation_old_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER participation_nodes_insert AFTER INSERT ON nodes REFERENCING NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER participation_nodes_update AFTER UPDATE ON nodes REFERENCING OLD TABLE AS participation_old_objects NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

ALTER TABLE api_token_refusals ADD CONSTRAINT api_token_refusals_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id) ON DELETE CASCADE;

ALTER TABLE api_tokens ADD CONSTRAINT api_tokens_node_fk FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE;

ALTER TABLE enrollment_tokens ADD CONSTRAINT enrollment_tokens_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id);

ALTER TABLE identity_providers ADD CONSTRAINT identity_providers_org_fk FOREIGN KEY (default_org_id) REFERENCES nodes(id);

ALTER TABLE invitations ADD CONSTRAINT invitations_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id);

ALTER TABLE invitations ADD CONSTRAINT invitations_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id);

ALTER TABLE users ADD CONSTRAINT users_personal_org_fk FOREIGN KEY (personal_org_id) REFERENCES nodes(id);

ALTER TABLE users ADD CONSTRAINT users_service_org_fk FOREIGN KEY (service_org_id) REFERENCES nodes(id);

CREATE TABLE org_members (
    org_id uuid NOT NULL,
    user_id uuid NOT NULL,
    domain_id uuid NOT NULL,
    joined_at timestamp with time zone DEFAULT now() NOT NULL,
    invited_by uuid,
    moved_at timestamp with time zone,
    moved_by uuid,
    CONSTRAINT org_members_pkey PRIMARY KEY (org_id, user_id),
    CONSTRAINT org_members_domain_id_fkey FOREIGN KEY (domain_id) REFERENCES nodes(id),
    CONSTRAINT org_members_invited_by_fkey FOREIGN KEY (invited_by) REFERENCES users(id),
    CONSTRAINT org_members_moved_by_fkey FOREIGN KEY (moved_by) REFERENCES users(id),
    CONSTRAINT org_members_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id) ON DELETE CASCADE,
    CONSTRAINT org_members_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX org_members_domain_idx ON org_members USING btree (domain_id);

CREATE INDEX org_members_user_idx ON org_members USING btree (user_id);

CREATE TRIGGER access_changed AFTER INSERT OR DELETE OR UPDATE ON org_members FOR EACH ROW EXECUTE FUNCTION access_changed();

CREATE TABLE org_suspensions (
    org_id uuid NOT NULL,
    user_id uuid NOT NULL,
    reason text DEFAULT ''::text NOT NULL,
    suspended_by uuid NOT NULL,
    suspended_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT org_suspensions_reason_check CHECK ((char_length(reason) <= 500)),
    CONSTRAINT org_suspensions_pkey PRIMARY KEY (org_id, user_id),
    CONSTRAINT org_suspensions_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id) ON DELETE CASCADE,
    CONSTRAINT org_suspensions_suspended_by_fkey FOREIGN KEY (suspended_by) REFERENCES users(id),
    CONSTRAINT org_suspensions_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TRIGGER access_changed AFTER INSERT OR DELETE OR UPDATE ON org_suspensions FOR EACH ROW EXECUTE FUNCTION access_changed();

CREATE TABLE org_user_activity (
    org_id uuid NOT NULL,
    user_id uuid NOT NULL,
    last_seen_at timestamp with time zone NOT NULL,
    CONSTRAINT org_user_activity_pkey PRIMARY KEY (org_id, user_id),
    CONSTRAINT org_user_activity_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id) ON DELETE CASCADE,
    CONSTRAINT org_user_activity_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE orgs (
    node_id uuid NOT NULL,
    parent_id uuid,
    slug text NOT NULL,
    org_kind text NOT NULL,
    closed boolean DEFAULT false NOT NULL,
    flat boolean DEFAULT false NOT NULL,
    tree_seq bigint DEFAULT 0 NOT NULL,
    tree_changed_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT orgs_check CHECK (((org_kind = 'team'::text) OR (parent_id IS NULL))),
    CONSTRAINT orgs_check1 CHECK (((parent_id IS NULL) OR ((NOT closed) AND (NOT flat)))),
    CONSTRAINT orgs_org_kind_check CHECK ((org_kind = ANY (ARRAY['personal'::text, 'team'::text]))),
    CONSTRAINT orgs_slug_check CHECK ((slug ~ '^[a-z0-9][a-z0-9_-]{1,31}$'::text)),
    CONSTRAINT orgs_pkey PRIMARY KEY (node_id),
    CONSTRAINT orgs_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE,
    CONSTRAINT orgs_parent_id_fkey FOREIGN KEY (parent_id) REFERENCES nodes(id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX orgs_slug_key ON orgs USING btree (parent_id, slug) NULLS NOT DISTINCT;

CREATE TRIGGER public_name_org AFTER INSERT OR UPDATE OF slug, parent_id, org_kind ON orgs FOR EACH ROW EXECUTE FUNCTION orch_public_name_org();

ALTER TABLE public_names ADD CONSTRAINT public_names_org_id_fkey FOREIGN KEY (org_id) REFERENCES orgs(node_id) ON DELETE SET NULL;

CREATE TABLE recents (
    user_id uuid NOT NULL,
    kind text NOT NULL,
    id uuid NOT NULL,
    visit_count double precision DEFAULT 1 NOT NULL,
    visited_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT recents_kind_check CHECK ((kind = ANY (ARRAY['org'::text, 'project'::text, 'topic'::text, 'thread'::text, 'document'::text, 'document_container'::text, 'chat'::text, 'tag'::text, 'user'::text, 'group'::text, 'provider'::text, 'agent'::text, 'environment'::text, 'host'::text, 'template'::text, 'mcp_server'::text, 'mount'::text, 'machine'::text, 'skill'::text]))),
    CONSTRAINT recents_visit_count_check CHECK (((visit_count >= (0)::double precision) AND (visit_count < 'Infinity'::double precision))),
    CONSTRAINT recents_pkey PRIMARY KEY (user_id, kind, id),
    CONSTRAINT recents_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX recents_visited_idx ON recents USING btree (user_id, visited_at DESC, kind, id);

CREATE TABLE tags (
    id uuid NOT NULL,
    node_id uuid NOT NULL,
    name text NOT NULL,
    color smallint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT tags_color_check CHECK (((color >= 0) AND (color <= 11))),
    CONSTRAINT tags_name_check CHECK (((char_length(name) >= 1) AND (char_length(name) <= 40))),
    CONSTRAINT tags_pkey PRIMARY KEY (id),
    CONSTRAINT tags_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX tags_node_name_key ON tags USING btree (node_id, lower(name));

CREATE TABLE topics (
    node_id uuid NOT NULL,
    status text DEFAULT 'open'::text NOT NULL,
    done_at timestamp with time zone,
    done_by uuid,
    topic_seq bigint DEFAULT 0 NOT NULL,
    CONSTRAINT topics_status_check CHECK ((status = ANY (ARRAY['open'::text, 'done'::text]))),
    CONSTRAINT topics_pkey PRIMARY KEY (node_id),
    CONSTRAINT topics_done_by_fkey FOREIGN KEY (done_by) REFERENCES users(id),
    CONSTRAINT topics_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE
);

CREATE TABLE groups (
    id uuid NOT NULL,
    org_id uuid NOT NULL,
    name text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    created_by uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT groups_name_check CHECK (((char_length(name) >= 1) AND (char_length(name) <= 60))),
    CONSTRAINT groups_pkey PRIMARY KEY (id),
    CONSTRAINT groups_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id),
    CONSTRAINT groups_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX groups_org_name_key ON groups USING btree (org_id, lower(name));

CREATE TRIGGER access_changed AFTER INSERT OR DELETE OR UPDATE OF id, org_id ON groups FOR EACH ROW EXECUTE FUNCTION access_changed();

CREATE TABLE node_roles (
    id uuid NOT NULL,
    node_id uuid NOT NULL,
    subject_kind text NOT NULL,
    subject_id uuid NOT NULL,
    subject_role text,
    role text NOT NULL,
    expires_at timestamp with time zone,
    note text DEFAULT ''::text NOT NULL,
    created_by uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    removed_at timestamp with time zone,
    removed_by uuid,
    revision bigint DEFAULT 1 NOT NULL,
    CONSTRAINT node_roles_check CHECK (((subject_kind = 'role'::text) = (subject_role IS NOT NULL))),
    CONSTRAINT node_roles_note_check CHECK ((char_length(note) <= 500)),
    CONSTRAINT node_roles_role_check CHECK ((role = ANY (ARRAY['viewer'::text, 'interactor'::text, 'developer'::text, 'admin'::text]))),
    CONSTRAINT node_roles_subject_kind_check CHECK ((subject_kind = ANY (ARRAY['user'::text, 'group'::text, 'members'::text, 'role'::text]))),
    CONSTRAINT node_roles_subject_role_check CHECK ((subject_role = ANY (ARRAY['viewer'::text, 'interactor'::text, 'developer'::text, 'admin'::text]))),
    CONSTRAINT node_roles_pkey PRIMARY KEY (id),
    CONSTRAINT node_roles_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id),
    CONSTRAINT node_roles_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE,
    CONSTRAINT node_roles_removed_by_fkey FOREIGN KEY (removed_by) REFERENCES users(id)
);

-- A holder keeps one lasting role at a place and others that end beside it, one grant for each role; the strongest one
-- in force applies, and when one that ends lapses the others stay.
CREATE UNIQUE INDEX node_roles_active_key ON node_roles USING btree (node_id, subject_kind, subject_id, subject_role, role) NULLS NOT DISTINCT WHERE (removed_at IS NULL);
CREATE UNIQUE INDEX node_roles_lasting_key ON node_roles USING btree (node_id, subject_kind, subject_id, subject_role) NULLS NOT DISTINCT WHERE (removed_at IS NULL AND expires_at IS NULL);

CREATE INDEX node_roles_ending_idx ON node_roles USING btree (expires_at) WHERE ((removed_at IS NULL) AND (expires_at IS NOT NULL));

CREATE INDEX node_roles_subject_idx ON node_roles USING btree (subject_kind, subject_id) WHERE (removed_at IS NULL);

CREATE TRIGGER access_changed AFTER INSERT OR DELETE OR UPDATE ON node_roles FOR EACH ROW EXECUTE FUNCTION access_changed();

CREATE TABLE node_summary_state (
    node_id uuid NOT NULL,
    due_at timestamp with time zone,
    next_at timestamp with time zone NOT NULL,
    last_at timestamp with time zone,
    last_seq bigint DEFAULT 0 NOT NULL,
    failures bigint DEFAULT 0 NOT NULL,
    reason text,
    run_id uuid,
    claimed_by uuid,
    claimed_until timestamp with time zone,
    CONSTRAINT node_summary_state_check CHECK (((claimed_by IS NULL) = (claimed_until IS NULL))),
    CONSTRAINT node_summary_state_pkey PRIMARY KEY (node_id),
    CONSTRAINT node_summary_state_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE
);

CREATE INDEX node_summary_due_idx ON node_summary_state USING btree (next_at, due_at) WHERE (due_at IS NOT NULL);

CREATE TABLE node_tags (
    node_id uuid NOT NULL,
    tag_id uuid NOT NULL,
    CONSTRAINT node_tags_pkey PRIMARY KEY (node_id, tag_id),
    CONSTRAINT node_tags_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE,
    CONSTRAINT node_tags_tag_id_fkey FOREIGN KEY (tag_id) REFERENCES tags(id) ON DELETE CASCADE
);

CREATE INDEX node_tags_tag_idx ON node_tags USING btree (tag_id, node_id);

CREATE TABLE group_members (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    group_id uuid NOT NULL,
    user_id uuid,
    member_group_id uuid,
    member_node_id uuid,
    member_org_id uuid,
    member_role text,
    added_by uuid,
    added_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT group_members_check CHECK ((num_nonnulls(user_id, member_group_id, member_node_id, member_org_id) = 1)),
    CONSTRAINT group_members_check1 CHECK (((member_node_id IS NOT NULL) = (member_role IS NOT NULL))),
    CONSTRAINT group_members_member_role_check CHECK ((member_role = ANY (ARRAY['viewer'::text, 'interactor'::text, 'developer'::text, 'admin'::text]))),
    CONSTRAINT group_members_group_id_user_id_key UNIQUE (group_id, user_id),
    CONSTRAINT group_members_group_id_user_id_member_group_id_member_node__key UNIQUE NULLS NOT DISTINCT (group_id, user_id, member_group_id, member_node_id, member_org_id, member_role),
    CONSTRAINT group_members_pkey PRIMARY KEY (id),
    CONSTRAINT group_members_added_by_fkey FOREIGN KEY (added_by) REFERENCES users(id),
    CONSTRAINT group_members_group_id_fkey FOREIGN KEY (group_id) REFERENCES groups(id) ON DELETE CASCADE,
    CONSTRAINT group_members_member_group_id_fkey FOREIGN KEY (member_group_id) REFERENCES groups(id) ON DELETE CASCADE,
    CONSTRAINT group_members_member_node_id_fkey FOREIGN KEY (member_node_id) REFERENCES nodes(id) ON DELETE CASCADE,
    CONSTRAINT group_members_member_org_id_fkey FOREIGN KEY (member_org_id) REFERENCES nodes(id) ON DELETE CASCADE,
    CONSTRAINT group_members_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX group_members_nested_idx ON group_members USING btree (member_group_id) WHERE (member_group_id IS NOT NULL);

CREATE TRIGGER access_changed AFTER INSERT OR DELETE OR UPDATE ON group_members FOR EACH ROW EXECUTE FUNCTION access_changed();
