-- Hosts, machines, runtimes, templates, environments, mounts, MCP servers, ports, terminals, displays, browsers and commands.

CREATE TABLE env_templates (
    id uuid NOT NULL,
    org_id uuid,
    home_id uuid,
    owner_id uuid,
    name text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    kind text DEFAULT 'environment'::text NOT NULL,
    driver text NOT NULL,
    spec jsonb NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    CONSTRAINT env_templates_kind_check CHECK ((kind = ANY (ARRAY['environment'::text, 'provider'::text]))),
    CONSTRAINT env_templates_name_check CHECK (((char_length(name) >= 1) AND (char_length(name) <= 80))),
    CONSTRAINT env_templates_pkey PRIMARY KEY (id),
    CONSTRAINT env_templates_home_id_fkey FOREIGN KEY (home_id) REFERENCES nodes(id),
    CONSTRAINT env_templates_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id),
    CONSTRAINT env_templates_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES users(id)
);

CREATE TRIGGER participation_env_templates_delete AFTER DELETE ON env_templates REFERENCING OLD TABLE AS participation_old_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER participation_env_templates_insert AFTER INSERT ON env_templates REFERENCING NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER participation_env_templates_update AFTER UPDATE ON env_templates REFERENCING OLD TABLE AS participation_old_objects NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TABLE mounts (
    id uuid NOT NULL,
    org_id uuid,
    home_id uuid,
    owner_id uuid,
    name text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    target text NOT NULL,
    read_only boolean DEFAULT true NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    default_fate text DEFAULT 'as_is'::text NOT NULL,
    CONSTRAINT mounts_default_fate_check CHECK ((default_fate = ANY (ARRAY['as_is'::text, 'detach'::text, 'copy'::text]))),
    CONSTRAINT mounts_name_check CHECK ((name ~ '^[a-z][a-z0-9_-]{1,31}$'::text)),
    CONSTRAINT mounts_org_id_name_key UNIQUE NULLS NOT DISTINCT (org_id, name),
    CONSTRAINT mounts_pkey PRIMARY KEY (id),
    CONSTRAINT mounts_home_id_fkey FOREIGN KEY (home_id) REFERENCES nodes(id),
    CONSTRAINT mounts_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id),
    CONSTRAINT mounts_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES users(id)
);

CREATE TRIGGER participation_mounts_delete AFTER DELETE ON mounts REFERENCING OLD TABLE AS participation_old_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER participation_mounts_insert AFTER INSERT ON mounts REFERENCING NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER participation_mounts_update AFTER UPDATE ON mounts REFERENCING OLD TABLE AS participation_old_objects NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TABLE browser_sessions (
    id uuid NOT NULL,
    environment_id uuid NOT NULL,
    env_generation integer NOT NULL,
    profile text,
    origin text NOT NULL,
    opened_by uuid,
    thread_id uuid,
    preset text,
    display_id uuid,
    endpoint text NOT NULL,
    program text,
    note text,
    state text DEFAULT 'opening'::text NOT NULL,
    close_reason text,
    opened_at timestamp with time zone DEFAULT now() NOT NULL,
    last_used_at timestamp with time zone DEFAULT now() NOT NULL,
    closed_at timestamp with time zone,
    CONSTRAINT browser_sessions_close_reason_check CHECK ((close_reason = ANY (ARRAY['quit'::text, 'closed_by_agent'::text, 'idle'::text, 'exited'::text, 'environment_stopped'::text, 'link_lost'::text, 'switched_off'::text, 'error'::text]))),
    CONSTRAINT browser_sessions_origin_check CHECK ((origin = ANY (ARRAY['started'::text, 'announced'::text]))),
    CONSTRAINT browser_sessions_preset_check CHECK ((preset = ANY (ARRAY['headless'::text, 'window'::text]))),
    CONSTRAINT browser_sessions_state_check CHECK ((state = ANY (ARRAY['opening'::text, 'open'::text, 'closed'::text]))),
    CONSTRAINT browser_sessions_pkey PRIMARY KEY (id),
    CONSTRAINT browser_sessions_opened_by_fkey FOREIGN KEY (opened_by) REFERENCES users(id)
);

CREATE INDEX browser_sessions_open_idx ON browser_sessions USING btree (environment_id) WHERE (state <> 'closed'::text);

CREATE UNIQUE INDEX browser_sessions_profile_key ON browser_sessions USING btree (environment_id, lower(profile)) WHERE ((state <> 'closed'::text) AND (profile IS NOT NULL));

CREATE TABLE commands (
    id uuid NOT NULL,
    machine_id uuid NOT NULL,
    kind text NOT NULL,
    body jsonb NOT NULL,
    serial_key text,
    state text DEFAULT 'queued'::text NOT NULL,
    result jsonb,
    error jsonb,
    attempts integer DEFAULT 0 NOT NULL,
    deadline_at timestamp with time zone NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    stop_confirmed_at timestamp with time zone,
    session_fact_revision bigint,
    CONSTRAINT commands_state_check CHECK ((state = ANY (ARRAY['queued'::text, 'sent'::text, 'accepted'::text, 'succeeded'::text, 'failed'::text, 'expired'::text, 'cancelled'::text]))),
    CONSTRAINT commands_pkey PRIMARY KEY (id)
);

CREATE INDEX commands_busy_detach_idx ON commands USING btree (updated_at, id) WHERE ((kind = 'env.detach'::text) AND (state = 'failed'::text));

