-- Permission rows and entries, authorization epochs, access changes and the audit trail.

CREATE TABLE access_changes (
    xid xid8 NOT NULL,
    at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT access_changes_pkey PRIMARY KEY (xid)
);

CREATE TABLE access_changes_floor (
    id boolean DEFAULT true NOT NULL,
    floor xid8 NOT NULL,
    CONSTRAINT access_changes_floor_id_check CHECK (id),
    CONSTRAINT access_changes_floor_pkey PRIMARY KEY (id)
);

CREATE TABLE agent_shares (
    id uuid NOT NULL,
    thread_id uuid NOT NULL,
    environment_id uuid,
    object_kind text NOT NULL,
    object_id uuid NOT NULL,
    include text[] DEFAULT '{timeline}'::text[] NOT NULL,
    path text NOT NULL,
    manifest_blob bytea,
    file_count integer DEFAULT 0 NOT NULL,
    total_bytes bigint DEFAULT 0 NOT NULL,
    read_first boolean DEFAULT false NOT NULL,
    as_of timestamp with time zone NOT NULL,
    state text DEFAULT 'pending'::text NOT NULL,
    created_by uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    delivered_at timestamp with time zone,
    revoked_by uuid,
    revoked_at timestamp with time zone,
    CONSTRAINT agent_shares_object_kind_check CHECK ((object_kind = ANY (ARRAY['node'::text, 'chat'::text, 'document'::text]))),
    CONSTRAINT agent_shares_state_check CHECK ((state = ANY (ARRAY['pending'::text, 'delivered'::text, 'revoked'::text]))),
    CONSTRAINT agent_shares_pkey PRIMARY KEY (id),
    CONSTRAINT agent_shares_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id),
    CONSTRAINT agent_shares_revoked_by_fkey FOREIGN KEY (revoked_by) REFERENCES users(id)
);

CREATE INDEX agent_shares_object_idx ON agent_shares USING btree (object_kind, object_id) WHERE (state <> 'revoked'::text);

CREATE INDEX agent_shares_thread_idx ON agent_shares USING btree (thread_id) WHERE (state <> 'revoked'::text);

CREATE TABLE audit_log (
    id uuid NOT NULL,
    at timestamp with time zone DEFAULT now() NOT NULL,
    actor_kind text NOT NULL,
    actor_id uuid,
    for_user_id uuid,
    action text NOT NULL,
    as_admin boolean DEFAULT false NOT NULL,
    history boolean DEFAULT false NOT NULL,
    override boolean DEFAULT false NOT NULL,
    object_kind text NOT NULL,
    object_id uuid,
    object_name text,
    scope_node_ids uuid[] DEFAULT '{}'::uuid[] NOT NULL,
    node_id uuid,
    org_id uuid,
    environment_id uuid,
    summary text NOT NULL,
    details jsonb DEFAULT '{}'::jsonb NOT NULL,
    ip inet,
    user_agent text,
    request_id text,
    person_notify_count numeric,
    CONSTRAINT audit_log_actor_kind_check CHECK ((actor_kind = ANY (ARRAY['user'::text, 'system'::text, 'agent'::text]))),
    CONSTRAINT audit_log_pkey PRIMARY KEY (id),
    CONSTRAINT audit_log_for_user_id_fkey FOREIGN KEY (for_user_id) REFERENCES users(id)
);

CREATE INDEX audit_actor_idx ON audit_log USING btree (actor_id, at DESC);

CREATE INDEX audit_at_idx ON audit_log USING btree (at DESC);

CREATE UNIQUE INDEX audit_deleted_inspected_day_idx ON audit_log USING btree (actor_id, object_kind, object_id, (((at AT TIME ZONE 'UTC'::text))::date)) WHERE (action = 'deleted.inspected'::text);

CREATE INDEX audit_environment_config_idx ON audit_log USING btree (object_id, at DESC, id DESC) WHERE ((action = 'environment.updated'::text) AND (object_kind = 'environment'::text));

CREATE INDEX audit_environment_idx ON audit_log USING btree (environment_id, at DESC) WHERE (environment_id IS NOT NULL);

