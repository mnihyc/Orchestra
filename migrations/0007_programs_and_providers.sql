-- Agent programs and their builds, harnesses, agents, providers, models and skills.

CREATE TABLE agent_program_builds (
    setting_key text NOT NULL,
    kind text NOT NULL,
    version text NOT NULL,
    os text NOT NULL,
    arch text NOT NULL,
    priority integer NOT NULL,
    version_sort text NOT NULL COLLATE pg_catalog."C",
    payload jsonb NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT agent_program_builds_kind_version_os_arch_priority_key UNIQUE (kind, version, os, arch, priority),
    CONSTRAINT agent_program_builds_pkey PRIMARY KEY (setting_key)
);

CREATE INDEX agent_program_builds_checked_idx ON agent_program_builds USING btree (kind, os, arch, version_sort DESC, version DESC, priority DESC) WHERE ((payload ->> 'status'::text) = 'checked'::text);

CREATE INDEX agent_program_builds_default_idx ON agent_program_builds USING btree (kind, os, arch) WHERE (((payload ->> 'is_default'::text) = 'true'::text) AND ((payload ->> 'source'::text) = ANY (ARRAY['admin'::text, 'mirror'::text])));

CREATE INDEX agent_program_builds_page_idx ON agent_program_builds USING btree (kind, version_sort DESC, version DESC, os, arch, priority DESC);

CREATE INDEX agent_program_builds_platform_idx ON agent_program_builds USING btree (kind, os, arch, version_sort DESC, version DESC, priority DESC);

CREATE INDEX agent_program_builds_status_page_idx ON agent_program_builds USING btree (kind, ((payload ->> 'status'::text)), version_sort DESC, version DESC, os, arch, priority DESC);

CREATE STATISTICS agent_program_builds_authority_stats (dependencies, mcv) ON kind, priority, (payload ->> 'source'::text), (payload ->> 'checksum_origin'::text) FROM agent_program_builds;

CREATE TRIGGER agent_program_kinds_build AFTER INSERT ON agent_program_builds FOR EACH ROW EXECUTE FUNCTION orch_remember_program_kind();

CREATE TABLE agent_program_kinds (
    kind text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    manifest jsonb,
    search_text text GENERATED ALWAYS AS (orch_program_search_text(kind, (manifest -> 'names'::text))) STORED,
    CONSTRAINT agent_program_kinds_pkey PRIMARY KEY (kind)
);

CREATE INDEX agent_program_names_search_idx ON agent_program_kinds USING gin (search_text gin_trgm_ops);

CREATE TABLE agent_program_reports (
    machine_id uuid NOT NULL,
    kind text NOT NULL,
    manifest jsonb,
    builtin boolean DEFAULT false NOT NULL,
    reported_at timestamp with time zone NOT NULL,
    CONSTRAINT agent_program_reports_pkey PRIMARY KEY (machine_id, kind)
);

CREATE INDEX agent_program_reports_kind_idx ON agent_program_reports USING btree (kind, reported_at, machine_id);

CREATE INDEX agent_program_reports_manifest_idx ON agent_program_reports USING btree (kind, reported_at, machine_id) WHERE ((NOT builtin) AND (jsonb_typeof(manifest) = 'object'::text));

CREATE TRIGGER agent_program_kinds_report AFTER INSERT ON agent_program_reports FOR EACH ROW EXECUTE FUNCTION orch_remember_program_kind();

CREATE TRIGGER agent_program_names_report AFTER INSERT OR DELETE OR UPDATE ON agent_program_reports FOR EACH ROW EXECUTE FUNCTION orch_cache_program_name();

CREATE TABLE agents (
    id uuid NOT NULL,
    name text NOT NULL,
    owner_id uuid,
    org_id uuid,
    home_id uuid,
    harness_kind text NOT NULL,
    machine_id uuid,
    profile jsonb DEFAULT '{}'::jsonb NOT NULL,
    descriptor jsonb DEFAULT '{}'::jsonb NOT NULL,
    descriptor_overrides jsonb DEFAULT '{}'::jsonb NOT NULL,
    disabled boolean DEFAULT false NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    pause_note text,
    CONSTRAINT agents_name_check CHECK (((char_length(name) >= 1) AND (char_length(name) <= 80))),
    CONSTRAINT agents_pkey PRIMARY KEY (id),
    CONSTRAINT agents_home_id_fkey FOREIGN KEY (home_id) REFERENCES nodes(id),
    CONSTRAINT agents_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id),
    CONSTRAINT agents_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES users(id)
);

