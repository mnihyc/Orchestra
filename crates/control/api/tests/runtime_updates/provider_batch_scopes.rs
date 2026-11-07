//! Baseline-compatible actual-key controls for only the two provider runtime batch operations.
use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::{identity, jobs};

const PREVIEW: &str = "ProviderRuntimeUpdateBatchPreflight";
const CREATE: &str = "ProviderRuntimeUpdateBatchCreate";

struct Key {
    id: Id,
    secret: String,
}

fn rule(operation: &str, kind: &str, id: Id, actions: &[&str]) -> serde_json::Value {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},
        "allow":[{"operation":operation,"actions":actions}]})
}

async fn key(world: &World, user: Id, rules: Vec<serde_json::Value>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let id = new_id();
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Provider batch scope control",
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
    Key { id, secret }
}

async fn preview(world: &World, token: &str, ids: &[Id]) -> (u16, serde_json::Value) {
    let reply = world
        .app
        .call(provider_api::ProviderRuntimeUpdateBatchPreflight, &[])
        .token(token)
        .body(&provider_api::ProviderRuntimeUpdateBatchPreflightBody {
            provider_ids: ids.to_vec(),
        })
        .send_bytes()
        .await;
    (
        reply.status.as_u16(),
        serde_json::from_slice(&reply.raw).unwrap(),
    )
}

async fn confirm(world: &World, token: &str, id: Id) -> (u16, serde_json::Value) {
    let reply = world
        .app
        .call(provider_api::ProviderRuntimeUpdateBatchCreate, &[])
        .token(token)
        .query(&orch_api_contract::forks::UpdateConfirmQuery {
            notify: Some(false),
        })
        .body(&ForkCreateBody {
            preflight_id: id,
            options: None,
            notify: None,
        })
        .send_bytes()
        .await;
    (
        reply.status.as_u16(),
        serde_json::from_slice(&reply.raw).unwrap(),
    )
}

fn plan(reply: &serde_json::Value) -> Id {
    serde_json::from_value(reply["preflight_id"].clone()).unwrap()
}

async fn queued(world: &World) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='provider_recreate'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}

async fn provider_template(world: &World, provider: Id) -> Id {
    sqlx::query_scalar("SELECT template_id FROM runtimes WHERE provider_id=$1")
        .bind(provider)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}

async fn retained(world: &World, id: Id) -> bool {
    jobs::forks::preflight(&mut *world.conn().await, id)
        .await
        .unwrap()
        .is_some()
}

async fn another_host(world: &World, team: &Team, name: &str) -> Id {
    let mut conn = world.conn().await;
    let id = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "host",
        Some(team.c.admin.id()),
        Some(team.c.acme),
        name,
    )
    .await
    .unwrap();
    orch_authz::ownership::start_resource(
        &mut conn,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Host,
            id,
        },
        team.c.admin.id(),
        orch_authz::ownership::NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    id
}

async fn desired_runtime(world: &World, team: &Team, name: &str, value: serde_json::Value) {
    // Provider own runtime values are a whole-map PATCH, not a settings Target variant.
    // Read its actual map/revision so the test preserves unrelated values and the saved plan.
    let (mut config, revision): (serde_json::Value, i64) =
        sqlx::query_as("SELECT runtime_config,revision FROM providers WHERE id=$1")
            .bind(team.provider)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    config
        .as_object_mut()
        .unwrap()
        .insert(name.to_owned(), value);
    let reply = world
        .app
        .call(provider_api::ProviderUpdate, &[&team.provider])
        .token(&team.c.admin.token)
        .body(&provider_api::ProviderUpdateBody {
            name: None,
            upstreams: None,
            config: None,
            arrangements: None,
            runtime_config: Some(config),
            expected_revision: revision,
        })
        .send_bytes()
        .await;
    assert_eq!(
        reply.status.as_u16(),
        200,
        "the ordinary owner stores desired runtime values: {}",
        String::from_utf8_lossy(&reply.raw)
    );
}

