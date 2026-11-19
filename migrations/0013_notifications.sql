-- Announcements, notifications, channels, routes, destinations and the outbox.

CREATE TABLE announcements (
    id uuid NOT NULL,
    topic_id uuid NOT NULL,
    from_thread_id uuid,
    from_user_id uuid,
    category text DEFAULT 'notice'::text NOT NULL,
    title text NOT NULL,
    body text NOT NULL,
    files jsonb DEFAULT '[]'::jsonb NOT NULL,
    severity text DEFAULT 'info'::text NOT NULL,
    delivery text NOT NULL,
    fallback text NOT NULL,
    wake_idle boolean NOT NULL,
    targets text DEFAULT 'all'::text NOT NULL,
    to_thread_id uuid,
    group_id uuid,
    topic_seq bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    reference_body text,
    CONSTRAINT announcements_body_check CHECK ((char_length(body) <= 8000)),
    CONSTRAINT announcements_category_check CHECK ((category = ANY (ARRAY['notice'::text, 'milestone'::text]))),
    CONSTRAINT announcements_check CHECK (((from_thread_id IS NULL) <> (from_user_id IS NULL))),
    CONSTRAINT announcements_check1 CHECK (((targets = 'thread'::text) = (to_thread_id IS NOT NULL))),
    CONSTRAINT announcements_check2 CHECK (((title <> ''::text) OR (targets = 'thread'::text))),
    CONSTRAINT announcements_check3 CHECK (((targets = 'thread'::text) OR (char_length(body) <= 1500))),
    CONSTRAINT announcements_delivery_check CHECK ((delivery = ANY (ARRAY['interrupt'::text, 'steer'::text, 'next_turn'::text]))),
    CONSTRAINT announcements_fallback_check CHECK ((fallback = ANY (ARRAY['next_turn'::text, 'interrupt'::text]))),
    CONSTRAINT announcements_severity_check CHECK ((severity = ANY (ARRAY['info'::text, 'important'::text]))),
    CONSTRAINT announcements_targets_check CHECK ((targets = ANY (ARRAY['all'::text, 'active'::text, 'thread'::text, 'none'::text]))),
    CONSTRAINT announcements_title_check CHECK ((char_length(title) <= 120)),
    CONSTRAINT announcements_pkey PRIMARY KEY (id),
    CONSTRAINT announcements_from_user_id_fkey FOREIGN KEY (from_user_id) REFERENCES users(id),
    CONSTRAINT announcements_topic_id_fkey FOREIGN KEY (topic_id) REFERENCES nodes(id) ON DELETE CASCADE
);

CREATE INDEX announcements_page_idx ON announcements USING btree (created_at DESC, id DESC);

CREATE TABLE destinations (
    id uuid NOT NULL,
    node_id uuid NOT NULL,
    channel text DEFAULT 'webhook'::text NOT NULL,
    name text NOT NULL,
    config jsonb DEFAULT '{}'::jsonb NOT NULL,
    templates jsonb DEFAULT '{}'::jsonb NOT NULL,
    kinds text[] DEFAULT '{}'::text[] NOT NULL,
    level text DEFAULT 'important'::text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    last_ok_at timestamp with time zone,
    last_error text,
    created_by uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT destinations_level_check CHECK ((level = ANY (ARRAY['all'::text, 'important'::text]))),
    CONSTRAINT destinations_name_check CHECK (((char_length(name) >= 1) AND (char_length(name) <= 80))),
    CONSTRAINT destinations_pkey PRIMARY KEY (id),
    CONSTRAINT destinations_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id),
    CONSTRAINT destinations_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE
);

CREATE INDEX destinations_node_idx ON destinations USING btree (node_id) WHERE enabled;

CREATE TABLE notification_channels (
    id uuid NOT NULL,
    org_id uuid,
    user_id uuid,
    kind text NOT NULL,
    name text NOT NULL,
    config jsonb DEFAULT '{}'::jsonb NOT NULL,
    templates jsonb DEFAULT '{}'::jsonb NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    link_cursor text,
    last_ok_at timestamp with time zone,
    last_error text,
    created_by uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT notification_channels_check CHECK (((org_id IS NULL) OR (user_id IS NULL))),
    CONSTRAINT notification_channels_name_check CHECK (((char_length(name) >= 1) AND (char_length(name) <= 80))),
    CONSTRAINT notification_channels_pkey PRIMARY KEY (id),
    CONSTRAINT notification_channels_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id),
    CONSTRAINT notification_channels_org_id_fkey FOREIGN KEY (org_id) REFERENCES nodes(id) ON DELETE CASCADE,
    CONSTRAINT notification_channels_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX notification_channels_one_idx ON notification_channels USING btree (org_id, kind) NULLS NOT DISTINCT WHERE ((user_id IS NULL) AND (kind = ANY (ARRAY['email'::text, 'telegram'::text, 'push'::text])));

