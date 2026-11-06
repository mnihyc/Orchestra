//! Real committed writes followed by a real metadata snapshot/credential race.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

use crate::middleware::authentication::{self, Via};
use crate::registry::Ctx;
use crate::rig::{PATIENCE, Scene};
use orch_access_contract::key_scope::KeyFacet;
use orch_api_contract::resources::{ProviderModelCreate, ProviderUpdate};
use orch_types::{Id, Json};
use serde_json::json;
use std::future::Future;
use std::sync::{Mutex, PoisonError};
use tokio::sync::oneshot;

#[path = "../../tests/thread_history_scope/mod.rs"]
mod thread_history_scope;

struct Boundary {
    operation: &'static str,
    reached: oneshot::Sender<Witness>,
    resume: oneshot::Receiver<()>,
}
struct Witness {
    provider: Id,
    user: Id,
    token: Option<Id>,
    session: Option<Id>,
    selection: Option<String>,
    facets: Vec<KeyFacet>,
}
tokio::task_local! {
    static BOUNDARY: Mutex<Option<Boundary>>;
}

/// The actual Provider Read plan has already loaded its stored target on this snapshot.
/// This request-local cut changes no result, effect, credential or scope decision.
pub(crate) async fn before_read(ctx: &Ctx, accepted: &orch_authz::Caller, provider: Id) {
    let boundary = BOUNDARY
        .try_with(|slot| {
            let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
            if slot
                .as_ref()
                .is_some_and(|held| held.operation == ctx.route.name)
            {
                slot.take()
            } else {
                None
            }
        })
        .ok()
        .flatten();
    let Some(boundary) = boundary else {
        return;
    };
    let authed = ctx.user().unwrap();
    let (token, session) = match authed.via {
        Via::Token { token_id, .. } => (Some(token_id), None),
        Via::Session { session_id } => (None, Some(session_id)),
    };
    let _ = boundary.reached.send(Witness {
        provider,
        user: authed.id(),
        token,
        session,
        selection: accepted.admitted_operation().map(str::to_owned),
        facets: accepted.admitted_facets().unwrap_or(&[]).to_vec(),
    });
    let _ = boundary.resume.await;
}

async fn interleave<R, W: Future<Output = ()>>(
    operation: &'static str,
    read: impl Future<Output = R>,
    write: impl FnOnce(Witness) -> W,
) -> R {
    let (reached, wait) = oneshot::channel();
    let (resume, blocked) = oneshot::channel();
    let reader = BOUNDARY.scope(
        Mutex::new(Some(Boundary {
            operation,
            reached,
            resume: blocked,
        })),
        read,
    );
    let writer = async {
        write(wait.await.unwrap()).await;
        resume
            .send(())
            .expect("paused metadata read ended before source change");
    };
    let (read, ()) = tokio::time::timeout(PATIENCE, async { tokio::join!(reader, writer) })
        .await
        .expect("the actual metadata read never reached/resumed its source cut");
    read
}

