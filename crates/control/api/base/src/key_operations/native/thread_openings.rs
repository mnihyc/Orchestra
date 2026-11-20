//! Share the projection only while requests for the same ordinary session overlap.
use super::{ApiError, ApiResult, Ctx, Id, Selected, transaction_guard};
use crate::middleware::authentication::Via;
use crate::registry::JsonBody;
use bytes::Bytes;
use orch_api_contract::threads::{ThreadOpening, ThreadOpeningGetQuery};
use orch_types::Time;
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;
use tokio::sync::watch;

#[derive(Hash, PartialEq, Eq)]
struct Key {
    app: usize,
    user: Id,
    session: Id,
    thread: Id,
    continuation: Option<Id>,
    revision: Option<i64>,
    limit: Option<u32>,
    headers: blake3::Hash,
    peer: Option<std::net::IpAddr>,
}

impl Key {
    fn for_request(ctx: &Ctx, query: &ThreadOpeningGetQuery) -> Option<Self> {
        if !ctx.ordinary_browser_opening() {
            return None;
        }
        let user = ctx.user().ok()?;
        let Via::Session { session_id } = user.via else {
            return None;
        };
        // Include the exact access token and all header inputs without retaining their values.
        let mut hash = blake3::Hasher::new();
        for (name, value) in &ctx.headers {
            for part in [name.as_str().as_bytes(), value.as_bytes()] {
                hash.update(&part.len().to_le_bytes());
                hash.update(part);
            }
        }
        Some(Self {
            app: Arc::as_ptr(&ctx.app) as usize,
            user: user.id(),
            session: session_id,
            thread: ctx.id("id").ok()?,
            continuation: query.continuation_id,
            revision: query.projection_revision,
            limit: query.limit,
            headers: hash.finalize(),
            peer: ctx.remote,
        })
    }
}

/// The shared JSON tree also supplies the authorized metadata used to record each visit.
pub(crate) struct EncodedOpening {
    value: Value,
    common: Bytes,
    failed: Vec<&'static str>,
}

impl EncodedOpening {
    fn new(body: &ThreadOpening) -> ApiResult<Self> {
        Self::from_value(serde_json::to_value(body).map_err(ApiError::internal)?)
    }

    fn from_value(mut value: Value) -> ApiResult<Self> {
        let fields = value
            .as_object_mut()
            .ok_or_else(|| ApiError::internal("an opening must encode an object"))?;
        // Only these top-level panes carry a Problem for the current HTTP request.
        let mut local = Vec::new();
        for name in ["screen", "gateway", "pending_agent_schedules"] {
            if fields
                .get(name)
                .and_then(|v| v.get("status"))
                .and_then(Value::as_str)
                == Some("failed")
                && let Some(part) = fields.remove(name)
            {
                local.push((name, part));
            }
        }
        let mut common = serde_json::to_vec(fields).map_err(ApiError::internal)?;
        if !local.is_empty() && common.pop() != Some(b'}') {
            return Err(ApiError::internal(
                "the opening fields must encode an object",
            ));
        }
        let mut failed = Vec::with_capacity(local.len());
        for (name, part) in local {
            fields.insert(name.to_owned(), part);
            failed.push(name);
        }
        Ok(Self {
            value,
            common: Bytes::from(common),
            failed,
        })
    }

    pub(crate) fn value_for(&self, request_id: &str) -> ApiResult<Cow<'_, Value>> {
        let mut value = Cow::Borrowed(&self.value);
        for &name in &self.failed {
            let field = value
                .to_mut()
                .get_mut(name)
                .and_then(|part| part.pointer_mut("/problem/request_id"))
                .ok_or_else(|| {
                    ApiError::internal("a failed opening pane has no request identifier")
                })?;
            *field = Value::String(request_id.to_owned());
        }
        Ok(value)
    }

    pub(crate) fn bytes_for(&self, value: &Value) -> ApiResult<Bytes> {
        if self.failed.is_empty() {
            return Ok(self.common.clone());
        }
        let mut json = self.common.to_vec();
        for &name in &self.failed {
            json.push(b',');
            serde_json::to_writer(&mut json, name).map_err(ApiError::internal)?;
            json.push(b':');
            serde_json::to_writer(&mut json, &value[name]).map_err(ApiError::internal)?;
        }
        json.push(b'}');
        Ok(Bytes::from(json))
    }
}

// Only a sealed group whose shared native View check committed receives this value.
struct CheckedOpening {
    body: Arc<EncodedOpening>,
    deadline: Option<Time>,
}
type ResultSlot = Option<Arc<CheckedOpening>>;

struct Flight {
    before_snapshot: Instant,
    reply: watch::Sender<ResultSlot>,
}

/// Tracks overlapping reads for the direct and session-restoring routes to the same thread opening.
#[derive(Default)]
pub struct Openings(Mutex<HashMap<Arc<Key>, Flight>>);

enum Read<'a> {
    Produce(Producer<'a>),
    Follow(watch::Receiver<ResultSlot>),
    Independent,
}

struct Producer<'a> {
    key: Arc<Key>,
    openings: &'a Openings,
    reply: watch::Sender<ResultSlot>,
}

