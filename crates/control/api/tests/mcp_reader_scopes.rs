//! MCP readers keep their ordinary subjects, complete collections and actual content sources.
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
use orch_access_contract::key_scope::KeyScope;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::access::NewResourceAccess;
use orch_api_contract::directory::{GroupMemberAdd, OrgGroupCreate, OrgGroupCreateBody};
use orch_api_contract::resources::{
    McpServerCreate, McpServerCreateBody, McpServerGet, McpServerList, McpServerListQuery,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::routes::resources::sealing::install_master_key;
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::identity::{self, NewApiToken};
use orch_db::queries::resources::{
    access_fixtures as fixtures,
    mcp::{self as rows, McpRow},
};
use orch_db::secrets::MasterKey;
use orch_types::{Hash, Id, Json, Time, new_id};
use serde_json::json;
use support::{Statements, World};

struct Key {
    id: Id,
    secret: String,
}
struct Scene {
    world: World,
    company: resources::Company,
}
impl Scene {
    async fn new() -> Self {
        let world = World::new().await;
        let company = resources::company(&world).await;
        Self { world, company }
    }
}
#[derive(Clone, Copy)]
enum Reader {
    List,
    Get,
}
impl Reader {
    fn operation(self) -> &'static str {
        match self {
            Self::List => "McpServerList",
            Self::Get => "McpServerGet",
        }
    }
    async fn read(self, s: &Scene, token: &str, id: Id) -> RawReply {
        match self {
            Self::List => list(s, token, &Default::default()).await,
            Self::Get => get(s, token, id).await,
        }
    }
    fn item(self, response: &Json, id: Id) -> &Json {
        match self {
            Self::List => row(response, id),
            Self::Get => response,
        }
    }
}
fn spec(marker: &str) -> Json {
    json!({"transport":"stdio","command":["tool-server"],"secret_env":[],"env":{"PUBLIC_MARKER":marker},"tools_allow":["search"]})
}
async fn start(s: &Scene, kind: ResourceKind, id: Id, owner: Id) {
    start_resource(
        &mut *s.world.conn().await,
        ObjectRef {
            kind: kind.into(),
            id,
        },
        owner,
        NewAccess::OnlyMe,
        Time::new(s.world.now()),
    )
    .await
    .unwrap();
}
async fn preset(s: &Scene, name: &str, home: Option<Id>) -> Id {
    let id = new_id();
    let org = match home {
        Some(home) => sqlx::query_scalar("SELECT org_id FROM nodes WHERE id=$1")
            .bind(home)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap(),
        None => None,
    };
    rows::insert(
        &mut *s.world.conn().await,
        &McpRow {
            id,
            name: name.into(),
            description: format!("Description {name}"),
            org_id: org,
            home_id: home,
            owner_id: Some(s.company.boss.id()),
            machine_id: None,
            environment_id: None,
            spec: spec(name),
            imported: false,
            enabled: true,
            revision: 1,
        },
        s.world.now(),
    )
    .await
    .unwrap();
    start(s, ResourceKind::McpServer, id, s.company.boss.id()).await;
    id
}
async fn environment(s: &Scene, machine: Option<Id>, name: &str) -> Id {
    let mut conn = s.world.conn().await;
    let id = fixtures::insert_resource(
        &mut conn,
        "environment",
        Some(s.company.boss.id()),
        Some(s.company.acme),
        name,
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE environments SET placement='connected',template_id=NULL,machine_id=$2 WHERE id=$1",
    )
    .bind(id)
    .bind(machine)
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    start(s, ResourceKind::Environment, id, s.company.boss.id()).await;
    id
}
async fn own_server(s: &Scene, environment: Id, name: &str) -> Id {
    let id = new_id();
    rows::insert(
        &mut *s.world.conn().await,
        &McpRow {
            id,
            name: name.into(),
            description: format!("Environment server {name}"),
            org_id: Some(s.company.acme),
            home_id: Some(s.company.acme),
            owner_id: Some(s.company.boss.id()),
            machine_id: None,
            environment_id: Some(environment),
            spec: spec(name),
            imported: false,
            enabled: true,
            revision: 1,
        },
        s.world.now(),
    )
    .await
    .unwrap();
    id // Environment-owned servers intentionally have no separate MCP access page.
}
async fn personal(s: &Scene) -> Id {
    fixtures::insert_persons_machine(
        &mut *s.world.conn().await,
        s.company.boss.id(),
        "MCP physical source",
    )
    .await
    .unwrap()
}
async fn hosted(s: &Scene) -> (Id, Id) {
    let machine = personal(s).await;
    sqlx::query("UPDATE machines SET roles=ARRAY['node','environment'] WHERE id=$1")
        .bind(machine)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    let host = new_id();
    sqlx::query("INSERT INTO hosts(id,machine_id,owner_id,home_id,org_id,name) SELECT $1,$2,$3,id,org_id,'Canonical MCP host' FROM nodes WHERE id=$4")
        .bind(host).bind(machine).bind(s.company.boss.id()).bind(s.company.acme).execute(&s.world.db.pool).await.unwrap();
    start(s, ResourceKind::Host, host, s.company.boss.id()).await;
    (machine, host)
}
fn origin(s: &Scene) -> Json {
    json!({"source":"codex","scope":"user","directory":null,"name":"native-tool","digest":"original",
        "definition_fingerprint":Hash::of(b"original MCP definition"),"at":Time::new(s.world.now()),"by":s.company.boss.id()})
}
async fn imported(s: &Scene, name: &str, machine: Id) -> Id {
    let id = new_id();
    rows::insert(
        &mut *s.world.conn().await,
        &McpRow {
            id,
            name: name.into(),
            description: format!("Imported {name}"),
            org_id: Some(s.company.acme),
            home_id: Some(s.company.acme),
            owner_id: Some(s.company.boss.id()),
            machine_id: Some(machine),
            environment_id: None,
            spec: json!({"transport":"stdio","imported":origin(s)}),
            imported: true,
            enabled: true,
            revision: 1,
        },
        s.world.now(),
    )
    .await
    .unwrap();
    start(s, ResourceKind::McpServer, id, s.company.boss.id()).await;
    id
}
fn rule(operation: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":["read"]}]})
}
fn inside(operation: &str, org: Id) -> Json {
    json!({"target":{"kind":"org","id":org,"reach":"subtree"},"allow":[{"operation":operation,"actions":["read"]}]})
}
fn primary(operation: &str, id: Id, home: Id) -> Vec<Json> {
    vec![
        rule(operation, "mcp_server", id),
        rule(operation, "org", home),
    ]
}
async fn issue(s: &Scene, user: Id, rules: Option<Vec<Json>>) -> Key {
    let policy = rules.map(|rules| {
        let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
        assert!(scope.validate().is_empty());
        json!(scope)
    });
    let scopes = if policy.is_none() {
        vec!["*".to_owned()]
    } else {
        Vec::new()
    };
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *s.world.conn().await,
        &NewApiToken {
            id,
            user_id: user,
            name: "MCP reader control",
            token_hash: &token_hash(&secret),
            scopes: &scopes,
            node_id: None,
            at: s.world.now(),
            expires_at: Some(s.world.now() + chrono::Duration::hours(1)),
        },
        policy.as_ref(),
    )
    .await
    .unwrap();
    Key { id, secret }
}
async fn key(s: &Scene, user: Id, rules: Vec<Json>) -> Key {
    issue(s, user, Some(rules)).await
}
async fn list(s: &Scene, token: &str, query: &McpServerListQuery) -> RawReply {
    s.world
        .app
        .call(McpServerList, &[])
        .token(token)
        .query(query)
        .send_bytes()
        .await
}
async fn get(s: &Scene, token: &str, id: Id) -> RawReply {
    s.world
        .app
        .call(McpServerGet, &[&id])
        .token(token)
        .send_bytes()
        .await
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
fn denied(reply: RawReply) {
    assert_eq!(
        reply.status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert_eq!(
        reply.problem().code,
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
}
fn ids(rows: &Json) -> Vec<Id> {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|row| serde_json::from_value(row["id"].clone()).unwrap())
        .collect()
}
fn row(rows: &Json, id: Id) -> &Json {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == json!(id))
        .expect("selected MCP remains")
}
fn same_content(actual: &Json, ordinary: &Json) {
    for field in [
        "id",
        "name",
        "description",
        "org_id",
        "owner",
        "home",
        "address",
        "machine",
        "environment",
        "imported",
        "spec",
        "enabled",
        "revision",
        "offerings",
        "enabled_places",
    ] {
        assert_eq!(
            actual[field], ordinary[field],
            "full/intrinsic MCP field {field}"
        );
    }
    let held = ordinary["access"]["permissions"].as_array().unwrap();
    for permission in actual["access"]["permissions"].as_array().unwrap() {
        assert!(held.contains(permission));
    }
    let through = ordinary["access"]["through"].as_array().unwrap();
    for source in actual["access"]["through"].as_array().unwrap() {
        assert!(through.contains(source));
    }
    assert!(actual["access"]["administers"] != true || ordinary["access"]["administers"] == true);
}
async fn no_native(s: &Scene) {
    let commands: i64 = sqlx::query_scalar("SELECT count(*) FROM commands")
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
    assert_eq!(commands, 0, "MCP stored reads dispatch no native command");
}

#[tokio::test]
async fn exact_preset_list_reads_keep_full_public_spec_and_equal_authority_legacy_parity() {
    let s = Scene::new().await;
    let id = preset(&s, "first-preset", Some(s.company.acme)).await;
    let other = preset(&s, "second-preset", Some(s.company.acme)).await;
    let env = environment(&s, None, "Enabled preset environment").await;
    rows::enable(&mut *s.world.conn().await, env, id, true, s.world.now())
        .await
        .unwrap();
    let ordinary = shown(list(&s, &s.company.boss.token, &Default::default()).await);
    assert_eq!(row(&ordinary, id)["enabled_places"], 1);
    assert_eq!(row(&ordinary, id)["spec"], spec("first-preset"));
    let legacy = issue(&s, s.company.boss.id(), None).await;
    assert_eq!(
        shown(list(&s, &legacy.secret, &Default::default()).await),
        ordinary
    );
    let token = key(
        &s,
        s.company.boss.id(),
        primary("McpServerList", id, s.company.acme),
    )
    .await;
    let selected = shown(list(&s, &token.secret, &Default::default()).await);
    assert_eq!(ids(&selected), [id]);
    assert!(!ids(&selected).contains(&other));
    same_content(row(&selected, id), row(&ordinary, id));
    let mut write = rule("McpServerList", "mcp_server", id);
    write["allow"][0]["actions"] = json!(["write"]);
    let mut wrong = rule("McpServerList", "mcp_server", id);
    wrong["resources"] = json!(["provider"]);
    for rules in [
        Vec::new(),
        vec![rule("McpServerGet", "mcp_server", id)],
        vec![write],
        vec![wrong],
    ] {
        let token = key(&s, s.company.boss.id(), rules).await;
        denied(list(&s, &token.secret, &Default::default()).await);
    }
    let empty =
        places::make_department(&s.world, &s.company.boss, s.company.acme, "Empty", "empty")
            .await
            .base
            .id;
    let query = McpServerListQuery {
        org_id: Some(empty),
    };
    assert!(ids(&shown(list(&s, &token.secret, &query).await)).is_empty());
    let none = key(&s, s.company.boss.id(), Vec::new()).await;
    denied(list(&s, &none.secret, &query).await);
    let stranger = key(
        &s,
        s.company.stranger.id(),
        primary("McpServerList", id, s.company.acme),
    )
    .await;
    assert!(
        ids(&shown(
            list(&s, &stranger.secret, &Default::default()).await
        ))
        .is_empty()
    );
    no_native(&s).await;
}

#[tokio::test]
async fn exhaustive_mcp_list_keeps_all_five_hundred_and_one_and_reaches_an_exact_late_row() {
    let s = Scene::new().await;
    let mut made = Vec::new();
    for index in 0..501 {
        made.push(preset(&s, &format!("window-{index}"), Some(s.company.acme)).await);
    }
    made.sort_unstable();
    let late = *made.last().unwrap();
    let env = environment(&s, None, "Not a preset page").await;
    let owned = own_server(&s, env, "environment-only").await;
    let ordinary = shown(list(&s, &s.company.boss.token, &Default::default()).await);
    assert_eq!(ids(&ordinary).len(), 501);
    assert!(ids(&ordinary).contains(&late) && !ids(&ordinary).contains(&owned));
    let legacy = issue(&s, s.company.boss.id(), None).await;
    assert_eq!(
        shown(list(&s, &legacy.secret, &Default::default()).await),
        ordinary
    );
    let wide = key(
        &s,
        s.company.boss.id(),
        vec![inside("McpServerList", s.company.acme)],
    )
    .await;
    let all = shown(list(&s, &wide.secret, &Default::default()).await);
    assert_eq!(
        ids(&all),
        ids(&ordinary),
        "500 is an internal window, never a new total cap"
    );
    let token = key(
        &s,
        s.company.boss.id(),
        primary("McpServerList", late, s.company.acme),
    )
    .await;
    let selected = shown(list(&s, &token.secret, &Default::default()).await);
    assert_eq!(ids(&selected), [late]);
    same_content(row(&selected, late), row(&ordinary, late));
}

#[tokio::test]
async fn org_system_and_hidden_mcp_rows_keep_their_existing_policy_without_loading_hidden_sources()
{
    let s = Scene::new().await;
    let department = places::make_department(
        &s.world,
        &s.company.boss,
        s.company.acme,
        "Research",
        "research",
    )
    .await
    .base
    .id;
    let local = preset(&s, "local-preset", Some(department)).await;
    let root = preset(&s, "root-preset", Some(s.company.acme)).await;
    let system = preset(&s, "system-preset", None).await;
    // An Owners entry counts through a real root membership; system pages have none.
    resources::name_person(
        &s.world,
        "mcp_server",
        system,
        "mcp.view",
        s.company.boss.id(),
    )
    .await;
    let all = shown(list(&s, &s.company.boss.token, &Default::default()).await);
    assert!(
        ids(&all).contains(&local),
        "the department preset is ordinarily visible"
    );
    assert!(
        ids(&all).contains(&system),
        "the system preset has explicit native View"
    );
    let machine = personal(&s).await;
    sqlx::query("UPDATE mcp_servers SET machine_id=$2 WHERE id=$1")
        .bind(root)
        .bind(machine)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    resources::name_person(
        &s.world,
        "mcp_server",
        root,
        "access.manage",
        s.company.viewer.id(),
    )
    .await;
    let viewer = shown(list(&s, &s.company.viewer.token, &Default::default()).await);
    let hidden = row(&viewer, root);
    assert_eq!(hidden["content_hidden"], true);
    assert!(hidden.get("spec").is_none() && hidden.get("machine").is_none());
    let query = McpServerListQuery {
        org_id: Some(department),
    };
    let ordinary = shown(list(&s, &s.company.boss.token, &query).await);
    assert!(
        ids(&ordinary).contains(&local),
        "the selected org includes its preset"
    );
    assert!(
        ids(&ordinary).contains(&system),
        "a readable system preset belongs to every org"
    );
    assert!(!ids(&ordinary).contains(&root));
    let legacy = issue(&s, s.company.boss.id(), None).await;
    assert_eq!(shown(list(&s, &legacy.secret, &query).await), ordinary);
    let wide = key(
        &s,
        s.company.boss.id(),
        vec![
            inside("McpServerList", s.company.acme),
            rule("McpServerList", "mcp_server", system),
        ],
    )
    .await;
    let selected = shown(list(&s, &wide.secret, &query).await);
    assert_eq!(ids(&selected), ids(&ordinary));
    let token = key(
        &s,
        s.company.viewer.id(),
        vec![rule("McpServerList", "mcp_server", root)],
    )
    .await;
    assert_eq!(
        row(
            &shown(list(&s, &token.secret, &Default::default()).await),
            root
        ),
        hidden
    );
}

#[tokio::test]
async fn preset_get_uses_its_real_mcp_subject_and_never_returns_sealed_authentication() {
    install_master_key(MasterKey::new([7; 32]));
    let s = Scene::new().await;
    let made = s.world.app.call(McpServerCreate, &[]).token(&s.company.boss.token).body(&McpServerCreateBody {
        name: "remote-secret".into(), description: Some("Public remote definition".into()), home_id: Some(s.company.acme),
        spec: json!({"transport":"http","url":"https://tools.example.test/mcp","authentication":{"kind":"bearer","secret":"MCP_TOKEN"},"secret_env":["MCP_TOKEN"],"tools_allow":["search"]}),
        secrets: Some([("MCP_TOKEN".into(),"never-return-this-private-value".into())].into()), access: Some(NewResourceAccess::OnlyMe),
    }).send().await.ok();
    let ordinary = shown(get(&s, &s.company.boss.token, made.id).await);
    assert_eq!(ordinary["spec"]["authentication"]["secret"], "MCP_TOKEN");
    assert!(
        !ordinary
            .to_string()
            .contains("never-return-this-private-value")
    );
    let legacy = issue(&s, s.company.boss.id(), None).await;
    assert_eq!(shown(get(&s, &legacy.secret, made.id).await), ordinary);
    let token = key(
        &s,
        s.company.boss.id(),
        primary("McpServerGet", made.id, s.company.acme),
    )
    .await;
    same_content(&shown(get(&s, &token.secret, made.id).await), &ordinary);
    for source in [
        rule("McpServerList", "mcp_server", made.id),
        rule("McpServerGet", "environment", made.id),
    ] {
        let token = key(&s, s.company.boss.id(), vec![source]).await;
        denied(get(&s, &token.secret, made.id).await);
    }
    let foreign = get(&s, &s.company.stranger.token, made.id).await;
    assert_ne!(foreign.status, StatusCode::OK);
    let stranger = key(
        &s,
        s.company.stranger.id(),
        primary("McpServerGet", made.id, s.company.acme),
    )
    .await;
    assert_eq!(
        get(&s, &stranger.secret, made.id).await.status,
        foreign.status
    );
    no_native(&s).await;
}

#[tokio::test]
async fn environment_owned_get_uses_the_actual_environment_and_keeps_the_existing_whole_read_door()
{
    let s = Scene::new().await;
    let env = environment(&s, None, "Whole read source").await;
    let id = own_server(&s, env, "owned-tools").await;
    // Environment Use is above View in the same area and cannot prove this exception.
    // Cut inherited Environment View, then grant an independent workspace door instead.
    let mut conn = s.world.conn().await;
    let page = orch_db::queries::access::PageKey::new("environment", env, "live");
    orch_db::queries::access::write_row(
        &mut conn,
        &orch_db::queries::access::RowWrite {
            page: &page,
            profile: "any",
            permission: "environment.view",
            inherit: false,
            locked: false,
        },
        Some(s.company.boss.id()),
        s.world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    resources::name_person(
        &s.world,
        "environment",
        env,
        "files.view",
        s.company.viewer.id(),
    )
    .await;
    let ordinary = shown(get(&s, &s.company.viewer.token, id).await);
    assert_eq!(ordinary["environment"]["id"], json!(env));
    let held = ordinary["access"]["permissions"].as_array().unwrap();
    assert!(
        held.contains(&json!("files.view")) && !held.contains(&json!("environment.view")),
        "the existing Whole door needs no added View gate"
    );
    assert!(
        !ids(&shown(
            list(&s, &s.company.boss.token, &Default::default()).await
        ))
        .contains(&id)
    );
    let legacy = issue(&s, s.company.viewer.id(), None).await;
    assert_eq!(shown(get(&s, &legacy.secret, id).await), ordinary);
    let token = key(
        &s,
        s.company.viewer.id(),
        vec![
            rule("McpServerGet", "environment", env),
            rule("McpServerGet", "org", s.company.acme),
        ],
    )
    .await;
    same_content(&shown(get(&s, &token.secret, id).await), &ordinary);
    let mut relabel = rule("McpServerGet", "environment", env);
    relabel["resources"] = json!(["mcp_server"]);
    for source in [
        rule("McpServerGet", "mcp_server", id),
        rule("McpServerList", "environment", env),
        rule("EnvironmentGet", "environment", env),
        relabel,
    ] {
        let token = key(&s, s.company.viewer.id(), vec![source]).await;
        denied(get(&s, &token.secret, id).await);
    }
    no_native(&s).await;
}

async fn imported_sources(reader: Reader) {
    let s = Scene::new().await;
    let mut machines = Vec::new();
    for _ in 0..4 {
        machines.push(personal(&s).await);
    }
    let id = imported(&s, "many-native-offerings", machines[0]).await;
    for (index, machine) in machines.iter().enumerate().skip(1) {
        rows::offer(
            &mut *s.world.conn().await,
            id,
            *machine,
            None,
            &origin(&s),
            s.world.now(),
        )
        .await
        .unwrap();
        let state = ["ready", "found", "pending", "failed"][index];
        sqlx::query("UPDATE mcp_server_offerings SET state=$3,reason='safe-native-reason' WHERE server_id=$1 AND machine_id=$2")
            .bind(id).bind(machine).bind(state).execute(&s.world.db.pool).await.unwrap();
    }
    let env = environment(&s, Some(machines[0]), "Native discovery").await;
    let mut changed = origin(&s);
    changed["digest"] = json!("changed");
    sqlx::query(
        "UPDATE environments SET facts=jsonb_build_object('mcp_found',$2::jsonb) WHERE id=$1",
    )
    .bind(env)
    .bind(json!([changed]))
    .execute(&s.world.db.pool)
    .await
    .unwrap();
    resources::name_person(
        &s.world,
        "mcp_server",
        id,
        "mcp.view",
        s.company.viewer.id(),
    )
    .await;
    let ordinary = shown(reader.read(&s, &s.company.viewer.token, id).await);
    let expected = reader.item(&ordinary, id);
    assert_eq!(expected["spec"], json!({"transport":"stdio"}));
    assert_eq!(expected["imported"]["source"], "codex");
    assert_eq!(expected["imported"]["stale"], true);
    let offers = expected["offerings"].as_array().unwrap();
    assert_eq!(offers.len(), 4);
    for state in ["ready", "found", "pending", "failed"] {
        assert!(offers.iter().any(|offer| offer["state"] == state));
    }
    assert!(
        offers
            .iter()
            .all(|offer| offer["can_use_definition"] == false)
    );
    assert_eq!(
        offers[0]["definition_fingerprint"],
        json!(Hash::of(b"original MCP definition"))
    );
    let operation = reader.operation();
    for extra in [
        Vec::new(),
        machines
            .iter()
            .map(|id| rule("MachineGet", "machine", *id))
            .collect(),
    ] {
        let mut rules = primary(operation, id, s.company.acme);
        rules.extend(extra);
        let token = key(&s, s.company.viewer.id(), rules).await;
        denied(reader.read(&s, &token.secret, id).await);
    }
    let mut rules = primary(operation, id, s.company.acme);
    rules.extend(machines.iter().map(|id| rule(operation, "machine", *id)));
    let token = key(&s, s.company.viewer.id(), rules).await;
    let actual = shown(reader.read(&s, &token.secret, id).await);
    same_content(reader.item(&actual, id), expected);
    // Native loss revokes the credential; the imported resource survives its suppliers.
    sqlx::query("UPDATE machines SET lost_at=$2,status='revoked',online=false WHERE id=ANY($1)")
        .bind(&machines)
        .bind(s.world.now())
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    // The native unavailable trigger reselects only a ready active supplier. None remain,
    // so it clears the MCP pointer even though the physical machine rows still exist.
    let stored_machine: Option<Id> =
        sqlx::query_scalar("SELECT machine_id FROM mcp_servers WHERE id=$1")
            .bind(id)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
    assert!(
        stored_machine.is_none(),
        "the native unavailable trigger clears the last supplier"
    );
    let physical_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM machines WHERE id=ANY($1) AND status='revoked' AND lost_at IS NOT NULL AND deleted_at IS NULL")
        .bind(&machines).fetch_one(&s.world.db.pool).await.unwrap();
    assert_eq!(
        physical_rows,
        i64::try_from(machines.len()).unwrap(),
        "loss retains the physical rows without retaining a live MCP supplier"
    );
    let lost = shown(reader.read(&s, &s.company.viewer.token, id).await);
    let retained = reader.item(&lost, id);
    assert_eq!(retained["id"], json!(id));
    assert!(retained["offerings"].as_array().is_none_or(Vec::is_empty));
    assert_eq!(retained["spec"], json!({"transport":"stdio"}));
    assert!(
        retained["machine"].is_null(),
        "the ordinary DTO follows the stored native supplier pointer"
    );
    sqlx::query("UPDATE machines SET deleted_at=$2 WHERE id=ANY($1)")
        .bind(&machines)
        .bind(s.world.now())
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    let plain = key(
        &s,
        s.company.viewer.id(),
        primary(operation, id, s.company.acme),
    )
    .await;
    let absent = shown(reader.read(&s, &plain.secret, id).await);
    assert!(reader.item(&absent, id)["machine"].is_null());
    assert!(
        reader.item(&absent, id)["offerings"]
            .as_array()
            .is_none_or(Vec::is_empty)
    );
    no_native(&s).await;
}
#[tokio::test]
async fn imported_list_keeps_all_safe_offering_fields_and_requires_actual_machine_sources() {
    imported_sources(Reader::List).await;
}
#[tokio::test]
async fn imported_get_keeps_all_safe_offering_fields_and_requires_actual_machine_sources() {
    imported_sources(Reader::Get).await;
}

#[tokio::test]
async fn imported_get_uses_canonical_host_machine_sources_without_an_added_account_machine_gate() {
    let s = Scene::new().await;
    let (machine, host) = hosted(&s).await;
    let id = imported(&s, "hosted-native-tools", machine).await;
    resources::name_person(
        &s.world,
        "mcp_server",
        id,
        "mcp.view",
        s.company.viewer.id(),
    )
    .await;
    let ordinary = shown(get(&s, &s.company.viewer.token, id).await);
    assert_eq!(ordinary["machine"]["id"], json!(machine));
    let account_machine = s
        .world
        .app
        .call(orch_api_contract::resources::MachineGet, &[&machine])
        .token(&s.company.viewer.token)
        .send_bytes()
        .await;
    assert_eq!(account_machine.status, StatusCode::NOT_FOUND);
    let mut canonical = rule("McpServerGet", "host", host);
    canonical["resources"] = json!(["machine"]);
    let mut wrong = canonical.clone();
    wrong["resources"] = json!(["host"]);
    for source in [
        rule("McpServerGet", "machine", machine),
        rule("MachineGet", "host", host),
        wrong,
    ] {
        let mut rules = primary("McpServerGet", id, s.company.acme);
        rules.push(source);
        let token = key(&s, s.company.viewer.id(), rules).await;
        denied(get(&s, &token.secret, id).await);
    }
    let mut rules = primary("McpServerGet", id, s.company.acme);
    rules.push(canonical);
    let token = key(&s, s.company.viewer.id(), rules).await;
    same_content(&shown(get(&s, &token.secret, id).await), &ordinary);
    no_native(&s).await;
}

fn holder<'a>(shown: &'a Json, kind: &str) -> &'a Json {
    shown["access"]["through"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|through| {
            (through["kind"] == "entry" && through["holder"]["kind"] == kind)
                .then_some(&through["holder"])
        })
        .expect("the real holder remains")
}
async fn named_sources(reader: Reader) {
    let s = Scene::new().await;
    let id = preset(&s, "named-source-tools", Some(s.company.acme)).await;
    let project = places::make_project(
        &s.world,
        &s.company.boss,
        s.company.acme,
        "Protected project",
    )
    .await;
    let topic = places::make_topic(
        &s.world,
        &s.company.boss,
        project.base.id,
        "Protected topic",
    )
    .await;
    resources::name_person(
        &s.world,
        "mcp_server",
        id,
        "mcp.view",
        s.company.viewer.id(),
    )
    .await;
    resources::name_on_page(
        &s.world,
        "mcp_server",
        id,
        "mcp.view",
        ("role", Some(topic.base.id), Some("viewer")),
    )
    .await;
    let ordinary = shown(reader.read(&s, &s.company.viewer.token, id).await);
    assert!(
        holder(reader.item(&ordinary, id), "role")["at"]["name"]
            .as_str()
            .unwrap()
            .contains("Protected topic")
    );
    let nodes = [
        ("org", s.company.acme),
        ("project", project.base.id),
        ("topic", topic.base.id),
    ];
    let operation = reader.operation();
    let other = if matches!(reader, Reader::List) {
        "McpServerGet"
    } else {
        "McpServerList"
    };
    let mut wrong = vec![rule(operation, "mcp_server", id)];
    wrong.extend(nodes.iter().map(|(kind, id)| rule(other, kind, *id)));
    let token = key(&s, s.company.viewer.id(), wrong).await;
    let limited = shown(reader.read(&s, &token.secret, id).await);
    let hidden = reader.item(&limited, id);
    assert!(hidden["home"]["name"].is_null() && hidden["home"]["redacted"] == true);
    assert!(holder(hidden, "role")["at"]["name"].is_null());
    assert_eq!(hidden["spec"], reader.item(&ordinary, id)["spec"]);
    assert_eq!(
        hidden["access"]["permissions"],
        reader.item(&ordinary, id)["access"]["permissions"]
    );
    assert_eq!(
        hidden["access"]["through"].as_array().unwrap().len(),
        reader.item(&ordinary, id)["access"]["through"]
            .as_array()
            .unwrap()
            .len()
    );
    let group = s
        .world
        .app
        .call(OrgGroupCreate, &[&s.company.acme])
        .token(&s.company.boss.token)
        .body(&OrgGroupCreateBody {
            name: "MCP readers".into(),
            description: Some("Actual full MCP source group".into()),
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("this ordinary control requires the full native creation response");
    for who in [&s.company.viewer, &s.company.admin] {
        s.world
            .app
            .call(GroupMemberAdd, &[&group.id, &who.id()])
            .token(&s.company.boss.token)
            .send()
            .await
            .ok();
    }
    resources::name_on_page(
        &s.world,
        "mcp_server",
        id,
        "mcp.view",
        ("group", Some(group.id), None),
    )
    .await;
    let ordinary = shown(reader.read(&s, &s.company.viewer.token, id).await);
    let expected = reader.item(&ordinary, id);
    assert_eq!(
        holder(expected, "group")["group"]["description"],
        "Actual full MCP source group"
    );
    assert_eq!(holder(expected, "group")["group"]["member_count"], 2);
    let mut full = vec![rule(operation, "mcp_server", id)];
    full.extend(nodes.iter().map(|(kind, id)| rule(operation, kind, *id)));
    for extra in [Vec::new(), vec![rule(other, "group", group.id)]] {
        let mut rules = full.clone();
        rules.extend(extra);
        let token = key(&s, s.company.viewer.id(), rules).await;
        denied(reader.read(&s, &token.secret, id).await);
    }
    full.push(rule(operation, "group", group.id));
    let token = key(&s, s.company.viewer.id(), full).await;
    same_content(
        reader.item(&shown(reader.read(&s, &token.secret, id).await), id),
        expected,
    );
}
#[tokio::test]
async fn mcp_list_keeps_optional_names_separate_from_required_full_group_sources() {
    named_sources(Reader::List).await;
}
#[tokio::test]
async fn mcp_get_keeps_optional_names_separate_from_required_full_group_sources() {
    named_sources(Reader::Get).await;
}

#[tokio::test]
async fn mcp_get_restores_current_key_bounds_and_actual_home_without_returning_old_spec() {
    let s = Scene::new().await;
    let id = preset(&s, "current-spec-marker", Some(s.company.acme)).await;
    for change in ["narrow", "revoke", "expiry"] {
        let token = key(
            &s,
            s.company.boss.id(),
            vec![inside("McpServerGet", s.company.acme)],
        )
        .await;
        assert_eq!(
            shown(get(&s, &token.secret, id).await)["spec"],
            spec("current-spec-marker")
        );
        match change {
            "narrow" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(token.id)
                    .bind(json!(KeyScope::default()))
                    .execute(&s.world.db.pool)
                    .await
                    .unwrap();
            }
            "revoke" => {
                assert!(
                    identity::revoke_api_token(
                        &mut *s.world.conn().await,
                        token.id,
                        s.company.boss.id(),
                        s.world.now()
                    )
                    .await
                    .unwrap()
                );
            }
            _ => {
                sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                    .bind(token.id)
                    .bind(s.world.now() - chrono::Duration::seconds(1))
                    .execute(&s.world.db.pool)
                    .await
                    .unwrap();
            }
        }
        let after = get(&s, &token.secret, id).await;
        assert!(!String::from_utf8_lossy(&after.raw).contains("current-spec-marker"));
        assert_eq!(
            after.status,
            if change == "narrow" {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::UNAUTHORIZED
            }
        );
    }
    let outside = places::make_org(&s.world, &s.company.boss, "Other root", "other-root")
        .await
        .base
        .id;
    let token = key(
        &s,
        s.company.boss.id(),
        vec![inside("McpServerGet", s.company.acme)],
    )
    .await;
    sqlx::query("UPDATE mcp_servers SET home_id=$2,org_id=$2 WHERE id=$1")
        .bind(id)
        .bind(outside)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        shown(get(&s, &s.company.boss.token, id).await)["spec"],
        spec("current-spec-marker")
    );
    denied(get(&s, &token.secret, id).await);
}

