-- Settings and secrets, jobs, leases, idempotency, installation and components.

CREATE TABLE component_heartbeats (
    component text NOT NULL,
    system uuid NOT NULL,
    version text NOT NULL,
    started_at timestamp with time zone NOT NULL,
    seen_at timestamp with time zone NOT NULL,
    CONSTRAINT component_heartbeats_component_check CHECK ((component = ANY (ARRAY['api'::text, 'gateway'::text, 'worker'::text]))),
    CONSTRAINT component_heartbeats_pkey PRIMARY KEY (component)
);

CREATE TABLE component_updates (
    machine_id uuid NOT NULL,
    recommended_version text NOT NULL,
    state text NOT NULL,
    detail text,
    command_id uuid,
    requested_by uuid,
    requested_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT component_updates_state_check CHECK ((state = ANY (ARRAY['queued'::text, 'updating'::text, 'current'::text, 'refused'::text, 'failed'::text]))),
    CONSTRAINT component_updates_pkey PRIMARY KEY (machine_id),
    CONSTRAINT component_updates_requested_by_fkey FOREIGN KEY (requested_by) REFERENCES users(id)
);

CREATE TABLE idempotency_keys (
    user_id uuid NOT NULL,
    key text NOT NULL,
    method text NOT NULL,
    path text NOT NULL,
    status integer NOT NULL,
    response jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT idempotency_keys_pkey PRIMARY KEY (user_id, key),
    CONSTRAINT idempotency_keys_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE installation_records (
    key text NOT NULL,
    value jsonb NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT installation_records_key_check CHECK ((key = ANY (ARRAY['release_check'::text, 'database_backup'::text]))),
    CONSTRAINT installation_records_pkey PRIMARY KEY (key)
);

CREATE TABLE job_leases (
    name text NOT NULL,
    holder text NOT NULL,
    until timestamp with time zone NOT NULL,
    CONSTRAINT job_leases_pkey PRIMARY KEY (name)
);

CREATE TABLE jobs (
    id uuid NOT NULL,
    kind text NOT NULL,
    node_id uuid,
    environment_id uuid,
    provider_id uuid,
    batch_id uuid,
    state text DEFAULT 'queued'::text NOT NULL,
    plan jsonb DEFAULT '{}'::jsonb NOT NULL,
    progress jsonb DEFAULT '{}'::jsonb NOT NULL,
    result jsonb,
    error jsonb,
    created_by uuid,
    topic_seq bigint,
    env_seq bigint,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    started_at timestamp with time zone,
    finished_at timestamp with time zone,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT jobs_kind_check CHECK (((kind = ANY (ARRAY['import'::text, 'snapshot'::text, 'thread_fork'::text, 'environment_move'::text, 'environment_recreate'::text, 'environment_restart'::text, 'mcp_restart'::text, 'environment_provision'::text, 'environment_remove'::text, 'environment_restore'::text, 'environment_purge'::text, 'thread_background_end'::text, 'provider_provision'::text, 'provider_recreate'::text, 'capability_change'::text, 'kept_images_sweep'::text, 'delivery'::text, 'export'::text, 'workspace_prepare'::text, 'git_commit'::text, 'git_push'::text, 'workspace_restore'::text, 'thread_continuation'::text, 'measurement'::text])) OR (kind = ANY ('{mcp_offering,host_delete}'::text[])) OR (kind = ANY ('{storage_cleanup,resource_quiesce}'::text[])))),
    CONSTRAINT jobs_state_check CHECK (((state = ANY (ARRAY['queued'::text, 'running'::text, 'succeeded'::text, 'failed'::text, 'cancelled'::text])) OR (state = ANY ('{lost}'::text[])))),
    CONSTRAINT jobs_pkey PRIMARY KEY (id),
    CONSTRAINT jobs_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id),
    CONSTRAINT jobs_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id)
);

CREATE INDEX jobs_active_command_idx ON jobs USING btree ((((plan -> 'active_command'::text) ->> 'id'::text))) WHERE (state = ANY (ARRAY['queued'::text, 'running'::text]));

CREATE INDEX jobs_active_environment_idx ON jobs USING btree (environment_id) WHERE ((environment_id IS NOT NULL) AND (state = ANY (ARRAY['queued'::text, 'running'::text])));

