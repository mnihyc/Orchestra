//! Actual Environment list scopes consume the native selected rows and their context sources.

use super::*;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::Listed;
use orch_api_contract::environment::{EnvironmentHolding, EnvironmentList, EnvironmentListQuery};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity;
use orch_types::{Json, new_id};

#[path = "trust_scopes.rs"]
mod trust;

#[path = "publication_scopes.rs"]
mod publication;

#[path = "capability_scopes.rs"]
mod capabilities;

#[path = "mcp_scopes.rs"]
mod mcp;

#[path = "recovery_scopes.rs"]
mod recovery;

#[path = "fleet_scopes.rs"]
mod fleet;

#[path = "loss_recovery_scopes.rs"]
mod loss_recovery;

#[path = "place_scopes.rs"]
mod place_choices;

#[path = "get_scopes.rs"]
mod individual;

#[path = "update_scopes.rs"]
mod configuration;

#[path = "sync_scopes.rs"]
mod synchronization;

#[path = "recycle_scopes.rs"]
mod recycling;

struct Key {
    id: Id,
    secret: String,
}

fn rule(
    operation: &str,
    kind: &str,
    id: Id,
    reach: &str,
    resources: &[&str],
    actions: &[&str],
) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"resources":resources,
        "allow":[{"operation":operation,"actions":actions}]})
}

async fn key(world: &World, actor: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: actor,
            name: "Environment list scope fixture",
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
    Key { id, secret }
}

struct Scene {
    world: World,
    team: Team,
    first: Environment,
    second: Environment,
}

impl Scene {
    async fn new() -> Self {
        let world = World::new().await;
        let team = team(&world).await;
        let first = world
            .app
            .call(EnvironmentCreate, &[])
            .token(&team.c.admin.token)
            .body(&wanted(&team, "First scoped environment"))
            .send()
            .await
            .ok();
        let second = world
            .app
            .call(EnvironmentCreate, &[])
            .token(&team.c.admin.token)
            .body(&wanted(&team, "Second scoped environment"))
            .send()
            .await
            .ok();
        // Reuse the ordinary restart fixture's persisted running-unit state. No native RPC is needed.
        for environment in [first.base.id, second.base.id] {
            sqlx::query("UPDATE jobs SET state='cancelled',finished_at=$2 WHERE environment_id=$1")
                .bind(environment)
                .bind(world.now())
                .execute(&world.db.pool)
                .await
                .unwrap();
            sqlx::query("UPDATE environments SET status='running',generation=1 WHERE id=$1")
                .bind(environment)
                .execute(&world.db.pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO runtimes(id,kind,host_id,unit_name,environment_id,driver,template_id,spec_hash,generation,state,volumes) VALUES($1,'environment',$2,$3,$4,'docker',$5,$6,1,'running','{}')")
                .bind(new_id()).bind(team.host).bind(format!("scoped-unit-{environment}"))
                .bind(environment).bind(team.template).bind([1_u8;32].as_slice())
                .execute(&world.db.pool).await.unwrap();
        }
        Self {
            world,
            team,
            first,
            second,
        }
    }

    fn sources(&self, operation: &str) -> Json {
        rule(
            operation,
            "org",
            self.team.c.acme,
            "subtree",
            &[
                "host", "template", "machine", "mount", "org", "project", "topic", "thread",
                "group",
            ],
            &["read"],
        )
    }

    fn rules(&self, ids: &[Id]) -> Vec<Json> {
        let mut rules: Vec<_> = ids
            .iter()
            .map(|id| {
                rule(
                    "EnvironmentList",
                    "environment",
                    *id,
                    "self",
                    &["environment"],
                    &["read"],
                )
            })
            .collect();
        rules.push(self.sources("EnvironmentList"));
        rules
    }

    async fn reader(&self, ids: &[Id]) -> Key {
        key(&self.world, self.team.c.admin.id(), self.rules(ids)).await
    }

    async fn list(&self, secret: &str, query: &EnvironmentListQuery) -> RawReply {
        self.world
            .app
            .call(EnvironmentList, &[])
            .token(secret)
            .query(query)
            .send_bytes()
            .await
    }

    async fn visible(&self, secret: &str, query: &EnvironmentListQuery) -> Vec<Environment> {
        let reply = self.list(secret, query).await;
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        let rows: Vec<Listed<Environment>> = serde_json::from_slice(&reply.raw).unwrap();
        rows.into_iter().filter_map(Listed::into_visible).collect()
    }

    async fn allowed_control(&self) -> Key {
        let reader = self.reader(&[self.first.base.id]).await;
        let shown = self
            .visible(&reader.secret, &EnvironmentListQuery::default())
            .await;
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].base.id, self.first.base.id);
        reader
    }
}

