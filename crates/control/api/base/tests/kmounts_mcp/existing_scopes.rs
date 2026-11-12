//! Real stored-resource copy admission, native completion and optional current Read.
use super::*;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::{
    testing::{RawReply, mirror_native_result},
    thread_access::AuthzAccess,
};
use orch_db::queries::{identity, link, resources::mcp as rows};
use orch_engine::{commands::CommandAuthority, environment_mcp::ForgetCustody};
use orch_types::{Hash, new_id};
use tokio::task::JoinHandle;
const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
struct Key {
    id: Id,
    secret: String,
}
#[derive(Clone, Copy)]
enum Mode {
    Reimport,
    Offering,
}
impl Mode {
    fn operation(self) -> &'static str {
        match self {
            Self::Reimport => "McpServerReimport",
            Self::Offering => "McpOfferingUse",
        }
    }
}
struct Scene {
    world: World,
    c: Company,
    env: Id,
    machine: Id,
    server: Id,
    home: Id,
    fp: Hash,
    revision: i64,
}
fn rule(op: &str, kind: &str, id: Id, families: &[&str], actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":families,"allow":[{"operation":op,"actions":actions}]})
}
async fn key(world: &World, user: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = orch_api_core::middleware::authentication::new_token(
        orch_api_core::middleware::authentication::API_TOKEN_PREFIX,
    );
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Stored MCP native copy",
            token_hash: &orch_api_core::middleware::authentication::token_hash(&secret),
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
async fn found(world: &World, env: Id, digest: &str, fp: Hash) {
    sqlx::query("UPDATE environments SET facts=jsonb_set(facts,'{mcp_found}',$2::jsonb) WHERE id=$1").bind(env).bind(json!([{"source":"codex","scope":"user","name":"search","directory":null,"transport":"stdio","digest":digest,"definition_fingerprint":fp,"tools":2}])).execute(&world.db.pool).await.unwrap();
}
impl Scene {
    async fn new(mode: Mode) -> Self {
        let world = World::new().await;
        let c = company(&world).await;
        let (env, machine) = environment(&world, &c, true).await;
        let old = Hash::of(b"original native definition");
        // Reimport takes a value-only refresh of the same structural definition.
        // A structural change belongs to the separately reviewed offering-use path.
        let fp = match mode {
            Mode::Reimport => old,
            Mode::Offering => Hash::of(b"reviewed native definition"),
        };
        found(&world, env, "first", old).await;
        let answered = answer(
            &world,
            machine,
            json!({"transport":"stdio","digest":"first","definition_fingerprint":old,"tools":2}),
        );
        let made = world
            .app
            .call(EnvironmentMcpImportCreate, &[&env])
            .token(&c.admin.token)
            .body(&EnvironmentMcpImportCreateBody {
                source: "codex".into(),
                scope: McpScope::User,
                name: "search".into(),
                directory: None,
                as_name: Some("stored-import".into()),
                enable: Some(false),
            })
            .send()
            .await
            .ok()
            .into_full()
            .unwrap();
        answered.await.unwrap();
        let home = made
            .home
            .as_ref()
            .expect("the native import has its actual home")
            .id;
        found(&world, env, "second", fp).await;
        if matches!(mode, Mode::Offering) {
            let source = json!({"source":"codex","scope":"user","name":"search","directory":null,"transport":"stdio","digest":"second","definition_fingerprint":fp,"tools":2});
            sqlx::query("UPDATE mcp_server_offerings SET state='found',reason='different_definition',proposal=$3 WHERE server_id=$1 AND machine_id=$2").bind(made.id).bind(machine).bind(json!({"reason":"different_definition","environment":env,"source":source})).execute(&world.db.pool).await.unwrap();
        }
        let revision = rows::servers(&mut *world.conn().await, &[made.id])
            .await
            .unwrap()
            .pop()
            .unwrap()
            .revision;
        Self {
            world,
            c,
            env,
            machine,
            server: made.id,
            home,
            fp,
            revision,
        }
    }
    fn rules(&self, mode: Mode, read: bool) -> Vec<Json> {
        let op = mode.operation();
        let mut rules = vec![
            rule(op, "mcp_server", self.server, &["mcp_server"], &["manage"]),
            rule(op, "environment", self.env, &["environment"], &["manage"]),
            rule(op, "machine", self.machine, &["machine"], &["manage"]),
        ];
        if read {
            rules.push(rule(
                op,
                "mcp_server",
                self.server,
                &["mcp_server"],
                &["read"],
            ));
            rules.push(json!({"target":{"kind":"org","id":self.c.acme,"reach":"subtree"},"resources":["mcp_server","environment","org","project","topic","thread","group"],"allow":[{"operation":op,"actions":["read"]}]}));
            rules.push(rule(op, "machine", self.machine, &["machine"], &["read"]));
            rules.push(rule(op, "org", self.home, &["org"], &["read"]));
        }
        rules
    }
    async fn actor(&self, mode: Mode, read: bool) -> Key {
        key(&self.world, self.c.admin.id(), self.rules(mode, read)).await
    }
    fn start(&self, mode: Mode, secret: &str) -> JoinHandle<RawReply> {
        let app = self.world.app.clone();
        let secret = secret.to_owned();
        let server = self.server;
        let machine = self.machine;
        let body = McpOfferingUseBody {
            expected_revision: self.revision,
            expected_fingerprint: Some(self.fp),
        };
        tokio::spawn(async move {
            match mode {
                Mode::Reimport => {
                    let request = app.call(McpServerReimport, &[&server]).token(&secret);
                    request.send_bytes().await
                }
                Mode::Offering => {
                    let request = app
                        .call(McpOfferingUse, &[&server, &machine])
                        .token(&secret)
                        .body(&body);
                    request.send_bytes().await
                }
            }
        })
    }
    async fn queued(&self, pending: &JoinHandle<RawReply>) -> link::CommandWithAuthority {
        let id=tokio::time::timeout(WAIT,async{loop{
            assert!(!pending.is_finished(),"the actual stored-server copy must reach its real native queue");
            let id:Option<Id>=sqlx::query_scalar("SELECT id FROM commands WHERE kind='mcp.import' AND body->>'server_id'=$1 AND state IN ('queued','sent','accepted') ORDER BY created_at,id LIMIT 1").bind(self.server.to_string()).fetch_optional(&self.world.db.pool).await.unwrap();if let Some(id)=id{break id;}tokio::task::yield_now().await;
        }}).await.expect("the actual existing-definition command");
        link::command_with_authority(&mut *self.world.conn().await, id)
            .await
            .unwrap()
            .unwrap()
    }
    async fn accept(&self, pending: &JoinHandle<RawReply>) -> link::CommandWithAuthority {
        let stored = self.queued(pending).await;
        let authority: CommandAuthority =
            serde_json::from_value(stored.authority.clone().unwrap()).unwrap();
        let mut tx = self.world.db.pool.begin().await.unwrap();
        assert!(
            authority
                .current_callers(
                    &mut tx,
                    &stored.command,
                    self.world.app.app.clock.as_ref(),
                    &AuthzAccess::new(self.world.app.app.clock.clone())
                )
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            link::mark_accepted(&mut tx, stored.command.id, self.world.now())
                .await
                .unwrap()
        );
        tx.commit().await.unwrap();
        link::command_with_authority(&mut *self.world.conn().await, stored.command.id)
            .await
            .unwrap()
            .unwrap()
    }
    async fn complete(&self, id: Id) {
        mirror_native_result(&self.world.app.app,id,json!({"transport":"stdio","digest":"second","definition_fingerprint":self.fp,"tools":3})).await.unwrap();
    }
    async fn done(&self, id: Id, pending: JoinHandle<RawReply>) -> RawReply {
        self.complete(id).await;
        tokio::time::timeout(WAIT, pending).await.unwrap().unwrap()
    }
    async fn row(&self) -> rows::McpRow {
        rows::servers(&mut *self.world.conn().await, &[self.server])
            .await
            .unwrap()
            .pop()
            .unwrap()
    }
    async fn close(&self, actor: &Key) {
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
            .bind(actor.id)
            .bind(json!({"rules":[]}))
            .execute(&self.world.db.pool)
            .await
            .unwrap();
    }
    async fn audits(&self, id: Id) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE action='mcp.imported' AND details->>'command'=$1",
        )
        .bind(id.to_string())
        .fetch_one(&self.world.db.pool)
        .await
        .unwrap()
    }
}
fn receipt(reply: &RawReply, server: Id) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let value: Json = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["server_id"], json!(server));
    assert!(
        value.get("spec").is_none()
            && value.get("offerings").is_none()
            && value.get("imported").is_none()
    );
    value
}

