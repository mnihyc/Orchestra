-- Documents: blocks, operations, versions, comments, discussions, files and containers.

CREATE TABLE document_containers (
    id uuid NOT NULL,
    node_id uuid NOT NULL,
    org_id uuid NOT NULL,
    parent_container_id uuid,
    owner_id uuid NOT NULL,
    created_by uuid,
    created_by_thread uuid,
    created_by_agent text,
    name text NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    deleted_at timestamp with time zone,
    deleted_by uuid,
    deleted_in uuid,
    CONSTRAINT document_containers_check CHECK ((parent_container_id IS DISTINCT FROM id)),
    CONSTRAINT document_containers_name_check CHECK (((char_length(name) >= 1) AND (char_length(name) <= 200))),
    CONSTRAINT document_containers_id_node_id_org_id_key UNIQUE (id, node_id, org_id),
    CONSTRAINT document_containers_pkey PRIMARY KEY (id),
    CONSTRAINT document_containers_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL,
    CONSTRAINT document_containers_created_by_thread_fkey FOREIGN KEY (created_by_thread) REFERENCES threads(node_id) ON DELETE SET NULL,
    CONSTRAINT document_containers_deleted_by_fkey FOREIGN KEY (deleted_by) REFERENCES users(id) ON DELETE SET NULL,
    CONSTRAINT document_containers_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE,
    CONSTRAINT document_containers_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id) ON DELETE CASCADE,
    CONSTRAINT document_containers_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES users(id),
    CONSTRAINT document_containers_parent_container_id_node_id_org_id_fkey FOREIGN KEY (parent_container_id, node_id, org_id) REFERENCES document_containers(id, node_id, org_id) DEFERRABLE INITIALLY DEFERRED
);

CREATE UNIQUE INDEX document_containers_live_name_idx ON document_containers USING btree (node_id, parent_container_id, translate(name, 'ABCDEFGHIJKLMNOPQRSTUVWXYZ'::text, 'abcdefghijklmnopqrstuvwxyz'::text)) NULLS NOT DISTINCT WHERE (deleted_at IS NULL);

CREATE INDEX document_containers_parent_idx ON document_containers USING btree (node_id, parent_container_id, name, id);

CREATE INDEX related_container_name_idx ON document_containers USING btree (node_id, parent_container_id, orch_related_name_key(translate(name, 'ABCDEFGHIJKLMNOPQRSTUVWXYZ'::text, 'abcdefghijklmnopqrstuvwxyz'::text))) WHERE (deleted_at IS NULL);

CREATE TRIGGER document_containers_structure BEFORE INSERT OR DELETE OR UPDATE ON document_containers FOR EACH ROW EXECUTE FUNCTION document_tree_structure();

CREATE TABLE document_model_inputs (
    model_input_id uuid NOT NULL,
    document_id uuid NOT NULL,
    CONSTRAINT document_model_inputs_pkey PRIMARY KEY (model_input_id),
    CONSTRAINT document_model_inputs_model_input_id_fkey FOREIGN KEY (model_input_id) REFERENCES model_inputs(id) ON DELETE CASCADE
);

CREATE INDEX document_model_inputs_document ON document_model_inputs USING btree (document_id);

CREATE TABLE document_notice_slots (
    slot smallint NOT NULL,
    job_id uuid,
    CONSTRAINT document_notice_slots_slot_check CHECK (((slot >= 1) AND (slot <= 4))),
    CONSTRAINT document_notice_slots_pkey PRIMARY KEY (slot),
    CONSTRAINT document_notice_slots_job_id_fkey FOREIGN KEY (job_id) REFERENCES jobs(id) ON DELETE SET NULL
);

INSERT INTO document_notice_slots (slot) SELECT generate_series(1, 4);

CREATE TABLE document_tree_operations (
    actor_kind text NOT NULL,
    actor_id uuid NOT NULL,
    request_ref text NOT NULL,
    arguments jsonb NOT NULL,
    operation_id uuid NOT NULL,
    result jsonb NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT document_tree_operations_actor_kind_check CHECK ((actor_kind = ANY (ARRAY['person'::text, 'agent'::text]))),
    CONSTRAINT document_tree_operations_operation_id_key UNIQUE (operation_id),
    CONSTRAINT document_tree_operations_pkey PRIMARY KEY (actor_kind, actor_id, request_ref)
);

