-- References between things, related items, and search with its optional pieces.

CREATE TABLE link_cursors (
    machine_id uuid NOT NULL,
    stream text NOT NULL,
    last_seq bigint NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT link_cursors_pkey PRIMARY KEY (machine_id, stream),
    CONSTRAINT link_cursors_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES machines(id) ON DELETE CASCADE
);

CREATE TABLE related_reference_targets (
    source_kind text NOT NULL,
    source_id uuid NOT NULL,
    target_kind text NOT NULL,
    target_id uuid NOT NULL,
    target_path_key bytea NOT NULL,
    links bigint NOT NULL,
    CONSTRAINT related_reference_targets_links_check CHECK ((links >= 0)),
    CONSTRAINT related_reference_targets_pkey PRIMARY KEY (source_kind, source_id, target_kind, target_id, target_path_key)
);

CREATE INDEX related_comment_targets_page_idx ON related_reference_targets USING btree (source_id, target_kind, target_id, target_path_key) INCLUDE (links) WHERE (source_kind = 'comment'::text);

CREATE INDEX related_document_targets_page_idx ON related_reference_targets USING btree (source_id, target_kind, target_id, target_path_key) INCLUDE (links) WHERE (source_kind = 'document'::text);

CREATE INDEX related_reference_targets_reverse_idx ON related_reference_targets USING btree (target_kind, target_id, target_path_key, source_id, source_kind);

CREATE TABLE related_references (
    source_kind text NOT NULL,
    source_id uuid NOT NULL,
    part_id uuid NOT NULL,
    target_kind text NOT NULL,
    target_id uuid NOT NULL,
    target_path_key bytea DEFAULT '\x'::bytea NOT NULL,
    source_moment timestamp with time zone,
    CONSTRAINT related_references_path_kind_check CHECK (((target_kind = 'environment_file'::text) = (octet_length(target_path_key) = 32))),
    CONSTRAINT related_references_source_kind_check CHECK ((source_kind = ANY (ARRAY['document'::text, 'chat'::text, 'conversation'::text, 'comment'::text]))),
    CONSTRAINT related_references_target_kind_check CHECK ((target_kind = ANY (ARRAY['document'::text, 'node'::text, 'chat'::text, 'file'::text, 'environment_file'::text]))),
    CONSTRAINT related_references_target_path_key_check CHECK ((octet_length(target_path_key) = ANY (ARRAY[0, 32]))),
    CONSTRAINT related_references_pkey PRIMARY KEY (source_kind, part_id, target_kind, target_id, target_path_key)
);

CREATE INDEX related_references_document_moment_idx ON related_references USING btree (source_id, target_kind, target_id, target_path_key, source_moment, part_id, source_kind) WHERE ((source_kind = ANY (ARRAY['document'::text, 'comment'::text])) AND (source_moment IS NOT NULL));

CREATE INDEX related_references_source_idx ON related_references USING btree (source_kind, source_id, target_kind, target_id, target_path_key, part_id);

CREATE INDEX related_references_target_idx ON related_references USING btree (target_kind, target_id, target_path_key, source_id, source_kind);

CREATE TRIGGER related_reference_source_moment BEFORE INSERT OR UPDATE OF source_kind, source_id, part_id ON related_references FOR EACH ROW EXECUTE FUNCTION orch_reference_source_moment();

CREATE TRIGGER related_reference_targets_added AFTER INSERT ON related_references REFERENCING NEW TABLE AS added FOR EACH STATEMENT EXECUTE FUNCTION orch_related_reference_targets_added();

CREATE TRIGGER related_reference_targets_removed AFTER DELETE ON related_references REFERENCING OLD TABLE AS removed FOR EACH STATEMENT EXECUTE FUNCTION orch_related_reference_targets_removed();

CREATE TRIGGER related_reference_targets_updated_1_removed AFTER UPDATE ON related_references REFERENCING OLD TABLE AS removed FOR EACH STATEMENT EXECUTE FUNCTION orch_related_reference_targets_removed();

CREATE TRIGGER related_reference_targets_updated_2_added AFTER UPDATE ON related_references REFERENCING NEW TABLE AS added FOR EACH STATEMENT EXECUTE FUNCTION orch_related_reference_targets_added();

