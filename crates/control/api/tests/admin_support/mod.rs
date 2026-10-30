//! A cast for the tests of Management: a system admin, an admin of an org who is none, an ordinary person, and the few things every request of Management is asked the same way.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

use crate::places::{Who, admin, make_org, person};
use crate::support::World;
use orch_api_contract::common::{ErrorCode, Problem};
use orch_api_contract::routes::Endpoint;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity::testing as user_rows;
use orch_db::secrets::MasterKey;
use orch_types::{Id, Json};
use serde_json::json;

/// The people of a test of Management.
pub struct Cast {
    /// A system admin, in a browser session.
    pub root: Who,
    /// The owner of Acme, and so its Admin: no system admin.
    pub boss: Who,
    /// A person with nothing but their own org.
    pub ada: Who,
    /// The org `boss` made.
    pub acme: Id,
}

/// Makes the cast, and gives the places that seal a secret the key of these tests.
pub async fn cast(world: &World) -> Cast {
    orch_api_core::routes::install_master_key(&MasterKey::new([7; 32]));
    let root = admin(world, "root").await;
    let boss = person(world, "boss").await;
    let ada = person(world, "ada").await;
    let acme = make_org(world, &boss, "Acme", "acme").await.base.id;
    Cast {
        root,
        boss,
        ada,
        acme,
    }
}

/// An API token of `who`, which never expires.
pub async fn api_token(world: &World, who: &Who) -> String {
    let token = new_token(API_TOKEN_PREFIX);
    let mut conn = world.conn().await;
    user_rows::insert_api_token(&mut conn, who.id(), &token_hash(&token))
        .await
        .unwrap();
    token
}

/// What a request answers when somebody who is no system admin makes it: the refusal, made with
/// `who`'s browser session.
pub async fn refused_to<E: Endpoint>(
    world: &World,
    endpoint: E,
    params: &[&dyn std::fmt::Display],
    who: &Who,
    body: Option<Json>,
    query: &str,
) -> Problem {
    let mut call = world.app.call(endpoint, params).token(&who.token);
    if let Some(body) = &body {
        call = call.raw_body(body);
    }
    if !query.is_empty() {
        call = call.raw_query(query);
    }
    call.send_bytes().await.problem()
}

/// Asserts that a refusal is the one for somebody who is no system admin.
pub fn assert_no_system_admin(problem: &Problem, what: &str) {
    assert_eq!(problem.code, ErrorCode::AuthzDenied, "{what}: {problem:?}");
    assert_eq!(problem.status, 403, "{what}");
    assert_eq!(problem.extra["required"], json!("system.admin"), "{what}");
}

/// One line of the audit trail, as the tests read it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Line {
    pub action: String,
    pub actor_kind: String,
    pub actor_id: Option<Id>,
    pub object_id: Option<Id>,
    pub node_id: Option<Id>,
    pub summary: String,
    pub details: Json,
}

/// The audit rows of the action `action`, oldest first.
pub async fn lines_of(world: &World, action: &str) -> Vec<Line> {
    sqlx::query_as(
        "SELECT action, actor_kind, actor_id, object_id, node_id, summary, details
           FROM audit_log WHERE action = $1 ORDER BY at, id",
    )
    .bind(action)
    .fetch_all(&world.db.pool)
    .await
    .unwrap()
}