CREATE INDEX audit_for_user_idx ON audit_log USING btree (for_user_id, at DESC) WHERE (for_user_id IS NOT NULL);

CREATE INDEX audit_node_idx ON audit_log USING btree (node_id, at DESC);

CREATE INDEX audit_object_idx ON audit_log USING btree (object_id, at DESC) WHERE (object_id IS NOT NULL);

CREATE INDEX audit_opening_idx ON audit_log USING btree (actor_id, object_id, ((details ->> 'session_id'::text)), ((details ->> 'opening_id'::text))) WHERE (action = ANY (ARRAY['admin.opened'::text, 'chat.inspected'::text, 'profile.inspected'::text]));

CREATE INDEX audit_org_idx ON audit_log USING btree (org_id, at DESC);

CREATE INDEX audit_person_notify_rate_idx ON audit_log USING btree (actor_id, org_id, at, id) INCLUDE (person_notify_count) WHERE (action = 'person.notified'::text);

CREATE INDEX audit_person_notify_system_rate_idx ON audit_log USING btree (actor_id, at, id) INCLUDE (person_notify_count) WHERE ((action = 'person.notified'::text) AND (org_id IS NULL));

CREATE INDEX audit_provider_plan_window_idx ON audit_log USING btree (object_id, at DESC, id) WHERE (action = ANY (ARRAY['provider.reset_used'::text, 'provider.window_started_over'::text]));

CREATE INDEX audit_provider_reset_receipt_idx ON audit_log USING btree (object_id, actor_id, ((details ->> 'request_id'::text))) WHERE (action = 'provider.reset_used'::text);

CREATE INDEX audit_scope_idx ON audit_log USING gin (scope_node_ids);

CREATE INDEX audit_upload_batch_idx ON audit_log USING btree (environment_id, actor_id, ((details ->> 'upload_batch'::text)), id) WHERE ((action = 'workspace.file_written'::text) AND ((details ->> 'upload_batch'::text) IS NOT NULL));

CREATE INDEX audit_upload_summary_idx ON audit_log USING btree (environment_id, actor_id, (((details -> 'upload'::text) ->> 'id'::text))) WHERE ((action = 'workspace.file_written'::text) AND (((details -> 'upload'::text) ->> 'id'::text) IS NOT NULL));

CREATE TRIGGER audit_person_notify_count BEFORE INSERT OR UPDATE OF action, details ON audit_log FOR EACH ROW EXECUTE FUNCTION audit_person_notify_count();

CREATE TABLE audit_object_state (
    object_kind text NOT NULL,
    object_id uuid NOT NULL,
    last_name text NOT NULL,
    deletion_id uuid,
    deleted_at timestamp with time zone,
    deleted_by uuid,
    purge_at timestamp with time zone,
    purged_at timestamp with time zone,
    purged_by uuid,
    home_node_id uuid,
    CONSTRAINT audit_object_state_pkey PRIMARY KEY (object_kind, object_id),
    CONSTRAINT audit_object_state_deleted_by_fkey FOREIGN KEY (deleted_by) REFERENCES users(id),
    CONSTRAINT audit_object_state_purged_by_fkey FOREIGN KEY (purged_by) REFERENCES users(id)
);

CREATE TABLE authz_epochs (
    org_id uuid NOT NULL,
    epoch bigint DEFAULT 0 NOT NULL,
    CONSTRAINT authz_epochs_pkey PRIMARY KEY (org_id)
);

INSERT INTO authz_epochs (org_id) VALUES ('00000000-0000-0000-0000-000000000000'), ('00000000-0000-0000-0000-000000000001');

CREATE TABLE authz_state (
    id boolean DEFAULT true NOT NULL,
    epoch bigint DEFAULT 0 NOT NULL,
    CONSTRAINT authz_state_id_check CHECK (id),
    CONSTRAINT authz_state_pkey PRIMARY KEY (id)
);

INSERT INTO authz_state (id) VALUES (true);

CREATE UNLOGGED TABLE commit_work (
    xact xid8 NOT NULL,
    CONSTRAINT commit_work_pkey PRIMARY KEY (xact)
);