#[tokio::test]
async fn scoped_environment_list_keeps_exact_rows_native_bodies_and_query_filters() {
    let s = Scene::new().await;
    let reader = s.allowed_control().await;
    let query = EnvironmentListQuery {
        org_id: Some(s.team.c.acme),
        holding: Some(EnvironmentHolding::Manage),
    };
    let ordinary = s.visible(&s.team.c.admin.token, &query).await;
    let expected = ordinary
        .iter()
        .find(|row| row.base.id == s.first.base.id)
        .unwrap();
    let selected = s.visible(&reader.secret, &query).await;
    assert_eq!(selected.len(), 1);
    assert_eq!(
        json!(selected[0]),
        json!(expected),
        "the same native Environment renderer supplies browser and scoped bodies"
    );
    assert!(
        s.visible(
            &reader.secret,
            &EnvironmentListQuery {
                org_id: Some(new_id()),
                holding: None
            }
        )
        .await
        .is_empty()
    );
}

#[tokio::test]
async fn native_environment_mount_references_need_the_actual_mount_scope_in_lists_and_trust_readback()
 {
    use orch_api_contract::access::NewResourceAccess;
    use orch_api_contract::mounts::{MountCreate, MountCreateBody};
    use orch_api_contract::trust::{ConfigTrustAnswer, ConfigTrustBody, EnvironmentConfigTrustSet};

    let s = Scene::new().await;
    let mut mounts = Vec::new();
    for name in ["selected-mount-content-marker", "unselected-mount"] {
        mounts.push(
            s.world
                .app
                .call(MountCreate, &[])
                .token(&s.team.c.admin.token)
                .body(&MountCreateBody {
                    name: name.to_owned(),
                    description: None,
                    home_id: Some(s.team.c.acme),
                    target: "/mnt/fixture-data".to_owned(),
                    read_only: Some(true),
                    default_fate: None,
                    access: Some(NewResourceAccess::OnlyMe),
                })
                .send()
                .await
                .ok()
                .into_full()
                .expect("this native creation control requires its full response"),
        );
    }
    let selected = mounts[0].base.id;
    let other = mounts[1].base.id;
    sqlx::query(
        "UPDATE environments SET config=jsonb_set(config,'{env.mounts}',$2,true) WHERE id=$1",
    )
    .bind(s.first.base.id)
    .bind(json!({(selected.to_string()):true}))
    .execute(&s.world.db.pool)
    .await
    .unwrap();
    let native = s
        .visible(&s.team.c.admin.token, &Default::default())
        .await
        .into_iter()
        .find(|row| row.base.id == s.first.base.id)
        .unwrap();
    assert_eq!(
        native.mounts.len(),
        1,
        "a real selected native attachment, even if unavailable"
    );
    assert_eq!(native.mounts[0].mount.id, selected);
    assert_eq!(native.mounts[0].mount.name, "selected-mount-content-marker");
    let source = |operation: &str| {
        rule(
            operation,
            "org",
            s.team.c.acme,
            "subtree",
            &[
                "host", "template", "machine", "org", "project", "topic", "thread", "group",
            ],
            &["read"],
        )
    };
    for mount in [selected, other] {
        let reader = key(
            &s.world,
            s.team.c.admin.id(),
            vec![
                rule(
                    "EnvironmentList",
                    "environment",
                    s.first.base.id,
                    "self",
                    &["environment"],
                    &["read"],
                ),
                source("EnvironmentList"),
                rule(
                    "EnvironmentList",
                    "mount",
                    mount,
                    "self",
                    &["mount"],
                    &["read"],
                ),
                rule("MountGet", "mount", selected, "self", &["mount"], &["read"]),
            ],
        )
        .await;
        let reply = s.list(&reader.secret, &Default::default()).await;
        if mount == selected {
            assert_eq!(reply.status, StatusCode::OK);
            let rows: Vec<Listed<Environment>> = serde_json::from_slice(&reply.raw).unwrap();
            let rows: Vec<_> = rows.into_iter().filter_map(Listed::into_visible).collect();
            assert_eq!(
                rows,
                vec![native.clone()],
                "the ordinary native projection remains intact"
            );
        } else {
            assert_eq!(
                reply.status,
                StatusCode::FORBIDDEN,
                "a different mount's Read or another operation cannot lend the selected mount's content"
            );
            assert!(!String::from_utf8_lossy(&reply.raw).contains("selected-mount-content-marker"));
        }
    }
    orch_db::queries::trust::ask(
        &mut *s.world.conn().await,
        s.first.base.id,
        "fixture-harness",
        "mount-trust-v1",
        &["fixture-config/settings.json".to_owned()],
        s.world.now(),
    )
    .await
    .unwrap();
    for mount in [selected, other] {
        let writer = key(
            &s.world,
            s.team.c.admin.id(),
            vec![
                rule(
                    "EnvironmentConfigTrustSet",
                    "environment",
                    s.first.base.id,
                    "self",
                    &["environment"],
                    &["manage", "read"],
                ),
                source("EnvironmentConfigTrustSet"),
                rule(
                    "EnvironmentConfigTrustSet",
                    "mount",
                    mount,
                    "self",
                    &["mount"],
                    &["read"],
                ),
            ],
        )
        .await;
        let reply = s
            .world
            .app
            .call(
                EnvironmentConfigTrustSet,
                &[&s.first.base.id, &"fixture-harness"],
            )
            .token(&writer.secret)
            .body(&ConfigTrustBody {
                answer: ConfigTrustAnswer::KeepOut,
            })
            .send_bytes()
            .await;
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "the native answer remains accepted"
        );
        let body: Json = serde_json::from_slice(&reply.raw).unwrap();
        if mount == selected {
            assert_eq!(body["mounts"][0]["mount"]["id"], json!(selected));
        } else {
            assert_eq!(
                body,
                json!({"projection":"receipt", "environment_id":s.first.base.id,
                "harness":"fixture-harness", "answer":"keep_out"})
            );
        }
    }
}

