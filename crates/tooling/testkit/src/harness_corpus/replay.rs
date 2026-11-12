//! Scripted protocol pipes and exact adapter observations, independent of a vendor launcher.
use super::{CorpusError, Frame, Observation};
use futures::StreamExt;
use orch_agent_contract::adapter::EventStream;
use orch_types::{Json, ManualClock};
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

/// Dynamic protocol identifiers, captured from actual requests rather than guessed in fixtures.
pub type Bindings = BTreeMap<String, Json>;

// Native identifiers, clocks and the launcher's own locations may vary. Text, commands,
// model names, settings and tool results must never be hidden by a fixture alias.
fn alias_field(path: &str) -> bool {
    let field = path.rsplit('.').next().unwrap_or(path);
    matches!(
        field,
        "id" | "request_id"
            | "requestId"
            | "request_ref"
            | "sessionId"
            | "session_id"
            | "threadId"
            | "thread_id"
            | "turnId"
            | "turn_id"
            | "messageId"
            | "message_id"
            | "uuid"
            | "message_uuid"
            | "callId"
            | "call_id"
            | "toolUseId"
            | "tool_use_id"
            | "native_ref"
            | "native_session_ref"
            | "native_turn_ref"
            | "native_item_ref"
            | "segment_id"
            | "reported_at"
            | "timestamp"
            | "created_at"
            | "at"
            | "cwd"
            | "directory"
            | "workspace_root"
            | "workspaceRoot"
            | "home"
            | "codexHome"
            | "path"
            | "url"
            | "baseUrl"
            | "base_url"
    )
}

/// Actual protocol values exchanged by the adapter, in the script's direction and order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Transcript {
    pub frames: Vec<Frame>,
    pub exit_code: Option<Option<i32>>,
}

/// Exact JSON comparison, or explicitly requested object subsets. Arrays always keep exact order/count.
/// `{"$replay_capture":"name"}` binds once; `{"$replay_ref":"name"}` compares the same native value later.
pub fn matches(
    expected: &Json,
    actual: &Json,
    subset: bool,
    bindings: &mut Bindings,
    path: &str,
) -> Result<(), CorpusError> {
    if let Some(object) = expected.as_object().filter(|object| object.len() == 1) {
        if let Some(name) = object.get("$replay_capture").and_then(Json::as_str) {
            if name.is_empty() || !alias_field(path) || !(actual.is_string() || actual.is_number())
            {
                return Err(CorpusError::Invalid(format!(
                    "alias outside a native scalar identity/clock/location at {path}"
                )));
            }
            if let Some(before) = bindings.get(name) {
                if before == actual {
                    return Ok(());
                }
            } else {
                bindings.insert(name.to_owned(), actual.clone());
                return Ok(());
            }
            return Err(different(path, expected, actual));
        }
        if let Some(name) = object.get("$replay_ref").and_then(Json::as_str) {
            if !alias_field(path) {
                return Err(CorpusError::Invalid(format!(
                    "alias outside native identity/clock/location at {path}"
                )));
            }
            return if bindings.get(name) == Some(actual) {
                Ok(())
            } else {
                Err(different(path, expected, actual))
            };
        }
    }
    match (expected, actual) {
        (Json::Object(one), Json::Object(other)) if subset || one.len() == other.len() => {
            for (key, value) in one {
                let child = format!("{path}.{key}");
                let Some(actual) = other.get(key) else {
                    return Err(CorpusError::Invalid(format!("missing member {child}")));
                };
                matches(value, actual, subset, bindings, &child)?;
            }
            Ok(())
        }
        (Json::Array(one), Json::Array(other)) if one.len() == other.len() => {
            for (at, (one, other)) in one.iter().zip(other).enumerate() {
                matches(one, other, subset, bindings, &format!("{path}[{at}]"))?;
            }
            Ok(())
        }
        _ if expected == actual => Ok(()),
        _ => Err(different(path, expected, actual)),
    }
}
fn different(path: &str, expected: &Json, actual: &Json) -> CorpusError {
    CorpusError::Mismatch {
        path: path.to_owned(),
        expected: expected.clone(),
        actual: actual.clone(),
    }
}
fn substitute(value: &Json, bindings: &Bindings, path: &str) -> Result<Json, CorpusError> {
    match value {
        Json::Object(object) if object.len() == 1 && object.contains_key("$replay_ref") => {
            let name = object["$replay_ref"]
                .as_str()
                .ok_or_else(|| CorpusError::Invalid("$replay_ref needs a name".into()))?;
            if !alias_field(path) {
                return Err(CorpusError::Invalid(format!(
                    "alias outside native identity/clock/location at {path}"
                )));
            }
            bindings
                .get(name)
                .cloned()
                .ok_or_else(|| CorpusError::Invalid(format!("unbound protocol value {name}")))
        }
        Json::Object(object) => object
            .iter()
            .map(|(key, value)| {
                Ok((
                    key.clone(),
                    substitute(value, bindings, &format!("{path}.{key}"))?,
                ))
            })
            .collect::<Result<serde_json::Map<_, _>, CorpusError>>()
            .map(Json::Object),
        Json::Array(array) => array
            .iter()
            .enumerate()
            .map(|(at, value)| substitute(value, bindings, &format!("{path}[{at}]")))
            .collect::<Result<Vec<_>, _>>()
            .map(Json::Array),
        _ => Ok(value.clone()),
    }
}