CREATE TABLE search_generations (
    id uuid NOT NULL,
    org_id uuid NOT NULL,
    provider_id uuid NOT NULL,
    provider_fingerprint bytea DEFAULT '\x'::bytea NOT NULL,
    writer_id uuid,
    model_name text NOT NULL,
    asked_dimensions integer,
    dimensions integer,
    query_prefix text DEFAULT ''::text NOT NULL,
    document_prefix text DEFAULT ''::text NOT NULL,
    chunking text NOT NULL,
    probe jsonb DEFAULT '[]'::jsonb NOT NULL,
    state text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    activated_at timestamp with time zone,
    retired_at timestamp with time zone,
    CONSTRAINT search_generations_asked_dimensions_check CHECK ((asked_dimensions > 0)),
    CONSTRAINT search_generations_dimensions_check CHECK ((dimensions > 0)),
    CONSTRAINT search_generations_state_check CHECK ((state = ANY (ARRAY['building'::text, 'active'::text, 'retired'::text]))),
    CONSTRAINT search_generations_pkey PRIMARY KEY (id),
    CONSTRAINT search_generations_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id) ON DELETE CASCADE,
    CONSTRAINT search_generations_writer_id_fkey FOREIGN KEY (writer_id) REFERENCES users(id) ON DELETE SET NULL
);

CREATE UNIQUE INDEX search_generations_active_idx ON search_generations USING btree (org_id) WHERE (state = 'active'::text);

CREATE UNIQUE INDEX search_generations_building_idx ON search_generations USING btree (org_id) WHERE (state = 'building'::text);

CREATE INDEX search_generations_org_idx ON search_generations USING btree (org_id, state);

CREATE TABLE search_indexes (
    org_id uuid NOT NULL,
    state text DEFAULT 'idle'::text NOT NULL,
    reason text,
    detail text,
    claimed_by uuid,
    claimed_until timestamp with time zone,
    rate double precision,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT search_indexes_state_check CHECK ((state = ANY (ARRAY['idle'::text, 'indexing'::text, 'paused'::text, 'waiting'::text]))),
    CONSTRAINT search_indexes_pkey PRIMARY KEY (org_id),
    CONSTRAINT search_indexes_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id) ON DELETE CASCADE
);

CREATE TABLE search_query_vectors (
    org_id uuid NOT NULL,
    generation_id uuid NOT NULL,
    query_hash text NOT NULL,
    embedding real[] NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    CONSTRAINT search_query_vectors_pkey PRIMARY KEY (org_id, generation_id, query_hash),
    CONSTRAINT search_query_vectors_generation_id_fkey FOREIGN KEY (generation_id) REFERENCES search_generations(id) ON DELETE CASCADE,
    CONSTRAINT search_query_vectors_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id) ON DELETE CASCADE
);

CREATE INDEX search_query_vectors_expiry_idx ON search_query_vectors USING btree (expires_at);

CREATE TABLE search_sessions (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    revision bigint NOT NULL,
    state jsonb NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    CONSTRAINT search_sessions_pkey PRIMARY KEY (id),
    CONSTRAINT search_sessions_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX search_sessions_expiry_idx ON search_sessions USING btree (expires_at);

CREATE INDEX search_sessions_user_idx ON search_sessions USING btree (user_id, expires_at DESC, id);

CREATE TABLE search_sources (
    generation_id uuid NOT NULL,
    kind text NOT NULL,
    source_id uuid NOT NULL,
    title text NOT NULL,
    org_id uuid NOT NULL,
    marker bigint NOT NULL,
    changed_at timestamp with time zone NOT NULL,
    chunks integer NOT NULL,
    tokens bigint NOT NULL,
    bytes bigint NOT NULL,
    embedded_for uuid,
    CONSTRAINT search_sources_kind_check CHECK ((kind = ANY (ARRAY['document'::text, 'conversation'::text, 'chat'::text]))),
    CONSTRAINT search_sources_pkey PRIMARY KEY (generation_id, kind, source_id),
    CONSTRAINT search_sources_generation_id_fkey FOREIGN KEY (generation_id) REFERENCES search_generations(id) ON DELETE CASCADE,
    CONSTRAINT search_sources_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id) ON DELETE CASCADE
);

CREATE INDEX search_sources_org_idx ON search_sources USING btree (org_id, changed_at DESC);

CREATE TRIGGER search_item_vector_refresh AFTER INSERT OR UPDATE OF marker, title, embedded_for ON search_sources FOR EACH ROW EXECUTE FUNCTION orch_search_item_vector_refresh();