#[tokio::test]
async fn scoped_environment_list_needs_its_own_read_and_never_lends_ordinary_visibility() {
    let s = Scene::new().await;
    s.allowed_control().await;
    for (operation, actions) in [
        ("EnvironmentGet", vec!["read"]),
        ("EnvironmentList", vec!["write"]),
    ] {
        let wrong = key(
            &s.world,
            s.team.c.admin.id(),
            vec![
                rule(
                    operation,
                    "environment",
                    s.first.base.id,
                    "self",
                    &["environment"],
                    &actions,
                ),
                s.sources(operation),
            ],
        )
        .await;
        let reply = s
            .list(&wrong.secret, &EnvironmentListQuery::default())
            .await;
        assert_eq!(
            reply.status,
            StatusCode::FORBIDDEN,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        assert!(!String::from_utf8_lossy(&reply.raw).contains(&s.first.base.name));
    }
    let wrong_target = s.reader(&[new_id()]).await;
    assert!(
        s.visible(&wrong_target.secret, &EnvironmentListQuery::default())
            .await
            .is_empty()
    );
    let stranger = key(
        &s.world,
        s.team.c.stranger.id(),
        s.rules(&[s.first.base.id]),
    )
    .await;
    assert!(
        s.visible(&stranger.secret, &EnvironmentListQuery::default())
            .await
            .is_empty(),
        "scope adds no ordinary Environment permission"
    );
}

#[tokio::test]
async fn scoped_environment_list_checks_foreign_content_and_uses_own_operation_for_names() {
    let s = Scene::new().await;
    s.allowed_control().await;
    let primary = rule(
        "EnvironmentList",
        "environment",
        s.first.base.id,
        "self",
        &["environment"],
        &["read"],
    );
    let missing = key(&s.world, s.team.c.admin.id(), vec![primary.clone()]).await;
    let reply = s
        .list(&missing.secret, &EnvironmentListQuery::default())
        .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("Build box"));
    assert!(!String::from_utf8_lossy(&reply.raw).contains("Small"));
    let names_unselected = key(
        &s.world,
        s.team.c.admin.id(),
        vec![
            primary,
            rule(
                "EnvironmentList",
                "host",
                s.team.host,
                "self",
                &["host", "machine"],
                &["read"],
            ),
            rule(
                "EnvironmentList",
                "template",
                s.team.template,
                "self",
                &["template"],
                &["read"],
            ),
            s.sources("OrgGet"),
        ],
    )
    .await;
    let selected = s
        .visible(&names_unselected.secret, &EnvironmentListQuery::default())
        .await;
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].home.id, s.first.home.id);
    assert!(
        selected[0].home.name.is_none(),
        "another operation's Read cannot supply a home title"
    );
    assert_eq!(selected[0].template.as_ref().unwrap().id, s.team.template);
}

