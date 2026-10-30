//! Changing requests ask for their authority again in the transaction that writes: shares with agents, requests for access and their decisions, and the registry that refuses a request the ledger names guarded when it answers without asking.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use axum::body::Body;
use bytes::Bytes;
use chrono::Duration;
use desk::Desk;
use futures::stream;
use http::{Request, StatusCode};
use orch_api_contract::access::{
    AccessRequestCreate, AccessRequestCreateBody, AccessRequestItem, AccessRequestObject,
    EffectiveAccessGet, EffectiveAccessGetQuery,
};
use orch_api_contract::approvals::{
    AccessDecision, Approval, ApprovalDecision, ApprovalList, ApprovalListQuery,
};
use orch_api_contract::common::Page;
use orch_api_contract::directory::GroupDelete;
use orch_api_contract::share::{
    AgentShareList, AgentShareListQuery, ShareDocumentsLevel, ShareParts, ShareTo, ShareToNode,
    ShareWhat, ShareWhatNode, ShareWithAgents,
};
use orch_api_contract::threads::ShareMode;
use orch_api_core::middleware::authentication::token_hash;
use orch_api_core::middleware::requests::ApiError;
use orch_api_core::middleware::transaction_guard::{self, Boundary};
use orch_api_core::registry::{Ctx, Registry};
use orch_db::queries::identity;
use orch_types::{Id, new_id};
use places::Who;
use std::future::Future;
use support::World;
use tower::ServiceExt;

/// Sends a request whose body arrives only after `change` has happened: the request is let in at its door
/// first, and whatever it writes is decided after the body came.
async fn send_after(
    world: &World,
    who: &Who,
    method: &str,
    path: &str,
    value: serde_json::Value,
    change: impl Future<Output = ()> + Send + 'static,
) -> axum::response::Response {
    let body = Body::from_stream(stream::once(async move {
        change.await;
        Ok::<_, std::io::Error>(Bytes::from(serde_json::to_vec(&value).unwrap()))
    }));
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    registry
        .into_router()
        .with_state(world.app.app.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {}", who.token))
                .header("content-type", "application/json")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap()
}

/// The browser session `who` is signed in with.
async fn session_of(world: &World, who: &Who) -> Id {
    let now = world.now();
    identity::session_by_access_token(&mut *world.conn().await, &token_hash(&who.token), now)
        .await
        .unwrap()
        .unwrap()
        .session_id
}

/// Signs `who` out of `session`, as another tab or an admin would while a body is on its way.
fn signing_out(world: &World, who: &Who, session: Id) -> impl Future<Output = ()> + Send + 'static {
    let pool = world.db.pool.clone();
    let user = who.id();
    let now = world.now();
    async move {
        let mut tx = pool.begin().await.unwrap();
        identity::revoke_session(&mut tx, session, user, now)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
}

/// Time passes while the body comes, and nothing else changes.
fn waiting(world: &World) -> impl Future<Output = ()> + Send + 'static {
    let clock = world.app.clock.clone();
    async move {
        clock.advance(Duration::seconds(2));
    }
}

// ---------------------------------------------------------------- the registry holds a request to its ledger row

/// One request the ledger names guarded, served by a handler that writes nothing: it checks its guard first
/// when `checks`, and answers at once otherwise.
fn group_delete(world: &World, checks: bool, allow: bool) -> axum::Router {
    let mut registry = Registry::new();
    if allow {
        registry.allow_unguarded();
    }
    registry.on(GroupDelete, move |ctx: Ctx, (): (), (): ()| async move {
        if checks {
            let (tx, _guard) = transaction_guard::begin(&ctx).await?;
            drop(tx);
        }
        Ok::<(), ApiError>(())
    });
    registry.into_router().with_state(world.app.app.clone())
}