#[derive(Clone, Copy)]
enum Act {
    Provider,
    Model,
}
impl Act {
    fn operation(self) -> &'static str {
        match self {
            Self::Provider => "ProviderUpdate",
            Self::Model => "ProviderModelCreate",
        }
    }
    async fn call(self, s: &Scene, token: &str, name: &str) -> crate::testing::RawReply {
        match self {
            Self::Provider => {
                let revision: i64 =
                    sqlx::query_scalar("SELECT revision FROM providers WHERE id=$1")
                        .bind(s.on.provider)
                        .fetch_one(s.rig.db.pool())
                        .await
                        .unwrap();
                s.rig
                    .world
                    .app
                    .call(ProviderUpdate, &[&s.on.provider])
                    .token(token)
                    .raw_body(&json!({"name":name,"expected_revision":revision}))
                    .send_bytes()
                    .await
            }
            Self::Model => {
                s.rig
                    .world
                    .app
                    .call(ProviderModelCreate, &[&s.on.provider])
                    .token(token)
                    .raw_body(&json!({"model_name":name,"display_name":name}))
                    .send_bytes()
                    .await
            }
        }
    }
}
fn full(reply: crate::testing::RawReply) -> Json {
    assert_eq!(
        reply.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let value: Json = serde_json::from_slice(&reply.raw).unwrap();
    assert!(
        value.get("projection").is_none(),
        "ordinary/native prime requires genuine Full"
    );
    value
}

async fn conflict_after_accepted_write(act: Act) {
    let s = Scene::new().await;
    full(act.call(&s, &s.ada.token, "ordinary-native-prime").await);
    let selected = thread_history_scope::key(
        &s.rig.world,
        s.ada.id(),
        vec![
            thread_history_scope::rule(
                act.operation(),
                "org",
                s.tree.org.base.id,
                "subtree",
                "manage",
            ),
            thread_history_scope::rule(
                act.operation(),
                "org",
                s.tree.org.base.id,
                "subtree",
                "read",
            ),
        ],
    )
    .await
    .unwrap();
    full(act.call(&s, &selected.secret, "same-key-full-prime").await);
    let before_revision: i64 = sqlx::query_scalar("SELECT revision FROM providers WHERE id=$1")
        .bind(s.on.provider)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    const ACCEPTED: &str = "accepted-before-real-projection-conflict";
    let mut accepted_revision = None;
    let mut accepted_model = None;
    let scene = &s;
    let key = &selected;
    let revision_out = &mut accepted_revision;
    let model_out = &mut accepted_model;
    let reply = interleave(
        act.operation(),
        act.call(&s, &selected.secret, ACCEPTED),
        |witness| {
            Box::pin(async move {
                assert_eq!(witness.provider, scene.on.provider);
                assert_eq!(witness.user, scene.ada.id());
                assert_eq!(witness.token, Some(key.id));
                assert!(witness.session.is_none());
                assert_eq!(witness.selection.as_deref(), Some(act.operation()));
                assert!(
                    witness
                        .facets
                        .iter()
                        .any(|facet| facet.operation == act.operation()
                            && facet.target.object.id == Some(scene.on.provider))
                );
                // This separate connection sees the accepted native mutation BEFORE the error.
                let revision: i64 =
                    sqlx::query_scalar("SELECT revision FROM providers WHERE id=$1")
                        .bind(scene.on.provider)
                        .fetch_one(scene.rig.db.pool())
                        .await
                        .unwrap();
                match act {
                    Act::Provider => {
                        let actual: String =
                            sqlx::query_scalar("SELECT name FROM providers WHERE id=$1")
                                .bind(scene.on.provider)
                                .fetch_one(scene.rig.db.pool())
                                .await
                                .unwrap();
                        assert_eq!(actual, ACCEPTED);
                        assert!(revision > before_revision);
                        *revision_out = Some(revision);
                    }
                    Act::Model => {
                        let model: Id = sqlx::query_scalar(
                            "SELECT id FROM provider_models WHERE provider_id=$1 AND model_name=$2",
                        )
                        .bind(scene.on.provider)
                        .bind(ACCEPTED)
                        .fetch_one(scene.rig.db.pool())
                        .await
                        .unwrap();
                        *model_out = Some(model);
                    }
                }
                // Metadata's RR snapshot was pinned by the real Provider target lookup. A
                // same-key NAME update changes no rights/lifetime, but its new row version
                // forces a real serialization retry at the following FOR SHARE restoration.
                let done = sqlx::query("UPDATE api_tokens SET name=$2 WHERE id=$1")
                    .bind(key.id)
                    .bind("Actual unchanged-scope key metadata rename")
                    .execute(scene.rig.db.pool())
                    .await
                    .unwrap();
                assert_eq!(done.rows_affected(), 1);
            })
        },
    )
    .await;
    assert_eq!(
        reply.status.as_u16(),
        200,
        "committed write became a failed request: {}",
        String::from_utf8_lossy(&reply.raw)
    );
    let value: Json = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["provider_id"], json!(s.on.provider));
    match act {
        Act::Provider => {
            assert_eq!(value["revision"], json!(accepted_revision.unwrap()));
            let current: i64 = sqlx::query_scalar("SELECT revision FROM providers WHERE id=$1")
                .bind(s.on.provider)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap();
            assert_eq!(
                Some(current),
                accepted_revision,
                "accepted write was not replayed"
            );
        }
        Act::Model => {
            assert_eq!(value["model_id"], json!(accepted_model.unwrap()));
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM provider_models WHERE provider_id=$1 AND model_name=$2",
            )
            .bind(s.on.provider)
            .bind(ACCEPTED)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
            assert_eq!(count, 1);
        }
    }
    for omitted in [
        "name",
        "upstreams",
        "config",
        "facts",
        "parameters",
        "groups",
    ] {
        assert!(value.get(omitted).is_none());
    }
}