CREATE INDEX agents_program_descriptor_idx ON agents USING btree (harness_kind, (((org_id IS NULL) AND (machine_id IS NULL))) DESC, updated_at DESC, created_at, id) WHERE ((deleted_at IS NULL) AND (descriptor IS NOT NULL) AND (descriptor <> '{}'::jsonb));

CREATE INDEX agents_program_live_idx ON agents USING btree (harness_kind, created_at, id) WHERE (deleted_at IS NULL);

CREATE TRIGGER agent_program_kinds_agent AFTER INSERT OR UPDATE OF harness_kind ON agents FOR EACH ROW EXECUTE FUNCTION orch_remember_program_kind();

CREATE TRIGGER participation_agents_delete AFTER DELETE ON agents REFERENCING OLD TABLE AS participation_old_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER participation_agents_insert AFTER INSERT ON agents REFERENCING NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER participation_agents_update AFTER UPDATE ON agents REFERENCING OLD TABLE AS participation_old_objects NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TABLE capability_defs (
    key text NOT NULL,
    label text NOT NULL,
    description text NOT NULL,
    risk text NOT NULL,
    driver_patch jsonb NOT NULL,
    default_decision text NOT NULL,
    default_approver_scope text NOT NULL,
    builtin boolean DEFAULT false NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT capability_defs_default_approver_scope_check CHECK ((default_approver_scope = ANY (ARRAY['environment'::text, 'host'::text, 'org'::text, 'system'::text]))),
    CONSTRAINT capability_defs_default_decision_check CHECK ((default_decision = ANY (ARRAY['allow'::text, 'ask'::text, 'deny'::text]))),
    CONSTRAINT capability_defs_key_check CHECK ((key ~ '^[a-z][a-z0-9_]{1,31}$'::text)),
    CONSTRAINT capability_defs_risk_check CHECK ((risk = ANY (ARRAY['low'::text, 'medium'::text, 'high'::text, 'critical'::text]))),
    CONSTRAINT capability_defs_pkey PRIMARY KEY (key)
);

CREATE TABLE model_call_recovery (
    node_id uuid NOT NULL,
    purpose text NOT NULL,
    call_key uuid NOT NULL,
    reference uuid NOT NULL,
    failure jsonb NOT NULL,
    rule jsonb NOT NULL,
    cursor jsonb NOT NULL,
    action jsonb,
    state text NOT NULL,
    due_at timestamp with time zone,
    context jsonb NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT model_call_recovery_check CHECK (((purpose <> 'agent'::text) OR (call_key = node_id))),
    CONSTRAINT model_call_recovery_pkey PRIMARY KEY (node_id, purpose, call_key),
    CONSTRAINT model_call_recovery_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE
);

CREATE INDEX model_call_recovery_due_idx ON model_call_recovery USING btree (due_at, node_id) WHERE ((purpose = 'agent'::text) AND (state = ANY (ARRAY['scheduled'::text, 'switching'::text])));

CREATE INDEX model_call_recovery_workflow_idx ON model_call_recovery USING btree (node_id, purpose, ((context ->> 'workflow_id'::text)), updated_at DESC) WHERE (state <> 'dismissed'::text);

CREATE TABLE model_call_recovery_attempts (
    id uuid NOT NULL,
    node_id uuid NOT NULL,
    purpose text NOT NULL,
    failure_kind text NOT NULL,
    automatic boolean NOT NULL,
    at timestamp with time zone NOT NULL,
    CONSTRAINT model_call_recovery_attempts_pkey PRIMARY KEY (id),
    CONSTRAINT model_call_recovery_attempts_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE
);

CREATE INDEX model_call_recovery_attempts_window_idx ON model_call_recovery_attempts USING btree (node_id, at) INCLUDE (failure_kind, purpose, automatic);