CREATE CONSTRAINT TRIGGER commit_work_run AFTER INSERT ON commit_work DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION orch_commit_work();

CREATE TABLE participation_direct_counts (
    object_kind text NOT NULL,
    object_id uuid NOT NULL,
    subject_kind text NOT NULL,
    subject_id uuid NOT NULL,
    owner_org uuid,
    time_shift smallint NOT NULL,
    time_bucket bigint NOT NULL,
    total bigint NOT NULL,
    CONSTRAINT participation_direct_counts_subject_kind_subject_id_object__key UNIQUE NULLS NOT DISTINCT (subject_kind, subject_id, object_kind, object_id, time_shift, time_bucket, owner_org)
);

CREATE INDEX participation_direct_empty_idx ON participation_direct_counts USING btree (total) WHERE (total <= 0);

CREATE TABLE participation_entries (
    entry_id uuid NOT NULL,
    object_kind text NOT NULL,
    object_id uuid NOT NULL,
    home_id uuid,
    subject_kind text NOT NULL,
    subject_id uuid NOT NULL,
    owner_org uuid,
    expires_at timestamp with time zone,
    CONSTRAINT participation_entries_pkey PRIMARY KEY (entry_id)
);

CREATE INDEX participation_entries_object_idx ON participation_entries USING btree (object_kind, object_id);

CREATE TRIGGER participation_sources_delete AFTER DELETE ON participation_entries REFERENCING OLD TABLE AS participation_old_sources FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_source_counts();

CREATE TRIGGER participation_sources_insert AFTER INSERT ON participation_entries REFERENCING NEW TABLE AS participation_new_sources FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_source_counts();

CREATE TRIGGER participation_sources_update AFTER UPDATE ON participation_entries REFERENCING OLD TABLE AS participation_old_sources NEW TABLE AS participation_new_sources FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_source_counts();

CREATE TABLE participation_home_counts (
    home_id uuid NOT NULL,
    subject_kind text NOT NULL,
    subject_id uuid NOT NULL,
    owner_org uuid,
    time_shift smallint NOT NULL,
    time_bucket bigint NOT NULL,
    total bigint NOT NULL,
    CONSTRAINT participation_home_counts_subject_kind_subject_id_home_id_t_key UNIQUE NULLS NOT DISTINCT (subject_kind, subject_id, home_id, time_shift, time_bucket, owner_org)
);

CREATE INDEX participation_home_counts_home_idx ON participation_home_counts USING btree (home_id);

CREATE INDEX participation_home_empty_idx ON participation_home_counts USING btree (total) WHERE (total <= 0);

CREATE TABLE participation_node_counts (
    node_id uuid NOT NULL,
    scope_root uuid NOT NULL,
    subject_kind text NOT NULL,
    subject_id uuid NOT NULL,
    owner_org uuid,
    time_shift smallint NOT NULL,
    time_bucket bigint NOT NULL,
    total bigint NOT NULL,
    CONSTRAINT participation_node_counts_subject_kind_subject_id_node_id_s_key UNIQUE NULLS NOT DISTINCT (subject_kind, subject_id, node_id, scope_root, time_shift, time_bucket, owner_org)
);

CREATE INDEX participation_node_empty_idx ON participation_node_counts USING btree (total) WHERE (total <= 0);

CREATE TABLE participation_object_facts (
    object_kind text NOT NULL,
    object_id uuid NOT NULL,
    home_id uuid,
    org_id uuid,
    live boolean NOT NULL,
    CONSTRAINT participation_object_facts_pkey PRIMARY KEY (object_kind, object_id)
);

CREATE TABLE participation_paths (
    node_id uuid NOT NULL,
    path uuid[] NOT NULL,
    live boolean NOT NULL,
    CONSTRAINT participation_paths_pkey PRIMARY KEY (node_id)
);

CREATE INDEX participation_paths_path_idx ON participation_paths USING gin (path);

