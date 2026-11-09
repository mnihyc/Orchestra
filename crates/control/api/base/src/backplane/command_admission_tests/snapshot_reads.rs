//! An actual HTTP read queues the native command before its original key or source changes.
//! The parent's real journal is authoritative for Accepted; envhost byte I/O has separate controls.

use super::*;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::workspace::SnapshotFileList;

use orch_api_contract::workspace::SnapshotPathQuery;

use orch_api_core::middleware::authentication::API_TOKEN_PREFIX;

use orch_api_core::middleware::authentication::new_token;

use orch_api_core::middleware::authentication::token_hash;

use orch_api_core::registry::Registry;

use orch_api_core::state::AppState;

use orch_api_core::testing::RawReply;

use orch_api_core::testing::TestApp;

use orch_authz::ownership;

use orch_blob_contract::store::stream_of;

use orch_db::queries::access;

use orch_db::queries::identity;

use orch_db::queries::storage;

use orch_engine::manifest::Entry;

use orch_engine::manifest::EntryKind;

use orch_engine::manifest::Header;

use orch_engine::manifest::MANIFEST_VERSION;

use orch_link_contract::source::FsRecordRead;

use orch_link_contract::source::FsRecordReadResult;
#[derive(Clone)]
struct Answer {
    environment: Id,
    manifest: Hash,
    size: i64,
}

static ANSWERS: OnceLock<Mutex<HashMap<Id, Answer>>> = OnceLock::new();

struct Registration(Id);

impl Drop for Registration {
    fn drop(&mut self) {
        ANSWERS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.0);
    }
}

pub(super) fn reply(
    context: CommandContext,
    body: FsRecordRead,
) -> Result<CommandResult, CommandError> {
    let expected = ANSWERS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&body.snapshot_id)
        .cloned()
        .ok_or_else(|| CommandError::new(ErrorCode::NotFound, "no such retained peer snapshot"))?;
    if body.environment_id != expected.environment
        || body.path.is_some()
        || body.export
        || context.key.is_some()
    {
        return Err(CommandError::new(
            ErrorCode::Conflict,
            "not the retained manifest-only read",
        ));
    }
    Ok(CommandResult::FsRecordRead(FsRecordReadResult {
        blob: expected.manifest,
        size: expected.size,
    }))
}

struct ReadFixture {
    transport: Fixture,
    app: TestApp,
    snapshot: Id,
    token_id: Id,
    token: String,
    owner: Id,
    _registration: Registration,
}