CREATE TABLE document_tree_scopes (
    node_id uuid NOT NULL,
    seq bigint DEFAULT 0 NOT NULL,
    CONSTRAINT document_tree_scopes_pkey PRIMARY KEY (node_id),
    CONSTRAINT document_tree_scopes_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE
);

CREATE TABLE documents (
    id uuid NOT NULL,
    node_id uuid NOT NULL,
    org_id uuid NOT NULL,
    owner_id uuid NOT NULL,
    created_by uuid,
    created_by_thread uuid,
    created_by_agent text,
    title text NOT NULL,
    parent_container_id uuid,
    basename text DEFAULT ''::text NOT NULL,
    deleted_in uuid,
    doc_seq bigint DEFAULT 0 NOT NULL,
    seq_updated bigint DEFAULT 0 NOT NULL,
    purged_seq bigint DEFAULT 0 NOT NULL,
    block_count integer DEFAULT 0 NOT NULL,
    bytes bigint DEFAULT 0 NOT NULL,
    files_bytes bigint DEFAULT 0 NOT NULL,
    version integer DEFAULT 0 NOT NULL,
    version_at timestamp with time zone,
    span jsonb DEFAULT '{}'::jsonb NOT NULL,
    told uuid[] DEFAULT '{}'::uuid[] NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_by uuid,
    deleted_at timestamp with time zone,
    deleted_by uuid,
    description text DEFAULT ''::text NOT NULL,
    description_source text DEFAULT 'auto'::text NOT NULL,
    search_file_count bigint DEFAULT 0 NOT NULL,
    CONSTRAINT documents_description_check CHECK ((char_length(description) <= 4000)),
    CONSTRAINT documents_description_source_check CHECK ((description_source = ANY (ARRAY['user'::text, 'auto'::text]))),
    CONSTRAINT documents_search_file_count_check CHECK ((search_file_count >= 0)),
    CONSTRAINT documents_title_check CHECK (((char_length(title) >= 1) AND (char_length(title) <= 200))),
    CONSTRAINT documents_pkey PRIMARY KEY (id),
    CONSTRAINT documents_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL,
    CONSTRAINT documents_created_by_thread_fkey FOREIGN KEY (created_by_thread) REFERENCES threads(node_id) ON DELETE SET NULL,
    CONSTRAINT documents_deleted_by_fkey FOREIGN KEY (deleted_by) REFERENCES users(id),
    CONSTRAINT documents_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE,
    CONSTRAINT documents_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id) ON DELETE CASCADE,
    CONSTRAINT documents_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES users(id),
    CONSTRAINT documents_parent_container_id_node_id_org_id_fkey FOREIGN KEY (parent_container_id, node_id, org_id) REFERENCES document_containers(id, node_id, org_id) DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT documents_updated_by_fkey FOREIGN KEY (updated_by) REFERENCES users(id)
);

CREATE INDEX documents_container_idx ON documents USING btree (node_id, parent_container_id, basename, id);

CREATE INDEX documents_everything_parent_idx ON documents USING btree (((node_id)::text), lower(title), id) WHERE (deleted_at IS NULL);

CREATE UNIQUE INDEX documents_live_name_idx ON documents USING btree (node_id, parent_container_id, translate(basename, 'ABCDEFGHIJKLMNOPQRSTUVWXYZ'::text, 'abcdefghijklmnopqrstuvwxyz'::text)) NULLS NOT DISTINCT WHERE (deleted_at IS NULL);

CREATE INDEX documents_node_idx ON documents USING btree (node_id, updated_at DESC) WHERE (deleted_at IS NULL);

CREATE INDEX documents_node_page_idx ON documents USING btree (node_id, ((deleted_at IS NOT NULL)), updated_at DESC, id DESC);

CREATE INDEX documents_org_idx ON documents USING btree (org_id) WHERE (deleted_at IS NULL);

CREATE INDEX related_document_basename_idx ON documents USING btree (node_id, parent_container_id, orch_related_name_key(translate(basename, 'ABCDEFGHIJKLMNOPQRSTUVWXYZ'::text, 'abcdefghijklmnopqrstuvwxyz'::text))) WHERE (deleted_at IS NULL);

