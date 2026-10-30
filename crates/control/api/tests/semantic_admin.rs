//! Search by meaning in Management: where each org's index stands, for system admins alone, and the provider Local embeddings made at home in an org from its template, with its one model offered and the value that names it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod admin_support;
mod places;
mod support;

use admin_support::{assert_no_system_admin, cast, refused_to};
use orch_api_contract::admin::{
    AdminSemanticSearchGet, AdminSemanticSearchKeep, AdminSettingSet, AdminSettingSetBody,
    SemanticSearchKeepBody, SemanticState,
};
use orch_api_contract::resources::*;
use orch_api_contract::settings::{NodeSettingSet, NodeSettingSetBody};
use orch_types::Json;
use serde_json::json;
use support::World;

#[tokio::test]
async fn only_a_system_admin_reads_where_search_by_meaning_stands_and_an_org_is_listed_once_it_is_on()
 {
    let world = World::new().await;
    let c = cast(&world).await;
    let shipped = world
        .app
        .call(AdminSemanticSearchGet, &[])
        .token(&c.root.token)
        .send()
        .await
        .ok();
    assert!(
        shipped.orgs.iter().all(|org| org.org.id != c.acme),
        "off as shipped, and so not listed"
    );
    assert_eq!(shipped.interval_secs, 10);
    world
        .app
        .call(NodeSettingSet, &[&c.acme, &"search.semantic.enabled"])
        .token(&c.boss.token)
        .body(&NodeSettingSetBody {
            value: json!(true),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    let on = world
        .app
        .call(AdminSemanticSearchGet, &[])
        .token(&c.root.token)
        .send()
        .await
        .ok();
    let acme = on.orgs.iter().find(|org| org.org.id == c.acme).unwrap();
    assert!(acme.enabled);
    assert_eq!(acme.state, SemanticState::NotNamed, "on, and nothing named");
    assert_eq!(acme.org.name.as_deref(), Some("Acme"));
    let refused = refused_to(&world, AdminSemanticSearchGet, &[], &c.boss, None, "").await;
    assert_no_system_admin(&refused, "the overview of search by meaning");
}

#[tokio::test]
async fn local_embeddings_uses_ordinary_editable_removable_templates_and_provider_creation_does_not_name_search()
 {
    let world = World::new().await;
    let c = cast(&world).await;
    places::join(&world, c.acme, c.acme, &c.root, "admin", &c.boss).await;
    let mut conn = world.conn().await;
    let host = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "host",
        Some(c.root.id()),
        Some(c.acme),
        "schema-host",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE hosts SET drivers=$2 WHERE id=$1").bind(host).bind(json!({"docker":{"name":"docker","version":"1","spec_schemas":{"provider":{"type":"object"}}}})).execute(&mut *conn).await.unwrap();
    drop(conn);
    let setup = world
        .app
        .call(ProviderSetupGet, &[])
        .query(&ProviderSetupGetQuery {
            org_id: c.acme,
            provider_id: None,
        })
        .token(&c.root.token)
        .send()
        .await
        .ok();
    let preset = setup
        .presets
        .iter()
        .find(|p| p.name == "Local embeddings")
        .unwrap();
    assert_eq!(preset.upstreams[0].protocol, "openai-embeddings");
    assert_eq!(preset.dimensions, Some(256));
    let recipe = preset.template.clone().unwrap();
    assert_eq!(recipe.home_id, Some(c.acme));
    let first = world
        .app
        .call(TemplateCreate, &[])
        .token(&c.root.token)
        .body(&recipe)
        .send()
        .await
        .ok();
    assert_eq!(first.owner.as_ref().unwrap().id, c.root.id());
    assert_eq!(first.kind, UnitKind::Provider);
    let listed = world
        .app
        .call(TemplateList, &[])
        .query(&TemplateListQuery {
            kind: Some(UnitKind::Provider),
            org_id: Some(c.acme),
        })
        .token(&c.root.token)
        .send()
        .await
        .ok();
    assert!(
        listed
            .iter()
            .any(|row| serde_json::to_value(row).unwrap()["id"] == json!(first.id)),
        "the ordinary template list contains Local embeddings"
    );
    assert!(
        first.spec["build"]["dockerfile"]
            .as_str()
            .unwrap()
            .contains("embeddinggemma-2")
    );
    let mut edited = first.spec.clone();
    edited["env"]["LLAMA_ARG_THREADS"] = json!("2");
    edited["build"]["dockerfile"] = json!(format!(
        "{}\n# operator model recipe\n",
        first.spec["build"]["dockerfile"].as_str().unwrap()
    ));
    let edited = world
        .app
        .call(TemplateUpdate, &[&first.id])
        .token(&c.root.token)
        .body(&TemplateUpdateBody {
            name: None,
            description: None,
            spec: Some(edited),
            enabled: None,
            expected_revision: first.revision,
        })
        .send()
        .await
        .ok();
    assert_eq!(edited.spec["env"]["LLAMA_ARG_THREADS"], "2");
    assert!(
        edited.spec["build"]["dockerfile"]
            .as_str()
            .unwrap()
            .contains("# operator model recipe")
    );
    world
        .app
        .call(TemplateDelete, &[&first.id])
        .token(&c.root.token)
        .send()
        .await
        .ok();
    let live = world
        .app
        .call(TemplateList, &[])
        .query(&TemplateListQuery {
            kind: Some(UnitKind::Provider),
            org_id: Some(c.acme),
        })
        .token(&c.root.token)
        .send()
        .await
        .ok();
    assert!(
        !live
            .iter()
            .any(|row| serde_json::to_value(row).unwrap()["id"] == json!(first.id))
    );
    let second = world
        .app
        .call(TemplateCreate, &[])
        .token(&c.root.token)
        .body(&recipe)
        .send()
        .await
        .ok();
    assert_ne!(first.id, second.id, "removal never resurrects an identity");
    let made = world
        .app
        .call(ProviderCreate, &[])
        .token(&c.root.token)
        .body(&CreateProvider::Api {
            runtime_machine_id: None,
            name: "Local embeddings".into(),
            home_id: c.acme,
            access: None,
            arrangements: None,
            runtime_config: Some(json!({"runtime.template_id":second.id})),
            upstreams: preset.upstreams.clone(),
            api_key: None,
            secrets: None,
            key_machine_id: None,
            sources: None,
        })
        .send()
        .await
        .ok();
    let id = made.provider.base.id;
    let model = world
        .app
        .call(ProviderModelCreate, &[&id])
        .token(&c.root.token)
        .body(&preset.model.clone().unwrap())
        .send()
        .await
        .ok();
    assert_eq!(model.model_name, "embeddinggemma-2");
    let shown = world
        .app
        .call(ProviderGet, &[&id])
        .token(&c.root.token)
        .send()
        .await
        .ok();
    assert_eq!(shown.base.status, ProviderStatus::Ready);
    assert_eq!(
        shown.runtime_config["runtime.template_id"],
        json!(second.id)
    );
    let mut conn = world.conn().await;
    let before_runtime = orch_db::queries::semantic::endpoint_fingerprint(&mut conn, id)
        .await
        .unwrap();
    let runtime = orch_types::new_id();
    sqlx::query("INSERT INTO runtimes(id,kind,host_id,unit_name,provider_id,driver,template_id,spec_hash,state)
       VALUES($1,'provider',$2,'local-embedding-proof',$3,'docker',$4,$5,'running')")
       .bind(runtime).bind(host).bind(id).bind(second.id).bind(orch_types::Hash::of(b"original-model").as_bytes().to_vec())
       .execute(&mut *conn).await.unwrap();
    let applied = orch_db::queries::semantic::endpoint_fingerprint(&mut conn, id)
        .await
        .unwrap();
    assert_ne!(before_runtime, applied);
    sqlx::query("UPDATE providers SET name='Renamed embeddings',revision=revision+1 WHERE id=$1")
        .bind(id)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        orch_db::queries::semantic::endpoint_fingerprint(&mut conn, id)
            .await
            .unwrap(),
        applied,
        "a label does not change the model"
    );
    sqlx::query("UPDATE runtimes SET spec_hash=$2 WHERE id=$1")
        .bind(runtime)
        .bind(orch_types::Hash::of(b"changed-model").as_bytes().to_vec())
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_ne!(
        orch_db::queries::semantic::endpoint_fingerprint(&mut conn, id)
            .await
            .unwrap(),
        applied,
        "an applied Dockerfile/model change must recheck the stored probes"
    );
    drop(conn);
    let used = world
        .app
        .call(TemplateDelete, &[&second.id])
        .token(&c.root.token)
        .send()
        .await
        .problem();
    assert_eq!(
        used.code,
        orch_api_contract::common::ErrorCode::ResourceInUse,
        "a used template follows the same removal rule as every template"
    );
    let named: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM settings WHERE node_id=$1 AND key='search.semantic.embeddings'",
    )
    .bind(c.acme)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        named, 0,
        "choosing a search provider is a separate ordinary setting write"
    );
}

