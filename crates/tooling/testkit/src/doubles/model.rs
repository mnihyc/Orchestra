//! Scripted Responses upstream and client.
//!
//! The scripted model server stands for an upstream in every automated test of the adapters, the forwarder and
//! the engine. It speaks the Responses format of OpenAI and the Messages format of Anthropic on one port,
//! streams with server-sent events the way the real interfaces do, and answers from a script. [`MockLlm`] starts
//! it in the process of a test on a free loopback port; the program `orch-mock-llm` is the same server for
//! the tests that run outside a process, with the control endpoint below. [`ModelClient`] is a client for
//! the first two formats.
//!
//! # Routes
//!
//! A request is routed by the end of its path, so any base address in front works. Everything below `/_mock/`
//! is the control endpoint. Any other request is answered `404` in the error shape of the format, once its
//! credential has been checked; the format is the Messages format if the request carries `anthropic-version`
//! or its path has `messages` in it, and the Responses format otherwise.
//!
//! | Request | Answer |
//! |---------|--------|
//! | `POST …/responses` | A response of the Responses format, as one document or, when the body says `"stream": true`, as events |
//! | `POST …/responses/compact` | The compaction of the items given: the user's items and a `compaction` item, with `usage` |
//! | `POST …/messages` | A message of the Messages format, as one document or, with `"stream": true`, as events |
//! | `POST …/messages/count_tokens` | `{"input_tokens": n}`, an estimate of one token to four characters of everything the body holds |
//! | `POST …/chat/completions` | A completion of the Chat Completions format, as one document or, when the body says `"stream": true`, as chunks (`reasoning_content`, `content`, `tool_calls`, the finish reason, and a last chunk of use with the cached and the reasoning tokens) ending with `data: [DONE]` |
//! | `GET …/models` | The models of the server, in the shape of the format of the request (the Messages format if it carries `anthropic-version`, the Responses format otherwise). The Messages list pages: 20 models unless `limit` asks for more, at most 1000, and `after_id` names the model after which the page starts |
//!
//! # The credential
//!
//! A server started with a credential refuses a request that does not present it with `401` and the error of
//! its format (`invalid_api_key` for the Responses format, `authentication_error` for the Messages format),
//! before any script is looked at. The credential is presented in one of the ways the server accepts:
//! `bearer` (`Authorization: Bearer <key>`), `x-api-key` and `header:<name>` (the key as the value of that
//! header). A server accepts bearer and `x-api-key` unless it is told otherwise; told `header:api-key` alone,
//! it refuses the right key presented the wrong way. A server without a credential lets everything in. What
//! the check found is in the record of each request ([`RecordedRequest::credential`]).
//!
//! # Choosing the script
//!
//! The script that answers a request to create a response is the first of these that exists:
//!
//! 1. A **directive in the message of the person**: the last message of role `user` that holds text and no tool
//!    result (a message with tool results is a round of tool calls, not the person, even when the harness adds
//!    a line to it). The script is the part of one of that message's text blocks from a line that is exactly
//!    `#!mock` to the end of the block.
//! 2. A script **set ahead** (the queue below) whose conditions the request meets; it answers once, or as many
//!    times as it was given.
//! 3. The **default script**, if one is set; it is read like a script in a message, `then` and all.
//! 4. The plain answer `You said: <the last text block of the message>`.
//!
//! A request in the Responses format that names `previous_response_id` and holds no message of the person
//! continues the script that made that response, in its next round, whichever of the first three it came from.
//! A malformed script in a message is refused with `400` that names its line.
//!
//! # The script language
//!
//! A script is lines. An empty line and a line that begins with `#` are nothing. Every other line is a directive:
//! a word and what follows it. In the text of `say`, `think`, `custom` and `error`, `\n` is a line break, `\t`
//! a tab and `\\` a backslash. A script makes one response for each round of tool calls: `then` ends the
//! response of one round, and what follows it is the response to the request that carries the tool results (the
//! round is counted from the request itself, so a script is as good after a restart). A script that has no
//! response left for a round answers `Done.`. In a script set ahead, each part between `then`s is the
//! response to the next request.
//!
//! | Directive | Effect |
//! |-----------|--------|
//! | `say <text>` | A message of the assistant, in pieces |
//! | `think <text>` | Reasoning: a `reasoning` item with a summary (Responses), a `thinking` block with a signature (Messages) |
//! | `tool <name> [<json object>]` | A call of a function tool; the arguments default to `{}` and are sent in pieces. In Responses, `namespace.name` addresses an advertised namespace. Several `tool` directives are parallel calls |
//! | `custom <name> <text>` | A call of a free-text tool: `custom_tool_call` (Responses); a `tool_use` with `{"input": <text>}` (Messages) |
//! | `chunk <n>` | Characters in one piece of the text that follows: 8 unless said, never inside a character |
//! | `delay <ms>` | Wait before each piece of the text that follows: 0 unless said |
//! | `sleep <ms>` | Wait once, here |
//! | `keepalive` | An SSE comment line (`: keep-alive`), here |
//! | `gateway` | Write the stream the way a gateway in front of the vendors does: the Responses format with no `event:` lines and an empty comment line first, both formats ending with `data: [DONE]` |
//! | `usage <input> <output> [cached=N] [reasoning=N] [cache_write=N] [cost=D]` | The figures of use; `input=`/`output=` also work. Unsaid, input is one token to four characters of the request and output one to four of the response. Cached and written tokens are parts of the input, reasoning is part of the output |
//! | `model <name>` | The model to report instead of the request's |
//! | `stop <reason>` | The stop reason of the Messages format (`end_turn`, `tool_use`, `max_tokens`, `refusal`, …); unsaid it follows the response |
//! | `incomplete [<reason>]` | The response ends cut short by its limit: `response.incomplete` with `incomplete_details.reason` (default `max_output_tokens`), or `stop_reason: max_tokens` |
//! | `fail <kind> [<message>]` | The response fails: `response.failed` (Responses), an `error` event (Messages) |
//! | `stream_error <kind> [<message>]` | An `error` event inside the stream, here; nothing after it |
//! | `error <kind> [retry-after=<s>\|none] [status=<code>] [window=<w>] [resets=<moment>] [plan=<name>] [<message>]` | The whole answer is this HTTP error, and nothing else of the segment is sent. `window`, `resets` and `plan` belong to the kind `plan_limit` |
//! | `cut` | Abort the connection here, leaving the last item open: the client sees the body break |
//! | `cut_after <n>` | Cut the last `say`, `think`, `tool` or `custom` after `n` characters of its text (of the arguments, for a call) and abort the connection there: a stream broken inside a sentence, or inside the arguments of a call |
//! | `truncate` | End the body here cleanly, without the closing events |
//! | `event <name> [<data>]` | An event of a name no client knows, here; the data (`{}` unless given) is written as it stands |
//! | `garbage <text>` | A line of data that is no JSON, here |
//! | `header <name> <value>` | Add this header to the answer, whatever it is |
//! | `stall [<ms>]` | Hold the answer back before a single byte of it, status line included, for this long; without a number until the client gives up (an hour) |
//! | `body <status> <text>` | The whole answer is this text with this status, as a gateway in the way would give a page of its own (`text/html`), and nothing else of the segment is sent |
//! | `then` | The rest is the response to the next round |
//!
//! The kinds of error are `rate_limit` (429, with `Retry-After`, 2 seconds unless said), `overloaded` (503 in
//! the Responses format, 529 in the Messages format, with `Retry-After`), `invalid_request` (400),
//! `authentication` (401), `permission` (403), `not_found` (404), `too_large` (413), `server` (500),
//! `timeout` (504), `billing` (429 `insufficient_quota` / 402), `context_length` (400: a prompt longer than the
//! context, `context_length_exceeded` in the Responses format and "prompt is too long: …" in the Messages
//! format) and `plan_limit` (429, a limit of a subscription's plan, which the vendors say in headers: see
//! below). In `text`, `{{result}}` is the text of the tool results of the round that came before.
//!
//! **A limit of a plan** is `error plan_limit [window=<w>] [resets=<moment>] [plan=<name>]`. The windows are
//! `session` (the few hours; the default), `weekly`, `weekly_opus` and `weekly_sonnet` (one family of models),
//! and `spend` (the month's spending). `resets` is a moment in seconds since the epoch, or `+` and seconds
//! from the moment of the answer; with none the vendor gives no time. The Messages format answers with the
//! `anthropic-ratelimit-unified-*` headers (the status `rejected`, the window that stopped the request as the
//! representative claim, the moment, how full the two windows are); the Responses format answers with an
//! error of the type `usage_limit_reached` (`plan_type`, `resets_at`, `resets_in_seconds`) and the
//! `x-codex-*` headers. Neither carries a `Retry-After`. The harnesses write their sentence ("You've hit your
//! weekly limit · resets …") from these.
//!
//! # The control endpoint
//!
//! JSON, below `/_mock/`, without the credential. A malformed script is `400 {"error": "…line N…"}`.
//!
//! | Request | Effect |
//! |---------|--------|
//! | `GET /_mock/health` | `{"ok": true, "requests": n}` |
//! | `GET /_mock/requests[?since=<seq>]` | The requests received, after `seq` if given: [`RecordedRequest`] |
//! | `DELETE /_mock/requests` | Forgets them (the numbering goes on) |
//! | `POST /_mock/queue` | `{"script": "…", "times": 1, "model": "claude-haiku*", "format": "messages"}`, or `{"scripts": [ … ]}`: sets responses ahead. `model` is a name or a pattern with `*`, `format` is `responses` or `messages`; both are optional conditions |
//! | `GET /_mock/queue`, `DELETE /_mock/queue` | Lists, empties |
//! | `PUT /_mock/default` `{"script": "…"}`, `DELETE /_mock/default` | Sets, clears the default script |
//! | `GET /_mock/models`, `PUT /_mock/models` | Reads, sets the models: `[{"id", "display_name"?, "context_length"?, "pricing"?: {"prompt", "completion", "input_cache_read"?, "input_cache_write"?}}]` |
//! | `PUT /_mock/credential` | `{"credential": "…" or null, "accept": ["bearer", "x-api-key", "header:<name>"]}`: rotates the credential, and optionally the ways |
//! | `POST /_mock/reset` | Forgets requests, queue, default and the numbering |
//!
//! # What it remembers
//!
//! Each request to the model interface is recorded when it arrives and completed as it ends:
//! [`RecordedRequest`] has its method, path, query, headers, body, format, model, whether it streams, the check
//! of its credential, which script answered (`directive`, `queue`, `default`, `echo`, and its round), the status,
//! and how the answer ended (`pending` while it is sent, then `completed`, `error`, `cut`, `truncated` or
//! `client_gone`). Header names are lower case. A header is secret when its name
//! has `auth`, `key`, `token`, `secret`, `password`, `cookie` or `credential` in it, or when the server was told
//! so; a secret, and the credential wherever it occurs, is written as `[secret:` and eight hexadecimal digits of
//! its BLAKE3 hash `]` ([`fingerprint`]), `Bearer` and other schemes kept. So a test can check that the right
//! secret arrived, and a secret is in no record, log or body the server writes.
//!
//! # Determinism
//!
//! Nothing is random and nothing reads a clock: the ids, the moments and the signatures of a response are
//! functions of the number of the request and of the script. Two servers that are sent the same requests
//! answer with the same bytes.