#[tokio::test]
async fn completed_projection_provider_update_keeps_accepted_revision_after_real_snapshot_retry() {
    conflict_after_accepted_write(Act::Provider).await;
}
#[tokio::test]
async fn completed_projection_model_create_keeps_accepted_model_after_real_snapshot_retry() {
    conflict_after_accepted_write(Act::Model).await;
}

#[tokio::test]
async fn completed_projection_actual_http_session_expiry_withholds_full_but_acknowledges_write() {
    let s = Scene::new().await;
    full(
        Act::Provider
            .call(&s, &s.ada.token, "ordinary-session-prime")
            .await,
    );
    let hash = authentication::token_hash(&s.ada.token);
    let session: Id =
        sqlx::query_scalar("SELECT session_id FROM access_tokens WHERE token_hash=$1")
            .bind(&hash)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    let until = s.rig.world.now() + chrono::Duration::seconds(1);
    let changed = sqlx::query("UPDATE access_tokens SET expires_at=$2 WHERE token_hash=$1")
        .bind(&hash)
        .bind(until)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(changed.rows_affected(), 1);
    assert!(s.rig.world.now() < until);
    let mut accepted_revision = None;
    let revision_out = &mut accepted_revision;
    let scene = &s;
    const ACCEPTED: &str = "accepted-before-actual-http-expiry";
    let reply = interleave(
        "ProviderUpdate",
        Act::Provider.call(&s, &s.ada.token, ACCEPTED),
        |witness| {
            Box::pin(async move {
                assert_eq!(witness.provider, scene.on.provider);
                assert_eq!(witness.user, scene.ada.id());
                assert_eq!(witness.token, None);
                assert_eq!(witness.session, Some(session));
                let actual: (String, i64) =
                    sqlx::query_as("SELECT name,revision FROM providers WHERE id=$1")
                        .bind(scene.on.provider)
                        .fetch_one(scene.rig.db.pool())
                        .await
                        .unwrap();
                assert_eq!(
                    actual.0, ACCEPTED,
                    "native write must be actually committed before expiry"
                );
                *revision_out = Some(actual.1);
                // No database credential row changes here: this proves actual HTTP deadline
                // checking, not a serialization conflict or a native CapturedSession lease.
                scene.rig.world.advance(chrono::Duration::seconds(1));
                assert_eq!(scene.rig.world.now(), until);
            })
        },
    )
    .await;
    assert_eq!(
        reply.status.as_u16(),
        200,
        "accepted effect was vetoed after its actual HTTP session expired: {}",
        String::from_utf8_lossy(&reply.raw)
    );
    let value: Json = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["provider_id"], json!(s.on.provider));
    assert_eq!(value["revision"], json!(accepted_revision.unwrap()));
    let actual: (String, i64) = sqlx::query_as("SELECT name,revision FROM providers WHERE id=$1")
        .bind(s.on.provider)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(actual, (ACCEPTED.to_owned(), accepted_revision.unwrap()));
    for omitted in ["provider", "name", "config", "facts", "upstreams"] {
        assert!(value.get(omitted).is_none());
    }
}