CREATE INDEX related_document_title_idx ON documents USING btree (node_id, parent_container_id, orch_related_name_key(translate(title, 'ABCDEFGHIJKLMNOPQRSTUVWXYZ'::text, 'abcdefghijklmnopqrstuvwxyz'::text))) WHERE (deleted_at IS NULL);

CREATE INDEX related_documents_folder_recent_idx ON documents USING btree (parent_container_id, updated_at DESC, id) WHERE ((deleted_at IS NULL) AND (parent_container_id IS NOT NULL));

CREATE INDEX related_documents_recent_idx ON documents USING btree (updated_at DESC, id) WHERE (deleted_at IS NULL);

CREATE INDEX search_descriptions_recent_idx ON documents USING btree (updated_at DESC, id DESC) WHERE ((deleted_at IS NULL) AND (description <> ''::text));

CREATE INDEX search_documents_created_idx ON documents USING btree (created_at, id) WHERE (deleted_at IS NULL);

CREATE INDEX search_documents_creator_recent_idx ON documents USING btree (created_by, updated_at DESC, id DESC) WHERE (deleted_at IS NULL);

CREATE INDEX search_documents_description_trgm_idx ON documents USING gin (description gin_trgm_ops) WHERE ((deleted_at IS NULL) AND (description <> ''::text));

CREATE INDEX search_documents_editor_recent_idx ON documents USING btree (updated_by, updated_at DESC, id DESC) WHERE (deleted_at IS NULL);

CREATE INDEX search_documents_files_recent_idx ON documents USING btree (updated_at DESC, id DESC) WHERE ((deleted_at IS NULL) AND (search_file_count > 0));

CREATE INDEX search_documents_owner_idx ON documents USING btree (owner_id) WHERE (deleted_at IS NULL);

CREATE INDEX search_documents_owner_recent_idx ON documents USING btree (owner_id, updated_at DESC, id DESC) WHERE (deleted_at IS NULL);

CREATE INDEX search_documents_parent_idx ON documents USING btree (parent_container_id, id) WHERE ((parent_container_id IS NOT NULL) AND (deleted_at IS NULL));

CREATE INDEX search_documents_recent_idx ON documents USING btree (updated_at DESC, id DESC) WHERE (deleted_at IS NULL);

CREATE INDEX search_documents_scope_recent_idx ON documents USING btree (node_id, updated_at DESC, id DESC) WHERE (deleted_at IS NULL);

CREATE INDEX search_documents_title_trgm_idx ON documents USING gin (title gin_trgm_ops) WHERE (deleted_at IS NULL);

CREATE TRIGGER documents_structure_delete BEFORE DELETE ON documents FOR EACH ROW EXECUTE FUNCTION document_tree_structure();

CREATE TRIGGER documents_structure_insert BEFORE INSERT ON documents FOR EACH ROW EXECUTE FUNCTION document_tree_structure();

CREATE TRIGGER documents_structure_update BEFORE UPDATE OF node_id, parent_container_id, title, basename, deleted_at ON documents FOR EACH ROW EXECUTE FUNCTION document_tree_structure();

CREATE TRIGGER participation_documents_delete AFTER DELETE ON documents REFERENCING OLD TABLE AS participation_old_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER participation_documents_insert AFTER INSERT ON documents REFERENCING NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

CREATE TRIGGER participation_documents_update AFTER UPDATE ON documents REFERENCING OLD TABLE AS participation_old_objects NEW TABLE AS participation_new_objects FOR EACH STATEMENT EXECUTE FUNCTION orch_participation_objects_changed();

ALTER TABLE nodes ADD CONSTRAINT nodes_readme_fk FOREIGN KEY (readme_document_id) REFERENCES documents(id) ON DELETE SET NULL;