#[tokio::test]
async fn scoped_environment_list_uses_current_key_bounds_expiry_and_revocation() {
    let s = Scene::new().await;
    let reader = s.reader(&[s.first.base.id, s.second.base.id]).await;
    assert_eq!(
        s.visible(&reader.secret, &EnvironmentListQuery::default())
            .await
            .len(),
        2
    );
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(reader.id)
        .bind(json!({"rules":s.rules(&[s.second.base.id])}))
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    let narrowed = s
        .visible(&reader.secret, &EnvironmentListQuery::default())
        .await;
    assert_eq!(narrowed.len(), 1);
    assert_eq!(narrowed[0].base.id, s.second.base.id);
    sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
        .bind(reader.id)
        .bind(s.world.now() - chrono::Duration::seconds(1))
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        s.list(&reader.secret, &EnvironmentListQuery::default())
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    let revoked = s.allowed_control().await;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(revoked.id)
        .bind(s.world.now())
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        s.list(&revoked.secret, &EnvironmentListQuery::default())
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn scoped_environment_list_keeps_actual_retained_template_and_host_context() {
    let s = Scene::new().await;
    let reader = s.allowed_control().await;
    let retained_home = places::make_department(
        &s.world,
        &s.team.c.boss,
        s.team.c.acme,
        "Retained source home",
        "retained-source",
    )
    .await
    .base
    .id;
    sqlx::query("UPDATE env_templates SET home_node_id=$2 WHERE id=$1")
        .bind(s.team.template)
        .bind(retained_home)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE hosts SET home_node_id=$2 WHERE id=$1")
        .bind(s.team.host)
        .bind(retained_home)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE nodes SET deleted_at=$2 WHERE id=$1")
        .bind(retained_home)
        .bind(s.world.now())
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    let native = s
        .visible(&s.team.c.admin.token, &EnvironmentListQuery::default())
        .await;
    let expected = native
        .iter()
        .find(|row| row.base.id == s.first.base.id)
        .unwrap();
    assert_eq!(
        expected.template.as_ref().unwrap().name,
        "Small",
        "the ordinary renderer actually retains the template context"
    );
    assert_eq!(
        expected.host.as_ref().unwrap().name,
        "Build box",
        "the ordinary renderer actually retains the host context"
    );
    let selected = s
        .visible(&reader.secret, &EnvironmentListQuery::default())
        .await;
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].template, expected.template);
    assert_eq!(selected[0].host, expected.host);
}

