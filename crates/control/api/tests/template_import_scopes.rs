//! Actual template proposals keep one Host and original archive custody through native IO.
#![recursion_limit = "256"]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod places;
mod resources;
mod support;
mod workspace;

use bytes::Bytes;
use http::{Method, StatusCode};
use orch_access_contract::{decisions::ObjectRef, key_scope::KeyScope, permissions::ResourceKind};
use orch_api_contract::resources::{TemplateImport, TemplateImportBody, UnitKind};
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::{RawReply, TestApp},
};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_blob_contract::{
    range::ByteRange,
    store::{BlobError, BlobRead, BlobStore, ByteStream, Stored},
};
use orch_db::queries::{identity, link, resources::access_fixtures as fixtures, storage, uploads};
use orch_engine::commands::CommandAuthority;
use orch_link_contract::commands::CommandKind;
use orch_types::{Hash, Id, Json, Time, new_id};
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use support::World;
use tokio::sync::Notify;
use workspace::Content;

const OP: &str = "TemplateImport";
const ARCHIVE: &[u8] = b"the original native proposal archive";
// The same bounded native rendezvous watchdog as the neighboring content controls.
const WAIT: Duration = Duration::from_secs(30);
struct Key {
    id: Id,
    secret: String,
}
struct Scene {
    world: World,
    company: resources::Company,
    host: Id,
    machine: Id,
    archive: Hash,
    label: Hash,
}
fn rule(operation: &str, kind: &str, id: Id, family: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[family],"allow":[{"operation":operation,"actions":actions}]})
}
async fn key(world: &World, user: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Native template proposal",
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
impl Scene {
    async fn new() -> Self {
        let world = World::new().await;
        let company = resources::company(&world).await;
        let host = fixtures::insert_resource(
            &mut *world.conn().await,
            "host",
            Some(company.boss.id()),
            Some(company.acme),
            "Proposal host",
        )
        .await
        .unwrap();
        start_resource(
            &mut *world.conn().await,
            ObjectRef {
                kind: ResourceKind::Host.into(),
                id: host,
            },
            company.boss.id(),
            NewAccess::OnlyMe,
            Time::new(world.now()),
        )
        .await
        .unwrap();
        let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
            .bind(host)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        let archive = world
            .app
            .app
            .blobs
            .put_bytes(Bytes::from_static(ARCHIVE))
            .await
            .unwrap();
        storage::register_blob(
            &mut *world.conn().await,
            &archive.hash,
            archive.size,
            world.now(),
        )
        .await
        .unwrap();
        uploads::remember(
            &mut *world.conn().await,
            company.boss.id(),
            &archive.hash,
            world.now(),
        )
        .await
        .unwrap();
        let label = world
            .app
            .app
            .blobs
            .put_bytes(Bytes::from_static(b"a hash-like label is not an archive"))
            .await
            .unwrap();
        storage::register_blob(
            &mut *world.conn().await,
            &label.hash,
            label.size,
            world.now(),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE machines SET online=true WHERE id=$1")
            .bind(machine)
            .execute(&world.db.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE hosts SET drivers=$2 WHERE id=$1").bind(host).bind(json!({"memory":{
            "name":"memory","title":"Proposal fixture","version":"1.0.0","spec_schemas":{"environment":{},"provider":{}},
            "imports":[{"format":"archive","title":"Stored archive","fields":[
                {"name":"blob","label":"Archive","type":"text","required":true,"default":archive.hash},
                {"name":"note","label":"Label","type":"text","required":false,"default":label.hash}
            ]},{"format":"plain","title":"Inline recipe","fields":[]}]
        }})).execute(&world.db.pool).await.unwrap();
        Self {
            world,
            company,
            host,
            machine,
            archive: archive.hash,
            label: label.hash,
        }
    }
    fn rules(&self) -> Vec<Json> {
        vec![
            rule(OP, "org", self.company.acme, "template", &["manage"]),
            rule(OP, "host", self.host, "host", &["read"]),
            rule(OP, "account", self.company.boss.id(), "upload", &["read"]),
        ]
    }
    fn body(&self) -> TemplateImportBody {
        TemplateImportBody {
            home_id: Some(self.company.acme),
            driver: "memory".into(),
            kind: Some(UnitKind::Environment),
            format: "archive".into(),
            values: Default::default(),
        }
    }
    async fn actor(&self) -> Key {
        key(&self.world, self.company.boss.id(), self.rules()).await
    }
    fn content(&self) -> Content {
        Content::over(&self.world.app.app)
    }
    fn start(&self, secret: &str) -> tokio::task::JoinHandle<RawReply> {
        let app = self.world.app.clone();
        let secret = secret.to_owned();
        let body = self.body();
        tokio::spawn(async move {
            let request = app.call(TemplateImport, &[]).token(&secret).body(&body);
            request.send_bytes().await
        })
    }
    async fn command(
        &self,
        pending: &tokio::task::JoinHandle<RawReply>,
    ) -> link::CommandWithAuthority {
        let id=tokio::time::timeout(WAIT,async {loop {
            assert!(!pending.is_finished(),"the real proposal must reach its native command");
            let id:Option<Id>=sqlx::query_scalar("SELECT id FROM commands WHERE machine_id=$1 AND kind='node.template.import' AND state IN ('queued','sent','accepted') ORDER BY created_at DESC,id DESC LIMIT 1").bind(self.machine).fetch_optional(&self.world.db.pool).await.unwrap();
            if let Some(id)=id {break id;}tokio::task::yield_now().await;
        }}).await.expect("the actual selected Host's native proposal command");
        let saved = link::command_with_authority(&mut *self.world.conn().await, id)
            .await
            .unwrap()
            .unwrap();
        let authority: CommandAuthority =
            serde_json::from_value(saved.authority.clone().unwrap()).unwrap();
        let mut tx = self.world.db.pool.begin().await.unwrap();
        assert!(
            authority
                .current_callers(
                    &mut tx,
                    &saved.command,
                    self.world.app.app.clock.as_ref(),
                    &orch_api_core::thread_access::AuthzAccess::new(
                        self.world.app.app.clock.clone()
                    )
                )
                .await
                .unwrap()
                .is_some(),
            "the issued native command has actual current original authority"
        );
        assert!(
            link::mark_accepted(&mut tx, id, self.world.now())
                .await
                .unwrap()
        );
        tx.commit().await.unwrap();
        link::command_with_authority(&mut *self.world.conn().await, id)
            .await
            .unwrap()
            .unwrap()
    }
    async fn finish(&self, id: Id, pending: tokio::task::JoinHandle<RawReply>) -> RawReply {
        let result = json!({"spec":{},"notes":[{"level":"info","text":"A generated proposal"}]});
        link::finish_command(
            &mut *self.world.conn().await,
            id,
            &link::Ending::Succeeded(&result),
            self.world.now(),
        )
        .await
        .unwrap();
        tokio::time::timeout(WAIT, pending).await.unwrap().unwrap()
    }
    async fn narrow(&self, actor: &Key) {
        let mut rules = self.rules();
        rules.pop();
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
            .bind(actor.id)
            .bind(json!({"rules":rules}))
            .execute(&self.world.db.pool)
            .await
            .unwrap();
    }
}
fn success(reply: &RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

#[tokio::test]
async fn template_import_binds_home_host_and_original_upload_before_one_native_proposal() {
    let s = Scene::new().await;
    let templates_before: i64 = sqlx::query_scalar("SELECT count(*) FROM env_templates")
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
    for rules in [
        vec![s.rules()[0].clone()],
        vec![s.rules()[1].clone(), s.rules()[2].clone()],
        vec![
            s.rules()[0].clone(),
            s.rules()[1].clone(),
            rule(
                "UploadBlobGet",
                "account",
                s.company.boss.id(),
                "upload",
                &["read"],
            ),
        ],
    ] {
        let actor = key(&s.world, s.company.boss.id(), rules).await;
        let reply = tokio::time::timeout(WAIT, s.start(&actor.secret))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
    }
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='node.template.import'")
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
    assert_eq!(before, 0);
    let actor = s.actor().await;
    let pending = s.start(&actor.secret);
    let command = s.command(&pending).await;
    assert_eq!(command.command.machine_id, s.machine);
    assert_eq!(command.command.body["values"]["blob"], json!(s.archive));
    let private = command.authority.as_ref().unwrap();
    assert_eq!(private["kind"], "template_import");
    assert_eq!(private["authority"]["origin"]["operation"], OP);
    assert!(!command.command.body.to_string().contains("authority"));
    let read = s
        .content()
        .get(s.machine, command.command.id, &s.archive, None)
        .await;
    assert_eq!(read.status, StatusCode::OK);
    assert_eq!(read.body.as_ref(), ARCHIVE);
    let label = s
        .content()
        .get(s.machine, command.command.id, &s.label, None)
        .await;
    assert_eq!(label.status, StatusCode::FORBIDDEN);
    let shown = success(&s.finish(command.command.id, pending).await);
    assert_eq!(shown["notes"][0]["text"], "A generated proposal");
    let templates: i64 = sqlx::query_scalar("SELECT count(*) FROM env_templates")
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        templates, templates_before,
        "import proposes, without saving a Template"
    );
    let pending = s.start(&s.company.boss.token);
    let command = s.command(&pending).await;
    assert_eq!(
        success(&s.finish(command.command.id, pending).await),
        shown,
        "ordinary native proposal remains the same complete generated reply"
    );
}