impl ReadFixture {
    async fn new() -> Proof<Self> {
        let transport = Fixture::new(false).await?;
        let (db, clock) = (
            transport.link.inner.db.clone(),
            transport.link.inner.clock.clone(),
        );
        let config = Arc::new(ServerConfig::default());
        let engine = crate::thread_engine::detached(&db, &clock, &config, &transport.link);
        let blobs = crate::blobs::Blobs::in_memory();
        engine.attach_blobs(blobs.store());
        let state = Arc::new(AppState {
            db: db.clone(),
            config: config.clone(),
            clock: clock.clone(),
            link: transport.link.clone(),
            engine,
            realtime: crate::realtime::Realtime::new(&db, &clock, &config),
            build: orch_build_info::BuildInfo::new("api", "0.0.0-test", "", "1"),
            blobs,
            cache: crate::cache::Cache::new(&config),
            host_ports: Default::default(),
            builtin_sources: Default::default(),
        });
        let mut registry = Registry::new();
        crate::routes::mount(&mut registry);
        let app = TestApp::over(state, transport.clock.clone(), registry);
        let mut conn = transport.test.pool.acquire().await?;
        let (owner, org, root): (Id, Id, String) =
            sqlx::query_as("SELECT owner_id,org_id,workspace_root FROM environments WHERE id=$1")
                .bind(transport.environment)
                .fetch_one(&mut *conn)
                .await?;
        access::testing::insert_member(&mut conn, org, owner, org).await?;
        access::testing::name_owner(&mut conn, org, ("user", owner), transport.clock.now()).await?;
        ownership::start_resource(
            &mut conn,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: transport.environment,
            },
            owner,
            ownership::NewAccess::OnlyMe,
            transport.clock.time(),
        )
        .await?;
        // Current custodian generation2 is deliberately distinct from the historical recording1.
        sqlx::query("UPDATE environments SET generation=2 WHERE id=$1")
            .bind(transport.environment)
            .execute(&mut *conn)
            .await?;
        sqlx::query("UPDATE machines SET online=true WHERE id=$1")
            .bind(transport.machine)
            .execute(&mut *conn)
            .await?;
        let (project, topic, thread) = (new_id(), new_id(), new_id());
        hierarchy::testing::insert_node(
            &mut conn,
            project,
            "project",
            Some(org),
            "Stored content",
            owner,
        )
        .await?;
        hierarchy::testing::insert_node(
            &mut conn,
            topic,
            "topic",
            Some(project),
            "Recorded content",
            owner,
        )
        .await?;
        hierarchy::testing::insert_node(
            &mut conn,
            thread,
            "thread",
            Some(topic),
            "Recorded thread",
            owner,
        )
        .await?;
        sqlx::query("UPDATE threads SET environment_id=$2,workspace_dir='web' WHERE node_id=$1")
            .bind(thread)
            .bind(transport.environment)
            .execute(&mut *conn)
            .await?;
        let snapshot = new_id();
        let manifest = orch_engine::manifest::build(
            &Header {
                v: MANIFEST_VERSION,
                root: format!("{root}/web"),
                generated_at: transport.clock.time(),
                generation: 1,
                case_sensitive: true,
            },
            vec![Entry {
                path: "recorded.bin".into(),
                kind: EntryKind::File,
                size: 8,
                mtime_ns: 0,
                mode: 0o600,
                hash: Some(Hash::of(b"retained")),
                link: None,
            }],
        )?;
        app.app
            .blobs
            .store()
            .put(
                &manifest.hash,
                Some(manifest.bytes.len() as i64),
                stream_of(vec![bytes::Bytes::copy_from_slice(&manifest.bytes)]),
            )
            .await?;
        storage::register_blob(
            &mut conn,
            &manifest.hash,
            manifest.bytes.len() as i64,
            transport.clock.now(),
        )
        .await?;
        storage::begin_snapshot(
            &mut conn,
            &storage::NewSnapshot {
                id: snapshot,
                thread_id: Some(thread),
                environment_id: transport.environment,
                env_generation: 1,
                label: "Original recorded source",
                selection: &json!({"include":["**"],"exclude":[]}),
                created_by: Some(owner),
                at: transport.clock.now(),
            },
        )
        .await?;
        storage::end_snapshot(
            &mut conn,
            snapshot,
            &storage::SnapshotEnd {
                state: storage::snapshot_state::COMPLETE,
                file_count: 1,
                total_bytes: 8,
                missing_count: 0,
                manifest_blob: Some(manifest.hash),
                at: transport.clock.now(),
            },
        )
        .await?;
        sqlx::query("UPDATE snapshots SET kind='turn',storage='environment',directory='web',tree_ref='original-recorded-root',expires_at=$2 WHERE id=$1")
            .bind(snapshot).bind(transport.clock.now() + chrono::Duration::days(7)).execute(&mut *conn).await?;
        let token_id = new_id();
        let token = new_token(API_TOKEN_PREFIX);
        let scopes = vec!["files.view".to_owned()];
        orch_authz::TokenGrant::from_stored(&scopes, None)?;
        identity::create_api_token_scoped(
            &mut conn,
            &identity::NewApiToken {
                id: token_id,
                user_id: owner,
                name: "Real queued Snapshot reader",
                token_hash: &token_hash(&token),
                scopes: &scopes,
                node_id: None,
                at: transport.clock.now(),
                expires_at: Some(transport.clock.now() + chrono::Duration::hours(1)),
            },
            None,
        )
        .await?;
        drop(conn);
        assert!(
            ANSWERS
                .get_or_init(Mutex::default)
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(
                    snapshot,
                    Answer {
                        environment: transport.environment,
                        manifest: manifest.hash,
                        size: manifest.bytes.len() as i64
                    }
                )
                .is_none()
        );
        Ok(Self {
            transport,
            app,
            snapshot,
            token_id,
            token,
            owner,
            _registration: Registration(snapshot),
        })
    }

    fn start(&self) -> JoinHandle<RawReply> {
        let (app, snapshot, token) = (self.app.clone(), self.snapshot, self.token.clone());
        tokio::spawn(async move {
            let request = app
                .call(SnapshotFileList, &[&snapshot])
                .token(&token)
                .query(&SnapshotPathQuery {
                    path: String::new(),
                });
            request.send_bytes().await
        })
    }

    async fn command(&self) -> Proof<rows::CommandRow> {
        let ids: Vec<Id> = sqlx::query_scalar(
            "SELECT id FROM commands WHERE kind='fs.record.read' AND body->>'snapshot_id'=$1",
        )
        .bind(self.snapshot.to_string())
        .fetch_all(&self.transport.test.pool)
        .await?;
        if ids.len() != 1 {
            return Err("the Snapshot read did not own exactly one actual command".into());
        }
        rows::command(&mut *self.transport.test.pool.acquire().await?, ids[0])
            .await?
            .ok_or_else(|| "the canonical Snapshot command disappeared".into())
    }

    async fn queued(&self) -> Proof<rows::CommandRow> {
        within(async {
            loop {
                let ids: Vec<Id> = sqlx::query_scalar("SELECT id FROM commands WHERE kind='fs.record.read' AND body->>'snapshot_id'=$1")
                    .bind(self.snapshot.to_string()).fetch_all(&self.transport.test.pool).await?;
                if !ids.is_empty() { return self.command().await; }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await?
    }
}

async fn same_path_positive() -> Proof {
    let mut read = ReadFixture::new().await?;
    within(read.transport.connect(false)).await??;
    let response = within(read.start()).await??;
    assert_eq!(
        response.status,
        http::StatusCode::OK,
        "ordinary legacy path: {}",
        String::from_utf8_lossy(&response.raw)
    );
    let value: orch_api_contract::workspace::DirListing = serde_json::from_slice(&response.raw)?;
    assert_eq!(value.entries.len(), 1);
    assert_eq!(value.entries[0].name, "recorded.bin");
    let command = read.command().await?;
    assert_eq!(command.machine_id, read.transport.machine);
    assert_eq!(command.kind, "fs.record.read");
    assert!(command.serial_key().is_none());
    assert_eq!(command.state, "succeeded");
    assert!(
        matches!(read.transport.server.entry(command.id).ok_or("the same-path positive has no real journal entry")?.step,
        Step::Done { ref result } if result.outcome.is_ok()),
        "actual Accepted and successful native result premise"
    );
    assert!(
        read.transport
            .native
            .wire
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|frame| frame.id == command.id
                && frame.kind == command.kind
                && frame.key.is_none()
                && frame.body == command.body)
    );
    read.transport.completed_probe().await?;
    read.transport.close().await
}

#[derive(Clone, Copy, Debug)]
enum Change {
    Revoke,
    CustodianGeneration,
}

async fn changed_before_wire(change: Change) -> Proof {
    // Mandatory same-path native Accepted/result proof prevents unsupported-kind/role false green.
    same_path_positive().await?;
    let mut read = ReadFixture::new().await?;
    let mut response = read.start();
    let original = tokio::select! {
        command = read.queued() => command?,
        response = &mut response => {
            let response = response?;
            return Err(format!("read never reached its actual disconnected queue: {} {}", response.status,
                String::from_utf8_lossy(&response.raw)).into());
        }
    };
    assert_eq!(original.machine_id, read.transport.machine);
    assert_eq!(original.kind, "fs.record.read");
    assert!(original.serial_key().is_none());
    assert_eq!(original.body["snapshot_id"], json!(read.snapshot));
    assert_eq!(
        original.body["environment_id"],
        json!(read.transport.environment)
    );
    assert!(
        read.transport.server.entry(original.id).is_none(),
        "the held peer never Accepted this queued read"
    );
    match change {
        Change::Revoke => {
            assert!(
                identity::revoke_api_token(
                    &mut *read.transport.test.pool.acquire().await?,
                    read.token_id,
                    read.owner,
                    read.transport.clock.now()
                )
                .await?
            );
        }
        Change::CustodianGeneration => {
            sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
                .bind(read.transport.environment)
                .execute(&read.transport.test.pool)
                .await?;
        }
    }
    within(read.transport.connect(false)).await??;
    read.transport.completed_probe().await?;
    let denied = within(response).await??;
    let retained = read.command().await?;
    assert!(
        read.transport.server.entry(original.id).is_none(),
        "{change:?}: no new native Accepted after original authority/source loss"
    );
    assert!(
        !read
            .transport
            .native
            .wire
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|frame| frame.id == original.id),
        "no exact revoked/rebound command frame crossed the real gateway cut"
    );
    assert!(
        matches!(retained.state.as_str(), "failed" | "cancelled" | "expired"),
        "actual refusal reached: {}",
        retained.state
    );
    assert!(
        !denied.status.is_success(),
        "no public data borrows the old command"
    );
    assert_eq!(retained.body, original.body);
    assert_eq!(retained.kind, original.kind);
    assert_eq!(retained.serial_key(), original.serial_key());
    read.transport.close().await
}

