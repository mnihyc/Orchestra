-- Threads: turns, items, segments, inputs, approvals, goals, continuations, history cuts and native guards.

CREATE TABLE input_schedules (
    id uuid NOT NULL,
    topic_id uuid NOT NULL,
    author_id uuid NOT NULL,
    name text NOT NULL,
    target jsonb NOT NULL,
    content jsonb NOT NULL,
    native_action jsonb,
    timing jsonb NOT NULL,
    state text NOT NULL,
    next_run_at timestamp with time zone,
    revision bigint DEFAULT 1 NOT NULL,
    topic_seq bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    CONSTRAINT input_schedules_name_check CHECK (((char_length(name) >= 1) AND (char_length(name) <= 80))),
    CONSTRAINT input_schedules_state_check CHECK ((state = ANY (ARRAY['active'::text, 'paused'::text, 'finished'::text, 'deleted'::text]))),
    CONSTRAINT input_schedules_pkey PRIMARY KEY (id),
    CONSTRAINT input_schedules_author_id_fkey FOREIGN KEY (author_id) REFERENCES users(id) ON DELETE CASCADE,
    CONSTRAINT input_schedules_topic_id_fkey FOREIGN KEY (topic_id) REFERENCES nodes(id) ON DELETE CASCADE
);

CREATE INDEX input_schedules_due_idx ON input_schedules USING btree (next_run_at) WHERE (state = 'active'::text);

CREATE INDEX thread_display_schedules_idx ON input_schedules USING btree (((target ->> 'thread_id'::text))) WHERE ((state = 'active'::text) AND (deleted_at IS NULL) AND ((target ->> 'kind'::text) = 'thread'::text));

CREATE TABLE raw_events (
    segment_id uuid NOT NULL,
    n bigint NOT NULL,
    at timestamp with time zone NOT NULL,
    direction text NOT NULL,
    payload bytea NOT NULL,
    CONSTRAINT raw_events_direction_check CHECK ((direction = ANY (ARRAY['to_harness'::text, 'from_harness'::text]))),
    CONSTRAINT raw_events_pkey PRIMARY KEY (segment_id, n, at)
)
PARTITION BY RANGE (at);

CREATE TABLE thread_tool_calls (
    answer_id uuid NOT NULL,
    thread_id uuid NOT NULL,
    turn_id uuid NOT NULL,
    segment_id uuid NOT NULL,
    continuation_id uuid NOT NULL,
    execution_revision bigint NOT NULL,
    tool text NOT NULL,
    arguments_hash bytea NOT NULL,
    plan jsonb DEFAULT '{}'::jsonb NOT NULL,
    completed boolean DEFAULT false NOT NULL,
    retired boolean DEFAULT false NOT NULL,
    CONSTRAINT thread_tool_calls_arguments_hash_check CHECK ((octet_length(arguments_hash) = 32)),
    CONSTRAINT thread_tool_calls_tool_check CHECK ((length(tool) <= 128)),
    CONSTRAINT thread_tool_calls_pkey PRIMARY KEY (answer_id)
);

CREATE INDEX thread_tool_calls_answer_pending ON thread_tool_calls USING btree (answer_id) WHERE ((NOT completed) AND (NOT retired));

CREATE INDEX thread_tool_calls_open_idx ON thread_tool_calls USING btree (answer_id) WHERE ((NOT completed) AND (NOT retired));

CREATE INDEX thread_tool_calls_source ON thread_tool_calls USING btree (thread_id, turn_id);

CREATE TABLE thread_tool_commands (
    command_id uuid NOT NULL,
    thread_id uuid NOT NULL,
    turn_id uuid NOT NULL,
    segment_id uuid NOT NULL,
    continuation_id uuid NOT NULL,
    answer_id uuid NOT NULL,
    environment_id uuid NOT NULL,
    runtime_id uuid,
    retirement_machine_id uuid,
    settled boolean DEFAULT false NOT NULL,
    download jsonb,
    fence_id uuid,
    fence_retry_at timestamp with time zone,
    CONSTRAINT thread_tool_commands_check CHECK (((runtime_id IS NULL) = (retirement_machine_id IS NULL))),
    CONSTRAINT thread_tool_commands_pkey PRIMARY KEY (command_id),
    CONSTRAINT thread_tool_commands_answer_id_fkey FOREIGN KEY (answer_id) REFERENCES thread_tool_calls(answer_id),
    CONSTRAINT thread_tool_commands_command_id_fkey FOREIGN KEY (command_id) REFERENCES commands(id) ON DELETE CASCADE,
    CONSTRAINT thread_tool_commands_fence_id_fkey FOREIGN KEY (fence_id) REFERENCES commands(id) ON DELETE SET NULL
);

CREATE INDEX thread_tool_commands_fence ON thread_tool_commands USING btree (fence_id) WHERE (fence_id IS NOT NULL);

CREATE INDEX thread_tool_commands_retirement ON thread_tool_commands USING btree (retirement_machine_id, runtime_id) WHERE (runtime_id IS NOT NULL);

CREATE INDEX thread_tool_commands_source ON thread_tool_commands USING btree (thread_id, command_id) WHERE (NOT settled);

CREATE TABLE input_schedule_blob_refs (
    schedule_id uuid NOT NULL,
    blob bytea NOT NULL,
    CONSTRAINT input_schedule_blob_refs_pkey PRIMARY KEY (schedule_id, blob),
    CONSTRAINT input_schedule_blob_refs_blob_fkey FOREIGN KEY (blob) REFERENCES blobs(hash),
    CONSTRAINT input_schedule_blob_refs_schedule_id_fkey FOREIGN KEY (schedule_id) REFERENCES input_schedules(id) ON DELETE CASCADE
);

CREATE TABLE thread_tool_command_blobs (
    command_id uuid NOT NULL,
    blob bytea NOT NULL,
    CONSTRAINT thread_tool_command_blobs_pkey PRIMARY KEY (command_id, blob),
    CONSTRAINT thread_tool_command_blobs_blob_fkey FOREIGN KEY (blob) REFERENCES blobs(hash),
    CONSTRAINT thread_tool_command_blobs_command_id_fkey FOREIGN KEY (command_id) REFERENCES thread_tool_commands(command_id) ON DELETE CASCADE
);

CREATE INDEX thread_tool_command_blobs_hash ON thread_tool_command_blobs USING btree (blob);

CREATE TABLE segments (
    id uuid NOT NULL,
    thread_id uuid NOT NULL,
    ordinal integer NOT NULL,
    agent_id uuid,
    provider_id uuid,
    environment_id uuid,
    machine_id uuid,
    env_generation integer NOT NULL,
    arrangement text NOT NULL,
    executor text,
    tool_set text,
    credential_path text NOT NULL,
    raw_last_n bigint DEFAULT 0 NOT NULL,
    native_session_ref text,
    started_by uuid,
    started_at timestamp with time zone DEFAULT now() NOT NULL,
    ended_at timestamp with time zone,
    end_reason text,
    import_fidelity text,
    import_report jsonb,
    context_sent jsonb DEFAULT '{}'::jsonb NOT NULL,
    instruction_sources jsonb DEFAULT '[]'::jsonb NOT NULL,
    approval_reviewer jsonb,
    seq_updated bigint NOT NULL,
    continuation_id uuid NOT NULL,
    delivered_resources jsonb,
    agent_history_bytes bigint,
    agent_history_measured_at timestamp with time zone,
    agent_history_removed_at timestamp with time zone,
    agent_history_removed_by uuid,
    agent_history_owned boolean DEFAULT false NOT NULL,
    agent_history_claim uuid,
    agent_history_claimed_at timestamp with time zone,
    agent_history_error text,
    agent_history_recovery jsonb,
    agent_history_measure_revision bigint DEFAULT 0 NOT NULL,
    agent_history_measured_revision bigint DEFAULT 0 NOT NULL,
    CONSTRAINT segments_agent_history_bytes_check CHECK ((agent_history_bytes >= 0)),
    CONSTRAINT segments_agent_history_claim_pair CHECK (((agent_history_claim IS NULL) = (agent_history_claimed_at IS NULL))),
    CONSTRAINT segments_agent_history_measure_revision_check CHECK ((agent_history_measure_revision >= 0)),
    CONSTRAINT segments_agent_history_measured_revision_check CHECK ((agent_history_measured_revision >= 0)),
    CONSTRAINT segments_credential_path_check CHECK ((credential_path = ANY (ARRAY['native'::text, 'forwarded'::text, 'shared'::text, 'handover'::text]))),
    CONSTRAINT segments_end_reason_check CHECK ((end_reason = ANY (ARRAY['switched'::text, 'environment_moved'::text, 'environment_recreated'::text, 'environment_restarted'::text, 'archived'::text, 'error'::text]))),
    CONSTRAINT segments_execution_binding_check CHECK ((((agent_id IS NOT NULL) AND (provider_id IS NOT NULL) AND (environment_id IS NOT NULL) AND (agent_id <> '00000000-0000-0000-0000-000000000000'::uuid) AND (provider_id <> '00000000-0000-0000-0000-000000000000'::uuid) AND (environment_id <> '00000000-0000-0000-0000-000000000000'::uuid)) OR ((agent_id IS NULL) AND (provider_id IS NULL) AND (environment_id IS NULL) AND (ended_at IS NOT NULL) AND (import_fidelity IS NOT NULL)))),
    CONSTRAINT segments_import_fidelity_check CHECK ((import_fidelity = ANY (ARRAY['lossless'::text, 'structured'::text, 'file'::text, 'summary'::text]))),
    CONSTRAINT segments_pkey PRIMARY KEY (id),
    CONSTRAINT segments_thread_id_ordinal_key UNIQUE (thread_id, ordinal),
    CONSTRAINT segments_agent_history_removed_by_fkey FOREIGN KEY (agent_history_removed_by) REFERENCES users(id) ON DELETE SET NULL,
    CONSTRAINT segments_machine_fk FOREIGN KEY (machine_id) REFERENCES machines(id),
    CONSTRAINT segments_started_by_fkey FOREIGN KEY (started_by) REFERENCES users(id)
);

CREATE FUNCTION public.orch_pending_notice_lane(s public.segments) RETURNS bytea
    LANGUAGE sql STABLE STRICT
    AS $$
  SELECT sha256(convert_to(CASE WHEN s.native_session_ref IS NULL THEN jsonb_build_array(s.id)::text
    ELSE jsonb_build_array(s.thread_id,s.continuation_id,s.provider_id,s.agent_id,s.machine_id,s.native_session_ref)::text END,'UTF8'))
$$;

CREATE INDEX segments_agent_facts_idx ON segments USING btree (agent_id, id);

