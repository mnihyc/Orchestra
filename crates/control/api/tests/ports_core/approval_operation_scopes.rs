//! The common approval operation uses the existing reviewed port writer and once-only result.
use super::*;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::approvals::{ApprovalDecide, ApprovalGet};
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_db::queries::identity;
use orch_types::Json;

const OP: &str = "ApprovalDecide.port";
struct Key {
    id: Id,
    secret: String,
}
fn rule(kind: &str, id: Id, resource: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[resource],"allow":[{"operation":OP,"actions":actions}]})
}
async fn key(
    world: &World,
    manager: &Who,
    env: Id,
    thread: Id,
    project: Id,
    machine: Id,
    full: bool,
) -> Key {
    let mut rules = vec![rule("environment", env, "environment", &["manage"])];
    if full {
        rules.push(rule("environment", env, "approval", &["read"]));
        rules.push(rule("environment", env, "environment", &["read"]));
        rules.push(rule("machine", machine, "machine", &["read"]));
        let mut conn = world.conn().await;
        let home: Id = sqlx::query_scalar("SELECT home_node_id FROM environments WHERE id=$1")
            .bind(env)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        let nodes =
            orch_db::queries::hierarchy::with_ancestors(&mut conn, &[home, project, thread])
                .await
                .unwrap();
        for node in nodes {
            rules.push(rule(&node.kind, node.id, &node.kind, &["read"]));
        }
    }
    let policy: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(policy.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: manager.id(),
            name: "Approval port operation control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(policy).unwrap()),
    )
    .await
    .unwrap();
    Key { id, secret }
}
fn checked(reply: RawReply, status: StatusCode) -> Json {
    let body: Json = serde_json::from_slice(&reply.raw).unwrap();
    // The result may contain an actual one-time link capability: print only typed problem metadata.
    assert_eq!(
        reply.status, status,
        "code={}, required={}",
        body["code"], body["required"]
    );
    body
}
async fn decide(
    world: &World,
    token: &str,
    id: Id,
    body: &orch_api_contract::approvals::ApprovalDecision,
) -> RawReply {
    world
        .app
        .call(ApprovalDecide, &[&id])
        .token(token)
        .body(body)
        .send_bytes()
        .await
}
fn link_request(world: &World, project: Id, port: u16) -> PortOpeningRequest {
    PortOpeningRequest {
        kind: PortOpeningKind::Link,
        place_id: None,
        until: Some(Time::new(world.now() + chrono::Duration::hours(1))),
        entry_methods: Some(vec![PortLinkMethod::Helper]),
        ..request(port, project, "a reviewer inspects the exact service")
    }
}
async fn waiting(
    world: &World,
    manager: &Who,
    thread: Id,
    request: &PortOpeningRequest,
) -> PortOpenReply {
    let made = world
        .app
        .call(ThreadPortRequestCreate, &[&thread])
        .token(&manager.token)
        .body(request)
        .send()
        .await
        .ok();
    assert_eq!(made.opening.state, PortOpeningState::Waiting);
    assert!(made.link.is_none());
    assert!(made.request.is_some());
    made
}
#[tokio::test]
async fn scoped_port_once_returns_the_exact_person_link_only_in_its_authorized_result() {
    let (world, manager, _, env, project, machine) = setup().await;
    links_asked(&world).await;
    let thread = asking_thread(&world, &manager, project, env).await;
    let made = waiting(
        &world,
        &manager,
        thread,
        &link_request(&world, project, 9171),
    )
    .await;
    let approval = made.request.as_ref().unwrap().id;
    let key = key(&world, &manager, env, thread, project, machine, true).await;
    let decision = port_decision(
        PortDecisionChoice::Once,
        Some(PortLinkRecipient::Person),
        made.opening.revision,
    );
    let shown = checked(
        decide(&world, &key.secret, approval, &decision).await,
        StatusCode::OK,
    );
    assert_eq!(shown["state"], "approved");
    assert_eq!(
        shown["opening_result"]["opening"]["id"],
        json!(made.opening.id)
    );
    let link = &shown["opening_result"]["link"];
    assert_eq!(link["recipient"], "person");
    assert!(
        link["helper_link_once"]
            .as_str()
            .is_some_and(|url| url.contains(&format!("/api/v1/ports/links/{}#", made.opening.id)))
    );
    let (digest, sealed, recipient): (Option<Vec<u8>>, Option<Vec<u8>>, Option<String>) =
        sqlx::query_as(
            "SELECT secret_hash,secret_sealed,link_recipient FROM port_openings WHERE id=$1",
        )
        .bind(made.opening.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert!(digest.is_some());
    assert!(sealed.is_none());
    assert_eq!(recipient.as_deref(), Some("person"));
    let read = checked(
        world
            .app
            .call(ApprovalGet, &[&approval])
            .token(&manager.token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert!(read.get("opening_result").is_none());
    assert!(!read.to_string().contains("helper_link_once"));
    let late = checked(
        decide(&world, &key.secret, approval, &decision).await,
        StatusCode::CONFLICT,
    );
    assert_eq!(late["code"], "approval.already_decided");
    assert!(!late.to_string().contains("helper_link_once"));
}
#[tokio::test]
async fn scoped_port_once_without_read_preserves_its_generated_person_proof_and_no_existing_card() {
    let (world, manager, _, env, project, machine) = setup().await;
    links_asked(&world).await;
    let thread = asking_thread(&world, &manager, project, env).await;
    let made = waiting(
        &world,
        &manager,
        thread,
        &link_request(&world, project, 9172),
    )
    .await;
    let approval = made.request.as_ref().unwrap().id;
    let key = key(&world, &manager, env, thread, project, machine, false).await;
    let decision = port_decision(
        PortDecisionChoice::Once,
        Some(PortLinkRecipient::Person),
        made.opening.revision,
    );
    let receipt = checked(
        decide(&world, &key.secret, approval, &decision).await,
        StatusCode::OK,
    );
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["approval_id"], json!(approval));
    assert_eq!(receipt["opening_id"], json!(made.opening.id));
    assert_eq!(receipt["state"], "approved");
    assert_eq!(receipt["link"]["recipient"], "person");
    assert!(receipt["link"]["helper_link_once"].is_string());
    assert!(receipt.get("request").is_none());
    assert!(receipt.get("title").is_none());
    assert!(receipt.get("opening_result").is_none());
    let (digest, sealed): (Option<Vec<u8>>, Option<Vec<u8>>) =
        sqlx::query_as("SELECT secret_hash,secret_sealed FROM port_openings WHERE id=$1")
            .bind(made.opening.id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert!(digest.is_some());
    assert!(sealed.is_none());
    let late = checked(
        decide(&world, &key.secret, approval, &decision).await,
        StatusCode::CONFLICT,
    );
    assert_eq!(late["code"], "approval.already_decided");
    assert!(late["current"].is_null());
    assert!(!late.to_string().contains("helper_link_once"));
}
#[tokio::test]
async fn scoped_port_grant_commits_the_native_environment_configuration_and_survives_issuer_key_revocation()
 {
    let (world, manager, _, env, project, machine) = setup().await;
    links_asked(&world).await;
    let thread = asking_thread(&world, &manager, project, env).await;
    let made = waiting(
        &world,
        &manager,
        thread,
        &link_request(&world, project, 9173),
    )
    .await;
    let approval = made.request.as_ref().unwrap().id;
    let card: Json = sqlx::query_scalar("SELECT request FROM approvals WHERE id=$1")
        .bind(approval)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        card["can_grant"], true,
        "the existing native card actually offers this grant"
    );
    let key = key(&world, &manager, env, thread, project, machine, true).await;
    let decision = port_decision(PortDecisionChoice::Grant, None, made.opening.revision);
    let shown = checked(
        decide(&world, &key.secret, approval, &decision).await,
        StatusCode::OK,
    );
    assert_eq!(shown["state"], "approved");
    assert_eq!(
        shown["opening_result"]["opening"]["admission"]["source"],
        "grant"
    );
    assert!(shown["opening_result"]["link"].is_null());
    let grants: Vec<String> =
        sqlx::query_scalar("SELECT granted_capabilities FROM environments WHERE id=$1")
            .bind(env)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert!(grants.contains(&"port_link".into()));
    let revoked = world
        .app
        .call(orch_api_contract::auth::MeTokenDelete, &[&key.id])
        .token(&manager.token)
        .send_bytes()
        .await;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT);
    checked(
        decide(&world, &key.secret, approval, &decision).await,
        StatusCode::UNAUTHORIZED,
    );
    let retained: Vec<String> =
        sqlx::query_scalar("SELECT granted_capabilities FROM environments WHERE id=$1")
            .bind(env)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(retained, grants);
    let next = world
        .app
        .call(ThreadPortRequestCreate, &[&thread])
        .token(&manager.token)
        .body(&link_request(&world, project, 9174))
        .send()
        .await
        .ok();
    assert_ne!(next.opening.state, PortOpeningState::Waiting);
    assert!(
        next.request.is_none(),
        "the accepted native configuration has no live parent-key lease"
    );
}
#[tokio::test]
async fn scoped_port_deny_receipt_ends_only_the_reviewed_opening_and_grants_nothing() {
    let (world, manager, _, env, project, machine) = setup().await;
    links_asked(&world).await;
    let thread = asking_thread(&world, &manager, project, env).await;
    let made = waiting(
        &world,
        &manager,
        thread,
        &link_request(&world, project, 9175),
    )
    .await;
    let approval = made.request.as_ref().unwrap().id;
    let key = key(&world, &manager, env, thread, project, machine, false).await;
    let decision = port_decision(PortDecisionChoice::Deny, None, made.opening.revision);
    let receipt = checked(
        decide(&world, &key.secret, approval, &decision).await,
        StatusCode::OK,
    );
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["approval_id"], json!(approval));
    assert_eq!(receipt["state"], "denied");
    assert!(receipt.get("request").is_none());
    assert!(receipt.get("opening_result").is_none());
    let state: String = sqlx::query_scalar("SELECT state FROM port_openings WHERE id=$1")
        .bind(made.opening.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(state, "refused");
    let grants: Vec<String> =
        sqlx::query_scalar("SELECT granted_capabilities FROM environments WHERE id=$1")
            .bind(env)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert!(!grants.contains(&"port_link".into()));
}
