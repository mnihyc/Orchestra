//! The control endpoint of the scripted model server: what a test or a script outside the process uses to set responses ahead and to read what was received.
//!
//! Everything is JSON, below `/_mock/`, and none of it needs the credential of the model interface. The
//! requests are listed in the documentation of the parent module. A script that is malformed is refused with
//! status 400 and a body that names the line.

use std::str::FromStr;

use axum::body::Body;
use axum::response::Response;
use http::header::CONTENT_TYPE;
use http::{HeaderValue, Method, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

use super::MockModel;
use super::record::Presentation;
use super::request::Format;
use super::script::{Script, ScriptError};
use super::server::{CONTROL_PREFIX, QueueFilter, Queued, Shared};

/// How many requests a response set ahead answers unless it is told.
const DEFAULT_TIMES: u64 = 1;

fn one() -> u64 {
    DEFAULT_TIMES
}

/// One response set ahead, as the request writes it.
#[derive(Debug, Deserialize)]
struct QueueItem {
    script: String,
    #[serde(default = "one")]
    times: u64,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    format: Option<Format>,
}

/// The body of a request to set responses ahead: one, or several.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum QueueBody {
    Many { scripts: Vec<QueueItem> },
    One(QueueItem),
}

/// The body of a request to change the credential.
#[derive(Debug, Deserialize)]
struct CredentialBody {
    #[serde(default)]
    credential: Option<String>,
    #[serde(default)]
    accept: Option<Vec<String>>,
}