async fn status_of(router: axum::Router, who: &Who) -> StatusCode {
    router
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/groups/{}", new_id()))
                .header("authorization", format!("Bearer {}", who.token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn a_request_the_ledger_names_guarded_cannot_answer_without_asking_again() {
    let world = World::new().await;
    let who = places::person(&world, "ada").await;
    assert_eq!(
        transaction_guard::boundary_of("GroupDelete"),
        Boundary::Guarded
    );
    let unasked = status_of(group_delete(&world, false, false), &who).await;
    if cfg!(debug_assertions) {
        assert_eq!(unasked, StatusCode::INTERNAL_SERVER_ERROR);
    } else {
        assert_eq!(unasked, StatusCode::NO_CONTENT, "a release build logs it");
    }
    assert_eq!(
        status_of(group_delete(&world, true, false), &who).await,
        StatusCode::NO_CONTENT,
        "asking again in the transaction is what the ledger asks"
    );
    assert_eq!(
        status_of(group_delete(&world, false, true), &who).await,
        StatusCode::NO_CONTENT,
        "a test that measures a write without its guard says so"
    );
}

// ---------------------------------------------------------------- sharing with agents

/// Ada shares the topic's documents, to read, with the agents of the topic itself.
fn documents_of_the_topic(desk: &Desk, client: &str) -> serde_json::Value {
    serde_json::to_value(ShareWithAgents {
        read_first: None,
        what: ShareWhat::Node(ShareWhatNode {
            node_id: desk.topic,
        }),
        inside: None,
        parts: Some(ShareParts {
            documents: Some(ShareDocumentsLevel::Read),
            ..ShareParts::default()
        }),
        mode: ShareMode::Live,
        to: ShareTo::Nodes(vec![ShareToNode {
            node_id: desk.topic,
        }]),
        comment: None,
        expires_at: None,
        download: None,
        client_id: client.to_owned(),
    })
    .unwrap()
}

async fn shares_of_the_topic(desk: &Desk, who: &Who) -> usize {
    desk.world
        .app
        .call(AgentShareList, &[&"nodes", &desk.topic])
        .token(&who.token)
        .query(&AgentShareListQuery { side: None })
        .send()
        .await
        .ok()
        .len()
}

#[tokio::test]
async fn a_share_whose_maker_was_signed_out_while_its_body_came_is_not_made() {
    let desk = Desk::new().await;
    let reader = desk.member("vic", "viewer").await;
    let session = session_of(&desk.world, &desk.ada).await;
    let refused = send_after(
        &desk.world,
        &desk.ada,
        "POST",
        "/share/agents",
        documents_of_the_topic(&desk, "signed-out"),
        signing_out(&desk.world, &desk.ada, session),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(shares_of_the_topic(&desk, &reader).await, 0);
}

#[tokio::test]
async fn a_share_by_somebody_still_signed_in_after_a_slow_body_is_made_once() {
    let desk = Desk::new().await;
    let made = send_after(
        &desk.world,
        &desk.ada,
        "POST",
        "/share/agents",
        documents_of_the_topic(&desk, "slow"),
        waiting(&desk.world),
    )
    .await;
    assert!(made.status().is_success(), "{}", made.status());
    assert_eq!(shares_of_the_topic(&desk, &desk.ada).await, 1);
}

// ---------------------------------------------------------------- asking for access, and deciding it

fn interact_on_the_project(desk: &Desk) -> AccessRequestCreateBody {
    AccessRequestCreateBody {
        items: vec![AccessRequestItem {
            label: None,
            object: AccessRequestObject {
                kind: "project".to_owned(),
                id: desk.project,
            },
            permissions: vec!["project.interact".to_owned()],
            options: None,
            holder: None,
        }],
        reason: "I need it".to_owned(),
        for_thread: None,
    }
}

async fn waiting_for(desk: &Desk, who: &Who) -> Vec<Id> {
    let page: Page<Approval> = desk
        .world
        .app
        .call(ApprovalList, &[])
        .token(&who.token)
        .query(&ApprovalListQuery::default())
        .send()
        .await
        .ok();
    page.items.iter().map(|approval| approval.id).collect()
}

async fn holds_interact(desk: &Desk, who: &Who) -> bool {
    desk.world
        .app
        .call(EffectiveAccessGet, &[&"nodes", &desk.project])
        .token(&who.token)
        .query(&EffectiveAccessGetQuery { user_id: who.id() })
        .send()
        .await
        .ok()
        .permissions
        .iter()
        .any(|held| held.permission == "project.interact")
}

#[tokio::test]
async fn a_request_for_access_whose_asker_was_signed_out_while_it_came_asks_nobody() {
    let desk = Desk::new().await;
    let rae = desk.member("rae", "viewer").await;
    let session = session_of(&desk.world, &rae).await;
    let refused = send_after(
        &desk.world,
        &rae,
        "POST",
        "/access-requests",
        serde_json::to_value(interact_on_the_project(&desk)).unwrap(),
        signing_out(&desk.world, &rae, session),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    assert!(waiting_for(&desk, &desk.ada).await.is_empty());
}

#[tokio::test]
async fn a_request_for_access_after_a_slow_body_is_asked_once() {
    let desk = Desk::new().await;
    let rae = desk.member("rae", "viewer").await;
    let made = send_after(
        &desk.world,
        &rae,
        "POST",
        "/access-requests",
        serde_json::to_value(interact_on_the_project(&desk)).unwrap(),
        waiting(&desk.world),
    )
    .await;
    assert!(made.status().is_success(), "{}", made.status());
    assert_eq!(waiting_for(&desk, &desk.ada).await.len(), 1);
}

fn approve() -> serde_json::Value {
    serde_json::to_value(ApprovalDecision::Access(AccessDecision {
        approve: true,
        permissions: None,
        options: None,
        expires_at: None,
        note: None,
        quick_duration: None,
    }))
    .unwrap()
}

/// Rae asks for Can interact on the project, which Ada may give.
async fn asked(desk: &Desk, rae: &Who) -> Id {
    desk.world
        .app
        .call(AccessRequestCreate, &[])
        .token(&rae.token)
        .body(&interact_on_the_project(desk))
        .send()
        .await
        .ok()
        .approvals[0]
        .id
}

#[tokio::test]
async fn a_decision_whose_decider_was_signed_out_while_it_came_gives_nothing() {
    let desk = Desk::new().await;
    let rae = desk.member("rae", "viewer").await;
    let approval = asked(&desk, &rae).await;
    let session = session_of(&desk.world, &desk.ada).await;
    let refused = send_after(
        &desk.world,
        &desk.ada,
        "POST",
        &format!("/approvals/{approval}/decision"),
        approve(),
        signing_out(&desk.world, &desk.ada, session),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    assert!(!holds_interact(&desk, &rae).await);
}

#[tokio::test]
async fn a_decision_after_a_slow_body_gives_what_was_asked() {
    let desk = Desk::new().await;
    let rae = desk.member("rae", "viewer").await;
    let approval = asked(&desk, &rae).await;
    let decided = send_after(
        &desk.world,
        &desk.ada,
        "POST",
        &format!("/approvals/{approval}/decision"),
        approve(),
        waiting(&desk.world),
    )
    .await;
    assert_eq!(decided.status(), StatusCode::OK);
    assert!(holds_interact(&desk, &rae).await);
}