#[tokio::test]
async fn stored_native_imports_bind_the_real_server_environment_and_physical_copy_before_dispatch()
{
    for mode in [Mode::Reimport, Mode::Offering] {
        let s = Scene::new(mode).await;
        for omitted in 0..3 {
            let mut rules = s.rules(mode, false);
            rules.remove(omitted);
            let actor = key(&s.world, s.c.admin.id(), rules).await;
            let reply = tokio::time::timeout(WAIT, s.start(mode, &actor.secret))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(reply.status, StatusCode::FORBIDDEN);
        }
        let actor = s.actor(mode, false).await;
        let pending = s.start(mode, &actor.secret);
        let stored = s.accept(&pending).await;
        assert_eq!(stored.command.machine_id, s.machine);
        assert_eq!(stored.command.body["server_id"], json!(s.server));
        assert_eq!(stored.command.body["environment_id"], json!(s.env));
        let authority = stored.authority.as_ref().unwrap();
        assert_eq!(authority["kind"], "http_origin");
        let native = &authority["authority"]["native"];
        assert_eq!(native["kind"], "channel");
        assert_eq!(native["evidence"]["kind"], "mcp_import");
        assert_eq!(
            native["evidence"]["authority"]["origin"]["operation"],
            mode.operation()
        );
        assert!(!stored.command.body.to_string().contains("authority"));
        let value = receipt(&s.done(stored.command.id, pending).await, s.server);
        assert_eq!(value["selected"], true);
        assert_eq!(value["cleanup_queued"], false);
        let row = s.row().await;
        assert_eq!(row.spec["imported"]["digest"], "second");
        assert_eq!(row.revision, s.revision + 1);
        assert_eq!(s.audits(stored.command.id).await, 1);
    }
}

