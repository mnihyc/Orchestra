//! Orch's own tools as the MCP server `orch` on the listener: the tools a session was given, listed to its harness, and each call carried to the system as an event and answered with its result, or with the error that Orch is unreachable when nothing answers in time.
//!
//! The tools belong to the system, which checks and does every call itself; this module only presents them to the
//! harness and relays. A harness that is given the server by its adapter (Codex and Claude Code do) reaches
//! `/mcp/<session key>/orch` with the token of its process, speaks the protocol's streamable HTTP in its plainest
//! form (every request is one POST, answered with one JSON message), and finds the tools the session was opened
//! with, listed with the schemas, titles and hints the system declares for them.
//!
//! **A call** (`tools/call`) becomes an `orch.tool_call` event of the thread's session, in the turn that runs, under
//! a reference of its own; the system answers it with an `orch.tool_result` command, which [`OrchCalls`] hands to
//! the request that waits. Reading results are text; effects are one structured object with no text duplicate.
//! A refusal is a result marked as an error whose text begins with its code and says what to do, so that the model
//! sees it and corrects itself. If no result comes within [`RESULT_WAIT`] the harness is answered with the error
//! a timeout that says whether an effect may still have happened.
//!
//! **A call made again.** After a call's answer did not come in time, the calling thread, the tool, the hash of its
//! arguments written canonically, actual turn/segment/native-conversation fingerprint and call reference are kept for [`RETAINED_FOR`], in the connector's record
//! on disk (`ledger`), so that a restart keeps them. The same tool called again with the same arguments by that thread
//! in that same native source is the same call: it is answered with the answer that came late, if one did, and is
//! otherwise carried again under the same reference, which the system answers without doing the call's effect twice.
//! An identical call made while one is under way in that source waits for its answer. Another turn or native lane
//! gets its own reference. A record from an older connector with no source refuses matching work until its original
//! bound expires; the current source is never guessed. After the interval a call is a new call.
//!
//! **The listing** is `orch_tools.json`: the definitions of the tools as the system declares them with its shipped texts,
//! kept beside this module. It is produced from the system's definitions and a test of the whole (the harness
//! lists the tools, and what it is given equals what the system declares) fails when the two part.

use crate::harnesses::Hosting;
use crate::loopback::{Body, Endpoint, SessionBinding, text};
use async_trait::async_trait;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use orch_agent_contract::loopback::ORCH_SERVER;
use orch_link_contract::commands::{OrchTool, ToolError};
use orch_link_contract::events::{Event, OrchToolCall};
use orch_link_contract::streams::Stream;
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;
use tokio::sync::oneshot;

/// How long an ordinary call waits, leaving ten seconds before the harness's deadline.
pub const RESULT_WAIT: Duration = Duration::from_secs(50);

/// How much longer than an ordinary call `open_port` waits: 70 s where the ordinary wait is 50, so that the
/// system's sixty seconds for a person's decision answer within it, ten seconds before the harness's deadline of
/// 80 s while `open_port` is offered.
fn wait_for(tool: OrchTool, ordinary: Duration) -> Duration {
    if tool == OrchTool::OpenPort {
        ordinary * 7 / 5
    } else {
        ordinary
    }
}

/// What the harness is told when no answer comes in time.
pub const UNREACHABLE: &str = "Orch did not answer in time. Nothing was changed. Try again.";

/// A missing answer to a changing call is not evidence that its effect did not happen.
pub const TIMEOUT_CHANGE: &str = "Orch did not answer in time, and the call may still have been carried out. Look before you repeat it (read_shared or list_shared on what it was to change): repeating it blindly does it twice.";

/// What a call's reference begins with, so that its result is known for the call of this server and not of an
/// adapter that makes calls of its own.
pub const CALL_PREFIX: &str = "mcp:";

/// How long a call whose answer did not come in time is kept as the call that the same call made again is: a
/// model's immediate retry and the confirmation interval of the tools that ask twice, and not so long that a later
/// intentional repetition is the same call forever. An older record with no source is never assigned to the
/// current turn: matching work is refused within that record's original retention bound.
pub const RETAINED_FOR: Duration = Duration::from_secs(10 * 60);

/// Arguments written canonically: an object's members in the order of their names, at every depth, so that two calls
/// with the same arguments written in another order are the same call.
fn canonical(value: &Json) -> String {
    match value {
        Json::Object(members) => {
            let mut names: Vec<&String> = members.keys().collect();
            names.sort();
            let inner: Vec<String> = names
                .into_iter()
                .map(|name| {
                    format!(
                        "{}:{}",
                        Json::String(name.clone()),
                        canonical(&members[name])
                    )
                })
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        Json::Array(items) => format!(
            "[{}]",
            items.iter().map(canonical).collect::<Vec<_>>().join(",")
        ),
        other => other.to_string(),
    }
}

/// The hash of arguments written canonically.
#[must_use]
pub fn arguments_hash(arguments: &Json) -> String {
    orch_types::Hash::of(canonical(arguments).as_bytes()).to_string()
}

/// Seconds since the epoch, now.
fn epoch_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

/// The most bytes of a request the server reads: the arguments of a tool are text, and a message is 8 000
/// characters at most, a few times that in bytes.
const REQUEST_MAX_BYTES: usize = 1024 * 1024;

/// Complete default answers fit the ordinary native result budgets, including their metadata.
const ANSWER_BYTES_DEFAULT: usize = 20_000;
/// A complete answer stays below the native saved-file threshold even in ASCII.
const ANSWER_BYTES_MAX: usize = 40_000;
/// Leave room for a header, one useful part and a continuation instruction.
const ANSWER_BYTES_MIN: usize = 1_000;

/// The versions of the protocol the server speaks, newest first. A client asks for one; the server answers with
/// it if it knows it and with the newest otherwise, and the client goes on or goes away.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];

/// The code of a JSON-RPC request that is none.
const INVALID_REQUEST: i64 = -32600;
/// The code of a method nobody serves.
const METHOD_NOT_FOUND: i64 = -32601;
/// The code of parameters that do not fit.
const INVALID_PARAMS: i64 = -32602;
/// The code of a message that is no JSON.
const PARSE_ERROR: i64 = -32700;

/// The definitions of the tools, as the system declares them.
fn listing() -> &'static [Json] {
    static LISTING: OnceLock<Vec<Json>> = OnceLock::new();
    LISTING
        .get_or_init(|| serde_json::from_str(include_str!("orch_tools.json")).unwrap_or_default())
}

/// The definitions of these tools, with the descriptions a place set in place of the shipped ones, in the order
/// the system lists them.
#[must_use]
pub fn definitions_with(
    tools: &[OrchTool],
    descriptions: &std::collections::BTreeMap<String, String>,
) -> Vec<Json> {
    definitions_of(tools)
        .into_iter()
        .map(|mut definition| {
            if let Some(own) = definition
                .get("name")
                .and_then(Json::as_str)
                .and_then(|name| descriptions.get(name))
            {
                definition["description"] = Json::String(supplied(own));
            }
            definition
        })
        .collect()
}

/// The current narrowed schemas supplied by the engine; the static catalogue stays unchanged.
#[must_use]
pub fn definitions_with_schemas(
    tools: &[OrchTool],
    descriptions: &std::collections::BTreeMap<String, String>,
    schemas: &std::collections::BTreeMap<String, Json>,
) -> Vec<Json> {
    definitions_with(tools, descriptions)
        .into_iter()
        .map(|mut definition| {
            if let Some(schema) = definition["name"]
                .as_str()
                .and_then(|name| schemas.get(name))
            {
                definition["inputSchema"] = schema.clone();
            }
            definition
        })
        .collect()
}

/// The definitions of these tools, in the order the system lists them.
#[must_use]
pub fn definitions_of(tools: &[OrchTool]) -> Vec<Json> {
    let wanted: Vec<String> = tools
        .iter()
        .filter_map(|tool| serde_json::to_value(tool).ok())
        .filter_map(|word| word.as_str().map(str::to_owned))
        .collect();
    listing()
        .iter()
        .filter(|definition| {
            definition
                .get("name")
                .and_then(Json::as_str)
                .is_some_and(|name| wanted.iter().any(|wanted| wanted == name))
        })
        .cloned()
        .collect()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where the answer to a call is sent when it comes.
type Reply = oneshot::Sender<Result<Json, ToolError>>;

/// Arguments alone never identify work from another native turn or session generation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CallKey {
    thread: Id,
    source: crate::ledger::ToolCallSource,
    tool: String,
    arguments_hash: String,
}

type PendingAnswer = oneshot::Receiver<Result<Json, ToolError>>;
type Beginning = (CallKey, Begun, Option<PendingAnswer>);

/// How a call begins.
#[derive(Debug, Clone, PartialEq)]
enum Begun {
    /// A new call, under a new reference.
    New(String),
    /// The same call as one whose answer did not come in time: carried again under its reference.
    Again(String),
    /// The same call as one under way: it waits for that one's answer.
    Attached(String),
    /// The same call as one whose answer came late: this is its answer.
    Answered(Result<Json, ToolError>),
}

/// The calls that wait for the system's answer, by thread and reference; those under way, by what makes them the same
/// call; and those whose answer did not come in time, kept for [`RETAINED_FOR`] in the connector's record.
#[derive(Default)]
struct CallsState {
    waiting: HashMap<(Id, String), Vec<Reply>>,
    under_way: HashMap<CallKey, String>,
    retained: Vec<crate::ledger::RetainedCall>,
}

#[derive(Default)]
pub struct OrchCalls {
    state: Mutex<CallsState>,
    ledger: Option<Arc<crate::ledger::Ledger>>,
}

impl std::fmt::Debug for OrchCalls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrchCalls").finish_non_exhaustive()
    }
}

