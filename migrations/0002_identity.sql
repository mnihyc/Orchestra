-- People: users, their identities and sign-in, sessions, tokens, devices and invitations.

CREATE TABLE identity_providers (
    id uuid NOT NULL,
    key text NOT NULL,
    name text NOT NULL,
    kind text DEFAULT 'oidc'::text NOT NULL,
    issuer text NOT NULL,
    client_id text NOT NULL,
    scopes text[] DEFAULT '{openid,profile,email}'::text[] NOT NULL,
    claim_subject text DEFAULT 'sub'::text NOT NULL,
    claim_email text DEFAULT 'email'::text NOT NULL,
    claim_name text DEFAULT 'name'::text NOT NULL,
    claim_username text DEFAULT 'preferred_username'::text NOT NULL,
    allowed_email_domains text[] DEFAULT '{}'::text[] NOT NULL,
    auto_register boolean DEFAULT false NOT NULL,
    default_org_id uuid,
    default_role text,
    enabled boolean DEFAULT true NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT identity_providers_default_role_check CHECK ((default_role = ANY (ARRAY['viewer'::text, 'interactor'::text, 'developer'::text, 'admin'::text]))),
    CONSTRAINT identity_providers_key_check CHECK ((key ~ '^[a-z][a-z0-9-]{1,31}$'::text)),
    CONSTRAINT identity_providers_kind_check CHECK ((kind = 'oidc'::text)),
    CONSTRAINT identity_providers_key_key UNIQUE (key),
    CONSTRAINT identity_providers_pkey PRIMARY KEY (id)
);

CREATE TABLE login_throttle (
    bucket text NOT NULL,
    times timestamp with time zone[] NOT NULL,
    noticed_at timestamp with time zone,
    CONSTRAINT login_throttle_pkey PRIMARY KEY (bucket)
);

CREATE TABLE users (
    id uuid NOT NULL,
    kind text DEFAULT 'human'::text NOT NULL,
    service_org_id uuid,
    username text NOT NULL,
    display_name text NOT NULL,
    email text,
    email_verified_at timestamp with time zone,
    password_hash text,
    totp_secret bytea,
    totp_enabled_at timestamp with time zone,
    totp_last_step bigint,
    email_code_at timestamp with time zone,
    is_system_admin boolean DEFAULT false NOT NULL,
    org_invited_at timestamp with time zone,
    org_invited_by uuid,
    status text DEFAULT 'active'::text NOT NULL,
    avatar_color smallint NOT NULL,
    locale text DEFAULT 'en'::text NOT NULL,
    timezone text DEFAULT 'UTC'::text NOT NULL,
    timezone_mode text DEFAULT 'device'::text NOT NULL,
    timezone_reported_at timestamp with time zone,
    theme text DEFAULT 'system'::text NOT NULL,
    density text DEFAULT 'comfortable'::text NOT NULL,
    personal_org_id uuid,
    inbox_seq bigint DEFAULT 0 NOT NULL,
    created_by uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    last_seen_at timestamp with time zone,
    CONSTRAINT users_avatar_color_check CHECK (((avatar_color >= 0) AND (avatar_color <= 11))),
    CONSTRAINT users_check CHECK (((kind = 'service'::text) = (service_org_id IS NOT NULL))),
    CONSTRAINT users_check1 CHECK (((org_invited_at IS NULL) = (org_invited_by IS NULL))),
    CONSTRAINT users_density_check CHECK ((density = ANY (ARRAY['comfortable'::text, 'compact'::text]))),
    CONSTRAINT users_display_name_check CHECK (((char_length(display_name) >= 1) AND (char_length(display_name) <= 64))),
    CONSTRAINT users_kind_check CHECK ((kind = ANY (ARRAY['human'::text, 'service'::text]))),
    CONSTRAINT users_locale_check CHECK ((locale = ANY (ARRAY['en'::text, 'zh-CN'::text]))),
    CONSTRAINT users_status_check CHECK ((status = ANY (ARRAY['active'::text, 'suspended'::text]))),
    CONSTRAINT users_theme_check CHECK ((theme = ANY (ARRAY['system'::text, 'light'::text, 'dark'::text]))),
    CONSTRAINT users_timezone_mode_check CHECK ((timezone_mode = ANY (ARRAY['device'::text, 'fixed'::text]))),
    CONSTRAINT users_username_check CHECK ((username ~ '^[a-z0-9][a-z0-9_-]{2,31}$'::text)),
    CONSTRAINT users_pkey PRIMARY KEY (id),
    CONSTRAINT users_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id),
    CONSTRAINT users_org_invited_by_fkey FOREIGN KEY (org_invited_by) REFERENCES users(id)
);