/// Replay one process through its native launcher pipes. The caller owns their lifetime and process control.
/// `deadline` is a deadlock guard, not recorded timing; use the same ManualClock for the real adapter.
/// Await native completion before asserting observations, and return `exit_code` through the launcher's control.
pub async fn replay_io<R, W, E>(
    frames: &[Frame],
    reader: &mut R,
    writer: &mut W,
    stderr: &mut E,
    clock: &ManualClock,
    bindings: &mut Bindings,
    deadline: Duration,
) -> Result<Transcript, CorpusError>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
{
    let mut transcript = Transcript::default();
    for (at, step) in frames.iter().enumerate() {
        match step {
            Frame::ToHarness { frame, subset } => {
                let mut line = String::new();
                let read = tokio::time::timeout(deadline, reader.read_line(&mut line))
                    .await
                    .map_err(|_| CorpusError::Timeout(format!("frame {at}")))??;
                if read == 0 {
                    return Err(CorpusError::Invalid(format!(
                        "adapter input ended before frame {at}"
                    )));
                }
                let actual: Json = serde_json::from_str(&line)?;
                matches(frame, &actual, *subset, bindings, &format!("frames[{at}]"))?;
                transcript.frames.push(Frame::ToHarness {
                    frame: actual,
                    subset: *subset,
                });
            }
            Frame::FromHarness { frame } => {
                let frame = substitute(frame, bindings, &format!("frames[{at}]"))?;
                writer.write_all(format!("{frame}\n").as_bytes()).await?;
                writer.flush().await?;
                transcript.frames.push(Frame::FromHarness { frame });
            }
            Frame::Raw { line, newline } => {
                writer.write_all(line.as_bytes()).await?;
                if *newline {
                    writer.write_all(b"\n").await?;
                }
                writer.flush().await?;
                transcript.frames.push(step.clone());
            }
            Frame::Stderr { line } => {
                stderr.write_all(format!("{line}\n").as_bytes()).await?;
                stderr.flush().await?;
                transcript.frames.push(step.clone());
            }
            Frame::Tick { milliseconds } => {
                clock.advance(chrono::Duration::milliseconds(i64::from(*milliseconds)));
                transcript.frames.push(step.clone());
                tokio::task::yield_now().await;
            }
            Frame::Exit { code } => {
                transcript.exit_code = Some(*code);
                writer.shutdown().await?;
                transcript.frames.push(step.clone());
                if at + 1 != frames.len() {
                    return Err(CorpusError::Invalid(
                        "steps after process exit cannot be replayed".into(),
                    ));
                }
            }
        }
    }
    Ok(transcript)
}

/// Collect the actual adapter stream through a native terminal event, preserving all intermediate events.
pub async fn collect_through(
    events: &mut EventStream,
    terminal: &str,
    deadline: Duration,
) -> Result<Vec<Observation>, CorpusError> {
    let mut observed = Vec::new();
    loop {
        let event = tokio::time::timeout(deadline, events.next())
            .await
            .map_err(|_| CorpusError::Timeout(format!("event {terminal}")))?
            .ok_or_else(|| {
                CorpusError::Invalid(format!("event stream closed before {terminal}"))
            })?;
        let event = Observation::from_event(&event)?;
        let done = event.name == terminal;
        observed.push(event);
        if done {
            return Ok(observed);
        }
    }
}