CREATE INDEX segments_agent_history_claim_idx ON segments USING btree (agent_history_claim) WHERE (agent_history_claim IS NOT NULL);

CREATE INDEX segments_agent_history_claim_page_idx ON segments USING btree (id) WHERE (agent_history_claim IS NOT NULL);

CREATE INDEX segments_agent_history_homes_idx ON segments USING btree (machine_id, provider_id, agent_id) WHERE ((machine_id IS NOT NULL) AND agent_history_owned);

CREATE INDEX segments_agent_history_inventory_idx ON segments USING btree (machine_id, provider_id, agent_id, id) WHERE ((native_session_ref IS NOT NULL) AND (agent_history_removed_at IS NULL));

CREATE INDEX segments_agent_history_membership_idx ON segments USING btree (machine_id, provider_id, agent_id, thread_id) INCLUDE (native_session_ref, agent_history_owned, agent_history_claim);

CREATE INDEX segments_agent_history_native_idx ON segments USING btree (provider_id, agent_id, native_session_ref, machine_id, started_at DESC, id DESC) WHERE (native_session_ref IS NOT NULL);

CREATE INDEX segments_agent_history_page_idx ON segments USING btree (id) WHERE ((native_session_ref IS NOT NULL) AND (agent_history_removed_at IS NULL));

CREATE INDEX segments_bound_thread_ordinal_idx ON segments USING btree (thread_id, ordinal) WHERE (agent_id IS NOT NULL);

CREATE INDEX segments_continuation_ordinal_idx ON segments USING btree (thread_id, continuation_id, ordinal);

CREATE INDEX segments_continuation_updated_idx ON segments USING btree (thread_id, continuation_id, seq_updated);

CREATE INDEX segments_delivered_resources_idx ON segments USING gin (delivered_resources jsonb_path_ops);

CREATE INDEX segments_environment_facts_idx ON segments USING btree (environment_id, id);

CREATE TRIGGER pending_notice_lane_changed AFTER INSERT OR UPDATE OF native_session_ref, continuation_id, provider_id, agent_id, machine_id ON segments FOR EACH ROW EXECUTE FUNCTION orch_pending_notice_lane_changed();

CREATE TRIGGER segments_continuation_owner BEFORE INSERT OR UPDATE OF continuation_id, thread_id ON segments FOR EACH ROW EXECUTE FUNCTION thread_continuation_owner();

CREATE TABLE agent_history_natives (
    machine_id uuid,
    provider_id uuid NOT NULL,
    agent_id uuid NOT NULL,
    native_session_ref text NOT NULL,
    closes bigint DEFAULT 0 NOT NULL,
    measured bigint DEFAULT 0 NOT NULL,
    segments bigint DEFAULT 0 NOT NULL,
    CONSTRAINT agent_history_natives_counts_check CHECK (((closes >= 0) AND (measured >= 0) AND (segments >= 0))),
    CONSTRAINT agent_history_natives_key UNIQUE NULLS NOT DISTINCT (machine_id, provider_id, agent_id, native_session_ref)
);

CREATE INDEX agent_history_natives_unmeasured_idx ON agent_history_natives USING btree (provider_id, agent_id, native_session_ref) WHERE (measured < closes);

CREATE TRIGGER segments_agent_history_natives AFTER INSERT OR DELETE OR UPDATE OF machine_id, provider_id, agent_id, native_session_ref, agent_history_measure_revision, agent_history_measured_revision ON segments FOR EACH ROW EXECUTE FUNCTION orch_agent_history_natives();

CREATE TABLE approvals (
    id uuid NOT NULL,
    kind text NOT NULL,
    state text DEFAULT 'pending'::text NOT NULL,
    node_id uuid,
    object_kind text,
    object_id uuid,
    thread_id uuid,
    turn_id uuid,
    item_id uuid,
    tool_item_id uuid,
    environment_id uuid,
    requester_user_id uuid,
    requester_thread_id uuid,
    title text NOT NULL,
    request jsonb NOT NULL,
    required_permission text NOT NULL,
    approver_node_id uuid,
    approver_scope text,
    batch_id uuid,
    native_ref text,
    decided_by uuid,
    decided_at timestamp with time zone,
    decision jsonb,
    expires_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT approvals_approver_scope_check CHECK ((approver_scope = ANY (ARRAY['environment'::text, 'host'::text, 'org'::text, 'system'::text]))),
    CONSTRAINT approvals_kind_check CHECK (((kind = ANY (ARRAY['harness'::text, 'capability'::text, 'access'::text, 'port'::text])) OR (kind = ANY (ARRAY['plan'::text, 'input'::text])))),
    CONSTRAINT approvals_state_check CHECK ((state = ANY (ARRAY['pending'::text, 'approved'::text, 'answered'::text, 'denied'::text, 'cancelled'::text, 'expired'::text]))),
    CONSTRAINT approvals_pkey PRIMARY KEY (id),
    CONSTRAINT approvals_approver_node_id_fkey FOREIGN KEY (approver_node_id) REFERENCES nodes(id),
    CONSTRAINT approvals_decided_by_fkey FOREIGN KEY (decided_by) REFERENCES users(id),
    CONSTRAINT approvals_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id),
    CONSTRAINT approvals_requester_user_id_fkey FOREIGN KEY (requester_user_id) REFERENCES users(id)
);

CREATE INDEX approvals_agent_history_waiting_idx ON approvals USING btree (thread_id) WHERE (state = ANY (ARRAY['pending'::text, 'approved'::text]));

CREATE INDEX approvals_item_idx ON approvals USING btree (item_id) WHERE (item_id IS NOT NULL);