CREATE INDEX commands_open_idx ON commands USING btree (machine_id) WHERE (state = ANY (ARRAY['queued'::text, 'sent'::text, 'accepted'::text]));

CREATE INDEX commands_open_thread_idx ON commands USING btree (((body ->> 'thread_id'::text))) WHERE (state = ANY (ARRAY['queued'::text, 'sent'::text, 'accepted'::text]));

CREATE INDEX commands_provider_active_idx ON commands USING btree (((body ->> 'provider_id'::text)), ((body ->> 'model_name'::text))) WHERE ((kind = ANY (ARRAY['provider.complete'::text, 'provider.embed'::text])) AND (state = ANY (ARRAY['queued'::text, 'sent'::text, 'accepted'::text])));

CREATE INDEX commands_provider_purge_retry_idx ON commands USING btree (updated_at, id) WHERE ((kind = 'provider.stop'::text) AND ((body ->> 'purge'::text) = 'true'::text) AND (state = ANY (ARRAY['failed'::text, 'expired'::text, 'cancelled'::text])));

CREATE INDEX commands_runtime_create_idx ON commands USING btree (((body ->> 'runtime_id'::text)), created_at DESC, id DESC) WHERE (kind = 'node.runtime.create'::text);

CREATE INDEX commands_tool_retirement_receipt ON commands USING btree (machine_id, ((body ->> 'runtime_id'::text))) WHERE ((kind = 'node.runtime.remove'::text) AND (state = 'succeeded'::text));

CREATE TRIGGER agent_history_closed AFTER UPDATE OF state ON commands FOR EACH ROW WHEN (((new.state = 'succeeded'::text) AND (old.state <> 'succeeded'::text) AND (new.kind = ANY (ARRAY['session.close'::text, 'session.close.fenced'::text, 'session.continuation.close'::text])))) EXECUTE FUNCTION agent_history_closed();

CREATE TRIGGER history_native_receipt AFTER UPDATE OF state, result, error ON commands FOR EACH ROW EXECUTE FUNCTION thread_history_native_receipt();

CREATE TABLE display_sessions (
    id uuid NOT NULL,
    environment_id uuid NOT NULL,
    env_generation integer NOT NULL,
    name text NOT NULL,
    origin text NOT NULL,
    opened_by uuid,
    thread_id uuid,
    endpoint text NOT NULL,
    x_display text,
    geometry text,
    note text,
    password bytea,
    state text DEFAULT 'opening'::text NOT NULL,
    close_reason text,
    opened_at timestamp with time zone DEFAULT now() NOT NULL,
    last_used_at timestamp with time zone DEFAULT now() NOT NULL,
    closed_at timestamp with time zone,
    CONSTRAINT display_sessions_close_reason_check CHECK ((close_reason = ANY (ARRAY['closed_by_user'::text, 'closed_by_agent'::text, 'exited'::text, 'environment_stopped'::text, 'link_lost'::text, 'switched_off'::text, 'error'::text]))),
    CONSTRAINT display_sessions_origin_check CHECK ((origin = ANY (ARRAY['started'::text, 'announced'::text]))),
    CONSTRAINT display_sessions_state_check CHECK ((state = ANY (ARRAY['opening'::text, 'open'::text, 'closed'::text]))),
    CONSTRAINT display_sessions_pkey PRIMARY KEY (id),
    CONSTRAINT display_sessions_opened_by_fkey FOREIGN KEY (opened_by) REFERENCES users(id)
);

CREATE INDEX display_sessions_open_idx ON display_sessions USING btree (environment_id) WHERE (state <> 'closed'::text);

ALTER TABLE browser_sessions ADD CONSTRAINT browser_sessions_display_id_fkey FOREIGN KEY (display_id) REFERENCES display_sessions(id);

CREATE TABLE environment_lifetimes (
    environment_id uuid NOT NULL,
    thread_id uuid,
    idle_hours bigint NOT NULL,
    activity_at timestamp with time zone NOT NULL,
    archive_at timestamp with time zone,
    expires_at timestamp with time zone NOT NULL,
    warned_at timestamp with time zone,
    warned_owner_id uuid,
    checked_at timestamp with time zone NOT NULL,
    CONSTRAINT environment_lifetimes_idle_hours_check CHECK ((idle_hours > 0)),
    CONSTRAINT environment_lifetimes_pkey PRIMARY KEY (environment_id),
    CONSTRAINT environment_lifetimes_warned_owner_id_fkey FOREIGN KEY (warned_owner_id) REFERENCES users(id) ON DELETE SET NULL
);

CREATE INDEX environment_lifetimes_checked_idx ON environment_lifetimes USING btree (checked_at, environment_id);