CREATE TABLE document_blocks (
    id uuid NOT NULL,
    document_id uuid NOT NULL,
    parent_id uuid,
    "position" numeric NOT NULL,
    type text NOT NULL,
    props jsonb DEFAULT '{}'::jsonb NOT NULL,
    content jsonb DEFAULT '[]'::jsonb NOT NULL,
    text text DEFAULT ''::text NOT NULL,
    seq_created bigint NOT NULL,
    seq_updated bigint NOT NULL,
    created_by uuid,
    updated_by uuid,
    updated_by_thread uuid,
    updated_by_agent text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    removed_at timestamp with time zone,
    CONSTRAINT document_blocks_type_check CHECK ((type = ANY (ARRAY['paragraph'::text, 'heading'::text, 'bullet'::text, 'numbered'::text, 'todo'::text, 'toggle'::text, 'quote'::text, 'callout'::text, 'code'::text, 'math'::text, 'diagram'::text, 'divider'::text, 'page_break'::text, 'image'::text, 'video'::text, 'audio'::text, 'file'::text, 'table'::text, 'columns'::text, 'column'::text]))),
    CONSTRAINT document_blocks_pkey PRIMARY KEY (id),
    CONSTRAINT document_blocks_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id),
    CONSTRAINT document_blocks_document_id_fkey FOREIGN KEY (document_id) REFERENCES documents(id) ON DELETE CASCADE,
    CONSTRAINT document_blocks_parent_id_fkey FOREIGN KEY (parent_id) REFERENCES document_blocks(id) ON DELETE CASCADE,
    CONSTRAINT document_blocks_updated_by_fkey FOREIGN KEY (updated_by) REFERENCES users(id),
    CONSTRAINT document_blocks_updated_by_thread_fkey FOREIGN KEY (updated_by_thread) REFERENCES threads(node_id) ON DELETE SET NULL
);

CREATE INDEX document_blocks_description_idx ON document_blocks USING btree (document_id, "position", id) WHERE ((parent_id IS NULL) AND (removed_at IS NULL) AND (type = 'paragraph'::text) AND ((btrim(text) <> ''::text) OR jsonb_path_exists(content, '$[*]?((@."type" == "ref" && @."label".type() == "string") && @."label" like_regex "[^ ]")'::jsonpath)));

CREATE INDEX document_blocks_document_expiry_idx ON document_blocks USING btree (document_id, removed_at) WHERE (removed_at IS NOT NULL);

CREATE INDEX document_blocks_removed_expiry_idx ON document_blocks USING btree (removed_at, document_id) WHERE (removed_at IS NOT NULL);

CREATE INDEX document_blocks_seq_idx ON document_blocks USING btree (document_id, seq_updated);

CREATE INDEX document_blocks_text_idx ON document_blocks USING gin (text gin_trgm_ops) WHERE (removed_at IS NULL);

CREATE INDEX document_blocks_tree_idx ON document_blocks USING btree (document_id, parent_id, "position") WHERE (removed_at IS NULL);

CREATE INDEX search_blocks_recent_idx ON document_blocks USING btree (updated_at DESC, id DESC) WHERE (removed_at IS NULL);

CREATE INDEX search_blocks_scope_recent_idx ON document_blocks USING btree (document_id, updated_at DESC, id DESC) INCLUDE (type) WHERE (removed_at IS NULL);

CREATE INDEX search_headings_recent_idx ON document_blocks USING btree (updated_at DESC, id DESC) WHERE ((removed_at IS NULL) AND (type = 'heading'::text));

CREATE INDEX search_headings_text_trgm_idx ON document_blocks USING gin (text gin_trgm_ops) WHERE ((removed_at IS NULL) AND (type = 'heading'::text));

CREATE STATISTICS document_blocks_expiry_distribution (dependencies, mcv) ON document_id, removed_at FROM document_blocks;

CREATE TRIGGER document_summary_on_write AFTER INSERT OR UPDATE OF content, props, type, removed_at, parent_id, "position" ON document_blocks FOR EACH ROW EXECUTE FUNCTION orch_document_summary_due();

CREATE TRIGGER related_document_references AFTER INSERT OR DELETE OR UPDATE OF content, removed_at ON document_blocks FOR EACH ROW EXECUTE FUNCTION orch_related_reference_refresh();

CREATE TRIGGER related_document_references_moment AFTER UPDATE OF updated_at ON document_blocks FOR EACH ROW WHEN (((old.updated_at IS DISTINCT FROM new.updated_at) AND (NOT (old.content IS DISTINCT FROM new.content)) AND (NOT (old.removed_at IS DISTINCT FROM new.removed_at)))) EXECUTE FUNCTION orch_document_reference_moments();