CREATE TABLE search_chunks (
    id uuid NOT NULL,
    generation_id uuid NOT NULL,
    node_id uuid NOT NULL,
    kind text NOT NULL,
    source_id uuid NOT NULL,
    org_id uuid NOT NULL,
    ordinal integer NOT NULL,
    first_part uuid,
    text_hash bytea NOT NULL,
    tokens integer NOT NULL,
    bytes integer NOT NULL,
    CONSTRAINT search_chunks_generation_id_kind_source_id_ordinal_key UNIQUE (generation_id, kind, source_id, ordinal),
    CONSTRAINT search_chunks_pkey PRIMARY KEY (id),
    CONSTRAINT search_chunks_generation_id_fkey FOREIGN KEY (generation_id) REFERENCES search_generations(id) ON DELETE CASCADE,
    CONSTRAINT search_chunks_generation_id_kind_source_id_fkey FOREIGN KEY (generation_id, kind, source_id) REFERENCES search_sources(generation_id, kind, source_id) ON DELETE CASCADE,
    CONSTRAINT search_chunks_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE,
    CONSTRAINT search_chunks_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id) ON DELETE CASCADE
);

CREATE INDEX search_chunks_node_idx ON search_chunks USING btree (node_id, generation_id, id);

CREATE VIEW related_items AS
 SELECT 'document'::text AS kind,
    d.id AS source_id,
    d.node_id,
    d.org_id,
    d.parent_container_id,
    d.title AS name,
    d.updated_at AS changed_at,
    d.doc_seq AS marker
   FROM documents d
  WHERE ((d.deleted_at IS NULL) AND (EXISTS ( SELECT 1
           FROM nodes live
          WHERE ((live.id = d.node_id) AND (live.deleted_at IS NULL))
         OFFSET 0)))
UNION ALL
 SELECT 'conversation'::text AS kind,
    n.id AS source_id,
    n.id AS node_id,
    n.org_id,
    NULL::uuid AS parent_container_id,
    n.name,
    GREATEST(n.updated_at, n.activity_at) AS changed_at,
    t.next_seq AS marker
   FROM (nodes n
     CROSS JOIN LATERAL ( SELECT t_1.next_seq
           FROM threads t_1
          WHERE (t_1.node_id = n.id)
         OFFSET 0) t)
  WHERE ((n.deleted_at IS NULL) AND (n.kind = 'thread'::text))
UNION ALL
 SELECT 'chat'::text AS kind,
    c.id AS source_id,
    COALESCE(c.node_id, c.org_id) AS node_id,
    c.org_id,
    NULL::uuid AS parent_container_id,
    COALESCE(c.name, n.name, ''::text) AS name,
    COALESCE(c.last_message_at, c.created_at) AS changed_at,
    c.next_seq AS marker
   FROM (chats c
     LEFT JOIN LATERAL ( SELECT n_1.name,
            n_1.deleted_at
           FROM nodes n_1
          WHERE (n_1.id = c.node_id)
         OFFSET 0) n ON (true))
  WHERE ((c.node_id IS NULL) OR (n.deleted_at IS NULL));

CREATE VIEW search_fields AS
 SELECT 'name'::text AS kind,
    n.id AS source_id,
    n.id AS part_id,
    n.id AS node_id,
    NULL::uuid AS block_id,
    'name'::text AS field,
    4 AS tier,
    n.name AS text,
    GREATEST(n.updated_at, n.activity_at) AS at,
    NULL::uuid AS author_id,
    NULL::text AS item_kind,
    false AS has_file,
    NULL::jsonb AS mentions
   FROM nodes n
  WHERE (n.deleted_at IS NULL)
UNION ALL
 SELECT 'name'::text AS kind,
    n.id AS source_id,
    n.id AS part_id,
    n.id AS node_id,
    NULL::uuid AS block_id,
    'description'::text AS field,
    3 AS tier,
    n.description AS text,
    GREATEST(n.updated_at, n.activity_at) AS at,
    NULL::uuid AS author_id,
    NULL::text AS item_kind,
    false AS has_file,
    NULL::jsonb AS mentions
   FROM nodes n
  WHERE ((n.deleted_at IS NULL) AND (n.description <> ''::text))
