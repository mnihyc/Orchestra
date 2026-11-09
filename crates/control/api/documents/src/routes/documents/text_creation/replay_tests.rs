//! Actual native creation followed by a new same-account response credential; no writer replay.

use super::*;

use crate::middleware::authentication;

use crate::places::make_org;

use crate::places::person;

use crate::registry::Ctx;

use crate::support::World;

use orch_access_contract::key_scope::KeyAction;

use orch_access_contract::key_scope::KeyOperation;

use orch_access_contract::key_scope::KeyReach;

use orch_access_contract::key_scope::KeyRule;

use orch_access_contract::key_scope::KeyScope;

use orch_access_contract::key_scope::KeyTarget;

use orch_api_contract::documents::NodeDocumentCreate;

use orch_api_contract::documents::NodeDocumentCreateBody;

use orch_api_contract::routes::Endpoint;

use orch_api_contract::surface::HttpSurface;

use orch_db::queries::identity;

use orch_types::new_id;
pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Fixture {
    world: World,
    creator: Id,
    org: Id,
    document: Id,
}

impl Fixture {
    async fn new() -> TestResult<Self> {
        let world = World::new().await;
        let maker = person(&world, "replay-maker").await;
        let org = make_org(&world, &maker, "Replay source", "replay-source").await;
        let made = world
            .app
            .call(NodeDocumentCreate, &[&org.base.id])
            .token(&maker.token)
            .body(&NodeDocumentCreateBody {
                title: "Original creation".into(),
                blocks: None,
                markdown: None,
                access: None,
            })
            .send()
            .await
            .ok()
            .into_full()
            .map_err(|_| "the ordinary native creation returned no document")?;
        // The creation's first credential really ends. A completed replay must not restore it.
        sqlx::query("UPDATE sessions SET revoked_at=$2 WHERE id=(SELECT session_id FROM access_tokens WHERE token_hash=$1)").bind(authentication::token_hash(&maker.token)).bind(world.now()).execute(&world.db.pool).await?;
        Ok(Self {
            creator: maker.id(),
            org: org.base.id,
            document: made.base.id,
            world,
        })
    }
    async fn current(&self, operation: &str, read: bool) -> TestResult<Ctx> {
        let bearer = authentication::new_token(authentication::API_TOKEN_PREFIX);
        let id = new_id();
        let (kind, target, resource, action) = if read {
            (
                ObjectKind::Document,
                self.document,
                DOCUMENT,
                KeyAction::Read,
            )
        } else {
            (ObjectKind::Org, self.org, DOCUMENT, KeyAction::Write)
        };
        let policy = KeyScope {
            rules: vec![KeyRule {
                target: KeyTarget {
                    kind: KeyTargetKind::Object(kind),
                    id: Some(target),
                    reach: KeyReach::SelfOnly,
                },
                resources: Some(vec![resource]),
                allow: vec![KeyOperation {
                    operation: operation.into(),
                    actions: vec![action],
                }],
            }],
            ..KeyScope::default()
        };
        let mut conn = self.world.conn().await;
        identity::create_api_token_scoped(
            &mut conn,
            &identity::NewApiToken {
                id,
                user_id: self.creator,
                name: "Current completed-replay reader",
                token_hash: &authentication::token_hash(&bearer),
                scopes: &[],
                node_id: None,
                at: self.world.now(),
                expires_at: Some(self.world.now() + chrono::Duration::minutes(10)),
            },
            Some(&serde_json::json!(policy)),
        )
        .await?;
        drop(conn);
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_str(&format!("Bearer {bearer}"))?,
        );
        let user = authentication::authenticate_on(
            HttpSurface::Key,
            &self.world.app.app,
            &headers,
            self.world.now(),
        )
        .await?;
        let mut ctx = Ctx::for_tool(
            self.world.app.app.clone(),
            user,
            NodeDocumentCreate::ROUTE,
            self.org,
        );
        ctx.surface = Some(HttpSurface::Key);
        ctx.headers = headers;
        Ok(ctx)
    }
    fn completed(&self) -> Created {
        Created {
            id: self.document,
            facts: vec![],
        }
    }
    async fn creations(&self) -> TestResult<i64> {
        Ok(
            sqlx::query_scalar("SELECT count(*) FROM documents WHERE created_by=$1")
                .bind(self.creator)
                .fetch_one(&self.world.db.pool)
                .await?,
        )
    }
}