CREATE TABLE profile_pins (
    user_id uuid NOT NULL,
    "position" smallint NOT NULL,
    object_kind text NOT NULL,
    object_id uuid NOT NULL,
    CONSTRAINT profile_pins_object_kind_check CHECK ((object_kind = ANY (ARRAY['node'::text, 'document'::text]))),
    CONSTRAINT profile_pins_position_check CHECK ((("position" >= 0) AND ("position" <= 5))),
    CONSTRAINT profile_pins_pkey PRIMARY KEY (user_id, "position"),
    CONSTRAINT profile_pins_user_id_object_kind_object_id_key UNIQUE (user_id, object_kind, object_id),
    CONSTRAINT profile_pins_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE providers (
    id uuid NOT NULL,
    name text NOT NULL,
    owner_id uuid NOT NULL,
    org_id uuid NOT NULL,
    home_id uuid NOT NULL,
    kind text NOT NULL,
    sources jsonb DEFAULT '{}'::jsonb NOT NULL,
    harness_kind text,
    agent_id uuid,
    upstreams jsonb DEFAULT '[]'::jsonb NOT NULL,
    config jsonb DEFAULT '{}'::jsonb NOT NULL,
    login_state jsonb,
    login_pending bytea,
    login_pending_at timestamp with time zone,
    protocols text[] DEFAULT '{}'::text[] NOT NULL,
    machine_id uuid,
    key_machine_id uuid,
    arrangements text[] NOT NULL,
    held_by_environment_id uuid,
    held_since timestamp with time zone,
    runtime_config jsonb DEFAULT '{}'::jsonb NOT NULL,
    status text DEFAULT 'needs_setup'::text NOT NULL,
    status_detail text,
    status_at timestamp with time zone DEFAULT now() NOT NULL,
    credential_trial_input_id uuid,
    credential_trial_turn_id uuid,
    facts_requested_at timestamp with time zone,
    credential_renewed_at timestamp with time zone,
    facts jsonb DEFAULT '{}'::jsonb NOT NULL,
    facts_overrides jsonb DEFAULT '{}'::jsonb NOT NULL,
    facts_at timestamp with time zone,
    revision bigint DEFAULT 1 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    runtime_unavailable text,
    CONSTRAINT providers_arrangements_check CHECK (((arrangements <@ ARRAY['separated'::text, 'key_separated'::text, 'credential_shared'::text, 'handed_over'::text]) AND (cardinality(arrangements) >= 1))),
    CONSTRAINT providers_check CHECK (((kind = 'subscription'::text) = (harness_kind IS NOT NULL))),
    CONSTRAINT providers_check1 CHECK (((kind = 'subscription'::text) = (agent_id IS NOT NULL))),
    CONSTRAINT providers_check2 CHECK (((kind <> 'api'::text) OR (jsonb_array_length(upstreams) > 0))),
    CONSTRAINT providers_check3 CHECK (((held_by_environment_id IS NULL) = (held_since IS NULL))),
    CONSTRAINT providers_kind_check CHECK ((kind = ANY (ARRAY['subscription'::text, 'api'::text]))),
    CONSTRAINT providers_name_check CHECK (((char_length(name) >= 1) AND (char_length(name) <= 80))),
    CONSTRAINT providers_status_check CHECK ((status = ANY (ARRAY['needs_setup'::text, 'ready'::text, 'exhausted'::text, 'error'::text, 'offline'::text, 'disabled'::text]))),
    CONSTRAINT providers_pkey PRIMARY KEY (id),
    CONSTRAINT providers_agent_fk FOREIGN KEY (agent_id) REFERENCES agents(id),
    CONSTRAINT providers_home_id_fkey FOREIGN KEY (home_id) REFERENCES nodes(id),
    CONSTRAINT providers_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id),
    CONSTRAINT providers_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES users(id)
);

CREATE INDEX provider_runtime_config_facts_idx ON providers USING gin (runtime_config jsonb_path_ops);

CREATE TRIGGER participation_providers_delete AFTER DELETE ON providers REFERENCING OLD TABLE AS participation_old_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER participation_providers_insert AFTER INSERT ON providers REFERENCING NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER participation_providers_update AFTER UPDATE ON providers REFERENCING OLD TABLE AS participation_old_objects NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER providers_status_time BEFORE INSERT OR UPDATE ON providers FOR EACH ROW EXECUTE FUNCTION provider_status_time();

CREATE TRIGGER providers_trial_recovered BEFORE UPDATE OF status ON providers FOR EACH ROW EXECUTE FUNCTION provider_trial_recovered();

ALTER TABLE jobs ADD CONSTRAINT jobs_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES providers(id);

ALTER TABLE leases ADD CONSTRAINT leases_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES providers(id);

CREATE TABLE summary_calls (
    id uuid NOT NULL,
    node_id uuid NOT NULL,
    topic_id uuid NOT NULL,
    thread_id uuid,
    provider_id uuid NOT NULL,
    person_id uuid,
    model_name text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    purpose text DEFAULT 'summary'::text NOT NULL,
    cancel_reason text,
    agent_id uuid,
    environment_id uuid,
    CONSTRAINT summary_calls_purpose_check CHECK ((purpose = ANY (ARRAY['summary'::text, 'embeddings'::text]))),
    CONSTRAINT summary_calls_pkey PRIMARY KEY (id)
);

