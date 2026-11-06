//! Same-operation scopes bound the actual compound reach payloads; ordinary rights stay independent.

use super::*;
use axum::body::{Body, to_bytes};
use http::{Request, StatusCode};
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::KeyScope,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::common::{ErrorCode, Problem};
use orch_api_contract::environment::EnvironmentReachGet;
use orch_api_contract::mounts::{MountCreate, MountCreateBody, MountGet};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::registry::Registry;
use orch_api_core::testing::RawReply;
use orch_db::queries::{identity, resources::mounts};
use orch_types::{Json, Time};
use serde_json::json;
use std::{collections::BTreeSet, time::Duration};
use tower::ServiceExt;

// Reuse the existing C01/provider-read identity rendezvous watchdog and JSON envelope bound.
const LOCK_WAIT: Duration = Duration::from_secs(30);
const MAX_JSON_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
enum Reader {
    Reach,
    Agent,
}
impl Reader {
    const ALL: [Self; 2] = [Self::Reach, Self::Agent];
    fn operation(self) -> &'static str {
        match self {
            Self::Reach => "EnvironmentReachGet",
            Self::Agent => "EnvironmentAgentReachGet",
        }
    }
    fn path(self, id: Id) -> String {
        format!(
            "/environments/{id}/{}",
            match self {
                Self::Reach => "reach",
                Self::Agent => "agent-reach",
            }
        )
    }
    async fn read(self, s: &Scene, token: &str) -> RawReply {
        match self {
            Self::Reach => {
                s.world
                    .app
                    .call(EnvironmentReachGet, &[&s.environment.base.id])
                    .token(token)
                    .send_bytes()
                    .await
            }
            Self::Agent => {
                s.world
                    .app
                    .call(EnvironmentAgentReachGet, &[&s.environment.base.id])
                    .token(token)
                    .send_bytes()
                    .await
            }
        }
    }
}