CREATE INDEX jobs_batch_idx ON jobs USING btree (batch_id) WHERE (batch_id IS NOT NULL);

CREATE UNIQUE INDEX jobs_cleanup_plan_idx ON jobs USING btree (((((plan -> 'recipe'::text) -> 'plan'::text) ->> 'id'::text))) WHERE (kind = 'storage_cleanup'::text);

CREATE INDEX jobs_cleanup_purges_idx ON jobs USING gin (((plan -> 'cleanup_purges'::text)) jsonb_path_ops) WHERE (kind = 'storage_cleanup'::text);

CREATE INDEX jobs_cleanup_work_idx ON jobs USING gin ((((plan -> 'recipe'::text) -> 'work'::text)) jsonb_path_ops) WHERE ((kind = 'storage_cleanup'::text) AND (state = ANY (ARRAY['queued'::text, 'running'::text])));

CREATE INDEX jobs_document_notice_dispatch_idx ON jobs USING btree (created_at, id) WHERE ((kind = 'document_notices'::text) AND (state = ANY (ARRAY['queued'::text, 'running'::text])));

CREATE INDEX jobs_document_notices_idx ON jobs USING btree (created_by, ((plan #>> '{notice,document}'::text[])), id DESC) WHERE ((kind = 'document_notices'::text) AND (state = ANY (ARRAY['queued'::text, 'running'::text])));

CREATE INDEX jobs_environment_host_idx ON jobs USING btree (environment_id, created_at DESC) WHERE (plan ? 'host'::text);

CREATE UNIQUE INDEX jobs_host_delete_open_idx ON jobs USING btree (((plan #>> '{cascade,host}'::text[]))) WHERE ((kind = 'host_delete'::text) AND (state = ANY (ARRAY['queued'::text, 'running'::text])));

CREATE INDEX jobs_host_facts_idx ON jobs USING btree (((plan ->> 'host'::text))) WHERE (state = ANY (ARRAY['queued'::text, 'running'::text]));

CREATE INDEX jobs_measurement_data_idx ON jobs USING btree (((plan ->> 'measurement_key'::text)), created_at DESC) WHERE (kind = 'measurement'::text);

CREATE INDEX jobs_notification_operation_idx ON jobs USING btree (((plan ->> 'notification_operation'::text))) WHERE (plan ? 'notification_operation'::text);

CREATE INDEX jobs_provider_facts_idx ON jobs USING btree (provider_id) WHERE (state = ANY (ARRAY['queued'::text, 'running'::text]));

CREATE INDEX jobs_runtime_created_spec_idx ON jobs USING btree (((plan ->> 'runtime'::text)), created_at DESC, id DESC) WHERE (((plan -> 'created_spec'::text) -> 'spec'::text) IS NOT NULL);

CREATE INDEX jobs_template_facts_idx ON jobs USING btree (((plan ->> 'template'::text))) WHERE (state = ANY (ARRAY['queued'::text, 'running'::text]));

CREATE INDEX thread_display_holding_jobs_idx ON jobs USING btree (node_id, environment_id, provider_id) WHERE ((state = ANY (ARRAY['queued'::text, 'running'::text])) OR ((kind = 'workspace_restore'::text) AND (state = 'failed'::text) AND ((plan ->> 'restore_recovery_required'::text) = 'true'::text)));

CREATE TRIGGER continuation_job_snapshot AFTER UPDATE OF state, progress ON jobs FOR EACH ROW WHEN ((new.kind = 'thread_continuation'::text)) EXECUTE FUNCTION thread_continuation_job_snapshot();

CREATE TRIGGER job_views_live AFTER INSERT OR DELETE OR UPDATE OF state, progress, result, error, node_id, environment_id, provider_id, batch_id, created_by, started_at, finished_at ON jobs FOR EACH ROW EXECUTE FUNCTION job_views_notify();

CREATE TABLE leases (
    id uuid NOT NULL,
    provider_id uuid NOT NULL,
    thread_id uuid NOT NULL,
    turn_id uuid,
    entry_id uuid,
    mode text DEFAULT 'forwarded'::text NOT NULL,
    form text,
    recipient_machine_id uuid,
    copy_expires_at timestamp with time zone,
    copy_issued_at timestamp with time zone,
    copy_issued_count bigint DEFAULT 0 NOT NULL,
    allowed_models text[] NOT NULL,
    allowance jsonb,
    opened_at timestamp with time zone DEFAULT now() NOT NULL,
    closed_at timestamp with time zone,
    close_reason text,
    CONSTRAINT leases_close_reason_check CHECK ((close_reason = ANY (ARRAY['turn_ended'::text, 'interrupted'::text, 'disabled'::text, 'archived'::text, 'cap_reached'::text, 'error'::text]))),
    CONSTRAINT leases_copy_issued_count_check CHECK ((copy_issued_count >= 0)),
    CONSTRAINT leases_forwarded_recipient CHECK (((mode <> 'forwarded'::text) OR (recipient_machine_id IS NULL))),
    CONSTRAINT leases_mode_check CHECK ((mode = ANY (ARRAY['forwarded'::text, 'shared'::text, 'handover'::text]))),
    CONSTRAINT leases_shared_recipient CHECK (((mode <> 'shared'::text) OR (recipient_machine_id IS NOT NULL))),
    CONSTRAINT leases_pkey PRIMARY KEY (id),
    CONSTRAINT leases_entry_id_fkey FOREIGN KEY (entry_id) REFERENCES permission_entries(id)
);

CREATE INDEX leases_forwarded_closed_idx ON leases USING btree (closed_at, id) WHERE ((closed_at IS NOT NULL) AND (mode = 'forwarded'::text));

CREATE UNIQUE INDEX leases_open_key ON leases USING btree (thread_id) WHERE (closed_at IS NULL);

CREATE INDEX leases_thread_closed_idx ON leases USING btree (thread_id, closed_at, id) WHERE (closed_at IS NOT NULL);

CREATE TABLE preflights (
    id uuid NOT NULL,
    kind text NOT NULL,
    node_id uuid,
    environment_id uuid,
    created_by uuid NOT NULL,
    plan jsonb NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT preflights_kind_check CHECK (((kind = ANY (ARRAY['thread_fork'::text, 'fork_batch'::text, 'environment_move'::text, 'move_batch'::text, 'environment_update'::text, 'provider_update'::text, 'update_batch'::text, 'workspace_restore'::text, 'thread_move'::text, 'port_opening'::text, 'node_delete'::text, 'node_archive'::text, 'node_move'::text, 'outside_removal'::text])) OR (kind = ANY ('{loss,recover_batch,host_delete}'::text[])) OR (kind = ANY ('{storage_cleanup}'::text[])))),
    CONSTRAINT preflights_pkey PRIMARY KEY (id),
    CONSTRAINT preflights_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id),
    CONSTRAINT preflights_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id)
);

CREATE TABLE secrets (
    id uuid NOT NULL,
    owner_kind text NOT NULL,
    owner_id uuid NOT NULL,
    name text NOT NULL,
    sealed bytea NOT NULL,
    created_by uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    rotated_at timestamp with time zone,
    CONSTRAINT secrets_owner_kind_check CHECK ((owner_kind = ANY (ARRAY['provider'::text, 'setting'::text, 'environment'::text, 'mcp_server'::text, 'destination'::text, 'notification_channel'::text, 'identity_provider'::text, 'skill'::text]))),
    CONSTRAINT secrets_owner_kind_owner_id_name_key UNIQUE (owner_kind, owner_id, name),
    CONSTRAINT secrets_pkey PRIMARY KEY (id),
    CONSTRAINT secrets_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id)
);

CREATE TABLE settings (
    node_id uuid,
    key text NOT NULL,
    value jsonb NOT NULL,
    locked boolean DEFAULT false NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    updated_by uuid,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT settings_node_id_key_key UNIQUE NULLS NOT DISTINCT (node_id, key),
    CONSTRAINT settings_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE,
    CONSTRAINT settings_updated_by_fkey FOREIGN KEY (updated_by) REFERENCES users(id)
);

CREATE INDEX settings_environment_default_idx ON settings USING btree (((value #>> '{}'::text[])), node_id) WHERE (key = 'env.default_id'::text);

CREATE INDEX settings_reference_facts_idx ON settings USING gin (value jsonb_path_ops);

CREATE TRIGGER agent_program_builds_write AFTER INSERT OR DELETE OR UPDATE ON settings FOR EACH ROW EXECUTE FUNCTION orch_project_program_build();