CREATE INDEX summary_calls_agent_active_idx ON summary_calls USING btree (agent_id) WHERE (cancel_reason IS NULL);

CREATE INDEX summary_calls_embeddings_provider_idx ON summary_calls USING btree (provider_id, id) WHERE (purpose = 'embeddings'::text);

CREATE INDEX summary_calls_environment_active_idx ON summary_calls USING btree (environment_id) WHERE (cancel_reason IS NULL);

CREATE INDEX summary_calls_provider_active_idx ON summary_calls USING btree (provider_id, model_name) WHERE (cancel_reason IS NULL);

CREATE INDEX summary_calls_purpose_created_idx ON summary_calls USING btree (purpose, created_at, id);

CREATE TABLE harnesses (
    provider_id uuid NOT NULL,
    agent_id uuid NOT NULL,
    machine_id uuid NOT NULL,
    status text DEFAULT 'offline'::text NOT NULL,
    status_detail text,
    start_command_id uuid,
    after_up_plan jsonb DEFAULT '{}'::jsonb NOT NULL,
    harness_version text,
    descriptor jsonb DEFAULT '{}'::jsonb NOT NULL,
    sessions_open integer DEFAULT 0 NOT NULL,
    last_active_at timestamp with time zone,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    last_error_at timestamp with time zone,
    last_error_detail text,
    CONSTRAINT harnesses_last_error_time CHECK (((last_error_detail IS NULL) OR (last_error_at IS NOT NULL))),
    CONSTRAINT harnesses_status_check CHECK ((status = ANY (ARRAY['offline'::text, 'starting'::text, 'online'::text, 'error'::text]))),
    CONSTRAINT harnesses_pkey PRIMARY KEY (provider_id, agent_id, machine_id),
    CONSTRAINT harnesses_agent_id_fkey FOREIGN KEY (agent_id) REFERENCES agents(id),
    CONSTRAINT harnesses_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES providers(id) ON DELETE CASCADE
);

CREATE TABLE provider_models (
    id uuid NOT NULL,
    provider_id uuid NOT NULL,
    model_name text NOT NULL,
    display_name text DEFAULT ''::text NOT NULL,
    parameters jsonb DEFAULT '{}'::jsonb NOT NULL,
    groups text[] DEFAULT '{}'::text[] NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    source text NOT NULL,
    created_by uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    pause_note text,
    CONSTRAINT provider_models_display_name_check CHECK ((char_length(display_name) <= 120)),
    CONSTRAINT provider_models_groups_check CHECK ((cardinality(groups) <= 8)),
    CONSTRAINT provider_models_model_name_check CHECK (((char_length(model_name) >= 1) AND (char_length(model_name) <= 200))),
    CONSTRAINT provider_models_source_check CHECK ((source = ANY (ARRAY['manual'::text, 'suggested'::text]))),
    CONSTRAINT provider_models_pkey PRIMARY KEY (id),
    CONSTRAINT provider_models_provider_id_model_name_key UNIQUE (provider_id, model_name),
    CONSTRAINT provider_models_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id),
    CONSTRAINT provider_models_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES providers(id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX provider_models_display_idx ON provider_models USING btree (provider_id, lower(display_name)) WHERE (display_name <> ''::text);

CREATE INDEX provider_models_groups_idx ON provider_models USING gin (groups);

CREATE TABLE skill_draft_files (
    skill_id uuid NOT NULL,
    path text NOT NULL,
    blob bytea NOT NULL,
    size bigint NOT NULL,
    executable boolean DEFAULT false NOT NULL,
    updated_by uuid NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT skill_draft_files_path_check CHECK (((char_length(path) >= 1) AND (char_length(path) <= 4096))),
    CONSTRAINT skill_draft_files_size_check CHECK ((size >= 0)),
    CONSTRAINT skill_draft_files_pkey PRIMARY KEY (skill_id, path),
    CONSTRAINT skill_draft_files_blob_fkey FOREIGN KEY (blob) REFERENCES blobs(hash),
    CONSTRAINT skill_draft_files_updated_by_fkey FOREIGN KEY (updated_by) REFERENCES users(id)
);

CREATE TRIGGER skill_draft_file_blob_pin AFTER INSERT OR DELETE OR UPDATE OF blob ON skill_draft_files FOR EACH ROW EXECUTE FUNCTION skill_blob_pin();

CREATE TABLE skill_drafts (
    skill_id uuid NOT NULL,
    based_on uuid,
    revision bigint DEFAULT 1 NOT NULL,
    created_by uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_by uuid NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT skill_drafts_pkey PRIMARY KEY (skill_id),
    CONSTRAINT skill_drafts_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id),
    CONSTRAINT skill_drafts_updated_by_fkey FOREIGN KEY (updated_by) REFERENCES users(id)
);