struct Key {
    id: Id,
    secret: String,
}
fn rule(operation: &str, kind: &str, id: Id, reach: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"allow":[{"operation":operation,"actions":actions}]})
}
fn read_rule(reader: Reader, kind: &str, id: Id) -> Json {
    let mut rule = rule(reader.operation(), kind, id, "self", &["read"]);
    rule["resources"] = json!([kind]);
    rule
}
fn full_rules(s: &Scene, reader: Reader) -> Vec<Json> {
    vec![rule(
        reader.operation(),
        "org",
        s.people.acme,
        "subtree",
        &["read"],
    )]
}
async fn key(s: &Scene, user: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(
        scope.validate().is_empty(),
        "fixture uses a structurally valid operation scope"
    );
    let selections = orch_api_contract::operations::catalogue()
        .unwrap()
        .into_iter()
        .flat_map(|operation| {
            operation
                .variants
                .into_iter()
                .map(|variant| variant.operation)
        })
        .collect::<BTreeSet<_>>();
    for rule in &scope.rules {
        for selected in &rule.allow {
            assert!(
                selections.contains(&selected.operation),
                "fixture operation selection must exist: {}",
                selected.operation
            );
        }
    }
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *s.world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Environment reach source control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: s.world.now(),
            expires_at: Some(s.world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    Key { id, secret }
}
async fn legacy(s: &Scene, user: Id, node: Option<Id>) -> Key {
    let scopes = vec!["*".to_owned()];
    orch_authz::TokenGrant::from_stored(&scopes, node)
        .expect("concrete legacy wildcard fixture is valid");
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *s.world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Ordinary reach permission control",
            token_hash: &token_hash(&secret),
            scopes: &scopes,
            node_id: node,
            at: s.world.now(),
            expires_at: Some(s.world.now() + chrono::Duration::hours(1)),
        },
        None,
    )
    .await
    .unwrap();
    Key { id, secret }
}
fn shown(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn threads(reader: Reader, body: &Json) -> BTreeSet<Id> {
    let ids: Vec<&Json> = match reader {
        Reader::Agent => body["credentials"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| &row["thread"]["id"])
            .collect(),
        Reader::Reach => body["people"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|person| person["through"].as_array().unwrap())
            .filter(|way| way["kind"] == "thread")
            .map(|way| &way["thread"]["id"])
            .collect(),
    };
    ids.into_iter()
        .map(|id| serde_json::from_value(id.clone()).unwrap())
        .collect()
}
async fn footprint(s: &Scene) -> Json {
    sqlx::query_scalar("SELECT jsonb_build_object('commands',(SELECT count(*) FROM commands),'jobs',(SELECT count(*) FROM jobs),'turns',(SELECT count(*) FROM turns))")
        .fetch_one(&s.world.db.pool).await.unwrap()
}
async fn scene() -> Scene {
    let s = Scene::new().await;
    // The parent Scene inserts raw nodes/threads/chat rows. Its owner_id identifies
    // the intended maker but does not create the live Owners page as ThreadCreate does.
    let owners = [
        (s.visible, s.people.boss.id()),
        (s.unbound, s.people.boss.id()),
        (s.hidden, s.people.stranger.id()),
    ];
    {
        let mut conn = s.world.conn().await;
        for (thread, owner) in owners {
            let recorded: Id = sqlx::query_scalar("SELECT owner_id FROM nodes WHERE id=$1")
                .bind(thread)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
            assert_eq!(
                recorded, owner,
                "fixture completes only its already-recorded intended owner"
            );
            orch_db::queries::access::testing::name_owner(
                &mut conn,
                thread,
                ("user", owner),
                s.world.now(),
            )
            .await
            .unwrap();
        }
    }
    // These are real ordinary Use grants; a raw binding never grants its owner access to the environment.
    resources::name_person(
        &s.world,
        "environment",
        s.environment.base.id,
        "environment.use",
        s.people.boss.id(),
    )
    .await;
    resources::name_person(
        &s.world,
        "environment",
        s.environment.base.id,
        "environment.use",
        s.people.stranger.id(),
    )
    .await;
    {
        let mut conn = s.world.conn().await;
        let reader = orch_authz::Principal::load(
            &mut conn,
            orch_authz::Caller::session(s.people.admin.id()),
            Time::new(s.world.now()),
        )
        .await
        .unwrap();
        for thread in [s.visible, s.unbound] {
            assert!(
                orch_authz::nodes::check(
                    &mut conn,
                    &reader,
                    thread,
                    Permission::ThreadView,
                    orch_authz::Act::Use,
                    orch_authz::Policy::default()
                )
                .await
                .unwrap()
                .is_allowed(),
                "ordinary reader must actually hold ThreadView without an As-admin loan"
            );
        }
        for (thread, owner) in owners {
            let owner = orch_authz::Principal::load(
                &mut conn,
                orch_authz::Caller::session(owner),
                Time::new(s.world.now()),
            )
            .await
            .unwrap();
            assert!(
                orch_authz::nodes::check(
                    &mut conn,
                    &owner,
                    thread,
                    Permission::ThreadInteract,
                    orch_authz::Act::Use,
                    orch_authz::Policy::default()
                )
                .await
                .unwrap()
                .is_allowed(),
                "the actual live Owners entry supplies the connected Thread's Interact"
            );
            assert!(
                orch_authz::resources::check(
                    &mut conn,
                    &owner,
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id: s.environment.base.id
                    },
                    Permission::EnvironmentUse,
                    orch_authz::Policy::default()
                )
                .await
                .unwrap()
                .is_allowed(),
                "a Thread owner reaches this environment only through the independently granted EnvUse"
            );
        }
    }
    for reader in Reader::ALL {
        let baseline = shown(reader.read(&s, &s.people.admin.token).await);
        assert!(
            threads(reader, &baseline).contains(&s.visible),
            "ordinary visible Thread source is a required fixture premise: {reader:?}"
        );
        assert!(threads(reader, &baseline).contains(&s.unbound));
        assert!(!threads(reader, &baseline).contains(&s.hidden));
        if matches!(reader, Reader::Reach) {
            assert!(
                baseline["hidden"].as_i64().unwrap() > 0,
                "ordinary hidden audience is genuinely present"
            );
        }
    }
    s
}