#[tokio::test]
async fn stored_native_imports_keep_ordinary_full_projection_and_require_their_own_current_read() {
    for mode in [Mode::Reimport, Mode::Offering] {
        let s = Scene::new(mode).await;
        let actor = s.actor(mode, true).await;
        let pending = s.start(mode, &actor.secret);
        let stored = s.accept(&pending).await;
        let reply = s.done(stored.command.id, pending).await;
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        let shown: McpServer = serde_json::from_slice(&reply.raw).unwrap();
        let ordinary = s
            .world
            .app
            .call(McpServerGet, &[&s.server])
            .token(&s.c.admin.token)
            .send()
            .await
            .ok();
        assert_eq!(shown, ordinary);
    }
}

#[tokio::test]
async fn existing_import_content_redacts_a_home_outside_its_own_current_read() {
    for mode in [Mode::Reimport, Mode::Offering] {
        let s = Scene::new(mode).await;
        assert_ne!(
            s.home, s.c.acme,
            "the native import keeps its creator's home"
        );
        let mut rules = s.rules(mode, true);
        rules.retain(|rule| rule["target"]["id"] != json!(s.home));
        let actor = key(&s.world, s.c.admin.id(), rules).await;
        let pending = s.start(mode, &actor.secret);
        let stored = s.accept(&pending).await;
        let reply = s.done(stored.command.id, pending).await;
        assert_eq!(reply.status, StatusCode::OK);
        let shown: McpServer = serde_json::from_slice(&reply.raw).unwrap();
        let mut ordinary = s
            .world
            .app
            .call(McpServerGet, &[&s.server])
            .token(&s.c.admin.token)
            .send()
            .await
            .ok();
        let home = ordinary.home.as_mut().unwrap();
        assert_eq!(home.id, s.home);
        assert!(home.name.is_some());
        assert!(!home.redacted);
        home.name = None;
        home.redacted = true;
        assert_eq!(shown, ordinary);
    }
}