ALTER TABLE skill_draft_files ADD CONSTRAINT skill_draft_files_skill_id_fkey FOREIGN KEY (skill_id) REFERENCES skill_drafts(skill_id) ON DELETE CASCADE;

CREATE TABLE skill_versions (
    id uuid NOT NULL,
    skill_id uuid NOT NULL,
    number integer NOT NULL,
    name text NOT NULL,
    description text NOT NULL,
    archive bytea NOT NULL,
    archive_size bigint NOT NULL,
    files jsonb NOT NULL,
    size bigint NOT NULL,
    origin jsonb NOT NULL,
    created_by uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT skill_versions_archive_size_check CHECK ((archive_size >= 0)),
    CONSTRAINT skill_versions_description_check CHECK (((char_length(description) >= 1) AND (char_length(description) <= 1024))),
    CONSTRAINT skill_versions_number_check CHECK ((number > 0)),
    CONSTRAINT skill_versions_size_check CHECK ((size >= 0)),
    CONSTRAINT skill_versions_pkey PRIMARY KEY (id),
    CONSTRAINT skill_versions_skill_id_number_key UNIQUE (skill_id, number),
    CONSTRAINT skill_versions_archive_fkey FOREIGN KEY (archive) REFERENCES blobs(hash),
    CONSTRAINT skill_versions_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id)
);

CREATE TRIGGER skill_version_blob_pin AFTER INSERT OR DELETE ON skill_versions FOR EACH ROW EXECUTE FUNCTION skill_blob_pin();

ALTER TABLE skill_drafts ADD CONSTRAINT skill_drafts_based_on_fkey FOREIGN KEY (based_on) REFERENCES skill_versions(id);

CREATE TABLE skills (
    id uuid NOT NULL,
    org_id uuid NOT NULL,
    home_id uuid NOT NULL,
    owner_id uuid NOT NULL,
    name text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    origin jsonb,
    latest_version_id uuid,
    enabled boolean DEFAULT true NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    source_url_secret_id uuid,
    CONSTRAINT skills_description_check CHECK ((char_length(description) <= 1024)),
    CONSTRAINT skills_name_check CHECK (((name ~ '^[a-z0-9]([a-z0-9-]{0,62}[a-z0-9])?$'::text) AND (name !~ '--'::text))),
    CONSTRAINT skills_pkey PRIMARY KEY (id),
    CONSTRAINT skills_home_id_fkey FOREIGN KEY (home_id) REFERENCES nodes(id),
    CONSTRAINT skills_latest_version_fk FOREIGN KEY (latest_version_id) REFERENCES skill_versions(id),
    CONSTRAINT skills_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id),
    CONSTRAINT skills_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES users(id),
    CONSTRAINT skills_source_url_secret_id_fkey FOREIGN KEY (source_url_secret_id) REFERENCES secrets(id)
);

CREATE UNIQUE INDEX skills_home_name_key ON skills USING btree (home_id, name) WHERE (deleted_at IS NULL);

CREATE INDEX skills_org_idx ON skills USING btree (org_id, id) WHERE (deleted_at IS NULL);

CREATE TRIGGER participation_skills_delete AFTER DELETE ON skills REFERENCING OLD TABLE AS participation_old_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER participation_skills_insert AFTER INSERT ON skills REFERENCING NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER participation_skills_update AFTER UPDATE ON skills REFERENCING OLD TABLE AS participation_old_objects NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER skill_source_secrets_release AFTER DELETE ON skills FOR EACH ROW EXECUTE FUNCTION skill_source_secrets_release();

ALTER TABLE skill_drafts ADD CONSTRAINT skill_drafts_skill_id_fkey FOREIGN KEY (skill_id) REFERENCES skills(id) ON DELETE CASCADE;

ALTER TABLE skill_versions ADD CONSTRAINT skill_versions_skill_id_fkey FOREIGN KEY (skill_id) REFERENCES skills(id) ON DELETE CASCADE;