async fn full(reader: Reader) {
    let s = scene().await;
    let ordinary = shown(reader.read(&s, &s.people.admin.token).await);
    let token = key(&s, s.people.admin.id(), full_rules(&s, reader)).await;
    let before = footprint(&s).await;
    let actual = shown(reader.read(&s, &token.secret).await);
    assert_eq!(
        actual, ordinary,
        "sufficient same-operation sources preserve the ordinary business reply"
    );
    assert_eq!(
        footprint(&s).await,
        before,
        "neither read starts work or native probing"
    );
}
#[tokio::test]
async fn reach_accepts_its_actual_environment_and_returned_thread_read_sources() {
    full(Reader::Reach).await;
}
#[tokio::test]
async fn agent_reach_accepts_its_actual_environment_and_compound_read_sources() {
    full(Reader::Agent).await;
}

#[tokio::test]
async fn returned_thread_ways_and_credentials_require_that_same_operation_on_each_thread() {
    let s = scene().await;
    let provider = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut *s.world.conn().await,
        "provider",
        Some(s.people.admin.id()),
        Some(s.people.acme),
        "Recorded copy source",
    )
    .await
    .unwrap();
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM environments WHERE id=$1")
        .bind(s.environment.base.id)
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
    let expires = Time::new(s.world.now() + chrono::Duration::hours(1));
    sqlx::query("UPDATE threads SET arrangement='credential_shared' WHERE node_id=$1")
        .bind(s.visible)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO leases(id,provider_id,thread_id,mode,recipient_machine_id,copy_expires_at,allowed_models,opened_at,closed_at,close_reason) VALUES($1,$2,$3,'shared',$4,$5,'{}',$6,$6,'turn_ended')")
        .bind(new_id()).bind(provider).bind(s.visible).bind(machine).bind(expires.get()).bind(s.world.now()).execute(&s.world.db.pool).await.unwrap();
    for reader in Reader::ALL {
        let ordinary = shown(reader.read(&s, &s.people.admin.token).await);
        assert!(threads(reader, &ordinary).contains(&s.visible));
        let root = read_rule(reader, "environment", s.environment.base.id);
        let root_only = key(&s, s.people.admin.id(), vec![root.clone()]).await;
        let limited = shown(reader.read(&s, &root_only.secret).await);
        assert!(
            threads(reader, &limited).is_empty(),
            "Env Read cannot lend returned Thread content"
        );
        if matches!(reader, Reader::Reach) {
            assert_eq!(
                limited["hidden"], ordinary["hidden"],
                "scope-omitted rows are not added to the ordinary hidden count"
            );
            assert!(
                limited["people"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|person| person["user"]["id"] == json!(s.people.admin.id())),
                "direct Environment ownership remains visible"
            );
        }
        let unrelated = key(
            &s,
            s.people.admin.id(),
            vec![
                root.clone(),
                rule("ThreadGoalGet", "thread", s.visible, "self", &["read"]),
            ],
        )
        .await;
        assert!(threads(reader, &shown(reader.read(&s, &unrelated.secret).await)).is_empty());
        let selected = key(
            &s,
            s.people.admin.id(),
            vec![root, read_rule(reader, "thread", s.visible)],
        )
        .await;
        let selected = shown(reader.read(&s, &selected.secret).await);
        assert_eq!(threads(reader, &selected), BTreeSet::from([s.visible]));
        if matches!(reader, Reader::Agent) {
            let line = &selected["credentials"][0];
            assert_eq!(line["placement"], "credential_shared");
            assert_eq!(
                line["expires_at"],
                json!(expires),
                "this is real recorded Thread content, not only its name"
            );
            let original = ordinary["credentials"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["thread"]["id"] == json!(s.visible))
                .unwrap();
            assert_eq!(
                line["thread"], original["thread"],
                "selected Thread Read covers its current name and display"
            );
        }
        assert!(!selected.to_string().contains("Hidden handover"));
    }
}

