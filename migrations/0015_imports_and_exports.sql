-- Packages exported and imported, with their origins and reports.

CREATE TABLE exports (
    id uuid NOT NULL,
    user_id uuid,
    node_id uuid,
    request jsonb NOT NULL,
    job_id uuid,
    state text DEFAULT 'pending'::text NOT NULL,
    blob text,
    bytes bigint,
    counts jsonb DEFAULT '{}'::jsonb NOT NULL,
    left_out jsonb DEFAULT '[]'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    ready_at timestamp with time zone,
    expires_at timestamp with time zone,
    downloaded_at timestamp with time zone,
    CONSTRAINT exports_state_check CHECK ((state = ANY (ARRAY['pending'::text, 'ready'::text, 'failed'::text, 'expired'::text]))),
    CONSTRAINT exports_pkey PRIMARY KEY (id),
    CONSTRAINT exports_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE SET NULL,
    CONSTRAINT exports_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE SET NULL
);

CREATE INDEX exports_due_idx ON exports USING btree (expires_at) WHERE (state = 'ready'::text);

CREATE INDEX exports_user_idx ON exports USING btree (user_id, created_at DESC);

CREATE TABLE imports (
    id uuid NOT NULL,
    user_id uuid,
    node_id uuid,
    client_id text NOT NULL,
    request jsonb NOT NULL,
    job_id uuid,
    undo_job_id uuid,
    state text DEFAULT 'pending'::text NOT NULL,
    files text[] DEFAULT '{}'::text[] NOT NULL,
    source_blobs text[] DEFAULT '{}'::text[] NOT NULL,
    package jsonb,
    counts jsonb,
    actions jsonb,
    losses jsonb DEFAULT '[]'::jsonb NOT NULL,
    failure jsonb,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    finished_at timestamp with time zone,
    undone_at timestamp with time zone,
    CONSTRAINT imports_state_check CHECK ((state = ANY (ARRAY['pending'::text, 'done'::text, 'failed'::text, 'cancelled'::text, 'undoing'::text, 'undone'::text]))),
    CONSTRAINT imports_pkey PRIMARY KEY (id),
    CONSTRAINT imports_user_id_client_id_key UNIQUE (user_id, client_id),
    CONSTRAINT imports_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE SET NULL,
    CONSTRAINT imports_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE SET NULL
);

CREATE INDEX imports_node_idx ON imports USING btree (node_id) WHERE (node_id IS NOT NULL);

CREATE INDEX imports_user_idx ON imports USING btree (user_id, created_at DESC, id DESC);

CREATE TABLE import_origins (
    scope_id uuid NOT NULL,
    source_id uuid NOT NULL,
    kind text NOT NULL,
    object_id uuid NOT NULL,
    parent_id uuid,
    created_by_import uuid,
    last_import uuid NOT NULL,
    source_name text,
    source_path text,
    source_sha256 text,
    source_updated_at timestamp with time zone,
    source_creator jsonb,
    imported_by uuid,
    imported_at timestamp with time zone NOT NULL,
    node_depth integer,
    CONSTRAINT import_origins_kind_check CHECK ((kind = ANY (ARRAY['org'::text, 'project'::text, 'topic'::text, 'thread'::text, 'chat'::text, 'folder'::text, 'document'::text, 'message'::text, 'turn'::text, 'item'::text, 'comment'::text, 'version'::text]))),
    CONSTRAINT import_origins_pkey PRIMARY KEY (scope_id, source_id),
    CONSTRAINT import_origins_created_by_import_fkey FOREIGN KEY (created_by_import) REFERENCES imports(id),
    CONSTRAINT import_origins_imported_by_fkey FOREIGN KEY (imported_by) REFERENCES users(id) ON DELETE SET NULL,
    CONSTRAINT import_origins_last_import_fkey FOREIGN KEY (last_import) REFERENCES imports(id),
    CONSTRAINT import_origins_scope_id_fkey FOREIGN KEY (scope_id) REFERENCES nodes(id) ON DELETE CASCADE
);

COMMENT ON COLUMN import_origins.node_depth IS 'Current path depth for a live node created by this import; NULL for other origins.';

CREATE INDEX import_origins_inside_idx ON import_origins USING btree (created_by_import, kind, parent_id, object_id) WHERE ((created_by_import IS NOT NULL) AND (parent_id IS NOT NULL));

CREATE INDEX import_origins_last_idx ON import_origins USING btree (last_import);

CREATE INDEX import_origins_made_idx ON import_origins USING btree (created_by_import, kind, object_id) WHERE (created_by_import IS NOT NULL);

CREATE INDEX import_origins_node_order_idx ON import_origins USING btree (created_by_import, node_depth DESC, object_id DESC) WHERE (node_depth IS NOT NULL);

CREATE INDEX import_origins_object_idx ON import_origins USING btree (object_id);

CREATE TRIGGER import_origin_node_depth BEFORE INSERT OR UPDATE OF object_id, kind, created_by_import ON import_origins FOR EACH ROW EXECUTE FUNCTION import_origin_node_depth();

CREATE TABLE import_report_rows (
    import_id uuid NOT NULL,
    "position" bigint NOT NULL,
    source_id uuid NOT NULL,
    kind text NOT NULL,
    outcome text NOT NULL,
    warned boolean DEFAULT false NOT NULL,
    "row" jsonb NOT NULL,
    CONSTRAINT import_report_rows_outcome_check CHECK ((outcome = ANY (ARRAY['created'::text, 'kept_both'::text, 'replaced'::text, 'unchanged'::text, 'skipped'::text, 'failed'::text]))),
    CONSTRAINT import_report_rows_pkey PRIMARY KEY (import_id, "position"),
    CONSTRAINT import_report_rows_import_id_fkey FOREIGN KEY (import_id) REFERENCES imports(id) ON DELETE CASCADE
);

CREATE INDEX import_report_rows_kind_idx ON import_report_rows USING btree (import_id, kind, "position");

CREATE INDEX import_report_rows_kind_outcome_idx ON import_report_rows USING btree (import_id, kind, outcome, "position");

CREATE INDEX import_report_rows_outcome_idx ON import_report_rows USING btree (import_id, outcome, "position");

CREATE INDEX import_report_rows_warned_idx ON import_report_rows USING btree (import_id, "position") WHERE warned;

CREATE INDEX import_report_rows_warned_kind_idx ON import_report_rows USING btree (import_id, kind, "position") WHERE warned;

CREATE INDEX import_report_rows_warned_kind_outcome_idx ON import_report_rows USING btree (import_id, kind, outcome, "position") WHERE warned;

CREATE INDEX import_report_rows_warned_outcome_idx ON import_report_rows USING btree (import_id, outcome, "position") WHERE warned;