impl OrchCalls {
    /// The connector's private replay record, with no inferred source for an older entry.
    #[must_use]
    pub fn with_ledger(ledger: Arc<crate::ledger::Ledger>) -> Self {
        Self {
            state: Mutex::new(CallsState {
                retained: ledger.retained_calls(),
                ..Default::default()
            }),
            ledger: Some(ledger),
        }
    }

    fn keep(&self, retained: &[crate::ledger::RetainedCall]) -> Result<(), ToolError> {
        if let Some(ledger) = &self.ledger {
            ledger.try_keep_retained_calls(retained).map_err(|error| {
                tracing::error!(%error,"the own-tool replay identity could not be retained");
                ToolError {code:"unavailable".into(),message:"The connector could not retain this call's identity. Nothing new was submitted. Check free space and write access in its data directory, then retry.".into()}
            })?;
        }
        Ok(())
    }

    /// Same-source retries keep their effect/ref. A different native turn/segment/session
    /// gets its own reference, even when its arguments are byte-for-byte identical.
    fn begin(
        &self,
        thread: Id,
        source: crate::ledger::ToolCallSource,
        tool: &str,
        arguments: &Json,
        wait: Duration,
    ) -> Beginning {
        let key = CallKey {
            thread,
            source,
            tool: tool.into(),
            arguments_hash: arguments_hash(arguments),
        };
        let mut state = lock(&self.state);
        if let Some(call_ref) = state.under_way.get(&key).cloned() {
            let answer = Self::expect_in(&mut state, thread, &call_ref);
            return (key, Begun::Attached(call_ref), Some(answer));
        }
        let now = epoch_now();
        state.retained.retain(|call| call.until > now);
        let kept = state.retained.iter().position(|call| {
            call.thread_id == thread
                && call.source == Some(source)
                && call.tool == key.tool
                && call.arguments_hash == key.arguments_hash
        });
        let begun = if let Some(at) = kept {
            let call = &mut state.retained[at];
            if let Some(answer) = &call.answer {
                return (key, Begun::Answered(answer.clone()), None);
            }
            call.timed_out = true; // A restart/earlier waiter left this exact outcome unconfirmed.
            Begun::Again(call.call_ref.clone())
        } else {
            if state.retained.iter().any(|call| {
                call.thread_id == thread
                    && call.source.is_none()
                    && call.tool == key.tool
                    && call.arguments_hash == key.arguments_hash
            }) {
                return (key,Begun::Answered(Err(ToolError {code:"source_unconfirmed".into(),
                    message:"A retained call has no recorded native source. Its result or effect cannot be assigned to this turn. Nothing new was submitted. Inspect the target before repeating the operation after that record expires.".into()})),None);
            }
            let call_ref = format!("{CALL_PREFIX}{}", new_id());
            state.retained.push(crate::ledger::RetainedCall {
                thread_id: thread,
                source: Some(source),
                timed_out: false,
                tool: key.tool.clone(),
                arguments_hash: key.arguments_hash.clone(),
                call_ref: call_ref.clone(),
                until: now
                    .saturating_add(i64::try_from(RETAINED_FOR.as_secs()).unwrap_or(i64::MAX))
                    .saturating_add(
                        i64::try_from(
                            wait.as_secs()
                                .saturating_add(u64::from(wait.subsec_nanos() != 0)),
                        )
                        .unwrap_or(i64::MAX),
                    ),
                answer: None,
            });
            Begun::New(call_ref)
        };
        // Persist before publishing. Failure retains the same unissued identity in memory;
        // another attempt must persist that identity before it may carry the call.
        if let Err(error) = self.keep(&state.retained) {
            return (key, Begun::Answered(Err(error)), None);
        }
        let call_ref = if let Begun::New(call_ref) | Begun::Again(call_ref) = &begun {
            call_ref.clone()
        } else {
            return (
                key,
                Begun::Answered(Err(ToolError {
                    code: "unavailable".into(),
                    message: "The connector did not admit this call. Nothing new was submitted."
                        .into(),
                })),
                None,
            );
        };
        state.under_way.insert(key.clone(), call_ref.clone());
        // Registration and attachment share the same state cut with answer(), so an
        // answer cannot win between begin and expect and leave a new waiter stranded.
        let answer = Self::expect_in(&mut state, thread, &call_ref);
        (key, begun, Some(answer))
    }

    /// A missing reply keeps its original source/ref for the existing ten-minute interval.
    fn timed_out(&self, key: &CallKey, call_ref: &str) {
        let mut state = lock(&self.state);
        if state
            .under_way
            .get(key)
            .is_some_and(|under| under == call_ref)
        {
            state.under_way.remove(key);
        }
        let entry = (key.thread, call_ref.to_owned());
        if let Some(replies) = state.waiting.get_mut(&entry) {
            replies.retain(|reply| !reply.is_closed());
            if replies.is_empty() {
                state.waiting.remove(&entry);
            }
        }
        let now = epoch_now();
        let until = now + i64::try_from(RETAINED_FOR.as_secs()).unwrap_or(i64::MAX);
        state.retained.retain(|call| call.until > now);
        match state
            .retained
            .iter_mut()
            .find(|call| call.thread_id == key.thread && call.call_ref == call_ref)
        {
            Some(call) => {
                call.until = until;
                call.timed_out = true;
            }
            None => state.retained.push(crate::ledger::RetainedCall {
                thread_id: key.thread,
                source: Some(key.source),
                timed_out: true,
                tool: key.tool.clone(),
                arguments_hash: key.arguments_hash.clone(),
                call_ref: call_ref.into(),
                until,
                answer: None,
            }),
        }
        let _ = self.keep(&state.retained); // The timeout already reports an uncertain possible effect.
    }

    fn expect_in(state: &mut CallsState, thread: Id, call_ref: &str) -> PendingAnswer {
        let (reply, answer) = oneshot::channel();
        state
            .waiting
            .entry((thread, call_ref.into()))
            .or_default()
            .push(reply);
        answer
    }

    #[cfg(test)]
    fn expect(&self, thread: Id, call_ref: &str) -> PendingAnswer {
        Self::expect_in(&mut lock(&self.state), thread, call_ref)
    }

    /// A failed publish is not proof that another reference may repeat a changing effect.
    fn forget(&self, key: &CallKey, call_ref: &str) {
        self.timed_out(key, call_ref);
        lock(&self.state)
            .waiting
            .remove(&(key.thread, call_ref.into()));
    }

    /// Unique listener references route only to their own waiters/retained record. Even an
    /// unknown expired mcp reference is consumed here, never sent to the newly selected adapter.
    pub fn answer(&self, thread: Id, call_ref: &str, outcome: Result<Json, ToolError>) -> bool {
        if !call_ref.starts_with(CALL_PREFIX) {
            return false;
        }
        let mut state = lock(&self.state);
        let outcome = state
            .retained
            .iter()
            .find(|call| call.thread_id == thread && call.call_ref == call_ref)
            .and_then(|call| call.answer.clone())
            .unwrap_or(outcome);
        state
            .under_way
            .retain(|key, under| !(key.thread == thread && under == call_ref));
        let replies = state
            .waiting
            .remove(&(thread, call_ref.into()))
            .unwrap_or_default();
        // Every attached waiter must receive the answer, including after an
        // earlier send succeeds. A short-circuiting iterator would lose them.
        let mut heard = false;
        for reply in replies {
            heard |= reply.send(outcome.clone()).is_ok();
        }
        if let Some(at) = state
            .retained
            .iter()
            .position(|call| call.thread_id == thread && call.call_ref == call_ref)
        {
            if !heard || state.retained[at].timed_out {
                state.retained[at].timed_out = true;
                state.retained[at].answer = Some(outcome);
                let _ = self.keep(&state.retained);
            } else {
                // Successful ordinary calls can be intentionally repeated; only uncertain
                // calls remain argument-deduplicated. A failed flush keeps the old identity.
                let mut completed = state.retained.clone();
                completed.remove(at);
                if self.keep(&completed).is_ok() {
                    state.retained = completed;
                } else {
                    state.retained[at].answer = Some(outcome);
                    state.retained[at].timed_out = true;
                }
            }
        }
        true
    }
}

/// The server called `orch`, on the address `/mcp/<session key>/orch`.
pub struct OrchEndpoint {
    hosting: Arc<Hosting>,
    wait: Duration,
}

impl OrchEndpoint {
    /// The server over the harnesses a connector hosts.
    #[must_use]
    pub fn new(hosting: Arc<Hosting>) -> Self {
        Self::with_wait(hosting, RESULT_WAIT)
    }

    /// The same, giving the system `wait` to answer a call.
    #[must_use]
    pub fn with_wait(hosting: Arc<Hosting>, wait: Duration) -> Self {
        Self { hosting, wait }
    }
}

fn reply(status: StatusCode, body: &Json) -> Response<Body> {
    let mut response = Response::new(
        Full::new(Bytes::from(serde_json::to_vec(body).unwrap_or_default()))
            .map_err(|never: Infallible| match never {})
            .boxed(),
    );
    *response.status_mut() = status;
    response.headers_mut().insert(
        hyper::header::CONTENT_TYPE,
        hyper::header::HeaderValue::from_static("application/json"),
    );
    response
}