#[tokio::test]
async fn optional_home_names_do_not_become_prerequisites_for_environment_values() {
    let s = scene().await;
    let ordinary = shown(Reader::Agent.read(&s, &s.people.admin.token).await);
    assert_eq!(
        ordinary["secrets"]["names"],
        json!(["DEPLOY_TOKEN", "NPM_TOKEN"])
    );
    let key_only = key(
        &s,
        s.people.admin.id(),
        vec![read_rule(
            Reader::Agent,
            "environment",
            s.environment.base.id,
        )],
    )
    .await;
    let limited = shown(Reader::Agent.read(&s, &key_only.secret).await);
    assert_eq!(
        limited["secrets"]["names"], ordinary["secrets"]["names"],
        "resolved Env values retain existing EnvironmentManage authority"
    );
    assert_eq!(limited["network"]["mode"], ordinary["network"]["mode"]);
    for node in limited["home_path"].as_array().unwrap() {
        assert!(node["name"].is_null());
        assert_eq!(node["redacted"], true);
        assert!(node["thread_display"].is_null());
    }
    assert!(limited["secrets"]["source"]["name"].is_null());
    assert_eq!(
        limited["secrets"]["source"]["id"],
        ordinary["secrets"]["source"]["id"]
    );
    let full = key(&s, s.people.admin.id(), full_rules(&s, Reader::Agent)).await;
    assert_eq!(shown(Reader::Agent.read(&s, &full.secret).await), ordinary);
}

#[tokio::test]
async fn scope_does_not_replace_the_existing_whole_cut_and_no_environment_doors() {
    let s = scene().await;
    let ordinary = shown(Reader::Agent.read(&s, &s.people.viewer.token).await);
    assert!(
        ordinary["home_path"].is_null()
            && ordinary["mounts"].is_null()
            && ordinary["browser_profiles"].is_null()
    );
    assert!(ordinary["secrets"]["names"].is_null());
    let admitted = key(&s, s.people.viewer.id(), full_rules(&s, Reader::Agent)).await;
    assert_eq!(
        shown(Reader::Agent.read(&s, &admitted.secret).await),
        ordinary,
        "the same account retains its ThroughAThread cut"
    );
    assert_eq!(
        Reader::Reach.read(&s, &s.people.viewer.token).await.status,
        StatusCode::NOT_FOUND
    );
    let no_page = key(&s, s.people.viewer.id(), full_rules(&s, Reader::Reach)).await;
    assert_eq!(
        Reader::Reach.read(&s, &no_page.secret).await.status,
        StatusCode::NOT_FOUND,
        "key scope never gives an Environment page right"
    );
    for reader in Reader::ALL {
        let no_env_scope = key(
            &s,
            s.people.viewer.id(),
            vec![read_rule(reader, "thread", s.visible)],
        )
        .await;
        assert_eq!(
            reader.read(&s, &no_env_scope.secret).await.status,
            StatusCode::FORBIDDEN,
            "Thread scope alone cannot supply an Environment target"
        );
    }
}