CREATE TABLE environments (
    id uuid NOT NULL,
    name text NOT NULL,
    owner_id uuid NOT NULL,
    org_id uuid NOT NULL,
    placement text NOT NULL,
    home_node_id uuid NOT NULL,
    workspace_root text DEFAULT '/workspace'::text NOT NULL,
    config jsonb DEFAULT '{}'::jsonb NOT NULL,
    machine_id uuid,
    template_id uuid,
    host_entry_id uuid,
    template_entry_id uuid,
    generation integer DEFAULT 0 NOT NULL,
    data_generation bigint DEFAULT 0 NOT NULL,
    remote_write boolean,
    terminal boolean,
    browser boolean,
    vnc boolean,
    config_trust jsonb DEFAULT '{}'::jsonb NOT NULL,
    last_activity_at timestamp with time zone,
    granted_capabilities text[] DEFAULT '{}'::text[] NOT NULL,
    status text DEFAULT 'none'::text NOT NULL,
    status_detail text,
    facts jsonb DEFAULT '{}'::jsonb NOT NULL,
    manifest_blob bytea,
    manifest_at timestamp with time zone,
    env_seq bigint DEFAULT 0 NOT NULL,
    unused_since timestamp with time zone,
    remove_at timestamp with time zone,
    attached_session timestamp with time zone,
    revision bigint DEFAULT 1 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    deleted_by uuid,
    removed_keep_days integer,
    volumes_purge_at timestamp with time zone,
    retained_volumes jsonb DEFAULT '[]'::jsonb NOT NULL,
    retained_bytes bigint,
    retained_size_at timestamp with time zone,
    removal_phase text,
    removal_revision bigint DEFAULT 0 NOT NULL,
    purged_at timestamp with time zone,
    lost_at timestamp with time zone,
    lost_by uuid,
    loss_missing boolean DEFAULT false NOT NULL,
    CONSTRAINT environments_check CHECK (((placement <> 'managed'::text) OR (template_id IS NOT NULL))),
    CONSTRAINT environments_placement_check CHECK ((placement = ANY (ARRAY['managed'::text, 'connected'::text]))),
    CONSTRAINT environments_removal_phase_check CHECK (((removal_phase = ANY (ARRAY['scheduled'::text, 'removing'::text, 'kept'::text, 'restoring'::text, 'purging'::text, 'purged'::text])) OR (removal_phase = ANY ('{lost}'::text[])))),
    CONSTRAINT environments_removed_keep_days_check CHECK ((removed_keep_days >= 0)),
    CONSTRAINT environments_status_check CHECK (((status = ANY (ARRAY['none'::text, 'provisioning'::text, 'running'::text, 'stopped'::text, 'recreating'::text, 'moving'::text, 'failed'::text, 'offline'::text, 'disabled'::text, 'removed'::text])) OR (status = ANY ('{lost}'::text[])))),
    CONSTRAINT environments_pkey PRIMARY KEY (id),
    CONSTRAINT environments_deleted_by_fkey FOREIGN KEY (deleted_by) REFERENCES users(id),
    CONSTRAINT environments_home_node_id_fkey FOREIGN KEY (home_node_id) REFERENCES nodes(id),
    CONSTRAINT environments_host_entry_id_fkey FOREIGN KEY (host_entry_id) REFERENCES permission_entries(id),
    CONSTRAINT environments_lost_by_fkey FOREIGN KEY (lost_by) REFERENCES users(id),
    CONSTRAINT environments_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id),
    CONSTRAINT environments_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES users(id),
    CONSTRAINT environments_template_entry_id_fkey FOREIGN KEY (template_entry_id) REFERENCES permission_entries(id),
    CONSTRAINT environments_template_id_fkey FOREIGN KEY (template_id) REFERENCES env_templates(id)
);

CREATE INDEX environment_config_facts_idx ON environments USING gin (config jsonb_path_ops);

CREATE INDEX environment_mcp_membership_idx ON environments USING gin (((facts -> 'mcp_servers'::text)) jsonb_path_ops);

CREATE INDEX environments_connected_idx ON environments USING btree (machine_id) WHERE ((placement = 'connected'::text) AND (deleted_at IS NULL));

CREATE INDEX environments_home_idx ON environments USING btree (home_node_id) WHERE (deleted_at IS NULL);

CREATE INDEX environments_kept_idx ON environments USING btree (home_node_id, volumes_purge_at) WHERE (removal_phase = 'kept'::text);

CREATE UNIQUE INDEX environments_owner_name_key ON environments USING btree (org_id, owner_id, lower(name)) WHERE (deleted_at IS NULL);

CREATE INDEX environments_removal_due_idx ON environments USING btree (remove_at, id) WHERE ((deleted_at IS NULL) AND (remove_at IS NOT NULL));

CREATE INDEX environments_retained_host_facts_idx ON environments USING gin (retained_volumes jsonb_path_ops);

CREATE INDEX environments_temporary_idx ON environments USING btree (id) WHERE ((deleted_at IS NULL) AND ((config ->> 'env.lifetime'::text) = ANY (ARRAY['temporary'::text, 'thread'::text])));

CREATE TRIGGER mcp_offerings_discovered AFTER INSERT OR UPDATE OF facts, machine_id ON environments FOR EACH ROW EXECUTE FUNCTION orch_mcp_offerings_discovered();

CREATE TRIGGER participation_environments_delete AFTER DELETE ON environments REFERENCING OLD TABLE AS participation_old_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER participation_environments_insert AFTER INSERT ON environments REFERENCING NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER participation_environments_update AFTER UPDATE ON environments REFERENCING OLD TABLE AS participation_old_objects NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

ALTER TABLE agent_shares ADD CONSTRAINT agent_shares_environment_id_fkey FOREIGN KEY (environment_id) REFERENCES environments(id);

ALTER TABLE browser_sessions ADD CONSTRAINT browser_sessions_environment_id_fkey FOREIGN KEY (environment_id) REFERENCES environments(id);

ALTER TABLE display_sessions ADD CONSTRAINT display_sessions_environment_id_fkey FOREIGN KEY (environment_id) REFERENCES environments(id);

ALTER TABLE environment_lifetimes ADD CONSTRAINT environment_lifetimes_environment_id_fkey FOREIGN KEY (environment_id) REFERENCES environments(id) ON DELETE CASCADE;

ALTER TABLE file_cache ADD CONSTRAINT file_cache_environment_id_fkey FOREIGN KEY (environment_id) REFERENCES environments(id) ON DELETE CASCADE;