#[tokio::test]
async fn populated_imported_mcp_rows_and_distinct_machine_sources_use_bulk_data_reads() {
    let s = Scene::new().await;
    async fn add(s: &Scene, index: usize) {
        let (machine, _) = hosted(s).await;
        let id = imported(s, &format!("batch-tools-{index}"), machine).await;
        resources::name_person(
            &s.world,
            "mcp_server",
            id,
            "mcp.view",
            s.company.viewer.id(),
        )
        .await;
    }
    for index in 0..2 {
        add(&s, index).await;
    }
    let token = key(
        &s,
        s.company.viewer.id(),
        vec![inside("McpServerList", s.company.acme)],
    )
    .await;
    async fn measure(s: &Scene, secret: &str, expected: usize) -> usize {
        let effects_before: (i64, i64) =
            sqlx::query_as("SELECT (SELECT count(*) FROM commands),(SELECT count(*) FROM jobs)")
                .fetch_one(&s.world.db.pool)
                .await
                .unwrap();
        let (reply, count) = Statements::measure(list(s, secret, &Default::default())).await;
        let effects_after: (i64, i64) =
            sqlx::query_as("SELECT (SELECT count(*) FROM commands),(SELECT count(*) FROM jobs)")
                .fetch_one(&s.world.db.pool)
                .await
                .unwrap();
        assert_eq!(
            effects_after, effects_before,
            "stored reads create neither native commands nor jobs"
        );
        let value = shown(reply);
        assert_eq!(ids(&value).len(), expected);
        for item in value.as_array().unwrap() {
            assert_eq!(item["offerings"].as_array().unwrap().len(), 1);
            assert!(!item["machine"].is_null());
            assert_eq!(item["spec"], json!({"transport":"stdio"}));
            assert_eq!(
                item["offerings"][0]["can_use_definition"], false,
                "ordinary viewer has no MCP management; native capability checks are not silently replaced"
            );
        }
        assert!(count > 0, "observer must see actual populated reads");
        count
    }
    let few = measure(&s, &token.secret, 2).await;
    for index in 2..12 {
        add(&s, index).await;
    }
    let many = measure(&s, &token.secret, 12).await;
    assert_eq!(
        few, many,
        "MCP rows/offerings/machine data are batched; this does not claim constant unique native-import authority checks"
    );
    no_native(&s).await;
}
