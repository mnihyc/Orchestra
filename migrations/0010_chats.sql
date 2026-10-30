-- Chats, their members and messages.

CREATE TABLE chats (
    id uuid NOT NULL,
    kind text NOT NULL,
    node_id uuid,
    org_id uuid NOT NULL,
    direct_key text,
    name text,
    created_by uuid,
    next_seq bigint DEFAULT 1 NOT NULL,
    seq_state bigint DEFAULT 0 NOT NULL,
    last_message_seq bigint DEFAULT 0 NOT NULL,
    last_message_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chats_check CHECK (((kind = 'node'::text) = (node_id IS NOT NULL))),
    CONSTRAINT chats_check1 CHECK (((kind = 'direct'::text) = (direct_key IS NOT NULL))),
    CONSTRAINT chats_check2 CHECK (((kind = 'group'::text) OR (name IS NULL))),
    CONSTRAINT chats_kind_check CHECK ((kind = ANY (ARRAY['node'::text, 'direct'::text, 'group'::text]))),
    CONSTRAINT chats_name_check CHECK (((char_length(name) >= 1) AND (char_length(name) <= 80))),
    CONSTRAINT chats_node_id_key UNIQUE (node_id),
    CONSTRAINT chats_org_id_direct_key_key UNIQUE (org_id, direct_key),
    CONSTRAINT chats_pkey PRIMARY KEY (id),
    CONSTRAINT chats_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id),
    CONSTRAINT chats_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE,
    CONSTRAINT chats_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id) ON DELETE CASCADE
);

CREATE INDEX chats_everything_parent_idx ON chats USING btree (((COALESCE(node_id, org_id))::text), id);

CREATE INDEX chats_org_idx ON chats USING btree (org_id, last_message_at DESC) WHERE (kind <> 'node'::text);

CREATE INDEX related_chats_recent_idx ON chats USING btree (COALESCE(last_message_at, created_at) DESC, id);

ALTER TABLE invitations ADD CONSTRAINT invitations_chat_fk FOREIGN KEY (chat_id) REFERENCES chats(id) ON DELETE CASCADE;

CREATE TABLE chat_agent_posting (
    chat_id uuid NOT NULL,
    thread_id uuid NOT NULL,
    stopped boolean DEFAULT true NOT NULL,
    changed_by uuid NOT NULL,
    changed_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chat_agent_posting_pkey PRIMARY KEY (chat_id, thread_id),
    CONSTRAINT chat_agent_posting_changed_by_fkey FOREIGN KEY (changed_by) REFERENCES users(id),
    CONSTRAINT chat_agent_posting_chat_id_fkey FOREIGN KEY (chat_id) REFERENCES chats(id) ON DELETE CASCADE,
    CONSTRAINT chat_agent_posting_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE CASCADE
);

