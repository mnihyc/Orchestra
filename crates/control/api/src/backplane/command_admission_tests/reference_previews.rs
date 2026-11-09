//! Full reference endpoint over the production dispatcher and a real journaled file peer.
use super::*;
use crate::{registry::Registry, state::AppState, testing::TestApp};
use orch_access_contract::{decisions::ObjectRef, key_scope::KeyScope, permissions::ObjectKind};
use orch_api_contract::documents::{
    InlineRefKind, ReferencePreview, ReferencePreviewGet, ReferencePreviewQuery,
};
use orch_authz::ownership;
use orch_blob_contract::store::{BlobStore, stream_of};
use orch_db::queries::{access, identity};
use tokio::sync::Notify;

const TEXT: &[u8] = b"actual native reference file\nsecond file line\n";
struct ReaderPeer {
    native: Arc<Native>,
    blobs: Arc<dyn BlobStore>,
    hold: bool,
    reached: Notify,
    release: Notify,
    stats: AtomicUsize,
    fetches: AtomicUsize,
}
#[async_trait::async_trait]
impl CommandHandler for ReaderPeer {
    async fn handle(
        &self,
        context: CommandContext,
        command: Command,
    ) -> Result<CommandResult, CommandError> {
        match command {
            Command::FsStat(body)
                if body.environment_id == self.native.environment
                    && body.mount.is_none()
                    && body.path == "preview.txt"
                    && context.key.is_none() =>
            {
                let data = tokio::fs::read(self.native._directory.path().join(&body.path))
                    .await
                    .map_err(|_| CommandError::new(ErrorCode::NotFound, "file missing"))?;
                self.stats.fetch_add(1, Ordering::SeqCst);
                if self.hold {
                    self.reached.notify_one();
                    self.release.notified().await;
                }
                Ok(CommandResult::FsStat(
                    orch_api_contract::workspace::FileStat {
                        entry: orch_api_contract::workspace::DirEntry {
                            name: body.path.clone(),
                            path: body.path,
                            kind: orch_api_contract::workspace::DirEntryKind::File,
                            size: i64::try_from(data.len())
                                .map_err(|_| CommandError::new(ErrorCode::TooLarge, "file size"))?,
                            mtime: self.native.clock.time(),
                            mode: 0o600,
                            link_target: None,
                            cached: false,
                        },
                        hash: Some(Hash::of(&data)),
                        media_type: "text/plain".into(),
                        is_text: true,
                    },
                ))
            }
            Command::FsFetch(body)
                if body.environment_id == self.native.environment
                    && body.mount.is_none()
                    && body.path == "preview.txt"
                    && context.key.is_none() =>
            {
                let data = tokio::fs::read(self.native._directory.path().join(&body.path))
                    .await
                    .map_err(|_| CommandError::new(ErrorCode::NotFound, "file missing"))?;
                let size = i64::try_from(data.len())
                    .map_err(|_| CommandError::new(ErrorCode::TooLarge, "file size"))?;
                if body.max_bytes.is_none_or(|limit| limit < size) {
                    return Err(CommandError::new(ErrorCode::TooLarge, "file limit"));
                }
                let hash = Hash::of(&data);
                self.blobs
                    .put(&hash, Some(size), stream_of(vec![bytes::Bytes::from(data)]))
                    .await
                    .map_err(|_| CommandError::new(ErrorCode::Internal, "file upload failed"))?;
                self.fetches.fetch_add(1, Ordering::SeqCst);
                Ok(CommandResult::FsFetch(
                    orch_link_contract::commands::FsFetchResult {
                        hash,
                        size,
                        mtime_ns: self
                            .native
                            .clock
                            .now()
                            .timestamp_nanos_opt()
                            .ok_or_else(|| CommandError::new(ErrorCode::Internal, "file time"))?,
                    },
                ))
            }
            command => self.native.handle(context, command).await,
        }
    }
}
fn rules(environment: Id) -> serde_json::Value {
    json!({"rules":[{"target":{"kind":"environment","id":environment,"reach":"self"},"allow":[{"operation":"ReferencePreviewGet","actions":["read"]}]}]})
}
#[tokio::test]
async fn reference_live_stat_fetch_and_delivery_keep_raw_identity_physical_source_and_exactly_once_io()
-> Proof {
    for change in ["none", "raw_rotated", "scope_closed", "generation", "root"] {
        let mut transport = Fixture::new(false).await?;
        let mut registry = Registry::new();
        crate::routes::mount(&mut registry);
        let original = TestApp::new(
            transport.link.inner.db.clone(),
            transport.clock.clone(),
            Registry::new(),
        );
        let old = &original.app;
        let app = TestApp::over(
            Arc::new(AppState {
                db: old.db.clone(),
                config: old.config.clone(),
                clock: old.clock.clone(),
                link: transport.link.clone(),
                engine: old.engine.clone(),
                realtime: old.realtime.clone(),
                build: old.build,
                blobs: old.blobs.clone(),
                cache: crate::cache::Cache::new(&old.config),
                host_ports: old.host_ports.clone(),
                builtin_sources: old.builtin_sources.clone(),
            }),
            transport.clock.clone(),
            registry,
        );
        let mut conn = transport.test.pool.acquire().await?;
        let (owner, home): (Id, Id) =
            sqlx::query_as("SELECT owner_id,home_node_id FROM environments WHERE id=$1")
                .bind(transport.environment)
                .fetch_one(&mut *conn)
                .await?;
        access::testing::insert_member(&mut conn, home, owner, home).await?;
        access::testing::name_owner(&mut conn, home, ("user", owner), transport.clock.now())
            .await?;
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
        let policy: KeyScope = serde_json::from_value(rules(transport.environment))?;
        assert!(policy.validate().is_empty());
        let token = crate::middleware::authentication::new_token(
            crate::middleware::authentication::API_TOKEN_PREFIX,
        );
        let id = new_id();
        identity::create_api_token_scoped(
            &mut conn,
            &identity::NewApiToken {
                id,
                user_id: owner,
                name: "Reference live read",
                token_hash: &crate::middleware::authentication::token_hash(&token),
                scopes: &[],
                node_id: None,
                at: transport.clock.now(),
                expires_at: Some(transport.clock.now() + chrono::Duration::hours(1)),
            },
            Some(&json!(policy)),
        )
        .await?;
        drop(conn);
        tokio::fs::write(transport._directory.path().join("preview.txt"), TEXT).await?;
        let reader = Arc::new(ReaderPeer {
            native: transport.native.clone(),
            blobs: app.app.blobs.store(),
            hold: change != "none",
            reached: Notify::new(),
            release: Notify::new(),
            stats: AtomicUsize::new(0),
            fetches: AtomicUsize::new(0),
        });
        transport.server = CommandServer::new(
            Journal::open(
                &transport._directory.path().join("reference-journal"),
                Duration::from_secs(24 * 3600),
                transport.clock.time(),
            )?,
            Arc::new(transport.clock.clone()),
            ServerOptions {
                roles: vec![Role::Environment],
                connected: true,
            },
        );
        transport.server.serve(reader.clone());
        within(transport.connect(false)).await??;
        sqlx::query("UPDATE machines SET online=true,last_seen_at=$2 WHERE id=$1")
            .bind(transport.machine)
            .bind(transport.clock.now())
            .execute(&transport.test.pool)
            .await?;
        sqlx::query("UPDATE environments e SET attached_session=COALESCE(m.session_started_at,m.last_seen_at) FROM machines m WHERE e.id=$1 AND m.id=e.machine_id").bind(transport.environment).execute(&transport.test.pool).await?;
        let environment = transport.environment;
        let query = ReferencePreviewQuery {
            kind: InlineRefKind::EnvironmentFile,
            id: environment,
            path: Some("preview.txt".into()),
            line: Some(1),
            block: None,
            turn: None,
            message: None,
        };
        let request = tokio::spawn(async move {
            let request = app
                .call(ReferencePreviewGet, &[])
                .token(&token)
                .query(&query);
            request.send_bytes().await
        });
        if change != "none" {
            within(reader.reached.notified()).await?;
            match change {
                "raw_rotated" => {
                    sqlx::query("UPDATE api_tokens SET token_hash=$2 WHERE id=$1")
                        .bind(id)
                        .bind(crate::middleware::authentication::token_hash(
                            "synthetic replacement bearer",
                        ))
                        .execute(&transport.test.pool)
                        .await?;
                }
                "scope_closed" => {
                    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                        .bind(id)
                        .bind(json!({"rules":[]}))
                        .execute(&transport.test.pool)
                        .await?;
                }
                "generation" => {
                    sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
                        .bind(environment)
                        .execute(&transport.test.pool)
                        .await?;
                }
                "root" => {
                    sqlx::query("UPDATE environments SET workspace_root=workspace_root || '/replacement' WHERE id=$1").bind(environment).execute(&transport.test.pool).await?;
                }
                _ => {}
            }
            reader.release.notify_one();
        }
        let reply = within(request).await??;
        assert_eq!(reader.stats.load(Ordering::SeqCst), 1);
        if change == "none" {
            assert_eq!(
                reply.status,
                http::StatusCode::OK,
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            let shown: ReferencePreview = serde_json::from_slice(&reply.raw)?;
            assert!(
                shown
                    .part
                    .ok_or("missing line")?
                    .text
                    .contains("actual native reference file")
            );
            assert_eq!(reader.fetches.load(Ordering::SeqCst), 1);
            let saved:Vec<(String,bool)>=sqlx::query_as("SELECT kind,authority_required FROM commands WHERE machine_id=$1 AND kind IN ('fs.stat','fs.fetch') ORDER BY kind").bind(transport.machine).fetch_all(&transport.test.pool).await?;
            assert_eq!(saved.len(), 2);
            assert!(saved.iter().all(|(_, required)| *required));
        } else {
            assert!(
                !reply.status.is_success(),
                "a completed old source cannot become a new read"
            );
            assert!(!String::from_utf8_lossy(&reply.raw).contains("actual native reference file"));
            assert_eq!(reader.fetches.load(Ordering::SeqCst), 0);
            let cached: i64 =
                sqlx::query_scalar("SELECT count(*) FROM file_cache WHERE environment_id=$1")
                    .bind(environment)
                    .fetch_one(&transport.test.pool)
                    .await?;
            assert_eq!(cached, 0);
        }
        transport.close().await?;
    }
    Ok(())
}