async fn another_provider(world: &World, team: &Team) -> Id {
    let mut conn = world.conn().await;
    let provider = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "provider",
        Some(team.c.admin.id()),
        Some(team.c.acme),
        "Second service",
    )
    .await
    .unwrap();
    orch_authz::ownership::start_resource(
        &mut conn,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Provider,
            id: provider,
        },
        team.c.admin.id(),
        orch_authz::ownership::NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO runtimes(id,kind,host_id,unit_name,provider_id,driver,template_id,spec_hash,generation,state,volumes) SELECT $1,'provider',host_id,$2,$3,driver,template_id,spec_hash,generation,state,volumes FROM runtimes WHERE provider_id=$4")
        .bind(new_id()).bind(format!("second-service-{}", new_id())).bind(provider)
        .bind(team.provider).execute(&mut *conn).await.unwrap();
    provider
}

#[tokio::test]
async fn distinct_preview_and_create_keys_return_only_acceptance_ids_without_read() {
    let world = World::new().await;
    let team = team(&world).await;
    let preview_key = key(
        &world,
        team.c.admin.id(),
        vec![rule(PREVIEW, "provider", team.provider, &["manage"])],
    )
    .await;
    let create_key = key(
        &world,
        team.c.admin.id(),
        vec![rule(CREATE, "provider", team.provider, &["manage"])],
    )
    .await;
    let (status, shown) = preview(&world, &preview_key.secret, &[team.provider]).await;
    assert_eq!(status, 200, "{shown}");
    assert_eq!(shown["projection"], "receipt");
    assert_eq!(shown["executable"], 1);
    assert!(!shown.to_string().contains("Service"));
    assert!(
        shown.get("plans").is_none(),
        "no existing named plan is exposed without Read"
    );
    let id = plan(&shown);
    let (status, made) = confirm(&world, &create_key.secret, id).await;
    assert_eq!(status, 202, "{made}");
    assert_eq!(made["projection"], "receipt");
    assert!(!made.to_string().contains("Service"));
    assert_eq!(made["jobs"].as_array().unwrap().len(), 1);
    assert!(made["jobs"][0]["job_id"].is_string());
    assert_eq!(made["jobs"][0]["provider_id"], json!(team.provider));
    assert_eq!(queued(&world).await, 1);
    assert!(!retained(&world, id).await);
    assert_eq!(confirm(&world, &create_key.secret, id).await.0, 404);
}

#[tokio::test]
async fn same_operation_read_preserves_the_ordinary_named_batch_projection() {
    let world = World::new().await;
    let team = team(&world).await;
    let (status, ordinary_hidden) = preview(&world, &team.c.stranger.token, &[team.provider]).await;
    assert_eq!(status, 200, "{ordinary_hidden}");
    assert_eq!(ordinary_hidden["executable"], 0);
    let unentitled = key(
        &world,
        team.c.stranger.id(),
        vec![
            rule(PREVIEW, "provider", team.provider, &["manage", "read"]),
            rule(PREVIEW, "host", team.host, &["read"]),
            rule(
                PREVIEW,
                "template",
                provider_template(&world, team.provider).await,
                &["read"],
            ),
        ],
    )
    .await;
    let (status, hidden) = preview(&world, &unentitled.secret, &[team.provider]).await;
    assert_eq!(status, 200, "{hidden}");
    assert_eq!(
        hidden["executable"], 0,
        "key Read/Manage selectors never add the account's ordinary privilege"
    );
    assert!(!hidden.to_string().contains("Service"));
    assert!(!hidden.to_string().contains("Build"));
    let preview_key = key(
        &world,
        team.c.admin.id(),
        vec![
            rule(PREVIEW, "provider", team.provider, &["manage", "read"]),
            rule(
                PREVIEW,
                "template",
                provider_template(&world, team.provider).await,
                &["read"],
            ),
        ],
    )
    .await;
    let (status, shown) =
        preview(&world, &preview_key.secret, &[team.provider, team.provider]).await;
    assert_eq!(status, 200, "{shown}");
    assert_eq!(shown["executable"], 1, "duplicates stay one ordinary plan");
    assert_eq!(shown["plans"][0]["provider"]["name"], "Service");
    assert!(shown.get("projection").is_none());
    let create_key = key(
        &world,
        team.c.admin.id(),
        vec![rule(CREATE, "provider", team.provider, &["manage", "read"])],
    )
    .await;
    let (status, made) = confirm(&world, &create_key.secret, plan(&shown)).await;
    assert_eq!(status, 202, "{made}");
    assert_eq!(made["jobs"][0]["provider"]["name"], "Service");
    assert_eq!(queued(&world).await, 1);
}