CREATE UNIQUE INDEX users_email_key ON users USING btree (lower(email)) WHERE (email IS NOT NULL);

CREATE INDEX users_people_name_idx ON users USING gin ((((lower(username) || ' '::text) || lower(display_name))) gin_trgm_ops) WHERE ((kind = 'human'::text) AND (status = 'active'::text));

CREATE INDEX users_people_page_idx ON users USING btree (id) WHERE ((kind = 'human'::text) AND (status = 'active'::text));

CREATE INDEX users_system_admin_idx ON users USING btree (id) WHERE (is_system_admin AND (status = 'active'::text));

CREATE UNIQUE INDEX users_username_key ON users USING btree (username);

CREATE TRIGGER access_changed_delete AFTER DELETE ON users FOR EACH ROW EXECUTE FUNCTION access_changed();

CREATE TRIGGER access_changed_update AFTER UPDATE OF status, is_system_admin ON users FOR EACH ROW WHEN (((old.status IS DISTINCT FROM new.status) OR (old.is_system_admin IS DISTINCT FROM new.is_system_admin))) EXECUTE FUNCTION access_changed();

CREATE TRIGGER public_name_user AFTER INSERT OR UPDATE OF username ON users FOR EACH ROW EXECUTE FUNCTION orch_public_name_user();

CREATE TABLE api_tokens (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    name text NOT NULL,
    token_hash bytea NOT NULL,
    scopes text[] DEFAULT '{}'::text[] NOT NULL,
    node_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_used_at timestamp with time zone,
    expires_at timestamp with time zone,
    revoked_at timestamp with time zone,
    CONSTRAINT api_tokens_name_check CHECK (((char_length(name) >= 1) AND (char_length(name) <= 80))),
    CONSTRAINT api_tokens_pkey PRIMARY KEY (id),
    CONSTRAINT api_tokens_token_hash_key UNIQUE (token_hash),
    CONSTRAINT api_tokens_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE email_tokens (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    purpose text NOT NULL,
    token_hash bytea NOT NULL,
    email text NOT NULL,
    clear_second_step boolean DEFAULT false NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    used_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT email_tokens_purpose_check CHECK ((purpose = ANY (ARRAY['verify_email'::text, 'reset_password'::text]))),
    CONSTRAINT email_tokens_pkey PRIMARY KEY (id),
    CONSTRAINT email_tokens_token_hash_key UNIQUE (token_hash),
    CONSTRAINT email_tokens_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE enrollment_tokens (
    token_hash bytea NOT NULL,
    created_by uuid NOT NULL,
    purpose text NOT NULL,
    issuer_kind text DEFAULT 'system'::text NOT NULL,
    issuer_credential_id uuid,
    issuer_as_admin boolean DEFAULT false NOT NULL,
    roles text[] DEFAULT ARRAY['agent'::text, 'environment'::text] NOT NULL,
    org_id uuid,
    runtime_id uuid,
    name_hint text,
    access text,
    expires_at timestamp with time zone NOT NULL,
    used_at timestamp with time zone,
    machine_id uuid,
    again_machine_id uuid,
    CONSTRAINT enrollment_tokens_access_check CHECK ((access = ANY (ARRAY['defaults'::text, 'only_me'::text, 'home'::text]))),
    CONSTRAINT enrollment_tokens_purpose_check CHECK ((purpose = ANY (ARRAY['machine'::text, 'connected'::text, 'host'::text, 'runtime'::text]))),
    CONSTRAINT enrollment_tokens_issuer_check CHECK (((issuer_kind = 'system' AND issuer_credential_id IS NULL) OR (issuer_kind IN ('session', 'token') AND issuer_credential_id IS NOT NULL))),
    CONSTRAINT enrollment_tokens_roles_check CHECK ((roles <@ ARRAY['provider'::text, 'agent'::text, 'environment'::text, 'node'::text])),
    CONSTRAINT enrollment_tokens_pkey PRIMARY KEY (token_hash),
    CONSTRAINT enrollment_tokens_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id)
);

CREATE TABLE invitations (
    id uuid NOT NULL,
    token_hash bytea NOT NULL,
    kind text NOT NULL,
    org_id uuid,
    node_id uuid,
    chat_id uuid,
    role text,
    permissions text[] DEFAULT '{}'::text[] NOT NULL,
    email text,
    max_uses integer DEFAULT 1 NOT NULL,
    uses integer DEFAULT 0 NOT NULL,
    note text DEFAULT ''::text NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    revoked_at timestamp with time zone,
    created_by uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    created_as_admin boolean,
    CONSTRAINT invitations_check CHECK (((kind = 'org'::text) = (org_id IS NOT NULL))),
    CONSTRAINT invitations_check1 CHECK (((kind = 'node'::text) = (node_id IS NOT NULL))),
    CONSTRAINT invitations_check2 CHECK (((kind = 'chat'::text) = (chat_id IS NOT NULL))),
    CONSTRAINT invitations_check3 CHECK (((email IS NULL) OR (max_uses = 1))),
    CONSTRAINT invitations_kind_check CHECK ((kind = ANY (ARRAY['system'::text, 'org'::text, 'node'::text, 'chat'::text]))),
    CONSTRAINT invitations_max_uses_check CHECK (((max_uses >= 1) AND (max_uses <= 1000))),
    CONSTRAINT invitations_role_check CHECK ((role = ANY (ARRAY['viewer'::text, 'interactor'::text, 'developer'::text, 'admin'::text]))),
    CONSTRAINT invitations_pkey PRIMARY KEY (id),
    CONSTRAINT invitations_token_hash_key UNIQUE (token_hash),
    CONSTRAINT invitations_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id)
);

