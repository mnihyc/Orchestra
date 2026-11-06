//! Audited resource operations retain their normal readers, permissions and writers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod resources;
mod support;

use http::StatusCode;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::resources::{
    ProviderModelList, ProviderPlanGet, ProviderPlanGetQuery, ProviderSecretSet,
    ProviderSecretSetBody, TemplateDelete, TemplateGet,
};
use orch_api_contract::skills::{
    SkillDelete, SkillDraftDelete, SkillGet, SkillVersionGet, SkillVersionList,
    SkillVersionListQuery,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::resources::{access_fixtures as fixtures, routes, skills};
use orch_db::queries::{identity, settings, storage};
use orch_types::{Hash, Id, Json, Time, new_id};
use serde_json::json;
use support::World;

#[test]
fn canonical_resource_cohort_has_exact_kinds_and_optional_mutation_disclosure() {
    use orch_access_contract::key_scope::{KeyAction as Action, KeyTargetKind};
    use orch_api_contract::operations::{FacetUsage, TargetResolver, catalogue};

    // The full canonical registry must remain valid, including public/protocol empty replies.
    let operations = catalogue().unwrap();
    for (name, kind, action) in [
        ("ProviderPlanGet", ResourceKind::Provider, Action::Read),
        ("ProviderModelList", ResourceKind::Provider, Action::Read),
        ("ProviderSecretSet", ResourceKind::Provider, Action::Manage),
        ("TemplateGet", ResourceKind::Template, Action::Read),
        ("TemplateDelete", ResourceKind::Template, Action::Manage),
        ("SkillGet", ResourceKind::Skill, Action::Read),
        ("SkillVersionList", ResourceKind::Skill, Action::Read),
        ("SkillVersionGet", ResourceKind::Skill, Action::Read),
        ("SkillDelete", ResourceKind::Skill, Action::Manage),
        ("SkillDraftDelete", ResourceKind::Skill, Action::Manage),
    ] {
        let operation = operations
            .iter()
            .find(|entry| entry.operation_id == name)
            .unwrap();
        let [variant] = operation.variants.as_slice() else {
            panic!("one direct selection")
        };
        assert_eq!(variant.operation, name);
        let target = TargetResolver::TypedPath {
            parameter: "id",
            kind: KeyTargetKind::Object(kind.into()),
        };
        assert!(variant.facets.iter().all(|facet| facet.target == target));
        assert_eq!(variant.facets[0].action, action);
        assert_eq!(variant.facets[0].usage, FacetUsage::Required);
        if action == Action::Manage {
            assert!(operation.receipt);
            assert_eq!(variant.facets.len(), 2);
            assert_eq!(variant.facets[1].action, Action::Read);
            assert_eq!(variant.facets[1].usage, FacetUsage::ResponseContent);
        } else {
            assert_eq!(variant.facets.len(), 1);
        }
    }
}

async fn key(world: &World, user: Id, kind: &str, id: Id, allow: Json) -> String {
    let scope: orch_access_contract::key_scope::KeyScope = serde_json::from_value(json!({
        "rules":[{"target":{"kind":kind,"id":id,"reach":"self"},"allow":allow}]
    }))
    .unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let scope = serde_json::to_value(scope).unwrap();
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Resource control",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: world.now(),
            // A one-hour fixture lifetime fits the existing place ceiling without moving time.
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&scope),
    )
    .await
    .unwrap();
    secret
}