mod retained {
    use super::*;
    use orch_api_contract::common::Page;
    use orch_api_contract::environment::{
        DeletedEnvironment, NodeDeletedEnvironmentList, NodeDeletedEnvironmentListQuery,
    };

    pub(super) async fn keep(s: &Scene) {
        for environment in [s.first.base.id, s.second.base.id] {
            let runtime: Id = sqlx::query_scalar("SELECT id FROM runtimes WHERE environment_id=$1")
                .bind(environment)
                .fetch_one(&s.world.db.pool)
                .await
                .unwrap();
            let volumes = json!([{"runtime_id":runtime,"environment_id":environment,"host_id":s.team.host,
                "driver":"docker","role":"workspace","name":format!("kept-workspace-{environment}"),"storage":"fast"}]);
            sqlx::query("UPDATE environments SET status='removed',deleted_at=$2,removed_keep_days=7,volumes_purge_at=$3,removal_phase='kept',removal_revision=17,retained_volumes=$4,retained_bytes=4096 WHERE id=$1")
                .bind(environment).bind(s.world.now()).bind(s.world.now()+chrono::Duration::days(7)).bind(volumes)
                .execute(&s.world.db.pool).await.unwrap();
            sqlx::query("UPDATE runtimes SET state='removed' WHERE environment_id=$1")
                .bind(environment)
                .execute(&s.world.db.pool)
                .await
                .unwrap();
        }
    }

    fn rules(s: &Scene, ids: &[Id], action: &str) -> Vec<Json> {
        let mut rules: Vec<_> = ids
            .iter()
            .map(|id| {
                rule(
                    "NodeDeletedEnvironmentList",
                    "environment",
                    *id,
                    "self",
                    &["environment"],
                    &[action],
                )
            })
            .collect();
        rules.push(s.sources("NodeDeletedEnvironmentList"));
        rules
    }

    async fn page(
        s: &Scene,
        secret: &str,
        node: Id,
        query: &NodeDeletedEnvironmentListQuery,
    ) -> Page<DeletedEnvironment> {
        let reply = s
            .world
            .app
            .call(NodeDeletedEnvironmentList, &[&node])
            .token(secret)
            .query(query)
            .send_bytes()
            .await;
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        serde_json::from_slice(&reply.raw).unwrap()
    }

    #[tokio::test]
    async fn kept_list_uses_manage_and_retained_rows_with_the_native_body_and_window() {
        let s = Scene::new().await;
        keep(&s).await;
        let ordinary = page(
            &s,
            &s.team.c.admin.token,
            s.team.topic,
            &NodeDeletedEnvironmentListQuery::default(),
        )
        .await;
        assert_eq!(
            ordinary.items.len(),
            2,
            "the ordinary fixture has two real retained rows"
        );
        let wanted = s.first.base.id.max(s.second.base.id);
        let reader = key(
            &s.world,
            s.team.c.admin.id(),
            rules(&s, &[wanted], "manage"),
        )
        .await;
        let query = NodeDeletedEnvironmentListQuery {
            limit: Some(1),
            ..Default::default()
        };
        let selected = page(&s, &reader.secret, s.team.topic, &query).await;
        assert_eq!(selected.items.len(), 1);
        assert_eq!(selected.items[0].environment.id, wanted);
        assert!(
            selected.next_cursor.is_none(),
            "an out-of-scope row is not look-ahead"
        );
        let expected = ordinary
            .items
            .iter()
            .find(|row| row.environment.id == wanted)
            .unwrap();
        assert_eq!(json!(selected.items[0]), json!(expected));
        let wrong = key(&s.world, s.team.c.admin.id(), rules(&s, &[wanted], "read")).await;
        let reply = s
            .world
            .app
            .call(NodeDeletedEnvironmentList, &[&s.team.topic])
            .token(&wrong.secret)
            .send_bytes()
            .await;
        assert_eq!(
            reply.status,
            StatusCode::FORBIDDEN,
            "Read cannot stand for the native Manage inspection"
        );
    }