CREATE TABLE document_discussions (
    id uuid NOT NULL,
    document_id uuid NOT NULL,
    block_id uuid,
    quote text DEFAULT ''::text NOT NULL,
    resolved_at timestamp with time zone,
    resolved_by uuid,
    created_by uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    seq_updated bigint NOT NULL,
    latest_comment_at timestamp with time zone,
    last_activity_at timestamp with time zone NOT NULL,
    CONSTRAINT document_discussions_quote_check CHECK ((char_length(quote) <= 400)),
    CONSTRAINT document_discussions_pkey PRIMARY KEY (id),
    CONSTRAINT document_discussions_block_id_fkey FOREIGN KEY (block_id) REFERENCES document_blocks(id) ON DELETE SET NULL,
    CONSTRAINT document_discussions_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id),
    CONSTRAINT document_discussions_document_id_fkey FOREIGN KEY (document_id) REFERENCES documents(id) ON DELETE CASCADE,
    CONSTRAINT document_discussions_resolved_by_fkey FOREIGN KEY (resolved_by) REFERENCES users(id)
);

CREATE INDEX document_discussions_activity_idx ON document_discussions USING btree (document_id, last_activity_at DESC, id DESC);

CREATE INDEX document_discussions_open_activity_idx ON document_discussions USING btree (document_id, last_activity_at DESC, id DESC) WHERE (resolved_at IS NULL);

CREATE INDEX document_discussions_resolved_activity_idx ON document_discussions USING btree (document_id, last_activity_at DESC, id DESC) WHERE (resolved_at IS NOT NULL);

CREATE INDEX document_discussions_seq_idx ON document_discussions USING btree (document_id, seq_updated, id);

CREATE TRIGGER document_discussion_activity_started BEFORE INSERT OR UPDATE OF created_at ON document_discussions FOR EACH ROW EXECUTE FUNCTION document_discussion_activity_started();

CREATE TRIGGER document_discussion_participant AFTER INSERT ON document_discussions FOR EACH ROW EXECUTE FUNCTION document_discussion_participant_added();

CREATE TABLE document_files (
    id uuid NOT NULL,
    document_id uuid NOT NULL,
    blob bytea NOT NULL,
    name text NOT NULL,
    media_type text NOT NULL,
    size bigint NOT NULL,
    is_text boolean NOT NULL,
    added_by uuid,
    added_by_thread uuid,
    added_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT document_files_document_id_name_key UNIQUE (document_id, name),
    CONSTRAINT document_files_pkey PRIMARY KEY (id),
    CONSTRAINT document_files_added_by_fkey FOREIGN KEY (added_by) REFERENCES users(id),
    CONSTRAINT document_files_added_by_thread_fkey FOREIGN KEY (added_by_thread) REFERENCES threads(node_id) ON DELETE SET NULL,
    CONSTRAINT document_files_blob_fkey FOREIGN KEY (blob) REFERENCES blobs(hash),
    CONSTRAINT document_files_document_id_fkey FOREIGN KEY (document_id) REFERENCES documents(id) ON DELETE CASCADE
);

CREATE INDEX document_files_name_idx ON document_files USING gin (name gin_trgm_ops);

CREATE INDEX search_files_family_name_idx ON document_files USING btree (split_part(lower(media_type), '/'::text, 1), document_id, name) WHERE (POSITION(('/'::text) IN (media_type)) > 0);

CREATE INDEX search_files_media_name_idx ON document_files USING btree (lower(media_type), document_id, name);

CREATE INDEX search_files_recent_idx ON document_files USING btree (added_at DESC, document_id DESC);

CREATE TRIGGER search_files_added AFTER INSERT ON document_files REFERENCING NEW TABLE AS added_files FOR EACH STATEMENT EXECUTE FUNCTION orch_search_files_added();

CREATE TRIGGER search_files_moved AFTER UPDATE ON document_files REFERENCING OLD TABLE AS removed_files NEW TABLE AS added_files FOR EACH STATEMENT EXECUTE FUNCTION orch_search_files_moved();

CREATE TRIGGER search_files_removed AFTER DELETE ON document_files REFERENCING OLD TABLE AS removed_files FOR EACH STATEMENT EXECUTE FUNCTION orch_search_files_removed();