#[tokio::test]
async fn ordinary_legacy_views_and_scoped_operation_action_target_limits_remain_independent() {
    let s = scene().await;
    let before = footprint(&s).await;
    for reader in Reader::ALL {
        let ordinary = shown(reader.read(&s, &s.people.admin.token).await);
        let token = legacy(&s, s.people.admin.id(), None).await;
        assert_eq!(shown(reader.read(&s, &token.secret).await), ordinary);
        let bounded = legacy(&s, s.people.viewer.id(), Some(s.people.acme)).await;
        let bounded_reply = reader.read(&s, &bounded.secret).await;
        if matches!(reader, Reader::Reach) {
            assert_eq!(
                bounded_reply.status,
                StatusCode::NOT_FOUND,
                "a legacy Thread path gives no Environment page right"
            );
        } else {
            let cut = shown(Reader::Agent.read(&s, &s.people.viewer.token).await);
            assert_eq!(
                shown(bounded_reply),
                cut,
                "the established ThroughAThread cut remains available within the legacy subtree"
            );
        }
        for rules in [
            vec![rule(
                reader.operation(),
                "environment",
                s.environment.base.id,
                "self",
                &["manage"],
            )],
            vec![rule(
                "EnvironmentGet.read",
                "environment",
                s.environment.base.id,
                "self",
                &["read"],
            )],
            vec![read_rule(reader, "thread", s.visible)],
            vec![read_rule(reader, "environment", new_id())],
        ] {
            let denied = key(&s, s.people.admin.id(), rules).await;
            let reply = reader.read(&s, &denied.secret).await;
            assert_eq!(reply.status, StatusCode::FORBIDDEN);
            let body = String::from_utf8_lossy(&reply.raw);
            assert!(
                !body.contains("DEPLOY_TOKEN")
                    && !body.contains("Visible work")
                    && !body.contains("Hidden handover")
            );
        }
    }
    assert_eq!(footprint(&s).await, before);
}

async fn attached_mounts(s: &Scene) -> Vec<Id> {
    use orch_api_contract::access::NewResourceAccess;
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM environments WHERE id=$1")
        .bind(s.environment.base.id)
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
    let descriptor = orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
        "fixture",
        orch_driver_contract::descriptor::EnvironmentDataStorage {
            made_by_orch: None,
            existing_directory: true,
            reason: None,
        },
    );
    sqlx::query(
        "UPDATE machines SET facts=jsonb_set(facts,'{environment_descriptor}',$2) WHERE id=$1",
    )
    .bind(machine)
    .bind(json!(descriptor))
    .execute(&s.world.db.pool)
    .await
    .unwrap();
    let mut ids = vec![];
    let mut enabled = serde_json::Map::new();
    let mut facts = vec![];
    for name in ["scope-mount-a", "scope-mount-b"] {
        let made = s
            .world
            .app
            .call(MountCreate, &[])
            .token(&s.people.boss.token)
            .body(&MountCreateBody {
                name: name.into(),
                description: None,
                home_id: Some(s.people.acme),
                target: format!("/mnt/{name}"),
                read_only: Some(true),
                default_fate: None,
                access: Some(NewResourceAccess::OnlyMe),
            })
            .send()
            .await
            .ok();
        let id = made.base.id;
        let path = format!("/srv/ada/{name}");
        resources::name_person(&s.world, "mount", id, "mount.use", s.people.admin.id()).await;
        mounts::set_location(
            &mut *s.world.conn().await,
            &mounts::LocationRow {
                mount_id: id,
                machine_id: machine,
                host_id: None,
                host_name: None,
                kind: "path".into(),
                source: path.clone(),
                storage: None,
                data_kind: None,
                created_by: s.people.admin.id(),
                created_at: s.world.now(),
            },
        )
        .await
        .unwrap();
        enabled.insert(id.to_string(), json!("read_only"));
        facts.push(json!({"mount_id":id,"state":"attached","path":path,"mode":"read_only"}));
        ids.push(id);
    }
    sqlx::query("UPDATE environments SET config=jsonb_set(config,'{env.mounts}',$2),facts=jsonb_set(facts,'{mounts}',$3) WHERE id=$1")
        .bind(s.environment.base.id).bind(Json::Object(enabled)).bind(json!(facts)).execute(&s.world.db.pool).await.unwrap();
    let ordinary = shown(Reader::Agent.read(s, &s.people.admin.token).await);
    assert_eq!(
        ordinary["mounts"].as_array().unwrap().len(),
        2,
        "both real resolver-valid Attached candidates are necessary fixture premises"
    );
    ids
}

