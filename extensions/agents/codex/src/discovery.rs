//! Sessions found in a Codex home; the MCP servers of its own configuration, the home's and a directory's, by name, and one definition read out for an import.
//!
//! **Sessions.** The threads a harness already has are listed by the harness itself (`thread/list`, across
//! every model provider it recorded them under, and without the sub-agents' threads), newest first, so that a
//! person can adopt one as a thread of Orch. When one is adopted, what it recorded is read without loading it
//! ([`read_session`]): the model it was configured with, and its turns, each with what the person said and the
//! items the mapper makes of the rest, under the names a resync gives them.
//!
//! **MCP servers.** The `mcp_servers` tables of the harness's own configuration are read: the home's
//! `config.toml` and a `.codex/config.toml` of a directory of the workspace. A server is *found* by name, with
//! how it is reached and a digest that changes when its definition does, and never with its command, its
//! address or a secret. When one is imported its definition is read out, for the connector to keep on this
//! machine and nowhere else: a program with its arguments and variables, or an address; the variables whose
//! names say they hold a secret are kept apart as secrets. A server reached by address with headers of its
//! own keeps those headers and environment references in that same private copy.
//!
//! A session is held to the servers it is given, and the harness would otherwise load those of the
//! configuration it reads ([`servers_to_switch_off`]). The harness is asked what it loads for the session's
//! directory, which covers the home's configuration and every other source it reads; the configuration of the
//! project's own directories is read as well, since the harness decides on its own, when the session starts
//! and by rules of its own, whether a project is trusted and which directory is its root.

use std::collections::BTreeMap;
use std::path::Path;

use orch_agent_contract::adapter::AdapterError;
use orch_agent_contract::discovery::{
    DEFAULT_STARTUP_TIMEOUT_SECS, McpDefinition, McpFound, McpHttpCredentials, McpImportRequest,
    McpRequires, McpScope, McpServerSpec, McpTransport, McpWorkingDir, NativeSession,
    RECORD_FORMAT, RECORD_MAX_BYTES, RecordedEntry, SessionRecord, record_lines, record_report,
};
use orch_agent_contract::history::{Fidelity, ReportBuilder, SessionArchive};
use orch_plugin_contract::Secret;
use orch_types::{Hash, Json, Time};
use serde_json::json;

use crate::descriptor::KIND;
use crate::process::CONFIG_FILE;
use crate::protocol::{Rpc, RpcError};

/// The directory of a workspace that holds the harness's own configuration for it.
pub const PROJECT_CONFIG_DIRECTORY: &str = ".codex";

/// How many threads are asked for at a time.
pub const SESSIONS_PAGE: u32 = 100;

/// The words in the name of a variable that say it holds a secret.
const SECRET_WORDS: [&str; 7] = [
    "KEY",
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "CREDENTIAL",
    "COOKIE",
    "AUTH",
];

fn remote_text(error: &RpcError) -> String {
    match error {
        RpcError::Remote { message, .. } => message.clone(),
        other => other.to_string(),
    }
}

/// The sessions the harness already has, newest first.
pub async fn discover_sessions(rpc: &Rpc) -> Result<Vec<NativeSession>, AdapterError> {
    let mut found = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = rpc
            .call(
                "thread/list",
                json!({
                    "cursor": cursor, "limit": SESSIONS_PAGE, "sortKey": "updated_at",
                    "modelProviders": [], "archived": false,
                    "sourceKinds": ["cli", "vscode", "exec", "appServer", "unknown"],
                }),
            )
            .await
            .map_err(|error| AdapterError::HarnessError(remote_text(&error)))?;
        if let Some(threads) = page.get("data").and_then(Json::as_array) {
            found.extend(
                threads
                    .iter()
                    .filter(|thread| thread.get("parentThreadId").is_none_or(Json::is_null))
                    .filter_map(session_of),
            );
        }
        cursor = page
            .get("nextCursor")
            .and_then(Json::as_str)
            .map(str::to_owned);
        if cursor.is_none() {
            return Ok(found);
        }
    }
}

/// A thread of the harness as a session that can be adopted.
#[must_use]
pub fn session_of(thread: &Json) -> Option<NativeSession> {
    let id = thread.get("id").and_then(Json::as_str)?;
    let title = thread
        .get("name")
        .and_then(Json::as_str)
        .filter(|name| !name.is_empty())
        .or_else(|| {
            thread
                .get("preview")
                .and_then(Json::as_str)
                .filter(|preview| !preview.is_empty())
        })
        .map(str::to_owned);
    Some(NativeSession {
        native_session_ref: id.to_owned(),
        title,
        cwd: thread.get("cwd").and_then(Json::as_str).map(str::to_owned),
        updated_at: thread
            .get("updatedAt")
            .and_then(Json::as_i64)
            .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
            .map(Time::new),
        adopted_thread_id: None,
    })
}

// ---------------------------------------------------------------------------------------------------------
// What a session recorded, read for its adoption

/// The turns of a thread, oldest first, as the harness lists them page by page; `view` says how much of each
/// turn's items comes with it (`full`, or `notLoaded` to count them).
async fn turns_of(rpc: &Rpc, native: &str, view: &str) -> Result<Vec<Json>, AdapterError> {
    let mut turns = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = rpc
            .call(
                "thread/turns/list",
                json!({ "threadId": native, "sortDirection": "asc", "limit": SESSIONS_PAGE,
                        "cursor": cursor, "itemsView": view }),
            )
            .await
            .map_err(|error| AdapterError::HarnessError(remote_text(&error)))?;
        turns.extend(
            page.get("data")
                .and_then(Json::as_array)
                .cloned()
                .unwrap_or_default(),
        );
        cursor = page
            .get("nextCursor")
            .and_then(Json::as_str)
            .map(str::to_owned);
        if cursor.is_none() {
            return Ok(turns);
        }
    }
}

/// One checkpoint detects cursor cycles without retaining every cursor of a large history.
struct CursorProgress {
    checkpoint: Option<String>,
    distance: u64,
    span: u64,
}