mod chat;
mod client;
mod control;
mod errors;
mod listing;
mod messages;
mod plan;
mod record;
mod request;
mod responses;
mod script;
mod server;
#[cfg(test)]
mod tests;

use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use orch_types::{Clock, SystemClock};
use serde::{Deserialize, Serialize};

pub use client::{ClientError, ClientReply, ClientRequest, ModelClient, OpenReply};
pub use record::{
    CredentialCheck, Outcome, Presentation, RecordedRequest, ScriptSource, Verdict, fingerprint,
};
pub use request::Format;
pub use script::{
    ErrorKind, ErrorSpec, LimitSpec, LimitWindow, MARKER, RawBody, Reset, Script, ScriptError,
};
pub use server::{CONTROL_PREFIX, MAX_REQUEST_BYTES, QueueFilter};

use server::Shared;

/// The moment a server reports unless it is told another: midnight of the day this was written, so that the
/// same script gives the same bytes.
pub const DEFAULT_CREATED_AT: i64 = 1_791_676_800;

/// The model a response is reported for when the request names none.
pub const DEFAULT_MODEL: &str = "mock-large";

/// How long [`MockLlm::settled`] waits for requests to end before it gives what it has.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(5);

/// How often [`MockLlm::settled`] looks.
const SETTLE_POLL: Duration = Duration::from_millis(10);