#[tokio::test]
async fn accepted_existing_copy_records_only_its_exact_outcome_after_origin_closes_and_never_replays()
 {
    for mode in [Mode::Reimport, Mode::Offering] {
        for expired in [false, true] {
            let s = Scene::new(mode).await;
            let actor = s.actor(mode, true).await;
            let pending = s.start(mode, &actor.secret);
            let stored = s.accept(&pending).await;
            if expired {
                s.world.advance(chrono::Duration::hours(2));
            } else {
                s.close(&actor).await;
            }
            let reply = s.done(stored.command.id, pending).await;
            let value = receipt(&reply, s.server);
            assert_eq!(value["selected"], true);
            let original = s.row().await;
            assert_eq!(original.spec["imported"]["digest"], "second");
            assert_eq!(original.revision, s.revision + 1);
            s.complete(stored.command.id).await;
            assert_eq!(s.row().await.revision, original.revision);
            assert_eq!(s.audits(stored.command.id).await, 1);
            let retry = tokio::time::timeout(WAIT, s.start(mode, &actor.secret))
                .await
                .unwrap()
                .unwrap();
            assert!(
                !retry.status.is_success(),
                "completed native facts cannot authorize another copy"
            );
        }
    }
}

#[tokio::test]
async fn native_result_mirroring_finishes_the_real_copy_after_its_http_waiter_is_gone() {
    let s = Scene::new(Mode::Reimport).await;
    let actor = s.actor(Mode::Reimport, true).await;
    let pending = s.start(Mode::Reimport, &actor.secret);
    let stored = s.accept(&pending).await;
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    s.close(&actor).await;
    s.complete(stored.command.id).await;
    let row = s.row().await;
    assert_eq!(row.spec["imported"]["digest"], "second");
    assert_eq!(row.revision, s.revision + 1);
    assert_eq!(s.audits(stored.command.id).await, 1);
    let outcome = link::mcp_import_completion(&mut *s.world.conn().await, stored.command.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome["server_id"], json!(s.server));
    assert_eq!(outcome["selected"], true);
}

