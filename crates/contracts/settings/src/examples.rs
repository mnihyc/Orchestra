//! Authored JSON examples for every built-in setting, checked against its actual schema.
//!
//! Examples describe a value's shape; resource identifiers are illustrative, and no example is
//! installed as a default. The registry macro requires a constant of the same name for every key,
//! so adding a key without help data cannot compile. These are data, not code-generated output.

pub const AGENT_MODEL: &str = r#""gpt-6""#;
pub const AGENT_EFFORT: &str = r#""high""#;
pub const AGENT_PARAMS: &str = "{}";
pub const AGENT_FEATURES: &str = r#"{"vision":true}"#;
pub const AGENT_INSTRUCTIONS: &str =
    r#""Run the relevant tests before describing a change as complete.""#;
pub const AGENT_MEMORY: &str = r#""on""#;
pub const AGENTS_HARNESS_VERSIONS: &str =
    r#"{"codex":{"use":"newest_checked"},"claude-code":{"use":"exact","version":"2.1.309"}}"#;
pub const AGENTS_HARNESS_CHANNEL_HOURS: &str = "12";
/// A shorter interface deadline illustrates that its default may be changed.
pub const AGENTS_HARNESS_INTERFACE_TIMEOUT_SECS: &str = "60";
/// A shorter case deadline illustrates the independent per-case setting.
pub const AGENTS_HARNESS_CASE_TIMEOUT_SECS: &str = "45";
/// A longer stage deadline illustrates that the shipped value is no ceiling.
pub const AGENTS_HARNESS_CONFORMANCE_TIMEOUT_SECS: &str = "900";
/// Mirror transport does not grant publisher authority by default.
pub const AGENTS_HARNESS_TRUST_MIRROR: &str = "false";
pub const AGENTS_HARNESS_MIRROR: &str = r#""https://mirror.example.invalid/releases""#;
pub const AGENTS_SUBAGENTS_AT_ONCE: &str = "8";
pub const AGENTS_SUBAGENT_DEPTH: &str = "1";
pub const AGENTS_SKILLS: &str = r#"{"019a0000-0000-7000-8000-000000000006":"latest","019a0000-0000-7000-8000-000000000007":"019a0000-0000-7000-8000-000000000008","019a0000-0000-7000-8000-000000000009":"off"}"#;
pub const AGENTS_PERSONAL_SKILLS: &str = "true";
pub const THREAD_CWD: &str = r#""web/services""#;
/// A bounded prefix sufficient for a reference card's short excerpt.
pub const REFERENCES_PREVIEW_BYTES: &str = "262144";
/// Two lines on either side give a short code excerpt.
pub const REFERENCES_PREVIEW_LINES: &str = "2";
/// Favor meaning while keeping place and references useful.
pub const RELATED_WEIGHTS: &str = r#"{"meaning":"0.6","place":"0.25","references":"0.15"}"#;
/// A shared folder is closer than the same topic, project or org.
pub const RELATED_PLACE_SCORES: &str =
    r#"{"folder":"1.0","topic":"0.7","project":"0.4","org":"0.1"}"#;