CREATE INDEX notification_channels_user_idx ON notification_channels USING btree (user_id) WHERE (user_id IS NOT NULL);

CREATE TABLE notification_operations (
    id uuid NOT NULL,
    data jsonb NOT NULL,
    ordinal bigint GENERATED ALWAYS AS IDENTITY (SEQUENCE NAME notification_operations_ordinal_seq START WITH 1 INCREMENT BY 1 NO MINVALUE NO MAXVALUE CACHE 1),
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT notification_operations_pkey PRIMARY KEY (id)
);

CREATE TABLE notification_prefs (
    user_id uuid NOT NULL,
    approvals_always boolean DEFAULT true NOT NULL,
    chat_node_level text DEFAULT 'mentions'::text NOT NULL,
    chat_direct_level text DEFAULT 'all'::text NOT NULL,
    quiet_start_minute integer,
    quiet_end_minute integer,
    pause_until timestamp with time zone,
    revision bigint DEFAULT 1 NOT NULL,
    disabled_kinds jsonb DEFAULT '[]'::jsonb NOT NULL,
    CONSTRAINT notification_prefs_chat_direct_level_check CHECK ((chat_direct_level = ANY (ARRAY['all'::text, 'mentions'::text, 'off'::text]))),
    CONSTRAINT notification_prefs_chat_node_level_check CHECK ((chat_node_level = ANY (ARRAY['all'::text, 'mentions'::text, 'off'::text]))),
    CONSTRAINT notification_prefs_check CHECK (((quiet_start_minute IS NULL) = (quiet_end_minute IS NULL))),
    CONSTRAINT notification_prefs_disabled_kinds_check CHECK ((jsonb_typeof(disabled_kinds) = 'array'::text)),
    CONSTRAINT notification_prefs_quiet_end_minute_check CHECK (((quiet_end_minute >= 0) AND (quiet_end_minute <= 1439))),
    CONSTRAINT notification_prefs_quiet_start_minute_check CHECK (((quiet_start_minute >= 0) AND (quiet_start_minute <= 1439))),
    CONSTRAINT notification_prefs_pkey PRIMARY KEY (user_id),
    CONSTRAINT notification_prefs_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE notification_routes (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    channel_id uuid,
    channel_snapshot jsonb NOT NULL,
    enabled boolean DEFAULT false NOT NULL,
    address jsonb DEFAULT '{}'::jsonb NOT NULL,
    link_hash bytea,
    link_expires_at timestamp with time zone,
    linked_at timestamp with time zone,
    kinds text[] DEFAULT '{}'::text[] NOT NULL,
    unread_delay_minutes integer,
    min_interval_minutes integer,
    sent_seq bigint DEFAULT 0 NOT NULL,
    last_sent_at timestamp with time zone,
    last_ok_at timestamp with time zone,
    last_error jsonb,
    failure_episode uuid,
    failed_at timestamp with time zone,
    health text DEFAULT 'never'::text NOT NULL,
    held boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT notification_routes_health_check CHECK ((health = ANY (ARRAY['never'::text, 'ok'::text, 'failed'::text, 'partial'::text, 'target_gone'::text, 'channel_removed'::text]))),
    CONSTRAINT notification_routes_min_interval_minutes_check CHECK (((min_interval_minutes >= 1) AND (min_interval_minutes <= 1440))),
    CONSTRAINT notification_routes_unread_delay_minutes_check CHECK (((unread_delay_minutes >= 1) AND (unread_delay_minutes <= 1440))),
    CONSTRAINT notification_routes_pkey PRIMARY KEY (id),
    CONSTRAINT notification_routes_user_id_channel_id_key UNIQUE (user_id, channel_id),
    CONSTRAINT notification_routes_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES notification_channels(id) ON DELETE SET NULL,
    CONSTRAINT notification_routes_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE notifications (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    kind text NOT NULL,
    node_id uuid,
    thread_id uuid,
    approval_id uuid,
    chat_id uuid,
    title text NOT NULL,
    body text DEFAULT ''::text NOT NULL,
    data jsonb DEFAULT '{}'::jsonb NOT NULL,
    dedupe_key text NOT NULL,
    inbox_seq bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    read_at timestamp with time zone,
    read_batch_id uuid,
    CONSTRAINT notifications_pkey PRIMARY KEY (id),
    CONSTRAINT notifications_user_id_dedupe_key_key UNIQUE (user_id, dedupe_key),
    CONSTRAINT notifications_chat_fk FOREIGN KEY (chat_id) REFERENCES chats(id) ON DELETE CASCADE,
    CONSTRAINT notifications_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE,
    CONSTRAINT notifications_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX notifications_approval_idx ON notifications USING btree (approval_id, user_id) WHERE (approval_id IS NOT NULL);

CREATE INDEX notifications_approval_source_idx ON notifications USING btree (approval_id) WHERE (approval_id IS NOT NULL);

CREATE INDEX notifications_inbox_idx ON notifications USING btree (user_id, inbox_seq);

CREATE INDEX notifications_read_batch_idx ON notifications USING btree (user_id, read_batch_id) WHERE (read_batch_id IS NOT NULL);

CREATE INDEX notifications_unread_idx ON notifications USING btree (user_id) WHERE (read_at IS NULL);

CREATE INDEX notifications_user_idx ON notifications USING btree (user_id, created_at DESC);

CREATE TABLE operation_recipient_previews (
    id uuid NOT NULL,
    users uuid[] NOT NULL,
    notification_kind text NOT NULL,
    CONSTRAINT operation_recipient_previews_pkey PRIMARY KEY (id),
    CONSTRAINT operation_recipient_previews_id_fkey FOREIGN KEY (id) REFERENCES preflights(id) ON DELETE CASCADE
);

CREATE TABLE outbox (
    id uuid NOT NULL,
    channel text NOT NULL,
    route_id uuid,
    destination_id uuid,
    channel_id uuid,
    payload jsonb NOT NULL,
    notification_ids uuid[] DEFAULT '{}'::uuid[] NOT NULL,
    dispatch_id uuid,
    dispatch_owner uuid,
    dispatch_expires_at timestamp with time zone,
    dispatch_started_at timestamp with time zone,
    cancel_reason text,
    state text DEFAULT 'pending'::text NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    next_attempt_at timestamp with time zone DEFAULT now() NOT NULL,
    last_error text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT outbox_check CHECK (((state <> 'sending'::text) OR ((dispatch_id IS NOT NULL) AND (dispatch_owner IS NOT NULL) AND (dispatch_expires_at IS NOT NULL)))),
    CONSTRAINT outbox_state_check CHECK ((state = ANY (ARRAY['pending'::text, 'sending'::text, 'sent'::text, 'failed'::text, 'cancelled'::text]))),
    CONSTRAINT outbox_pkey PRIMARY KEY (id),
    CONSTRAINT outbox_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES notification_channels(id) ON DELETE CASCADE,
    CONSTRAINT outbox_destination_id_fkey FOREIGN KEY (destination_id) REFERENCES destinations(id) ON DELETE CASCADE,
    CONSTRAINT outbox_route_id_fkey FOREIGN KEY (route_id) REFERENCES notification_routes(id) ON DELETE CASCADE
);

CREATE INDEX outbox_due_idx ON outbox USING btree (next_attempt_at) WHERE (state = 'pending'::text);

CREATE INDEX outbox_route_idx ON outbox USING btree (route_id, state) WHERE (route_id IS NOT NULL);

CREATE INDEX outbox_sending_idx ON outbox USING btree (dispatch_expires_at) WHERE (state = 'sending'::text);

CREATE TABLE push_subscriptions (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    endpoint text NOT NULL,
    p256dh text NOT NULL,
    auth text NOT NULL,
    user_agent text DEFAULT ''::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_ok_at timestamp with time zone,
    failed_count integer DEFAULT 0 NOT NULL,
    CONSTRAINT push_subscriptions_endpoint_key UNIQUE (endpoint),
    CONSTRAINT push_subscriptions_pkey PRIMARY KEY (id),
    CONSTRAINT push_subscriptions_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE subscriptions (
    user_id uuid NOT NULL,
    node_id uuid NOT NULL,
    level text NOT NULL,
    source text NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT subscriptions_level_check CHECK ((level = ANY (ARRAY['all'::text, 'important'::text, 'off'::text]))),
    CONSTRAINT subscriptions_source_check CHECK ((source = ANY (ARRAY['auto'::text, 'manual'::text]))),
    CONSTRAINT subscriptions_pkey PRIMARY KEY (user_id, node_id),
    CONSTRAINT subscriptions_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE,
    CONSTRAINT subscriptions_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);