CREATE TABLE permission_entries (
    id uuid NOT NULL,
    object_kind text NOT NULL,
    object_id uuid NOT NULL,
    page text NOT NULL,
    profile text DEFAULT 'any'::text NOT NULL,
    permission text NOT NULL,
    holder_kind text NOT NULL,
    holder_id uuid,
    holder_role text,
    options jsonb DEFAULT '{}'::jsonb NOT NULL,
    read_first boolean DEFAULT false NOT NULL,
    share_inside boolean,
    expires_at timestamp with time zone,
    note text DEFAULT ''::text NOT NULL,
    source text DEFAULT 'page'::text NOT NULL,
    created_by uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    removed_at timestamp with time zone,
    removed_by uuid,
    revision bigint DEFAULT 1 NOT NULL,
    CONSTRAINT permission_entries_check CHECK (((share_inside IS NULL) OR ((holder_kind = 'agents'::text) AND (object_kind = 'node'::text)))),
    CONSTRAINT permission_entries_check1 CHECK (((holder_kind = 'role'::text) = (holder_role IS NOT NULL))),
    CONSTRAINT permission_entries_check2 CHECK (((holder_kind <> 'everyone'::text) OR (holder_id IS NULL))),
    CONSTRAINT permission_entries_check3 CHECK (((holder_kind = ANY (ARRAY['role'::text, 'everyone'::text])) OR (holder_id IS NOT NULL))),
    CONSTRAINT permission_entries_check4 CHECK (((page = 'defaults'::text) OR (profile = 'any'::text))),
    CONSTRAINT permission_entries_check5 CHECK (((permission <> 'access.own'::text) OR ((page = 'live'::text) AND (holder_kind = ANY (ARRAY['user'::text, 'group'::text]))))),
    CONSTRAINT permission_entries_holder_kind_check CHECK ((holder_kind = ANY (ARRAY['role'::text, 'group'::text, 'user'::text, 'node'::text, 'agents'::text, 'members'::text, 'everyone'::text]))),
    CONSTRAINT permission_entries_holder_role_check CHECK ((holder_role = ANY (ARRAY['viewer'::text, 'interactor'::text, 'developer'::text, 'admin'::text]))),
    CONSTRAINT permission_entries_note_check CHECK ((char_length(note) <= 500)),
    CONSTRAINT permission_entries_object_kind_check CHECK ((object_kind = ANY (ARRAY['system'::text, 'node'::text, 'provider'::text, 'agent'::text, 'environment'::text, 'host'::text, 'machine'::text, 'template'::text, 'mcp_server'::text, 'mount'::text, 'skill'::text, 'document'::text, 'document_container'::text, 'chat'::text]))),
    CONSTRAINT permission_entries_page_check CHECK ((page = ANY (ARRAY['live'::text, 'defaults'::text]))),
    CONSTRAINT permission_entries_profile_check CHECK ((profile = ANY (ARRAY['any'::text, 'managed'::text, 'connected'::text]))),
    CONSTRAINT permission_entries_source_check CHECK ((source = ANY (ARRAY['page'::text, 'creation'::text, 'request'::text, 'seed'::text]))),
    CONSTRAINT permission_entries_pkey PRIMARY KEY (id),
    CONSTRAINT permission_entries_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id),
    CONSTRAINT permission_entries_removed_by_fkey FOREIGN KEY (removed_by) REFERENCES users(id)
);

CREATE UNIQUE INDEX permission_entries_active_key ON permission_entries USING btree (object_kind, object_id, page, profile, permission, holder_kind, holder_id, holder_role, share_inside) NULLS NOT DISTINCT WHERE (removed_at IS NULL);

CREATE INDEX permission_entries_ending_idx ON permission_entries USING btree (expires_at) WHERE ((removed_at IS NULL) AND (expires_at IS NOT NULL) AND (object_kind = ANY (ARRAY['system'::text, 'node'::text])));

CREATE INDEX permission_entries_hierarchy_idx ON permission_entries USING btree (object_id) WHERE ((object_kind = 'node'::text) AND (page = 'live'::text) AND (permission <> 'access.own'::text) AND (removed_at IS NULL));

CREATE INDEX permission_entries_holder_idx ON permission_entries USING btree (holder_kind, holder_id) WHERE (removed_at IS NULL);