#[tokio::test]
async fn template_archive_delivery_keeps_required_private_origin_and_fixed_command_source() {
    let s = Scene::new().await;
    let actor = s.actor().await;
    let pending = s.start(&actor.secret);
    let command = s.command(&pending).await;
    let id = command.command.id;
    let original = command.authority.clone().unwrap();
    let content = s.content();
    assert_eq!(
        content.head(s.machine, id, &s.archive).await.status,
        StatusCode::OK
    );
    assert!(
        link::command_authority_required(&mut *s.world.conn().await, id)
            .await
            .unwrap()
    );
    for value in [
        None,
        Some(json!(null)),
        Some(json!({"version":1,"kind":"unknown","authority":{}})),
    ] {
        sqlx::query("UPDATE commands SET login_authority=$2 WHERE id=$1")
            .bind(id)
            .bind(value)
            .execute(&s.world.db.pool)
            .await
            .unwrap();
        assert_eq!(
            content.get(s.machine, id, &s.archive, None).await.status,
            StatusCode::FORBIDDEN
        );
    }
    sqlx::query("UPDATE commands SET login_authority=$2 WHERE id=$1")
        .bind(id)
        .bind(&original)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        content.get(s.machine, id, &s.archive, None).await.status,
        StatusCode::OK
    );
    let drivers: Json = sqlx::query_scalar("SELECT drivers FROM hosts WHERE id=$1")
        .bind(s.host)
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE hosts SET drivers=jsonb_set(drivers,'{memory,version}',to_jsonb('2.0.0'::text)) WHERE id=$1")
        .bind(s.host).execute(&s.world.db.pool).await.unwrap();
    assert_eq!(
        content.get(s.machine, id, &s.archive, None).await.status,
        StatusCode::FORBIDDEN,
        "a replacement driver cannot borrow the original proposal source"
    );
    sqlx::query("UPDATE hosts SET drivers=$2 WHERE id=$1")
        .bind(s.host)
        .bind(drivers)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    let body = command.command.body;
    sqlx::query("UPDATE commands SET body=jsonb_set(body,'{values,blob}',$2) WHERE id=$1")
        .bind(id)
        .bind(json!(s.label))
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        content.get(s.machine, id, &s.label, None).await.status,
        StatusCode::FORBIDDEN
    );
    sqlx::query("UPDATE commands SET body=$2 WHERE id=$1")
        .bind(id)
        .bind(body)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    s.narrow(&actor).await;
    assert_eq!(
        content.get(s.machine, id, &s.archive, None).await.status,
        StatusCode::FORBIDDEN,
        "Accepted is not a new archive Read grant"
    );
    let reply = s.finish(id, pending).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("A generated proposal"));
}