CREATE INDEX approvals_native_content_source ON approvals USING btree (thread_id, ((request #>> '{detail,native_item_ref}'::text[])), turn_id) WHERE ((kind = 'harness'::text) AND ((request #>> '{detail,native_item_ref}'::text[]) IS NOT NULL));

CREATE UNIQUE INDEX approvals_one_open_chat_join ON approvals USING btree (object_id, requester_user_id) WHERE ((kind = 'access'::text) AND (object_kind = 'chat'::text) AND (state = 'pending'::text));

CREATE UNIQUE INDEX approvals_one_open_source ON approvals USING btree (kind, object_id) WHERE ((state = 'pending'::text) AND (kind = ANY (ARRAY['plan'::text, 'input'::text])));

CREATE INDEX approvals_pending_idx ON approvals USING btree (kind, node_id) WHERE (state = 'pending'::text);

CREATE INDEX approvals_thread_pending_idx ON approvals USING btree (thread_id) WHERE (state = 'pending'::text);

CREATE INDEX approvals_tool_item_idx ON approvals USING btree (tool_item_id) WHERE (tool_item_id IS NOT NULL);

CREATE INDEX resource_use_denied_turn_idx ON approvals USING btree (turn_id) WHERE (state = 'denied'::text);

CREATE INDEX thread_display_approvals_idx ON approvals USING btree (thread_id, expires_at) WHERE (state = 'pending'::text);

ALTER TABLE port_openings ADD CONSTRAINT port_openings_approval_id_fkey FOREIGN KEY (approval_id) REFERENCES approvals(id);

CREATE TABLE drafts (
    user_id uuid NOT NULL,
    thread_id uuid NOT NULL,
    content jsonb NOT NULL,
    review_comments jsonb DEFAULT '[]'::jsonb NOT NULL,
    native_action jsonb,
    delivery text DEFAULT 'queue'::text NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT drafts_delivery_check CHECK ((delivery = ANY (ARRAY['queue'::text, 'steer'::text, 'interrupt'::text]))),
    CONSTRAINT drafts_pkey PRIMARY KEY (user_id, thread_id),
    CONSTRAINT drafts_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TRIGGER private_thread_draft_pins AFTER INSERT OR UPDATE OF content ON drafts FOR EACH ROW EXECUTE FUNCTION private_thread_draft_pins();

CREATE TABLE input_schedule_runs (
    id uuid NOT NULL,
    schedule_id uuid NOT NULL,
    occurrence text NOT NULL,
    scheduled_for timestamp with time zone,
    state text NOT NULL,
    thread_id uuid,
    input_id uuid,
    turn_id uuid,
    reason text,
    topic_seq bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    started_at timestamp with time zone,
    CONSTRAINT input_schedule_runs_state_check CHECK ((state = ANY (ARRAY['queued'::text, 'blocked'::text, 'started'::text, 'failed'::text, 'skipped'::text]))),
    CONSTRAINT input_schedule_runs_pkey PRIMARY KEY (id),
    CONSTRAINT input_schedule_runs_schedule_id_occurrence_key UNIQUE (schedule_id, occurrence),
    CONSTRAINT input_schedule_runs_schedule_id_fkey FOREIGN KEY (schedule_id) REFERENCES input_schedules(id) ON DELETE CASCADE
);

CREATE TABLE items (
    id uuid NOT NULL,
    thread_id uuid NOT NULL,
    segment_id uuid NOT NULL,
    turn_id uuid,
    model_input_id uuid,
    stream text DEFAULT 'main'::text NOT NULL,
    parent_item_id uuid,
    kind text NOT NULL,
    status text NOT NULL,
    author_id uuid,
    payload jsonb NOT NULL,
    native_ref text,
    before_orch boolean DEFAULT false NOT NULL,
    search_text text,
    pinned_at timestamp with time zone,
    pinned_by uuid,
    chat_refs integer DEFAULT 0 NOT NULL,
    seq_created bigint NOT NULL,
    seq_updated bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    continuation_id uuid NOT NULL,
    contains_internal_marker boolean DEFAULT false NOT NULL,
    payload_bytes bigint DEFAULT 0 NOT NULL,
    CONSTRAINT items_kind_check CHECK ((kind = ANY (ARRAY['user_message'::text, 'agent_message'::text, 'reasoning'::text, 'plan'::text, 'tool_call'::text, 'approval'::text, 'announcement'::text, 'marker'::text, 'error'::text]))),
    CONSTRAINT items_status_check CHECK ((status = ANY (ARRAY['in_progress'::text, 'completed'::text, 'failed'::text, 'cancelled'::text, 'pending'::text, 'approved'::text, 'answered'::text, 'denied'::text, 'expired'::text]))),
    CONSTRAINT items_payload_bytes_check CHECK ((payload_bytes >= 0)),
    CONSTRAINT items_pkey PRIMARY KEY (id),
    CONSTRAINT items_author_id_fkey FOREIGN KEY (author_id) REFERENCES users(id),
    CONSTRAINT items_parent_item_id_fkey FOREIGN KEY (parent_item_id) REFERENCES items(id),
    CONSTRAINT items_pinned_by_fkey FOREIGN KEY (pinned_by) REFERENCES users(id),
    CONSTRAINT items_segment_id_fkey FOREIGN KEY (segment_id) REFERENCES segments(id)
);

COMMENT ON COLUMN items.parent_item_id IS 'Native owner item of a substream, or the source tool of a content request';

CREATE INDEX items_before_history_idx ON items USING btree (thread_id, continuation_id, seq_created) WHERE (before_orch AND (turn_id IS NULL) AND (stream = 'main'::text));

CREATE INDEX items_continuation_created_idx ON items USING btree (thread_id, continuation_id, seq_created);

CREATE INDEX items_continuation_stream_created_idx ON items USING btree (thread_id, continuation_id, stream, seq_created);

CREATE INDEX items_continuation_updated_idx ON items USING btree (thread_id, continuation_id, seq_updated);

CREATE INDEX items_execution_history_idx ON items USING btree (thread_id) WHERE ((NOT before_orch) AND (kind <> ALL (ARRAY['marker'::text, 'announcement'::text])));

CREATE UNIQUE INDEX items_native_key ON items USING btree (segment_id, native_ref) WHERE (native_ref IS NOT NULL);

CREATE INDEX items_parent_item_idx ON items USING btree (parent_item_id) WHERE (parent_item_id IS NOT NULL);

CREATE INDEX items_pinned_idx ON items USING btree (thread_id, pinned_at) WHERE (pinned_at IS NOT NULL);

CREATE INDEX items_prompt_input_idx ON items USING btree (thread_id, ((payload ->> 'input_id'::text))) WHERE (kind = 'user_message'::text);

CREATE INDEX items_resource_resume_idx ON items USING btree (turn_id) WHERE ((kind = 'marker'::text) AND ((payload ->> 'marker'::text) = 'resource_resumed'::text) AND ((payload #>> '{data,input_id}'::text[]) IS NOT NULL));

CREATE INDEX items_resource_stop_idx ON items USING btree (((payload #>> '{data,object,id}'::text[])), ((payload #>> '{data,object,kind}'::text[])), turn_id) WHERE ((kind = 'marker'::text) AND ((payload ->> 'marker'::text) = 'resource_stopped'::text));

CREATE INDEX items_search_idx ON items USING gin (thread_id, search_text gin_trgm_ops) WHERE (search_text IS NOT NULL);

CREATE INDEX items_segment_marker_idx ON items USING btree (segment_id, ((payload ->> 'marker'::text)), seq_created) WHERE (kind = 'marker'::text);

CREATE INDEX items_thread_created_idx ON items USING btree (thread_id, seq_created);

CREATE INDEX items_thread_native_idx ON items USING btree (thread_id, native_ref) WHERE (native_ref IS NOT NULL);

CREATE INDEX items_thread_stream_created_idx ON items USING btree (thread_id, stream, seq_created);

CREATE INDEX items_thread_updated_idx ON items USING btree (thread_id, seq_updated);

CREATE INDEX items_turn_idx ON items USING btree (turn_id, seq_created) WHERE (turn_id IS NOT NULL);

CREATE INDEX search_items_media_idx ON items USING gin (orch_search_file_types(payload)) WHERE ((search_text IS NOT NULL) AND (kind <> 'marker'::text) AND (search_text !~~ '%<orch>%'::text));

CREATE INDEX search_items_recent_idx ON items USING btree (created_at DESC, id DESC) WHERE ((search_text IS NOT NULL) AND (kind <> 'marker'::text) AND (search_text !~~ '%<orch>%'::text));

CREATE INDEX search_items_scope_recent_idx ON items USING btree (thread_id, created_at DESC, id DESC) INCLUDE (kind) WHERE ((search_text IS NOT NULL) AND (kind <> 'marker'::text) AND (search_text !~~ '%<orch>%'::text));

CREATE INDEX search_revision_items_recent_idx ON items USING btree (created_at DESC, id DESC) WHERE (kind <> 'marker'::text);

CREATE TRIGGER items_continuation_owner BEFORE INSERT OR UPDATE OF continuation_id, thread_id, segment_id, turn_id ON items FOR EACH ROW EXECUTE FUNCTION thread_continuation_owner();

CREATE TRIGGER items_current_pins AFTER INSERT OR UPDATE OF payload ON items FOR EACH ROW EXECUTE FUNCTION thread_item_current_pins();

CREATE TRIGGER items_freeze_revision BEFORE UPDATE ON items FOR EACH ROW EXECUTE FUNCTION thread_item_freeze_revision();

CREATE TRIGGER items_history_cut_freeze BEFORE DELETE OR UPDATE ON items FOR EACH ROW EXECUTE FUNCTION thread_history_cut_freeze();

CREATE TRIGGER items_internal_marker BEFORE INSERT OR UPDATE OF payload, search_text, contains_internal_marker, payload_bytes ON items FOR EACH ROW EXECUTE FUNCTION orch_item_internal_marker();

CREATE TRIGGER related_thread_references AFTER INSERT OR UPDATE OF payload, status ON items FOR EACH ROW WHEN ((new.status <> 'in_progress'::text)) EXECUTE FUNCTION orch_related_reference_refresh();

COMMENT ON TRIGGER related_thread_references ON items IS 'What an item refers to is kept once it stops growing: a streamed piece rewrites the whole item, and reading every reference out of it again for each piece would cost the square of its length. The change that ends it (completed, failed, cancelled, settled) keeps them.';

CREATE TRIGGER related_thread_references_removed AFTER DELETE ON items FOR EACH ROW EXECUTE FUNCTION orch_related_reference_refresh();

ALTER TABLE approvals ADD CONSTRAINT approvals_item_id_fkey FOREIGN KEY (item_id) REFERENCES items(id);

ALTER TABLE approvals ADD CONSTRAINT approvals_tool_item_id_fkey FOREIGN KEY (tool_item_id) REFERENCES items(id);

CREATE TABLE model_inputs (
    id uuid NOT NULL,
    node_id uuid NOT NULL,
    segment_id uuid,
    turn_id uuid,
    input_id uuid,
    item_id uuid,
    purpose text NOT NULL,
    reference text NOT NULL,
    blocks jsonb NOT NULL,
    state text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    sent_at timestamp with time zone,
    seq_created bigint NOT NULL,
    seq_updated bigint NOT NULL,
    CONSTRAINT model_inputs_purpose_check CHECK ((purpose = ANY (ARRAY['session'::text, 'input'::text, 'tool'::text, 'summary'::text]))),
    CONSTRAINT model_inputs_state_check CHECK ((state = ANY (ARRAY['prepared'::text, 'sent'::text, 'failed'::text, 'uncertain'::text]))),
    CONSTRAINT model_inputs_node_id_purpose_reference_key UNIQUE (node_id, purpose, reference),
    CONSTRAINT model_inputs_pkey PRIMARY KEY (id),
    CONSTRAINT model_inputs_item_id_fkey FOREIGN KEY (item_id) REFERENCES items(id),
    CONSTRAINT model_inputs_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE,
    CONSTRAINT model_inputs_segment_id_fkey FOREIGN KEY (segment_id) REFERENCES segments(id)
);

CREATE INDEX model_inputs_input_idx ON model_inputs USING btree (input_id, seq_created) WHERE (input_id IS NOT NULL);

CREATE INDEX model_inputs_item_idx ON model_inputs USING btree (item_id) WHERE (item_id IS NOT NULL);

CREATE INDEX model_inputs_node_seq_idx ON model_inputs USING btree (node_id, seq_created);

CREATE INDEX model_inputs_segment_idx ON model_inputs USING btree (segment_id, purpose, seq_created);

CREATE TABLE threads (
    node_id uuid NOT NULL,
    topic_id uuid NOT NULL,
    environment_id uuid,
    workspace_dir text DEFAULT ''::text NOT NULL,
    source jsonb,
    links jsonb DEFAULT '[]'::jsonb NOT NULL,
    work_observed jsonb,
    native_catalog jsonb,
    state text DEFAULT 'idle'::text NOT NULL,
    blocked_reason text,
    blocked_detail text,
    starting_step text,
    agent_id uuid,
    provider_id uuid,
    arrangement text,
    executor text,
    tool_set text,
    current_segment_id uuid,
    current_turn_id uuid,
    queue_held boolean DEFAULT false NOT NULL,
    queue_held_by uuid,
    queue_held_at timestamp with time zone,
    queue_revision bigint DEFAULT 1 NOT NULL,
    next_seq bigint DEFAULT 1 NOT NULL,
    seq_state bigint DEFAULT 0 NOT NULL,
    activity_seq bigint DEFAULT 0 NOT NULL,
    pending_notices jsonb DEFAULT '[]'::jsonb NOT NULL,
    usage jsonb,
    last_activity_at timestamp with time zone,
    side_of uuid,
    side_from_turn integer,
    side_reading text,
    active_continuation_id uuid NOT NULL,
    execution_revision bigint DEFAULT 1 NOT NULL,
    CONSTRAINT threads_arrangement_check CHECK ((arrangement = ANY (ARRAY['separated'::text, 'key_separated'::text, 'credential_shared'::text, 'handed_over'::text, 'together'::text]))),
    CONSTRAINT threads_blocked_reason_check CHECK ((blocked_reason = ANY (ARRAY['unbound'::text, 'agent_offline'::text, 'agent_disabled'::text, 'provider_unavailable'::text, 'provider_exhausted'::text, 'environment_unavailable'::text, 'binding_invalid'::text, 'provider_not_allowed'::text, 'model_not_set'::text, 'model_not_allowed'::text, 'quota_exceeded'::text, 'attachments'::text, 'workspace_trust'::text, 'switching'::text, 'error'::text]))),
    CONSTRAINT threads_check CHECK (((state = 'blocked'::text) = (blocked_reason IS NOT NULL))),
    CONSTRAINT threads_check1 CHECK (((starting_step IS NULL) OR (state = 'starting'::text))),
    CONSTRAINT threads_check2 CHECK (((side_of IS NULL) OR (side_of <> node_id))),
    CONSTRAINT threads_execution_revision_check CHECK ((execution_revision > 0)),
    CONSTRAINT threads_pending_notice_preview_bounded CHECK (((jsonb_typeof(pending_notices) = 'array'::text) AND (jsonb_array_length(pending_notices) <= 64))),
    CONSTRAINT threads_side_reading_check CHECK ((side_reading = ANY (ARRAY['read_only'::text, 'approval_gated'::text, 'no_tools'::text]))),
    CONSTRAINT threads_starting_step_check CHECK ((starting_step = ANY (ARRAY['environment'::text, 'source'::text, 'provider'::text, 'harness'::text, 'credential'::text, 'session'::text, 'files'::text, 'turn'::text]))),
    CONSTRAINT threads_state_check CHECK ((state = ANY (ARRAY['idle'::text, 'starting'::text, 'running'::text, 'waiting_approval'::text, 'blocked'::text]))),
    CONSTRAINT threads_pkey PRIMARY KEY (node_id),
    CONSTRAINT threads_agent_fk FOREIGN KEY (agent_id) REFERENCES agents(id),
    CONSTRAINT threads_environment_fk FOREIGN KEY (environment_id) REFERENCES environments(id),
    CONSTRAINT threads_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE,
    CONSTRAINT threads_provider_fk FOREIGN KEY (provider_id) REFERENCES providers(id),
    CONSTRAINT threads_queue_held_by_fkey FOREIGN KEY (queue_held_by) REFERENCES users(id),
    CONSTRAINT threads_side_of_fkey FOREIGN KEY (side_of) REFERENCES threads(node_id) ON DELETE CASCADE,
    CONSTRAINT threads_topic_id_fkey FOREIGN KEY (topic_id) REFERENCES nodes(id)
);

CREATE INDEX thread_display_topic_page_idx ON threads USING btree (topic_id, node_id);

CREATE INDEX threads_agent_facts_idx ON threads USING btree (agent_id, node_id) WHERE (agent_id IS NOT NULL);

CREATE INDEX threads_current_turn_idx ON threads USING btree (current_turn_id) WHERE (current_turn_id IS NOT NULL);

CREATE INDEX threads_environment_idx ON threads USING btree (environment_id, node_id) WHERE (environment_id IS NOT NULL);

CREATE INDEX threads_provider_facts_idx ON threads USING btree (provider_id, node_id) WHERE (provider_id IS NOT NULL);

CREATE INDEX threads_side_idx ON threads USING btree (side_of) WHERE (side_of IS NOT NULL);

CREATE INDEX threads_topic_idx ON threads USING btree (topic_id);

CREATE TRIGGER pending_notice_legacy_insert AFTER INSERT ON threads FOR EACH ROW EXECUTE FUNCTION orch_pending_notice_legacy_write();

CREATE TRIGGER pending_notice_legacy_write BEFORE UPDATE OF pending_notices ON threads FOR EACH ROW WHEN ((old.pending_notices IS DISTINCT FROM new.pending_notices)) EXECUTE FUNCTION orch_pending_notice_legacy_write();

CREATE TRIGGER threads_continuation_queue BEFORE UPDATE OF active_continuation_id, queue_held, queue_held_by, queue_held_at, queue_revision, current_segment_id ON threads FOR EACH ROW EXECUTE FUNCTION thread_continuation_queue();

CREATE TRIGGER threads_continuation_root BEFORE INSERT ON threads FOR EACH ROW EXECUTE FUNCTION thread_continuation_root();

ALTER TABLE agent_shares ADD CONSTRAINT agent_shares_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE CASCADE;

ALTER TABLE approvals ADD CONSTRAINT approvals_requester_thread_id_fkey FOREIGN KEY (requester_thread_id) REFERENCES threads(node_id);

ALTER TABLE approvals ADD CONSTRAINT approvals_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id);

ALTER TABLE browser_sessions ADD CONSTRAINT browser_sessions_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE SET NULL;

ALTER TABLE display_sessions ADD CONSTRAINT display_sessions_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE SET NULL;

ALTER TABLE drafts ADD CONSTRAINT drafts_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE CASCADE;

ALTER TABLE environment_lifetimes ADD CONSTRAINT environment_lifetimes_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE SET NULL;

ALTER TABLE input_schedule_runs ADD CONSTRAINT input_schedule_runs_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE SET NULL;

ALTER TABLE items ADD CONSTRAINT items_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE CASCADE;

ALTER TABLE leases ADD CONSTRAINT leases_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id);

ALTER TABLE port_open_requests ADD CONSTRAINT port_open_requests_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE CASCADE;

ALTER TABLE port_openings ADD CONSTRAINT port_openings_requested_by_thread_fkey FOREIGN KEY (requested_by_thread) REFERENCES threads(node_id) ON DELETE SET NULL;

ALTER TABLE segments ADD CONSTRAINT segments_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE CASCADE;

ALTER TABLE snapshots ADD CONSTRAINT snapshots_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE CASCADE;

ALTER TABLE terminal_sessions ADD CONSTRAINT terminal_sessions_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE SET NULL;

CREATE TABLE thread_item_revisions (
    item_id uuid NOT NULL,
    seq_updated bigint NOT NULL,
    body jsonb NOT NULL,
    CONSTRAINT thread_item_revisions_pkey PRIMARY KEY (item_id, seq_updated),
    CONSTRAINT thread_item_revisions_item_id_fkey FOREIGN KEY (item_id) REFERENCES items(id) ON DELETE CASCADE
);

CREATE INDEX search_revisions_media_idx ON thread_item_revisions USING gin (orch_search_file_types((body -> 'payload'::text))) WHERE (((body ->> 'search_text'::text) IS NOT NULL) AND ((body ->> 'search_text'::text) !~~ '%<orch>%'::text));

CREATE INDEX thread_item_revision_text_idx ON thread_item_revisions USING gin (((body ->> 'search_text'::text)) gin_trgm_ops);

CREATE TRIGGER item_revision_pins AFTER INSERT ON thread_item_revisions FOR EACH ROW EXECUTE FUNCTION thread_item_revision_pins();

CREATE TABLE thread_approval_rules (
    id uuid NOT NULL,
    thread_id uuid NOT NULL,
    approval_id uuid NOT NULL,
    harness_kind text NOT NULL,
    provider_id uuid NOT NULL,
    agent_id uuid NOT NULL,
    environment_id uuid NOT NULL,
    directory text NOT NULL,
    matcher jsonb NOT NULL,
    created_by uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    revoked_at timestamp with time zone,
    revoked_by uuid,
    inactive_reason text,
    revision bigint DEFAULT 1 NOT NULL,
    seq_updated bigint NOT NULL,
    CONSTRAINT thread_approval_rules_pkey PRIMARY KEY (id),
    CONSTRAINT thread_approval_rules_approval_id_fkey FOREIGN KEY (approval_id) REFERENCES approvals(id),
    CONSTRAINT thread_approval_rules_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id),
    CONSTRAINT thread_approval_rules_revoked_by_fkey FOREIGN KEY (revoked_by) REFERENCES users(id),
    CONSTRAINT thread_approval_rules_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE CASCADE
);

CREATE TABLE thread_attachment_uploads (
    thread_id uuid NOT NULL,
    path text NOT NULL,
    name text NOT NULL,
    created_by uuid,
    blob bytea,
    expires_at timestamp with time zone,
    CONSTRAINT thread_attachment_uploads_pkey PRIMARY KEY (thread_id, path),
    CONSTRAINT thread_attachment_uploads_blob_fkey FOREIGN KEY (blob) REFERENCES blobs(hash),
    CONSTRAINT thread_attachment_uploads_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE CASCADE,
    CONSTRAINT thread_attachment_uploads_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE CASCADE
);

CREATE INDEX thread_attachment_uploads_expiry_idx ON thread_attachment_uploads USING btree (expires_at) WHERE (expires_at IS NOT NULL);

CREATE INDEX thread_attachment_uploads_pending_idx ON thread_attachment_uploads USING btree (created_by, thread_id) WHERE (blob IS NOT NULL);

CREATE TRIGGER private_attachment_upload_pin AFTER INSERT OR DELETE OR UPDATE OF blob, expires_at ON thread_attachment_uploads FOR EACH ROW EXECUTE FUNCTION pin_private_attachment_upload();

CREATE TABLE thread_continuation_operations (
    id uuid NOT NULL,
    job_id uuid,
    job_snapshot jsonb NOT NULL,
    thread_id uuid NOT NULL,
    actor_id uuid NOT NULL,
    client_id text NOT NULL,
    body_hash bytea NOT NULL,
    kind text NOT NULL,
    source_continuation_id uuid NOT NULL,
    target_continuation_id uuid NOT NULL,
    input_id uuid,
    segment_id uuid NOT NULL,
    expected_active_continuation_id uuid NOT NULL,
    expected_execution_revision bigint NOT NULL,
    expected_queue_revision bigint NOT NULL,
    status text NOT NULL,
    activation_seq bigint,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT thread_continuation_operations_body_hash_check CHECK ((octet_length(body_hash) = 32)),
    CONSTRAINT thread_continuation_operations_kind_check CHECK ((kind = ANY (ARRAY['edit'::text, 'activate'::text]))),
    CONSTRAINT thread_continuation_operations_status_check CHECK ((status = ANY (ARRAY['preparing'::text, 'committed'::text, 'failed'::text, 'cancelled'::text]))),
    CONSTRAINT thread_continuation_operations_pkey PRIMARY KEY (id),
    CONSTRAINT thread_continuation_operations_thread_id_actor_id_client_id_key UNIQUE (thread_id, actor_id, client_id),
    CONSTRAINT thread_continuation_operations_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES users(id),
    CONSTRAINT thread_continuation_operations_job_id_fkey FOREIGN KEY (job_id) REFERENCES jobs(id) ON DELETE SET NULL,
    CONSTRAINT thread_continuation_operations_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX continuation_transition_one_idx ON thread_continuation_operations USING btree (thread_id) WHERE (status = 'preparing'::text);

CREATE TABLE thread_continuation_spans (
    continuation_id uuid NOT NULL,
    owner_continuation_id uuid NOT NULL,
    first_turn integer NOT NULL,
    last_turn integer NOT NULL,
    first_seq bigint NOT NULL,
    last_seq bigint NOT NULL,
    frozen_at bigint NOT NULL,
    CONSTRAINT thread_continuation_spans_check CHECK ((continuation_id <> owner_continuation_id)),
    CONSTRAINT thread_continuation_spans_check1 CHECK ((last_turn >= (first_turn - 1))),
    CONSTRAINT thread_continuation_spans_check2 CHECK (((last_seq >= first_seq) AND (frozen_at >= last_seq))),
    CONSTRAINT thread_continuation_spans_first_seq_check CHECK ((first_seq >= 0)),
    CONSTRAINT thread_continuation_spans_first_turn_check CHECK ((first_turn > 0)),
    CONSTRAINT thread_continuation_spans_pkey PRIMARY KEY (continuation_id, owner_continuation_id)
);

CREATE INDEX thread_spans_owner_idx ON thread_continuation_spans USING btree (owner_continuation_id, first_seq, last_seq, frozen_at);

CREATE TABLE thread_continuations (
    id uuid NOT NULL,
    thread_id uuid NOT NULL,
    parent_id uuid,
    before_input_id uuid,
    status text NOT NULL,
    created_by uuid,
    created_at timestamp with time zone NOT NULL,
    created_seq bigint NOT NULL,
    current_segment_id uuid,
    projection_revision bigint DEFAULT 1 NOT NULL,
    queue_held boolean DEFAULT false NOT NULL,
    queue_held_by uuid,
    queue_held_at timestamp with time zone,
    queue_revision bigint DEFAULT 1 NOT NULL,
    job_id uuid,
    native_source jsonb,
    native_prefix jsonb,
    open_result jsonb,
    native_creation jsonb,
    native_creation_result jsonb,
    failure jsonb,
    CONSTRAINT thread_continuations_check CHECK (((parent_id IS NULL) OR (parent_id <> id))),
    CONSTRAINT thread_continuations_created_seq_check CHECK ((created_seq >= 0)),
    CONSTRAINT thread_continuations_projection_revision_check CHECK ((projection_revision > 0)),
    CONSTRAINT thread_continuations_queue_revision_check CHECK ((queue_revision > 0)),
    CONSTRAINT thread_continuations_status_check CHECK ((status = ANY (ARRAY['preparing'::text, 'ready'::text, 'failed'::text]))),
    CONSTRAINT thread_continuations_pkey PRIMARY KEY (id),
    CONSTRAINT thread_continuations_thread_id_id_key UNIQUE (thread_id, id),
    CONSTRAINT thread_continuations_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id),
    CONSTRAINT thread_continuations_current_segment_id_fkey FOREIGN KEY (current_segment_id) REFERENCES segments(id),
    CONSTRAINT thread_continuations_job_id_fkey FOREIGN KEY (job_id) REFERENCES jobs(id) ON DELETE SET NULL,
    CONSTRAINT thread_continuations_queue_held_by_fkey FOREIGN KEY (queue_held_by) REFERENCES users(id),
    CONSTRAINT thread_continuations_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT thread_continuations_thread_id_parent_id_fkey FOREIGN KEY (thread_id, parent_id) REFERENCES thread_continuations(thread_id, id)
);

CREATE INDEX continuations_agent_history_creation_idx ON thread_continuations USING btree (((native_creation ->> 'segment_id'::text))) WHERE (native_creation IS NOT NULL);

CREATE INDEX continuations_agent_history_prefix_idx ON thread_continuations USING btree (((native_prefix #>> '{source,native_session_ref}'::text[]))) WHERE (native_prefix IS NOT NULL);

CREATE INDEX continuations_agent_history_source_idx ON thread_continuations USING btree (((native_source ->> 'native_session_ref'::text))) WHERE (native_source IS NOT NULL);

CREATE INDEX thread_continuations_thread_idx ON thread_continuations USING btree (thread_id, created_seq, id);

CREATE TRIGGER continuation_definition BEFORE INSERT OR UPDATE ON thread_continuations FOR EACH ROW EXECUTE FUNCTION thread_continuation_definition();

ALTER TABLE items ADD CONSTRAINT items_continuation_fk FOREIGN KEY (thread_id, continuation_id) REFERENCES thread_continuations(thread_id, id);

ALTER TABLE segments ADD CONSTRAINT segments_continuation_fk FOREIGN KEY (thread_id, continuation_id) REFERENCES thread_continuations(thread_id, id);

ALTER TABLE thread_continuation_operations ADD CONSTRAINT thread_continuation_operation_expected_active_continuation_fkey FOREIGN KEY (expected_active_continuation_id) REFERENCES thread_continuations(id);

ALTER TABLE thread_continuation_operations ADD CONSTRAINT thread_continuation_operations_source_continuation_id_fkey FOREIGN KEY (source_continuation_id) REFERENCES thread_continuations(id);

ALTER TABLE thread_continuation_operations ADD CONSTRAINT thread_continuation_operations_target_continuation_id_fkey FOREIGN KEY (target_continuation_id) REFERENCES thread_continuations(id);

ALTER TABLE thread_continuation_spans ADD CONSTRAINT thread_continuation_spans_continuation_id_fkey FOREIGN KEY (continuation_id) REFERENCES thread_continuations(id) ON DELETE CASCADE;

ALTER TABLE thread_continuation_spans ADD CONSTRAINT thread_continuation_spans_owner_continuation_id_fkey FOREIGN KEY (owner_continuation_id) REFERENCES thread_continuations(id) ON DELETE CASCADE;

ALTER TABLE threads ADD CONSTRAINT threads_active_continuation_fk FOREIGN KEY (node_id, active_continuation_id) REFERENCES thread_continuations(thread_id, id) DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE thread_draft_blob_refs (
    user_id uuid NOT NULL,
    thread_id uuid NOT NULL,
    blob bytea NOT NULL,
    CONSTRAINT thread_draft_blob_refs_pkey PRIMARY KEY (user_id, thread_id, blob),
    CONSTRAINT thread_draft_blob_refs_blob_fkey FOREIGN KEY (blob) REFERENCES blobs(hash),
    CONSTRAINT thread_draft_blob_refs_user_id_thread_id_fkey FOREIGN KEY (user_id, thread_id) REFERENCES drafts(user_id, thread_id) ON DELETE CASCADE
);

CREATE INDEX thread_draft_blob_refs_blob_idx ON thread_draft_blob_refs USING btree (blob);

CREATE TRIGGER private_thread_draft_blob_pin AFTER INSERT OR DELETE ON thread_draft_blob_refs FOR EACH ROW EXECUTE FUNCTION thread_history_blob_pin();

CREATE TABLE thread_goal_reports (
    id uuid NOT NULL,
    goal_id uuid NOT NULL,
    thread_id uuid NOT NULL,
    turn_id uuid,
    status text NOT NULL,
    evidence text DEFAULT ''::text NOT NULL,
    next text,
    outcome text,
    checked_by text,
    reason text,
    attempts integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    decided_at timestamp with time zone,
    CONSTRAINT thread_goal_reports_checked_by_check CHECK ((checked_by = ANY (ARRAY['helper'::text, 'agent'::text]))),
    CONSTRAINT thread_goal_reports_outcome_check CHECK ((outcome = ANY (ARRAY['noted'::text, 'met'::text, 'not_yet'::text, 'blocked'::text]))),
    CONSTRAINT thread_goal_reports_status_check CHECK ((status = ANY (ARRAY['working'::text, 'met'::text, 'blocked'::text]))),
    CONSTRAINT thread_goal_reports_pkey PRIMARY KEY (id),
    CONSTRAINT thread_goal_reports_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE CASCADE
);

CREATE INDEX thread_goal_reports_goal_idx ON thread_goal_reports USING btree (goal_id, created_at DESC, id DESC);

CREATE TABLE thread_goals (
    id uuid NOT NULL,
    thread_id uuid NOT NULL,
    objective text NOT NULL,
    condition text NOT NULL,
    set_by uuid,
    set_authority jsonb,
    set_at timestamp with time zone NOT NULL,
    status text NOT NULL,
    reason text,
    detail text,
    plan_first boolean DEFAULT false NOT NULL,
    notify_on_end boolean DEFAULT false NOT NULL,
    max_turns bigint,
    max_minutes bigint,
    max_credits numeric,
    turns_used bigint DEFAULT 0 NOT NULL,
    tokens_used bigint DEFAULT 0 NOT NULL,
    credits_used numeric DEFAULT 0 NOT NULL,
    elapsed_ms bigint DEFAULT 0 NOT NULL,
    active_since timestamp with time zone,
    after_turn integer DEFAULT 0 NOT NULL,
    counted_turn integer DEFAULT 0 NOT NULL,
    checking_until timestamp with time zone,
    ended_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    retry_attempts bigint DEFAULT 0 NOT NULL,
    CONSTRAINT thread_goals_check CHECK (((status <> ALL (ARRAY['paused'::text, 'stopped'::text])) OR (reason IS NOT NULL))),
    CONSTRAINT thread_goals_check1 CHECK (((status <> 'active'::text) OR (active_since IS NOT NULL))),
    CONSTRAINT thread_goals_condition_check CHECK ((btrim(condition) <> ''::text)),
    CONSTRAINT thread_goals_max_credits_check CHECK (((max_credits IS NULL) OR (max_credits >= (0)::numeric))),
    CONSTRAINT thread_goals_max_minutes_check CHECK (((max_minutes IS NULL) OR (max_minutes >= 0))),
    CONSTRAINT thread_goals_max_turns_check CHECK (((max_turns IS NULL) OR (max_turns >= 0))),
    CONSTRAINT thread_goals_objective_check CHECK ((btrim(objective) <> ''::text)),
    CONSTRAINT thread_goals_reason_check CHECK ((reason = ANY (ARRAY['person'::text, 'interrupted'::text, 'blocked'::text, 'failed'::text, 'turns'::text, 'minutes'::text, 'credits'::text, 'access'::text, 'archived'::text]))),
    CONSTRAINT thread_goals_retry_attempts_check CHECK ((retry_attempts >= 0)),
    CONSTRAINT thread_goals_status_check CHECK ((status = ANY (ARRAY['active'::text, 'paused'::text, 'met'::text, 'stopped'::text, 'cleared'::text]))),
    CONSTRAINT thread_goals_pkey PRIMARY KEY (id),
    CONSTRAINT thread_goals_set_by_fkey FOREIGN KEY (set_by) REFERENCES users(id) ON DELETE SET NULL,
    CONSTRAINT thread_goals_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE CASCADE
);

CREATE INDEX thread_goals_active_idx ON thread_goals USING btree (thread_id) WHERE (status = 'active'::text);

CREATE INDEX thread_goals_ended_idx ON thread_goals USING btree (thread_id, set_at DESC, id DESC) WHERE (status = ANY (ARRAY['met'::text, 'stopped'::text, 'cleared'::text]));

CREATE UNIQUE INDEX thread_goals_live_idx ON thread_goals USING btree (thread_id) WHERE (status = ANY (ARRAY['active'::text, 'paused'::text]));

CREATE INDEX thread_goals_thread_idx ON thread_goals USING btree (thread_id, set_at DESC);

ALTER TABLE thread_goal_reports ADD CONSTRAINT thread_goal_reports_goal_id_fkey FOREIGN KEY (goal_id) REFERENCES thread_goals(id) ON DELETE CASCADE;

CREATE TABLE thread_history_cut_blobs (
    cut_id uuid NOT NULL,
    blob bytea NOT NULL,
    CONSTRAINT thread_history_cut_blobs_pkey PRIMARY KEY (cut_id, blob),
    CONSTRAINT thread_history_cut_blobs_blob_fkey FOREIGN KEY (blob) REFERENCES blobs(hash)
);

CREATE TRIGGER history_cut_blob_pin AFTER INSERT OR DELETE ON thread_history_cut_blobs FOR EACH ROW EXECUTE FUNCTION thread_history_blob_pin();

CREATE TABLE thread_history_cut_commands (
    cut_id uuid NOT NULL,
    command_id uuid NOT NULL,
    runtime_id uuid,
    retirement_machine_id uuid,
    settled boolean DEFAULT false NOT NULL,
    fence_id uuid,
    fence_attempt bigint DEFAULT 0 NOT NULL,
    CONSTRAINT thread_history_cut_commands_fence_attempt_check CHECK ((fence_attempt >= 0)),
    CONSTRAINT thread_history_cut_commands_pkey PRIMARY KEY (command_id),
    CONSTRAINT thread_history_cut_commands_command_id_fkey FOREIGN KEY (command_id) REFERENCES commands(id),
    CONSTRAINT thread_history_cut_commands_fence_id_fkey FOREIGN KEY (fence_id) REFERENCES commands(id)
);

CREATE INDEX history_cut_commands_fence_idx ON thread_history_cut_commands USING btree (fence_id) WHERE (fence_id IS NOT NULL);

CREATE INDEX history_cut_commands_owner_idx ON thread_history_cut_commands USING btree (cut_id, settled, command_id);

CREATE INDEX history_cut_commands_unsettled_idx ON thread_history_cut_commands USING btree (cut_id, command_id) WHERE (NOT settled);

CREATE TABLE thread_history_cut_items (
    cut_id uuid NOT NULL,
    item_id uuid NOT NULL,
    seq_created bigint NOT NULL,
    turn_id uuid,
    stream text NOT NULL,
    kind text NOT NULL,
    parent_cut_id uuid,
    revision_seq bigint,
    native_alias text,
    body jsonb,
    CONSTRAINT thread_history_cut_items_check CHECK (((parent_cut_id IS NULL) OR (parent_cut_id <> cut_id))),
    CONSTRAINT thread_history_cut_items_cut_id_seq_created_key UNIQUE (cut_id, seq_created),
    CONSTRAINT thread_history_cut_items_pkey PRIMARY KEY (cut_id, item_id),
    CONSTRAINT thread_history_cut_items_item_id_revision_seq_fkey FOREIGN KEY (item_id, revision_seq) REFERENCES thread_item_revisions(item_id, seq_updated) DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT thread_history_cut_items_parent_cut_id_item_id_fkey FOREIGN KEY (parent_cut_id, item_id) REFERENCES thread_history_cut_items(cut_id, item_id) DEFERRABLE INITIALLY DEFERRED
);

CREATE INDEX history_cut_items_turn_idx ON thread_history_cut_items USING btree (cut_id, turn_id, seq_created);

CREATE CONSTRAINT TRIGGER history_cut_parent AFTER INSERT OR UPDATE ON thread_history_cut_items DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION thread_history_cut_parent();

CREATE TABLE thread_history_cut_rows (
    cut_id uuid NOT NULL,
    kind text NOT NULL,
    row_id uuid NOT NULL,
    ordinal integer DEFAULT 0 NOT NULL,
    body jsonb NOT NULL,
    CONSTRAINT thread_history_cut_rows_kind_check CHECK ((kind = ANY (ARRAY['segment'::text, 'input'::text, 'turn'::text, 'approval'::text]))),
    CONSTRAINT thread_history_cut_rows_pkey PRIMARY KEY (cut_id, kind, row_id)
);

CREATE INDEX history_cut_rows_order_idx ON thread_history_cut_rows USING btree (cut_id, kind, ordinal, row_id);

CREATE TABLE thread_history_cuts (
    id uuid NOT NULL,
    thread_id uuid NOT NULL,
    continuation_id uuid NOT NULL,
    job_id uuid,
    owner_thread_id uuid,
    source_segment_id uuid,
    source_native_ref text,
    execution_revision bigint NOT NULL,
    first_turn integer NOT NULL,
    last_turn integer NOT NULL,
    captured_seq bigint NOT NULL,
    captured_at timestamp with time zone NOT NULL,
    head jsonb NOT NULL,
    artifacts jsonb DEFAULT '{}'::jsonb NOT NULL,
    CONSTRAINT thread_history_cuts_captured_seq_check CHECK ((captured_seq >= 0)),
    CONSTRAINT thread_history_cuts_check CHECK ((num_nonnulls(job_id, owner_thread_id) = 1)),
    CONSTRAINT thread_history_cuts_check1 CHECK ((((job_id IS NULL) OR (id = job_id)) AND ((owner_thread_id IS NULL) OR (id = owner_thread_id)))),
    CONSTRAINT thread_history_cuts_check2 CHECK (((last_turn = 0) OR (last_turn >= first_turn))),
    CONSTRAINT thread_history_cuts_execution_revision_check CHECK ((execution_revision > 0)),
    CONSTRAINT thread_history_cuts_first_turn_check CHECK ((first_turn > 0)),
    CONSTRAINT thread_history_cuts_last_turn_check CHECK ((last_turn >= 0)),
    CONSTRAINT thread_history_cuts_job_id_key UNIQUE (job_id),
    CONSTRAINT thread_history_cuts_owner_thread_id_key UNIQUE (owner_thread_id),
    CONSTRAINT thread_history_cuts_pkey PRIMARY KEY (id),
    CONSTRAINT thread_history_cuts_continuation_id_fkey FOREIGN KEY (continuation_id) REFERENCES thread_continuations(id) DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT thread_history_cuts_job_id_fkey FOREIGN KEY (job_id) REFERENCES jobs(id) ON DELETE CASCADE,
    CONSTRAINT thread_history_cuts_owner_thread_id_fkey FOREIGN KEY (owner_thread_id) REFERENCES threads(node_id) ON DELETE CASCADE,
    CONSTRAINT thread_history_cuts_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) DEFERRABLE INITIALLY DEFERRED
);

CREATE INDEX history_cuts_agent_history_source_idx ON thread_history_cuts USING btree (source_native_ref, source_segment_id) WHERE (source_native_ref IS NOT NULL);

CREATE INDEX history_cuts_source_idx ON thread_history_cuts USING btree (thread_id, continuation_id);

CREATE INDEX thread_history_cuts_native_history_idx ON thread_history_cuts USING btree (source_segment_id) WHERE (source_native_ref IS NOT NULL);

CREATE TRIGGER history_cut_identity BEFORE UPDATE ON thread_history_cuts FOR EACH ROW EXECUTE FUNCTION thread_history_cut_identity();

CREATE TRIGGER history_cut_release BEFORE DELETE ON thread_history_cuts FOR EACH ROW EXECUTE FUNCTION thread_history_cut_release();

ALTER TABLE thread_history_cut_blobs ADD CONSTRAINT thread_history_cut_blobs_cut_id_fkey FOREIGN KEY (cut_id) REFERENCES thread_history_cuts(id) ON DELETE CASCADE;

ALTER TABLE thread_history_cut_commands ADD CONSTRAINT thread_history_cut_commands_cut_id_fkey FOREIGN KEY (cut_id) REFERENCES thread_history_cuts(id) ON DELETE CASCADE;

ALTER TABLE thread_history_cut_items ADD CONSTRAINT thread_history_cut_items_cut_id_fkey FOREIGN KEY (cut_id) REFERENCES thread_history_cuts(id) ON DELETE CASCADE;

ALTER TABLE thread_history_cut_rows ADD CONSTRAINT thread_history_cut_rows_cut_id_fkey FOREIGN KEY (cut_id) REFERENCES thread_history_cuts(id) ON DELETE CASCADE;

CREATE TABLE thread_history_cut_spans (
    cut_id uuid NOT NULL,
    thread_id uuid NOT NULL,
    owner_continuation_id uuid NOT NULL,
    first_seq bigint NOT NULL,
    last_seq bigint NOT NULL,
    frozen_at bigint NOT NULL,
    seq_offset bigint NOT NULL,
    loose_before_orch boolean NOT NULL,
    loose_low bigint NOT NULL,
    loose_high bigint NOT NULL,
    CONSTRAINT thread_history_cut_spans_check CHECK (((first_seq >= 0) AND (last_seq >= (first_seq - 1)) AND (frozen_at >= last_seq) AND (seq_offset >= 0))),
    CONSTRAINT thread_history_cut_spans_pkey PRIMARY KEY (cut_id, thread_id, owner_continuation_id),
    CONSTRAINT thread_history_cut_spans_cut_id_fkey FOREIGN KEY (cut_id) REFERENCES thread_history_cuts(id) ON DELETE CASCADE
);

CREATE INDEX history_cut_spans_cover_idx ON thread_history_cut_spans USING btree (thread_id, owner_continuation_id, first_seq, last_seq);

COMMENT ON TABLE thread_history_cut_spans IS 'What a cut holds of a thread: the items of one continuation between two positions, in its turns or loose as given, as they stood at frozen_at. thread_history_cut_items keeps only the copies of held items changed or removed since, and proved native aliases.';

CREATE TABLE thread_input_blob_refs (
    input_id uuid NOT NULL,
    blob bytea NOT NULL,
    CONSTRAINT thread_input_blob_refs_pkey PRIMARY KEY (input_id, blob),
    CONSTRAINT thread_input_blob_refs_blob_fkey FOREIGN KEY (blob) REFERENCES blobs(hash)
);

CREATE TABLE thread_inputs (
    id uuid NOT NULL,
    thread_id uuid NOT NULL,
    author_id uuid,
    source text NOT NULL,
    content jsonb NOT NULL,
    native_action jsonb,
    native_action_info jsonb,
    delivery text NOT NULL,
    state text DEFAULT 'pending'::text NOT NULL,
    state_detail text,
    attachment_issues jsonb DEFAULT '[]'::jsonb NOT NULL,
    queue_held boolean DEFAULT false NOT NULL,
    named boolean DEFAULT false NOT NULL,
    limit_bypass jsonb,
    "position" numeric NOT NULL,
    turn_id uuid,
    client_id text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    delivered_at timestamp with time zone,
    cancelled_by uuid,
    seq_created bigint DEFAULT 0 NOT NULL,
    seq_updated bigint NOT NULL,
    continuation_id uuid NOT NULL,
    run_as uuid,
    plan boolean DEFAULT false NOT NULL,
    submission_authority jsonb,
    run_as_authority jsonb,
    approval_id uuid,
    goal_id uuid,
    goal_facts jsonb,
    CONSTRAINT thread_inputs_delivery_check CHECK ((delivery = ANY (ARRAY['queue'::text, 'steer'::text, 'interrupt'::text]))),
    CONSTRAINT thread_inputs_run_as_authority_check CHECK (((run_as_authority IS NULL) OR COALESCE(((jsonb_typeof(run_as_authority) = 'object'::text) AND (jsonb_typeof((run_as_authority -> 'credentials'::text)) = 'array'::text) AND (jsonb_array_length((run_as_authority -> 'credentials'::text)) > 0)), false))),
    CONSTRAINT thread_inputs_source_check CHECK ((source = ANY (ARRAY['user'::text, 'announcement'::text, 'system'::text, 'goal'::text]))),
    CONSTRAINT thread_inputs_state_check CHECK ((state = ANY (ARRAY['pending'::text, 'delivering'::text, 'delivered'::text, 'cancelled'::text, 'failed'::text]))),
    CONSTRAINT thread_inputs_submission_authority_check CHECK (((submission_authority IS NULL) OR COALESCE(((jsonb_typeof(submission_authority) = 'object'::text) AND (jsonb_typeof((submission_authority -> 'credentials'::text)) = 'array'::text) AND (jsonb_array_length((submission_authority -> 'credentials'::text)) > 0)), false))),
    CONSTRAINT thread_input_thread_identity UNIQUE (thread_id, id),
    CONSTRAINT thread_inputs_pkey PRIMARY KEY (id),
    CONSTRAINT thread_inputs_thread_id_author_id_client_id_key UNIQUE (thread_id, author_id, client_id),
    CONSTRAINT inputs_continuation_fk FOREIGN KEY (thread_id, continuation_id) REFERENCES thread_continuations(thread_id, id),
    CONSTRAINT thread_inputs_approval_id_fkey FOREIGN KEY (approval_id) REFERENCES approvals(id),
    CONSTRAINT thread_inputs_author_id_fkey FOREIGN KEY (author_id) REFERENCES users(id),
    CONSTRAINT thread_inputs_cancelled_by_fkey FOREIGN KEY (cancelled_by) REFERENCES users(id),
    CONSTRAINT thread_inputs_goal_id_fkey FOREIGN KEY (goal_id) REFERENCES thread_goals(id) ON DELETE SET NULL,
    CONSTRAINT thread_inputs_run_as_fkey FOREIGN KEY (run_as) REFERENCES users(id) ON DELETE SET NULL,
    CONSTRAINT thread_inputs_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE CASCADE
);

CREATE INDEX inputs_agent_history_waiting_idx ON thread_inputs USING btree (thread_id) WHERE (state = ANY (ARRAY['pending'::text, 'delivering'::text]));

CREATE INDEX inputs_continuation_pending_idx ON thread_inputs USING btree (thread_id, continuation_id, "position") WHERE (state = 'pending'::text);

CREATE INDEX inputs_continuation_updated_idx ON thread_inputs USING btree (thread_id, continuation_id, seq_updated);

CREATE INDEX inputs_custody_waiting_idx ON thread_inputs USING btree (thread_id, continuation_id, created_at) INCLUDE (id) WHERE ((state = 'pending'::text) AND (NOT queue_held));

CREATE INDEX inputs_uncertain_history_idx ON thread_inputs USING btree (thread_id, id) WHERE (state <> 'delivered'::text);

CREATE INDEX thread_display_accepted_idx ON thread_inputs USING btree (thread_id, seq_created) WHERE (seq_created > 0);

CREATE INDEX thread_display_imported_idx ON thread_inputs USING btree (thread_id, created_at) WHERE (seq_created = 0);

CREATE INDEX thread_display_inputs_live_idx ON thread_inputs USING btree (thread_id, continuation_id) WHERE (state = ANY (ARRAY['pending'::text, 'delivering'::text]));

CREATE INDEX thread_inputs_authors_idx ON thread_inputs USING btree (thread_id, author_id) WHERE ((author_id IS NOT NULL) AND (state = ANY (ARRAY['delivering'::text, 'delivered'::text])));

CREATE INDEX thread_inputs_delivered_idx ON thread_inputs USING btree (thread_id, delivered_at DESC NULLS LAST, id DESC) WHERE ((author_id IS NOT NULL) AND (state = 'delivered'::text));

CREATE INDEX thread_inputs_goal_lost_idx ON thread_inputs USING btree (goal_id) WHERE ((goal_id IS NOT NULL) AND (state = ANY (ARRAY['cancelled'::text, 'failed'::text])));

CREATE INDEX thread_inputs_pending_idx ON thread_inputs USING btree (thread_id, "position") WHERE (state = 'pending'::text);

CREATE TRIGGER inputs_continuation_owner BEFORE INSERT OR UPDATE OF continuation_id, thread_id ON thread_inputs FOR EACH ROW EXECUTE FUNCTION thread_continuation_owner();

CREATE TRIGGER thread_inputs_goal_retry_attempt AFTER INSERT OR UPDATE OF state, goal_id ON thread_inputs FOR EACH ROW EXECUTE FUNCTION goal_note_retry_attempt();

ALTER TABLE thread_continuations ADD CONSTRAINT continuation_input_thread_fk FOREIGN KEY (thread_id, before_input_id) REFERENCES thread_inputs(thread_id, id);

ALTER TABLE input_schedule_runs ADD CONSTRAINT input_schedule_runs_input_id_fkey FOREIGN KEY (input_id) REFERENCES thread_inputs(id) ON DELETE SET NULL;

ALTER TABLE model_inputs ADD CONSTRAINT model_inputs_input_id_fkey FOREIGN KEY (input_id) REFERENCES thread_inputs(id);

ALTER TABLE providers ADD CONSTRAINT providers_credential_trial_input_id_fkey FOREIGN KEY (credential_trial_input_id) REFERENCES thread_inputs(id) ON DELETE SET NULL;

ALTER TABLE thread_continuation_operations ADD CONSTRAINT thread_continuation_operations_input_id_fkey FOREIGN KEY (input_id) REFERENCES thread_inputs(id);

ALTER TABLE thread_continuations ADD CONSTRAINT thread_continuations_before_input_id_fkey FOREIGN KEY (before_input_id) REFERENCES thread_inputs(id);

ALTER TABLE thread_input_blob_refs ADD CONSTRAINT thread_input_blob_refs_input_id_fkey FOREIGN KEY (input_id) REFERENCES thread_inputs(id) ON DELETE CASCADE;

CREATE TABLE thread_item_blob_refs (
    item_id uuid NOT NULL,
    blob bytea NOT NULL,
    CONSTRAINT thread_item_blob_refs_pkey PRIMARY KEY (item_id, blob),
    CONSTRAINT thread_item_blob_refs_blob_fkey FOREIGN KEY (blob) REFERENCES blobs(hash),
    CONSTRAINT thread_item_blob_refs_item_id_fkey FOREIGN KEY (item_id) REFERENCES items(id) ON DELETE CASCADE
);

CREATE TRIGGER item_blob_pin AFTER INSERT OR DELETE ON thread_item_blob_refs FOR EACH ROW EXECUTE FUNCTION thread_history_blob_pin();

CREATE TABLE thread_item_revision_blob_refs (
    item_id uuid NOT NULL,
    seq_updated bigint NOT NULL,
    blob bytea NOT NULL,
    CONSTRAINT thread_item_revision_blob_refs_pkey PRIMARY KEY (item_id, seq_updated, blob),
    CONSTRAINT thread_item_revision_blob_refs_blob_fkey FOREIGN KEY (blob) REFERENCES blobs(hash),
    CONSTRAINT thread_item_revision_blob_refs_item_id_seq_updated_fkey FOREIGN KEY (item_id, seq_updated) REFERENCES thread_item_revisions(item_id, seq_updated) ON DELETE CASCADE
);

CREATE TRIGGER item_revision_blob_pin AFTER INSERT OR DELETE ON thread_item_revision_blob_refs FOR EACH ROW EXECUTE FUNCTION thread_history_blob_pin();

CREATE TABLE thread_native_guards (
    thread_id uuid NOT NULL,
    preparation_job uuid,
    empty_segment_id uuid,
    ready boolean DEFAULT true NOT NULL,
    CONSTRAINT thread_native_guards_pkey PRIMARY KEY (thread_id),
    CONSTRAINT thread_native_guards_preparation_job_fkey FOREIGN KEY (preparation_job) REFERENCES jobs(id) ON DELETE SET NULL,
    CONSTRAINT thread_native_guards_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE CASCADE
);

CREATE TABLE thread_native_item_aliases (
    item_id uuid NOT NULL,
    native_ref text NOT NULL,
    CONSTRAINT thread_native_item_aliases_native_ref_check CHECK ((native_ref <> ''::text)),
    CONSTRAINT thread_native_item_aliases_pkey PRIMARY KEY (item_id),
    CONSTRAINT thread_native_item_aliases_item_id_fkey FOREIGN KEY (item_id) REFERENCES items(id) ON DELETE CASCADE
);

CREATE TRIGGER native_alias_identity BEFORE UPDATE ON thread_native_item_aliases FOR EACH ROW EXECUTE FUNCTION thread_native_alias_identity();

CREATE TABLE thread_pending_notices (
    thread_id uuid NOT NULL,
    "position" bigint GENERATED ALWAYS AS IDENTITY (SEQUENCE NAME thread_pending_notices_position_seq START WITH 1 INCREMENT BY 1 NO MINVALUE NO MAXVALUE CACHE 1),
    notice jsonb NOT NULL,
    notice_id text GENERATED ALWAYS AS ((notice ->> 'id'::text)) STORED,
    dedup_key text,
    recipient_segment_id uuid,
    lane_key bytea DEFAULT '\x'::bytea NOT NULL,
    CONSTRAINT thread_pending_notices_pkey PRIMARY KEY (thread_id, "position"),
    CONSTRAINT thread_pending_notices_thread_id_dedup_key_key UNIQUE (thread_id, dedup_key),
    CONSTRAINT thread_pending_notices_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE CASCADE
);

CREATE INDEX thread_pending_notices_identity_idx ON thread_pending_notices USING btree (thread_id, notice_id, "position");

CREATE INDEX thread_pending_notices_lane_idx ON thread_pending_notices USING btree (thread_id, lane_key, "position");

CREATE INDEX thread_pending_notices_recipient_idx ON thread_pending_notices USING btree (recipient_segment_id) WHERE (recipient_segment_id IS NOT NULL);

CREATE TRIGGER pending_notice_preview_added AFTER INSERT ON thread_pending_notices REFERENCING NEW TABLE AS added_notices FOR EACH STATEMENT EXECUTE FUNCTION orch_pending_notice_preview_added();

CREATE TRIGGER pending_notice_preview_removed AFTER DELETE ON thread_pending_notices REFERENCING OLD TABLE AS removed_notices FOR EACH STATEMENT EXECUTE FUNCTION orch_pending_notice_preview_removed();

CREATE TRIGGER pending_notice_recipient BEFORE INSERT OR UPDATE OF notice ON thread_pending_notices FOR EACH ROW EXECUTE FUNCTION orch_pending_notice_recipient();

CREATE TABLE thread_prompt_groups (
    id uuid NOT NULL,
    thread_id uuid NOT NULL,
    total bigint DEFAULT 1 NOT NULL,
    seq_updated bigint NOT NULL,
    CONSTRAINT thread_prompt_groups_total_check CHECK ((total > 0)),
    CONSTRAINT thread_prompt_groups_pkey PRIMARY KEY (id),
    CONSTRAINT thread_prompt_groups_id_fkey FOREIGN KEY (id) REFERENCES thread_inputs(id) ON DELETE CASCADE,
    CONSTRAINT thread_prompt_groups_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE CASCADE
);

CREATE INDEX thread_prompt_groups_changed_idx ON thread_prompt_groups USING btree (thread_id, seq_updated, id);

CREATE TABLE thread_prompt_versions (
    input_id uuid NOT NULL,
    group_id uuid NOT NULL,
    continuation_id uuid NOT NULL,
    ordinal bigint NOT NULL,
    job_id uuid,
    operation_id uuid,
    CONSTRAINT thread_prompt_versions_ordinal_check CHECK ((ordinal > 0)),
    CONSTRAINT thread_prompt_versions_group_id_ordinal_key UNIQUE (group_id, ordinal),
    CONSTRAINT thread_prompt_versions_pkey PRIMARY KEY (input_id),
    CONSTRAINT prompt_version_operation_fk FOREIGN KEY (operation_id) REFERENCES thread_continuation_operations(id) ON DELETE SET NULL,
    CONSTRAINT thread_prompt_versions_continuation_id_fkey FOREIGN KEY (continuation_id) REFERENCES thread_continuations(id) ON DELETE CASCADE,
    CONSTRAINT thread_prompt_versions_group_id_fkey FOREIGN KEY (group_id) REFERENCES thread_prompt_groups(id) ON DELETE CASCADE,
    CONSTRAINT thread_prompt_versions_input_id_fkey FOREIGN KEY (input_id) REFERENCES thread_inputs(id) ON DELETE CASCADE,
    CONSTRAINT thread_prompt_versions_job_id_fkey FOREIGN KEY (job_id) REFERENCES jobs(id) ON DELETE SET NULL
);

CREATE INDEX prompt_versions_continuation_idx ON thread_prompt_versions USING btree (continuation_id, input_id);

CREATE UNIQUE INDEX prompt_versions_edited_continuation_idx ON thread_prompt_versions USING btree (continuation_id) WHERE (input_id <> group_id);

CREATE TABLE thread_reads (
    user_id uuid NOT NULL,
    thread_id uuid NOT NULL,
    last_read_seq bigint NOT NULL,
    last_read_at timestamp with time zone DEFAULT now() NOT NULL,
    continuation_id uuid NOT NULL,
    CONSTRAINT thread_reads_pkey PRIMARY KEY (user_id, thread_id, continuation_id),
    CONSTRAINT thread_reads_continuation_fk FOREIGN KEY (thread_id, continuation_id) REFERENCES thread_continuations(thread_id, id) ON DELETE CASCADE,
    CONSTRAINT thread_reads_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE CASCADE,
    CONSTRAINT thread_reads_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TRIGGER thread_read_continuation BEFORE INSERT ON thread_reads FOR EACH ROW EXECUTE FUNCTION thread_read_continuation();

CREATE TABLE turns (
    id uuid NOT NULL,
    thread_id uuid NOT NULL,
    segment_id uuid NOT NULL,
    ordinal integer NOT NULL,
    state text NOT NULL,
    input_id uuid NOT NULL,
    audit_for_user_id uuid,
    before_snapshot_id uuid,
    end_manifest_blob bytea,
    files_work_pending boolean DEFAULT false NOT NULL,
    files_end_manifest jsonb,
    files_end_due boolean DEFAULT false NOT NULL,
    model text NOT NULL,
    effort text,
    params jsonb DEFAULT '{}'::jsonb NOT NULL,
    lease_id uuid,
    native_turn_ref text,
    native_end_confirmed boolean DEFAULT false NOT NULL,
    command_cleanup jsonb,
    interrupt_cause text,
    interrupted_by uuid,
    cut_notice_sent boolean DEFAULT false NOT NULL,
    error jsonb,
    usage jsonb,
    usage_detail jsonb,
    end_work jsonb,
    context jsonb,
    limit_bypass jsonb,
    phase text,
    phase_at timestamp with time zone,
    waiting_since timestamp with time zone,
    waited_ms bigint DEFAULT 0 NOT NULL,
    started_at timestamp with time zone,
    completed_at timestamp with time zone,
    completed_seq bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    seq_updated bigint NOT NULL,
    continuation_id uuid NOT NULL,
    skill_versions uuid[],
    plan boolean DEFAULT false NOT NULL,
    resource_delivery jsonb,
    pending_notice_positions bigint[],
    CONSTRAINT turns_interrupt_cause_check CHECK (((interrupt_cause = ANY (ARRAY['person'::text, 'usage_limit'::text, 'environment_change'::text, 'topic_setting'::text])) OR (interrupt_cause = ANY ('{resource_disabled}'::text[])))),
    CONSTRAINT turns_pending_notice_positions_check CHECK (((pending_notice_positions IS NULL) OR (cardinality(pending_notice_positions) <= 64))),
    CONSTRAINT turns_phase_check CHECK ((phase = ANY (ARRAY['working'::text, 'compacting'::text]))),
    CONSTRAINT turns_state_check CHECK ((state = ANY (ARRAY['starting'::text, 'running'::text, 'completed'::text, 'interrupted'::text, 'failed'::text]))),
    CONSTRAINT turns_waited_ms_check CHECK ((waited_ms >= 0)),
    CONSTRAINT turns_pkey PRIMARY KEY (id),
    CONSTRAINT turns_thread_id_ordinal_key UNIQUE (thread_id, ordinal),
    CONSTRAINT turns_audit_for_user_id_fkey FOREIGN KEY (audit_for_user_id) REFERENCES users(id),
    CONSTRAINT turns_before_snapshot_fk FOREIGN KEY (before_snapshot_id) REFERENCES snapshots(id),
    CONSTRAINT turns_continuation_fk FOREIGN KEY (thread_id, continuation_id) REFERENCES thread_continuations(thread_id, id),
    CONSTRAINT turns_end_manifest_blob_fkey FOREIGN KEY (end_manifest_blob) REFERENCES blobs(hash),
    CONSTRAINT turns_input_id_fkey FOREIGN KEY (input_id) REFERENCES thread_inputs(id),
    CONSTRAINT turns_interrupted_by_fkey FOREIGN KEY (interrupted_by) REFERENCES users(id),
    CONSTRAINT turns_segment_id_fkey FOREIGN KEY (segment_id) REFERENCES segments(id),
    CONSTRAINT turns_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES threads(node_id) ON DELETE CASCADE
);

CREATE INDEX thread_display_completed_idx ON turns USING btree (thread_id, continuation_id, ordinal DESC) WHERE (state = 'completed'::text);

CREATE INDEX turns_active_thread_idx ON turns USING btree (thread_id) WHERE (state = ANY (ARRAY['starting'::text, 'running'::text]));

CREATE INDEX turns_agent_history_activity_idx ON turns USING btree (thread_id, GREATEST(created_at, started_at, completed_at) DESC);

CREATE INDEX turns_continuation_ordinal_idx ON turns USING btree (thread_id, continuation_id, ordinal);

CREATE INDEX turns_continuation_updated_idx ON turns USING btree (thread_id, continuation_id, seq_updated);

CREATE INDEX turns_end_due_idx ON turns USING btree (thread_id) WHERE files_end_due;

CREATE INDEX turns_end_work_idx ON turns USING btree (thread_id, ordinal) WHERE (end_work IS NOT NULL);

CREATE INDEX turns_files_work_idx ON turns USING btree (id) WHERE (files_work_pending AND (completed_at IS NOT NULL));

CREATE INDEX turns_input_history_idx ON turns USING btree (thread_id, input_id) INCLUDE (continuation_id, ordinal);

CREATE INDEX turns_segment_idx ON turns USING btree (segment_id);

CREATE INDEX turns_segment_use_idx ON turns USING btree (segment_id, started_at) WHERE (started_at IS NOT NULL);

CREATE INDEX turns_skill_facts_idx ON turns USING gin (skill_versions);

CREATE INDEX turns_thread_ordinal_idx ON turns USING btree (thread_id, ordinal);

CREATE INDEX turns_unconfirmed_native_idx ON turns USING btree (thread_id, segment_id) WHERE ((state = 'interrupted'::text) AND ((error ->> 'code'::text) = 'forced'::text) AND (NOT native_end_confirmed));

CREATE TRIGGER turns_continuation_owner BEFORE INSERT OR UPDATE OF continuation_id, thread_id, segment_id, input_id ON turns FOR EACH ROW EXECUTE FUNCTION thread_continuation_owner();

CREATE TRIGGER turns_provider_trial_ended AFTER UPDATE OF state ON turns FOR EACH ROW WHEN ((new.state <> ALL (ARRAY['starting'::text, 'running'::text]))) EXECUTE FUNCTION provider_trial_ended();

ALTER TABLE approvals ADD CONSTRAINT approvals_turn_id_fkey FOREIGN KEY (turn_id) REFERENCES turns(id);

ALTER TABLE input_schedule_runs ADD CONSTRAINT input_schedule_runs_turn_id_fkey FOREIGN KEY (turn_id) REFERENCES turns(id) ON DELETE SET NULL;

ALTER TABLE items ADD CONSTRAINT items_turn_id_fkey FOREIGN KEY (turn_id) REFERENCES turns(id);

ALTER TABLE leases ADD CONSTRAINT leases_turn_id_fkey FOREIGN KEY (turn_id) REFERENCES turns(id);

ALTER TABLE model_inputs ADD CONSTRAINT model_inputs_turn_id_fkey FOREIGN KEY (turn_id) REFERENCES turns(id);

ALTER TABLE providers ADD CONSTRAINT providers_credential_trial_turn_id_fkey FOREIGN KEY (credential_trial_turn_id) REFERENCES turns(id);

ALTER TABLE thread_goal_reports ADD CONSTRAINT thread_goal_reports_turn_id_fkey FOREIGN KEY (turn_id) REFERENCES turns(id) ON DELETE SET NULL;
