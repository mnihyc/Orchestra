//! Own native source roles and independent completed Skill Read.
use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity::{self, NewApiToken};
use serde_json::json;
fn home(operation: &str, home: Id, actions: &[&str]) -> serde_json::Value {
    json!({"target":{"kind":"org","id":home,"reach":"subtree"},"resources":["skill"],"allow":[{"operation":operation,"actions":actions}]})
}
fn custody(operation: &str, user: Id) -> serde_json::Value {
    json!({"target":{"kind":"account","id":user,"reach":"self"},"resources":["upload"],"allow":[{"operation":operation,"actions":["read"]}]})
}
async fn key(world: &World, user: Id, rules: Vec<serde_json::Value>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let token = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: orch_types::new_id(),
            user_id: user,
            name: "Native Skill creation",
            token_hash: &token_hash(&token),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    token
}
async fn create(
    world: &World,
    token: &str,
    body: &SkillCreateBody,
) -> orch_api_core::testing::RawReply {
    let request = world.app.call(SkillCreate, &[]).token(token).body(body);
    request.send_bytes().await
}
#[tokio::test]
async fn new_skill_requires_the_selected_home_act_and_keeps_write_only_draft_receipts() {
    let world = World::new().await;
    let c = company(&world).await;
    let body = SkillCreateBody {
        home_id: c.acme,
        from: SkillSource::New {
            name: "scope-draft".into(),
            description: "A supplied draft description.".into(),
        },
        access: Some(NewResourceAccess::OnlyMe),
    };
    for rules in [
        vec![home("SkillGet", c.acme, &["read"])],
        vec![home("SkillCreate.new", c.acme, &["read"])],
    ] {
        let token = key(&world, c.boss.id(), rules).await;
        assert_eq!(
            create(&world, &token, &body).await.status,
            http::StatusCode::FORBIDDEN
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM skills")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
    let token = key(
        &world,
        c.boss.id(),
        vec![home("SkillCreate.new", c.acme, &["manage"])],
    )
    .await;
    let reply = create(&world, &token, &body).await;
    assert_eq!(
        reply.status,
        http::StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let receipt: serde_json::Value = serde_json::from_slice(&reply.raw).unwrap();
    let id: Id = serde_json::from_value(receipt["skill_id"].clone()).unwrap();
    assert_eq!(receipt, json!({"projection":"receipt","skill_id":id}));
    let (owner, home): (Id, Id) = sqlx::query_as("SELECT owner_id,home_id FROM skills WHERE id=$1")
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!((owner, home), (c.boss.id(), c.acme));
    let files: i64 = sqlx::query_scalar("SELECT count(*) FROM skill_draft_files WHERE skill_id=$1")
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(files, 1);
}
#[tokio::test]
async fn archived_skill_uses_actual_upload_custody_and_its_own_completed_read() {
    let world = World::new().await;
    let c = company(&world).await;
    let blob = upload(
        &world,
        &c.boss,
        "scope-upload.tar.gz",
        &archive("scope-upload", "Native source bytes."),
    )
    .await;
    let body = SkillCreateBody {
        home_id: c.acme,
        from: uploaded(blob, "scope-upload.tar.gz"),
        access: Some(NewResourceAccess::OnlyMe),
    };
    for rules in [
        vec![home("SkillCreate.upload", c.acme, &["manage"])],
        vec![
            home("SkillCreate.upload", c.acme, &["manage"]),
            custody("SkillCreate.upload", c.viewer.id()),
        ],
    ] {
        let token = key(&world, c.boss.id(), rules).await;
        assert_eq!(
            create(&world, &token, &body).await.status,
            http::StatusCode::FORBIDDEN
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM skills")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
    let token = key(
        &world,
        c.boss.id(),
        vec![
            home("SkillCreate.upload", c.acme, &["manage", "read"]),
            custody("SkillCreate.upload", c.boss.id()),
        ],
    )
    .await;
    let reply = create(&world, &token, &body).await;
    assert_eq!(
        reply.status,
        http::StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let full: Skill = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(full.name, "scope-upload");
    assert_eq!(full.versions, 1);
    assert!(
        full.home.name.is_none(),
        "parent names need the operation's independently bound node Read"
    );
    let version: Id = sqlx::query_scalar("SELECT id FROM skill_versions WHERE skill_id=$1")
        .bind(full.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(full.latest.unwrap().id, version);
}