impl CursorProgress {
    fn new() -> Self {
        Self {
            checkpoint: None,
            distance: 0,
            span: 1,
        }
    }

    fn observe(&mut self, cursor: &str) -> Result<(), AdapterError> {
        if self.checkpoint.as_deref() == Some(cursor) {
            return Err(AdapterError::HarnessError(
                "the native turn-list cursor repeats".into(),
            ));
        }
        if self.checkpoint.is_none() {
            self.checkpoint = Some(cursor.to_owned());
            return Ok(());
        }
        self.distance += 1;
        // Brent's cycle check doubles the distance between checkpoints. A cycle meets a checkpoint;
        // a progressing history needs only its current cursor and this one retained checkpoint.
        if self.distance == self.span {
            self.checkpoint = Some(cursor.to_owned());
            self.distance = 0;
            self.span = self.span.checked_mul(2).ok_or_else(|| {
                AdapterError::HarnessError("the native turn-list cursor does not finish".into())
            })?;
        }
        Ok(())
    }
}

/// Counts completed metadata one page at a time; an inspection never retains the thread's turn list.
async fn ended_turns(rpc: &Rpc, native: &str) -> Result<i64, AdapterError> {
    let mut count = 0_i64;
    let mut cursor: Option<String> = None;
    let mut progress = CursorProgress::new();
    loop {
        let page = rpc
            .call(
                "thread/turns/list",
                json!({
                    "threadId":native, "sortDirection":"asc", "limit":SESSIONS_PAGE,
                    "cursor":cursor, "itemsView":"notLoaded"
                }),
            )
            .await
            .map_err(|error| AdapterError::HarnessError(remote_text(&error)))?;
        let ended = page
            .get("data")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter(|turn| turn.get("status").and_then(Json::as_str) != Some("inProgress"))
            .count();
        count = count.saturating_add(i64::try_from(ended).unwrap_or(i64::MAX));
        cursor = page
            .get("nextCursor")
            .and_then(Json::as_str)
            .map(str::to_owned);
        match &cursor {
            Some(cursor) => progress.observe(cursor)?,
            None => return Ok(count),
        }
    }
}