ALTER TABLE jobs ADD CONSTRAINT jobs_environment_id_fkey FOREIGN KEY (environment_id) REFERENCES environments(id);

ALTER TABLE preflights ADD CONSTRAINT preflights_environment_id_fkey FOREIGN KEY (environment_id) REFERENCES environments(id);

ALTER TABLE providers ADD CONSTRAINT providers_holder_fk FOREIGN KEY (held_by_environment_id) REFERENCES environments(id);

ALTER TABLE snapshots ADD CONSTRAINT snapshots_environment_id_fkey FOREIGN KEY (environment_id) REFERENCES environments(id);

CREATE TABLE hosts (
    id uuid NOT NULL,
    machine_id uuid NOT NULL,
    owner_id uuid,
    org_id uuid,
    home_id uuid,
    name text NOT NULL,
    labels text[] DEFAULT '{}'::text[] NOT NULL,
    ports jsonb,
    ports_revision bigint DEFAULT 0 NOT NULL,
    drivers jsonb DEFAULT '{}'::jsonb NOT NULL,
    kept_images jsonb DEFAULT '{"epoch": 0, "images": []}'::jsonb NOT NULL,
    capacity jsonb DEFAULT '{}'::jsonb NOT NULL,
    load jsonb DEFAULT '{}'::jsonb NOT NULL,
    load_at timestamp with time zone,
    storages jsonb DEFAULT '[]'::jsonb NOT NULL,
    schedulable boolean DEFAULT true NOT NULL,
    disabled boolean DEFAULT false NOT NULL,
    when_full text,
    revision bigint DEFAULT 1 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    cleanup_keep_days bigint,
    resource_inventory jsonb,
    driver_configuration_schemas jsonb DEFAULT '{}'::jsonb NOT NULL,
    CONSTRAINT hosts_cleanup_keep_days_check CHECK ((cleanup_keep_days >= 0)),
    CONSTRAINT hosts_driver_configuration_schemas_check CHECK ((jsonb_typeof(driver_configuration_schemas) = 'object'::text)),
    CONSTRAINT hosts_when_full_check CHECK ((when_full = ANY (ARRAY['warn'::text, 'block'::text]))),
    CONSTRAINT hosts_machine_id_key UNIQUE (machine_id),
    CONSTRAINT hosts_pkey PRIMARY KEY (id),
    CONSTRAINT hosts_home_id_fkey FOREIGN KEY (home_id) REFERENCES nodes(id),
    CONSTRAINT hosts_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id),
    CONSTRAINT hosts_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES users(id)
);

CREATE TRIGGER participation_hosts_delete AFTER DELETE ON hosts REFERENCING OLD TABLE AS participation_old_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER participation_hosts_insert AFTER INSERT ON hosts REFERENCING NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER participation_hosts_update AFTER UPDATE ON hosts REFERENCING OLD TABLE AS participation_old_objects NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TABLE machines (
    id uuid NOT NULL,
    name text NOT NULL,
    owner_id uuid,
    home_id uuid,
    org_id uuid,
    revision bigint DEFAULT 1 NOT NULL,
    placement text NOT NULL,
    host_id uuid,
    runtime_id uuid,
    roles text[] DEFAULT '{}'::text[] NOT NULL,
    platform_os text NOT NULL,
    platform_arch text NOT NULL,
    connector_version text NOT NULL,
    protocol_version integer NOT NULL,
    cert_serial text NOT NULL,
    cert_fingerprint bytea NOT NULL,
    cert_not_after timestamp with time zone NOT NULL,
    cert_public_key text,
    cert_next_fingerprint text,
    cert_next_public_key text,
    cert_next_serial text,
    cert_next_not_after timestamp with time zone,
    cert_next_again boolean,
    status text DEFAULT 'active'::text NOT NULL,
    online boolean DEFAULT false NOT NULL,
    current_session_id uuid,
    session_started_at timestamp with time zone,
    offline_since timestamp with time zone,
    last_seen_at timestamp with time zone,
    last_ip inet,
    rate_up_bps bigint,
    rate_down_bps bigint,
    facts jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    lost_at timestamp with time zone,
    lost_by uuid,
    CONSTRAINT machines_lost_revoked CHECK (((lost_at IS NULL) OR (status = 'revoked'::text))),
    CONSTRAINT machines_name_check CHECK (((char_length(name) >= 1) AND (char_length(name) <= 80))),
    CONSTRAINT machines_placement_check CHECK ((placement = ANY (ARRAY['connected'::text, 'managed'::text]))),
    CONSTRAINT machines_platform_arch_check CHECK ((platform_arch = ANY (ARRAY['x86_64'::text, 'aarch64'::text]))),
    CONSTRAINT machines_platform_os_check CHECK ((platform_os = ANY (ARRAY['linux'::text, 'macos'::text, 'windows'::text]))),
    CONSTRAINT machines_roles_check CHECK ((roles <@ ARRAY['provider'::text, 'agent'::text, 'environment'::text, 'node'::text])),
    CONSTRAINT machines_status_check CHECK ((status = ANY (ARRAY['active'::text, 'revoked'::text]))),
    CONSTRAINT machines_cert_fingerprint_key UNIQUE (cert_fingerprint),
    CONSTRAINT machines_pkey PRIMARY KEY (id),
    CONSTRAINT machines_home_id_fkey FOREIGN KEY (home_id) REFERENCES nodes(id),
    CONSTRAINT machines_host_fk FOREIGN KEY (host_id) REFERENCES hosts(id),
    CONSTRAINT machines_lost_by_fkey FOREIGN KEY (lost_by) REFERENCES users(id),
    CONSTRAINT machines_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id),
    CONSTRAINT machines_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES users(id)
);

