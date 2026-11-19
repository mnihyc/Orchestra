-- Blobs, uploads, snapshots and the file cache.

CREATE TABLE blob_uploads (
    hash bytea NOT NULL,
    size bigint NOT NULL,
    received bigint DEFAULT 0 NOT NULL,
    machine_id uuid,
    user_id uuid,
    started_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    command_id uuid,
    CONSTRAINT blob_uploads_pkey PRIMARY KEY (hash),
    CONSTRAINT blob_uploads_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id)
);

CREATE INDEX blob_uploads_machine_command_idx ON blob_uploads USING btree (machine_id, command_id, hash) WHERE (command_id IS NOT NULL);

CREATE INDEX blob_uploads_machine_updated_idx ON blob_uploads USING btree (machine_id, updated_at);

CREATE TABLE blobs (
    hash bytea NOT NULL,
    size bigint NOT NULL,
    stored_at timestamp with time zone DEFAULT now() NOT NULL,
    last_access_at timestamp with time zone DEFAULT now() NOT NULL,
    pin_count integer DEFAULT 0 NOT NULL,
    CONSTRAINT blobs_pkey PRIMARY KEY (hash)
);

ALTER TABLE agent_shares ADD CONSTRAINT agent_shares_manifest_blob_fkey FOREIGN KEY (manifest_blob) REFERENCES blobs(hash);

CREATE TABLE completed_uploads (
    user_id uuid NOT NULL,
    hash bytea NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT completed_uploads_pkey PRIMARY KEY (user_id, hash),
    CONSTRAINT completed_uploads_hash_fkey FOREIGN KEY (hash) REFERENCES blobs(hash) ON DELETE CASCADE,
    CONSTRAINT completed_uploads_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX completed_uploads_hash_idx ON completed_uploads USING btree (hash);

CREATE TABLE file_cache (
    environment_id uuid NOT NULL,
    path text NOT NULL,
    size bigint NOT NULL,
    mtime_ns bigint NOT NULL,
    hash bytea NOT NULL,
    cached_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT file_cache_pkey PRIMARY KEY (environment_id, path, size, mtime_ns),
    CONSTRAINT file_cache_hash_fkey FOREIGN KEY (hash) REFERENCES blobs(hash)
);

CREATE INDEX file_cache_env_idx ON file_cache USING btree (environment_id, cached_at);

CREATE TABLE snapshots (
    id uuid NOT NULL,
    thread_id uuid,
    environment_id uuid NOT NULL,
    env_generation integer NOT NULL,
    kind text DEFAULT 'manual'::text NOT NULL,
    storage text DEFAULT 'blobs'::text NOT NULL,
    directory text,
    tree_ref text,
    repository_head text,
    unavailable_reason text,
    expires_at timestamp with time zone,
    label text DEFAULT ''::text NOT NULL,
    selection jsonb NOT NULL,
    state text NOT NULL,
    file_count bigint DEFAULT 0 NOT NULL,
    total_bytes bigint DEFAULT 0 NOT NULL,
    missing_count bigint DEFAULT 0 NOT NULL,
    manifest_blob bytea,
    created_by uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    completed_at timestamp with time zone,
    CONSTRAINT snapshots_check CHECK (((thread_id IS NOT NULL) OR ((kind = 'manual'::text) AND (storage = 'blobs'::text)))),
    CONSTRAINT snapshots_kind_check CHECK ((kind = ANY (ARRAY['manual'::text, 'turn'::text, 'restore'::text]))),
    CONSTRAINT snapshots_state_check CHECK ((state = ANY (ARRAY['running'::text, 'complete'::text, 'partial'::text, 'failed'::text, 'unavailable'::text, 'expired'::text]))),
    CONSTRAINT snapshots_storage_check CHECK ((storage = ANY (ARRAY['blobs'::text, 'environment'::text]))),
    CONSTRAINT snapshots_pkey PRIMARY KEY (id),
    CONSTRAINT snapshots_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id),
    CONSTRAINT snapshots_manifest_blob_fkey FOREIGN KEY (manifest_blob) REFERENCES blobs(hash)
);

CREATE TABLE upload_sessions (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    name text NOT NULL,
    media_type text,
    size bigint NOT NULL,
    received bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    cancelled_at timestamp with time zone,
    completed_at timestamp with time zone,
    CONSTRAINT upload_sessions_pkey PRIMARY KEY (id),
    CONSTRAINT upload_sessions_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE snapshot_blob_refs (
    snapshot_id uuid NOT NULL,
    blob bytea NOT NULL,
    CONSTRAINT snapshot_blob_refs_pkey PRIMARY KEY (snapshot_id, blob),
    CONSTRAINT snapshot_blob_refs_blob_fkey FOREIGN KEY (blob) REFERENCES blobs(hash),
    CONSTRAINT snapshot_blob_refs_snapshot_id_fkey FOREIGN KEY (snapshot_id) REFERENCES snapshots(id) ON DELETE CASCADE
);