/// The ordinary compact inspector list.
pub const RELATED_COUNT: &str = "8";
/// Oversample the displayed list before checking each candidate's current access.
pub const RELATED_CANDIDATES: &str = "200";
/// A compressed source is bounded before a decoder sees it.
pub const FILES_PREVIEW_SOURCE_BYTES: &str = "16777216";
/// A photograph may have up to forty million pixels under this example policy.
pub const FILES_PREVIEW_PIXELS: &str = "40000000";
/// A high-density thumbnail remains small enough for a file card.
pub const FILES_PREVIEW_EDGE: &str = "512";
/// Encoded previews and inert excerpts stay bounded separately from their originals.
pub const FILES_PREVIEW_BYTES: &str = "262144";
/// The cache has one byte budget across all content hashes.
pub const FILES_PREVIEW_CACHE_BYTES: &str = "67108864";
/// A viewer reads larger originals only after its operator increases this policy.
pub const FILES_VIEWER_BYTES: &str = "67108864";
/// The example matches the ordinary whole-image upload allowance; zero uses files alone.
pub const FILES_NATIVE_IMAGE_BYTES: &str = "20971520";
/// A document's automatic description begins with the same short line as a place's.
pub const DOCS_DESCRIPTION_CHARS: &str = "200";
pub const THREAD_SIDE_THREADS: &str = "true";
pub const THREAD_SIDE_MAX: &str = "4";
pub const THREAD_SIDE_IDLE_HOURS: &str = "24";
pub const THREAD_INPUTS_WITHOUT_USE: &str = r#""held""#;
pub const THREAD_SWITCH_HISTORY: &str = r#""carry""#;
pub const THREAD_SWITCH_CARRY_SHARE: &str = "80";
/// A smaller limit illustrates that the shipped value, the size of the largest record a program keeps, may be lowered.
pub const THREAD_NATIVE_FORK_CHECK_BYTES: &str = "16777216";
/// A smaller size illustrates that a place may read less of a large turn for its summaries and goal checks.
pub const THREAD_READ_TURN_BYTES: &str = "16777216";
pub const THREAD_GOAL_MAX_TURNS: &str = "30";
pub const THREAD_GOAL_MAX_MINUTES: &str = "120";
pub const THREAD_GOAL_MAX_CREDITS: &str = r#""5.00""#;
pub const SUMMARY_HANDOFF_INPUT_SHARE: &str = "50";
pub const THREAD_PLAN_FIRST: &str = "true";
pub const THREAD_PLAN_ANSWERED_BY: &str = r#""managers""#;
pub const APPROVAL_MODE: &str = r#""on_request""#;
pub const APPROVAL_ALLOWED_MODES: &str = r#"["ask","on_request"]"#;
pub const SANDBOX_MODE: &str = r#""workspace""#;
pub const AGENT_MESSAGES: &str =
    r#"{"tool_read_shared":"Read a shared item at the requested depth."}"#;
pub const MODELS_ALLOWED: &str = r#"["gpt-6*"]"#;
pub const PROVIDERS_ALLOWED: &str = r#"["acme.*"]"#;
pub const COLLAB_ATTRIBUTION: &str = r#""always""#;
pub const COLLAB_DEFAULT_DELIVERY: &str = r#""queue""#;
pub const COMPOSER_PASTE_INLINE_CONTEXT_SHARE: &str = "25";
pub const ENV_THREAD_DEFAULT: &str = r#""shared""#;
pub const ENV_UPDATE: &str = r#""manual""#;
pub const ENV_TEMPLATE_ID: &str = r#""019a0000-0000-7000-8000-000000000001""#;
pub const ENV_HOST_ID: &str = r#""019a0000-0000-7000-8000-000000000002""#;
pub const ENV_DEFAULT_ID: &str = r#""019a0000-0000-7000-8000-000000000005""#;
pub const ENV_RUNTIME: &str = r#""kata""#;
pub const ENVIRONMENTS_RUNTIMES: &str = r#"{"*":"forbid","runc":"allow","kata":"require"}"#;
pub const ENV_STORAGE: &str = r#"{"workspace":"ssd","home":"ssd","state":"ssd"}"#;
pub const ENV_DATA_SOURCES: &str = r#"{"workspace":{"kind":"existing_directory","path":"/srv/work/project"},"home":{"kind":"made_by_orch"}}"#;
pub const ENV_AFTER_UP: &str =
    r#"{"10-tools":{"command":["tool","setup"],"timeout_secs":300},"20-inherited":null}"#;
pub const ENV_MOUNTS: &str = r#"{"019a0000-0000-7000-8000-000000000003":"read_only"}"#;
pub const ENV_RESOURCES: &str =
    r#"{"cpu_millis":2000,"memory_mb":4096,"memory_swap_mb":4096,"pids":256,"blkio_weight":500}"#;
