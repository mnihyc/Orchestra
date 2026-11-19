//! Fresh provider defaults are usable system data, and restarting never replaces a template or access an operator customized.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use chrono::Duration;
use orch_api_contract::resources::{
    TemplateGet, TemplateList, TemplateListQuery, TemplateUpdate, TemplateUpdateBody, UnitKind,
};
use orch_db::seeds::{self, BuiltInAdapter, PROVIDER_RUNTIME_TEMPLATE_ID, Release, Seeded};
use serde_json::json;
use support::World;

const ADAPTERS: &[BuiltInAdapter<'static>] = &[
    BuiltInAdapter {
        kind: "codex",
        title: "Codex",
        colocated: true,
    },
    BuiltInAdapter {
        kind: "claude-code",
        title: "Claude Code",
        colocated: true,
    },
];

async fn start(world: &World) -> Seeded {
    let mut transaction = world.db.pool.begin().await.unwrap();
    let made = seeds::seed(
        &mut transaction,
        &Release {
            version: "0.1.0",
            adapters: ADAPTERS,
        },
        world.now(),
    )
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    made
}

#[tokio::test]
async fn a_fresh_system_offers_its_provider_runtime_without_a_substitute_template() {
    let world = World::new().await;
    assert_eq!(start(&world).await.templates, 3);
    let user = places::person(&world, "member").await;
    let offered = world
        .app
        .call(TemplateList, &[])
        .token(&user.token)
        .query(&TemplateListQuery {
            kind: Some(UnitKind::Provider),
            ..TemplateListQuery::default()
        })
        .send()
        .await
        .ok();
    let offered: Vec<_> = offered
        .into_iter()
        .filter_map(orch_api_contract::common::Listed::into_visible)
        .collect();
    assert_eq!(offered.len(), 1);
    assert_eq!(offered[0].id, PROVIDER_RUNTIME_TEMPLATE_ID);
    assert!(offered[0].enabled);
    assert!(
        offered[0]
            .access
            .permissions
            .iter()
            .any(|p| p == "template.use")
    );
    let read = world
        .app
        .call(TemplateGet, &[&PROVIDER_RUNTIME_TEMPLATE_ID])
        .token(&user.token)
        .send()
        .await
        .ok();
    assert_eq!(read, offered[0]);
    world.advance(Duration::days(1));
    assert!(start(&world).await.nothing());
    let restarted = world
        .app
        .call(TemplateGet, &[&PROVIDER_RUNTIME_TEMPLATE_ID])
        .token(&user.token)
        .send()
        .await
        .ok();
    assert_eq!(restarted, read);
}

#[tokio::test]
async fn restarting_preserves_the_operators_provider_image_limits_and_use_row() {
    let world = World::new().await;
    start(&world).await;
    let admin = places::admin(&world, "operator").await;
    // A template's configuration is checked against its driver's schema as a host reports it.
    {
        let mut conn = world.conn().await;
        let host = orch_db::queries::resources::access_fixtures::insert_resource(
            &mut conn, "host", None, None, "Node",
        )
        .await
        .unwrap();
        sqlx::query("UPDATE hosts SET drivers = $2 WHERE id = $1")
            .bind(host)
            .bind(json!({"docker": {"name": "docker", "title": "Docker",
                "spec_schemas": {"environment": {}, "provider": {}}}}))
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let changed = world
        .app
        .call(TemplateUpdate, &[&PROVIDER_RUNTIME_TEMPLATE_ID])
        .token(&admin.token)
        .body(&TemplateUpdateBody {
            name: Some("Our provider runtime".to_owned()),
            description: Some("An operator's own runtime".to_owned()),
            spec: Some(json!({
                "image": "registry.example/our-provider:stable",
                "resources": { "cpu_millis": 4000, "memory_mb": 4096, "pids": 1024 }
            })),
            enabled: Some(false),
            expected_revision: 1,
        })
        .send()
        .await
        .ok();
    sqlx::query(
        "UPDATE permission_entries SET removed_at = $1
         WHERE object_kind = 'template' AND object_id = $2 AND permission = 'template.use'",
    )
    .bind(world.now())
    .bind(PROVIDER_RUNTIME_TEMPLATE_ID)
    .execute(&world.db.pool)
    .await
    .unwrap();
    world.advance(Duration::days(1));
    assert!(start(&world).await.nothing());
    let restarted = world
        .app
        .call(TemplateGet, &[&PROVIDER_RUNTIME_TEMPLATE_ID])
        .token(&admin.token)
        .send()
        .await
        .ok();
    assert_eq!(restarted.name, changed.name);
    assert_eq!(restarted.description, changed.description);
    assert_eq!(restarted.spec, changed.spec);
    assert_eq!(restarted.revision, changed.revision);
    assert_eq!(restarted.enabled, changed.enabled);
    let use_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM permission_entries WHERE object_kind = 'template'
         AND object_id = $1 AND permission = 'template.use' AND removed_at IS NULL",
    )
    .bind(PROVIDER_RUNTIME_TEMPLATE_ID)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        use_rows, 0,
        "a restart must not widen the operator's access"
    );
}
