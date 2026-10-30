//! The HTTP side of the scripted model server: routing, the credential check, the choice of script, and the playing of a response.
//!
//! Every request that is not for the control endpoint is remembered, its credential is checked, and it is
//! routed by the end of its path, so that any base address in front of the paths works: `…/responses`,
//! `…/responses/compact`, `…/messages`, `…/messages/count_tokens` and `…/models`. The script that answers a
//! request to create a response is chosen in this order: a directive in the message of the person, a script
//! set ahead whose conditions the request meets, the default script, and at last the plain answer
//! `You said: <the message>`.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::response::Response;
use bytes::Bytes;
use futures::Stream;
use http::header::CONTENT_TYPE;
use http::{HeaderName, HeaderValue, Method, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

use super::chat;
use super::errors;
use super::messages;
use super::plan::{Ctx, Ending, Frame, Usage, estimate, usage_of};
use super::record::{
    CredentialCheck, Outcome, RecordedRequest, ScriptSource, Secrets, Verdict, check_credential,
};
use super::request::{Format, Inbound, analyze};
use super::responses;
use super::script::{Script, ScriptError, Segment, Step};
use super::{MockConfig, control, listing};

/// The largest request body the server reads: what the vendors take (32 MiB for the Messages format).
pub const MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;

/// The path below which the control endpoint is.
pub const CONTROL_PREFIX: &str = "/_mock/";

/// The media type of a stream.
const STREAM_TYPE: &str = "text/event-stream; charset=utf-8";

/// The media type of a document.
const JSON_TYPE: &str = "application/json";

/// The media type of an answer that is no document of either format: the page a gateway in the way gives.
const RAW_TYPE: &str = "text/html; charset=utf-8";

/// The header that carries the identifier of a request in the Responses format.
const RESPONSES_REQUEST_ID: &str = "x-request-id";

/// The header that carries the identifier of a request in the Messages format.
const MESSAGES_REQUEST_ID: &str = "request-id";

/// The header the client is told to wait by.
const RETRY_AFTER: &str = "retry-after";

/// How long the server waits before it breaks a connection on purpose: long enough for the pieces sent before
/// to leave the machine, so that the client sees them and then sees the break, and not the break alone.
const ABORT_GRACE: Duration = Duration::from_millis(20);

/// The text of the plain answer to a message without a script.
const ECHO: &str = "You said: ";

/// A script set ahead: a response, and which requests it is for.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct QueueFilter {
    /// A model name, or a pattern with `*` for any run of characters, that the request must name.
    #[serde(default)]
    pub model: Option<String>,
    /// The format the request must be in.
    #[serde(default)]
    pub format: Option<Format>,
}