fn reply(status: StatusCode, body: &Value) -> Response {
    let mut response = Response::new(Body::from(body.to_string()));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

fn refuse(message: &str) -> Response {
    reply(StatusCode::BAD_REQUEST, &json!({ "error": message }))
}

/// Sets responses ahead: each segment of the script is the response to one request, `times` requests each.
/// Returns how many entries were added.
pub fn enqueue(
    shared: &Shared,
    script: &str,
    filter: &QueueFilter,
    times: u64,
) -> Result<usize, ScriptError> {
    let segments = Script::parse(script)?.segments;
    let added = segments.len();
    let mut inner = shared.lock();
    for segment in segments {
        inner.queue.push_back(Queued {
            segment,
            filter: filter.clone(),
            remaining: times.max(1),
        });
    }
    Ok(added)
}

/// Sets the script that answers any request nothing else answers.
pub fn set_default(shared: &Shared, script: Option<&str>) -> Result<(), ScriptError> {
    let parsed = script.map(Script::parse).transpose()?;
    shared.lock().default = parsed;
    Ok(())
}

/// Forgets what was received, what was set ahead and the default script, and starts counting again.
pub fn reset(shared: &Shared) {
    let mut inner = shared.lock();
    inner.seq = 0;
    inner.records.clear();
    inner.queue.clear();
    inner.default = None;
    inner.stored.clear();
}

fn queue(shared: &Shared, body: &[u8]) -> Response {
    let parsed: QueueBody = match serde_json::from_slice(body) {
        Ok(parsed) => parsed,
        Err(error) => return refuse(&format!("the body is not a script to queue: {error}")),
    };
    let items = match parsed {
        QueueBody::Many { scripts } => scripts,
        QueueBody::One(item) => vec![item],
    };
    let mut added = 0;
    for item in items {
        let filter = QueueFilter {
            model: item.model,
            format: item.format,
        };
        match enqueue(shared, &item.script, &filter, item.times) {
            Ok(count) => added += count,
            Err(error) => return refuse(&format!("the script is malformed: {error}")),
        }
    }
    reply(StatusCode::OK, &json!({ "queued": added }))
}

fn queue_listing(shared: &Shared) -> Response {
    let inner = shared.lock();
    let entries: Vec<Value> = inner
        .queue
        .iter()
        .map(|entry| {
            json!({
                "model": entry.filter.model, "format": entry.filter.format,
                "remaining": entry.remaining
            })
        })
        .collect();
    reply(StatusCode::OK, &json!({ "queue": entries }))
}

fn requests(shared: &Shared, query: Option<&str>) -> Response {
    let since = form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .find(|(name, _)| name == "since")
        .and_then(|(_, value)| value.parse::<u64>().ok())
        .unwrap_or(0);
    let inner = shared.lock();
    let seen: Vec<Value> = inner
        .records
        .iter()
        .filter(|record| record.seq > since)
        .filter_map(|record| serde_json::to_value(record).ok())
        .collect();
    reply(StatusCode::OK, &Value::Array(seen))
}

fn models(shared: &Shared, method: &Method, body: &[u8]) -> Response {
    if method == Method::GET {
        let models = shared.config().models;
        return reply(
            StatusCode::OK,
            &serde_json::to_value(models).unwrap_or(Value::Null),
        );
    }
    match serde_json::from_slice::<Vec<MockModel>>(body) {
        Ok(models) => {
            let count = models.len();
            shared
                .config
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .models = models;
            reply(StatusCode::OK, &json!({ "models": count }))
        }
        Err(error) => refuse(&format!("the body is not a list of models: {error}")),
    }
}

fn credential(shared: &Shared, body: &[u8]) -> Response {
    let parsed: CredentialBody = match serde_json::from_slice(body) {
        Ok(parsed) => parsed,
        Err(error) => return refuse(&format!("the body is not a credential: {error}")),
    };
    let accept = match parsed
        .accept
        .map(|ways| {
            ways.iter()
                .map(|way| Presentation::from_str(way))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()
    {
        Ok(accept) => accept,
        Err(error) => return refuse(&error),
    };
    let mut config = shared
        .config
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    config.credential = parsed.credential;
    if let Some(accept) = accept {
        config.accept = accept;
    }
    reply(
        StatusCode::OK,
        &json!({ "checked": config.credential.is_some() }),
    )
}

/// Answers a request to the control endpoint.
pub fn handle(
    shared: &Shared,
    method: &Method,
    path: &str,
    query: Option<&str>,
    body: &[u8],
) -> Response {
    let name = path.strip_prefix(CONTROL_PREFIX).unwrap_or_default();
    let writes = method == Method::POST || method == Method::PUT;
    match (name, method) {
        ("health", &Method::GET) => reply(
            StatusCode::OK,
            &json!({ "ok": true, "requests": shared.lock().seq }),
        ),
        ("requests", &Method::GET) => requests(shared, query),
        ("requests", &Method::DELETE) => {
            let mut inner = shared.lock();
            let cleared = inner.records.len();
            inner.records.clear();
            reply(StatusCode::OK, &json!({ "cleared": cleared }))
        }
        ("queue", &Method::GET) => queue_listing(shared),
        ("queue", _) if writes => queue(shared, body),
        ("queue", &Method::DELETE) => {
            let mut inner = shared.lock();
            let cleared = inner.queue.len();
            inner.queue.clear();
            reply(StatusCode::OK, &json!({ "cleared": cleared }))
        }
        ("default", _) if writes => {
            let script = serde_json::from_slice::<Value>(body)
                .ok()
                .and_then(|value| {
                    value
                        .get("script")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                });
            match script {
                Some(script) => match set_default(shared, Some(&script)) {
                    Ok(()) => reply(StatusCode::OK, &json!({ "default": true })),
                    Err(error) => refuse(&format!("the script is malformed: {error}")),
                },
                None => refuse("the body is {\"script\": \"…\"}"),
            }
        }
        ("default", &Method::DELETE) => {
            shared.lock().default = None;
            reply(StatusCode::OK, &json!({ "default": false }))
        }
        ("models", &Method::GET) => models(shared, method, body),
        ("models", _) if writes => models(shared, method, body),
        ("credential", _) if writes => credential(shared, body),
        ("reset", _) if writes => {
            reset(shared);
            reply(StatusCode::OK, &json!({ "reset": true }))
        }
        _ => reply(
            StatusCode::NOT_FOUND,
            &json!({ "error": format!("no such control request: {method} {path}") }),
        ),
    }
}