#[cfg(any(test, feature = "test-support"))]
async fn ordinary_snapshot_http_read_has_an_actual_native_journal_result() -> Proof {
    same_path_positive().await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_ordinary_snapshot_http_read_has_an_actual_native_journal_result()
-> ::futures::future::LocalBoxFuture<
    'static,
    ::orch_api_base::backplane::command_admission_tests::Proof,
> {
    ::std::boxed::Box::pin(self::ordinary_snapshot_http_read_has_an_actual_native_journal_result())
}

#[cfg(any(test, feature = "test-support"))]
async fn revoked_queued_snapshot_http_read_never_reaches_native_accepted_after_reconnect() -> Proof
{
    changed_before_wire(Change::Revoke).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_revoked_queued_snapshot_http_read_never_reaches_native_accepted_after_reconnect()
-> ::futures::future::LocalBoxFuture<
    'static,
    ::orch_api_base::backplane::command_admission_tests::Proof,
> {
    ::std::boxed::Box::pin(
        self::revoked_queued_snapshot_http_read_never_reaches_native_accepted_after_reconnect(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn queued_snapshot_read_cannot_rebind_a_new_custodian_generation_to_its_old_wire_command()
-> Proof {
    changed_before_wire(Change::CustodianGeneration).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_queued_snapshot_read_cannot_rebind_a_new_custodian_generation_to_its_old_wire_command()
-> ::futures::future::LocalBoxFuture<
    'static,
    ::orch_api_base::backplane::command_admission_tests::Proof,
> {
    ::std::boxed::Box::pin(
        self::queued_snapshot_read_cannot_rebind_a_new_custodian_generation_to_its_old_wire_command(
        ),
    )
}