CREATE TABLE known_devices (
    id uuid NOT NULL,
    mark_hash bytea NOT NULL,
    user_id uuid NOT NULL,
    first_at timestamp with time zone DEFAULT now() NOT NULL,
    last_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT known_devices_mark_hash_user_id_key UNIQUE (mark_hash, user_id),
    CONSTRAINT known_devices_pkey PRIMARY KEY (id),
    CONSTRAINT known_devices_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX known_devices_user_idx ON known_devices USING btree (user_id, last_at);

CREATE TABLE public_names (
    name text NOT NULL,
    user_id uuid,
    org_id uuid,
    CONSTRAINT public_names_org_id_key UNIQUE (org_id),
    CONSTRAINT public_names_pkey PRIMARY KEY (name),
    CONSTRAINT public_names_user_id_key UNIQUE (user_id),
    CONSTRAINT public_names_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE SET NULL
);

CREATE TRIGGER public_name_cleanup AFTER INSERT OR UPDATE ON public_names FOR EACH ROW EXECUTE FUNCTION orch_public_name_cleanup();

CREATE TABLE second_step_recovery_codes (
    user_id uuid NOT NULL,
    code_hash bytea NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    used_at timestamp with time zone,
    CONSTRAINT second_step_recovery_codes_code_hash_check CHECK ((octet_length(code_hash) = 32)),
    CONSTRAINT second_step_recovery_codes_pkey PRIMARY KEY (user_id, code_hash),
    CONSTRAINT second_step_recovery_codes_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE sessions (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    refresh_hash bytea NOT NULL,
    user_agent text DEFAULT ''::text NOT NULL,
    ip inet,
    second_step_at timestamp with time zone,
    as_admin boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_used_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    revoked_at timestamp with time zone,
    CONSTRAINT sessions_user_agent_check CHECK ((char_length(user_agent) <= 400)),
    CONSTRAINT sessions_pkey PRIMARY KEY (id),
    CONSTRAINT sessions_refresh_hash_key UNIQUE (refresh_hash),
    CONSTRAINT sessions_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX sessions_user_idx ON sessions USING btree (user_id) WHERE (revoked_at IS NULL);

CREATE TABLE sign_in_codes (
    user_id uuid NOT NULL,
    code_hash bytea NOT NULL,
    email text NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT sign_in_codes_pkey PRIMARY KEY (user_id),
    CONSTRAINT sign_in_codes_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE user_identities (
    identity_provider_id uuid NOT NULL,
    subject text NOT NULL,
    user_id uuid NOT NULL,
    email text,
    linked_at timestamp with time zone DEFAULT now() NOT NULL,
    last_login_at timestamp with time zone,
    CONSTRAINT user_identities_pkey PRIMARY KEY (identity_provider_id, subject),
    CONSTRAINT user_identities_identity_provider_id_fkey FOREIGN KEY (identity_provider_id) REFERENCES identity_providers(id) ON DELETE CASCADE,
    CONSTRAINT user_identities_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX user_identities_user_idx ON user_identities USING btree (user_id);

CREATE TABLE user_profiles (
    user_id uuid NOT NULL,
    bio text DEFAULT ''::text NOT NULL,
    pronouns text DEFAULT ''::text NOT NULL,
    links jsonb DEFAULT '[]'::jsonb NOT NULL,
    show_time boolean DEFAULT true NOT NULL,
    show_activity boolean DEFAULT true NOT NULL,
    audience text DEFAULT 'system'::text NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT user_profiles_audience_check CHECK ((audience = ANY (ARRAY['system'::text, 'orgs'::text, 'private'::text]))),
    CONSTRAINT user_profiles_bio_check CHECK ((char_length(bio) <= 2000)),
    CONSTRAINT user_profiles_pronouns_check CHECK ((char_length(pronouns) <= 40)),
    CONSTRAINT user_profiles_pkey PRIMARY KEY (user_id),
    CONSTRAINT user_profiles_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE user_state (
    user_id uuid NOT NULL,
    key text NOT NULL,
    value jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT user_state_key_check CHECK ((key = ANY (ARRAY['help'::text, 'layouts'::text, 'viewers'::text, 'timeline'::text, 'sidebar'::text, 'inbox'::text, 'keys'::text, 'project_views'::text, 'reactions'::text]))),
    CONSTRAINT user_state_value_check CHECK ((pg_column_size(value) <= 65536)),
    CONSTRAINT user_state_pkey PRIMARY KEY (user_id, key),
    CONSTRAINT user_state_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE access_tokens (
    token_hash bytea NOT NULL,
    session_id uuid NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    CONSTRAINT access_tokens_pkey PRIMARY KEY (token_hash),
    CONSTRAINT access_tokens_session_id_fkey FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
);

CREATE TABLE api_token_refusals (
    token_id uuid NOT NULL,
    org_id uuid NOT NULL,
    by_user_id uuid NOT NULL,
    at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT api_token_refusals_pkey PRIMARY KEY (token_id, org_id),
    CONSTRAINT api_token_refusals_by_user_id_fkey FOREIGN KEY (by_user_id) REFERENCES users(id),
    CONSTRAINT api_token_refusals_token_id_fkey FOREIGN KEY (token_id) REFERENCES api_tokens(id) ON DELETE CASCADE
);

CREATE INDEX api_token_refusals_org_idx ON api_token_refusals USING btree (org_id, at DESC);

CREATE TABLE invitation_uses (
    invitation_id uuid NOT NULL,
    user_id uuid NOT NULL,
    used_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT invitation_uses_pkey PRIMARY KEY (invitation_id, user_id),
    CONSTRAINT invitation_uses_invitation_id_fkey FOREIGN KEY (invitation_id) REFERENCES invitations(id) ON DELETE CASCADE,
    CONSTRAINT invitation_uses_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE oidc_states (
    state text NOT NULL,
    identity_provider_id uuid NOT NULL,
    nonce text NOT NULL,
    pkce_verifier text NOT NULL,
    redirect_path text DEFAULT '/'::text NOT NULL,
    link_user_id uuid,
    invitation text,
    accept_invitation boolean DEFAULT false NOT NULL,
    session_id uuid,
    sensitive_purpose text,
    service_account_id uuid,
    browser_hash bytea NOT NULL,
    started_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    CONSTRAINT oidc_states_check CHECK (((sensitive_purpose IS NULL AND link_user_id IS NULL) = (session_id IS NULL)) AND (sensitive_purpose IS NULL OR link_user_id IS NULL)),
    CONSTRAINT oidc_states_check1 CHECK (((NOT accept_invitation) OR ((invitation IS NOT NULL) AND (link_user_id IS NULL) AND (sensitive_purpose IS NULL)))),
    CONSTRAINT oidc_states_sensitive_purpose_check CHECK ((sensitive_purpose = ANY (ARRAY['token_create'::text, 'second_step_setup'::text, 'recovery_codes'::text]))),
    CONSTRAINT oidc_states_pkey PRIMARY KEY (state),
    CONSTRAINT oidc_states_identity_provider_id_fkey FOREIGN KEY (identity_provider_id) REFERENCES identity_providers(id) ON DELETE CASCADE,
    CONSTRAINT oidc_states_link_user_id_fkey FOREIGN KEY (link_user_id) REFERENCES users(id) ON DELETE CASCADE,
    CONSTRAINT oidc_states_service_account_id_fkey FOREIGN KEY (service_account_id) REFERENCES users(id) ON DELETE CASCADE,
    CONSTRAINT oidc_states_session_id_fkey FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
);

CREATE TABLE sensitive_proofs (
    proof_hash bytea NOT NULL,
    session_id uuid NOT NULL,
    purpose text NOT NULL,
    factor_hash bytea,
    service_account_id uuid,
    verified_at timestamp with time zone NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    used_at timestamp with time zone,
    CONSTRAINT sensitive_proofs_factor_hash_check CHECK (((factor_hash IS NULL) OR (octet_length(factor_hash) = 32))),
    CONSTRAINT sensitive_proofs_pkey PRIMARY KEY (proof_hash),
    CONSTRAINT sensitive_proofs_service_account_id_fkey FOREIGN KEY (service_account_id) REFERENCES users(id) ON DELETE CASCADE,
    CONSTRAINT sensitive_proofs_session_id_fkey FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
);

CREATE INDEX sensitive_proofs_session_purpose_idx ON sensitive_proofs USING btree (session_id, purpose, expires_at);