CREATE TABLE document_links_display (
    document_id uuid NOT NULL,
    shown text[] NOT NULL,
    set_by uuid,
    set_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT document_links_display_shown_check CHECK ((shown <@ ARRAY['referenced_by'::text, 'references'::text, 'related'::text, 'graph'::text])),
    CONSTRAINT document_links_display_pkey PRIMARY KEY (document_id),
    CONSTRAINT document_links_display_document_id_fkey FOREIGN KEY (document_id) REFERENCES documents(id) ON DELETE CASCADE,
    CONSTRAINT document_links_display_set_by_fkey FOREIGN KEY (set_by) REFERENCES users(id) ON DELETE SET NULL
);

CREATE TABLE document_ops (
    document_id uuid NOT NULL,
    author_id uuid NOT NULL,
    client_id uuid NOT NULL,
    seq bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT document_ops_pkey PRIMARY KEY (document_id, author_id, client_id),
    CONSTRAINT document_ops_author_id_fkey FOREIGN KEY (author_id) REFERENCES users(id) ON DELETE CASCADE,
    CONSTRAINT document_ops_document_id_fkey FOREIGN KEY (document_id) REFERENCES documents(id) ON DELETE CASCADE
);

CREATE INDEX document_ops_created_expiry_idx ON document_ops USING btree (created_at, document_id);

CREATE INDEX document_ops_document_expiry_idx ON document_ops USING btree (document_id, created_at);

CREATE STATISTICS document_ops_expiry_distribution (dependencies, mcv) ON document_id, created_at FROM document_ops;