#[tokio::test]
async fn a_revoked_preview_key_cannot_be_laundered_by_the_same_accounts_browser() {
    let world = World::new().await;
    let team = team(&world).await;
    let key = key(
        &world,
        team.c.admin.id(),
        vec![rule(PREVIEW, "provider", team.provider, &["manage"])],
    )
    .await;
    let (status, shown) = preview(&world, &key.secret, &[team.provider]).await;
    assert_eq!(status, 200, "{shown}");
    let id = plan(&shown);
    identity::revoke_api_token(
        &mut *world.conn().await,
        key.id,
        team.c.admin.id(),
        world.now(),
    )
    .await
    .unwrap();
    let (status, refused) = confirm(&world, &team.c.admin.token, id).await;
    assert_eq!(
        status, 403,
        "the consumer's browser cannot drop the accepted key: {refused}"
    );
    assert_eq!(queued(&world).await, 0);
    assert!(retained(&world, id).await);
}

#[tokio::test]
async fn current_preview_scope_and_create_scope_are_independent_atomic_bounds() {
    let world = World::new().await;
    let team = team(&world).await;
    let rules = vec![rule(PREVIEW, "provider", team.provider, &["manage"])];
    let preview_key = key(&world, team.c.admin.id(), rules.clone()).await;
    let create_key = key(
        &world,
        team.c.admin.id(),
        vec![rule(CREATE, "provider", team.provider, &["manage"])],
    )
    .await;
    let (status, shown) = preview(&world, &preview_key.secret, &[team.provider]).await;
    assert_eq!(status, 200, "{shown}");
    let id = plan(&shown);
    let wrong = key(
        &world,
        team.c.admin.id(),
        vec![rule(PREVIEW, "provider", team.provider, &["manage"])],
    )
    .await;
    assert_eq!(confirm(&world, &wrong.secret, id).await.0, 403);
    let elsewhere = key(
        &world,
        team.c.admin.id(),
        vec![rule(CREATE, "provider", new_id(), &["manage"])],
    )
    .await;
    assert_eq!(confirm(&world, &elsewhere.secret, id).await.0, 403);
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(preview_key.id)
        .bind(json!(KeyScope::default()))
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(confirm(&world, &create_key.secret, id).await.0, 403);
    assert_eq!(queued(&world).await, 0);
    assert!(retained(&world, id).await);
    // A normal policy edit can restore the original accepted footprint; no new origin replaces it.
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(preview_key.id)
        .bind(json!({"rules":rules}))
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(confirm(&world, &create_key.secret, id).await.0, 202);
}

#[tokio::test]
async fn an_explicit_empty_batch_is_not_an_unclassified_or_unbounded_operation() {
    let world = World::new().await;
    let team = team(&world).await;
    let key = key(
        &world,
        team.c.admin.id(),
        vec![
            rule(PREVIEW, "provider", team.provider, &["manage"]),
            rule(CREATE, "provider", team.provider, &["manage"]),
        ],
    )
    .await;
    let (status, shown) = preview(&world, &key.secret, &[]).await;
    assert_eq!(status, 200, "{shown}");
    assert_eq!(shown["executable"], 0);
    let (status, made) = confirm(&world, &key.secret, plan(&shown)).await;
    assert_eq!(status, 202, "{made}");
    assert!(made["jobs"].as_array().unwrap().is_empty());
    assert_eq!(queued(&world).await, 0);
}