/// The prices of a model, as a gateway's list states them: US dollars for one token, as text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockPricing {
    /// For one token of input.
    pub prompt: String,
    /// For one token of output.
    pub completion: String,
    /// For one token read from a cache.
    #[serde(default)]
    pub input_cache_read: Option<String>,
    /// For one token written to a cache.
    #[serde(default)]
    pub input_cache_write: Option<String>,
}

/// A model the server lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockModel {
    /// Its routing name.
    pub id: String,
    /// The name to show, if the list gives one.
    #[serde(default)]
    pub display_name: Option<String>,
    /// The size of its context, if the list gives it.
    #[serde(default)]
    pub context_length: Option<i64>,
    /// Its prices, if the list gives them.
    #[serde(default)]
    pub pricing: Option<MockPricing>,
}

impl MockModel {
    /// A model with a routing name and nothing else, as the vendors' own lists have it.
    #[must_use]
    pub fn named(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            display_name: None,
            context_length: None,
            pricing: None,
        }
    }
}

/// How a server is set up.
#[derive(Debug, Clone)]
pub struct MockConfig {
    /// The credential a request must present; `None` lets everything in.
    pub credential: Option<String>,
    /// The ways in which the credential is accepted.
    pub accept: Vec<Presentation>,
    /// Names of headers to treat as secret beside those whose names say so.
    pub secret_headers: Vec<String>,
    /// Values to treat as secret wherever they occur, beside the credential.
    pub secret_values: Vec<String>,
    /// The models the lists give.
    pub models: Vec<MockModel>,
    /// The model reported when a request names none.
    pub default_model: String,
    /// The moment reported, in seconds since the epoch.
    pub created_at: i64,
    /// The text of the default script, if there is one.
    pub default_script: Option<String>,
    /// The clock a limit that ends after so many seconds is dated by (`resets=+N` of a plan limit); nothing
    /// else reads it.
    pub clock: Arc<dyn Clock>,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            credential: None,
            accept: vec![Presentation::Bearer, Presentation::XApiKey],
            secret_headers: Vec::new(),
            secret_values: Vec::new(),
            models: vec![
                MockModel::named("mock-small"),
                MockModel::named(DEFAULT_MODEL),
            ],
            default_model: DEFAULT_MODEL.to_owned(),
            created_at: DEFAULT_CREATED_AT,
            default_script: None,
            clock: Arc::new(SystemClock),
        }
    }
}