CREATE TRIGGER agent_program_reports_write AFTER INSERT OR UPDATE OF facts, status, deleted_at ON machines FOR EACH ROW EXECUTE FUNCTION orch_project_program_reports();

CREATE TRIGGER machine_resources_unavailable AFTER UPDATE OF lost_at, deleted_at ON machines FOR EACH ROW WHEN (((new.lost_at IS DISTINCT FROM old.lost_at) OR (new.deleted_at IS DISTINCT FROM old.deleted_at))) EXECUTE FUNCTION orch_machine_resources_unavailable();

CREATE TRIGGER participation_machines_delete AFTER DELETE ON machines REFERENCING OLD TABLE AS participation_old_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER participation_machines_insert AFTER INSERT ON machines REFERENCING NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER participation_machines_update AFTER UPDATE ON machines REFERENCING OLD TABLE AS participation_old_objects NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

ALTER TABLE agent_program_reports ADD CONSTRAINT agent_program_reports_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES machines(id) ON DELETE CASCADE;

ALTER TABLE agents ADD CONSTRAINT agents_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES machines(id);

ALTER TABLE blob_uploads ADD CONSTRAINT blob_uploads_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES machines(id);

ALTER TABLE commands ADD CONSTRAINT commands_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES machines(id) ON DELETE CASCADE;

ALTER TABLE component_updates ADD CONSTRAINT component_updates_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES machines(id);

ALTER TABLE enrollment_tokens ADD CONSTRAINT enrollment_tokens_again_machine_id_fkey FOREIGN KEY (again_machine_id) REFERENCES machines(id);

ALTER TABLE enrollment_tokens ADD CONSTRAINT enrollment_tokens_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES machines(id);

ALTER TABLE environments ADD CONSTRAINT environments_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES machines(id);

ALTER TABLE harnesses ADD CONSTRAINT harnesses_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES machines(id);

ALTER TABLE hosts ADD CONSTRAINT hosts_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES machines(id);

ALTER TABLE leases ADD CONSTRAINT leases_recipient_machine_id_fkey FOREIGN KEY (recipient_machine_id) REFERENCES machines(id);

ALTER TABLE providers ADD CONSTRAINT providers_key_machine_id_fkey FOREIGN KEY (key_machine_id) REFERENCES machines(id);

ALTER TABLE providers ADD CONSTRAINT providers_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES machines(id);

CREATE TABLE mcp_server_offerings (
    server_id uuid NOT NULL,
    machine_id uuid NOT NULL,
    environment_id uuid,
    state text NOT NULL,
    source jsonb NOT NULL,
    proposal jsonb,
    offered_at timestamp with time zone,
    seen_at timestamp with time zone NOT NULL,
    lost_at timestamp with time zone,
    reason text,
    job_id uuid,
    CONSTRAINT mcp_server_offerings_state_check CHECK ((state = ANY (ARRAY['found'::text, 'pending'::text, 'ready'::text, 'failed'::text, 'lost'::text]))),
    CONSTRAINT mcp_server_offerings_pkey PRIMARY KEY (server_id, machine_id),
    CONSTRAINT mcp_server_offerings_environment_id_fkey FOREIGN KEY (environment_id) REFERENCES environments(id) ON DELETE SET NULL,
    CONSTRAINT mcp_server_offerings_job_id_fkey FOREIGN KEY (job_id) REFERENCES jobs(id) ON DELETE SET NULL,
    CONSTRAINT mcp_server_offerings_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES machines(id) ON DELETE CASCADE
);

CREATE INDEX mcp_offerings_machine_idx ON mcp_server_offerings USING btree (machine_id, state);

CREATE TABLE mcp_servers (
    id uuid NOT NULL,
    org_id uuid,
    home_id uuid,
    owner_id uuid,
    machine_id uuid,
    environment_id uuid,
    name text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    spec jsonb NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    imported boolean DEFAULT false NOT NULL,
    CONSTRAINT mcp_servers_check CHECK (((machine_id IS NULL) OR (environment_id IS NULL))),
    CONSTRAINT mcp_servers_name_check CHECK ((name ~ '^[a-z][a-z0-9_-]{1,31}$'::text)),
    CONSTRAINT mcp_servers_pkey PRIMARY KEY (id),
    CONSTRAINT mcp_servers_environment_id_fkey FOREIGN KEY (environment_id) REFERENCES environments(id) ON DELETE CASCADE,
    CONSTRAINT mcp_servers_home_id_fkey FOREIGN KEY (home_id) REFERENCES nodes(id),
    CONSTRAINT mcp_servers_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES machines(id) ON DELETE SET NULL,
    CONSTRAINT mcp_servers_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id),
    CONSTRAINT mcp_servers_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES users(id)
);

CREATE INDEX mcp_servers_import_name_idx ON mcp_servers USING btree (home_id, name) WHERE (imported AND (deleted_at IS NULL));

CREATE UNIQUE INDEX mcp_servers_preset_name_idx ON mcp_servers USING btree (org_id, environment_id, name) NULLS NOT DISTINCT WHERE (NOT imported);

