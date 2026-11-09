//! Native template E rows retain reported-schema validation, real administration and completed Read.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod places;
mod resources;
mod support;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::access::NewResourceAccess;
use orch_api_contract::common::{ErrorCode, Problem};
use orch_api_contract::resources::{TemplateCreate, TemplateCreateBody, UnitKind};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::{
    identity::{self, NewApiToken},
    resources::access_fixtures,
};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use support::World;
fn rule(operation: &str, kind: &str, id: Option<Id>, reach: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"allow":[{"operation":operation,"actions":actions}]})
}
async fn key(world: &World, user: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Native recipe creation",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    secret
}
fn body(home: Option<Id>, name: &str) -> TemplateCreateBody {
    TemplateCreateBody {
        name: name.into(),
        description: Some("Protected native recipe".into()),
        home_id: home,
        kind: None,
        driver: "native".into(),
        spec: json!({"image":"fixture:v1"}),
        access: Some(NewResourceAccess::OnlyMe),
    }
}
async fn schema(world: &World, c: &resources::Company) {
    let mut conn = world.conn().await;
    let host = access_fixtures::insert_resource(
        &mut conn,
        "host",
        Some(c.boss.id()),
        Some(c.acme),
        "reported-template-schema",
    )
    .await
    .unwrap();
    let object =
        json!({"type":"object","required":["image"],"properties":{"image":{"type":"string"}}});
    sqlx::query("UPDATE hosts SET drivers=$2 WHERE id=$1")
        .bind(host).bind(json!({"native":{"name":"native","version":"1.0.0","spec_schemas":{"environment":object,"provider":object}}}))
        .execute(&mut *conn).await.unwrap();
}
async fn create(world: &World, token: &str, body: &TemplateCreateBody) -> RawReply {
    world
        .app
        .call(TemplateCreate, &[])
        .token(token)
        .body(body)
        .send_bytes()
        .await
}
fn accepted(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn saved(world: &World, name: &str) -> Option<(Id, Option<Id>, Id, String, Json)> {
    sqlx::query_as("SELECT id,home_id,owner_id,xmin::text,spec FROM env_templates WHERE name=$1")
        .bind(name)
        .fetch_optional(&world.db.pool)
        .await
        .unwrap()
}
#[tokio::test]
async fn recipe_creation_manage_only_keeps_the_validated_spec_without_borrowing_get_read() {
    let world = World::new().await;
    let c = resources::company(&world).await;
    schema(&world, &c).await;
    let token = key(
        &world,
        c.boss.id(),
        vec![
            rule("TemplateCreate", "org", Some(c.acme), "self", &["manage"]),
            rule("TemplateGet", "org", Some(c.acme), "subtree", &["read"]),
        ],
    )
    .await;
    let reply = accepted(create(&world, &token, &body(Some(c.acme), "Write only recipe")).await);
    let row = saved(&world, "Write only recipe").await.unwrap();
    assert_eq!(reply, json!({"projection":"receipt","template_id":row.0}));
    assert_eq!((row.1, row.2), (Some(c.acme), c.boss.id()));
    assert_eq!(row.4, json!({"image":"fixture:v1"}));
    let mut invalid = body(Some(c.acme), "Schema still checked");
    invalid.spec = json!({"image":false});
    let reply = create(&world, &token, &invalid).await;
    let problem: Problem = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(problem.code, ErrorCode::RequestInvalid);
    assert!(saved(&world, "Schema still checked").await.is_none());
}
#[tokio::test]
async fn recipe_creation_own_subtree_read_keeps_both_unit_kinds_and_current_native_home_authority()
{
    let world = World::new().await;
    let c = resources::company(&world).await;
    schema(&world, &c).await;
    let token = key(
        &world,
        c.viewer.id(),
        vec![rule(
            "TemplateCreate",
            "org",
            Some(c.acme),
            "subtree",
            &["manage", "read"],
        )],
    )
    .await;
    for kind in [UnitKind::Environment, UnitKind::Provider] {
        let mut input = body(
            Some(c.acme),
            if kind == UnitKind::Environment {
                "Environment recipe"
            } else {
                "Provider recipe"
            },
        );
        input.kind = Some(kind);
        let full = accepted(create(&world, &token, &input).await);
        assert!(full.get("projection").is_none());
        assert_eq!(full["name"], input.name);
        assert_eq!(full["spec"], input.spec);
        assert_eq!(full["kind"], serde_json::to_value(kind).unwrap());
    }
    let outside = key(
        &world,
        c.stranger.id(),
        vec![rule(
            "TemplateCreate",
            "org",
            Some(c.acme),
            "subtree",
            &["manage", "read"],
        )],
    )
    .await;
    assert_eq!(
        create(&world, &outside, &body(Some(c.acme), "Outside recipe"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let wrong = key(
        &world,
        c.boss.id(),
        vec![rule(
            "TemplateCreate",
            "template",
            Some(c.acme),
            "self",
            &["manage", "read"],
        )],
    )
    .await;
    assert_eq!(
        create(&world, &wrong, &body(Some(c.acme), "Wrong typed home"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert!(saved(&world, "Outside recipe").await.is_none());
    assert!(saved(&world, "Wrong typed home").await.is_none());
}
#[tokio::test]
async fn restricted_recipe_driver_needs_the_same_operations_real_system_administration() {
    let world = World::new().await;
    let c = resources::company(&world).await;
    schema(&world, &c).await;
    world
        .set("templates.restricted_drivers", json!(["native"]))
        .await;
    let home = || {
        rule(
            "TemplateCreate",
            "org",
            Some(c.acme),
            "subtree",
            &["manage", "read"],
        )
    };
    let system = || rule("TemplateCreate", "system", None, "self", &["manage"]);
    let outsider = key(&world, c.boss.id(), vec![home(), system()]).await;
    assert_eq!(
        create(&world, &outsider, &body(Some(c.acme), "No admin standing"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    sqlx::query("UPDATE users SET is_system_admin=true WHERE id=$1")
        .bind(c.boss.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let home_only = key(&world, c.boss.id(), vec![home()]).await;
    assert_eq!(
        create(&world, &home_only, &body(Some(c.acme), "Org scope alone"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let unrelated = key(
        &world,
        c.boss.id(),
        vec![
            home(),
            rule("TemplateGet", "system", None, "self", &["read"]),
        ],
    )
    .await;
    assert_eq!(
        create(
            &world,
            &unrelated,
            &body(Some(c.acme), "Unrelated system read")
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    let bounded = key(&world, c.boss.id(), vec![home(), system()]).await;
    let full = accepted(
        create(
            &world,
            &bounded,
            &body(Some(c.acme), "Restricted authorized recipe"),
        )
        .await,
    );
    assert!(full.get("projection").is_none());
    assert_eq!(full["name"], "Restricted authorized recipe");
    assert_eq!(full["driver"], "native");
    assert_eq!(full["spec"], json!({"image":"fixture:v1"}));
    assert!(saved(&world, "Org scope alone").await.is_none());
    assert!(saved(&world, "No admin standing").await.is_none());
}
#[tokio::test]
async fn system_recipe_creation_uses_actual_system_admission_without_inventing_a_stored_ancestor() {
    let world = World::new().await;
    let c = resources::company(&world).await;
    schema(&world, &c).await;
    sqlx::query("UPDATE users SET is_system_admin=true WHERE id=$1")
        .bind(c.boss.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let home_only = key(
        &world,
        c.boss.id(),
        vec![rule(
            "TemplateCreate",
            "org",
            Some(c.acme),
            "subtree",
            &["manage", "read"],
        )],
    )
    .await;
    assert_eq!(
        create(&world, &home_only, &body(None, "Not an org alias"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let token = key(
        &world,
        c.boss.id(),
        vec![rule(
            "TemplateCreate",
            "system",
            None,
            "self",
            &["manage", "read"],
        )],
    )
    .await;
    let reply = accepted(create(&world, &token, &body(None, "System recipe")).await);
    let row = saved(&world, "System recipe").await.unwrap();
    assert_eq!(row.1, None);
    assert_eq!(reply, json!({"projection":"receipt","template_id":row.0}));
}
#[tokio::test]
async fn recipe_creation_rechecks_expiry_after_the_actual_home_and_root_row_wait() {
    for expire in [false, true] {
        let world = World::new().await;
        let c = resources::company(&world).await;
        schema(&world, &c).await;
        let token = key(
            &world,
            c.boss.id(),
            vec![rule(
                "TemplateCreate",
                "org",
                Some(c.acme),
                "self",
                &["manage"],
            )],
        )
        .await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
            .bind(c.acme)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let home = c.acme;
        let request = tokio::spawn(async move {
            let request = app
                .call(TemplateCreate, &[])
                .token(&token)
                .body(&body(Some(home), "Waiting native recipe"));
            request.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait,async {
            loop {
                assert!(!request.is_finished(),"the native template home footprint was not reached");
                let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                    AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%SELECT id FROM nodes WHERE id=ANY(%' AND query LIKE '%FOR SHARE%')")
                    .bind(pid).fetch_one(&world.db.pool).await.unwrap();
                if blocked {break;}
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual native home/root rows must wait");
        assert!(saved(&world, "Waiting native recipe").await.is_none());
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert!(saved(&world, "Waiting native recipe").await.is_none());
        } else {
            let row = saved(&world, "Waiting native recipe").await.unwrap();
            assert_eq!(
                accepted(reply),
                json!({"projection":"receipt","template_id":row.0})
            );
        }
    }
}
#[tokio::test]
async fn recipe_creation_publication_keeps_committed_identity_recipe_and_writer_xmin() {
    for expire in [false, true] {
        let world = World::new().await;
        let c = resources::company(&world).await;
        schema(&world, &c).await;
        let token = key(
            &world,
            c.boss.id(),
            vec![rule(
                "TemplateCreate",
                "org",
                Some(c.acme),
                "subtree",
                &["manage", "read"],
            )],
        )
        .await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let home = c.acme;
        let request = tokio::spawn(async move {
            let request = app
                .call(TemplateCreate, &[])
                .token(&token)
                .body(&body(Some(home), "Held native recipe"));
            request.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait,async {
            loop {
                assert!(!request.is_finished(),"the native completed recipe did not reach activity bookkeeping");
                let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                    AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(&world.db.pool).await.unwrap();
                if blocked {break;}
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual completed recipe must wait at activity append");
        let before = saved(&world, "Held native recipe").await.unwrap();
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = accepted(tokio::time::timeout(wait, request).await.unwrap().unwrap());
        if expire {
            assert_eq!(
                reply,
                json!({"projection":"receipt","template_id":before.0})
            );
        } else {
            assert!(reply.get("projection").is_none());
            assert_eq!(reply["spec"], before.4);
        }
        assert_eq!(saved(&world, "Held native recipe").await.unwrap(), before);
        let audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='template.created'",
        )
        .bind(before.0)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert_eq!(audits, 1);
    }
}
#[tokio::test]
async fn recipe_creation_legacy_settings_writer_keeps_its_native_generated_result() {
    let world = World::new().await;
    let c = resources::company(&world).await;
    schema(&world, &c).await;
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: c.boss.id(),
            name: "Legacy native recipe writer",
            token_hash: &token_hash(&secret),
            scopes: &["settings.manage".into()],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
    )
    .await
    .unwrap();
    let full = accepted(create(&world, &secret, &body(Some(c.acme), "Legacy native recipe")).await);
    assert!(full.get("projection").is_none());
    assert_eq!(full["name"], "Legacy native recipe");
    let row = saved(&world, "Legacy native recipe").await.unwrap();
    assert_eq!(full["id"], json!(row.0));
}

#[tokio::test]
async fn accepted_template_creation_replays_current_native_content_with_read_only_and_never_recreates()
 {
    let world = World::new().await;
    let c = resources::company(&world).await;
    schema(&world, &c).await;
    let writer = key(
        &world,
        c.boss.id(),
        vec![rule(
            "TemplateCreate",
            "org",
            Some(c.acme),
            "self",
            &["manage"],
        )],
    )
    .await;
    let input = body(Some(c.acme), "Once accepted native template");
    let act = new_id();
    let first = world
        .app
        .call(TemplateCreate, &[])
        .token(&writer)
        .header(
            orch_api_contract::common::IDEMPOTENCY_HEADER,
            &act.to_string(),
        )
        .body(&input)
        .send_bytes()
        .await;
    let first = accepted(first);
    let id: Id = serde_json::from_value(first["template_id"].clone()).unwrap();
    assert_eq!(first, json!({"projection":"receipt","template_id":id}));

    let record: Json =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(c.boss.id())
            .bind(act.to_string())
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        record["outcome"],
        json!({"kind":"template","id":id,"data":null})
    );
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE token_hash=$1")
        .bind(token_hash(&writer))
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE env_templates SET name='Current native template',revision=revision+1 WHERE id=$1",
    )
    .bind(id)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let reader = key(
        &world,
        c.boss.id(),
        vec![rule(
            "TemplateCreate",
            "template",
            Some(id),
            "self",
            &["read"],
        )],
    )
    .await;
    let full = accepted(
        world
            .app
            .call(TemplateCreate, &[])
            .token(&reader)
            .header(
                orch_api_contract::common::IDEMPOTENCY_HEADER,
                &act.to_string(),
            )
            .body(&input)
            .send_bytes()
            .await,
    );
    assert!(full.get("projection").is_none(), "{full}");
    assert_eq!(full["id"], json!(id));
    assert_eq!(full["name"], "Current native template");

    let unrelated = key(
        &world,
        c.boss.id(),
        vec![rule("TemplateGet", "template", Some(id), "self", &["read"])],
    )
    .await;
    let receipt = accepted(
        world
            .app
            .call(TemplateCreate, &[])
            .token(&unrelated)
            .header(
                orch_api_contract::common::IDEMPOTENCY_HEADER,
                &act.to_string(),
            )
            .body(&input)
            .send_bytes()
            .await,
    );
    assert_eq!(receipt, first);
    let refused = world
        .app
        .call(TemplateCreate, &[])
        .token(&reader)
        .header(
            orch_api_contract::common::IDEMPOTENCY_HEADER,
            &new_id().to_string(),
        )
        .body(&input)
        .send_bytes()
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM env_templates WHERE id=$1 OR name=$2")
            .bind(id)
            .bind(&input.name)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='template.created'",
    )
    .bind(id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(audits, 1);
}