CREATE INDEX related_agent_entrances_idx ON permission_entries USING btree (holder_id, expires_at, object_kind, object_id, permission) WHERE ((holder_kind = 'agents'::text) AND (removed_at IS NULL));

CREATE INDEX related_agent_grants_idx ON permission_entries USING btree (holder_id, object_kind, object_id, expires_at, permission) WHERE ((holder_kind = 'agents'::text) AND (removed_at IS NULL));

CREATE TRIGGER access_changed_delete AFTER DELETE ON permission_entries FOR EACH ROW WHEN ((old.object_kind = ANY (ARRAY['system'::text, 'node'::text]))) EXECUTE FUNCTION access_page_changed();

CREATE TRIGGER access_changed_insert AFTER INSERT ON permission_entries FOR EACH ROW WHEN ((new.object_kind = ANY (ARRAY['system'::text, 'node'::text]))) EXECUTE FUNCTION access_page_changed();

CREATE TRIGGER access_changed_update AFTER UPDATE ON permission_entries FOR EACH ROW WHEN (((old.object_kind = ANY (ARRAY['system'::text, 'node'::text])) OR (new.object_kind = ANY (ARRAY['system'::text, 'node'::text])))) EXECUTE FUNCTION access_page_changed();

CREATE TRIGGER participation_permission_entries_delete AFTER DELETE ON permission_entries REFERENCING OLD TABLE AS participation_old_entries FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_entries_changed();

CREATE TRIGGER participation_permission_entries_insert AFTER INSERT ON permission_entries REFERENCING NEW TABLE AS participation_new_entries FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_entries_changed();

CREATE TRIGGER participation_permission_entries_update AFTER UPDATE ON permission_entries REFERENCING OLD TABLE AS participation_old_entries NEW TABLE AS participation_new_entries FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_entries_changed();

CREATE TABLE permission_rows (
    object_kind text NOT NULL,
    object_id uuid NOT NULL,
    page text NOT NULL,
    profile text DEFAULT 'any'::text NOT NULL,
    permission text NOT NULL,
    inherit boolean DEFAULT true NOT NULL,
    locked boolean DEFAULT false NOT NULL,
    updated_by uuid,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    CONSTRAINT permission_rows_check CHECK (((page = 'defaults'::text) OR ((profile = 'any'::text) AND (NOT locked)))),
    CONSTRAINT permission_rows_object_kind_check CHECK ((object_kind = ANY (ARRAY['system'::text, 'node'::text, 'provider'::text, 'agent'::text, 'environment'::text, 'host'::text, 'machine'::text, 'template'::text, 'mcp_server'::text, 'mount'::text, 'skill'::text, 'document'::text, 'document_container'::text]))),
    CONSTRAINT permission_rows_page_check CHECK ((page = ANY (ARRAY['live'::text, 'defaults'::text]))),
    CONSTRAINT permission_rows_profile_check CHECK ((profile = ANY (ARRAY['any'::text, 'managed'::text, 'connected'::text]))),
    CONSTRAINT permission_rows_pkey PRIMARY KEY (object_kind, object_id, page, profile, permission),
    CONSTRAINT permission_rows_updated_by_fkey FOREIGN KEY (updated_by) REFERENCES users(id)
);

CREATE TRIGGER access_changed_delete AFTER DELETE ON permission_rows FOR EACH ROW WHEN ((old.object_kind = ANY (ARRAY['system'::text, 'node'::text]))) EXECUTE FUNCTION access_page_changed();

CREATE TRIGGER access_changed_insert AFTER INSERT ON permission_rows FOR EACH ROW WHEN ((new.object_kind = ANY (ARRAY['system'::text, 'node'::text]))) EXECUTE FUNCTION access_page_changed();

CREATE TRIGGER access_changed_update AFTER UPDATE ON permission_rows FOR EACH ROW WHEN (((old.object_kind = ANY (ARRAY['system'::text, 'node'::text])) OR (new.object_kind = ANY (ARRAY['system'::text, 'node'::text])))) EXECUTE FUNCTION access_page_changed();