#[tokio::test]
async fn attached_mount_content_requires_same_operation_scope_but_no_new_account_mount_view() {
    let s = scene().await;
    let mounts = attached_mounts(&s).await;
    resources::name_person(
        &s.world,
        "environment",
        s.environment.base.id,
        "files.view",
        s.people.viewer.id(),
    )
    .await;
    let ordinary = shown(Reader::Agent.read(&s, &s.people.viewer.token).await);
    assert_eq!(ordinary["mounts"].as_array().unwrap().len(), 2);
    let mut conn = s.world.conn().await;
    let principal = orch_authz::Principal::load(
        &mut conn,
        orch_authz::Caller::session(s.people.viewer.id()),
        Time::new(s.world.now()),
    )
    .await
    .unwrap();
    assert!(
        !orch_authz::resources::check(
            &mut conn,
            &principal,
            ObjectRef {
                kind: ObjectKind::Mount,
                id: mounts[0]
            },
            Permission::MountView,
            orch_authz::Policy::default()
        )
        .await
        .unwrap()
        .is_allowed()
    );
    drop(conn);
    assert!(
        !s.world
            .app
            .call(MountGet, &[&mounts[0]])
            .token(&s.people.viewer.token)
            .send_bytes()
            .await
            .status
            .is_success()
    );
    let root = read_rule(Reader::Agent, "environment", s.environment.base.id);
    let root_only = key(&s, s.people.viewer.id(), vec![root.clone()]).await;
    assert_eq!(
        shown(Reader::Agent.read(&s, &root_only.secret).await)["mounts"],
        json!([])
    );
    let other_endpoint = key(
        &s,
        s.people.viewer.id(),
        vec![
            root.clone(),
            rule("MountGet", "mount", mounts[0], "self", &["read"]),
        ],
    )
    .await;
    assert_eq!(
        shown(Reader::Agent.read(&s, &other_endpoint.secret).await)["mounts"],
        json!([])
    );
    let selected = key(
        &s,
        s.people.viewer.id(),
        vec![root, read_rule(Reader::Agent, "mount", mounts[0])],
    )
    .await;
    let actual = shown(Reader::Agent.read(&s, &selected.secret).await);
    let expected = ordinary["mounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["mount"]["id"] == json!(mounts[0]))
        .unwrap();
    assert_eq!(actual["mounts"], json!([expected]));
    assert!(!actual.to_string().contains("scope-mount-b"));
    assert!(actual["secrets"]["names"].is_null() && actual["browser_profiles"].is_null());
}

struct ReportedProfile {
    environment: Id,
}
#[async_trait::async_trait]
impl orch_api_core::testing::Machine for ReportedProfile {
    async fn run(
        &self,
        command: &orch_link_contract::commands::Cmd,
    ) -> Result<Json, orch_link_contract::commands::CommandError> {
        if command.kind != "screen.look"
            || command.body["environment_id"] != json!(self.environment)
        {
            return Err(orch_link_contract::commands::CommandError::new(
                orch_link_contract::commands::ErrorCode::Unsupported,
                "the profile fixture supplies only its exact environment report",
            ));
        }
        Ok(json!(orch_link_contract::browser::ScreenLookResult {
            browser: Default::default(),
            vnc: Default::default(),
            profiles: vec![orch_link_contract::browser::BrowserProfile {
                name: "retained-work-profile".into(),
                size_bytes: Some(4096),
                used_at: None,
                browser: None
            }],
            browsers: vec![],
            displays: vec![],
        }))
    }
}

#[tokio::test]
async fn existing_profile_names_belong_to_the_environment_read_without_a_new_browser_target_or_probe()
 {
    let s = scene().await;
    let environment = s.environment.base.id;
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM environments WHERE id=$1")
        .bind(environment)
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET online=true,roles=ARRAY['environment'] WHERE id=$1")
        .bind(machine)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET status='running' WHERE id=$1")
        .bind(environment)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    // Seed the ordinary cache through its existing report path. Only setup asks ScreenLook;
    // the actual operation below must consume that exact cache without another native request.
    let peer = s.world.app.play(
        machine,
        std::sync::Arc::new(ReportedProfile { environment }),
    );
    s.world
        .app
        .app
        .engine
        .browsers()
        .look(environment)
        .await
        .unwrap();
    let ordinary = shown(Reader::Agent.read(&s, &s.people.admin.token).await);
    assert_eq!(
        ordinary["browser_profiles"],
        json!([{"name":"retained-work-profile"}])
    );
    let token = key(
        &s,
        s.people.admin.id(),
        vec![read_rule(Reader::Agent, "environment", environment)],
    )
    .await;
    let before = footprint(&s).await;
    let actual = shown(Reader::Agent.read(&s, &token.secret).await);
    assert_eq!(actual["browser_profiles"], ordinary["browser_profiles"]);
    assert_eq!(
        footprint(&s).await,
        before,
        "reading a reported profile never probes or starts a browser"
    );
    assert_eq!(
        actual["browser_profiles"][0].as_object().unwrap().len(),
        1,
        "the overview exposes only the profile name"
    );
    drop(peer);
}

async fn checkpoint(reader: Reader, narrow: bool) {
    let s = scene().await;
    let token = key(&s, s.people.admin.id(), full_rules(&s, reader)).await;
    let mut blocker = s.world.db.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_person(&mut blocker, s.people.admin.id())
        .await
        .unwrap()
        .unwrap();
    sqlx::query("SELECT id FROM api_tokens WHERE id=$1 AND user_id=$2 FOR UPDATE")
        .bind(token.id)
        .bind(s.people.admin.id())
        .execute(&mut *blocker)
        .await
        .unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = registry
        .into_router_for(orch_api_contract::surface::HttpSurface::Key)
        .with_state(s.world.app.app.clone());
    let request = Request::builder()
        .method("GET")
        .uri(reader.path(s.environment.base.id))
        .header("authorization", format!("Bearer {}", token.secret))
        .body(Body::empty())
        .unwrap();
    let action = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
    let observed=tokio::time::timeout(LOCK_WAIT,async {
        loop {
            if action.is_finished(){break false;}
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))")
                .bind(blocker_pid).fetch_one(&s.world.db.pool).await.unwrap();
            if waiting {break true;}tokio::task::yield_now().await;
        }
    }).await.expect("the real read waits at its current identity or finishes");
    if narrow {
        let scope: KeyScope =
            serde_json::from_value(json!({"rules":[read_rule(reader,"thread",s.visible)]}))
                .unwrap();
        assert!(scope.validate().is_empty());
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
            .bind(token.id)
            .bind(json!(scope))
            .execute(&mut *blocker)
            .await
            .unwrap();
    }
    blocker.commit().await.unwrap();
    let response = tokio::time::timeout(LOCK_WAIT, action)
        .await
        .expect("released reader finishes")
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), MAX_JSON_BYTES)
        .await
        .unwrap();
    assert_eq!(
        status,
        if narrow {
            StatusCode::FORBIDDEN
        } else {
            StatusCode::OK
        },
        "{reader:?}: actual identity lock observed={observed}; {}",
        String::from_utf8_lossy(&bytes)
    );
    assert!(
        observed,
        "the actual read checkpoint must wait on its current identity"
    );
    if narrow {
        let problem: Problem = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(problem.code, ErrorCode::AuthzDenied);
        assert!(!String::from_utf8_lossy(&bytes).contains("DEPLOY_TOKEN"));
    }
}
#[tokio::test]
async fn both_reach_readers_recheck_current_scope_on_their_actual_snapshot_connection() {
    for reader in Reader::ALL {
        checkpoint(reader, false).await;
        checkpoint(reader, true).await;
    }
}
