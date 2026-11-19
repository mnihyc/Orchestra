//! Native Mount usage counts and references stay inside the consuming operation's scope.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

#[path = "workspace_snapshots/scope_helpers.rs"]
mod keys;
mod places;
mod support;
mod workspace;

use http::StatusCode;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::mounts::{MountUsesGet, MountUsesGetReply};
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::resources::access_fixtures;
use orch_types::{Id, Time};
use serde_json::json;
use workspace::Site;

struct Scene {
    site: Site,
    mount: Id,
    hidden_environment: Id,
}

impl Scene {
    async fn new() -> Self {
        let site = Site::new().await;
        let mut conn = site.world.conn().await;
        let mount = access_fixtures::insert_resource(
            &mut conn,
            "mount",
            Some(site.ada.id()),
            Some(site.topic),
            "native-usage-source",
        )
        .await
        .unwrap();
        start_resource(
            &mut conn,
            ObjectRef {
                kind: ObjectKind::Mount,
                id: mount,
            },
            site.ada.id(),
            NewAccess::OnlyMe,
            Time::new(site.world.now()),
        )
        .await
        .unwrap();
        let hidden_environment = access_fixtures::insert_environment(
            &mut conn,
            site.eve.id(),
            Some(site.eve.person.personal_org),
            "Private usage Environment",
            "connected",
        )
        .await
        .unwrap();
        start_resource(
            &mut conn,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: hidden_environment,
            },
            site.eve.id(),
            NewAccess::OnlyMe,
            Time::new(site.world.now()),
        )
        .await
        .unwrap();
        drop(conn);
        for target in [site.topic, site.eve.person.personal_org] {
            site.world
                .set_at(
                    orch_settings::Target::Node(target),
                    "env.mounts",
                    json!({mount.to_string():true}),
                )
                .await;
        }
        sqlx::query(
            "UPDATE environments SET facts=jsonb_set(facts,'{mounts}',$2::jsonb) WHERE id=$1",
        )
        .bind(site.environment)
        .bind(json!([{"mount_id":mount,"state":"attached"}]))
        .execute(&site.world.db.pool)
        .await
        .unwrap();
        Self {
            site,
            mount,
            hidden_environment,
        }
    }

    async fn key(&self, user: Id, sources: bool) -> keys::Key {
        let mut rules = vec![keys::rule(
            "MountUsesGet",
            "mount",
            self.mount,
            "self",
            &["read"],
        )];
        if sources {
            rules.extend([
                keys::rule("MountUsesGet", "topic", self.site.topic, "self", &["read"]),
                keys::rule(
                    "MountUsesGet",
                    "org",
                    self.site.eve.person.personal_org,
                    "self",
                    &["read"],
                ),
                keys::read("MountUsesGet", self.site.environment),
                keys::read("MountUsesGet", self.hidden_environment),
            ]);
        }
        keys::scoped(&self.site, user, rules).await
    }

    async fn read(&self, token: &str) -> RawReply {
        self.site
            .app()
            .call(MountUsesGet, &[&self.mount])
            .token(token)
            .send_bytes()
            .await
    }
}