impl MockConfig {
    /// The same set-up with a credential to check.
    #[must_use]
    pub fn with_credential(mut self, credential: &str) -> Self {
        self.credential = Some(credential.to_owned());
        self
    }

    /// The same set-up accepting the credential in these ways only.
    #[must_use]
    pub fn accepting(mut self, ways: &[Presentation]) -> Self {
        self.accept = ways.to_vec();
        self
    }

    /// The same set-up with another header treated as secret.
    #[must_use]
    pub fn with_secret_header(mut self, name: &str) -> Self {
        self.secret_headers.push(name.to_owned());
        self
    }

    /// The same set-up with another value treated as secret wherever it occurs.
    #[must_use]
    pub fn with_secret_value(mut self, value: &str) -> Self {
        self.secret_values.push(value.to_owned());
        self
    }

    /// The same set-up with these models.
    #[must_use]
    pub fn with_models(mut self, models: Vec<MockModel>) -> Self {
        self.models = models;
        self
    }

    /// The same set-up with a default script.
    #[must_use]
    pub fn with_default_script(mut self, script: &str) -> Self {
        self.default_script = Some(script.to_owned());
        self
    }
}

/// Why a server could not be started.
#[derive(Debug, thiserror::Error)]
pub enum MockError {
    /// The address could not be listened on.
    #[error("the scripted model server could not listen on {address}: {source}")]
    Listen {
        /// The address.
        address: SocketAddr,
        /// Why not.
        source: io::Error,
    },
}