pub const ENV_NEEDS: &str = r#"{"cpu_millis":1000,"memory_mb":2048,"disk_gb":10}"#;
pub const ENV_IDLE_STOP_MINUTES: &str = "1440";
pub const ENV_VARIABLES: &str = r#"{"LANG":"C.UTF-8"}"#;
pub const ENV_SECRET_NAMES: &str = r#"["PACKAGE_TOKEN"]"#;
pub const NETWORK_ALLOWED_HOSTS: &str = r#"["pypi.org","registry.npmjs.org"]"#;
pub const MCP_ENABLED: &str = r#"{"019a0000-0000-7000-8000-000000000004":true}"#;
pub const EXEC_OFFERS: &str = r#"["orch"]"#;
pub const EXEC_ORCH_TOOLS: &str = "true";
pub const ORCH_TOOLS_ENABLED: &str = r#"["list_shared","read_shared","search_shared"]"#;
pub const ORCH_TOOLS_CALLS_PER_MINUTE: &str = "120";
pub const SHARING_AGENT_REACH: &str = r#""none""#;
pub const SHARING_AGENT_DOCUMENTS: &str = r#""write""#;
pub const SHARING_SNAPSHOT_MAX_BYTES: &str = "67108864";
pub const EXPORT_MAX_BYTES: &str = "2147483648";
pub const EXPORT_MAX_ITEMS: &str = "10000";
pub const EXPORT_KEEP_HOURS: &str = "24";
pub const IMPORT_MAX_BYTES: &str = "1073741824";
pub const IMPORT_MAX_ITEMS: &str = "5000";
pub const WORKSPACE_IGNORE: &str = r#"["node_modules/","target/"]"#;
pub const WORKSPACE_INDEX: &str = r#""live""#;
pub const SOURCE_REPOSITORY_URL: &str = r#""https://example.invalid/team/repository.git""#;
pub const SOURCE_BRANCH: &str = r#""release""#;
pub const SOURCE_SECRET_NAME: &str = r#""REPOSITORY_TOKEN""#;
pub const WORKSPACE_RECORD_DAYS: &str = "7";
pub const WORKSPACE_RECORD_MAX_BYTES: &str = "1073741824";
pub const BROADCAST_DELIVERY: &str = r#""steer""#;
pub const BROADCAST_FALLBACK: &str = r#""next_turn""#;
pub const BROADCAST_WAKE_IDLE: &str = "false";
pub const BROADCAST_AGENT_OPTIONS: &str = r#"["steer","next_turn"]"#;
pub const TERMINAL_ENABLED: &str = "true";
pub const TERMINAL_RECORD: &str = "true";
pub const TERMINAL_DETACHED_MINUTES: &str = "480";
pub const TERMINAL_MAX_PER_PERSON: &str = "4";
pub const TERMINAL_MAX_PER_ENVIRONMENT: &str = "16";
pub const BROWSER_ENABLED: &str = "true";
pub const PORTS_ENABLED: &str = "true";
pub const PORTS_MAX_OPEN: &str = "16";
pub const PORTS_LINK_MAX_SECS: &str = "86400";
pub const REQUESTS_EXPIRY_HOURS: &str = "24";
pub const BROWSER_PRESET: &str = r#""headless""#;
pub const BROWSER_ARGS: &str = r#"["--lang=en-US"]"#;
pub const BROWSER_VIEWPORT: &str = r#""1280x800""#;
pub const BROWSER_IDLE_MINUTES: &str = "30";
pub const BROWSER_MAX_PER_ENVIRONMENT: &str = "4";
pub const VNC_ENABLED: &str = "true";
pub const VNC_GEOMETRY: &str = r#""1280x800""#;
pub const VNC_MAX_PER_ENVIRONMENT: &str = "4";
pub const ENV_IDLE_WARNING_MINUTES: &str = "60";
pub const ENV_REMOVED_KEEP_DAYS: &str = "7";
pub const ENV_UNUSED_REMOVE_DAYS: &str = "0";
pub const ENV_LIFETIME: &str = r#""temporary""#;
pub const ENV_TEMPORARY_IDLE_HOURS: &str = "24";
pub const ENV_RECYCLE_IDLE_MINUTES: &str = "60";
pub const HOSTS_FULL_PERCENT: &str = "90";
pub const HOSTS_WHEN_FULL: &str = r#""warn""#;
pub const RUNTIME_IDLE_STOP_MINUTES: &str = "1440";
pub const RUNTIME_TEMPLATE_ID: &str = r#""019a0000-0000-7000-8000-000000000005""#;
pub const RUNTIME_HOST_ID: &str = r#""019a0000-0000-7000-8000-000000000002""#;
pub const RUNTIME_RESOURCES: &str = r#"{"cpu_millis":1000,"memory_mb":2048,"memory_swap_mb":2048,"pids":128,"blkio_weight":500,"disk_gb":10}"#;
pub const RUNTIME_STORAGE: &str = r#"{"state":"ssd"}"#;
pub const RUNTIME_HOME: &str = r#"{"kind":"existing_directory","path":"/srv/harness/home"}"#;
pub const RUNTIME_NEEDS: &str = r#"{"cpu_millis":500,"memory_mb":512,"disk_gb":1}"#;
pub const RUNTIME_AFTER_UP: &str =
    r#"{"10-helper":{"command":["/usr/local/bin/provider-helper","install"],"timeout_secs":300}}"#;