#[tokio::test]
async fn existing_import_source_changes_refuse_unaccepted_delivery_without_lending_creator_custody()
{
    let s = Scene::new(Mode::Reimport).await;
    let actor = s.actor(Mode::Reimport, false).await;
    let pending = s.start(Mode::Reimport, &actor.secret);
    let stored = s.queued(&pending).await;
    let authority: CommandAuthority =
        serde_json::from_value(stored.authority.clone().unwrap()).unwrap();
    let mut tx = s.world.db.pool.begin().await.unwrap();
    assert!(
        authority
            .current_callers(
                &mut tx,
                &stored.command,
                s.world.app.app.clock.as_ref(),
                &AuthzAccess::new(s.world.app.app.clock.clone())
            )
            .await
            .unwrap()
            .is_some()
    );
    tx.rollback().await.unwrap();
    found(
        &s.world,
        s.env,
        "a replacement source",
        Hash::of(b"another definition"),
    )
    .await;
    let mut tx = s.world.db.pool.begin().await.unwrap();
    assert!(
        authority
            .current_callers(
                &mut tx,
                &stored.command,
                s.world.app.app.clock.as_ref(),
                &AuthzAccess::new(s.world.app.app.clock.clone())
            )
            .await
            .unwrap()
            .is_none()
    );
    tx.rollback().await.unwrap();
    link::finish_command(
        &mut *s.world.conn().await,
        stored.command.id,
        &link::Ending::Cancelled,
        s.world.now(),
    )
    .await
    .unwrap();
    let reply = tokio::time::timeout(WAIT, pending).await.unwrap().unwrap();
    assert!(!reply.status.is_success());
    assert_eq!(s.row().await.spec["imported"]["digest"], "first");
    assert!(
        link::mcp_import_completion(&mut *s.world.conn().await, stored.command.id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn an_existing_copy_whose_target_ended_owns_only_its_exact_reducing_inverse() {
    let s = Scene::new(Mode::Reimport).await;
    let actor = s.actor(Mode::Reimport, false).await;
    let pending = s.start(Mode::Reimport, &actor.secret);
    let stored = s.accept(&pending).await;
    sqlx::query("UPDATE mcp_servers SET deleted_at=$2,revision=revision+1 WHERE id=$1")
        .bind(s.server)
        .bind(s.world.now())
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    s.world.advance(chrono::Duration::hours(2));
    let value = receipt(&s.done(stored.command.id, pending).await, s.server);
    assert_eq!(value["selected"], false);
    assert_eq!(value["cleanup_queued"], true);
    assert!(value["revision"].is_null());
    let id = orch_engine::ids::derived(
        "mcp.existing-import.inverse",
        &[stored.command.id.as_bytes()],
    );
    let cleanup = link::command_with_authority(&mut *s.world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    let authority: CommandAuthority = serde_json::from_value(cleanup.authority.unwrap()).unwrap();
    let CommandAuthority::McpForget(ref proof) = authority else {
        panic!("one native Forget authority owns the reducing inverse")
    };
    assert!(matches!(
        proof.custody(),
        ForgetCustody::AbandonedExistingCopy { .. }
    ));
    let mut tx = s.world.db.pool.begin().await.unwrap();
    let current = authority
        .current_callers(
            &mut tx,
            &cleanup.command,
            s.world.app.app.clock.as_ref(),
            &AuthzAccess::new(s.world.app.app.clock.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.len(), 1);
    assert!(
        current[0].0.is_none(),
        "an ended invoker is not renewed for cleanup"
    );
    tx.rollback().await.unwrap();
    sqlx::query("UPDATE mcp_servers SET deleted_at=NULL WHERE id=$1")
        .bind(s.server)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    let mut tx = s.world.db.pool.begin().await.unwrap();
    assert!(
        authority
            .current_callers(
                &mut tx,
                &cleanup.command,
                s.world.app.app.clock.as_ref(),
                &AuthzAccess::new(s.world.app.app.clock.clone())
            )
            .await
            .unwrap()
            .is_none()
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn current_existing_import_scope_is_checked_after_the_actual_server_row_wait_before_any_copy()
{
    for expired in [false, true] {
        let s = Scene::new(Mode::Reimport).await;
        let actor = s.actor(Mode::Reimport, false).await;
        let baseline: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM commands WHERE kind='mcp.import' AND body->>'server_id'=$1",
        )
        .bind(s.server.to_string())
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
        let mut blocker = s.world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM mcp_servers WHERE id=$1 FOR UPDATE")
            .bind(s.server)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pending = s.start(Mode::Reimport, &actor.secret);
        tokio::time::timeout(WAIT,async{loop{assert!(!pending.is_finished());let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%SELECT id FROM mcp_servers%FOR SHARE%')").bind(pid).fetch_one(&s.world.db.pool).await.unwrap();if waiting{break;}tokio::task::yield_now().await;}}).await.expect("the actual native server source lock is waiting");
        if expired {
            s.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        if expired {
            let reply = tokio::time::timeout(WAIT, pending).await.unwrap().unwrap();
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM commands WHERE kind='mcp.import' AND body->>'server_id'=$1",
            )
            .bind(s.server.to_string())
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
            assert_eq!(count, baseline);
        } else {
            let stored = s.accept(&pending).await;
            receipt(&s.done(stored.command.id, pending).await, s.server);
        }
    }
}

#[tokio::test]
async fn a_competing_definition_copy_sees_the_real_pending_native_reservation() {
    let s = Scene::new(Mode::Reimport).await;
    let actor = s.actor(Mode::Reimport, false).await;
    let pending = s.start(Mode::Reimport, &actor.secret);
    let stored = s.accept(&pending).await;
    let mut tx = s.world.db.pool.begin().await.unwrap();
    assert!(matches!(
        rows::lock_definition(&mut tx, s.server).await,
        Err(orch_db::DbError::Retry)
    ));
    tx.rollback().await.unwrap();
    receipt(&s.done(stored.command.id, pending).await, s.server);
    let mut tx = s.world.db.pool.begin().await.unwrap();
    rows::lock_definition(&mut tx, s.server).await.unwrap();
    tx.rollback().await.unwrap();
}