#[tokio::test]
async fn keeping_the_current_model_restores_its_prompts_and_chunk_size_together_and_respects_locks_and_a_changed_generation()
 {
    let world = World::new().await;
    let c = cast(&world).await;
    places::join(&world, c.acme, c.acme, &c.root, "admin", &c.boss).await;
    let generation = orch_types::new_id();
    let provider = orch_types::new_id();
    sqlx::query("INSERT INTO search_generations (id,org_id,provider_id,model_name,asked_dimensions,dimensions,query_prefix,document_prefix,chunking,state,created_at)
        VALUES ($1,$2,$3,'old-model',256,256,'query: ','text: ','parts-v1:64','active',$4)")
        .bind(generation).bind(c.acme).bind(provider).bind(world.now()).execute(&world.db.pool).await.unwrap();
    let named = json!({"provider_id":provider,"model_name":"new-model","dimensions":128});
    world
        .app
        .call(NodeSettingSet, &[&c.acme, &"search.semantic.embeddings"])
        .token(&c.boss.token)
        .body(&NodeSettingSetBody {
            value: named.clone(),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    let stale = world
        .app
        .call(AdminSemanticSearchKeep, &[&c.acme])
        .token(&c.root.token)
        .body(&SemanticSearchKeepBody {
            generation_id: orch_types::new_id(),
        })
        .send()
        .await
        .problem();
    assert_eq!(
        stale.code,
        orch_api_contract::common::ErrorCode::ConflictStaleRevision
    );
    let kept = world
        .app
        .call(AdminSemanticSearchKeep, &[&c.acme])
        .token(&c.root.token)
        .body(&SemanticSearchKeepBody {
            generation_id: generation,
        })
        .send()
        .await
        .ok();
    assert_eq!(kept.active.unwrap().id, generation);
    let mut conn = world.conn().await;
    let chosen = orch_engine::semantic::choice(&mut conn, c.acme)
        .await
        .unwrap();
    assert_eq!(chosen.tuning.chunk_tokens, 64);
    let old = chosen.embeddings.unwrap();
    assert_eq!(old.model_name, "old-model");
    assert_eq!(old.dimensions, Some(256));
    assert_eq!(old.query_prefix, "query: ");
    assert_eq!(old.document_prefix, "text: ");
    let mut locked = orch_settings::get::<Json>(
        &mut conn,
        orch_settings::Place::Node(c.acme),
        "search.semantic.tuning",
    )
    .await
    .unwrap()
    .unwrap();
    locked["chunk_tokens"] = json!(128);
    drop(conn);
    world
        .app
        .call(NodeSettingSet, &[&c.acme, &"search.semantic.embeddings"])
        .token(&c.boss.token)
        .body(&NodeSettingSetBody {
            value: named.clone(),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    world
        .app
        .call(AdminSettingSet, &[&"search.semantic.tuning"])
        .token(&c.root.token)
        .body(&AdminSettingSetBody {
            value: locked,
            locked: Some(true),
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    let refused = world
        .app
        .call(AdminSemanticSearchKeep, &[&c.acme])
        .token(&c.root.token)
        .body(&SemanticSearchKeepBody {
            generation_id: generation,
        })
        .send()
        .await
        .problem();
    assert_eq!(
        refused.code,
        orch_api_contract::common::ErrorCode::AccessLocked,
        "a lock cannot be bypassed by keeping the old model"
    );
    let now: Json = sqlx::query_scalar(
        "SELECT value FROM settings WHERE node_id=$1 AND key='search.semantic.embeddings'",
    )
    .bind(c.acme)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        now, named,
        "the refused tuning write also rolls back the model write"
    );
    let refused = refused_to(
        &world,
        AdminSemanticSearchKeep,
        &[&c.acme],
        &c.boss,
        Some(json!({"generation_id":generation})),
        "",
    )
    .await;
    assert_no_system_admin(&refused, "keeping the current model");
}