CREATE TABLE chat_members (
    chat_id uuid NOT NULL,
    user_id uuid NOT NULL,
    added_by uuid,
    joined_at timestamp with time zone DEFAULT now() NOT NULL,
    left_at timestamp with time zone,
    guest boolean DEFAULT false NOT NULL,
    seq_updated bigint NOT NULL,
    expires_at timestamp with time zone,
    CONSTRAINT chat_members_pkey PRIMARY KEY (chat_id, user_id),
    CONSTRAINT chat_members_added_by_fkey FOREIGN KEY (added_by) REFERENCES users(id),
    CONSTRAINT chat_members_chat_id_fkey FOREIGN KEY (chat_id) REFERENCES chats(id) ON DELETE CASCADE,
    CONSTRAINT chat_members_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX chat_members_expiry_idx ON chat_members USING btree (expires_at) WHERE ((expires_at IS NOT NULL) AND (left_at IS NULL));

CREATE INDEX chat_members_user_idx ON chat_members USING btree (user_id) WHERE (left_at IS NULL);

CREATE INDEX related_chat_label_members_idx ON chat_members USING btree (chat_id, expires_at, user_id) WHERE (left_at IS NULL);

CREATE TABLE chat_messages (
    id uuid NOT NULL,
    chat_id uuid NOT NULL,
    kind text DEFAULT 'message'::text NOT NULL,
    author_id uuid,
    author_thread_id uuid,
    author_agent text,
    content jsonb NOT NULL,
    reply_to_id uuid,
    mentions uuid[] DEFAULT '{}'::uuid[] NOT NULL,
    mentions_all boolean DEFAULT false NOT NULL,
    mention_targets jsonb DEFAULT '[]'::jsonb NOT NULL,
    asked_agents jsonb DEFAULT '[]'::jsonb NOT NULL,
    reactions jsonb DEFAULT '{}'::jsonb NOT NULL,
    client_id text,
    search_text text,
    edited_at timestamp with time zone,
    pinned_at timestamp with time zone,
    pinned_by uuid,
    deleted_at timestamp with time zone,
    deleted_by uuid,
    seq_created bigint NOT NULL,
    seq_updated bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chat_messages_kind_check CHECK ((kind = ANY (ARRAY['message'::text, 'system'::text]))),
    CONSTRAINT chat_messages_chat_id_author_id_client_id_key UNIQUE (chat_id, author_id, client_id),
    CONSTRAINT chat_messages_pkey PRIMARY KEY (id),
    CONSTRAINT chat_messages_author_id_fkey FOREIGN KEY (author_id) REFERENCES users(id),
    CONSTRAINT chat_messages_author_thread_id_fkey FOREIGN KEY (author_thread_id) REFERENCES threads(node_id) ON DELETE SET NULL,
    CONSTRAINT chat_messages_chat_id_fkey FOREIGN KEY (chat_id) REFERENCES chats(id) ON DELETE CASCADE,
    CONSTRAINT chat_messages_deleted_by_fkey FOREIGN KEY (deleted_by) REFERENCES users(id),
    CONSTRAINT chat_messages_pinned_by_fkey FOREIGN KEY (pinned_by) REFERENCES users(id),
    CONSTRAINT chat_messages_reply_to_id_fkey FOREIGN KEY (reply_to_id) REFERENCES chat_messages(id)
);

CREATE INDEX chat_messages_created_idx ON chat_messages USING btree (chat_id, seq_created);

CREATE INDEX chat_messages_pinned_idx ON chat_messages USING btree (chat_id, pinned_at) WHERE (pinned_at IS NOT NULL);

CREATE INDEX chat_messages_search_idx ON chat_messages USING gin (search_text gin_trgm_ops) WHERE ((search_text IS NOT NULL) AND (deleted_at IS NULL));

CREATE INDEX chat_messages_updated_idx ON chat_messages USING btree (chat_id, seq_updated);

CREATE INDEX search_chat_files_idx ON chat_messages USING btree (chat_id) WHERE ((deleted_at IS NULL) AND (jsonb_typeof(content) = 'array'::text) AND (content @? '$[*]?(((@."type" == "image" || @."type" == "attachment") || @."type" == "pasted_text") || @."type" == "forwarded" && (exists (@."images"[0]) || exists (@."attachments"[0])))'::jsonpath));

CREATE INDEX search_messages_media_idx ON chat_messages USING gin (orch_search_file_types(content)) WHERE ((deleted_at IS NULL) AND (search_text IS NOT NULL));

CREATE INDEX search_messages_recent_idx ON chat_messages USING btree (created_at DESC, id DESC) WHERE ((deleted_at IS NULL) AND (search_text IS NOT NULL));

CREATE INDEX search_messages_scope_recent_idx ON chat_messages USING btree (chat_id, created_at DESC, id DESC) WHERE ((deleted_at IS NULL) AND (search_text IS NOT NULL));

CREATE TRIGGER related_chat_references AFTER INSERT OR DELETE OR UPDATE OF content, deleted_at ON chat_messages FOR EACH ROW EXECUTE FUNCTION orch_related_reference_refresh();

CREATE TABLE chat_states (
    user_id uuid NOT NULL,
    chat_id uuid NOT NULL,
    level text,
    engaged boolean DEFAULT false NOT NULL,
    last_read_seq bigint DEFAULT 0 NOT NULL,
    read_revision bigint DEFAULT 0 NOT NULL,
    mentions_unread integer DEFAULT 0 NOT NULL,
    draft jsonb,
    draft_reply_to uuid,
    hidden boolean DEFAULT false NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chat_states_level_check CHECK ((level = ANY (ARRAY['all'::text, 'mentions'::text, 'off'::text]))),
    CONSTRAINT chat_states_pkey PRIMARY KEY (user_id, chat_id),
    CONSTRAINT chat_states_chat_id_fkey FOREIGN KEY (chat_id) REFERENCES chats(id) ON DELETE CASCADE,
    CONSTRAINT chat_states_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE chat_file_references (
    id uuid NOT NULL,
    chat_id uuid NOT NULL,
    message_id uuid NOT NULL,
    carried integer NOT NULL,
    CONSTRAINT chat_file_references_carried_check CHECK ((carried > 0)),
    CONSTRAINT chat_file_references_message_id_carried_key UNIQUE (message_id, carried),
    CONSTRAINT chat_file_references_pkey PRIMARY KEY (id),
    CONSTRAINT chat_file_references_chat_id_fkey FOREIGN KEY (chat_id) REFERENCES chats(id) ON DELETE CASCADE,
    CONSTRAINT chat_file_references_message_id_fkey FOREIGN KEY (message_id) REFERENCES chat_messages(id) ON DELETE CASCADE
);

CREATE TABLE chat_item_refs (
    message_id uuid NOT NULL,
    item_id uuid NOT NULL,
    CONSTRAINT chat_item_refs_pkey PRIMARY KEY (message_id, item_id),
    CONSTRAINT chat_item_refs_item_id_fkey FOREIGN KEY (item_id) REFERENCES items(id) ON DELETE CASCADE,
    CONSTRAINT chat_item_refs_message_id_fkey FOREIGN KEY (message_id) REFERENCES chat_messages(id) ON DELETE CASCADE
);

CREATE INDEX chat_item_refs_item_idx ON chat_item_refs USING btree (item_id);