#[cfg(any(test, feature = "test-support"))]
async fn completed_creation_uses_new_credential_current_read_and_native_metadata_without_spent_sources()
-> TestResult {
    let s = Fixture::new().await?;
    let ctx = s.current("NodeDocumentCreate", true).await?;
    sqlx::query("UPDATE documents SET title='Current native metadata' WHERE id=$1")
        .bind(s.document)
        .execute(&s.world.db.pool)
        .await?;
    let before = s.creations().await?;
    let full = reply::render_completed_replay::<AtNode>(&ctx, s.creator, s.completed())
        .await?
        .ok_or("current same-operation Read was refused")?;
    assert_eq!(full.base.id, s.document);
    assert_eq!(full.base.title, "Current native metadata");
    assert_eq!(s.creations().await?, before);
    crate::key_operations::publication::check(&ctx).await?;
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_completed_creation_uses_new_credential_current_read_and_native_metadata_without_spent_sources()
-> ::futures::future::LocalBoxFuture<
    'static,
    ::orch_api_documents::routes::documents::text_creation::replay_tests::TestResult,
> {
    ::std::boxed::Box::pin(self::completed_creation_uses_new_credential_current_read_and_native_metadata_without_spent_sources())
}

#[cfg(any(test, feature = "test-support"))]
async fn completed_creation_without_own_operation_read_returns_no_full_body_or_second_creation()
-> TestResult {
    let s = Fixture::new().await?;
    let before = s.creations().await?;
    for (operation, read) in [("NodeDocumentCreate", false), ("DocumentGet", true)] {
        let ctx = s.current(operation, read).await?;
        assert!(
            reply::render_completed_replay::<AtNode>(&ctx, s.creator, s.completed())
                .await?
                .is_none()
        );
    }
    assert_eq!(s.creations().await?, before);
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_completed_creation_without_own_operation_read_returns_no_full_body_or_second_creation()
-> ::futures::future::LocalBoxFuture<
    'static,
    ::orch_api_documents::routes::documents::text_creation::replay_tests::TestResult,
> {
    ::std::boxed::Box::pin(
        self::completed_creation_without_own_operation_read_returns_no_full_body_or_second_creation(
        ),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn completed_creation_replay_is_bound_to_record_account_and_current_delivery_deadline()
-> TestResult {
    let s = Fixture::new().await?;
    let ctx = s.current("NodeDocumentCreate", true).await?;
    let error = reply::render_completed_replay::<AtNode>(&ctx, new_id(), s.completed())
        .await
        .err()
        .ok_or("a different completed-record account was accepted")?;
    assert_eq!(error.problem.status, 404);
    assert!(
        reply::render_completed_replay::<AtNode>(&ctx, s.creator, s.completed())
            .await?
            .is_some()
    );
    s.world.advance(chrono::Duration::minutes(11));
    assert!(
        crate::key_operations::publication::check(&ctx)
            .await
            .is_err()
    );
    assert_eq!(s.creations().await?, 1);
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_completed_creation_replay_is_bound_to_record_account_and_current_delivery_deadline()
-> ::futures::future::LocalBoxFuture<
    'static,
    ::orch_api_documents::routes::documents::text_creation::replay_tests::TestResult,
> {
    ::std::boxed::Box::pin(
        self::completed_creation_replay_is_bound_to_record_account_and_current_delivery_deadline(),
    )
}