impl Producer<'_> {
    /// Removing the map entry closes enrollment before the group's fresh View check starts.
    fn seal(self) -> watch::Sender<ResultSlot> {
        self.reply.clone()
    }
}

impl Drop for Producer<'_> {
    fn drop(&mut self) {
        self.openings
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
    }
}

impl Openings {
    fn enter(&self, key: Key, arrived: Instant) -> Read<'_> {
        let mut current = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(work) = current.get(&key) {
            // An HTTP request already in authentication when the producer started
            // can observe that snapshot. Later requests need their own current read.
            return if arrived < work.before_snapshot {
                Read::Follow(work.reply.subscribe())
            } else {
                Read::Independent
            };
        }
        let key = Arc::new(key);
        let (reply, _) = watch::channel(None);
        current.insert(
            key.clone(),
            Flight {
                before_snapshot: Instant::now(),
                reply: reply.clone(),
            },
        );
        Read::Produce(Producer {
            key,
            openings: self,
            reply,
        })
    }

    /// Serve the declared opening with its own native source selection and Thread View gate.
    pub(crate) async fn read<F: Selected<Query = ThreadOpeningGetQuery, Reply = ThreadOpening>>(
        &self,
        ctx: Ctx,
        query: ThreadOpeningGetQuery,
    ) -> ApiResult<JsonBody<ThreadOpening>> {
        if F::WATCH_ORDINARY || F::DECLARATION.operation != ctx.route.name {
            return super::selected_read::<F>(ctx, query, ())
                .await
                .map(JsonBody::Typed);
        }
        crate::testing::note_boundary(ctx.route.name, "opening.handler");
        let work = Key::for_request(&ctx, &query)
            .map_or(Read::Independent, |key| self.enter(key, ctx.arrived));
        match work {
            Read::Independent => super::selected_read::<F>(ctx, query, ())
                .await
                .map(JsonBody::Typed),
            Read::Produce(producer) => {
                let body = super::selected_read::<F>(ctx.clone(), query, ()).await?;
                let reply = producer.seal();
                if reply.receiver_count() != 0 {
                    // All subscribers have the producer's exact inputs and were already
                    // in this HTTP read before its selection. Check their common Thread
                    // View once more on a fresh snapshot after the last enrollment.
                    super::read(ctx.clone(), F::require).await?;
                    crate::testing::note_boundary(ctx.route.name, "opening.view_checked");
                    let body = Arc::new(EncodedOpening::new(&body)?);
                    drop(reply.send_replace(Some(Arc::new(CheckedOpening {
                        body: body.clone(),
                        deadline: ctx.publication.retained_deadline()?,
                    }))));
                    return Ok(JsonBody::Opening(body));
                }
                Ok(JsonBody::Typed(body))
            }
            Read::Follow(mut receiver) => {
                // Keep this waiter's entry restrictions and lifetime while its checked group renders.
                let user = ctx.user()?;
                transaction_guard::guard_user(user)?;
                ctx.publication
                    .retain_ceiling(&crate::middleware::authorization::caller_of(user)?)?;
                ctx.publication.check_deadline(&ctx)?;
                crate::testing::note_boundary(ctx.route.name, "opening.joined");
                crate::testing::hold_boundary(ctx.route.name, "opening.joined").await;
                let shared = receiver
                    .wait_for(Option::is_some)
                    .await
                    .ok()
                    .and_then(|slot| slot.clone());
                let Some(shared) = shared else {
                    // A cancelled or unsuccessful producer did not publish a body.
                    return super::selected_read::<F>(ctx, query, ())
                        .await
                        .map(JsonBody::Typed);
                };
                // The private receiver carries the completed native snapshot check for
                // this participant. The registry also checks its own exact bearer before
                // headers and before yielding the body frame.
                ctx.publication.retain_deadline(shared.deadline)?;
                ctx.publication.check_deadline(&ctx)?;
                transaction_guard::checked_snapshot_read();
                crate::testing::note_boundary(ctx.route.name, "opening.shared_json_ready");
                Ok(JsonBody::Opening(shared.body.clone()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn simultaneous_replies_preserve_content_and_correlate_only_failed_panes() -> ApiResult<()> {
        let one = "first-request";
        let two = "second-request";
        let pane = json!({"status":"failed","problem":{"request_id":one,"detail":"refused","extra":{"request_id":one}}});
        let initial = json!({"sync":{"items":[{"text":one,"problem":{"request_id":one}}]},"screen":{"status":"ready","data":{"request_id":one}},"gateway":pane,"pending_agent_schedules":pane});
        let payload = EncodedOpening::from_value(initial.clone())?;
        for id in [one, two] {
            let actual = payload.value_for(id)?;
            let bytes = payload.bytes_for(&actual)?;
            let decoded: Value = serde_json::from_slice(&bytes).map_err(ApiError::internal)?;
            let mut expected = initial.clone();
            expected["gateway"]["problem"]["request_id"] = json!(id);
            expected["pending_agent_schedules"]["problem"]["request_id"] = json!(id);
            assert_eq!(decoded, expected);
            assert_eq!(&*actual, &decoded);
        }
        Ok(())
    }
}