#[derive(Clone, Copy)]
enum GateAt {
    Open,
    Size,
    Frame,
}
#[derive(Default)]
struct Barrier {
    used: AtomicBool,
    reached: Notify,
    release: Notify,
}
impl Barrier {
    async fn pause(&self) {
        if !self.used.swap(true, Ordering::SeqCst) {
            self.reached.notify_one();
            self.release.notified().await;
        }
    }
}
struct Gate {
    inner: Arc<dyn BlobStore>,
    directory: tempfile::TempDir,
    hash: Hash,
    at: GateAt,
    barrier: Arc<Barrier>,
}
#[async_trait::async_trait]
impl BlobStore for Gate {
    async fn put(
        &self,
        hash: &Hash,
        size: Option<i64>,
        body: ByteStream,
    ) -> Result<Stored, BlobError> {
        self.inner.put(hash, size, body).await
    }
    async fn get(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError> {
        let read = self.inner.get(hash, range).await;
        if *hash != self.hash {
            return read;
        }
        match self.at {
            GateAt::Open => {
                self.barrier.pause().await;
                read
            }
            GateAt::Frame => {
                let mut read = read?;
                let barrier = self.barrier.clone();
                let mut stream = read.body;
                read.body = Box::pin(
                    async_stream::try_stream! {use futures::StreamExt as _;while let Some(piece)=stream.next().await {let bytes=piece?;barrier.pause().await;yield bytes;}},
                );
                Ok(read)
            }
            GateAt::Size => read,
        }
    }
    async fn exists(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.inner.exists(hash).await
    }
    async fn size(&self, hash: &Hash) -> Result<Option<i64>, BlobError> {
        let value = self.inner.size(hash).await?;
        if *hash == self.hash && matches!(self.at, GateAt::Size) {
            self.barrier.pause().await;
        }
        Ok(value)
    }
    async fn delete(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.inner.delete(hash).await
    }
}
fn gate(s: &mut Scene, at: GateAt) -> Arc<Barrier> {
    use orch_api_core::{registry::Registry, state::AppState};
    let old = s.world.app.app.clone();
    let barrier = Arc::new(Barrier::default());
    let store = Arc::new(Gate {
        inner: old.blobs.store(),
        directory: tempfile::tempdir().unwrap(),
        hash: s.archive,
        at,
        barrier: barrier.clone(),
    });
    let app = Arc::new(AppState {
        db: old.db.clone(),
        config: old.config.clone(),
        clock: old.clock.clone(),
        link: old.link.clone(),
        engine: old.engine.clone(),
        realtime: old.realtime.clone(),
        build: old.build,
        blobs: orch_api_core::blobs::Blobs::new(store.clone(), store.directory.path()),
        cache: orch_api_core::cache::Cache::new(&old.config),
        host_ports: old.host_ports.clone(),
        builtin_sources: old.builtin_sources.clone(),
    });
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    s.world.app = TestApp::over(app, s.world.app.clock.clone(), registry);
    barrier
}

#[tokio::test]
async fn template_archive_metadata_rechecks_after_actual_store_open_and_head_size_waits() {
    for (at, head, range) in [
        (GateAt::Open, false, None),
        (GateAt::Open, false, Some("bytes=99999-")),
        (GateAt::Size, true, None),
    ] {
        for closed in [false, true] {
            let mut s = Scene::new().await;
            let actor = s.actor().await;
            let pending = s.start(&actor.secret);
            let command = s.command(&pending).await;
            let barrier = gate(&mut s, at);
            let content = s.content();
            let id = command.command.id;
            let machine = s.machine;
            let hash = s.archive;
            let request = tokio::spawn(async move {
                if head {
                    content.head(machine, id, &hash).await
                } else {
                    content.get(machine, id, &hash, range).await
                }
            });
            tokio::time::timeout(WAIT, barrier.reached.notified())
                .await
                .expect("the actual native store has reached its read/metadata boundary");
            if closed {
                s.narrow(&actor).await;
            }
            barrier.release.notify_one();
            let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
            assert_eq!(
                reply.status,
                if closed {
                    StatusCode::FORBIDDEN
                } else if range.is_some() {
                    StatusCode::RANGE_NOT_SATISFIABLE
                } else {
                    StatusCode::OK
                }
            );
            if closed {
                assert!(!reply.headers.contains_key("content-range"));
                assert!(
                    !reply
                        .body
                        .windows(ARCHIVE.len())
                        .any(|part| part == ARCHIVE)
                );
            }
            let reply = s.finish(id, pending).await;
            if closed {
                assert_eq!(reply.status, StatusCode::FORBIDDEN);
            } else {
                success(&reply);
            }
        }
    }
}

#[tokio::test]
async fn template_archive_stream_checks_its_original_source_after_the_actual_upstream_frame_wait() {
    use http_body_util::BodyExt;
    for closed in [false, true] {
        let mut s = Scene::new().await;
        let actor = s.actor().await;
        let pending = s.start(&actor.secret);
        let command = s.command(&pending).await;
        let barrier = gate(&mut s, GateAt::Frame);
        let content = s.content();
        let id = command.command.id;
        let request = content.request(
            Method::GET,
            &format!("/internal/link/blobs/{}", s.archive),
            Some(s.machine),
            Some(id),
            &[],
            Bytes::new(),
        );
        let response = content.send_open(request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body();
        let read = tokio::spawn(async move { body.frame().await });
        tokio::time::timeout(WAIT, barrier.reached.notified())
            .await
            .expect("the actual stored native archive frame is waiting");
        if closed {
            s.narrow(&actor).await;
        }
        barrier.release.notify_one();
        let frame = tokio::time::timeout(WAIT, read)
            .await
            .unwrap()
            .unwrap()
            .expect("one upstream result");
        if closed {
            assert!(
                frame.is_err(),
                "no private archive frame may cross a closed source"
            );
        } else {
            assert_eq!(frame.unwrap().into_data().unwrap().as_ref(), ARCHIVE);
        }
        let reply = s.finish(id, pending).await;
        if closed {
            assert_eq!(reply.status, StatusCode::FORBIDDEN);
        } else {
            success(&reply);
        }
    }
}

#[tokio::test]
async fn template_archive_rechecks_after_its_actual_blob_touch_lock_and_expired_origin() {
    for expired in [false, true] {
        let s = Scene::new().await;
        let actor = s.actor().await;
        let pending = s.start(&actor.secret);
        let command = s.command(&pending).await;
        let id = command.command.id;
        let mut locked = s.world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT hash FROM blobs WHERE hash=$1 FOR UPDATE")
            .bind(s.archive.as_bytes().as_slice())
            .fetch_one(&mut *locked)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *locked)
            .await
            .unwrap();
        let content = s.content();
        let machine = s.machine;
        let hash = s.archive;
        let request = tokio::spawn(async move { content.get(machine, id, &hash, None).await });
        tokio::time::timeout(WAIT,async{loop{assert!(!request.is_finished());let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%UPDATE blobs SET last_access_at%')").bind(pid).fetch_one(&s.world.db.pool).await.unwrap();if waiting{break;}tokio::task::yield_now().await;}}).await.expect("native byte delivery reached its real cache-touch lock");
        if expired {
            s.world.advance(chrono::Duration::hours(2));
        }
        locked.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        assert_eq!(
            reply.status,
            if expired {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::OK
            }
        );
        if expired {
            assert!(
                !reply
                    .body
                    .windows(ARCHIVE.len())
                    .any(|part| part == ARCHIVE)
            );
        }
        let reply = s.finish(id, pending).await;
        if expired {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        } else {
            success(&reply);
        }
    }
}

#[tokio::test]
async fn template_archive_keeps_genuine_old_native_commands_without_inventing_a_session() {
    let s = Scene::new().await;
    let id=workspace::issue(&s.world.app.app,s.machine,CommandKind::NodeTemplateImport,&json!({"driver":"memory","kind":"environment","format":"archive","values":{"blob":s.archive,"note":s.label}})).await;
    assert!(
        !link::command_authority_required(&mut *s.world.conn().await, id)
            .await
            .unwrap()
    );
    let content = s.content();
    let reply = content.get(s.machine, id, &s.archive, None).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body.as_ref(), ARCHIVE);
    assert_eq!(
        content.head(s.machine, id, &s.archive).await.status,
        StatusCode::OK
    );
    assert_eq!(
        content.get(s.machine, id, &s.label, None).await.status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn template_archive_uses_its_actual_browser_session_after_native_acceptance() {
    let s = Scene::new().await;
    let pending = s.start(&s.company.boss.token);
    let command = s.command(&pending).await;
    let id = command.command.id;
    let evidence = command.authority.unwrap();
    let session: Id = evidence["authority"]["session"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let user: Id = sqlx::query_scalar("SELECT user_id FROM sessions WHERE id=$1")
        .bind(session)
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
    assert_eq!(user, s.company.boss.id());
    let content = s.content();
    assert_eq!(
        content.get(s.machine, id, &s.archive, None).await.status,
        StatusCode::OK
    );
    sqlx::query("UPDATE sessions SET revoked_at=$2 WHERE id=$1")
        .bind(session)
        .bind(s.world.now())
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        content.get(s.machine, id, &s.archive, None).await.status,
        StatusCode::FORBIDDEN
    );
    let reply = s.finish(id, pending).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("A generated proposal"));
}

#[tokio::test]
async fn template_import_rechecks_after_the_actual_host_lock_before_its_first_native_command() {
    for expired in [false, true] {
        let s = Scene::new().await;
        let actor = s.actor().await;
        let mut blocker = s.world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM hosts WHERE id=$1 FOR UPDATE")
            .bind(s.host)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pending = s.start(&actor.secret);
        tokio::time::timeout(WAIT, async {
            loop {
                assert!(!pending.is_finished(), "the native Host lock must be reached");
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%SELECT id FROM hosts%FOR SHARE%')")
                    .bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
                if waiting { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual proposal Host source is waiting");
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='node.template.import'")
                .fetch_one(&s.world.db.pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
        if expired {
            s.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        if expired {
            let reply = tokio::time::timeout(WAIT, pending).await.unwrap().unwrap();
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM commands WHERE kind='node.template.import'",
            )
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
            assert_eq!(count, 0);
        } else {
            let command = s.command(&pending).await;
            success(&s.finish(command.command.id, pending).await);
        }
    }
}
