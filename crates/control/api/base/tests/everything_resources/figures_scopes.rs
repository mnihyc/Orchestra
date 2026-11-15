//! Real native resource roots, dependency names and model-qualified impact sources.

use super::*;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::library::{
    LibraryEntry, LibraryReadBody, LibraryTarget, ResourceLibraryRead,
};
use orch_api_contract::resource_facts::{
    DependentRelation, FactTarget, ResourceImpact, ResourceImpactRead,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Json, new_id};
use serde_json::json;

fn rule(operation: &str, kind: &str, id: Id, reach: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"allow":[{"operation":operation,"actions":["read"]}]})
}
async fn key(world: &World, actor: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: actor,
            name: "Native resource figure scopes",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    secret
}
fn target(kind: ObjectKind, id: Id) -> LibraryTarget {
    LibraryTarget {
        object: ObjectRef { kind, id },
        version_id: None,
    }
}
async fn library(world: &World, token: &str, targets: &[LibraryTarget]) -> RawReply {
    world
        .app
        .call(ResourceLibraryRead, &[])
        .token(token)
        .body(&LibraryReadBody {
            targets: targets.to_vec(),
        })
        .send_bytes()
        .await
}
async fn impact(world: &World, token: &str, target: &FactTarget) -> RawReply {
    world
        .app
        .call(ResourceImpactRead, &[])
        .token(token)
        .body(target)
        .send_bytes()
        .await
}
fn accepted<T: serde::de::DeserializeOwned>(reply: RawReply) -> T {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn names(entries: &[LibraryEntry]) -> Vec<Id> {
    entries
        .iter()
        .flat_map(|entry| &entry.facts.dependents)
        .flat_map(|group| &group.visible)
        .filter_map(|edge| edge.object.map(|object| object.id))
        .collect()
}

#[tokio::test]
async fn library_figures_require_every_actual_typed_root_keep_duplicates_and_prove_real_empty_requests()
 {
    let world = World::new().await;
    let reader = person(&world, "figures_batch_reader").await;
    let outsider = person(&world, "figures_batch_outsider").await;
    let home = make_org(&world, &reader, "Figures home", "figures-home")
        .await
        .base
        .id;
    let first = resource(&world, &reader, home, "template", "first-figures-template").await;
    let second = resource(&world, &reader, home, "mount", "second-figures-mount").await;
    let inputs = vec![
        target(ObjectKind::Template, first),
        target(ObjectKind::Mount, second),
        target(ObjectKind::Template, first),
    ];
    let one = key(
        &world,
        reader.id(),
        vec![rule("ResourceLibraryRead", "template", first, "self")],
    )
    .await;
    assert_eq!(
        library(&world, &one, &inputs).await.status,
        StatusCode::FORBIDDEN
    );
    let all = key(
        &world,
        reader.id(),
        vec![
            rule("ResourceLibraryRead", "template", first, "self"),
            rule("ResourceLibraryRead", "mount", second, "self"),
        ],
    )
    .await;
    let result: Vec<LibraryEntry> = accepted(library(&world, &all, &inputs).await);
    assert_eq!(
        result
            .iter()
            .map(|entry| entry.target.clone())
            .collect::<Vec<_>>(),
        inputs
    );
    assert_eq!(result[0].facts, result[2].facts);
    let empty: Vec<LibraryEntry> = accepted(library(&world, &all, &[]).await);
    assert!(empty.is_empty());
    let missing = target(ObjectKind::Template, new_id());
    let token = key(
        &world,
        reader.id(),
        vec![rule(
            "ResourceLibraryRead",
            "template",
            missing.object.id,
            "self",
        )],
    )
    .await;
    assert_eq!(
        library(&world, &token, &[missing]).await.status,
        StatusCode::NOT_FOUND
    );
    let token = key(
        &world,
        outsider.id(),
        vec![rule("ResourceLibraryRead", "template", first, "self")],
    )
    .await;
    assert_eq!(
        library(&world, &token, &[target(ObjectKind::Template, first)])
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn library_figures_keep_intrinsic_counts_while_names_need_the_exact_consuming_operation_read()
{
    let world = World::new().await;
    let reader = person(&world, "figures_names_reader").await;
    let home = make_org(&world, &reader, "Figures names", "figures-names")
        .await
        .base
        .id;
    let template = resource(&world, &reader, home, "template", "named-figures-template").await;
    let env = resource(
        &world,
        &reader,
        home,
        "environment",
        "Protected dependent environment",
    )
    .await;
    sqlx::query("UPDATE environments SET template_id=$2 WHERE id=$1")
        .bind(env)
        .bind(template)
        .execute(&world.db.pool)
        .await
        .unwrap();
    for operation in [
        None,
        Some("ResourceImpactRead"),
        Some("ResourceLibraryRead"),
    ] {
        let mut rules = vec![rule("ResourceLibraryRead", "template", template, "self")];
        if let Some(operation) = operation {
            rules.push(rule(operation, "environment", env, "self"));
        }
        let token = key(&world, reader.id(), rules).await;
        let result: Vec<LibraryEntry> =
            accepted(library(&world, &token, &[target(ObjectKind::Template, template)]).await);
        let group = result[0]
            .facts
            .dependents
            .iter()
            .find(|group| group.relation == DependentRelation::Attachment)
            .unwrap();
        assert_eq!(group.count, 1);
        if operation == Some("ResourceLibraryRead") {
            assert_eq!(names(&result), vec![env]);
            assert_eq!(group.hidden, 0);
        } else {
            assert!(names(&result).is_empty());
            assert_eq!(group.hidden, 1);
            assert!(
                !serde_json::to_string(&result)
                    .unwrap()
                    .contains("Protected dependent environment")
            );
        }
    }
}

#[tokio::test]
async fn library_figures_owned_mcp_roots_keep_both_native_read_classes_at_the_actual_environment() {
    let world = World::new().await;
    let reader = person(&world, "figures_owned_reader").await;
    let home = make_org(
        &world,
        &reader,
        "Owned figure sources",
        "owned-figure-sources",
    )
    .await
    .base
    .id;
    let env = resource(
        &world,
        &reader,
        home,
        "environment",
        "owned-figure-environment",
    )
    .await;
    let id = new_id();
    orch_db::queries::resources::mcp::insert(
        &mut *world.conn().await,
        &orch_db::queries::resources::mcp::McpRow {
            id,
            name: "owned-figure-server".to_owned(),
            description: String::new(),
            org_id: Some(home),
            home_id: Some(home),
            owner_id: Some(reader.id()),
            machine_id: None,
            environment_id: Some(env),
            imported: false,
            spec: json!({"transport":"stdio","command":["fixture-owned-figures"],"secret_env":[]}),
            enabled: true,
            revision: 1,
        },
        world.now(),
    )
    .await
    .unwrap();
    let input = target(ObjectKind::McpServer, id);
    for family in ["mcp_server", "environment"] {
        let mut row = rule("ResourceLibraryRead", "environment", env, "self");
        row["resources"] = json!([family]);
        let token = key(&world, reader.id(), vec![row]).await;
        assert_eq!(
            library(&world, &token, std::slice::from_ref(&input))
                .await
                .status,
            StatusCode::FORBIDDEN
        );
    }
    let token = key(
        &world,
        reader.id(),
        vec![rule("ResourceLibraryRead", "environment", env, "self")],
    )
    .await;
    let result: Vec<LibraryEntry> =
        accepted(library(&world, &token, std::slice::from_ref(&input)).await);
    assert_eq!(result[0].target, input);
    let alias = key(
        &world,
        reader.id(),
        vec![rule("ResourceLibraryRead", "mcp_server", id, "self")],
    )
    .await;
    assert_eq!(
        library(&world, &alias, &[target(ObjectKind::McpServer, id)])
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn library_figures_versions_use_the_actual_skill_membership_and_keep_native_version_only_shape()
 {
    let world = World::new().await;
    let reader = person(&world, "figures_version_reader").await;
    let home = make_org(&world, &reader, "Figures versions", "figures-versions")
        .await
        .base
        .id;
    let (skill, version) =
        super::library_people_scopes::skill_version(&world, &reader, home, "figures-version-one")
            .await;
    let (_, foreign) =
        super::library_people_scopes::skill_version(&world, &reader, home, "figures-version-two")
            .await;
    let token = key(
        &world,
        reader.id(),
        vec![rule("ResourceLibraryRead", "skill", skill, "self")],
    )
    .await;
    let root = target(ObjectKind::Skill, skill);
    let pinned = LibraryTarget {
        version_id: Some(version),
        ..root.clone()
    };
    let result: Vec<LibraryEntry> = accepted(
        library(
            &world,
            &token,
            &[pinned.clone(), root.clone(), pinned.clone()],
        )
        .await,
    );
    assert_eq!(
        result
            .iter()
            .map(|entry| entry.target.clone())
            .collect::<Vec<_>>(),
        vec![pinned.clone(), root, pinned]
    );
    assert!(result[0].facts.dependents.is_empty());
    assert!(!result[0].facts.dependents_complete);
    let refused = library(
        &world,
        &token,
        &[LibraryTarget {
            version_id: Some(foreign),
            object: ObjectRef {
                kind: ObjectKind::Skill,
                id: skill,
            },
        }],
    )
    .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn resource_impact_keeps_its_native_root_and_thread_display_privacy_under_its_own_read() {
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await;
    let world = &scene.rig.world;
    let input = FactTarget {
        object: ObjectRef {
            kind: ObjectKind::Provider,
            id: scene.on.provider,
        },
        model_id: None,
    };
    for operation in [
        None,
        Some("ResourceLibraryRead"),
        Some("ResourceImpactRead"),
    ] {
        let mut rules = vec![rule(
            "ResourceImpactRead",
            "provider",
            input.object.id,
            "self",
        )];
        if let Some(operation) = operation {
            rules.push(rule(operation, "thread", thread.base.id, "self"));
        }
        let token = key(world, scene.ada.id(), rules).await;
        let result: ResourceImpact = accepted(impact(world, &token, &input).await);
        let bindings = result
            .facts
            .dependents
            .iter()
            .find(|group| group.relation == DependentRelation::Binding)
            .unwrap();
        assert_eq!(bindings.count, 1);
        if operation == Some("ResourceImpactRead") {
            assert_eq!(
                result.thread_displays.get(&thread.base.id),
                Some(&thread.display)
            );
            assert_eq!(bindings.hidden, 0);
        } else {
            assert!(result.thread_displays.is_empty());
            assert!(bindings.visible.is_empty());
            assert_eq!(bindings.hidden, 1);
        }
    }
    let outsider = key(
        world,
        scene.bea.id(),
        vec![rule(
            "ResourceImpactRead",
            "provider",
            input.object.id,
            "self",
        )],
    )
    .await;
    assert_eq!(
        impact(world, &outsider, &input).await.status,
        StatusCode::NOT_FOUND
    );
}

async fn model(world: &World, provider: Id, name: &str, by: Id) -> Id {
    let id = new_id();
    sqlx::query("INSERT INTO provider_models(id,provider_id,model_name,display_name,parameters,enabled,source,created_by) VALUES($1,$2,$3,$3,'{}',true,'manual',$4)")
        .bind(id).bind(provider).bind(name).bind(by).execute(&world.db.pool).await.unwrap();
    id
}

#[tokio::test]
async fn resource_impact_uses_exact_current_provider_model_membership_instead_of_an_id_alias() {
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await;
    let world = &scene.rig.world;
    let current: Id = sqlx::query_scalar(
        "SELECT id FROM provider_models WHERE provider_id=$1 ORDER BY id LIMIT 1",
    )
    .bind(scene.on.provider)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    let alternate = model(
        world,
        scene.on.provider,
        "alternate-figures-model",
        scene.ada.id(),
    )
    .await;
    let token = key(
        world,
        scene.ada.id(),
        vec![
            rule("ResourceImpactRead", "provider", scene.on.provider, "self"),
            rule("ResourceImpactRead", "thread", thread.base.id, "self"),
        ],
    )
    .await;
    let input = FactTarget {
        object: ObjectRef {
            kind: ObjectKind::Provider,
            id: scene.on.provider,
        },
        model_id: Some(current),
    };
    let before: ResourceImpact = accepted(impact(world, &token, &input).await);
    assert!(before.thread_displays.contains_key(&thread.base.id));
    let other: ResourceImpact = accepted(
        impact(
            world,
            &token,
            &FactTarget {
                model_id: Some(alternate),
                ..input.clone()
            },
        )
        .await,
    );
    assert!(other.thread_displays.is_empty());
    let provider = resource(
        world,
        &scene.ada,
        scene.tree.org.base.id,
        "provider",
        "foreign-model-provider",
    )
    .await;
    let foreign = model(world, provider, "foreign-figures-model", scene.ada.id()).await;
    assert_eq!(
        impact(
            world,
            &token,
            &FactTarget {
                model_id: Some(foreign),
                ..input.clone()
            }
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
}

async fn activity_wait(world: &World, pid: i32, request: &tokio::task::JoinHandle<RawReply>) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async { loop {
        assert!(!request.is_finished(), "the native figures result must reach actual activity bookkeeping");
        let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
            AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
            .bind(pid).fetch_one(&world.db.pool).await.unwrap();
        if blocked { break; } tokio::task::yield_now().await;
    }}).await.expect("the actual final figures activity INSERT must wait on the exact backend");
}

#[tokio::test]
async fn library_figures_publication_rechecks_the_original_named_dependency_and_current_governor() {
    for change in ["unchanged", "expired", "detached", "read_lost"] {
        let world = World::new().await;
        let reader = person(&world, "figures_held_reader").await;
        let home = make_org(
            &world,
            &reader,
            "Held figure sources",
            "held-figure-sources",
        )
        .await
        .base
        .id;
        let template = resource(&world, &reader, home, "template", "held-figures-template").await;
        let other = resource(&world, &reader, home, "template", "other-figures-template").await;
        let env = resource(
            &world,
            &reader,
            home,
            "environment",
            "Held named figure dependency",
        )
        .await;
        sqlx::query("UPDATE environments SET template_id=$2 WHERE id=$1")
            .bind(env)
            .bind(template)
            .execute(&world.db.pool)
            .await
            .unwrap();
        let token = key(
            &world,
            reader.id(),
            vec![
                rule("ResourceLibraryRead", "template", template, "self"),
                rule("ResourceLibraryRead", "template", other, "self"),
                rule("ResourceLibraryRead", "org", home, "subtree"),
            ],
        )
        .await;
        let input = LibraryReadBody {
            targets: vec![target(ObjectKind::Template, template)],
        };
        let control: Vec<LibraryEntry> = accepted(library(&world, &token, &input.targets).await);
        assert_eq!(names(&control), vec![env]);
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
        let request_token = token.clone();
        let request_input = input.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(ResourceLibraryRead, &[])
                .token(&request_token)
                .body(&request_input);
            request.send_bytes().await
        });
        activity_wait(&world, pid, &request).await;
        if change == "expired" {
            world.advance(chrono::Duration::hours(2));
        }
        if change == "detached" {
            sqlx::query("UPDATE environments SET template_id=$2 WHERE id=$1")
                .bind(env)
                .bind(other)
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
        if change == "read_lost" {
            sqlx::query("UPDATE environments SET home_node_id=$2,org_id=$2 WHERE id=$1")
                .bind(env)
                .bind(reader.person.personal_org)
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        if change == "unchanged" {
            let result: Vec<LibraryEntry> = accepted(reply);
            assert_eq!(result, control);
        } else {
            assert_eq!(
                reply.status,
                if change == "expired" {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::FORBIDDEN
                }
            );
            assert!(!String::from_utf8_lossy(&reply.raw).contains("Held named figure dependency"));
        }
        if change == "detached" {
            let current: Vec<LibraryEntry> =
                accepted(library(&world, &token, &[target(ObjectKind::Template, other)]).await);
            assert_eq!(names(&current), vec![env]);
        }
        if change == "read_lost" {
            let current: Vec<LibraryEntry> =
                accepted(library(&world, &token, &input.targets).await);
            assert!(names(&current).is_empty());
        }
    }
}

#[tokio::test]
async fn resource_impact_publication_rechecks_the_original_model_qualified_binding_after_activity_wait()
 {
    for change in ["unchanged", "expired", "different_model"] {
        let scene = rig::Scene::new().await;
        let thread = scene.thread().await;
        let world = &scene.rig.world;
        let current: Id = sqlx::query_scalar(
            "SELECT id FROM provider_models WHERE provider_id=$1 ORDER BY id LIMIT 1",
        )
        .bind(scene.on.provider)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        let alternate = model(
            world,
            scene.on.provider,
            "held-alternate-figures-model",
            scene.ada.id(),
        )
        .await;
        let token = key(
            world,
            scene.ada.id(),
            vec![
                rule("ResourceImpactRead", "provider", scene.on.provider, "self"),
                rule("ResourceImpactRead", "thread", thread.base.id, "self"),
            ],
        )
        .await;
        let input = FactTarget {
            object: ObjectRef {
                kind: ObjectKind::Provider,
                id: scene.on.provider,
            },
            model_id: Some(current),
        };
        let control: ResourceImpact = accepted(impact(world, &token, &input).await);
        assert!(control.thread_displays.contains_key(&thread.base.id));
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
        let request_token = token.clone();
        let request_input = input.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(ResourceImpactRead, &[])
                .token(&request_token)
                .body(&request_input);
            request.send_bytes().await
        });
        activity_wait(world, pid, &request).await;
        if change == "expired" {
            world.advance(chrono::Duration::hours(2));
        }
        if change == "different_model" {
            world
                .set_at(
                    orch_settings::Target::Node(thread.base.id),
                    "agent.model",
                    json!("held-alternate-figures-model"),
                )
                .await;
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        if change == "unchanged" {
            let result: ResourceImpact = accepted(reply);
            assert_eq!(result, control);
        } else {
            assert_eq!(
                reply.status,
                if change == "expired" {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::FORBIDDEN
                }
            );
        }
        if change == "different_model" {
            let current: ResourceImpact = accepted(
                impact(
                    world,
                    &token,
                    &FactTarget {
                        model_id: Some(alternate),
                        ..input
                    },
                )
                .await,
            );
            assert!(current.thread_displays.contains_key(&thread.base.id));
        }
    }
}

#[tokio::test]
async fn a_resource_list_carries_sharing_only_from_homes_readable_in_that_same_inventory() {
    let world = World::new().await;
    let owner = person(&world, "sharing-inventory-reader").await;
    let home = make_org(
        &world,
        &owner,
        "Sharing inventory home",
        "sharing-inventory-home",
    )
    .await
    .base
    .id;
    let provider = resource(&world, &owner, home, "provider", "Listed with sharing").await;
    let ask = EverythingListQuery {
        resource_details: None,
        resource_rows: Some(true),
        limit: Some(20),
        ..query("provider")
    };
    let ordinary = read(&world, &owner, &ask).await;
    assert!(
        ordinary
            .items
            .iter()
            .filter_map(|row| row.sharing.as_ref())
            .any(|summary| summary.rows.is_some())
    );
    for operation in ["NodeDefaultsGet", "EverythingList.mine"] {
        let token = key(
            &world,
            owner.id(),
            vec![
                rule("EverythingList.mine", "provider", provider, "self"),
                rule(operation, "org", home, "self"),
            ],
        )
        .await;
        let page: Page<EverythingRow> = accepted(
            world
                .app
                .call(EverythingList, &[])
                .token(&token)
                .query(&ask)
                .send_bytes()
                .await,
        );
        assert_eq!(page.items.len(), 1);
        assert!(page.items[0].resource_row.is_some() && page.items[0].resource.is_none());
        let disclosed = page
            .items
            .iter()
            .filter_map(|row| row.sharing.as_ref())
            .any(|summary| summary.rows.is_some());
        assert_eq!(
            disclosed,
            operation == "EverythingList.mine",
            "another operation never lends its sharing read to the inventory"
        );
    }
}