pub const RUNTIME_VARIABLES: &str = r#"{"LANG":"C.UTF-8"}"#;
pub const RUNTIME_SECRET_NAMES: &str = r#"["PROVIDER_TOKEN"]"#;
pub const RUNTIME_UPDATE: &str = r#""manual""#;
pub const RUNTIME_RENEW_DAYS: &str = "5";
pub const PROVIDERS_PRESETS: &str = r#"{"gateway":{"upstreams":[{"protocol":"openai-responses","base_url":"https://gateway.example.invalid/v1"}]}}"#;
pub const PROVIDERS_ALLOW_HTTP_UPSTREAMS: &str = "false";
pub const PROVIDERS_EXPIRY_NOTICE_DAYS: &str = "14";
pub const PROVIDERS_SUBSCRIPTION_HARNESSES: &str = r#"["codex","claude-code"]"#;
pub const PROVIDERS_SUBSCRIPTION_SHARING: &str = r#"["codex"]"#;
pub const RESOURCES_CREATED_BY_MEMBERS: &str =
    r#"["provider","template","mcp_server","mount","skill"]"#;
pub const SKILLS_MAX_BYTES: &str = "33554432";
pub const SKILLS_MAX_FILES: &str = "10000";
pub const BROADCAST_RATE_PER_HOUR: &str = "6";
/// A place that wants every reviewed operation to begin with Notify ticked.
/// Turned on, the followers of a thread hear when one of its sub-agents ends.
pub const NOTIFY_SUBAGENTS: &str = "true";
pub const NOTIFY_OPERATIONS: &str = r#""always""#;
pub const BROADCAST_CHAT_MESSAGES_PER_HOUR: &str = "60";
pub const BROADCAST_AGENT_SCOPE: &str = r#""topic""#;
pub const CAPABILITY_RECREATE_GRACE_SECS: &str = "120";
pub const ACCESS_REQUEST_EXPIRY_DAYS: &str = "14";
pub const ACCESS_MAX_PENDING_REQUESTS: &str = "10";
pub const ACCESS_QUICK_GRANT_DURATIONS: &str = r#"["1d","7d","30d","always"]"#;
pub const NODES_NEW_COLOR: &str = r#""random""#;
pub const TAGS_MAX_PER_PLACE: &str = "5";
pub const AGENT_DEFAULT_PROVIDER_ID: &str = r#""019a0000-0000-7000-8000-000000000006""#;
pub const AGENT_DEFAULT_AGENT_ID: &str = r#""019a0000-0000-7000-8000-000000000007""#;
pub const TEMPLATES_RESTRICTED_DRIVERS: &str = r#"["host"]"#;
pub const SWITCH_BATCH_PARALLEL: &str = "4";
/// A larger reviewed batch can plan five hundred threads.
pub const SWITCH_BATCH_THREADS: &str = "500";
/// A closed plan leaves its count running for five minutes.
pub const MEASUREMENT_KEEPALIVE_SECS: &str = "300";
/// A count can be reused for the normal fifteen-minute plan lifetime.
pub const MEASUREMENT_VALID_SECS: &str = "900";
/// Background scans may continue for a day unless cancelled sooner.
pub const MEASUREMENT_TIMEOUT_SECS: &str = "86400";
/// Unchanged activity is labelled after half a minute.
pub const JOB_STALL_SECS: &str = "30";
/// An unconfirmed stop names its machine after half a minute.
pub const JOB_STOP_WAIT_SECS: &str = "30";
/// The usual file-work allowance is six hours.
pub const JOB_IO_TIMEOUT_SECS: &str = "21600";
pub const SWITCH_ACTIVE_TURN: &str = r#""wait""#;
pub const SWITCH_WAIT_MINUTES: &str = "10";
pub const SWITCH_VERIFY_TIMEOUT_SECS: &str = "120";
pub const SWITCH_KEEP_PREVIOUS: &str = "false";
pub const BINDING_PREFER: &str = r#"["separated","key_separated"]"#;
pub const BINDING_ALLOWED: &str = r#"["separated","key_separated"]"#;
pub const CREDENTIAL_REFRESH_MARGIN_SECS: &str = "300";
pub const AUTO_TITLE: &str = r#""harness""#;
pub const AUTO_DESCRIPTION: &str = r#""off""#;
pub const AUTO_TAGS: &str = r#""off""#;
pub const AUTO_TOPIC_ROLLUP: &str = r#""rollup""#;
pub const AUTO_SUMMARIZER: &str = r#"{"kind":"model"}"#;
pub const CALLS_RULES: &str = orch_provider_contract::calls::DEFAULT_RULES;
pub const CALLS_REPEAT_LIMIT: &str = "3";
pub const CALLS_REPEAT_WINDOW: &str = "300";
pub const CALLS_NOTIFY_ON_STOP: &str = "false";
pub const CALLS_RETURN: &str = "\"next_turn\"";
pub const AUTO_TRIGGERS: &str = r#"{"on":["turn_completed","milestone"],"min_interval_secs":120}"#;
pub const CAPABILITY_POLICY: &str = r#"{"kvm":{"decision":"ask","approver_scope":"host"},"port_place":{"decision":"allow","approver_scope":"environment"}}"#;
pub const SHARING_ALLOW_EXTERNAL: &str = "true";
pub const SHARING_ALLOW_HOLDERS: &str = "true";
pub const SHARING_EXTERNAL_MAX_DAYS: &str = "30";
pub const USAGE_RATES: &str =
    r#"{"gpt-6*":{"input_per_mtok":"1","cached_input_per_mtok":"0.1","output_per_mtok":"4"}}"#;