CREATE TABLE document_states (
    user_id uuid NOT NULL,
    document_id uuid NOT NULL,
    discussion_level text,
    seen_seq bigint DEFAULT 0 NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    discussion_subscription_seq bigint DEFAULT nextval('document_discussion_subscription_seq'::regclass),
    CONSTRAINT document_states_discussion_level_check CHECK ((discussion_level = ANY (ARRAY['all'::text, 'involved'::text]))),
    CONSTRAINT document_states_pkey PRIMARY KEY (user_id, document_id),
    CONSTRAINT document_states_document_id_fkey FOREIGN KEY (document_id) REFERENCES documents(id) ON DELETE CASCADE,
    CONSTRAINT document_states_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX document_states_all_discussions_idx ON document_states USING btree (document_id, user_id) WHERE (discussion_level = 'all'::text);

CREATE TRIGGER document_discussion_subscription BEFORE UPDATE OF discussion_level ON document_states FOR EACH ROW EXECUTE FUNCTION document_discussion_subscription_changed();

CREATE TABLE document_summary_state (
    document_id uuid NOT NULL,
    due_at timestamp with time zone,
    next_at timestamp with time zone NOT NULL,
    last_at timestamp with time zone,
    failures bigint DEFAULT 0 NOT NULL,
    claimed_until timestamp with time zone,
    claimed_by uuid,
    run_id uuid,
    reason text,
    CONSTRAINT document_summary_state_check CHECK (((claimed_by IS NULL) = (claimed_until IS NULL))),
    CONSTRAINT document_summary_state_pkey PRIMARY KEY (document_id),
    CONSTRAINT document_summary_state_document_id_fkey FOREIGN KEY (document_id) REFERENCES documents(id) ON DELETE CASCADE
);

CREATE INDEX document_summary_due ON document_summary_state USING btree (next_at, due_at) WHERE (due_at IS NOT NULL);

CREATE TABLE document_versions (
    document_id uuid NOT NULL,
    version integer NOT NULL,
    title text NOT NULL,
    whole jsonb,
    base integer,
    change jsonb,
    stored_bytes bigint NOT NULL,
    whole_bytes bigint NOT NULL,
    bytes bigint NOT NULL,
    cause text NOT NULL,
    authors jsonb DEFAULT '[]'::jsonb NOT NULL,
    changed jsonb DEFAULT '{}'::jsonb NOT NULL,
    from_at timestamp with time zone NOT NULL,
    to_at timestamp with time zone NOT NULL,
    block_count bigint NOT NULL,
    top_blocks bigint NOT NULL,
    CONSTRAINT document_versions_block_count_nonnegative CHECK ((block_count >= 0)),
    CONSTRAINT document_versions_top_blocks_check CHECK (((top_blocks >= 0) AND (top_blocks <= block_count))),
    CONSTRAINT document_versions_held_check CHECK ((((whole IS NOT NULL) AND (base IS NULL) AND (change IS NULL)) OR ((whole IS NULL) AND (base IS NOT NULL) AND (change IS NOT NULL) AND (base < version)))),
    CONSTRAINT document_versions_sizes_check CHECK (((stored_bytes >= 0) AND (whole_bytes >= 0))),
    CONSTRAINT document_versions_cause_check CHECK ((cause = ANY (ARRAY['window'::text, 'idle'::text, 'rewrite'::text, 'restore'::text]))),
    CONSTRAINT document_versions_pkey PRIMARY KEY (document_id, version),
    CONSTRAINT document_versions_document_id_fkey FOREIGN KEY (document_id) REFERENCES documents(id) ON DELETE CASCADE,
    CONSTRAINT document_versions_base_fkey FOREIGN KEY (document_id, base) REFERENCES document_versions(document_id, version)
);

CREATE INDEX document_versions_base_idx ON document_versions USING btree (document_id, base) WHERE (base IS NOT NULL);

COMMENT ON COLUMN document_versions.change IS 'What changed from the version base, when it is held as a change: {"put": [blocks], "gone": [block ids]}.';

CREATE TABLE document_comments (
    id uuid NOT NULL,
    discussion_id uuid NOT NULL,
    document_id uuid NOT NULL,
    author_id uuid NOT NULL,
    content jsonb NOT NULL,
    reactions jsonb DEFAULT '{}'::jsonb NOT NULL,
    edited_at timestamp with time zone,
    deleted_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    seq_updated bigint NOT NULL,
    CONSTRAINT document_comments_pkey PRIMARY KEY (id),
    CONSTRAINT document_comments_author_id_fkey FOREIGN KEY (author_id) REFERENCES users(id),
    CONSTRAINT document_comments_discussion_id_fkey FOREIGN KEY (discussion_id) REFERENCES document_discussions(id) ON DELETE CASCADE,
    CONSTRAINT document_comments_document_id_fkey FOREIGN KEY (document_id) REFERENCES documents(id) ON DELETE CASCADE
);

CREATE INDEX document_comments_author_idx ON document_comments USING btree (discussion_id, author_id);

CREATE INDEX document_comments_discussion_idx ON document_comments USING btree (discussion_id, created_at);

CREATE INDEX document_comments_page_idx ON document_comments USING btree (discussion_id, created_at, id);

CREATE INDEX document_comments_seq_idx ON document_comments USING btree (document_id, seq_updated);

CREATE TRIGGER document_comment_activity_added AFTER INSERT ON document_comments FOR EACH ROW EXECUTE FUNCTION document_comment_activity_changed();

CREATE TRIGGER document_comment_activity_moved AFTER UPDATE OF discussion_id, created_at ON document_comments FOR EACH ROW EXECUTE FUNCTION document_comment_activity_changed();

CREATE TRIGGER document_comment_activity_removed AFTER DELETE ON document_comments FOR EACH ROW EXECUTE FUNCTION document_comment_activity_changed();

CREATE TRIGGER document_comment_participant AFTER INSERT ON document_comments FOR EACH ROW EXECUTE FUNCTION document_discussion_participant_added();

CREATE TRIGGER related_comment_references AFTER INSERT OR DELETE OR UPDATE OF content, deleted_at ON document_comments FOR EACH ROW EXECUTE FUNCTION orch_document_comment_reference_refresh();

CREATE TRIGGER related_comment_references_moment AFTER UPDATE OF created_at ON document_comments FOR EACH ROW WHEN (((old.created_at IS DISTINCT FROM new.created_at) AND (NOT (old.content IS DISTINCT FROM new.content)) AND (NOT (old.deleted_at IS DISTINCT FROM new.deleted_at)))) EXECUTE FUNCTION orch_document_reference_moments();

CREATE TABLE document_discussion_participants (
    discussion_id uuid NOT NULL,
    user_id uuid NOT NULL,
    first_seq bigint NOT NULL,
    CONSTRAINT document_discussion_participants_pkey PRIMARY KEY (discussion_id, user_id) INCLUDE (first_seq),
    CONSTRAINT document_discussion_participants_discussion_id_fkey FOREIGN KEY (discussion_id) REFERENCES document_discussions(id) ON DELETE CASCADE,
    CONSTRAINT document_discussion_participants_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);