CREATE TRIGGER participation_mcp_servers_delete AFTER DELETE ON mcp_servers REFERENCING OLD TABLE AS participation_old_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER participation_mcp_servers_insert AFTER INSERT ON mcp_servers REFERENCING NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER participation_mcp_servers_update AFTER UPDATE ON mcp_servers REFERENCING OLD TABLE AS participation_old_objects NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

ALTER TABLE mcp_server_offerings ADD CONSTRAINT mcp_server_offerings_server_id_fkey FOREIGN KEY (server_id) REFERENCES mcp_servers(id) ON DELETE CASCADE;

CREATE TABLE mount_kept_data (
    id uuid NOT NULL,
    mount_id uuid NOT NULL,
    machine_id uuid NOT NULL,
    kind text NOT NULL,
    source text NOT NULL,
    storage text,
    data_kind text,
    file_count bigint,
    total_bytes bigint,
    job_id uuid,
    kept_by uuid NOT NULL,
    kept_at timestamp with time zone DEFAULT now() NOT NULL,
    lost_at timestamp with time zone,
    CONSTRAINT mount_kept_data_data_kind_check CHECK ((data_kind = ANY (ARRAY['own'::text, 'shared'::text, 'synced'::text]))),
    CONSTRAINT mount_kept_data_kind_check CHECK ((kind = ANY (ARRAY['path'::text, 'volume'::text]))),
    CONSTRAINT mount_kept_data_machine_id_kind_source_key UNIQUE (machine_id, kind, source),
    CONSTRAINT mount_kept_data_pkey PRIMARY KEY (id),
    CONSTRAINT mount_kept_data_job_id_fkey FOREIGN KEY (job_id) REFERENCES jobs(id) ON DELETE SET NULL,
    CONSTRAINT mount_kept_data_kept_by_fkey FOREIGN KEY (kept_by) REFERENCES users(id),
    CONSTRAINT mount_kept_data_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES machines(id) ON DELETE CASCADE,
    CONSTRAINT mount_kept_data_mount_id_fkey FOREIGN KEY (mount_id) REFERENCES mounts(id) ON DELETE CASCADE
);

CREATE INDEX mount_kept_data_mount_idx ON mount_kept_data USING btree (mount_id, machine_id);

CREATE TABLE mount_locations (
    mount_id uuid NOT NULL,
    machine_id uuid NOT NULL,
    kind text NOT NULL,
    source text NOT NULL,
    storage text,
    created_by uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    data_kind text,
    last_bytes bigint,
    size_at timestamp with time zone,
    CONSTRAINT mount_locations_data_kind_check CHECK ((data_kind = ANY (ARRAY['own'::text, 'shared'::text, 'synced'::text]))),
    CONSTRAINT mount_locations_kind_check CHECK ((kind = ANY (ARRAY['path'::text, 'volume'::text]))),
    CONSTRAINT mount_locations_pkey PRIMARY KEY (mount_id, machine_id),
    CONSTRAINT mount_locations_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id),
    CONSTRAINT mount_locations_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES machines(id) ON DELETE CASCADE,
    CONSTRAINT mount_locations_mount_id_fkey FOREIGN KEY (mount_id) REFERENCES mounts(id) ON DELETE CASCADE
);

CREATE TABLE mount_lost_locations (
    id uuid NOT NULL,
    mount_id uuid NOT NULL,
    machine_id uuid,
    machine_name text NOT NULL,
    host_name text,
    storage text,
    total_bytes bigint,
    size_at timestamp with time zone,
    lost_at timestamp with time zone NOT NULL,
    CONSTRAINT mount_lost_locations_pkey PRIMARY KEY (id),
    CONSTRAINT mount_lost_locations_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES machines(id) ON DELETE SET NULL,
    CONSTRAINT mount_lost_locations_mount_id_fkey FOREIGN KEY (mount_id) REFERENCES mounts(id) ON DELETE CASCADE
);

CREATE INDEX mount_lost_locations_mount_idx ON mount_lost_locations USING btree (mount_id, lost_at);

CREATE TABLE port_link_uses (
    opening_id uuid NOT NULL,
    day date NOT NULL,
    CONSTRAINT port_link_uses_pkey PRIMARY KEY (opening_id, day)
);

CREATE TABLE port_open_requests (
    environment_id uuid NOT NULL,
    user_id uuid,
    thread_id uuid,
    client_id uuid NOT NULL,
    request_hash bytea NOT NULL,
    effect_hash bytea NOT NULL,
    opening_id uuid NOT NULL,
    refused_until timestamp with time zone,
    secret_response_taken boolean DEFAULT false NOT NULL,
    outcome_delivered boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT port_open_requests_check CHECK (((user_id IS NULL) <> (thread_id IS NULL))),
    CONSTRAINT port_open_requests_environment_id_fkey FOREIGN KEY (environment_id) REFERENCES environments(id) ON DELETE CASCADE,
    CONSTRAINT port_open_requests_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id)
);

CREATE UNIQUE INDEX port_open_requests_thread_idx ON port_open_requests USING btree (environment_id, thread_id, client_id) WHERE (thread_id IS NOT NULL);

CREATE UNIQUE INDEX port_open_requests_user_idx ON port_open_requests USING btree (environment_id, user_id, client_id) WHERE (user_id IS NOT NULL);

CREATE TRIGGER port_open_requests_live AFTER INSERT OR DELETE OR UPDATE ON port_open_requests FOR EACH ROW EXECUTE FUNCTION machine_views_notify();