fn shown(reply: RawReply) -> MountUsesGetReply {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

#[tokio::test]
async fn mount_usage_binds_its_actual_sources_before_names_and_hidden_counts() {
    let s = Scene::new().await;
    let ordinary = shown(s.read(&s.site.ada.token).await);
    assert_eq!(ordinary.scopes.len(), 1);
    assert_eq!(ordinary.scopes[0].node.id, s.site.topic);
    assert_eq!(ordinary.environments.len(), 1);
    assert_eq!(ordinary.environments[0].environment.id, s.site.environment);
    assert_eq!(ordinary.environments[0].state, "attached");
    assert_eq!(
        ordinary.hidden, 2,
        "the native hidden node and Environment are real selected sources"
    );
    let primary = s.key(s.site.ada.id(), false).await;
    assert_eq!(
        shown(s.read(&primary.secret).await),
        MountUsesGetReply {
            scopes: Vec::new(),
            environments: Vec::new(),
            hidden: 0,
        }
    );
    let full = s.key(s.site.ada.id(), true).await;
    assert_eq!(shown(s.read(&full.secret).await), ordinary);
}

#[tokio::test]
async fn mount_usage_requires_own_operation_read_for_each_context_family() {
    let s = Scene::new().await;
    let mut rules = vec![keys::rule(
        "MountUsesGet",
        "mount",
        s.mount,
        "self",
        &["read"],
    )];
    for (kind, id) in [
        ("topic", s.site.topic),
        ("org", s.site.eve.person.personal_org),
        ("environment", s.site.environment),
        ("environment", s.hidden_environment),
    ] {
        rules.push(keys::rule("MountGet", kind, id, "self", &["read"]));
    }
    let other = keys::scoped(&s.site, s.site.ada.id(), rules).await;
    let empty = shown(s.read(&other.secret).await);
    assert!(empty.scopes.is_empty());
    assert!(empty.environments.is_empty());
    assert_eq!(empty.hidden, 0);
    let nodes = keys::scoped(
        &s.site,
        s.site.ada.id(),
        vec![
            keys::rule("MountUsesGet", "mount", s.mount, "self", &["read"]),
            keys::rule("MountUsesGet", "topic", s.site.topic, "self", &["read"]),
        ],
    )
    .await;
    let nodes = shown(s.read(&nodes.secret).await);
    assert_eq!(nodes.scopes.len(), 1);
    assert!(nodes.environments.is_empty());
    assert_eq!(nodes.hidden, 0);
    let environments = keys::scoped(
        &s.site,
        s.site.ada.id(),
        vec![
            keys::rule("MountUsesGet", "mount", s.mount, "self", &["read"]),
            keys::read("MountUsesGet", s.site.environment),
        ],
    )
    .await;
    let environments = shown(s.read(&environments.secret).await);
    assert!(environments.scopes.is_empty());
    assert_eq!(environments.environments.len(), 1);
    assert_eq!(environments.hidden, 0);
}

#[tokio::test]
async fn mount_usage_keeps_typed_subject_current_key_and_ordinary_mount_view() {
    let s = Scene::new().await;
    let wrong = keys::scoped(
        &s.site,
        s.site.ada.id(),
        vec![keys::rule(
            "MountUsesGet",
            "environment",
            s.mount,
            "self",
            &["read"],
        )],
    )
    .await;
    assert_eq!(s.read(&wrong.secret).await.status, StatusCode::FORBIDDEN);
    let no_account_view = s.key(s.site.vic.id(), true).await;
    assert_eq!(
        s.read(&no_account_view.secret).await.status,
        StatusCode::FORBIDDEN
    );
    let key = s.key(s.site.ada.id(), true).await;
    assert_eq!(shown(s.read(&key.secret).await).environments.len(), 1);
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(key.id)
        .bind(serde_json::to_value(orch_access_contract::key_scope::KeyScope::default()).unwrap())
        .execute(&s.site.world.db.pool)
        .await
        .unwrap();
    let denied = s.read(&key.secret).await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&denied.raw).contains("Workspace"));
}

#[tokio::test]
async fn mount_usage_publication_cannot_borrow_a_new_empty_selection_after_real_activity_wait() {
    let s = Scene::new().await;
    let key = keys::scoped(
        &s.site,
        s.site.ada.id(),
        vec![
            keys::rule("MountUsesGet", "mount", s.mount, "self", &["read"]),
            keys::rule("MountUsesGet", "topic", s.site.topic, "self", &["read"]),
            keys::read("MountUsesGet", s.site.environment),
        ],
    )
    .await;
    let control = shown(s.read(&key.secret).await);
    assert_eq!(control.scopes.len(), 1);
    assert_eq!(control.environments.len(), 1);
    let mut blocker = s.site.world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = s.site.world.app.clone();
    let mount = s.mount;
    let secret = key.secret.clone();
    let request = tokio::spawn(async move {
        let request = app.call(MountUsesGet, &[&mount]).token(&secret);
        request.send_bytes().await
    });
    let wait = std::time::Duration::from_secs(30);
    tokio::time::timeout(wait,async {
        loop {
            assert!(!request.is_finished(),"the response did not reach its actual activity append");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                WHERE datname=current_database() AND wait_event_type='Lock'
                AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&s.site.world.db.pool).await.unwrap();
            if blocked {break;}
            tokio::task::yield_now().await;
        }
    }).await.expect("the actual native projection must wait at activity bookkeeping");
    s.site
        .world
        .set_at(
            orch_settings::Target::Node(s.site.topic),
            "env.mounts",
            json!({}),
        )
        .await;
    blocker.commit().await.unwrap();
    let held = tokio::time::timeout(wait, request).await.unwrap().unwrap();
    assert_eq!(held.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&held.raw).contains("Workspace"));
    let current = shown(s.read(&key.secret).await);
    assert!(current.scopes.is_empty());
    assert!(current.environments.is_empty());
    assert_eq!(current.hidden, 0);
}