async fn resource(
    world: &World,
    company: &resources::Company,
    kind: ResourceKind,
    name: &str,
) -> Id {
    let mut conn = world.conn().await;
    let id = fixtures::insert_resource(
        &mut conn,
        kind.as_str(),
        Some(company.boss.id()),
        Some(company.acme),
        name,
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: kind.into(),
            id,
        },
        company.boss.id(),
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn provider_readers_and_sealed_write_obey_the_same_provider_boundary() {
    let world = World::new().await;
    orch_api_core::routes::resources::sealing::install_master_key(
        orch_db::secrets::MasterKey::new([7; 32]),
    );
    let company = resources::company(&world).await;
    let a = resource(&world, &company, ResourceKind::Provider, "A").await;
    let b = resource(&world, &company, ResourceKind::Provider, "B").await;
    fixtures::insert_model(
        &mut *world.conn().await,
        a,
        "model-a",
        &[],
        true,
        company.boss.id(),
    )
    .await
    .unwrap();
    let read = key(
        &world,
        company.boss.id(),
        "provider",
        a,
        json!([
            {"operation":"ProviderModelList","actions":["read"]},
            {"operation":"ProviderPlanGet","actions":["read"]}
        ]),
    )
    .await;
    let models = world
        .app
        .call(ProviderModelList, &[&a])
        .token(&read)
        .send()
        .await
        .ok();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].model_name, "model-a");
    let date = world.now().date_naive().to_string();
    let plan = world
        .app
        .call(ProviderPlanGet, &[&a])
        .token(&read)
        .query(&ProviderPlanGetQuery {
            metric: None,
            from: date.clone(),
            to: date,
            bucket: None,
            limit: None,
            cursor: None,
        })
        .send()
        .await
        .ok();
    assert!(plan.items.is_empty());
    assert_eq!(
        world
            .app
            .call(ProviderModelList, &[&b])
            .token(&read)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let powerless = key(
        &world,
        company.viewer.id(),
        "provider",
        a,
        json!([{"operation":"ProviderModelList","actions":["read"]}]),
    )
    .await;
    assert_eq!(
        world
            .app
            .call(ProviderModelList, &[&a])
            .token(&powerless)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );

    let mut conn = world.conn().await;
    let row = routes::providers(&mut conn, &[a])
        .await
        .unwrap()
        .pop()
        .unwrap();
    let upstreams = json!([{"protocol":"openai-responses","base_url":"https://api.example.test",
        "options":{"auth":"none","secret_headers":["X-Credential"]}}]);
    assert!(
        routes::change_provider(
            &mut conn,
            a,
            row.revision,
            &routes::ProviderChange {
                upstreams: Some((&upstreams, &["openai-responses".into()])),
                ..Default::default()
            },
            world.now()
        )
        .await
        .unwrap()
    );
    drop(conn);
    let manage = key(
        &world,
        company.boss.id(),
        "provider",
        a,
        json!([{"operation":"ProviderSecretSet","actions":["manage"]}]),
    )
    .await;
    let result = world
        .app
        .call(ProviderSecretSet, &[&a, &"X-Credential"])
        .token(&manage)
        .body(&ProviderSecretSetBody {
            value: "private credential".into(),
        })
        .send()
        .await;
    assert_eq!(result.status, StatusCode::NO_CONTENT);
    assert!(result.raw.is_empty());
    assert!(
        settings::secret(
            &mut *world.conn().await,
            "provider",
            a,
            &orch_api_core::routes::resources::sealing::header_secret("X-Credential")
        )
        .await
        .unwrap()
        .is_some()
    );
    assert_eq!(
        world
            .app
            .call(ProviderModelList, &[&a])
            .token(&manage)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN,
        "setting a secret grants no model readback"
    );
}