CREATE TABLE port_openings (
    id uuid NOT NULL,
    environment_id uuid NOT NULL,
    port integer NOT NULL,
    kind text NOT NULL,
    transports text[] DEFAULT '{tcp,udp}'::text[] NOT NULL,
    entry_methods text[] NOT NULL,
    place_id uuid,
    label text NOT NULL,
    requested_by uuid,
    requested_by_thread uuid,
    allowed_by uuid,
    decider_credential_kind text,
    decider_credential_id uuid,
    decider_as_admin boolean DEFAULT false NOT NULL,
    admission jsonb,
    opened_at timestamp with time zone NOT NULL,
    until_at timestamp with time zone,
    state text NOT NULL,
    reason text,
    secret_hash bytea,
    secret_sealed bytea,
    link_recipient text,
    link_recipient_user uuid,
    approval_id uuid,
    host_id uuid,
    publish_machine_id uuid,
    host_port integer,
    fixed_host_port boolean DEFAULT false NOT NULL,
    host_allocated boolean DEFAULT false NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    closed_at timestamp with time zone,
    CONSTRAINT port_openings_host_port_check CHECK (((host_port >= 1) AND (host_port <= 65535))),
    CONSTRAINT port_openings_decider_credential_check CHECK (((decider_credential_kind IS NULL AND decider_credential_id IS NULL AND NOT decider_as_admin) OR (decider_credential_kind IS NOT NULL AND decider_credential_kind IN ('session', 'token') AND decider_credential_id IS NOT NULL AND (NOT decider_as_admin OR decider_credential_kind = 'session')))),
    CONSTRAINT port_openings_kind_check CHECK ((kind = ANY (ARRAY['place'::text, 'link'::text, 'host'::text]))),
    CONSTRAINT port_openings_link_recipient_check CHECK ((link_recipient = ANY (ARRAY['thread'::text, 'person'::text]))),
    CONSTRAINT port_openings_port_check CHECK (((port >= 1) AND (port <= 65535))),
    CONSTRAINT port_openings_state_check CHECK ((state = ANY (ARRAY['waiting'::text, 'opening'::text, 'open'::text, 'stopped'::text, 'not_listening'::text, 'unavailable'::text, 'refused'::text, 'closed'::text]))),
    CONSTRAINT port_openings_pkey PRIMARY KEY (id),
    CONSTRAINT port_openings_allowed_by_fkey FOREIGN KEY (allowed_by) REFERENCES users(id),
    CONSTRAINT port_openings_environment_id_fkey FOREIGN KEY (environment_id) REFERENCES environments(id) ON DELETE CASCADE,
    CONSTRAINT port_openings_host_id_fkey FOREIGN KEY (host_id) REFERENCES hosts(id),
    CONSTRAINT port_openings_link_recipient_user_fkey FOREIGN KEY (link_recipient_user) REFERENCES users(id),
    CONSTRAINT port_openings_place_id_fkey FOREIGN KEY (place_id) REFERENCES nodes(id) ON DELETE SET NULL,
    CONSTRAINT port_openings_publish_machine_id_fkey FOREIGN KEY (publish_machine_id) REFERENCES machines(id),
    CONSTRAINT port_openings_requested_by_fkey FOREIGN KEY (requested_by) REFERENCES users(id)
);

CREATE INDEX port_openings_environment_idx ON port_openings USING btree (environment_id, state);

CREATE INDEX port_openings_expiry_idx ON port_openings USING btree (until_at) WHERE ((kind = 'link'::text) AND (state <> 'closed'::text));

CREATE UNIQUE INDEX port_openings_host_idx ON port_openings USING btree (publish_machine_id, host_port) WHERE ((kind = 'host'::text) AND host_allocated);

CREATE INDEX port_openings_place_idx ON port_openings USING btree (place_id) WHERE (kind = 'place'::text);

CREATE TRIGGER port_openings_live AFTER INSERT OR DELETE OR UPDATE ON port_openings FOR EACH ROW EXECUTE FUNCTION machine_views_notify();

CREATE TRIGGER port_openings_publisher BEFORE INSERT OR UPDATE OF host_id, publish_machine_id, host_allocated ON port_openings FOR EACH ROW EXECUTE FUNCTION port_opening_publisher();

ALTER TABLE port_link_uses ADD CONSTRAINT port_link_uses_opening_id_fkey FOREIGN KEY (opening_id) REFERENCES port_openings(id) ON DELETE CASCADE;

ALTER TABLE port_open_requests ADD CONSTRAINT port_open_requests_opening_id_fkey FOREIGN KEY (opening_id) REFERENCES port_openings(id) ON DELETE CASCADE;

CREATE TABLE port_targets (
    environment_id uuid NOT NULL,
    port integer NOT NULL,
    http_scheme text DEFAULT 'http'::text NOT NULL,
    host_header text,
    proxy_protocol boolean DEFAULT false NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    CONSTRAINT port_targets_http_scheme_check CHECK ((http_scheme = ANY (ARRAY['http'::text, 'https'::text]))),
    CONSTRAINT port_targets_port_check CHECK (((port >= 1) AND (port <= 65535))),
    CONSTRAINT port_targets_pkey PRIMARY KEY (environment_id, port),
    CONSTRAINT port_targets_environment_id_fkey FOREIGN KEY (environment_id) REFERENCES environments(id) ON DELETE CASCADE
);

CREATE TRIGGER port_targets_live AFTER INSERT OR DELETE OR UPDATE ON port_targets FOR EACH ROW EXECUTE FUNCTION machine_views_notify();

