//! Shipped templates fit the Docker driver's schemas and can be saved through the API.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::Duration;
use orch_api_contract::auth::{AdminModeSetBody, MeAdminModeSet};
use orch_api_contract::resources::{
    TemplateCheck, TemplateCheckBody, TemplateCreate, TemplateCreateBody, TemplateGet,
    TemplateUpdate, TemplateUpdateBody, UnitKind,
};
use orch_api_core::middleware::authentication::{ACCESS_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::registry::Registry;
use orch_api_core::testing::TestApp;
use orch_db::Db;
use orch_db::queries::identity::testing::insert_session;
use orch_db::queries::resources::{access_fixtures, testing};
use orch_db::seeds::{self, Release};
use orch_testkit::{clock, database::TestDb};
use orch_types::{Id, Json, new_id};
use serde_json::json;

#[tokio::test]
async fn every_shipped_template_matches_its_driver_schema_and_can_be_used() {
    let db = TestDb::fresh().await;
    let now = clock::start();
    let mut conn = db.pool.acquire().await.unwrap();
    seeds::system_defaults(&mut conn, now).await.unwrap();
    seeds::seed(
        &mut conn,
        &Release {
            version: "0.1.0",
            adapters: &[],
        },
        now,
    )
    .await
    .unwrap();
    let user = testing::insert_person_with_home(&mut conn, "operator", now)
        .await
        .unwrap();
    testing::make_system_admin(&mut conn, user).await.unwrap();
    let token = new_token(ACCESS_TOKEN_PREFIX);
    insert_session(
        &mut conn,
        new_id(),
        user,
        &token_hash(&token),
        now + Duration::days(400),
    )
    .await
    .unwrap();
    let host = access_fixtures::insert_resource(&mut conn, "host", None, None, "Node")
        .await
        .unwrap();
    let environment_schema = orch_driver_docker::descriptor::environment_schema();
    let provider_schema = orch_driver_docker::descriptor::provider_schema();
    sqlx::query("UPDATE hosts SET drivers = $2 WHERE id = $1")
        .bind(host)
        .bind(
            json!({"docker": {"name": "docker", "title": "Docker", "spec_schemas": {
                "environment": environment_schema, "provider": provider_schema
            }}}),
        )
        .execute(&mut *conn)
        .await
        .unwrap();
    let stored: Vec<(Id, String, String, String, Json)> =
        sqlx::query_as("SELECT id, name, kind, driver, spec FROM env_templates ORDER BY name")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    drop(conn);

    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = TestApp::new(Db::from_pool(db.pool.clone()), clock::manual(), registry);
    app.call(MeAdminModeSet, &[])
        .token(&token)
        .body(&AdminModeSetBody { as_admin: true })
        .send()
        .await
        .ok();

    let presets = seeds::provider_presets();
    let mut old_local = presets["Local embeddings"]["template"]["spec"].clone();
    old_local["image"] = Json::Null;
    old_local["allowed_capabilities"] = json!(["port_place", "port_host", "port_link"]);
    let refused = app
        .call(TemplateCheck, &[])
        .token(&token)
        .body(&TemplateCheckBody {
            driver: "docker".into(),
            kind: UnitKind::Provider,
            spec: old_local,
        })
        .send()
        .await
        .ok();
    assert!(
        !refused.valid,
        "the earlier invalid provider shape is refused"
    );
    assert!(!refused.errors.is_empty());
    let mut templates: Vec<(Option<Id>, String, String, String, Json)> = stored
        .into_iter()
        .map(|(id, name, kind, driver, spec)| (Some(id), name, kind, driver, spec))
        .collect();
    for (name, preset) in presets.as_object().unwrap() {
        if let Some(template) = preset.get("template") {
            templates.push((
                None,
                name.clone(),
                template["kind"].as_str().unwrap().to_owned(),
                template["driver"].as_str().unwrap().to_owned(),
                template["spec"].clone(),
            ));
        }
    }
    for required in [
        "Default",
        "Developer tools",
        "Provider runtime",
        "Local embeddings",
    ] {
        assert!(
            templates.iter().any(|(_, name, ..)| name == required),
            "{required}"
        );
    }
    for (id, name, kind, driver, spec) in templates {
        let (schema, pool) = match (driver.as_str(), kind.as_str()) {
            ("docker", "environment") => (
                orch_driver_docker::descriptor::environment_schema(),
                UnitKind::Environment,
            ),
            ("docker", "provider") => (
                orch_driver_docker::descriptor::provider_schema(),
                UnitKind::Provider,
            ),
            other => panic!("no schema for shipped template {name}: {other:?}"),
        };
        if let Err(error) = orch_plugin::schema::validate(&schema, &spec) {
            panic!("shipped template {name} does not fit its schema: {error:?}");
        }
        let checked = app
            .call(TemplateCheck, &[])
            .token(&token)
            .body(&TemplateCheckBody {
                driver,
                kind: pool,
                spec: spec.clone(),
            })
            .send()
            .await
            .ok();
        assert!(checked.valid, "{name}: {:?}", checked.errors);
        if let Some(id) = id.filter(|_| pool == UnitKind::Environment) {
            let read = app
                .call(TemplateGet, &[&id])
                .token(&token)
                .send()
                .await
                .ok();
            let mut proposed = spec;
            proposed["build"]["dockerfile"] = json!(format!(
                "{}\n# Add a project tool here\n",
                proposed["build"]["dockerfile"].as_str().unwrap()
            ));
            let saved = app
                .call(TemplateUpdate, &[&id])
                .token(&token)
                .body(&TemplateUpdateBody {
                    name: None,
                    description: None,
                    spec: Some(proposed.clone()),
                    enabled: None,
                    expected_revision: read.revision,
                })
                .send()
                .await
                .ok();
            assert_eq!(saved.spec, proposed);
        } else if name == "Local embeddings" {
            assert_eq!(spec["env"]["LLAMA_ARG_HOST"], "127.0.0.1");
            assert!(spec.get("allowed_capabilities").is_none());
            let body: TemplateCreateBody =
                serde_json::from_value(presets["Local embeddings"]["template"].clone()).unwrap();
            let saved = app
                .call(TemplateCreate, &[])
                .token(&token)
                .body(&body)
                .send()
                .await
                .ok();
            assert_eq!(saved.spec, spec);
        }
    }
}