UNION ALL
 SELECT 'document'::text AS kind,
    d.id AS source_id,
    d.id AS part_id,
    d.node_id,
    NULL::uuid AS block_id,
    'title'::text AS field,
    4 AS tier,
    d.title AS text,
    d.updated_at AS at,
    d.updated_by AS author_id,
    NULL::text AS item_kind,
    (EXISTS ( SELECT 1
           FROM document_files f
          WHERE (f.document_id = d.id))) AS has_file,
    NULL::jsonb AS mentions
   FROM documents d
  WHERE (d.deleted_at IS NULL)
UNION ALL
 SELECT 'document'::text AS kind,
    d.id AS source_id,
    d.id AS part_id,
    d.node_id,
    NULL::uuid AS block_id,
    'description'::text AS field,
    3 AS tier,
    d.description AS text,
    d.updated_at AS at,
    d.updated_by AS author_id,
    NULL::text AS item_kind,
    (EXISTS ( SELECT 1
           FROM document_files f
          WHERE (f.document_id = d.id))) AS has_file,
    NULL::jsonb AS mentions
   FROM documents d
  WHERE ((d.deleted_at IS NULL) AND (d.description <> ''::text))
UNION ALL
 SELECT 'document'::text AS kind,
    d.id AS source_id,
    d.id AS part_id,
    d.node_id,
    NULL::uuid AS block_id,
    'file'::text AS field,
    2 AS tier,
    f.name AS text,
    f.added_at AS at,
    f.added_by AS author_id,
    NULL::text AS item_kind,
    true AS has_file,
    NULL::jsonb AS mentions
   FROM (document_files f
     JOIN documents d ON ((d.id = f.document_id)))
  WHERE (d.deleted_at IS NULL)
UNION ALL
 SELECT 'document'::text AS kind,
    b.document_id AS source_id,
    b.id AS part_id,
    d.node_id,
    b.id AS block_id,
    'heading'::text AS field,
    2 AS tier,
    b.text,
    b.updated_at AS at,
    b.updated_by AS author_id,
    NULL::text AS item_kind,
    (EXISTS ( SELECT 1
           FROM document_files f
          WHERE (f.document_id = d.id))) AS has_file,
    b.content AS mentions
   FROM (document_blocks b
     JOIN documents d ON ((d.id = b.document_id)))
  WHERE ((b.removed_at IS NULL) AND (d.deleted_at IS NULL) AND (b.type = 'heading'::text))
UNION ALL
 SELECT 'document'::text AS kind,
    b.document_id AS source_id,
    b.id AS part_id,
    d.node_id,
    b.id AS block_id,
    'block'::text AS field,
    0 AS tier,
    b.text,
    b.updated_at AS at,
    b.updated_by AS author_id,
    NULL::text AS item_kind,
    (EXISTS ( SELECT 1
           FROM document_files f
          WHERE (f.document_id = d.id))) AS has_file,
    b.content AS mentions
   FROM (document_blocks b
     JOIN documents d ON ((d.id = b.document_id)))
  WHERE ((b.removed_at IS NULL) AND (d.deleted_at IS NULL) AND (b.type <> 'heading'::text))
UNION ALL
 SELECT 'chat_message'::text AS kind,
    m.chat_id AS source_id,
    m.id AS part_id,
    COALESCE(c.node_id, c.org_id) AS node_id,
    NULL::uuid AS block_id,
    'chat'::text AS field,
    0 AS tier,
    m.search_text AS text,
    m.created_at AS at,
    m.author_id,
    NULL::text AS item_kind,
    jsonb_path_exists(m.content, '$[*]?((((@."type" == "attachment" || @."type" == "pasted_text") || @."type" == "image") || exists (@."attachments"[0])) || exists (@."images"[0]))'::jsonpath) AS has_file,
    to_jsonb(m.mentions) AS mentions
   FROM (chat_messages m
     JOIN chats c ON ((c.id = m.chat_id)))
  WHERE ((m.deleted_at IS NULL) AND (m.search_text IS NOT NULL))
UNION ALL
 SELECT 'item'::text AS kind,
    i.thread_id AS source_id,
    i.id AS part_id,
    i.thread_id AS node_id,
    NULL::uuid AS block_id,
    'item'::text AS field,
    0 AS tier,
    i.search_text AS text,
    i.created_at AS at,
    i.author_id,
    i.kind AS item_kind,
    jsonb_path_exists(i.payload, '$."parts"[*]?(((@."type" == "file" || @."type" == "image") || exists (@."attachments"[0])) || exists (@."images"[0]))'::jsonpath) AS has_file,
    i.payload AS mentions
   FROM items i
  WHERE ((i.search_text IS NOT NULL) AND (i.kind <> 'marker'::text) AND (i.search_text !~~ '%<orch>%'::text))