    #[tokio::test]
    async fn kept_list_binds_real_environment_identity_and_preserves_native_restoration_authority()
    {
        let s = Scene::new().await;
        keep(&s).await;
        let reader = key(
            &s.world,
            s.team.c.admin.id(),
            rules(&s, &[s.first.base.id], "manage"),
        )
        .await;
        assert_eq!(
            page(&s, &reader.secret, s.team.topic, &Default::default())
                .await
                .items
                .len(),
            1
        );
        let wrong_target = key(
            &s.world,
            s.team.c.admin.id(),
            rules(&s, &[s.team.topic], "manage"),
        )
        .await;
        assert!(
            page(&s, &wrong_target.secret, s.team.topic, &Default::default())
                .await
                .items
                .is_empty(),
            "the node path is not an Environment identity"
        );
        let viewer = key(
            &s.world,
            s.team.c.viewer.id(),
            rules(&s, &[s.first.base.id], "manage"),
        )
        .await;
        assert!(
            page(&s, &viewer.secret, s.team.topic, &Default::default())
                .await
                .items
                .is_empty(),
            "scope cannot create retained-data management"
        );
        let wrong_operation = key(
            &s.world,
            s.team.c.admin.id(),
            vec![
                rule(
                    "EnvironmentGet.inspect_deleted",
                    "environment",
                    s.first.base.id,
                    "self",
                    &["environment"],
                    &["manage"],
                ),
                s.sources("EnvironmentGet.inspect_deleted"),
            ],
        )
        .await;
        let reply = s
            .world
            .app
            .call(NodeDeletedEnvironmentList, &[&s.team.topic])
            .token(&wrong_operation.secret)
            .send_bytes()
            .await;
        assert_eq!(
            reply.status,
            StatusCode::FORBIDDEN,
            "another inspection operation cannot lend admission"
        );
    }

    #[tokio::test]
    async fn kept_list_manage_only_keeps_intrinsic_metadata_and_redacts_optional_home_names() {
        let s = Scene::new().await;
        keep(&s).await;
        let full = key(
            &s.world,
            s.team.c.admin.id(),
            rules(&s, &[s.first.base.id], "manage"),
        )
        .await;
        assert_eq!(
            page(&s, &full.secret, s.team.topic, &Default::default())
                .await
                .items
                .len(),
            1
        );
        let manager = key(
            &s.world,
            s.team.c.admin.id(),
            vec![rule(
                "NodeDeletedEnvironmentList",
                "environment",
                s.first.base.id,
                "self",
                &["environment"],
                &["manage"],
            )],
        )
        .await;
        let selected = page(&s, &manager.secret, s.team.topic, &Default::default()).await;
        assert_eq!(selected.items.len(), 1);
        let row = &selected.items[0];
        assert_eq!(row.removal.revision, 17);
        assert_eq!(row.removal.bytes, Some(4096));
        assert!(row.removal.can_restore);
        assert_eq!(row.home.id, s.team.topic);
        assert!(
            row.home.name.is_none(),
            "the Manage inspection does not borrow optional node-name Read"
        );
        assert!(
            row.host.is_none(),
            "no removed runtime supplies an active host reference"
        );
    }