pub const USAGE_CATALOGUE: &str = "true";
pub const USAGE_CATALOGUE_SCALE: &str = r#""1""#;
pub const USAGE_CATALOGUE_URL: &str = r#""https://example.invalid/rates.json""#;
pub const USAGE_CATALOGUE_REFRESH_DAYS: &str = "0";
pub const CHAT_ENABLED: &str = "true";
pub const CHAT_DIRECT: &str = r#""members""#;
pub const CHAT_GROUP_MAX_MEMBERS: &str = "10";
pub const CHAT_GROUP_MENTIONS: &str = r#""writers""#;
/// A configurable rate that allows ordinary bursts of explicit notices.
pub const NOTIFICATIONS_PERSON_RATE_PER_HOUR: &str = "120";
/// A short note fits the compact person card.
pub const NOTIFICATIONS_PERSON_NOTE_CHARS: &str = "200";
pub const REGISTRATION_MODE: &str = r#""invite""#;
/// Read-only release checks ship disabled.
pub const RELEASE_CHECK_DAYS: &str = "0";
/// Keep five unused releases per payload in addition to the current and previous releases.
pub const CLEANUP_RELEASE_KEEP_COUNT: &str = "5";
/// A host that needs a longer cache window can retain unused tools for two weeks.
pub const CLEANUP_UNUSED_KEEP_DAYS: &str = "14";
/// Tolerate one delayed daily backup, then show the missing second night.
pub const BACKUP_WARN_DAYS: &str = "2";
pub const DOCS_ENABLED: &str = "true";
pub const DOCS_TEMPLATES: &str = r#"["019a0000-0000-7000-8000-000000000001"]"#;
pub const DOCS_MAX_BYTES: &str = "2097152";
pub const DOCS_MAX_BLOCKS: &str = "5000";
pub const DOCS_VERSIONS_KEPT: &str = "200";
pub const DOCS_HISTORY_MAX_BYTES: &str = "16777216";
pub const DOCS_FILE_MAX_BYTES: &str = "8589934592";
pub const DOCS_FILES_MAX_BYTES: &str = "1073741824";
/// A reading-only place can leave the graph and the meaning list out.
pub const DOCUMENTS_LINKS_SHOWN: &str = r#"["referenced_by","references"]"#;
/// A longer first page for readers who scan many sources.
pub const DOCUMENTS_LINKS_PAGE_SIZE: &str = "40";
pub const DOCUMENTS_LINKS_OCCURRENCES: &str = "10";
pub const DOCUMENTS_LINKS_LINE_CHARS: &str = "200";
pub const DOCUMENTS_LINKS_MENTIONS_COUNTED: &str = "500";
pub const DOCUMENTS_LINKS_ACCESS_CHECKS: &str = "2000";
pub const DOCUMENTS_LINKS_GRAPH_NODES: &str = "300";
pub const CHAT_ATTACHMENT_MAX_BYTES: &str = "8589934592";
pub const SEARCH_WORD_RECENT_CANDIDATES: &str = "200";
pub const SEARCH_WORD_CANDIDATES: &str = "50";
pub const SEARCH_WORD_PROBE_CANDIDATES: &str = "20000";
pub const SEARCH_PAGE_SIZE: &str = "20";
pub const SEARCH_REORDER_WINDOW_SECS: &str = "5";
pub const SEARCH_MEANING_PAUSE_MS: &str = "800";
pub const SEARCH_CURSOR_ENTRIES: &str = "100";
pub const SEARCH_CURSOR_TTL_SECS: &str = "900";
pub const SEARCH_VECTOR_CACHE_ENTRIES: &str = "1000";
pub const SEARCH_VECTOR_CACHE_SECS: &str = "300";
pub const SEARCH_SEMANTIC_ENABLED: &str = "true";
pub const SEARCH_SEMANTIC_EMBEDDINGS: &str = r#"{"provider_id":"019a0000-0000-7000-8000-00000000000a","model_name":"embeddinggemma-2","dimensions":256,"query_prefix":"task: search result | query: ","document_prefix":"title: {title} | text: "}"#;
pub const SEARCH_SEMANTIC_MAX_DISK_SHARE: &str = "60";
pub const SEARCH_SEMANTIC_TUNING: &str = r#"{"chunk_tokens":400,"batch_tokens":2048,"candidates":50,"oversample":4,"fusion_k":60,"same_model_cosine_milli":990,"interval_secs":10,"query_wait_secs":3,"source_batch":64,"exact_filter_candidates":20000,"max_scan_tuples":20000,"scan_mem_multiplier":2}"#;
pub const SEARCH_WORD_TUNING: &str = r#"{"scope_candidates":30000,"request_windows":1,"tiers":{"title":50,"description":50,"heading":50,"body":50}}"#;
pub const RETENTION_ARCHIVE_IDLE_DAYS: &str = "0";
pub const RETENTION_DELETE_ARCHIVED_DAYS: &str = "0";
/// A week of idle time keeps resumable caches small while retaining the conversation.
pub const RETENTION_AGENT_HISTORY_IDLE_DAYS: &str = "7";
pub const RETENTION_INBOX_DAYS: &str = "30";
pub const REDACTION_MASK_SECRETS: &str = "true";
pub const REDACTION_PATTERNS: &str = r#"[{"name":"Internal reference","regex":"INTERNAL-[0-9]+"}]"#;
pub const DIAGNOSTICS_RETENTION_DAYS: &str = "14";
pub const ORGS_PERSONAL_ENABLED: &str = "false";
pub const ORGS_PERSONAL_CLOSED: &str = "true";
pub const ORGS_PERSONAL_FLAT: &str = "true";
pub const ORGS_CREATED_BY: &str = r#""invited""#;
pub const NAVIGATION_ORDER: &str = r#"{"by":"changed"}"#;
pub const INVITATIONS_FORMS: &str = r#"["link","email"]"#;
pub const INVITATIONS_DEFAULT_HOURS: &str = "168";
pub const INVITATIONS_MAX_HOURS: &str = "720";
pub const INVITATIONS_MAX_USES: &str = "100";
pub const MAIL_SHARED: &str = "true";
pub const REGISTRATION_EMAIL_VERIFICATION: &str = r#""required""#;
pub const AUTH_LOCAL_LOGIN: &str = "true";
pub const AUTH_EMAIL_CODE: &str = r#"["second_step"]"#;
pub const AUTH_SECOND_STEP: &str = r#""required""#;
pub const TOKENS_MAX_DAYS: &str = "90";
pub const SYSTEM_BANNER: &str =
    r#"{"text":"Maintenance starts at 18:00 UTC.","level":"info","until":"2026-12-01T18:00:00Z"}"#;
pub const OUTBOUND_ALLOWED_PRIVATE_HOSTS: &str = r#"["mail.internal.example"]"#;
pub const STORAGE_MAX_BLOB_BYTES: &str = "8589934592";
pub const STORAGE_ENV_CACHE_QUOTA_BYTES: &str = "21474836480";