#[tokio::test]
async fn partial_preview_and_atomic_confirmation_keep_distinct_provider_members() {
    let world = World::new().await;
    let team = team(&world).await;
    let other = another_provider(&world, &team).await;
    let partial = key(
        &world,
        team.c.admin.id(),
        vec![
            rule(PREVIEW, "provider", team.provider, &["manage", "read"]),
            rule(PREVIEW, "provider", other, &["read"]),
            rule(
                PREVIEW,
                "template",
                provider_template(&world, team.provider).await,
                &["read"],
            ),
        ],
    )
    .await;
    let absent = new_id();
    let (status, shown) = preview(
        &world,
        &partial.secret,
        &[team.provider, other, team.provider, absent],
    )
    .await;
    assert_eq!(status, 200, "{shown}");
    assert_eq!(shown["executable"], 1);
    assert_eq!(shown["plans"].as_array().unwrap().len(), 1);
    assert_eq!(shown["skipped"].as_array().unwrap().len(), 1);
    assert_eq!(shown["skipped"][0]["provider"]["id"], json!(other));
    assert_eq!(shown["skipped"][0]["reason"], "no_permission");
    let first = plan(&shown);
    let whole = key(
        &world,
        team.c.admin.id(),
        vec![
            rule(PREVIEW, "provider", team.provider, &["manage"]),
            rule(PREVIEW, "provider", other, &["manage"]),
        ],
    )
    .await;
    let (status, shown) = preview(&world, &whole.secret, &[team.provider, other]).await;
    assert_eq!(status, 200, "{shown}");
    assert_eq!(shown["executable"], 2);
    let both = plan(&shown);
    let consumer = key(
        &world,
        team.c.admin.id(),
        vec![rule(CREATE, "provider", team.provider, &["manage"])],
    )
    .await;
    assert_eq!(confirm(&world, &consumer.secret, both).await.0, 403);
    assert_eq!(
        queued(&world).await,
        0,
        "one refused member rolls back the whole confirmation"
    );
    assert!(retained(&world, both).await);
    let (status, made) = confirm(&world, &consumer.secret, first).await;
    assert_eq!(status, 202, "{made}");
    assert_eq!(made["jobs"].as_array().unwrap().len(), 1);
    assert_eq!(made["jobs"][0]["provider_id"], json!(team.provider));
    let untouched: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE provider_id=$1")
        .bind(other)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        untouched, 0,
        "the read-only skipped member is never smuggled into execution"
    );
}

#[tokio::test]
async fn a_manage_key_does_not_receive_live_host_labels_through_a_batch_plan() {
    let world = World::new().await;
    let team = team(&world).await;
    let destination = another_host(&world, &team, "Private selected host 572").await;
    desired_runtime(
        &world,
        &team,
        orch_settings_contract::registry::keys::RUNTIME_HOST_ID,
        json!(destination),
    )
    .await;
    let rules = vec![
        rule(PREVIEW, "provider", team.provider, &["manage", "read"]),
        rule(
            PREVIEW,
            "template",
            provider_template(&world, team.provider).await,
            &["read"],
        ),
    ];
    let limited = key(&world, team.c.admin.id(), rules.clone()).await;
    let (status, shown) = preview(&world, &limited.secret, &[team.provider]).await;
    assert_eq!(status, 200, "{shown}");
    assert_eq!(shown["projection"], "receipt");
    assert!(!shown.to_string().contains("Private selected host 572"));
    assert!(!shown.to_string().contains("Build"));
    let mut unrelated = rules.clone();
    unrelated.push(rule("HostGet", "host", team.host, &["read"]));
    unrelated.push(rule("HostGet", "host", destination, &["read"]));
    let unrelated = key(&world, team.c.admin.id(), unrelated).await;
    let (status, shown) = preview(&world, &unrelated.secret, &[team.provider]).await;
    assert_eq!(status, 200, "{shown}");
    assert_eq!(
        shown["projection"], "receipt",
        "HostGet is not this operation's source Read"
    );
    let mut complete = rules;
    complete.push(rule(PREVIEW, "host", team.host, &["read"]));
    complete.push(rule(PREVIEW, "host", destination, &["read"]));
    let full = key(&world, team.c.admin.id(), complete).await;
    let (status, shown) = preview(&world, &full.secret, &[team.provider]).await;
    assert_eq!(status, 200, "{shown}");
    assert!(shown.get("projection").is_none());
    assert!(shown.to_string().contains("Private selected host 572"));
    assert!(shown.to_string().contains("Build"));
}