    #[tokio::test]
    async fn kept_list_keeps_under_cursor_and_current_key_scope_on_each_page() {
        let s = Scene::new().await;
        keep(&s).await;
        let reader = key(
            &s.world,
            s.team.c.admin.id(),
            rules(&s, &[s.first.base.id, s.second.base.id], "manage"),
        )
        .await;
        let query = NodeDeletedEnvironmentListQuery {
            under: Some(true),
            limit: Some(1),
            cursor: None,
        };
        let first = page(&s, &reader.secret, s.team.c.acme, &query).await;
        assert_eq!(first.items.len(), 1);
        let cursor = first
            .next_cursor
            .clone()
            .expect("one more native retained row");
        let second = page(
            &s,
            &reader.secret,
            s.team.c.acme,
            &NodeDeletedEnvironmentListQuery {
                cursor: Some(cursor),
                ..query.clone()
            },
        )
        .await;
        assert_eq!(second.items.len(), 1);
        assert_ne!(
            first.items[0].environment.id,
            second.items[0].environment.id
        );
        assert!(
            page(
                &s,
                &reader.secret,
                s.team.c.acme,
                &NodeDeletedEnvironmentListQuery {
                    under: Some(false),
                    ..Default::default()
                }
            )
            .await
            .items
            .is_empty()
        );
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
            .bind(reader.id)
            .bind(json!({"rules":rules(&s,&[second.items[0].environment.id],"manage")}))
            .execute(&s.world.db.pool)
            .await
            .unwrap();
        let narrowed = page(
            &s,
            &reader.secret,
            s.team.c.acme,
            &NodeDeletedEnvironmentListQuery {
                under: Some(true),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(narrowed.items.len(), 1);
        assert_eq!(
            narrowed.items[0].environment.id,
            second.items[0].environment.id
        );
    }
}

#[tokio::test]
async fn environment_list_publication_retains_the_native_management_field_gate_after_activity_wait()
{
    const MARKER: &str = "manager-config-publication-marker";
    for reduce_authority in [false, true] {
        let s = Scene::new().await;
        sqlx::query(
            "UPDATE environments SET config=jsonb_set(config,'{env.variables}',$2) WHERE id=$1",
        )
        .bind(s.first.base.id)
        .bind(json!({"MARKER":MARKER}))
        .execute(&s.world.db.pool)
        .await
        .unwrap();
        let grant = resources::name_person(
            &s.world,
            "environment",
            s.first.base.id,
            "environment.manage",
            s.team.c.viewer.id(),
        )
        .await;
        let reader = key(&s.world, s.team.c.viewer.id(), s.rules(&[s.first.base.id])).await;
        let before = s
            .visible(&reader.secret, &EnvironmentListQuery::default())
            .await;
        assert_eq!(before.len(), 1);
        assert_eq!(
            before[0].config["env.variables"]["MARKER"], MARKER,
            "the real scoped native renderer exposes config while this account manages it"
        );

        let mut blocker = s.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = s.world.app.clone();
        let secret = reader.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(EnvironmentList, &[]);
            call.token(&secret)
                .query(&EnvironmentListQuery::default())
                .send_bytes()
                .await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait,async {
            loop {
                assert!(!request.is_finished(),"the native reply did not reach activity publication");
                let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
                if blocked {break;}
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual completed Environment body must wait at activity append");
        if reduce_authority {
            assert!(
                orch_db::queries::access::remove_entry(
                    &mut *s.world.conn().await,
                    grant,
                    s.team.c.admin.id(),
                    s.world.now()
                )
                .await
                .unwrap()
            );
            let current = s
                .visible(&s.team.c.viewer.token, &EnvironmentListQuery::default())
                .await;
            let weaker = current
                .iter()
                .find(|row| row.base.id == s.first.base.id)
                .unwrap();
            assert_eq!(
                weaker.config,
                json!({}),
                "the native View-only projection really hides config while retaining the same Environment"
            );
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        if reduce_authority {
            assert_eq!(
                reply.status,
                StatusCode::FORBIDDEN,
                "matching source IDs cannot authorize an older management-rich body"
            );
            assert!(!String::from_utf8_lossy(&reply.raw).contains(MARKER));
        } else {
            assert_eq!(reply.status, StatusCode::OK);
            assert!(
                String::from_utf8_lossy(&reply.raw).contains(MARKER),
                "the unchanged native management body stays available"
            );
        }
    }
}