/// A member of the harness's thread that names something, where it does.
fn named(thread: &Json, member: &str) -> Option<String> {
    thread
        .get(member)
        .and_then(Json::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// What a record of a thread holds, from what the harness answered: the thread as `thread/read` gives it, and
/// its turns with their items. Answers the model and the effort the thread ran with, and its turns as entries.
#[must_use]
pub fn record_of(
    thread: &Json,
    turns: &[Json],
    ctx: &crate::events::Context<'_>,
    turn_id: impl Fn(&str) -> orch_types::Id,
) -> (Option<String>, Option<String>, Vec<Vec<RecordedEntry>>) {
    let config = record_config(thread);
    let entries = crate::events::record(turns, config, ctx, turn_id);
    (
        named(thread, "model"),
        named(thread, "reasoningEffort"),
        entries,
    )
}

fn record_config(thread: &Json) -> crate::events::MapperConfig {
    let native = named(thread, "id").unwrap_or_default();
    crate::events::MapperConfig {
        // No thread of Orch holds the session yet: the entries name nothing of Orch's.
        thread_id: orch_types::Id::nil(),
        segment_id: orch_types::Id::nil(),
        native_thread: native,
        directory: named(thread, "cwd").unwrap_or_default(),
    }
}

/// Reads what the harness recorded of a thread it holds, for a person who adopts it: what it ran with, from
/// the thread's own record (`thread/read`, without its turns), and its turns (`thread/turns/list`, whole, the
/// way the harness asks a long history to be read), written as a record in `directory` when `write` is asked.
/// The thread is neither loaded nor changed.
pub async fn read_session(
    rpc: &Rpc,
    native: &str,
    write: bool,
    directory: &Path,
    ctx: &crate::events::Context<'_>,
    turn_id: impl Fn(&str) -> orch_types::Id,
) -> Result<SessionRecord, AdapterError> {
    read_session_with_images(rpc, native, write, directory, ctx, turn_id, None).await
}

pub(crate) async fn read_session_with_images(
    rpc: &Rpc,
    native: &str,
    write: bool,
    directory: &Path,
    ctx: &crate::events::Context<'_>,
    turn_id: impl Fn(&str) -> orch_types::Id,
    images: Option<crate::viewed_images::Sources>,
) -> Result<SessionRecord, AdapterError> {
    let read = rpc
        .call(
            "thread/read",
            json!({ "threadId": native, "includeTurns": false }),
        )
        .await
        .map_err(|error| {
            let text = remote_text(&error);
            if text.to_ascii_lowercase().contains("not found") {
                AdapterError::NotFound(text)
            } else {
                AdapterError::HarnessError(text)
            }
        })?;
    let thread = read.get("thread").cloned().unwrap_or(Json::Null);
    if thread.get("id").and_then(Json::as_str).is_none() {
        return Err(AdapterError::NotFound(format!(
            "the harness holds no thread {native}"
        )));
    }
    if !write {
        let ended = ended_turns(rpc, native).await?;
        return Ok(SessionRecord {
            model: named(&thread, "model"),
            effort: named(&thread, "reasoningEffort"),
            turns: ended,
            entries: 0,
            file: None,
            report: ReportBuilder::new().finish(Fidelity::Structured),
        });
    }
    let turns = turns_of(rpc, native, "full").await?;
    let (model, effort, entries) = if let Some(mut images) = images {
        images.observe(&thread);
        let mut recorder =
            crate::events::Recorder::new(record_config(&thread), true).with_native_web_results();
        for page in turns.chunks(
            usize::try_from(SESSIONS_PAGE)
                .map_err(|error| AdapterError::Internal(error.to_string()))?,
        ) {
            let keys = crate::viewed_images::page_keys(native, page);
            let web_keys = crate::viewed_images::page_web_keys(native, page);
            let reported = crate::native_web::page_reported(native, page);
            if !keys.is_empty() || !web_keys.is_empty() {
                let mut cancelled =
                    crate::native_history::CancelOnDrop(Some(images.cancellation()));
                let outputs = ctx.outputs.clone();
                let prepared = tokio::task::spawn_blocking(move || {
                    let values = images.prepare(&keys, &outputs);
                    let web = images.prepare_web_inputs(&web_keys, &reported, &outputs);
                    (images, values, web)
                })
                .await
                .map_err(|error| {
                    AdapterError::Internal(format!("the native result reader ended: {error}"))
                })?;
                cancelled.disarm();
                images = prepared.0;
                recorder.install_view_results(prepared.1);
                recorder.install_web_results(prepared.2);
            }
            recorder.page(page, ctx, &turn_id);
        }
        (
            named(&thread, "model"),
            named(&thread, "reasoningEffort"),
            recorder.finish(),
        )
    } else {
        record_of(&thread, &turns, ctx, turn_id)
    };
    let (text, kept, count, left_out) =
        record_lines(&entries, RECORD_MAX_BYTES).map_err(|error| {
            AdapterError::Internal(format!("the record could not be written: {error}"))
        })?;
    let skipped = usize::try_from(left_out).unwrap_or(0);
    let report = record_report(&entries[skipped.min(entries.len())..], left_out);
    let file = if text.is_empty() {
        None
    } else {
        let path = directory.join(format!("record-{native}.jsonl"));
        crate::process::write_private(&path, text.as_bytes()).map_err(|error| {
            AdapterError::HarnessError(format!("the record could not be written: {error}"))
        })?;
        Some(SessionArchive {
            format: RECORD_FORMAT.to_owned(),
            path: path.to_string_lossy().into_owned(),
            size: i64::try_from(text.len()).unwrap_or(i64::MAX),
        })
    };
    Ok(SessionRecord {
        model,
        effort,
        turns: kept,
        entries: count,
        file,
        report,
    })
}

// ---------------------------------------------------------------------------------------------------------
// MCP servers

/// The servers a configuration names: each by name with its table.
#[must_use]
pub fn servers_in(config_text: &str) -> Vec<(String, toml::Table)> {
    let Ok(config) = toml::from_str::<toml::Table>(config_text) else {
        return Vec::new();
    };
    config
        .get("mcp_servers")
        .and_then(toml::Value::as_table)
        .map(|servers| {
            servers
                .iter()
                .filter_map(|(name, table)| Some((name.clone(), table.as_table()?.clone())))
                .collect()
        })
        .unwrap_or_default()
}

/// How a server is reached, or `None` for an entry that names neither a program nor an address.
#[must_use]
pub fn transport_of(table: &toml::Table) -> Option<McpTransport> {
    if table.get("url").and_then(toml::Value::as_str).is_some() {
        Some(McpTransport::Http)
    } else if table.get("command").and_then(toml::Value::as_str).is_some() {
        Some(McpTransport::Stdio)
    } else {
        None
    }
}

/// The same normalized definition digest kept by import and the local copy.
#[must_use]
pub fn digest_of(table: &toml::Table) -> String {
    match definition(table) {
        Ok(definition) => definition.digest().to_string(),
        Err(_) => Hash::of(
            serde_json::to_value(table)
                .unwrap_or(Json::Null)
                .to_string()
                .as_bytes(),
        )
        .to_string(),
    }
}

/// The servers of one configuration file, by name; `directory` is the directory it belongs to.
#[must_use]
pub fn found_in(config_text: &str, scope: McpScope, directory: Option<&str>) -> Vec<McpFound> {
    servers_in(config_text)
        .into_iter()
        .filter_map(|(name, table)| {
            let parsed = definition(&table).ok();
            Some(McpFound {
                definition_fingerprint: parsed.as_ref().map(McpDefinition::definition_fingerprint),
                name,
                source: KIND.to_owned(),
                scope,
                directory: directory.map(str::to_owned),
                transport: transport_of(&table)?,
                digest: parsed
                    .as_ref()
                    .map(|definition| definition.digest().to_string())
                    .unwrap_or_else(|| digest_of(&table)),
            })
        })
        .collect()
}

fn project_config(directory: &str) -> std::path::PathBuf {
    Path::new(directory)
        .join(PROJECT_CONFIG_DIRECTORY)
        .join(CONFIG_FILE)
}

/// The servers the harness's own configuration names: the home's, and those of each directory given.
#[must_use]
pub fn found(home: Option<&Path>, directories: &[String]) -> Vec<McpFound> {
    let mut found = Vec::new();
    if let Some(text) = home.and_then(|home| std::fs::read_to_string(home.join(CONFIG_FILE)).ok()) {
        found.extend(found_in(&text, McpScope::User, None));
    }
    for directory in directories {
        if let Ok(text) = std::fs::read_to_string(project_config(directory)) {
            found.extend(found_in(&text, McpScope::Directory, Some(directory)));
        }
    }
    found
}

/// The entry that switches a server off. It says `enabled = false` and repeats how the server is reached, so
/// that it stands on its own: the configuration of the session that the entry is merged into may lack the
/// server (a project's configuration is read or not by the harness's own rules), and an entry with no program
/// and no address is refused by the harness as an invalid transport. Where the session's configuration has the
/// server, the repeated program or address changes nothing.
fn switch_off_entry(command: Option<&str>, url: Option<&str>) -> Json {
    let mut entry = serde_json::Map::new();
    if let Some(command) = command {
        entry.insert("command".to_owned(), json!(command));
    }
    if let Some(url) = url {
        entry.insert("url".to_owned(), json!(url));
    }
    entry.insert("enabled".to_owned(), Json::Bool(false));
    Json::Object(entry)
}

/// The entries that switch off the servers of the configuration a `config/read` answered with, by name.
#[must_use]
pub fn switch_off_entries(read: &Json) -> BTreeMap<String, Json> {
    read.get("config")
        .and_then(|config| config.get("mcp_servers"))
        .and_then(Json::as_object)
        .map(|servers| {
            servers
                .iter()
                .map(|(name, server)| {
                    let way = |key: &str| server.get(key).and_then(Json::as_str);
                    (name.clone(), switch_off_entry(way("command"), way("url")))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The directories whose own configuration the harness may read for a session that works in `cwd` of the
/// workspace `directory`: those from `cwd` up to `directory`, as the harness finds a project's root (the
/// session's configuration says the workspace is the root), and `directory` itself when `cwd` is elsewhere.
#[must_use]
pub fn project_chain(directory: &str, cwd: &str) -> Vec<std::path::PathBuf> {
    let root = Path::new(directory);
    let start = Path::new(cwd);
    if !start.starts_with(root) {
        return vec![start.to_path_buf(), root.to_path_buf()];
    }
    start
        .ancestors()
        .take_while(|at| at.starts_with(root))
        .map(Path::to_path_buf)
        .collect()
}

/// The entries that switch off the servers the project configurations of a session's directories name.
fn project_entries(directory: &str, cwd: &str) -> BTreeMap<String, Json> {
    let mut entries = BTreeMap::new();
    for at in project_chain(directory, cwd) {
        let path = at.join(PROJECT_CONFIG_DIRECTORY).join(CONFIG_FILE);
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        for (name, table) in servers_in(&text) {
            let way = |key: &str| table.get(key).and_then(toml::Value::as_str);
            // An entry that names neither a program nor an address is no server the harness could load.
            if way("command").is_some() || way("url").is_some() {
                entries
                    .entry(name)
                    .or_insert_with(|| switch_off_entry(way("command"), way("url")));
            }
        }
    }
    entries
}

/// The servers a session must switch off, each as the entry that does so: those the harness says it loads
/// (`config/read`, once for each directory of the session, or once for no directory in particular when the
/// session's tools run elsewhere and `place` is `None`) and those the project configurations of the
/// session's directories name, whether or not the harness will read them. Workspace configuration trust
/// admits its other settings; it does not import or enable its tool servers.
pub async fn servers_to_switch_off(
    rpc: &Rpc,
    place: Option<(&str, &str)>,
) -> Result<BTreeMap<String, Json>, AdapterError> {
    session_configuration(rpc, place)
        .await
        .map(|(servers, _, _)| servers)
}

/// Native instruction discovery choices, from the same effective cwd configuration as the session.
pub(crate) struct InstructionConfig {
    pub filenames: Vec<String>,
    pub max_bytes: u64,
    project_filenames: bool,
    project_budget: bool,
}

impl InstructionConfig {
    fn from_read(read: &Json) -> Self {
        let project_may_override = |key: &str| {
            !matches!(
                read["origins"][key]["name"]["type"].as_str(),
                Some(
                    "sessionFlags"
                        | "legacyManagedConfigTomlFromFile"
                        | "legacyManagedConfigTomlFromMdm"
                )
            )
        };
        Self {
            filenames: read["config"]["project_doc_fallback_filenames"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Json::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect(),
            max_bytes: read["config"]["project_doc_max_bytes"]
                .as_u64()
                .unwrap_or(crate::sessions::PROJECT_DOC_BUDGET),
            project_filenames: project_may_override("project_doc_fallback_filenames"),
            project_budget: project_may_override("project_doc_max_bytes"),
        }
    }

    /// The process-wide config reader cannot see the trust overrides of thread/start.
    /// Apply only the accepted workspace's instruction choices, in native ancestor order.
    pub(crate) fn with_workspace(
        mut self,
        root: &str,
        cwd: &str,
        answer: Option<orch_agent_contract::session::ConfigTrust>,
    ) -> Result<Self, AdapterError> {
        if answer != Some(orch_agent_contract::session::ConfigTrust::Trust) {
            return Ok(self);
        }
        for directory in crate::sessions::instruction_chain(root, cwd)? {
            let path = directory.join(PROJECT_CONFIG_DIRECTORY).join(CONFIG_FILE);
            let text = match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    return Err(AdapterError::HarnessError(
                        "the accepted workspace configuration could not be read".to_owned(),
                    ));
                }
            };
            let table: toml::Table = toml::from_str(&text).map_err(|_| {
                AdapterError::HarnessError(
                    "the accepted workspace configuration is not valid TOML".to_owned(),
                )
            })?;
            if self.project_budget
                && let Some(budget) = table
                    .get("project_doc_max_bytes")
                    .and_then(toml::Value::as_integer)
                && let Ok(budget) = u64::try_from(budget)
            {
                self.max_bytes = budget;
            }
            if self.project_filenames
                && let Some(names) = table
                    .get("project_doc_fallback_filenames")
                    .and_then(toml::Value::as_array)
            {
                self.filenames = names
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
        }
        Ok(self)
    }
}

/// Read the same effective configuration once for server isolation and managed-launch observations.
pub(crate) async fn session_configuration(
    rpc: &Rpc,
    place: Option<(&str, &str)>,
) -> Result<(BTreeMap<String, Json>, Vec<String>, InstructionConfig), AdapterError> {
    let mut looks: Vec<Json> = Vec::new();
    for directory in place.iter().flat_map(|(directory, cwd)| [*cwd, *directory]) {
        let look = json!({ "includeLayers": false, "cwd": directory });
        if !looks.contains(&look) {
            looks.push(look);
        }
    }
    if looks.is_empty() {
        looks.push(json!({ "includeLayers": false }));
    }
    let mut off = BTreeMap::new();
    let mut mismatches = Vec::new();
    let mut instructions = None;
    for look in looks {
        let read = rpc
            .call("config/read", look)
            .await
            .map_err(|error| AdapterError::HarnessError(remote_text(&error)))?;
        // The first lookup is the effective cwd; the directory-only lookup also isolates its servers.
        let _ = instructions.get_or_insert_with(|| InstructionConfig::from_read(&read));
        for key in crate::process::managed_pin_mismatches(&read["config"]) {
            if !mismatches.contains(&key) {
                mismatches.push(key);
            }
        }
        for (name, entry) in switch_off_entries(&read) {
            off.entry(name).or_insert(entry);
        }
    }
    if let Some((directory, cwd)) = place {
        for (name, entry) in project_entries(directory, cwd) {
            off.entry(name).or_insert(entry);
        }
    }
    Ok((
        off,
        mismatches,
        instructions.unwrap_or_else(|| InstructionConfig::from_read(&Json::Null)),
    ))
}

fn looks_secret(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_WORDS.iter().any(|word| upper.contains(word))
}

/// The definition of one server, as an import keeps it.
pub fn definition(table: &toml::Table) -> Result<McpDefinition, AdapterError> {
    let transport = transport_of(table).ok_or_else(|| {
        AdapterError::NotFound("the entry names neither a program nor an address".to_owned())
    })?;
    let command = table
        .get("command")
        .and_then(toml::Value::as_str)
        .map(|program| {
            let mut argv = vec![program.to_owned()];
            if let Some(args) = table.get("args").and_then(toml::Value::as_array) {
                argv.extend(
                    args.iter()
                        .filter_map(|arg| arg.as_str().map(str::to_owned)),
                );
            }
            argv
        });
    let mut env = BTreeMap::new();
    let mut secret_env = Vec::new();
    let mut secrets = BTreeMap::new();
    if let Some(variables) = table.get("env").and_then(toml::Value::as_table) {
        for (name, value) in variables {
            let Some(value) = value.as_str() else {
                continue;
            };
            if looks_secret(name) {
                secret_env.push(name.clone());
                secrets.insert(name.clone(), Secret::new(value));
            } else {
                env.insert(name.clone(), value.to_owned());
            }
        }
    }
    let seconds = table
        .get("startup_timeout_sec")
        .and_then(|seconds| {
            seconds
                .as_integer()
                .or_else(|| seconds.as_float().map(|float| float.ceil() as i64))
        })
        .and_then(|seconds| u32::try_from(seconds).ok())
        .or_else(|| {
            table
                .get("startup_timeout_ms")
                .and_then(toml::Value::as_integer)
                .and_then(|millis| u32::try_from((millis + 999) / 1000).ok())
        })
        .unwrap_or(DEFAULT_STARTUP_TIMEOUT_SECS);
    let tools = |key: &str| {
        table.get(key).and_then(toml::Value::as_array).map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
    };
    let working_dir = McpWorkingDir::Workspace;
    let strings = |key: &str| {
        table
            .get(key)
            .and_then(toml::Value::as_table)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|(name, value)| {
                        value.as_str().map(|value| (name.clone(), value.to_owned()))
                    })
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default()
    };
    let headers = strings("http_headers")
        .into_iter()
        .map(|(name, value)| (name, Secret::new(value)))
        .collect::<BTreeMap<_, _>>();
    let env_headers = strings("env_http_headers");
    let bearer_token_env_var = table
        .get("bearer_token_env_var")
        .and_then(toml::Value::as_str)
        .map(str::to_owned);
    let http_credentials = (transport == McpTransport::Http
        && (!headers.is_empty() || !env_headers.is_empty() || bearer_token_env_var.is_some()))
    .then_some(McpHttpCredentials {
        headers,
        env_headers,
        bearer_token_env_var,
        ..Default::default()
    });
    Ok(McpDefinition {
        http_credentials,
        spec: McpServerSpec {
            transport,
            command: (transport == McpTransport::Stdio)
                .then_some(command)
                .flatten(),
            url: table
                .get("url")
                .and_then(toml::Value::as_str)
                .map(str::to_owned),
            authentication: None,
            env,
            secret_env,
            working_dir,
            startup_timeout_secs: seconds,
            tools_allow: tools("enabled_tools"),
            tools_deny: tools("disabled_tools"),
            requires: McpRequires::default(),
        },
        secrets,
    })
}

/// Reads the definition an import asks for out of the configuration it names.
pub fn read_definition(
    home: Option<&Path>,
    request: &McpImportRequest,
) -> Result<McpDefinition, AdapterError> {
    let text = match request.scope {
        McpScope::User => {
            home.and_then(|home| std::fs::read_to_string(home.join(CONFIG_FILE)).ok())
        }
        McpScope::Directory => request
            .directory
            .as_deref()
            .and_then(|directory| std::fs::read_to_string(project_config(directory)).ok()),
    }
    .ok_or_else(|| {
        AdapterError::NotFound("the configuration that names the server is not there".to_owned())
    })?;
    let (_, table) = servers_in(&text)
        .into_iter()
        .find(|(name, _)| *name == request.name)
        .ok_or_else(|| {
            AdapterError::NotFound(format!(
                "the configuration names no server {:?}",
                request.name
            ))
        })?;
    definition(&table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_project_instruction_choices_follow_the_native_chain_without_replacing_session_flags()
     {
        use orch_agent_contract::session::ConfigTrust;
        let root = tempfile::tempdir().unwrap();
        let child = root.path().join("child");
        std::fs::create_dir(root.path().join(".codex")).unwrap();
        std::fs::create_dir_all(child.join(".codex")).unwrap();
        std::fs::write(
            root.path().join(".codex/config.toml"),
            "project_doc_max_bytes = 4\nproject_doc_fallback_filenames = ['ROOT.md']\n",
        )
        .unwrap();
        std::fs::write(
            child.join(".codex/config.toml"),
            "project_doc_max_bytes = 6\nproject_doc_fallback_filenames = [' CHILD.md ', '']\n",
        )
        .unwrap();
        let root = root.path().to_str().unwrap();
        let cwd = child.to_str().unwrap();
        let native = json!({"config":{"project_doc_max_bytes":64,"project_doc_fallback_filenames":["NATIVE.md"]},"origins":{}});
        for answer in [None, Some(ConfigTrust::KeepOut)] {
            let config = InstructionConfig::from_read(&native)
                .with_workspace(root, cwd, answer)
                .unwrap();
            assert_eq!(config.max_bytes, 64);
            assert_eq!(config.filenames, ["NATIVE.md"]);
        }
        let accepted = InstructionConfig::from_read(&native)
            .with_workspace(root, cwd, Some(ConfigTrust::Trust))
            .unwrap();
        assert_eq!(accepted.max_bytes, 6);
        assert_eq!(accepted.filenames, ["CHILD.md"]);
        for origin in [
            "sessionFlags",
            "legacyManagedConfigTomlFromFile",
            "legacyManagedConfigTomlFromMdm",
        ] {
            let mut protected = native.clone();
            protected["origins"] = json!({
                "project_doc_max_bytes":{"name":{"type":origin}},
                "project_doc_fallback_filenames":{"name":{"type":origin}}
            });
            let config = InstructionConfig::from_read(&protected)
                .with_workspace(root, cwd, Some(ConfigTrust::Trust))
                .unwrap();
            assert_eq!(config.max_bytes, 64);
            assert_eq!(config.filenames, ["NATIVE.md"]);
        }
    }

    const CONFIG: &str = r#"
model = "gpt-6-sol"

[mcp_servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
startup_timeout_sec = 20
enabled_tools = ["create_issue"]

[mcp_servers.github.env]
GITHUB_TOKEN = "ghp_secret_value"
LOG_LEVEL = "info"

[mcp_servers.docs]
url = "https://docs.example.test/mcp"
bearer_token_env_var = "DOCS_TOKEN"

[mcp_servers.broken]
args = ["no program"]
"#;

    #[test]
    fn the_servers_of_a_configuration_are_found_by_name_and_never_with_a_secret() {
        let found = found_in(CONFIG, McpScope::User, None);
        let names: Vec<_> = found
            .iter()
            .map(|server| (server.name.as_str(), server.transport))
            .collect();
        assert_eq!(
            names,
            [
                ("docs", McpTransport::Http),
                ("github", McpTransport::Stdio)
            ]
        );
        assert!(
            found
                .iter()
                .all(|server| server.source == "codex" && server.directory.is_none())
        );
        let written = serde_json::to_string(&found).unwrap();
        for forbidden in [
            "ghp_secret_value",
            "npx",
            "docs.example.test",
            "GITHUB_TOKEN",
        ] {
            assert!(!written.contains(forbidden), "{forbidden}");
        }
    }

    #[test]
    fn the_digest_of_a_server_changes_when_its_definition_does() {
        let one = found_in(CONFIG, McpScope::User, None);
        let same = found_in(CONFIG, McpScope::User, None);
        let changed = found_in(&CONFIG.replace("info", "debug"), McpScope::User, None);
        let digest = |servers: &[McpFound]| {
            servers
                .iter()
                .find(|server| server.name == "github")
                .unwrap()
                .digest
                .clone()
        };
        assert_eq!(digest(&one), digest(&same));
        assert_ne!(digest(&one), digest(&changed));
        assert_eq!(digest(&one).len(), 64);
    }

    #[test]
    fn native_configuration_discovery_and_the_imported_copy_share_the_same_digest() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(CONFIG_FILE), CONFIG).unwrap();
        for found in found(Some(home.path()), &[]) {
            let kept = read_definition(
                Some(home.path()),
                &McpImportRequest {
                    server_id: orch_types::new_id(),
                    source: KIND.to_owned(),
                    scope: found.scope,
                    name: found.name.clone(),
                    directory: found.directory.clone(),
                },
            )
            .unwrap();
            assert_eq!(found.digest, kept.imported(None).digest);
            assert_eq!(
                found.definition_fingerprint,
                Some(kept.definition_fingerprint())
            );
        }
        let before = found(Some(home.path()), &[]);
        std::fs::write(
            home.path().join(CONFIG_FILE),
            CONFIG.replace("ghp_secret_value", "different_private_token"),
        )
        .unwrap();
        let after = found(Some(home.path()), &[]);
        let github = |values: &[McpFound]| {
            values
                .iter()
                .find(|server| server.name == "github")
                .unwrap()
                .digest
                .clone()
        };
        assert_ne!(github(&before), github(&after));
        let public = serde_json::to_string(&after).unwrap();
        assert!(!public.contains("different_private_token"));
    }

    #[test]
    fn a_server_started_as_a_program_is_read_out_with_its_secrets_apart() {
        let table = servers_in(CONFIG)
            .into_iter()
            .find(|(name, _)| name == "github")
            .unwrap()
            .1;
        let read = definition(&table).unwrap();
        assert_eq!(read.spec.transport, McpTransport::Stdio);
        assert_eq!(
            read.spec.command.as_deref(),
            Some(
                &[
                    "npx".to_owned(),
                    "-y".to_owned(),
                    "@modelcontextprotocol/server-github".to_owned()
                ][..]
            )
        );
        assert_eq!(
            read.spec.env.get("LOG_LEVEL").map(String::as_str),
            Some("info")
        );
        assert_eq!(read.spec.secret_env, ["GITHUB_TOKEN"]);
        assert_eq!(read.secrets["GITHUB_TOKEN"].expose(), "ghp_secret_value");
        assert_eq!(read.spec.startup_timeout_secs, 20);
        assert_eq!(
            read.spec.tools_allow.as_deref(),
            Some(&["create_issue".to_owned()][..])
        );
        assert_eq!(read.check(), Ok(()));
        assert!(!format!("{read:?}").contains("ghp_secret_value"));
    }

    #[test]
    fn a_server_reached_by_address_is_read_out_by_its_address() {
        let table = servers_in(CONFIG)
            .into_iter()
            .find(|(name, _)| name == "docs")
            .unwrap()
            .1;
        let read = definition(&table).unwrap();
        assert_eq!(read.spec.transport, McpTransport::Http);
        assert_eq!(
            read.spec.url.as_deref(),
            Some("https://docs.example.test/mcp")
        );
        assert!(read.spec.command.is_none());
        assert_eq!(
            read.http_credentials
                .as_ref()
                .unwrap()
                .bearer_token_env_var
                .as_deref(),
            Some("DOCS_TOKEN")
        );
        assert_eq!(read.spec.startup_timeout_secs, DEFAULT_STARTUP_TIMEOUT_SECS);
        let broken = servers_in(CONFIG)
            .into_iter()
            .find(|(name, _)| name == "broken")
            .unwrap()
            .1;
        assert!(definition(&broken).is_err());
    }

    #[test]
    fn imported_http_credentials_keep_headers_and_variable_names_in_the_private_copy() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join(CONFIG_FILE),
            r#"
[mcp_servers.private_api]
url = "https://api.example.test/mcp"
bearer_token_env_var = "PRIVATE_API_TOKEN"
http_headers = { "X-Api-Key" = "private-header-value" }
env_http_headers = { "X-Workspace" = "PRIVATE_WORKSPACE" }
"#,
        )
        .unwrap();
        let found = found(Some(home.path()), &[]).pop().unwrap();
        let definition = read_definition(
            Some(home.path()),
            &McpImportRequest {
                server_id: orch_types::new_id(),
                source: KIND.to_owned(),
                scope: found.scope,
                name: found.name.clone(),
                directory: found.directory.clone(),
            },
        )
        .unwrap();
        let kept: McpDefinition =
            serde_json::from_slice(&serde_json::to_vec(&definition).unwrap()).unwrap();
        let credentials = kept.http_credentials.as_ref().unwrap();
        assert_eq!(
            credentials.headers["X-Api-Key"].expose(),
            "private-header-value"
        );
        assert_eq!(credentials.env_headers["X-Workspace"], "PRIVATE_WORKSPACE");
        assert_eq!(
            credentials.bearer_token_env_var.as_deref(),
            Some("PRIVATE_API_TOKEN")
        );
        assert_eq!(found.digest, kept.imported(None).digest);
        assert_eq!(
            found.definition_fingerprint,
            Some(kept.definition_fingerprint())
        );
        assert_eq!(kept.check(), Ok(()));
        let public = serde_json::to_string(&(found, kept.imported(None), &kept.spec)).unwrap();
        assert!(!public.contains("private-header-value"));
        assert!(!public.contains("PRIVATE_API_TOKEN"));
        assert!(!format!("{kept:?}").contains("private-header-value"));
        let mut changed = kept.clone();
        changed
            .http_credentials
            .as_mut()
            .unwrap()
            .headers
            .insert("X-Api-Key".into(), Secret::new("replacement-header-value"));
        assert_ne!(changed.digest(), kept.digest());
        assert_eq!(
            changed.definition_fingerprint(),
            kept.definition_fingerprint()
        );
    }

    #[test]
    fn the_homes_and_a_directorys_configuration_are_both_read() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(CONFIG_FILE), CONFIG).unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let project = workspace.path().join(PROJECT_CONFIG_DIRECTORY);
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join(CONFIG_FILE),
            "[mcp_servers.local]\ncommand = \"./serve\"\n",
        )
        .unwrap();
        let directory = workspace.path().to_string_lossy().into_owned();
        let all = found(Some(home.path()), std::slice::from_ref(&directory));
        assert_eq!(all.len(), 3);
        let local = all.iter().find(|server| server.name == "local").unwrap();
        assert_eq!(
            (local.scope, local.directory.as_deref()),
            (McpScope::Directory, Some(directory.as_str()))
        );
        let request = McpImportRequest {
            server_id: orch_types::new_id(),
            source: "codex".into(),
            scope: McpScope::Directory,
            name: "local".into(),
            directory: Some(directory),
        };
        let read = read_definition(Some(home.path()), &request).unwrap();
        assert_eq!(
            read.spec.command.as_deref(),
            Some(&["./serve".to_owned()][..])
        );
        let gone = McpImportRequest {
            name: "nothing".into(),
            ..request
        };
        assert!(matches!(
            read_definition(Some(home.path()), &gone),
            Err(AdapterError::NotFound(_))
        ));
    }

    #[test]
    fn what_the_harness_would_load_is_switched_off_by_entries_that_stand_on_their_own() {
        let off = switch_off_entries(&json!({ "config": { "model": null, "mcp_servers": {
            "github": { "command": "npx", "args": ["-y", "x"], "environment_id": "local", "enabled": true },
            "docs": { "url": "https://docs.example.test/mcp", "enabled": true },
            "odd": { "enabled": true }
        } } }));
        assert_eq!(
            off["github"],
            json!({ "command": "npx", "enabled": false }),
            "the arguments stay where they are: arrays are not merged, they are replaced"
        );
        assert_eq!(
            off["docs"],
            json!({ "url": "https://docs.example.test/mcp", "enabled": false })
        );
        assert_eq!(off["odd"], json!({ "enabled": false }));
        assert_eq!(off.len(), 3);
        for read in [
            json!({ "config": { "mcp_servers": null } }),
            json!({ "config": {} }),
            json!({}),
        ] {
            assert!(switch_off_entries(&read).is_empty());
        }
    }

    #[test]
    fn the_directories_whose_configuration_counts_are_those_from_the_working_directory_up_to_the_workspace()
     {
        let chain = |directory: &str, cwd: &str| -> Vec<String> {
            project_chain(directory, cwd)
                .iter()
                .map(|at| at.to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(chain("/w/app", "/w/app"), ["/w/app"]);
        assert_eq!(
            chain("/w/app", "/w/app/a/b"),
            ["/w/app/a/b", "/w/app/a", "/w/app"]
        );
        assert_eq!(
            chain("/w/app", "/w/application"),
            ["/w/application", "/w/app"],
            "a directory that only shares the beginning of the name is not inside"
        );
        assert_eq!(chain("/w/app", "/elsewhere"), ["/elsewhere", "/w/app"]);
    }

    #[test]
    fn the_servers_of_the_project_configurations_are_switched_off_by_entries_that_stand_alone() {
        let workspace = tempfile::tempdir().unwrap();
        let inner = workspace.path().join("pkg");
        for (at, text) in [
            (
                workspace.path(),
                "[mcp_servers.top]\ncommand = \"./serve\"\n[mcp_servers.address]\nurl = \"http://127.0.0.1:9/mcp\"\n[mcp_servers.nameless]\nargs = [\"x\"]\n",
            ),
            (
                inner.as_path(),
                "[mcp_servers.deep]\ncommand = \"./deeper\"\n[mcp_servers.top]\ncommand = \"./shadow\"\n",
            ),
        ] {
            std::fs::create_dir_all(at.join(PROJECT_CONFIG_DIRECTORY)).unwrap();
            std::fs::write(at.join(PROJECT_CONFIG_DIRECTORY).join(CONFIG_FILE), text).unwrap();
        }
        let entries = project_entries(
            &workspace.path().to_string_lossy(),
            &inner.to_string_lossy(),
        );
        assert_eq!(
            entries.keys().map(String::as_str).collect::<Vec<_>>(),
            ["address", "deep", "top"],
            "an entry with neither a program nor an address is no server"
        );
        assert_eq!(
            entries["top"],
            json!({ "command": "./shadow", "enabled": false }),
            "the nearer configuration is the one that counts"
        );
        assert_eq!(
            entries["address"],
            json!({ "url": "http://127.0.0.1:9/mcp", "enabled": false })
        );
    }

    #[test]
    fn a_thread_of_the_harness_is_a_session_to_adopt() {
        let session = session_of(&json!({
            "id": "thr_1", "name": null, "preview": "Fix the login redirect", "cwd": "/workspace/web",
            "updatedAt": 1_791_730_800, "parentThreadId": null
        }))
        .unwrap();
        assert_eq!(session.native_session_ref, "thr_1");
        assert_eq!(session.title.as_deref(), Some("Fix the login redirect"));
        assert_eq!(session.cwd.as_deref(), Some("/workspace/web"));
        assert!(session.updated_at.is_some() && session.adopted_thread_id.is_none());
        let named = session_of(
            &json!({ "id": "thr_2", "name": "Named", "preview": "x", "cwd": "/w", "updatedAt": 1 }),
        )
        .unwrap();
        assert_eq!(named.title.as_deref(), Some("Named"));
    }

    /// What the pinned harness answered, recorded: a thread's own record, and its turns with their items.
    const THREAD_READ: &str = include_str!("../tests/fixtures/adopt/thread-read.json");
    const TURNS_LIST: &str = include_str!("../tests/fixtures/adopt/turns-list.json");

    fn recorded() -> (Option<String>, Option<String>, Vec<Vec<RecordedEntry>>) {
        let read: Json = serde_json::from_str(THREAD_READ).unwrap();
        let page: Json = serde_json::from_str(TURNS_LIST).unwrap();
        let turns = page["data"].as_array().unwrap().clone();
        let dir = tempfile::tempdir().unwrap();
        let outputs = crate::events::OutputStore::new(dir.path().join("outputs"));
        let clock = orch_types::ManualClock::new(
            chrono::DateTime::from_timestamp(1_791_800_000, 0).unwrap(),
        );
        let ctx = crate::events::Context {
            clock: &clock,
            outputs: &outputs,
            windows: None,
        };
        record_of(&read["thread"], &turns, &ctx, |_| orch_types::new_id())
    }

    #[test]
    fn a_thread_s_record_holds_what_the_person_said_and_what_the_agent_did_turn_by_turn() {
        let (model, effort, turns) = recorded();
        assert_eq!(
            model.as_deref(),
            Some("mock-large"),
            "the model the thread was configured with"
        );
        assert_eq!(effort, None, "the harness recorded no effort for it");
        assert_eq!(
            turns.len(),
            3,
            "the turn still running is not part of what was recorded"
        );
        let RecordedEntry::Said(first) = &turns[0][0] else {
            panic!("a turn begins with what the person said: {:?}", turns[0][0]);
        };
        assert_eq!(first.text, "remember the word banana");
        assert_eq!(
            first.native_ref,
            "said:01a12ab3-f58f-7da2-ab39-2d575a0c6c88"
        );
        assert_eq!(first.turn, 1);
        assert!(first.at.is_some(), "when the turn began");
        let RecordedEntry::Item(answer) = &turns[0][1] else {
            panic!("then what the agent said: {:?}", turns[0][1]);
        };
        assert_eq!(
            answer.native_ref, "msg_mock_000021_0",
            "the name a resync gives the same item"
        );
        assert!(matches!(
            &answer.content,
            orch_agent_contract::events::ItemContent::AgentMessage(message)
                if message.text == "You said: remember the word banana"
        ));
        let kinds: Vec<&str> = turns[2]
            .iter()
            .map(|entry| match entry {
                RecordedEntry::Said(_) => "said",
                RecordedEntry::Item(item) => match &item.content {
                    orch_agent_contract::events::ItemContent::ToolCall(_) => "tool_call",
                    orch_agent_contract::events::ItemContent::AgentMessage(_) => "agent_message",
                    _ => "other",
                },
            })
            .collect();
        assert_eq!(
            kinds,
            ["said", "tool_call", "agent_message"],
            "in the order the harness recorded them"
        );
        let RecordedEntry::Item(command) = &turns[2][1] else {
            unreachable!("checked above");
        };
        let orch_agent_contract::events::ItemContent::ToolCall(call) = &command.content else {
            unreachable!("checked above");
        };
        assert!(
            matches!(&call.tool, orch_agent_contract::events::ToolPayload::Command { exit_code: Some(0), output, .. }
                if output.inline.contains("tick20")),
            "the command with what it printed: {:?}",
            call.tool
        );
        assert!(
            call.raw.name.contains("commandExecution"),
            "and as the harness made it: {}",
            call.raw.name
        );
    }

    #[test]
    fn a_record_says_what_it_holds_by_kind_and_what_it_left_out() {
        let (.., turns) = recorded();
        let report = record_report(&turns, 2);
        assert_eq!(report.by_kind["user_message"].kept, 3);
        assert_eq!(report.by_kind["agent_message"].kept, 3);
        assert_eq!(report.by_kind["tool_call"].kept, 1);
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.starts_with("2 earlier turns")),
            "{:?}",
            report.warnings
        );
    }
}