CREATE TABLE runtimes (
    id uuid NOT NULL,
    network_required text,
    network_state jsonb,
    kind text NOT NULL,
    host_id uuid NOT NULL,
    machine_id uuid,
    unit_name text NOT NULL,
    successor_of uuid,
    provider_id uuid,
    environment_id uuid,
    driver text NOT NULL,
    template_id uuid NOT NULL,
    spec_hash bytea NOT NULL,
    generation integer DEFAULT 1 NOT NULL,
    state text NOT NULL,
    state_detail text,
    remove_at timestamp with time zone,
    volumes jsonb DEFAULT '{}'::jsonb NOT NULL,
    kept_image jsonb,
    last_active_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    lost_at timestamp with time zone,
    loss_missing boolean DEFAULT false NOT NULL,
    resources_used jsonb,
    CONSTRAINT runtimes_check CHECK (((kind = 'provider'::text) = (provider_id IS NOT NULL))),
    CONSTRAINT runtimes_check1 CHECK (((kind = 'environment'::text) = (environment_id IS NOT NULL))),
    CONSTRAINT runtimes_kind_check CHECK ((kind = ANY (ARRAY['provider'::text, 'environment'::text]))),
    CONSTRAINT runtimes_network_required_check CHECK ((network_required = ANY (ARRAY['open'::text, 'restricted'::text]))),
    CONSTRAINT runtimes_state_check CHECK ((state = ANY (ARRAY['creating'::text, 'running'::text, 'stopping'::text, 'stopped'::text, 'failed'::text, 'retired'::text, 'removed'::text]))),
    CONSTRAINT runtimes_pkey PRIMARY KEY (id),
    CONSTRAINT runtimes_unit_name_key UNIQUE (unit_name),
    CONSTRAINT runtimes_environment_id_fkey FOREIGN KEY (environment_id) REFERENCES environments(id),
    CONSTRAINT runtimes_host_id_fkey FOREIGN KEY (host_id) REFERENCES hosts(id),
    CONSTRAINT runtimes_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES machines(id),
    CONSTRAINT runtimes_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES providers(id),
    CONSTRAINT runtimes_successor_of_fkey FOREIGN KEY (successor_of) REFERENCES runtimes(id),
    CONSTRAINT runtimes_template_id_fkey FOREIGN KEY (template_id) REFERENCES env_templates(id)
);

CREATE INDEX runtimes_custody_waiting_idx ON runtimes USING btree (host_id) INCLUDE (state, remove_at) WHERE ((lost_at IS NULL) AND (state <> 'removed'::text));

CREATE UNIQUE INDEX runtimes_env_key ON runtimes USING btree (environment_id) WHERE ((kind = 'environment'::text) AND (state <> ALL (ARRAY['retired'::text, 'removed'::text])) AND (successor_of IS NULL));

CREATE INDEX runtimes_mount_host_idx ON runtimes USING btree (host_id, environment_id, machine_id) WHERE ((environment_id IS NOT NULL) AND (successor_of IS NULL) AND (state <> ALL (ARRAY['retired'::text, 'removed'::text])));

CREATE UNIQUE INDEX runtimes_provider_key ON runtimes USING btree (provider_id) WHERE ((kind = 'provider'::text) AND (state <> ALL (ARRAY['retired'::text, 'removed'::text])) AND (successor_of IS NULL));

CREATE INDEX runtimes_resource_facts_idx ON runtimes USING gin (resources_used jsonb_path_ops);

CREATE INDEX runtimes_volume_facts_idx ON runtimes USING gin (volumes);

CREATE TABLE terminal_sessions (
    id uuid NOT NULL,
    thread_id uuid,
    environment_id uuid NOT NULL,
    env_generation integer NOT NULL,
    opened_by uuid NOT NULL,
    shell text NOT NULL,
    cwd text NOT NULL,
    cols integer NOT NULL,
    rows integer NOT NULL,
    state text DEFAULT 'opening'::text NOT NULL,
    attached boolean DEFAULT false NOT NULL,
    detached_at timestamp with time zone,
    close_reason text,
    exit_code integer,
    bytes_in bigint DEFAULT 0 NOT NULL,
    bytes_out bigint DEFAULT 0 NOT NULL,
    recording_blob bytea,
    opened_at timestamp with time zone DEFAULT now() NOT NULL,
    last_io_at timestamp with time zone DEFAULT now() NOT NULL,
    closed_at timestamp with time zone,
    CONSTRAINT terminal_sessions_close_reason_check CHECK ((close_reason = ANY (ARRAY['exited'::text, 'closed_by_user'::text, 'detached'::text, 'environment_stopped'::text, 'link_lost'::text, 'revoked'::text, 'error'::text]))),
    CONSTRAINT terminal_sessions_state_check CHECK ((state = ANY (ARRAY['opening'::text, 'open'::text, 'closed'::text]))),
    CONSTRAINT terminal_sessions_pkey PRIMARY KEY (id),
    CONSTRAINT terminal_sessions_environment_id_fkey FOREIGN KEY (environment_id) REFERENCES environments(id),
    CONSTRAINT terminal_sessions_opened_by_fkey FOREIGN KEY (opened_by) REFERENCES users(id),
    CONSTRAINT terminal_sessions_recording_blob_fkey FOREIGN KEY (recording_blob) REFERENCES blobs(hash)
);

CREATE INDEX terminal_sessions_open_idx ON terminal_sessions USING btree (environment_id, opened_by) WHERE (state <> 'closed'::text);

CREATE TRIGGER terminal_sessions_live AFTER INSERT OR DELETE OR UPDATE ON terminal_sessions FOR EACH ROW EXECUTE FUNCTION machine_views_notify();