UNION ALL
 SELECT 'item'::text AS kind,
    i.thread_id AS source_id,
    i.id AS part_id,
    i.thread_id AS node_id,
    NULL::uuid AS block_id,
    'revision'::text AS field,
    0 AS tier,
    (r.body ->> 'search_text'::text) AS text,
    i.created_at AS at,
    i.author_id,
    i.kind AS item_kind,
    jsonb_path_exists((r.body -> 'payload'::text), '$."parts"[*]?(((@."type" == "file" || @."type" == "image") || exists (@."attachments"[0])) || exists (@."images"[0]))'::jsonpath) AS has_file,
    (r.body -> 'payload'::text) AS mentions
   FROM (thread_item_revisions r
     JOIN items i ON ((i.id = r.item_id)))
  WHERE (((r.body ->> 'search_text'::text) IS NOT NULL) AND (i.kind <> 'marker'::text) AND ((r.body ->> 'search_text'::text) !~~ '%<orch>%'::text));

-- Search by meaning stays optional: it is set up now where pgvector can be had (search_vectors and, for related
-- items, search_item_vectors with its per-generation indexes); where it cannot, the reason is said once, here.
DO $$
DECLARE
  state text := orch_search_vectors_setup();
BEGIN
  IF state = 'absent' THEN
    RAISE NOTICE 'pgvector (the extension "vector") is not available in this database: search by meaning stays unavailable and everything else works';
  ELSIF state = 'not_permitted' THEN
    RAISE NOTICE 'this database user may not create the extension "vector": search by meaning stays unavailable';
  END IF;
END
$$;

DO $$
BEGIN
  IF to_regclass('search_item_vectors') IS NOT NULL THEN
    CREATE INDEX search_item_vectors_source_idx ON search_item_vectors(generation_id,source_id);
  END IF;
END
$$;

-- Short literals: native bigrams where pg_bigm can be had, portable exact Unicode grams everywhere else.
DO $$
DECLARE ready boolean:=false; entry record;
BEGIN
 IF EXISTS(SELECT 1 FROM pg_available_extensions WHERE name='pg_bigm') THEN
  BEGIN CREATE EXTENSION IF NOT EXISTS pg_bigm; ready:=true;
  EXCEPTION WHEN insufficient_privilege THEN RAISE NOTICE 'pg_bigm is not permitted; short search uses portable Unicode grams'; END;
 ELSE RAISE NOTICE 'pg_bigm is absent; short search uses portable Unicode grams'; END IF;
 FOR entry IN SELECT * FROM (VALUES
  ('search_nodes_name_short_idx','nodes','name','deleted_at IS NULL'),
  ('search_nodes_description_short_idx','nodes','description','deleted_at IS NULL'),
  ('search_documents_title_short_idx','documents','title','deleted_at IS NULL'),
  ('search_documents_description_short_idx','documents','description','deleted_at IS NULL'),
  ('search_files_name_short_idx','document_files','name','true'),
  ('search_blocks_text_short_idx','document_blocks','text','removed_at IS NULL'),
  ('search_headings_text_short_idx','document_blocks','text','removed_at IS NULL AND type=''heading'''),
  ('search_messages_text_short_idx','chat_messages','search_text','deleted_at IS NULL AND search_text IS NOT NULL'),
  ('search_items_text_short_idx','items','search_text','search_text IS NOT NULL AND kind<>''marker''')
 ) target(index_name,table_name,column_name,predicate) LOOP
  IF ready THEN EXECUTE format('CREATE INDEX %I ON %I USING gin(lower(%I) gin_bigm_ops) WHERE %s',entry.index_name,entry.table_name,entry.column_name,entry.predicate);
  ELSE EXECUTE format('CREATE INDEX %I ON %I USING gin(orch_search_grams(%I)) WHERE %s',entry.index_name,entry.table_name,entry.column_name,entry.predicate); END IF;
 END LOOP;
 IF ready THEN CREATE INDEX search_revisions_text_short_idx ON thread_item_revisions USING gin(lower(body->>'search_text') gin_bigm_ops) WHERE body->>'search_text' IS NOT NULL;
 ELSE CREATE INDEX search_revisions_text_short_idx ON thread_item_revisions USING gin(orch_search_grams(body->>'search_text')) WHERE body->>'search_text' IS NOT NULL; END IF;
END $$;