#[tokio::test]
async fn stored_network_values_are_not_scrubbed_when_they_equal_an_unselected_host_name() {
    let world = World::new().await;
    let team = team(&world).await;
    let literal = "private.host.test";
    another_host(&world, &team, literal).await;
    world
        .set(
            orch_settings_contract::registry::keys::NETWORK_ALLOWED_HOSTS,
            json!([literal]),
        )
        .await;
    sqlx::query("UPDATE env_templates SET spec=jsonb_set(spec,'{network}',$2) WHERE id=(SELECT template_id FROM runtimes WHERE provider_id=$1)")
        .bind(team.provider).bind(json!({"mode":"bridge","egress":"restricted"}))
        .execute(&world.db.pool).await.unwrap();
    let (status, ordinary) = preview(&world, &team.c.admin.token, &[team.provider]).await;
    assert_eq!(status, 200, "{ordinary}");
    assert!(
        ordinary["plans"][0]["network"]["allowed_hosts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == literal),
        "the ordinary stored-value premise is real"
    );
    let scoped = key(
        &world,
        team.c.admin.id(),
        vec![
            rule(PREVIEW, "provider", team.provider, &["manage", "read"]),
            rule(
                PREVIEW,
                "template",
                provider_template(&world, team.provider).await,
                &["read"],
            ),
        ],
    )
    .await;
    let (status, shown) = preview(&world, &scoped.secret, &[team.provider]).await;
    assert_eq!(status, 200, "{shown}");
    assert!(shown.get("projection").is_none());
    assert_eq!(
        shown["plans"][0]["network"]["allowed_hosts"],
        ordinary["plans"][0]["network"]["allowed_hosts"],
        "stored management values need no unrelated Host source Read and are never text-scrubbed"
    );
}

#[tokio::test]
async fn a_changed_selected_host_invalidates_an_ordinary_saved_batch_before_any_job() {
    let world = World::new().await;
    let team = team(&world).await;
    let (status, shown) = preview(&world, &team.c.admin.token, &[team.provider]).await;
    assert_eq!(status, 200, "{shown}");
    let id = plan(&shown);
    let destination = another_host(&world, &team, "Changed after review").await;
    desired_runtime(
        &world,
        &team,
        orch_settings_contract::registry::keys::RUNTIME_HOST_ID,
        json!(destination),
    )
    .await;
    let (status, refused) = confirm(&world, &team.c.admin.token, id).await;
    assert_eq!(status, 409, "{refused}");
    assert_eq!(refused["code"], "conflict.stale_revision");
    assert_eq!(queued(&world).await, 0);
    assert!(retained(&world, id).await);
}

#[tokio::test]
async fn a_distinct_same_definition_template_invalidates_an_ordinary_saved_batch() {
    let world = World::new().await;
    let team = team(&world).await;
    let (status, shown) = preview(&world, &team.c.admin.token, &[team.provider]).await;
    assert_eq!(status, 200, "{shown}");
    let id = plan(&shown);
    let mut conn = world.conn().await;
    let template = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "template",
        Some(team.c.admin.id()),
        Some(team.c.acme),
        "Same definition new identity",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE env_templates target SET kind=source.kind,driver=source.driver,spec=source.spec FROM env_templates source JOIN runtimes r ON r.template_id=source.id WHERE target.id=$1 AND r.provider_id=$2")
        .bind(template).bind(team.provider).execute(&mut *conn).await.unwrap();
    orch_authz::ownership::start_resource(
        &mut conn,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Template,
            id: template,
        },
        team.c.admin.id(),
        orch_authz::ownership::NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    let principal = orch_authz::Principal::load(
        &mut conn,
        orch_authz::Caller::session(team.c.admin.id()),
        Time::new(world.now()),
    )
    .await
    .unwrap();
    assert!(
        orch_authz::resources::check(
            &mut conn,
            &principal,
            orch_access_contract::decisions::ObjectRef {
                kind: orch_access_contract::permissions::ObjectKind::Template,
                id: template,
            },
            orch_access_contract::permissions::Permission::TemplateUse,
            orch_authz::Policy::default()
        )
        .await
        .unwrap()
        .is_allowed(),
        "the different same-definition template must pass the unchanged ordinary Use prerequisite"
    );
    drop(conn);
    desired_runtime(
        &world,
        &team,
        orch_settings_contract::registry::keys::RUNTIME_TEMPLATE_ID,
        json!(template),
    )
    .await;
    let (status, refused) = confirm(&world, &team.c.admin.token, id).await;
    assert_eq!(status, 409, "{refused}");
    assert_eq!(refused["code"], "conflict.stale_revision", "{refused}");
    assert_eq!(queued(&world).await, 0);
    assert!(retained(&world, id).await);
}