fn failure(id: &Json, code: i64, message: &str) -> Json {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// The result is one readable form, or one refusal beginning with its code.
fn supplied(text: &str) -> String {
    orch_agent_contract::tool_answers::supplied(text)
}

fn field(value: &Json, name: &str) -> String {
    supplied(value.get(name).and_then(Json::as_str).unwrap_or_default())
}

/// A directory or search row stays one line even when supplied names or descriptions contain line breaks.
fn line_text(text: &str) -> String {
    supplied(text).replace('\r', "\\r").replace('\n', "\\n")
}

fn line_field(value: &Json, name: &str) -> String {
    line_text(value.get(name).and_then(Json::as_str).unwrap_or_default())
}

/// Remove only a whole directory prefix; similarly named siblings remain absolute.
fn relative_name(path: &str, root: &str) -> String {
    let prefix = format!("{}/", root.trim_end_matches('/'));
    line_text(path.strip_prefix(&prefix).unwrap_or(path))
}

/// One rounded-down unit is enough to compare recency; an unknown date never becomes a fresh one.
fn changed_age(value: &str, now: Time) -> Option<String> {
    let at: Time = value.parse().ok()?;
    let seconds = now.unix_millis().saturating_sub(at.unix_millis()).max(0) / 1_000;
    for (unit, suffix) in [(86_400, "d"), (3_600, "h"), (60, "m")] {
        if seconds >= unit {
            return Some(format!("changed {}{suffix} ago", seconds / unit));
        }
    }
    Some("changed just now".to_owned())
}

fn listing_row(entry: &Json, root: &str, ids: bool, now: Time) -> String {
    let mut line = format!(
        "{} · {}",
        relative_name(entry["path"].as_str().unwrap_or_default(), root),
        line_field(entry, "kind")
    );
    for name in ["agent", "state", "status", "description"] {
        if let Some(text) = entry[name].as_str().filter(|text| !text.is_empty()) {
            line.push_str(&format!(" · {}", line_text(text)));
        }
    }
    if let Some(at) = entry["updated_at"].as_str() {
        line.push_str(&format!(
            " · {}",
            changed_age(at, now).unwrap_or_else(|| "change time unknown".to_owned())
        ));
    }
    if let Some(children) = entry["children"].as_u64() {
        line.push_str(&format!(" · children: {children}"));
    }
    if entry["is_self"] == true {
        line.push_str(" · this is you");
    }
    if entry["writable"] == true {
        line.push_str(" · read, write");
    } else if entry["readable"] == true {
        line.push_str(" · read");
    }
    if entry["manageable"] == true {
        line.push_str(" · manageable");
    }
    if entry["deleted"] == true {
        line.push_str(" · deleted");
    }
    if entry["archived"] == true && entry["state"] != "archived" {
        line.push_str(" · archived");
    }
    if let Some(parts) = entry.get("parts") {
        line.push_str(&format!(" · {parts} {}s", line_field(entry, "unit")));
    }
    if let Some(size) = entry.get("size") {
        line.push_str(&format!(" · {size} bytes"));
    }
    if ids && let Some(id) = entry.get("id") {
        line.push_str(&format!(" · id {id}"));
    }
    line
}

/// JSON escapes preserve the exact cursor while keeping supplied paths out of reserved markup.
fn cursor_word(cursor: &str) -> String {
    Json::String(cursor.to_owned())
        .to_string()
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
}

fn next_listing(cursor: &str) -> String {
    format!(
        "Continue list_shared with cursor {} and the same path, sort and filters.",
        cursor_word(cursor)
    )
}

/// A local byte cut needs the last displayed entry's cursor, never the end of the unseen page.
fn listing_cut(arguments: &Json, last: Option<&Json>) -> String {
    if let Some(cursor) = last.and_then(|entry| entry["cursor"].as_str()) {
        return format!("Answer cut to fit. {}", next_listing(cursor));
    }
    let start = arguments["cursor"]
        .as_str()
        .map_or_else(String::new, |cursor| {
            format!(" with starting cursor {}", cursor_word(cursor))
        });
    format!(
        "Answer cut to fit. Repeat this same listing page{start} with a smaller limit or larger max_bytes (at most 40000); keep the path, sort and filters. Unshown entries have not been skipped."
    )
}

fn listing_text(value: &Json, arguments: &Json, ids: bool, now: Time, budget: usize) -> String {
    if let Some(text) = value.as_str() {
        return bounded_lines(&supplied(text), budget, &listing_cut(arguments, None));
    }
    let root = value["path"]
        .as_str()
        .or_else(|| arguments["path"].as_str())
        .unwrap_or(".");
    let about = &value["about"];
    let mut header = line_text(root);
    if !about.is_null() {
        header.push_str(&format!(
            " · {} \"{}\"",
            line_field(about, "kind"),
            line_field(about, "name")
        ));
        if let Some(status) = about["status"].as_str() {
            header.push_str(&format!(" · {}", line_text(status)));
        }
    }
    if let Some(notice) = value["also_named_so"].as_str() {
        header.push('\n');
        header.push_str(&line_text(notice));
    }
    let entries = value["entries"].as_array().map_or(&[][..], Vec::as_slice);
    let rows: Vec<_> = entries
        .iter()
        .map(|entry| listing_row(entry, root, ids, now))
        .collect();
    let archived = value["archived_count"].as_u64().filter(|count| *count > 0)
        .map(|count| format!("{count} archived entries omitted; list again without a cursor and with include_archived:true to show them."));
    let more = value["next_cursor"].as_str().map(next_listing).or_else(|| (value["truncated"] == true)
        .then(|| "More entries remain; no continuation cursor was supplied. Repeat with a smaller limit or narrow the path.".to_owned()));
    let mut lines = vec![header.clone()];
    lines.extend(rows.iter().cloned());
    if rows.is_empty() && archived.is_none() {
        lines.push("No entries on this page.".to_owned());
    }
    lines.extend(archived.iter().cloned());
    lines.extend(more);
    let whole = lines.join("\n");
    if whole.len() <= budget {
        return whole;
    }
    // Prefix lengths make this linear in the page size; build the selected text only once.
    let mut lengths = vec![header.len()];
    for row in &rows {
        lengths.push(lengths.last().copied().unwrap_or_default() + 1 + row.len());
    }
    for shown in (0..rows.len()).rev() {
        let continuation = listing_cut(
            arguments,
            shown.checked_sub(1).and_then(|at| entries.get(at)),
        );
        let extra = archived.as_ref().map_or(0, |line| line.len() + 1);
        if lengths[shown] + extra + 1 + continuation.len() <= budget {
            let mut lines = vec![header];
            lines.extend(rows[..shown].iter().cloned());
            lines.extend(archived);
            lines.push(continuation);
            return lines.join("\n");
        }
    }
    let explanation = "The listing header or continuation does not fit. Repeat the same page with a larger max_bytes (at most 40000). No entries were shown or skipped.";
    bounded_lines(explanation, budget, explanation)
}

fn reading_text(tool: &str, value: &Json, ids: bool, snippets: bool) -> String {
    if let Some(text) = value.as_str() {
        return supplied(text);
    }
    match tool {
        "read_shared" => {
            let entry = &value["entry"];
            let mut header = format!(
                "{} · {} \"{}\" · {}s {}-{} of {}",
                field(entry, "path"),
                field(entry, "kind"),
                field(entry, "name"),
                field(entry, "unit"),
                value["from"],
                value["to"],
                value["last"]
            );
            if let Some(seq) = entry.get("seq") {
                header.push_str(&format!(" · seq {seq}"));
            }
            if ids && let Some(id) = entry.get("id") {
                header.push_str(&format!(" · id {id}"));
            }
            let mut lines = vec![header];
            if let Some(notice) = value["also_named_so"].as_str() {
                lines.push(line_text(notice));
            }
            lines.push(field(value, "text"));
            if let Some(files) = value["files"].as_array() {
                for file in files {
                    lines.push(format!("File: {} ({})", field(file, "path"), file["size"]));
                }
            }
            if value["truncated"] == true || value["to"].as_i64() < value["last"].as_i64() {
                lines.push(format!(
                    "More parts remain. Continue with from {} or narrow the range.",
                    value["to"].as_i64().unwrap_or_default() + 1
                ));
            }
            lines.join("\n")
        }
        "search_shared" => {
            let mut lines = Vec::new();
            let referenced = value["referenced_by"].as_str();
            if let Some(source) = referenced {
                lines.push(format!("Referenced by {}", line_text(source)));
            }
            let related = value["related_to"].as_str();
            if let Some(source) = related {
                lines.push(format!("Related to {}", line_text(source)));
                let state = value["meaning"]["state"].as_str().unwrap_or("unavailable");
                if state != "ready" {
                    lines.push(format!("Meaning: {}", line_text(state)));
                }
            }
            if let Some(notice) = value["also_named_so"].as_str() {
                lines.push(line_text(notice));
            }
            if let Some(hits) = value["hits"].as_array() {
                for hit in hits {
                    let count = hit["match_count"].as_u64().map_or_else(
                        || "unknown".to_owned(),
                        |count| {
                            if hit["match_count_partial"] == true {
                                format!("at least {count}")
                            } else {
                                count.to_string()
                            }
                        },
                    );
                    let mut line = if referenced.is_some() {
                        format!(
                            "{} · {} · mentions: {}{}",
                            line_field(hit, "path"),
                            line_field(hit, "kind"),
                            hit["mentions"].as_u64().unwrap_or_default(),
                            if hit["mentions_more"] == true {
                                " or more"
                            } else {
                                ""
                            }
                        )
                    } else if related.is_some() {
                        format!("{} · {}", line_field(hit, "path"), line_field(hit, "kind"))
                    } else {
                        format!(
                            "{} · {} · matches: {count}",
                            line_field(hit, "path"),
                            line_field(hit, "kind")
                        )
                    };
                    // The lines around the first mentions, each cut to one quoted line.
                    if let Some(mentions) = hit["lines"].as_array() {
                        for mention in mentions {
                            if let Some(text) =
                                mention["text"].as_str().filter(|text| !text.is_empty())
                            {
                                line.push_str(&format!(
                                    " · {}: {}",
                                    line_field(mention, "in"),
                                    Json::String(supplied(text))
                                ));
                            }
                        }
                    }
                    if let Some(by) = hit["related_by"].as_array() {
                        line.push_str(&format!(
                            " · related by: {}",
                            by.iter()
                                .filter_map(Json::as_str)
                                .map(line_text)
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    // Where results by meaning are joined, each entry says what found it.
                    if let Some(found_by) = hit["found_by"]
                        .as_array()
                        .filter(|found_by| !found_by.is_empty())
                    {
                        line.push_str(&format!(
                            " · found by: {}",
                            found_by
                                .iter()
                                .filter_map(Json::as_str)
                                .map(line_text)
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    if let Some(positions) = hit["positions"]
                        .as_array()
                        .filter(|positions| !positions.is_empty())
                    {
                        line.push_str(&format!(
                            " · positions: {}",
                            positions
                                .iter()
                                .map(|position| line_text(&position.to_string()))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    if snippets {
                        if let Some(samples) = hit["snippets"].as_array() {
                            for sample in samples {
                                if let Some(text) = sample["snippet"].as_str() {
                                    let position = sample["position"]
                                        .as_i64()
                                        .map_or_else(String::new, |at| format!(" {at}"));
                                    line.push_str(&format!(
                                        " · snippet{position}: {}",
                                        Json::String(supplied(text))
                                    ));
                                }
                            }
                        } else if let Some(text) = hit["snippet"].as_str() {
                            line.push_str(&format!(" · snippet: {}", Json::String(supplied(text))));
                        }
                    }
                    if ids && let Some(id) = hit.get("id") {
                        line.push_str(&format!(" · id {id}"));
                    }
                    lines.push(line);
                }
            }
            if let Some(skipped) = value["skipped"].as_array() {
                for entry in skipped {
                    lines.push(format!(
                        "Skipped {}: {}",
                        field(entry, "path"),
                        field(entry, "reason")
                    ));
                }
            }
            let seen = &value["searched"];
            lines.push(format!(
                "Searched {} entries, {} files, {} bytes.",
                seen["entries"], seen["files"], seen["bytes"]
            ));
            if let Some(cursor) = value["next_cursor"].as_str() {
                lines.push(next_search(cursor));
            } else if value["truncated"] == true {
                lines.push("More matches may remain. Narrow with path, kinds or names.".to_owned());
            }
            lines.join("\n")
        }
        _ => String::new(),
    }
}

fn next_search(cursor: &str) -> String {
    format!(
        "Continue search_shared with cursor {} and the same search options.",
        cursor_word(cursor)
    )
}

fn search_cut(arguments: &Json, last: Option<&Json>) -> String {
    if let Some(cursor) = last.and_then(|entry| entry["cursor"].as_str()) {
        return format!("Answer cut to fit. {}", next_search(cursor));
    }
    if arguments["related_to"].is_string() {
        return "Answer cut to fit. Repeat this related page with smaller max_results or larger max_bytes; unshown entries have not been skipped.".to_owned();
    }
    let start = arguments["cursor"]
        .as_str()
        .map_or_else(String::new, |cursor| {
            format!(" with starting cursor {}", cursor_word(cursor))
        });
    format!(
        "Answer cut to fit. Repeat this same search page{start} with smaller max_results or larger max_bytes (at most 40000); keep the query and search options. Unshown entries have not been skipped."
    )
}

/// Every hit occupies one physical line, including requested, JSON-quoted snippets. Cut only between hits
/// and continue after the last one shown, retaining the producer's current bounded scan window.
fn search_text(value: &Json, arguments: &Json, ids: bool, budget: usize) -> String {
    let text = reading_text("search_shared", value, ids, arguments["snippets"] == true);
    if text.len() <= budget {
        return text;
    }
    if value.is_string() {
        return bounded_lines(&text, budget, &search_cut(arguments, None));
    }
    let hits = value["hits"].as_array().map_or(&[][..], Vec::as_slice);
    if !hits.is_empty() {
        let related_header =
            usize::from(value["related_to"].is_string() || value["referenced_by"].is_string());
        let meaning_header =
            usize::from(value["related_to"].is_string() && value["meaning"]["state"] != "ready");
        let notice =
            usize::from(value["also_named_so"].is_string()) + related_header + meaning_header;
        let lines: Vec<_> = text.lines().collect();
        let mut lengths = vec![0];
        for line in lines.iter().take(notice + hits.len()) {
            lengths.push(lengths.last().copied().unwrap_or_default() + line.len() + 1);
        }
        for shown in (0..=hits.len()).rev() {
            let last = shown.checked_sub(1).and_then(|at| hits.get(at));
            let continuation = search_cut(arguments, last);
            if let Some(length) = lengths.get(notice + shown)
                && *length + continuation.len() <= budget
            {
                let mut output = lines[..notice + shown].join("\n");
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str(&continuation);
                return output;
            }
        }
    }
    let explanation = "The search header or continuation does not fit. Repeat the same page with a larger max_bytes (at most 40000). No entries were shown or skipped.";
    bounded_lines(explanation, budget, explanation)
}

fn bounded_reading(text: &str, budget: usize) -> String {
    bounded_lines(
        text,
        budget,
        "Answer cut to fit. Narrow the path or range, request more max_bytes (at most 40000), or use download_shared for the whole entry.",
    )
}

fn bounded_lines(text: &str, budget: usize, continuation: &str) -> String {
    if text.len() <= budget {
        return text.to_owned();
    }
    if continuation.len() > budget {
        let mut end = budget.min(continuation.len());
        while !continuation.is_char_boundary(end) {
            end -= 1;
        }
        return continuation[..end].to_owned();
    }
    let room = budget.saturating_sub(continuation.len() + 1);
    let mut output = String::new();
    for line in text.lines() {
        if output.len() + line.len() + 1 > room {
            break;
        }
        output.push_str(line);
        output.push('\n');
    }
    output.push_str(continuation);
    output
}

fn bounded_error(code: &str, message: &str, budget: usize) -> String {
    let prefix = format!("{code}: ");
    let rendered = supplied(message);
    if prefix.len() + rendered.len() <= budget {
        return prefix + &rendered;
    }
    let continuation = "The remaining explanation was omitted to fit max_bytes. Request a larger bound (at most 40000) or inspect the affected entry before repeating a change.";
    let room = budget.saturating_sub(prefix.len() + continuation.len() + 1);
    let mut answer = prefix;
    let mut used = 0;
    for line in rendered.lines() {
        if used + line.len() + 1 > room {
            break;
        }
        answer.push_str(line);
        answer.push('\n');
        used += line.len() + 1;
    }
    answer.push_str(continuation);
    answer
}

fn result_of_at(tool: &str, arguments: &Json, outcome: Result<Json, ToolError>, now: Time) -> Json {
    match outcome {
        Ok(value) => {
            if matches!(tool, "read_shared" | "list_shared" | "search_shared") {
                let budget = arguments["max_bytes"]
                    .as_u64()
                    .and_then(|bytes| usize::try_from(bytes).ok())
                    .unwrap_or(ANSWER_BYTES_DEFAULT)
                    .min(ANSWER_BYTES_MAX);
                let ids = arguments["ids"].as_bool().unwrap_or(false);
                let text = match tool {
                    "list_shared" => listing_text(&value, arguments, ids, now, budget),
                    "search_shared" => search_text(&value, arguments, ids, budget),
                    _ => bounded_reading(&reading_text(tool, &value, ids, false), budget),
                };
                json!({"content":[{"type":"text","text":text}],"isError":false})
            } else {
                match orch_agent_contract::tool_answers::bounded(tool, arguments, &value) {
                    Ok(shown) => json!({"content":[],"structuredContent":shown,"isError":false}),
                    Err(message) => result_of_at(
                        tool,
                        arguments,
                        Err(ToolError {
                            code: "too_large".to_owned(),
                            message: message.to_owned(),
                        }),
                        now,
                    ),
                }
            }
        }
        Err(refusal) => json!({
            "content": [{ "type": "text", "text": bounded_error(&refusal.code, &refusal.message, orch_agent_contract::tool_answers::max_bytes(arguments)) }],
            "isError": true,
        }),
    }
}

impl OrchEndpoint {
    /// The answer to one message of a client; none for a notification.
    async fn answer(
        &self,
        session: &SessionBinding,
        source: Option<crate::ledger::ToolCallSource>,
        message: Json,
    ) -> Option<Json> {
        let Some(method) = message.get("method").and_then(Json::as_str) else {
            // The client's answer to something it was asked, or no message at all.
            return if message.get("result").is_some() || message.get("error").is_some() {
                None
            } else {
                Some(failure(
                    &Json::Null,
                    INVALID_REQUEST,
                    "that is no JSON-RPC message",
                ))
            };
        };
        let id = message.get("id").cloned()?;
        let params = message.get("params").cloned().unwrap_or(Json::Null);
        Some(match method {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Json::as_str);
                let version = asked
                    .filter(|asked| PROTOCOL_VERSIONS.contains(asked))
                    .unwrap_or(PROTOCOL_VERSIONS[0]);
                json!({ "jsonrpc": "2.0", "id": id, "result": {
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": ORCH_SERVER, "version": env!("CARGO_PKG_VERSION") },
                } })
            }
            "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
            "tools/list" => {
                let (tools, descriptions, schemas) = self.listed_of(session);
                json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": definitions_with_schemas(&tools, &descriptions, &schemas) } })
            }
            "tools/call" => self.call(session, source, &id, &params).await,
            other => failure(
                &id,
                METHOD_NOT_FOUND,
                &format!("{other} is not served here"),
            ),
        })
    }

    /// The tools the session was opened with, and the descriptions its place set for them.
    fn listed_of(
        &self,
        session: &SessionBinding,
    ) -> (
        Vec<OrchTool>,
        std::collections::BTreeMap<String, String>,
        std::collections::BTreeMap<String, Json>,
    ) {
        self.hosting
            .services()
            .loopback
            .reach(&session.session_key)
            .map(|reach| {
                (
                    reach.orch_tools,
                    reach.orch_tool_descriptions,
                    reach.orch_tool_schemas,
                )
            })
            .unwrap_or_default()
    }

    /// The tools the session was opened with.
    fn tools_of(&self, session: &SessionBinding) -> Vec<OrchTool> {
        self.hosting
            .services()
            .loopback
            .reach(&session.session_key)
            .map(|reach| reach.orch_tools)
            .unwrap_or_default()
    }

    fn result_of(&self, tool: &str, arguments: &Json, outcome: Result<Json, ToolError>) -> Json {
        result_of_at(
            tool,
            arguments,
            outcome,
            self.hosting.services().clock.time(),
        )
    }

    fn native_source(&self, session: &SessionBinding) -> Option<crate::ledger::ToolCallSource> {
        self.hosting
            .process_of(session.thread_id)
            .ok()
            .and_then(|process| process.orch_tool_source(session))
    }

    /// Carries one call to the system and waits for its answer.
    async fn call(
        &self,
        session: &SessionBinding,
        received: Option<crate::ledger::ToolCallSource>,
        id: &Json,
        params: &Json,
    ) -> Json {
        let name = params
            .get("name")
            .and_then(Json::as_str)
            .unwrap_or_default();
        let Some(tool) = serde_json::from_value::<OrchTool>(Json::String(name.to_owned()))
            .ok()
            .filter(|tool| self.tools_of(session).contains(tool))
        else {
            return failure(id, INVALID_PARAMS, &format!("unknown tool: {name}"));
        };
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if arguments.get("max_bytes").is_some_and(|bytes| {
            !bytes
                .as_u64()
                .and_then(|bytes| usize::try_from(bytes).ok())
                .is_some_and(|bytes| (ANSWER_BYTES_MIN..=ANSWER_BYTES_MAX).contains(&bytes))
        }) {
            return json!({"jsonrpc":"2.0","id":id,"result":self.result_of(name,&arguments,Err(ToolError {code:"invalid".to_owned(),message:"max_bytes is an integer from 1000 to 40000; no call was made.".to_owned()}))});
        }
        let thread = session.thread_id;
        let current = self.native_source(session);
        let Some(source) = received.filter(|source| current == Some(*source)) else {
            return json!({ "jsonrpc": "2.0", "id": id, "result": self.result_of(name, &arguments, Err(ToolError {
                code: if received.is_some() {"changed"} else {"invalid"}.to_owned(),
                message: if received.is_some() {
                    "The native turn that admitted this request changed or ended before it could be submitted. Nothing new was submitted."
                } else {
                    "No turn of yours is running for this listener’s native session, so there is nothing to answer this call for."
                }.to_owned(),
            })) });
        };
        let wait = wait_for(tool, self.wait);
        let calls = self.hosting.orch_calls();
        // The same call as one under way waits for its answer; the same as one whose answer came late is answered
        // with it; the same as one whose answer did not come is carried again under its reference.
        let (key, begun, answer) = calls.begin(thread, source, name, &arguments, wait);
        let (call_ref, carry) = match begun {
            Begun::Answered(outcome) => {
                return json!({ "jsonrpc": "2.0", "id": id, "result": self.result_of(name, &arguments, outcome) });
            }
            Begun::Attached(call_ref) => (call_ref, false),
            Begun::Again(call_ref) | Begun::New(call_ref) => (call_ref, true),
        };
        let Some(answer) = answer else {
            return json!({ "jsonrpc": "2.0", "id": id, "result": self.result_of(name, &arguments,
                Err(ToolError {code:"unavailable".into(),message:"The connector could not register this call's answer. Nothing new was submitted.".into()})) });
        };
        // begin registers the exact-source waiter before exposing the reference to any answer.
        if !carry {
            let outcome = match tokio::time::timeout(wait, answer).await {
                Ok(Ok(outcome)) => outcome,
                Ok(Err(_)) | Err(_) => {
                    calls.timed_out(&key, &call_ref);
                    Err(unreachable_error(name))
                }
            };
            return json!({ "jsonrpc": "2.0", "id": id, "result": self.result_of(name, &arguments, outcome) });
        }
        let published = self
            .hosting
            .services()
            .publisher
            .publish(
                Stream::Session {
                    thread_id: thread,
                    segment_id: session.segment_id,
                },
                Event::OrchToolCall(OrchToolCall {
                    turn_id: source.turn_id,
                    call_ref: call_ref.clone(),
                    attempt: Some(orch_types::new_id()),
                    tool,
                    arguments: arguments.clone(),
                }),
            )
            .await;
        if published.is_err() {
            calls.forget(&key, &call_ref);
            return json!({ "jsonrpc": "2.0", "id": id, "result": self.result_of(name, &arguments, Err(unreachable_error(name))) });
        }
        let outcome = match tokio::time::timeout(wait, answer).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) | Err(_) => {
                calls.timed_out(&key, &call_ref);
                Err(unreachable_error(name))
            }
        };
        json!({ "jsonrpc": "2.0", "id": id, "result": self.result_of(name, &arguments, outcome) })
    }
}

fn unreachable_error(tool: &str) -> ToolError {
    ToolError {
        code: "unavailable".to_owned(),
        message: if matches!(tool, "list_shared" | "read_shared" | "search_shared") {
            UNREACHABLE
        } else {
            TIMEOUT_CHANGE
        }
        .to_owned(),
    }
}

#[async_trait]
impl Endpoint for OrchEndpoint {
    async fn serve(
        &self,
        request: Request<Incoming>,
        session: Arc<SessionBinding>,
        _route: orch_agent_contract::loopback::Route,
    ) -> Response<Body> {
        match *request.method() {
            Method::POST => {}
            // There is no session of the protocol's own to end, and no stream of the server's to open.
            Method::DELETE => return text(StatusCode::OK, ""),
            _ => {
                let mut response = text(
                    StatusCode::METHOD_NOT_ALLOWED,
                    "this server is asked with POST",
                );
                response.headers_mut().insert(
                    hyper::header::ALLOW,
                    hyper::header::HeaderValue::from_static("POST, DELETE"),
                );
                return response;
            }
        }
        // Capture before the first body wait. Delayed HTTP bytes or later messages in
        // one batch cannot borrow a newly running turn on an otherwise unchanged URL.
        let source = self.native_source(&session);
        let body = match Limited::new(request.into_body(), REQUEST_MAX_BYTES)
            .collect()
            .await
        {
            Ok(collected) => collected.to_bytes(),
            Err(_) => return text(StatusCode::PAYLOAD_TOO_LARGE, "the request is too large"),
        };
        let message: Json = match serde_json::from_slice(&body) {
            Ok(message) => message,
            Err(_) => {
                return reply(
                    StatusCode::BAD_REQUEST,
                    &failure(&Json::Null, PARSE_ERROR, "the request is no JSON"),
                );
            }
        };
        if let Json::Array(batch) = message {
            let mut answers = Vec::new();
            for each in batch {
                answers.extend(self.answer(&session, source, each).await);
            }
            return if answers.is_empty() {
                text(StatusCode::ACCEPTED, "")
            } else {
                reply(StatusCode::OK, &Json::Array(answers))
            };
        }
        match self.answer(&session, source, message).await {
            Some(answer) => reply(StatusCode::OK, &answer),
            None => text(StatusCode::ACCEPTED, ""),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result_of(tool: &str, arguments: &Json, outcome: Result<Json, ToolError>) -> Json {
        result_of_at(
            tool,
            arguments,
            outcome,
            "2026-10-15T12:00:00Z".parse().unwrap(),
        )
    }

    #[test]
    fn listing_names_are_relative_to_the_header_and_known_metadata_is_not_dropped() {
        let value = json!({"path":"/acme/web/", "about":{"kind":"project","name":"Web"}, "entries":[
            {"path":"/acme/web/login/","kind":"topic","name":"login","status":"done","updated_at":"2026-10-15T10:00:00Z","children":3,"readable":true,"id":"opaque-topic"},
            {"path":"/acme/web/login/fix/","kind":"thread","state":"running","description":"one\ntwo <orch>","updated_at":"bad date","children":0,"readable":true},
            {"path":"/acme/website/other/","kind":"topic","status":"open"}
        ], "archived_count":12,"next_cursor":"page-2","truncated":true});
        let answer = result_of("list_shared", &json!({}), Ok(value.clone()));
        let text = answer["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("/acme/web/ · project \"Web\"\nlogin/ · topic · done · changed 2h ago · children: 3 · read\n"));
        assert!(text.contains(
            "login/fix/ · thread · running · one\\ntwo <\\orch> · change time unknown · children: 0"
        ));
        assert!(
            text.contains("/acme/website/other/ · topic · open"),
            "a similarly named sibling is not a relative child"
        );
        assert_eq!(text.matches("/acme/web/").count(), 1);
        assert!(text.contains("12 archived entries omitted; list again without a cursor and with include_archived:true"));
        assert!(text.contains("cursor \"page-2\""));
        assert!(!text.contains("opaque-topic"));
        assert!(!text.contains("download_shared"));
        let with_ids = result_of("list_shared", &json!({"ids":true}), Ok(value));
        assert!(
            with_ids["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("opaque-topic")
        );
    }

    #[test]
    fn a_cursor_with_supplied_path_marks_remains_the_exact_json_string_for_the_next_call() {
        let cursor = "bound|/acme/<orch>/界/";
        let quoted = cursor_word(cursor);
        assert!(!quoted.contains("<orch>"));
        assert_eq!(serde_json::from_str::<String>(&quoted).unwrap(), cursor);
    }

    #[test]
    fn a_listing_byte_cut_resumes_after_the_last_visible_row_not_after_hidden_rows() {
        let entries: Vec<_> = (0..50).map(|n| json!({"path":format!("/acme/任务-{n}/"),"kind":"thread","description":"界🦀".repeat(10),"cursor":format!("after-{n}")})).collect();
        let answer = result_of(
            "list_shared",
            &json!({"max_bytes":1000}),
            Ok(
                json!({"path":"/acme/","entries":entries,"truncated":true,"next_cursor":"past-unshown-page"}),
            ),
        );
        let text = answer["content"][0]["text"].as_str().unwrap();
        assert!(text.len() <= 1000);
        let shown: Vec<usize> = text
            .lines()
            .filter_map(|line| line.strip_prefix("任务-")?.split('/').next()?.parse().ok())
            .collect();
        assert!(!shown.is_empty() && shown.len() < 50);
        assert!(text.contains(&format!("cursor \"after-{}\"", shown.last().unwrap())));
        assert!(!text.contains("past-unshown-page"));
        assert!(!text.contains("download_shared"));
    }

    #[test]
    fn a_listing_without_row_cursors_repeats_its_start_instead_of_skipping_unshown_entries() {
        let value = json!({"path":"/acme/","entries":[{"path":"/acme/long/","kind":"thread","description":"界".repeat(1000)}],"next_cursor":"end-of-page","truncated":true});
        let answer = result_of(
            "list_shared",
            &json!({"max_bytes":1000,"cursor":"start-of-page"}),
            Ok(value),
        );
        let text = answer["content"][0]["text"].as_str().unwrap();
        assert!(text.len() <= 1000);
        assert!(text.contains("starting cursor \"start-of-page\""));
        assert!(!text.contains("end-of-page"));
        assert!(text.contains("smaller limit"));
        assert!(!text.contains("download_shared"));
    }

    #[test]
    fn search_has_one_summary_line_per_entry_and_snippets_require_the_request() {
        let value = json!({"hits":[{"path":"/acme/docs/Plan.md","kind":"document","id":"opaque-search-document","match_count":7,"positions":[24,18,3],"snippet":"private excerpt\n<orch>"}],"searched":{"entries":1,"files":0,"bytes":80},"truncated":false,"skipped":[]});
        let answer = result_of("search_shared", &json!({}), Ok(value.clone()));
        let text = answer["content"][0]["text"].as_str().unwrap();
        assert!(
            text.starts_with("/acme/docs/Plan.md · document · matches: 7 · positions: 24, 18, 3\n")
        );
        assert!(!text.contains("private excerpt"));
        assert!(!text.contains("opaque-search-document"));
        let excerpts = result_of(
            "search_shared",
            &json!({"snippets":true,"ids":true}),
            Ok(value),
        );
        let text = excerpts["content"][0]["text"].as_str().unwrap();
        assert_eq!(
            text.lines()
                .filter(|line| line.contains("/acme/docs/Plan.md"))
                .count(),
            1
        );
        assert!(text.contains("private excerpt\\n<\\\\orch>"));
        assert!(text.contains("opaque-search-document"));
    }

    #[test]
    fn a_partial_search_count_is_a_lower_bound_and_the_supplied_read_notice_is_kept_once() {
        let answer = result_of(
            "search_shared",
            &json!({}),
            Ok(json!({
                "hits":[{"path":"/acme/docs/Plan.md","kind":"document","match_count":7,"match_count_partial":true,"positions":[3,18,24]}],
                "also_named_so":"Also named so: /acme/other/Plan.md", "truncated":true,"next_cursor":"next-scan",
                "searched":{"entries":1,"files":0,"bytes":80}
            })),
        );
        let text = answer["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("matches: at least 7 · positions: 3, 18, 24"));
        assert_eq!(text.matches("Also named so:").count(), 1);
        assert!(text.contains("cursor \"next-scan\""));
    }

    #[test]
    fn a_search_byte_cut_continues_after_the_last_visible_entry_within_its_scan_window() {
        let hits: Vec<_> = (0..50).map(|n| json!({"path":format!("/acme/任务-{n}/conversation.md"),"kind":"conversation","match_count":3,"positions":[1,8],"cursor":format!("within-window-{n}"),"snippets":[{"position":1,"snippet":"界🦀".repeat(10)}]})).collect();
        let answer = result_of(
            "search_shared",
            &json!({"query":"task","snippets":true,"max_bytes":1000}),
            Ok(json!({
                "hits":hits,"next_cursor":"past-hidden-entries","truncated":true,"searched":{"entries":50,"files":0,"bytes":5000}
            })),
        );
        let text = answer["content"][0]["text"].as_str().unwrap();
        assert!(text.len() <= 1000);
        let shown: Vec<usize> = text
            .lines()
            .filter_map(|line| {
                line.strip_prefix("/acme/任务-")?
                    .split('/')
                    .next()?
                    .parse()
                    .ok()
            })
            .collect();
        assert!(!shown.is_empty() && shown.len() < 50);
        assert!(text.contains(&format!(
            "cursor \"within-window-{}\"",
            shown.last().unwrap()
        )));
        assert!(!text.contains("past-hidden-entries"));
        assert!(!text.contains("download_shared"));
    }

    #[test]
    fn a_referenced_by_byte_cut_continues_after_the_last_source_actually_shown() {
        let hits: Vec<_> = (0..50)
            .map(|n| {
                json!({
                    "path":format!("/acme/Source-{n}.md"),"kind":"document","mentions":2,
                    "lines":[{"in":"text","text":"Decision: see [Plan] for the dates."}],
                    "cursor":format!("after-source-{n}")
                })
            })
            .collect();
        let answer = result_of(
            "search_shared",
            &json!({"referenced_by":"/acme/Plan.md","max_bytes":1000}),
            Ok(json!({"referenced_by":"/acme/Plan.md","hits":hits,
                "next_cursor":"past-unshown-sources","truncated":true,
                "searched":{"entries":50,"files":0,"bytes":0}})),
        );
        let text = answer["content"][0]["text"].as_str().unwrap();
        assert!(text.len() <= 1000);
        let shown: Vec<usize> = text
            .lines()
            .filter_map(|line| {
                line.strip_prefix("/acme/Source-")?
                    .split('.')
                    .next()?
                    .parse()
                    .ok()
            })
            .collect();
        assert!(!shown.is_empty() && shown.len() < 50);
        assert!(text.contains(&format!(
            "cursor \"after-source-{}\"",
            shown.last().unwrap()
        )));
        assert!(!text.contains("past-unshown-sources"));
    }

    #[test]
    fn a_search_without_row_cursors_repeats_its_start_instead_of_skipping_hidden_entries() {
        let answer = result_of(
            "search_shared",
            &json!({"query":"task","cursor":"scan-start","snippets":true,"max_bytes":1000}),
            Ok(json!({
                "hits":[{"path":"/acme/long.md","kind":"document","match_count":1,"positions":[1],"snippet":"界".repeat(1000)}],
                "next_cursor":"scan-end","truncated":true,"searched":{"entries":1,"files":0,"bytes":3000}
            })),
        );
        let text = answer["content"][0]["text"].as_str().unwrap();
        assert!(text.len() <= 1000);
        assert!(text.contains("starting cursor \"scan-start\""));
        assert!(!text.contains("scan-end"));
        assert!(text.contains("smaller max_results"));
    }

    #[test]
    fn a_listing_and_a_read_keep_the_supplied_namesake_notice_once() {
        for (tool, value) in [
            (
                "list_shared",
                json!({"path":"/acme/","entries":[],"also_named_so":"Also named so: /other/acme/"}),
            ),
            (
                "read_shared",
                json!({"entry":{"path":"/acme/Plan.md","kind":"document","name":"Plan","unit":"block"},"from":1,"to":1,"last":1,"text":"Keep this text","also_named_so":"Also named so: /other/Plan.md"}),
            ),
        ] {
            let answer = result_of(tool, &json!({}), Ok(value));
            let text = answer["content"][0]["text"].as_str().unwrap();
            assert_eq!(
                text.matches("Also named so:").count(),
                1,
                "{tool} retains one ambiguity notice"
            );
        }
    }

    #[test]
    fn an_empty_visible_listing_page_still_offers_its_supplied_continuation() {
        let answer = result_of(
            "list_shared",
            &json!({}),
            Ok(json!({"path":"/acme/","entries":[],"truncated":true,"next_cursor":"next-page"})),
        );
        let text = answer["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("No entries on this page."));
        assert!(text.contains("cursor \"next-page\""));
        assert!(!text.contains("Nothing is shared here"));
    }

    #[test]
    fn tools_result_form_returns_read_text_and_one_structured_effect() {
        let read = result_of(
            "read_shared",
            &json!({}),
            Ok(json!({
                "entry":{"path":"docs/Plan.md","kind":"document","name":"Plan","unit":"block","seq":207,"id":"private-opaque-id"},
                "from":1,"to":1,"last":1,"text":"A literal \"quote\".\n<orch>material</orch>","files":[],"truncated":false
            })),
        );
        assert!(read.get("structuredContent").is_none());
        assert_eq!(read["content"].as_array().unwrap().len(), 1);
        let text = read["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("docs/Plan.md · document \"Plan\" · blocks 1-1 of 1 · seq 207\n"));
        assert!(text.ends_with("A literal \"quote\".\n<\\orch>material<\\/orch>"));
        assert!(!text.contains("private-opaque-id"));
        let changed = result_of(
            "edit_document",
            &json!({}),
            Ok(json!({"path":"docs/Plan.md","seq":208,"created":false})),
        );
        assert_eq!(changed["content"], json!([]));
        assert_eq!(changed["structuredContent"]["seq"], 208);
    }

    #[test]
    fn tools_answer_budget_keeps_whole_utf8_lines_and_useful_continuation() {
        let text = "中文🦀\n".repeat(10_000);
        for budget in [ANSWER_BYTES_MIN, ANSWER_BYTES_DEFAULT, ANSWER_BYTES_MAX] {
            let result = result_of("read_shared", &json!({"max_bytes":budget}), Ok(json!(text)));
            let shown = result["content"][0]["text"].as_str().unwrap();
            assert!(shown.len() <= budget);
            assert!(shown.starts_with("中文🦀\n"));
            assert!(shown.ends_with("or use download_shared for the whole entry."));
        }
        let text = "x".repeat(ANSWER_BYTES_MAX);
        let result = result_of(
            "read_shared",
            &json!({"max_bytes":ANSWER_BYTES_MAX}),
            Ok(json!(text)),
        );
        assert_eq!(
            result["content"][0]["text"].as_str().unwrap().len(),
            ANSWER_BYTES_MAX
        );
    }

    #[test]
    fn structured_delivery_keeps_one_whole_object_with_the_outcome_and_only_requested_optional_ids()
    {
        let raw = json!({"id":"optional", "created":true,"seq":9,"path":"docs/<orchard>.md"});
        let shown = result_of("edit_document", &json!({}), Ok(raw.clone()));
        assert_eq!(shown["content"], json!([]));
        assert_eq!(shown["isError"], false);
        assert!(shown["structuredContent"].get("id").is_none());
        assert_eq!(shown["structuredContent"]["path"], "docs/<\\orchard>.md");
        assert_eq!(shown["structuredContent"]["seq"], 9);
        assert_eq!(raw["path"], "docs/<orchard>.md");
        let with_ids = result_of("edit_document", &json!({"ids":true}), Ok(raw));
        assert_eq!(with_ids["structuredContent"]["id"], "optional");
        let entries: Vec<_> = (0..30)
            .map(|i| json!({"path":format!("docs/{i}{}.md", "界".repeat(40)),"action":"delete"}))
            .collect();
        let done = result_of(
            "manage_shared",
            &json!({"max_bytes":1000}),
            Ok(json!({"done":true,"results":entries})),
        );
        assert_eq!(done["isError"], false);
        assert_eq!(done["structuredContent"]["done"], true);
        assert_eq!(done["structuredContent"]["truncated"], true);
        assert!(done["structuredContent"].to_string().len() <= 1000);
    }

    #[test]
    fn even_a_long_refusal_keeps_its_code_one_copy_and_the_complete_utf8_bound() {
        let message = "界 <orch>\n".repeat(1000);
        let answer = result_of(
            "edit_document",
            &json!({"max_bytes":1000}),
            Err(ToolError {
                code: "changed".to_owned(),
                message,
            }),
        );
        let text = answer["content"][0]["text"].as_str().unwrap();
        assert_eq!(answer["content"].as_array().unwrap().len(), 1);
        assert!(answer.get("structuredContent").is_none());
        assert!(text.starts_with("changed: 界 <\\orch>\n"));
        assert!(text.len() <= 1000);
        assert!(text.ends_with("before repeating a change."));
        assert_eq!(answer["isError"], true);
    }

    #[test]
    fn tools_timeout_names_the_possible_effect_instead_of_promising_a_safe_retry() {
        assert_eq!(unreachable_error("read_shared").message, UNREACHABLE);
        for tool in [
            "download_shared",
            "edit_document",
            "send_message",
            "announce",
        ] {
            let result = result_of(tool, &json!({}), Err(unreachable_error(tool)));
            assert_eq!(
                result["content"][0]["text"],
                format!("unavailable: {TIMEOUT_CHANGE}")
            );
            assert!(result.get("structuredContent").is_none());
        }
    }

    #[test]
    fn the_listing_holds_each_tool_of_the_system_once_with_a_closed_schema_a_title_and_hints() {
        let all = listing();
        let listed: Vec<&str> = all
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        let known: Vec<&str> = orch_agent_contract::events::OrchTool::ALL
            .iter()
            .map(|tool| tool.name())
            .collect();
        assert_eq!(listed, known, "the tools the system has, in its order");
        for tool in all {
            let name = tool["name"].as_str().unwrap();
            assert_eq!(tool["inputSchema"]["additionalProperties"], false, "{name}");
            assert!(
                tool["title"]
                    .as_str()
                    .is_some_and(|title| !title.is_empty()),
                "{name}"
            );
            assert_eq!(
                tool["annotations"]["openWorldHint"],
                name == orch_agent_contract::events::OrchTool::UseBrowser.name(),
                "only the browser tool reaches the open web: {name}"
            );
            assert!(tool["annotations"]["readOnlyHint"].is_boolean(), "{name}");
        }
    }

    #[test]
    fn a_session_is_listed_only_the_tools_it_was_given() {
        let given = definitions_of(&[OrchTool::RequestCapability, OrchTool::Announce]);
        let names: Vec<&str> = given
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            ["announce", "request_capability"],
            "in the order the system lists them"
        );
        assert!(definitions_of(&[]).is_empty());
    }

    #[test]
    fn a_description_a_place_set_is_listed_in_place_of_the_shipped_one_and_nothing_else_changes() {
        let tools = [OrchTool::RequestCapability, OrchTool::Announce];
        let shipped = definitions_of(&tools);
        let own = std::collections::BTreeMap::from([(
            "announce".to_owned(),
            "Tell the people of this topic what changed.".to_owned(),
        )]);
        let listed = definitions_with(&tools, &own);
        assert_eq!(
            listed[0]["description"],
            "Tell the people of this topic what changed."
        );
        assert_eq!(listed[0]["name"], shipped[0]["name"]);
        assert_eq!(listed[0]["inputSchema"], shipped[0]["inputSchema"]);
        assert_eq!(
            listed[1], shipped[1],
            "a tool the place left alone keeps its shipped text"
        );
    }

    #[test]
    fn a_new_session_schema_replaces_the_old_choices_without_changing_the_static_catalogue() {
        let tools = [OrchTool::Announce];
        let stock = definitions_of(&tools);
        let mut schema = stock[0]["inputSchema"].clone();
        schema["properties"]
            .as_object_mut()
            .unwrap()
            .remove("delivery");
        schema["properties"]
            .as_object_mut()
            .unwrap()
            .remove("wake_idle");
        schema["properties"]["audience"]["enum"] = json!(["followers", "topic"]);
        let own = std::collections::BTreeMap::from([("announce".to_owned(), schema.clone())]);
        let narrowed = definitions_with_schemas(&tools, &Default::default(), &own);
        assert_eq!(narrowed[0]["inputSchema"], schema);
        assert!(
            narrowed[0]["inputSchema"]["properties"]
                .get("delivery")
                .is_none()
        );
        let refreshed = definitions_with_schemas(&tools, &Default::default(), &Default::default());
        assert_eq!(
            refreshed, stock,
            "a next setup reads its own schema, not a mutated cached listing"
        );
    }

    #[test]
    fn a_refusal_is_a_result_marked_as_an_error_whose_text_begins_with_its_code() {
        let refusal = result_of(
            "edit_document",
            &json!({}),
            Err(ToolError {
                code: "rate_limited".to_owned(),
                message: "Wait 12 seconds.".to_owned(),
            }),
        );
        assert_eq!(refusal["isError"], true);
        assert_eq!(
            refusal["content"][0]["text"],
            "rate_limited: Wait 12 seconds."
        );
        let done = result_of(
            "request_capability",
            &json!({}),
            Ok(json!({ "status": "granted" })),
        );
        assert_eq!(done["isError"], false);
        assert_eq!(done["structuredContent"]["status"], "granted");
        assert_eq!(done["content"], json!([]));
    }

    #[test]
    fn a_call_made_again_after_its_answer_did_not_come_is_the_same_call_and_is_kept_across_a_restart()
     {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(crate::ledger::Ledger::open(dir.path()).unwrap());
        let calls = OrchCalls::with_ledger(Arc::clone(&ledger));
        let thread = new_id();
        let source = crate::ledger::ToolCallSource {
            segment_id: new_id(),
            turn_id: new_id(),
            native: orch_types::Hash::of(b"synthetic original native source"),
        };
        let arguments = json!({ "text": "hello", "target": { "path": "Team", "kind": "chat" } });
        let (key, begun, _) = calls.begin(thread, source, "send_message", &arguments, RESULT_WAIT);
        let Begun::New(call_ref) = begun else {
            panic!("a new call: {begun:?}")
        };
        // The same call made while it is under way, its arguments written in another order, waits for its answer.
        let (_, attached, _) = calls.begin(
            thread,
            source,
            "send_message",
            &json!({ "target": { "kind": "chat", "path": "Team" }, "text": "hello" }),
            RESULT_WAIT,
        );
        assert_eq!(attached, Begun::Attached(call_ref.clone()));
        drop(calls.expect(thread, &call_ref));
        calls.timed_out(&key, &call_ref);
        // After a restart it is still kept: made again, it is carried again under its reference.
        let restarted =
            OrchCalls::with_ledger(Arc::new(crate::ledger::Ledger::open(dir.path()).unwrap()));
        let (again, begun, _) =
            restarted.begin(thread, source, "send_message", &arguments, RESULT_WAIT);
        assert_eq!(begun, Begun::Again(call_ref.clone()));
        drop(restarted.expect(thread, &call_ref));
        restarted.timed_out(&again, &call_ref);
        // Its answer comes when nobody waits any more: the same call made again is answered with it.
        assert!(restarted.answer(thread, &call_ref, Ok(json!({ "number": 7 }))));
        let (_, begun, _) =
            restarted.begin(thread, source, "send_message", &arguments, RESULT_WAIT);
        assert_eq!(begun, Begun::Answered(Ok(json!({ "number": 7 }))));
        // Other arguments, or another thread, make another call.
        assert!(matches!(
            restarted
                .begin(
                    thread,
                    source,
                    "send_message",
                    &json!({ "text": "other" }),
                    RESULT_WAIT
                )
                .1,
            Begun::New(_)
        ));
        assert!(matches!(
            restarted
                .begin(new_id(), source, "send_message", &arguments, RESULT_WAIT)
                .1,
            Begun::New(_)
        ));
    }

    #[test]
    fn only_a_call_of_this_server_is_answered_here() {
        let calls = OrchCalls::default();
        let thread = new_id();
        assert!(
            !calls.answer(thread, "call_7", Ok(json!({}))),
            "an adapter's call is the adapter's"
        );
        let waiting = calls.expect(thread, "mcp:1");
        assert!(calls.answer(thread, "mcp:1", Ok(json!({ "ok": true }))));
        assert!(waiting.blocking_recv().is_ok());
        assert!(
            calls.answer(thread, "mcp:1", Ok(json!({}))),
            "late, and still not the adapter's"
        );
    }
    #[tokio::test]
    async fn a_retained_native_source_never_answers_or_attaches_another_turn_lane_or_conversation()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let ledger = Arc::new(crate::ledger::Ledger::open(dir.path())?);
        let calls = OrchCalls::with_ledger(ledger.clone());
        let thread = new_id();
        let source = crate::ledger::ToolCallSource {
            segment_id: new_id(),
            turn_id: new_id(),
            native: orch_types::Hash::of(b"synthetic native A"),
        };
        let arguments = json!({"path":"docs/source.md"});
        let (key, begun, pending) =
            calls.begin(thread, source, "read_shared", &arguments, RESULT_WAIT);
        let Begun::New(original) = begun else {
            return Err("the original source must admit its own call".into());
        };
        drop(pending);
        calls.timed_out(&key, &original);
        let value = json!({"text":"Only the original source answer"});
        assert!(calls.answer(thread, &original, Ok(value.clone())));
        let recorded = ledger.retained_calls();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].source, Some(source));
        drop(calls);
        let restarted = OrchCalls::with_ledger(Arc::new(crate::ledger::Ledger::open(dir.path())?));
        assert_eq!(
            restarted
                .begin(thread, source, "read_shared", &arguments, RESULT_WAIT)
                .1,
            Begun::Answered(Ok(value.clone()))
        );
        for different in [
            crate::ledger::ToolCallSource {
                turn_id: new_id(),
                ..source
            },
            crate::ledger::ToolCallSource {
                segment_id: new_id(),
                ..source
            },
            crate::ledger::ToolCallSource {
                native: orch_types::Hash::of(b"synthetic replacement native"),
                ..source
            },
        ] {
            let (_, new, pending) =
                restarted.begin(thread, different, "read_shared", &arguments, RESULT_WAIT);
            let Begun::New(reference) = new else {
                return Err(
                    "another actual source cannot inherit an answer or pending call".into(),
                );
            };
            assert_ne!(reference, original);
            let (_, attached, other) =
                restarted.begin(thread, different, "read_shared", &arguments, RESULT_WAIT);
            assert_eq!(attached, Begun::Attached(reference.clone()));
            assert!(restarted.answer(
                thread,
                &original,
                Ok(json!({"text":"A contradictory duplicate cannot replace the original answer"}))
            ));
            let mut pending = pending.ok_or("new source waiter absent")?;
            assert_eq!(
                pending.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            );
            let own = json!({"text":"Only this source's fresh answer"});
            assert!(restarted.answer(thread, &reference, Ok(own.clone())));
            assert_eq!(pending.await?, Ok(own.clone()));
            assert_eq!(
                other.ok_or("attached source waiter absent")?.await?,
                Ok(own)
            );
        }
        assert_eq!(
            restarted
                .begin(thread, source, "read_shared", &arguments, RESULT_WAIT)
                .1,
            Begun::Answered(Ok(value))
        );
        Ok(())
    }

    #[test]
    fn legacy_unscoped_records_refuse_without_reassigning_a_source_or_extending_their_original_bound()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let ledger = Arc::new(crate::ledger::Ledger::open(dir.path())?);
        let thread = new_id();
        let arguments =
            json!({"command":"rewrite","path":"docs/source.md","seq":7,"markdown":"synthetic"});
        let until = epoch_now() + i64::try_from(RETAINED_FOR.as_secs())?;
        let old = json!({"thread_id":thread,"tool":"edit_document","arguments_hash":arguments_hash(&arguments),
            "call_ref":format!("{CALL_PREFIX}{}",new_id()),"until":until});
        let retained: crate::ledger::RetainedCall = serde_json::from_value(old.clone())?;
        assert_eq!(retained.source, None);
        assert!(retained.timed_out);
        assert_eq!(
            serde_json::to_value(&retained)?,
            old,
            "an old missing-source record retains its exact omission semantics"
        );
        ledger.try_keep_retained_calls(&[retained])?;
        let calls = OrchCalls::with_ledger(Arc::new(crate::ledger::Ledger::open(dir.path())?));
        let source = crate::ledger::ToolCallSource {
            segment_id: new_id(),
            turn_id: new_id(),
            native: orch_types::Hash::of(b"synthetic current native"),
        };
        for turn in [
            source,
            crate::ledger::ToolCallSource {
                turn_id: new_id(),
                ..source
            },
        ] {
            let (_, begun, pending) =
                calls.begin(thread, turn, "edit_document", &arguments, RESULT_WAIT);
            let Begun::Answered(Err(error)) = begun else {
                return Err(
                    "an unscoped retained effect cannot be guessed into a current turn".into(),
                );
            };
            assert_eq!(error.code, "source_unconfirmed");
            assert!(pending.is_none());
            assert!(lock(&calls.state).under_way.is_empty());
        }
        let after = crate::ledger::Ledger::open(dir.path())?.retained_calls();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].until, until);
        assert_eq!(after[0].source, None);
        assert_eq!(serde_json::to_value(&after[0])?, old);
        Ok(())
    }

    #[test]
    fn an_unissued_scoped_identity_survives_restart_and_failed_persistence_publishes_nothing()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let ledger = Arc::new(crate::ledger::Ledger::open(dir.path())?);
        let thread = new_id();
        let source = crate::ledger::ToolCallSource {
            segment_id: new_id(),
            turn_id: new_id(),
            native: orch_types::Hash::of(b"synthetic durable native"),
        };
        let arguments =
            json!({"target":{"kind":"chat","path":"Team"},"text":"synthetic exact effect"});
        let calls = OrchCalls::with_ledger(ledger.clone());
        let (_, first, pending) =
            calls.begin(thread, source, "send_message", &arguments, RESULT_WAIT);
        let Begun::New(reference) = first else {
            return Err("initial source identity absent".into());
        };
        let recorded = crate::ledger::Ledger::open(dir.path())?.retained_calls();
        assert_eq!(recorded.len(), 1);
        assert_eq!(
            (&recorded[0].call_ref, recorded[0].source),
            (&reference, Some(source))
        );
        assert!(
            !recorded[0].timed_out,
            "the identity is durable even before a timeout/publish"
        );
        drop(pending);
        drop(calls);
        let restarted = OrchCalls::with_ledger(Arc::new(crate::ledger::Ledger::open(dir.path())?));
        assert_eq!(
            restarted
                .begin(thread, source, "send_message", &arguments, RESULT_WAIT)
                .1,
            Begun::Again(reference)
        );
        let broken = tempfile::tempdir()?;
        let ledger = Arc::new(crate::ledger::Ledger::open(broken.path())?);
        std::fs::create_dir(
            broken
                .path()
                .join(crate::ledger::STATE_DIR)
                .join(crate::ledger::HOSTED_FILE),
        )?;
        let calls = OrchCalls::with_ledger(ledger);
        let (_, begun, pending) =
            calls.begin(thread, source, "send_message", &arguments, RESULT_WAIT);
        let Begun::Answered(Err(error)) = begun else {
            return Err("failed durable identity write must refuse before publication".into());
        };
        assert_eq!(error.code, "unavailable");
        assert!(pending.is_none());
        assert!(lock(&calls.state).under_way.is_empty());
        Ok(())
    }
}
#[test]
fn related_search_keeps_each_enabled_signal_and_explains_missing_meaning_in_text() {
    let value = serde_json::json!({
        "related_to":"/work/docs/Plan.md",
        "meaning":{"state":"off","reason":null},
        "hits":[{"path":"/work/docs/Notes.md","kind":"document","related_by":["place","references"]}],
        "searched":{"entries":1,"files":0,"bytes":0}
    });
    let text = reading_text("search_shared", &value, false, false);
    assert!(text.contains("Related to /work/docs/Plan.md"), "{text}");
    assert!(text.contains("Meaning: off"), "{text}");
    assert!(text.contains("related by: place, references"), "{text}");
    assert!(!text.contains("matches: unknown"), "{text}");
}

#[cfg(test)]
#[test]
fn related_byte_cuts_continue_after_the_last_shown_hit() {
    let first = format!("/notes/{}.md", "a".repeat(90));
    let second = format!("/notes/{}.md", "b".repeat(90));
    let value = serde_json::json!({"related_to":"/docs/source.md","meaning":{"state":"off"},
        "hits":[{"path":first,"kind":"document","related_by":["references"],"cursor":"after-first"},
                {"path":second,"kind":"document","related_by":["references"],"cursor":"after-second"}],
        "searched":{"entries":2,"files":0,"bytes":0},"next_cursor":"after-second"});
    let arguments = serde_json::json!({"related_to":"id:source"});
    let prefix = reading_text("search_shared", &value, false, false);
    // Exactly the two headers, one hit and its continuation fit; a second hit must stay for its cursor.
    let budget = prefix
        .lines()
        .take(3)
        .map(|line| line.len() + 1)
        .sum::<usize>()
        + search_cut(
            &arguments,
            value["hits"].as_array().and_then(|hits| hits.first()),
        )
        .len();
    let text = search_text(&value, &arguments, false, budget);
    assert!(text.contains(&first), "{text}");
    assert!(!text.contains(&second), "{text}");
    assert!(text.contains("after-first"), "{text}");
    assert!(!text.contains("after-second"), "{text}");
    assert!(text.len() <= budget);
}