/// A running scripted model server; it stops when this is dropped.
#[derive(Debug)]
pub struct MockLlm {
    address: SocketAddr,
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
}

impl MockLlm {
    /// Starts a server on a free port of the loopback.
    pub async fn start(config: MockConfig) -> Result<Self, MockError> {
        Self::start_on(SocketAddr::from(([127, 0, 0, 1], 0)), config).await
    }

    /// Starts a server on `address`.
    pub async fn start_on(address: SocketAddr, config: MockConfig) -> Result<Self, MockError> {
        let listen = |source| MockError::Listen { address, source };
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .map_err(listen)?;
        let bound = listener.local_addr().map_err(listen)?;
        let shared = Arc::new(Shared::new(config));
        let app = axum::Router::new()
            .fallback(server::handle)
            .with_state(Arc::clone(&shared));
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self {
            address: bound,
            shared,
            task,
        })
    }

    /// The address it listens on.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.address
    }

    /// Its address as a URL, such as `http://127.0.0.1:41500`.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// The base address of an endpoint in the Responses format: requests go to `<base>/responses`.
    #[must_use]
    pub fn responses_base_url(&self) -> String {
        format!("{}/v1", self.url())
    }

    /// The base address of an endpoint in the Messages format: requests go to `<base>/v1/messages`.
    #[must_use]
    pub fn messages_base_url(&self) -> String {
        self.url()
    }

    /// A client of this server.
    #[must_use]
    pub fn client(&self) -> ModelClient {
        ModelClient::new(&self.url())
    }

    /// What was received so far, in order of arrival.
    #[must_use]
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.shared.lock().records.clone()
    }

    /// Waits until `count` requests have ended (or the wait of a few seconds is over) and gives all the
    /// requests; a record is final only once its answer has ended.
    pub async fn settled(&self, count: usize) -> Vec<RecordedRequest> {
        let deadline = tokio::time::Instant::now() + SETTLE_TIMEOUT;
        loop {
            let records = self.requests();
            let ended = records
                .iter()
                .filter(|record| record.outcome != Outcome::Pending)
                .count();
            if ended >= count || tokio::time::Instant::now() >= deadline {
                return records;
            }
            tokio::time::sleep(SETTLE_POLL).await;
        }
    }

    /// Forgets what was received; the numbering goes on.
    pub fn clear_requests(&self) {
        self.shared.lock().records.clear();
    }

    /// Sets responses ahead: each part of `script` between `then`s answers the next request, once.
    pub fn queue(&self, script: &str) -> Result<(), ScriptError> {
        control::enqueue(&self.shared, script, &QueueFilter::default(), 1).map(|_| ())
    }

    /// Sets responses ahead for the requests that meet `filter`, each answering `times` requests.
    pub fn queue_for(
        &self,
        script: &str,
        filter: &QueueFilter,
        times: u64,
    ) -> Result<(), ScriptError> {
        control::enqueue(&self.shared, script, filter, times).map(|_| ())
    }

    /// Sets the script that answers any request nothing else answers.
    pub fn set_default(&self, script: &str) -> Result<(), ScriptError> {
        control::set_default(&self.shared, Some(script))
    }

    /// Changes the credential that is checked; `None` lets everything in.
    pub fn set_credential(&self, credential: Option<&str>) {
        self.shared
            .config
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .credential = credential.map(str::to_owned);
    }

    /// Changes the ways in which the credential is accepted.
    pub fn set_accepted(&self, ways: &[Presentation]) {
        self.shared
            .config
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .accept = ways.to_vec();
    }

    /// Changes the models the lists give.
    pub fn set_models(&self, models: Vec<MockModel>) {
        self.shared
            .config
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .models = models;
    }

    /// Forgets requests, queue, default script and the numbering.
    pub fn reset(&self) {
        control::reset(&self.shared);
    }
}

impl Drop for MockLlm {
    fn drop(&mut self) {
        self.task.abort();
    }
}