/// Whether `text` matches `pattern`, in which `*` stands for any run of characters.
fn glob(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !text.starts_with(first) || !text.ends_with(last) || text.len() < first.len() + last.len() {
        return false;
    }
    let mut rest = &text[first.len()..text.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

impl QueueFilter {
    /// Whether a request for `model` in `format` meets the conditions.
    #[must_use]
    pub fn matches(&self, model: Option<&str>, format: Format) -> bool {
        self.format.is_none_or(|wanted| wanted == format)
            && self
                .model
                .as_deref()
                .is_none_or(|pattern| model.is_some_and(|model| glob(pattern, model)))
    }
}

/// A response set ahead.
#[derive(Debug, Clone)]
pub struct Queued {
    /// What it does.
    pub segment: Segment,
    /// Which requests it is for.
    pub filter: QueueFilter,
    /// How many more requests it answers.
    pub remaining: u64,
}

/// A script that a later request may continue, by naming the response it follows.
#[derive(Debug, Clone)]
pub struct Stored {
    script: Script,
    round: usize,
    source: ScriptSource,
}

/// What the server changes as it runs.
#[derive(Debug, Default)]
pub struct Inner {
    /// The number of the last request.
    pub seq: u64,
    /// What was remembered of the requests.
    pub records: Vec<RecordedRequest>,
    /// The responses set ahead.
    pub queue: VecDeque<Queued>,
    /// The script that answers anything else.
    pub default: Option<Script>,
    /// Scripts that a request naming a response continues.
    pub stored: HashMap<String, Stored>,
}

/// What the handlers share.
#[derive(Debug)]
pub struct Shared {
    /// How the server is set up; it can be changed while it runs.
    pub config: RwLock<MockConfig>,
    /// What changes as it runs.
    pub inner: Mutex<Inner>,
}

impl Shared {
    /// A server with the given set-up and nothing remembered.
    #[must_use]
    pub fn new(config: MockConfig) -> Self {
        let default = config
            .default_script
            .as_deref()
            .and_then(|text| Script::parse(text).ok());
        Self {
            config: RwLock::new(config),
            inner: Mutex::new(Inner {
                default,
                ..Inner::default()
            }),
        }
    }

    /// The state, for a short while.
    pub fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A copy of the set-up.
    pub fn config(&self) -> MockConfig {
        self.config
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Changes what was remembered of a request.
    fn update(&self, seq: u64, change: impl FnOnce(&mut RecordedRequest)) {
        let mut inner = self.lock();
        if let Some(record) = inner
            .records
            .iter_mut()
            .rev()
            .find(|record| record.seq == seq)
        {
            change(record);
        }
    }
}

/// What a request is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Responses,
    Compact,
    Messages,
    CountTokens,
    Chat,
    Models,
    Unknown,
}

impl Route {
    fn of(method: &Method, path: &str) -> Self {
        let ends = |suffix: &str| path.ends_with(suffix);
        if method == Method::POST && ends("/responses/compact") {
            Self::Compact
        } else if method == Method::POST && ends("/responses") {
            Self::Responses
        } else if method == Method::POST && ends("/messages/count_tokens") {
            Self::CountTokens
        } else if method == Method::POST && ends("/messages") {
            Self::Messages
        } else if method == Method::POST && ends("/chat/completions") {
            Self::Chat
        } else if method == Method::GET && ends("/models") {
            Self::Models
        } else {
            Self::Unknown
        }
    }

    fn format(self) -> Option<Format> {
        match self {
            Self::Responses | Self::Compact => Some(Format::Responses),
            Self::Messages | Self::CountTokens => Some(Format::Messages),
            Self::Chat => Some(Format::Chat),
            Self::Models | Self::Unknown => None,
        }
    }
}

/// The header that tells the Messages format from the other, where the path does not.
const VERSION_HEADER: &str = "anthropic-version";

/// How a response is answered.
struct Answer {
    status: u16,
    content_type: &'static str,
    headers: Vec<(String, String)>,
    frames: Vec<Frame>,
    ending: Ending,
    error: bool,
    /// How long nothing at all is sent, the status line included.
    stall: Option<Duration>,
}

impl Answer {
    /// A document, answered whole.
    fn document(status: u16, body: &Value) -> Self {
        Self {
            status,
            content_type: JSON_TYPE,
            headers: Vec::new(),
            frames: vec![Frame::Bytes(Bytes::from(body.to_string()))],
            ending: Ending::Completed,
            error: !(200..300).contains(&status),
            stall: None,
        }
    }

    fn with_header(mut self, name: &str, value: String) -> Self {
        self.headers.push((name.to_owned(), value));
        self
    }
}

/// The error a request is answered with, in the format; `now` is the moment, in seconds since the epoch, that
/// a limit which ends after some seconds is dated from.
fn failure(format: Format, wanted: &super::script::ErrorSpec, seq: u64, now: i64) -> Answer {
    let reply = errors::reply(format, wanted, seq, now);
    let mut answer = Answer::document(reply.status, &reply.body);
    if let Some(seconds) = reply.retry_after {
        answer = answer.with_header(RETRY_AFTER, seconds.to_string());
    }
    for (name, value) in reply.headers {
        answer = answer.with_header(&name, value);
    }
    answer.error = true;
    answer
}

/// A plain error of a kind, in the format.
fn plain_failure(
    format: Format,
    kind: super::script::ErrorKind,
    message: &str,
    seq: u64,
) -> Answer {
    failure(
        format,
        &super::script::ErrorSpec {
            message: Some(message.to_owned()),
            ..super::script::ErrorSpec::of(kind)
        },
        seq,
        0,
    )
}

/// The identifier of a request, as the vendors send it back in a header.
fn request_id(format: Format, seq: u64) -> (&'static str, String) {
    match format {
        Format::Responses | Format::Chat => (RESPONSES_REQUEST_ID, format!("req_mock_{seq:06}")),
        Format::Messages => (MESSAGES_REQUEST_ID, format!("req_mock_{seq:06}")),
    }
}

/// The set of everything one response of a script is: the choice made for it.
struct Selected {
    segment: Segment,
    source: ScriptSource,
    script: Option<Script>,
    round: usize,
}

/// Chooses the script that answers a request to create a response.
fn select(shared: &Shared, inbound: &Inbound, format: Format) -> Result<Selected, ScriptError> {
    let result = inbound.results.join("\n");
    if let Some(found) = Script::find(&inbound.driving) {
        let script = found?;
        let round = inbound.rounds;
        return Ok(Selected {
            segment: script.segment(round).with_result(&result),
            source: ScriptSource::Directive,
            script: Some(script),
            round,
        });
    }
    let mut inner = shared.lock();
    if inbound.driving.is_empty()
        && let Some(stored) = inbound
            .previous_response_id
            .as_ref()
            .and_then(|previous| inner.stored.get(previous))
            .cloned()
    {
        return Ok(Selected {
            segment: stored.script.segment(stored.round).with_result(&result),
            source: stored.source,
            round: stored.round,
            script: Some(stored.script),
        });
    }
    if let Some(at) = inner
        .queue
        .iter()
        .position(|entry| entry.filter.matches(inbound.model.as_deref(), format))
    {
        let entry = &mut inner.queue[at];
        let segment = entry.segment.with_result(&result);
        entry.remaining = entry.remaining.saturating_sub(1);
        if entry.remaining == 0 {
            inner.queue.remove(at);
        }
        return Ok(Selected {
            segment,
            source: ScriptSource::Queue,
            script: None,
            round: inbound.rounds,
        });
    }
    if let Some(script) = inner.default.clone() {
        let round = inbound.rounds;
        return Ok(Selected {
            segment: script.segment(round).with_result(&result),
            source: ScriptSource::Default,
            script: Some(script),
            round,
        });
    }
    let segment = if inbound.rounds > 0 {
        Segment::done()
    } else {
        let said = inbound.driving.last().map_or("", String::as_str);
        Segment::saying(&format!("{ECHO}{said}"))
    };
    Ok(Selected {
        segment,
        source: ScriptSource::Echo,
        script: None,
        round: inbound.rounds,
    })
}

/// How long a plain answer is held back: the sleeps of the script, and the waits between the pieces.
fn total_wait(segment: &Segment) -> Duration {
    segment
        .steps
        .iter()
        .map(|step| match step {
            Step::Sleep(duration) => *duration,
            Step::Text { text, pace } | Step::Reasoning { text, pace } => {
                pace.delay
                    * u32::try_from(super::script::pieces(text, pace.chunk).len())
                        .unwrap_or(u32::MAX)
            }
            Step::Tool {
                arguments: text,
                pace,
                ..
            }
            | Step::Custom {
                input: text, pace, ..
            } => {
                pace.delay
                    * u32::try_from(super::script::pieces(text, pace.chunk).len())
                        .unwrap_or(u32::MAX)
            }
            _ => Duration::ZERO,
        })
        .sum()
}

/// The answer to a request to create a response, as the selected script says: what the script asks of the
/// answer as a whole (headers to add, how long to hold it back) is put on it here.
fn generate(
    shared: &Shared,
    config: &MockConfig,
    format: Format,
    seq: u64,
    inbound: &Inbound,
    selected: &Selected,
) -> Answer {
    let segment = &selected.segment;
    let mut answer = generate_body(shared, config, format, seq, inbound, selected);
    answer.headers.extend(segment.headers.iter().cloned());
    answer.stall = segment.stall;
    answer
}

/// The answer itself, before the headers and the wait the script adds.
fn generate_body(
    shared: &Shared,
    config: &MockConfig,
    format: Format,
    seq: u64,
    inbound: &Inbound,
    selected: &Selected,
) -> Answer {
    let segment = &selected.segment;
    let now = config.clock.now().timestamp();
    if let Some(raw) = &segment.raw_body {
        return Answer {
            status: raw.status,
            content_type: RAW_TYPE,
            headers: Vec::new(),
            frames: vec![Frame::Bytes(Bytes::from(raw.text.clone()))],
            ending: Ending::Completed,
            error: !(200..300).contains(&raw.status),
            stall: None,
        };
    }
    if let Some(wanted) = &segment.error {
        return failure(format, wanted, seq, now);
    }
    let usage: Usage = usage_of(segment, inbound.chars);
    let ctx = Ctx {
        seq,
        model: segment
            .model
            .clone()
            .or_else(|| inbound.model.clone())
            .unwrap_or_else(|| config.default_model.clone()),
        created_at: config.created_at,
        encrypted_reasoning: inbound.encrypted_reasoning,
        previous_response_id: inbound.previous_response_id.clone(),
    };
    let calls = segment
        .steps
        .iter()
        .any(|step| matches!(step, Step::Tool { .. } | Step::Custom { .. }));
    if calls
        && format == Format::Responses
        && let Some(script) = &selected.script
    {
        shared.lock().stored.insert(
            format!("resp_mock_{seq:06}"),
            Stored {
                script: script.clone(),
                round: selected.round + 1,
                source: selected.source,
            },
        );
    }
    let (name, id) = request_id(format, seq);
    if inbound.stream {
        let (frames, ending) = match format {
            Format::Responses => responses::stream(segment, &usage, &ctx),
            Format::Messages => messages::stream(segment, &usage, &ctx),
            Format::Chat => chat::stream(segment, &usage, &ctx),
        };
        return Answer {
            status: StatusCode::OK.as_u16(),
            content_type: STREAM_TYPE,
            headers: vec![(name.to_owned(), id)],
            frames,
            ending,
            error: false,
            stall: None,
        };
    }
    if format == Format::Messages
        && let super::script::Finish::Fail(wanted) = &segment.finish
    {
        return failure(format, wanted, seq, now);
    }
    let body = match format {
        Format::Responses => responses::plain(segment, &usage, &ctx),
        Format::Messages => messages::plain(segment, &usage, &ctx),
        Format::Chat => chat::plain(segment, &usage, &ctx),
    }
    .to_string();
    let mut frames = Vec::new();
    let wait = total_wait(segment);
    if !wait.is_zero() {
        frames.push(Frame::Wait(wait));
    }
    let mut ending = Ending::Completed;
    let cut = segment.steps.iter().find_map(|step| match step {
        Step::Cut => Some(Ending::Cut),
        Step::Truncate => Some(Ending::Truncated),
        _ => None,
    });
    match cut {
        Some(how) => {
            let half = body.len() / 2;
            frames.push(Frame::Bytes(Bytes::from(body.as_bytes()[..half].to_vec())));
            if how == Ending::Cut {
                frames.push(Frame::Abort);
            }
            ending = how;
        }
        None => frames.push(Frame::Bytes(Bytes::from(body))),
    }
    Answer {
        status: StatusCode::OK.as_u16(),
        content_type: JSON_TYPE,
        headers: vec![(name.to_owned(), id)],
        frames,
        ending,
        error: false,
        stall: None,
    }
}

/// The answer to a request to compact a conversation.
fn compact(config: &MockConfig, seq: u64, body: &Value, chars: usize) -> Answer {
    let kept: Vec<Value> = match body.get("input") {
        Some(Value::Array(items)) => items
            .iter()
            .filter(|item| {
                item.get("role").and_then(Value::as_str) == Some("user")
                    && item
                        .get("type")
                        .and_then(Value::as_str)
                        .is_none_or(|kind| kind == "message")
            })
            .cloned()
            .collect(),
        Some(Value::String(text)) => {
            vec![json!({ "type": "message", "role": "user", "content": text })]
        }
        _ => Vec::new(),
    };
    let ctx = Ctx {
        seq,
        model: config.default_model.clone(),
        created_at: config.created_at,
        encrypted_reasoning: false,
        previous_response_id: None,
    };
    let usage = Usage {
        input: estimate(chars).max(1),
        output: 1,
        cached: 0,
        reasoning: 0,
        cache_write: 0,
        cost: None,
    };
    let (name, id) = request_id(Format::Responses, seq);
    Answer::document(
        StatusCode::OK.as_u16(),
        &responses::compaction(&ctx, kept, &usage),
    )
    .with_header(name, id)
}

/// Remembers, plays and answers one request.
async fn serve_api(shared: Arc<Shared>, parts: http::request::Parts, bytes: Bytes) -> Response {
    let config = shared.config();
    let route = Route::of(&parts.method, parts.uri.path());
    let format = route.format().unwrap_or_else(|| {
        if parts.headers.contains_key(VERSION_HEADER) || parts.uri.path().contains("messages") {
            Format::Messages
        } else {
            Format::Responses
        }
    });
    let secrets = Secrets::new(
        &config.secret_headers,
        config
            .credential
            .iter()
            .cloned()
            .chain(config.secret_values.iter().cloned()),
    );
    let credential: CredentialCheck =
        check_credential(&parts.headers, config.credential.as_deref(), &config.accept);
    let body_text = secrets.mask_text(&String::from_utf8_lossy(&bytes));
    let parsed: Option<Value> = serde_json::from_slice(&bytes).ok();
    let inbound = match (&parsed, route) {
        (Some(body), Route::Responses | Route::Messages | Route::Chat) => {
            Some(analyze(format, body))
        }
        _ => None,
    };
    let seq = {
        let mut inner = shared.lock();
        inner.seq += 1;
        let seq = inner.seq;
        inner.records.push(RecordedRequest {
            seq,
            method: parts.method.to_string(),
            path: parts.uri.path().to_owned(),
            query: parts.uri.query().map(|query| secrets.mask_text(query)),
            headers: secrets.mask_headers(&parts.headers),
            body: serde_json::from_str(&body_text).unwrap_or(Value::String(body_text.clone())),
            format: route.format(),
            model: parsed
                .as_ref()
                .and_then(|body| body.get("model"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            stream: inbound.as_ref().is_some_and(|inbound| inbound.stream),
            credential: credential.clone(),
            script: None,
            round: 0,
            status: None,
            outcome: Outcome::Pending,
        });
        seq
    };
    let answer = if matches!(credential.verdict, Verdict::Missing | Verdict::Wrong) {
        let message = match credential.verdict {
            Verdict::Missing => "no credential was presented",
            _ => "the credential presented is not accepted here",
        };
        plain_failure(
            format,
            super::script::ErrorKind::Authentication,
            message,
            seq,
        )
    } else {
        match route {
            Route::Models => Answer::document(
                StatusCode::OK.as_u16(),
                &listing::list(&config, format, parts.uri.query()),
            ),
            Route::Unknown => plain_failure(
                format,
                super::script::ErrorKind::NotFound,
                &format!("no such route: {} {}", parts.method, parts.uri.path()),
                seq,
            ),
            Route::CountTokens
            | Route::Compact
            | Route::Responses
            | Route::Messages
            | Route::Chat
                if parsed.is_none() =>
            {
                plain_failure(
                    format,
                    super::script::ErrorKind::InvalidRequest,
                    "the body is no JSON",
                    seq,
                )
            }
            Route::CountTokens => {
                let chars = parsed
                    .as_ref()
                    .map_or(0, |body| analyze(format, body).chars);
                Answer::document(
                    StatusCode::OK.as_u16(),
                    &messages::token_count(estimate(chars).max(1)),
                )
            }
            Route::Compact => {
                let body = parsed.unwrap_or(Value::Null);
                let chars = analyze(format, &body).chars;
                compact(&config, seq, &body, chars)
            }
            Route::Responses | Route::Messages | Route::Chat => {
                let inbound = inbound.unwrap_or_default();
                match select(&shared, &inbound, format) {
                    Ok(selected) => {
                        shared.update(seq, |record| {
                            record.script = Some(selected.source);
                            record.round = selected.round;
                        });
                        generate(&shared, &config, format, seq, &inbound, &selected)
                    }
                    Err(error) => plain_failure(
                        format,
                        super::script::ErrorKind::InvalidRequest,
                        &format!("the script in the message is malformed: {error}"),
                        seq,
                    ),
                }
            }
        }
    };
    shared.update(seq, |record| record.status = Some(answer.status));
    if let Some(stall) = answer.stall {
        tokio::time::sleep(stall).await;
    }
    play(shared, seq, answer)
}

/// Writes the outcome when the body is over or the client is gone.
struct Guard {
    shared: Arc<Shared>,
    seq: u64,
    planned: Outcome,
    reached_end: bool,
}

impl Guard {
    /// The body has reached its end, whatever the end was.
    fn reached_end(&mut self) {
        self.reached_end = true;
    }

    /// The body is over: the outcome is the one planned.
    fn finish(mut self) {
        self.reached_end();
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let outcome = if self.reached_end {
            self.planned
        } else {
            Outcome::ClientGone
        };
        self.shared
            .update(self.seq, |record| record.outcome = outcome);
    }
}

/// The body played from frames: bytes are sent, waits are slept, and an abort breaks the connection.
fn frames_stream(frames: Vec<Frame>, guard: Guard) -> impl Stream<Item = Result<Bytes, io::Error>> {
    futures::stream::unfold(
        (frames.into_iter(), guard),
        |(mut frames, mut guard)| async move {
            loop {
                match frames.next() {
                    Some(Frame::Bytes(bytes)) => return Some((Ok(bytes), (frames, guard))),
                    Some(Frame::Wait(duration)) => tokio::time::sleep(duration).await,
                    Some(Frame::Abort) => {
                        tokio::time::sleep(ABORT_GRACE).await;
                        guard.reached_end();
                        let error = io::Error::new(
                            io::ErrorKind::ConnectionAborted,
                            "the script cut the connection",
                        );
                        return Some((Err(error), (Vec::new().into_iter(), guard)));
                    }
                    None => {
                        guard.finish();
                        return None;
                    }
                }
            }
        },
    )
}

/// The outcome an answer is recorded with once it is over.
fn planned(answer: &Answer) -> Outcome {
    if answer.error {
        return Outcome::Error;
    }
    match answer.ending {
        Ending::Completed => Outcome::Completed,
        Ending::StreamError => Outcome::Error,
        Ending::Cut => Outcome::Cut,
        Ending::Truncated => Outcome::Truncated,
    }
}

/// Builds the response of an answer.
fn play(shared: Arc<Shared>, seq: u64, answer: Answer) -> Response {
    let outcome = planned(&answer);
    let mut builder = Response::builder()
        .status(StatusCode::from_u16(answer.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
        .header(CONTENT_TYPE, answer.content_type);
    for (name, value) in &answer.headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::try_from(name.as_str()),
            HeaderValue::from_str(value),
        ) {
            builder = builder.header(name, value);
        }
    }
    let whole = matches!(answer.frames.as_slice(), [Frame::Bytes(_)]);
    let body = if whole {
        shared.update(seq, |record| record.outcome = outcome);
        match answer.frames.into_iter().next() {
            Some(Frame::Bytes(bytes)) => Body::from(bytes),
            _ => Body::empty(),
        }
    } else {
        let guard = Guard {
            shared,
            seq,
            planned: outcome,
            reached_end: false,
        };
        Body::from_stream(frames_stream(answer.frames, guard))
    };
    builder.body(body).unwrap_or_else(|_| {
        let mut response =
            Response::new(Body::from("the scripted server could not build its answer"));
        *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        response
    })
}

/// The handler of every request.
pub async fn handle(State(shared): State<Arc<Shared>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let Ok(bytes) = to_bytes(body, MAX_REQUEST_BYTES).await else {
        let mut response = Response::new(Body::from(
            "the request body is larger than the server reads",
        ));
        *response.status_mut() = StatusCode::PAYLOAD_TOO_LARGE;
        return response;
    };
    if parts.uri.path().starts_with(CONTROL_PREFIX) {
        return control::handle(
            &shared,
            &parts.method,
            parts.uri.path(),
            parts.uri.query(),
            &bytes,
        );
    }
    serve_api(shared, parts, bytes).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_with_stars_matches_runs_of_characters_and_one_without_matches_exactly() {
        assert!(glob("claude-haiku-5.5", "claude-haiku-5.5"));
        assert!(!glob("claude-haiku-5.5", "claude-haiku-5.5x"));
        assert!(glob("claude-haiku*", "claude-haiku-5.5"));
        assert!(glob("*haiku*", "anthropic/claude-haiku-5.5"));
        assert!(glob("a*b*c", "a-x-b-y-c"));
        assert!(!glob("a*b*c", "a-x-c-y-b"));
        assert!(glob("*", ""));
        assert!(!glob("ab*ba", "aba"), "the two ends may not overlap");
    }

    #[test]
    fn a_path_is_routed_by_its_end_so_that_any_base_address_works() {
        for (method, path, route) in [
            (Method::POST, "/v1/responses", Route::Responses),
            (Method::POST, "/responses", Route::Responses),
            (Method::POST, "/openai/v1/responses/compact", Route::Compact),
            (Method::POST, "/v1/messages", Route::Messages),
            (Method::POST, "/v1/chat/completions", Route::Chat),
            (Method::POST, "/chat/completions", Route::Chat),
            (
                Method::POST,
                "/anthropic/v1/messages/count_tokens",
                Route::CountTokens,
            ),
            (Method::GET, "/v1/models", Route::Models),
            (Method::GET, "/models", Route::Models),
            (Method::GET, "/v1/responses", Route::Unknown),
            (Method::POST, "/v1/models", Route::Unknown),
            (Method::POST, "/v1/messages/batches", Route::Unknown),
            (Method::POST, "/v1/xresponses", Route::Unknown),
        ] {
            assert_eq!(Route::of(&method, path), route, "{method} {path}");
        }
    }

    #[test]
    fn a_filter_meets_a_request_by_model_and_format() {
        let filter = QueueFilter {
            model: Some("claude-haiku*".into()),
            format: Some(Format::Messages),
        };
        assert!(filter.matches(Some("claude-haiku-5.5"), Format::Messages));
        assert!(!filter.matches(Some("claude-opus-5"), Format::Messages));
        assert!(!filter.matches(Some("claude-haiku-5.5"), Format::Responses));
        assert!(!filter.matches(None, Format::Messages));
        assert!(QueueFilter::default().matches(None, Format::Responses));
    }
}