#[tokio::test]
async fn template_control_has_an_empty_receipt_and_does_not_grant_ordinary_management() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let a = resource(&world, &company, ResourceKind::Template, "Private template").await;
    let b = resource(&world, &company, ResourceKind::Template, "Other template").await;
    let read = key(
        &world,
        company.boss.id(),
        "template",
        a,
        json!([{"operation":"TemplateGet","actions":["read"]}]),
    )
    .await;
    assert_eq!(
        world
            .app
            .call(TemplateGet, &[&a])
            .token(&read)
            .send()
            .await
            .ok()
            .id,
        a
    );
    let allow = json!([{"operation":"TemplateDelete","actions":["manage"]}]);
    let powerless = key(&world, company.viewer.id(), "template", a, allow.clone()).await;
    let denied = world
        .app
        .call(TemplateDelete, &[&a])
        .token(&powerless)
        .send()
        .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    let problem = denied.problem();
    assert_eq!(
        problem.code,
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
    assert!(!problem.extra.contains_key("missing"));
    assert!(!problem.extra.contains_key("object"));
    let browser = world
        .app
        .call(TemplateDelete, &[&a])
        .token(&company.viewer.token)
        .send()
        .await
        .problem();
    assert_eq!(browser.code, problem.code);
    assert!(browser.extra.contains_key("missing"));
    let manage = key(&world, company.boss.id(), "template", a, allow).await;
    assert_eq!(
        world
            .app
            .call(TemplateDelete, &[&b])
            .token(&manage)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let deleted = world
        .app
        .call(TemplateDelete, &[&a])
        .token(&manage)
        .send()
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert!(deleted.raw.is_empty());
    assert!(
        routes::templates(&mut *world.conn().await, &[a])
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        routes::templates(&mut *world.conn().await, &[b])
            .await
            .unwrap()
            .len(),
        1
    );

    let used = resource(&world, &company, ResourceKind::Template, "Used template").await;
    // The same retained-use fixture as the ordinary TemplateDelete lifecycle control.
    sqlx::query(
        "INSERT INTO environments (id, name, owner_id, org_id, home_node_id, placement, template_id)
         VALUES ($1, 'From it', $2, $3, $3, 'managed', $4)",
    )
    .bind(new_id())
    .bind(company.boss.id())
    .bind(company.acme)
    .bind(used)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let attempt = key(
        &world,
        company.boss.id(),
        "template",
        used,
        json!([{"operation":"TemplateDelete","actions":["manage"]}]),
    )
    .await;
    let bounded = world
        .app
        .call(TemplateDelete, &[&used])
        .token(&attempt)
        .send()
        .await
        .problem();
    assert_eq!(
        bounded.code,
        orch_api_contract::common::ErrorCode::ResourceInUse
    );
    assert!(!bounded.extra.contains_key("units"));
    let disclosed = key(
        &world,
        company.boss.id(),
        "template",
        used,
        json!([{"operation":"TemplateDelete","actions":["manage","read"]}]),
    )
    .await;
    let full = world
        .app
        .call(TemplateDelete, &[&used])
        .token(&disclosed)
        .send()
        .await
        .problem();
    assert_eq!(full.code, bounded.code);
    assert_eq!(
        full.extra["units"], 1,
        "only this operation's optional Read discloses the count"
    );
}

async fn version(world: &World, skill: Id, owner: Id, name: &str) -> Id {
    let id = new_id();
    let blob = Hash::of(name.as_bytes());
    let mut conn = world.conn().await;
    storage::register_blob(
        &mut conn,
        &blob,
        i64::try_from(name.len()).unwrap(),
        world.now(),
    )
    .await
    .unwrap();
    skills::insert_version(
        &mut conn,
        &skills::NewVersion {
            id,
            skill_id: skill,
            name: name.into(),
            description: "A stored version".into(),
            archive: blob,
            archive_size: i64::try_from(name.len()).unwrap(),
            files: json!([]),
            size: 0,
            origin: json!({"kind":"draft","based_on":null}),
            keeps_origin: false,
            created_by: Some(owner),
        },
        world.now(),
    )
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn skill_versions_remain_under_the_skill_and_delete_preserves_retained_versions() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let a = resource(&world, &company, ResourceKind::Skill, "skill-a").await;
    let b = resource(&world, &company, ResourceKind::Skill, "skill-b").await;
    let va = version(&world, a, company.boss.id(), "skill-a").await;
    let vb = version(&world, b, company.boss.id(), "skill-b").await;
    skills::start_draft(
        &mut *world.conn().await,
        a,
        Some(va),
        &[],
        company.boss.id(),
        world.now(),
    )
    .await
    .unwrap();
    let read = key(
        &world,
        company.boss.id(),
        "skill",
        a,
        json!([
            {"operation":"SkillGet","actions":["read"]},
            {"operation":"SkillVersionList","actions":["read"]},
            {"operation":"SkillVersionGet","actions":["read"]}
        ]),
    )
    .await;
    assert_eq!(
        world
            .app
            .call(SkillGet, &[&a])
            .token(&read)
            .send()
            .await
            .ok()
            .id,
        a
    );
    let listed = world
        .app
        .call(SkillVersionList, &[&a])
        .token(&read)
        .query(&SkillVersionListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(listed.items.len(), 1);
    assert_eq!(listed.items[0].id, va);
    assert_eq!(
        world
            .app
            .call(SkillVersionGet, &[&a, &va])
            .token(&read)
            .send()
            .await
            .ok()
            .version
            .id,
        va
    );
    assert_eq!(
        world
            .app
            .call(SkillVersionGet, &[&a, &vb])
            .token(&read)
            .send()
            .await
            .status,
        StatusCode::NOT_FOUND,
        "the ordinary version producer verifies skill ownership"
    );
    assert_eq!(
        world
            .app
            .call(SkillGet, &[&b])
            .token(&read)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let manage = key(
        &world,
        company.boss.id(),
        "skill",
        a,
        json!([
            {"operation":"SkillDraftDelete","actions":["manage"]},
            {"operation":"SkillDelete","actions":["manage"]}
        ]),
    )
    .await;
    let discarded = world
        .app
        .call(SkillDraftDelete, &[&a])
        .token(&manage)
        .send()
        .await;
    assert_eq!(discarded.status, StatusCode::NO_CONTENT);
    assert!(discarded.raw.is_empty());
    assert!(
        skills::draft_infos(&mut *world.conn().await, &[a])
            .await
            .unwrap()
            .is_empty()
    );
    let deleted = world
        .app
        .call(SkillDelete, &[&a])
        .token(&manage)
        .send()
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert!(deleted.raw.is_empty());
    assert_eq!(
        skills::versions_by_id(&mut *world.conn().await, &[va])
            .await
            .unwrap()
            .len(),
        1,
        "the same writer retains immutable versions for prior turns"
    );
    assert_eq!(
        skills::skills(&mut *world.conn().await, &[b])
            .await
            .unwrap()
            .len(),
        1
    );
}
