//! Native Skill creation endpoint over the production dispatcher and real journal/file/blob peers.

use super::*;

use crate::registry::Registry;

use crate::state::AppState;

use crate::testing::TestApp;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyScope;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::skills::SkillCreate;

use orch_api_contract::skills::SkillCreateBody;

use orch_api_contract::skills::SkillSource;

use orch_api_contract::workspace::DirEntry;

use orch_api_contract::workspace::DirEntryKind;

use orch_authz::ownership;

use orch_blob_contract::store::BlobStore;

use orch_blob_contract::store::stream_of;

use orch_db::queries::access;

use orch_db::queries::identity;

use orch_link_contract::commands::BlobRef;

use orch_link_contract::commands::FsFetchResult;

use orch_link_contract::commands::FsListResult;

use orch_link_contract::commands::NodeSkillFetchResult;

use orch_link_contract::commands::Secret;

use orch_link_contract::commands::SecretOrText;

use orch_link_contract::commands::SkillFetchSource;

use tokio::sync::Notify;
const FILE:&[u8]=b"---\nname: native-skill\ndescription: A real native skill source.\n---\nOriginal native source bytes.\n";

const PRIVATE_URL: &str = "https://example.invalid/skill.tar.gz?token=synthetic-private-source";

#[derive(Clone, Copy)]
enum Kind {
    Environment,
    Git,
    Url,
}

struct SkillPeer {
    native: Arc<Native>,
    blobs: Arc<dyn BlobStore>,
    key: orch_pki::certificates::LeafKey,
    archive: Vec<u8>,
    hold: bool,
    reached: Notify,
    release: Notify,
    lists: AtomicUsize,
    files: AtomicUsize,
    archives: AtomicUsize,
}

impl SkillPeer {
    async fn pause(&self) {
        if self.hold {
            self.reached.notify_one();
            self.release.notified().await;
        }
    }
}

#[async_trait::async_trait]
impl CommandHandler for SkillPeer {
    async fn handle(
        &self,
        context: CommandContext,
        command: Command,
    ) -> Result<CommandResult, CommandError> {
        match command {
            Command::FsList(body)
                if body.environment_id == self.native.environment
                    && body.path == "skill"
                    && body.mount.is_none()
                    && context.key.is_none() =>
            {
                let path = self.native._directory.path().join("skill/SKILL.md");
                let metadata = tokio::fs::metadata(path).await.map_err(|_| {
                    CommandError::new(ErrorCode::NotFound, "the actual Skill file is absent")
                })?;
                self.lists.fetch_add(1, Ordering::SeqCst);
                self.pause().await;
                Ok(CommandResult::FsList(FsListResult {
                    entries: vec![DirEntry {
                        name: "SKILL.md".into(),
                        path: "skill/SKILL.md".into(),
                        kind: DirEntryKind::File,
                        size: i64::try_from(metadata.len())
                            .map_err(|_| CommandError::new(ErrorCode::TooLarge, "file size"))?,
                        mtime: self.native.clock.time(),
                        mode: 0o644,
                        link_target: None,
                        cached: false,
                    }],
                    truncated: false,
                    watch_instance: None,
                    page: None,
                }))
            }
            Command::FsFetch(body)
                if body.environment_id == self.native.environment
                    && body.path == "skill/SKILL.md"
                    && body.mount.is_none()
                    && context.key.is_none() =>
            {
                let bytes = tokio::fs::read(self.native._directory.path().join(&body.path))
                    .await
                    .map_err(|_| {
                        CommandError::new(ErrorCode::NotFound, "the actual Skill file is absent")
                    })?;
                let size = i64::try_from(bytes.len())
                    .map_err(|_| CommandError::new(ErrorCode::TooLarge, "file size"))?;
                if body.max_bytes.is_none_or(|limit| limit < size) {
                    return Err(CommandError::new(ErrorCode::TooLarge, "file limit"));
                }
                let hash = Hash::of(&bytes);
                self.blobs
                    .put(
                        &hash,
                        Some(size),
                        stream_of(vec![bytes::Bytes::from(bytes)]),
                    )
                    .await
                    .map_err(|_| CommandError::new(ErrorCode::Internal, "file upload"))?;
                self.files.fetch_add(1, Ordering::SeqCst);
                Ok(CommandResult::FsFetch(FsFetchResult {
                    hash,
                    size,
                    mtime_ns: self
                        .native
                        .clock
                        .now()
                        .timestamp_nanos_opt()
                        .ok_or_else(|| CommandError::new(ErrorCode::Internal, "file time"))?,
                }))
            }
            Command::NodeSkillFetch(body) if context.key.is_none() => {
                match &body.source {
                    SkillFetchSource::Git {
                        repository,
                        reference,
                        folder,
                    } if repository == "https://example.invalid/native.git"
                        && reference == "main"
                        && folder.is_none() => {}
                    SkillFetchSource::Url {
                        url: SecretOrText::Secret(Secret::Sealed(sealed)),
                    } => {
                        let opened = orch_pki::sealing::open(
                            &self.key,
                            &orch_pki::sealing::info_for_command(context.id),
                            sealed,
                        )
                        .map_err(|_| {
                            CommandError::new(
                                ErrorCode::Internal,
                                "the synthetic private source did not open",
                            )
                        })?;
                        if opened != PRIVATE_URL.as_bytes() {
                            return Err(CommandError::new(
                                ErrorCode::Conflict,
                                "another private source",
                            ));
                        }
                    }
                    _ => {
                        return Err(CommandError::new(
                            ErrorCode::Conflict,
                            "another native Skill archive source",
                        ));
                    }
                }
                let size = i64::try_from(self.archive.len())
                    .map_err(|_| CommandError::new(ErrorCode::TooLarge, "archive size"))?;
                if body.max_bytes < size {
                    return Err(CommandError::new(ErrorCode::TooLarge, "archive limit"));
                }
                let hash = Hash::of(&self.archive);
                self.blobs
                    .put(
                        &hash,
                        Some(size),
                        stream_of(vec![bytes::Bytes::copy_from_slice(&self.archive)]),
                    )
                    .await
                    .map_err(|_| CommandError::new(ErrorCode::Internal, "archive upload"))?;
                self.archives.fetch_add(1, Ordering::SeqCst);
                self.pause().await;
                Ok(CommandResult::NodeSkillFetch(NodeSkillFetchResult {
                    archive: BlobRef { hash, size },
                    commit: matches!(body.source, SkillFetchSource::Git { .. })
                        .then(|| "synthetic-commit".into()),
                }))
            }
            command => self.native.handle(context, command).await,
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
async fn native_skill_sources_keep_actual_environment_or_host_raw_origin_and_once_only_supplier_work()
-> Proof {
    for kind in [Kind::Environment, Kind::Git, Kind::Url] {
        for change in ["none", "scope_closed", "physical_source"] {
            let mut transport = Fixture::new(false).await?;
            let original = TestApp::new(
                transport.link.inner.db.clone(),
                transport.clock.clone(),
                Registry::new(),
            );
            let old = &original.app;
            let mut registry = Registry::new();
            crate::routes::mount(&mut registry);
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
            transport
                .link
                .set_secret_source(Arc::new(crate::dispatch::StoredSecrets::new(
                    transport.link.inner.db.clone(),
                    orch_db::secrets::MasterKey::new([7; 32]),
                )));
            let ca = orch_pki::ca::CertificateAuthority::create(
                "Synthetic Skill peer",
                365,
                &transport.clock,
            )?;
            let (leaf, certificate) = ca.issue_with_new_key(
                &orch_pki::ca::Holder::Machine(transport.machine),
                30,
                &transport.clock,
            )?;
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
            let host = new_id();
            resources::insert_host(
                &mut conn,
                &resources::NewHost {
                    id: host,
                    machine_id: transport.machine,
                    owner_id: Some(owner),
                    home_id: Some(home),
                    name: "Original native Skill host",
                },
                transport.clock.now(),
            )
            .await?;
            ownership::start_resource(
                &mut conn,
                ObjectRef {
                    kind: ObjectKind::Host,
                    id: host,
                },
                owner,
                ownership::NewAccess::OnlyMe,
                transport.clock.time(),
            )
            .await?;
            sqlx::query(
                "UPDATE machines SET roles=ARRAY['environment','node'],public_key=$2 WHERE id=$1",
            )
            .bind(transport.machine)
            .bind(orch_pki::sealing::RecipientKey::from_certificate(&certificate).to_text())
            .execute(&mut *conn)
            .await?;
            let (operation, from) = match kind {
                Kind::Environment => (
                    "SkillCreate.environment",
                    SkillSource::Environment {
                        environment_id: transport.environment,
                        path: "skill".into(),
                    },
                ),
                Kind::Git => (
                    "SkillCreate.git",
                    SkillSource::Git {
                        host_id: host,
                        repository: "https://example.invalid/native.git".into(),
                        reference: "main".into(),
                        folder: None,
                    },
                ),
                Kind::Url => (
                    "SkillCreate.url",
                    SkillSource::Url {
                        host_id: host,
                        url: PRIVATE_URL.into(),
                        folder: None,
                    },
                ),
            };
            let source = match kind {
                Kind::Environment => {
                    json!({"target":{"kind":"environment","id":transport.environment,"reach":"self"},"allow":[{"operation":operation,"actions":["read","manage"]}]})
                }
                _ => {
                    json!({"target":{"kind":"host","id":host,"reach":"self"},"allow":[{"operation":operation,"actions":["write"]}]})
                }
            };
            let scope: KeyScope = serde_json::from_value(
                json!({"rules":[{"target":{"kind":"org","id":home,"reach":"subtree"},"resources":["skill"],"allow":[{"operation":operation,"actions":["manage"]}]},source]}),
            )?;
            assert!(scope.validate().is_empty());
            let token = crate::middleware::authentication::new_token(
                crate::middleware::authentication::API_TOKEN_PREFIX,
            );
            let key = new_id();
            identity::create_api_token_scoped(
                &mut conn,
                &identity::NewApiToken {
                    id: key,
                    user_id: owner,
                    name: "Original native Skill source",
                    token_hash: &crate::middleware::authentication::token_hash(&token),
                    scopes: &[],
                    node_id: None,
                    at: transport.clock.now(),
                    expires_at: Some(transport.clock.now() + chrono::Duration::hours(1)),
                },
                Some(&json!(scope)),
            )
            .await?;
            drop(conn);
            tokio::fs::create_dir(transport._directory.path().join("skill")).await?;
            tokio::fs::write(transport._directory.path().join("skill/SKILL.md"), FILE).await?;
            let built = orch_engine::skills::archive::build(
                vec![orch_engine::skills::archive::File {
                    path: "SKILL.md".into(),
                    data: FILE.to_vec(),
                    executable: false,
                }],
                orch_engine::skills::archive::Limits {
                    max_bytes: 1024 * 1024,
                    max_files: 100,
                },
            )?;
            let peer = Arc::new(SkillPeer {
                native: transport.native.clone(),
                blobs: app.app.blobs.store(),
                key: leaf,
                archive: built.archive,
                hold: change != "none",
                reached: Notify::new(),
                release: Notify::new(),
                lists: AtomicUsize::new(0),
                files: AtomicUsize::new(0),
                archives: AtomicUsize::new(0),
            });
            transport.server = CommandServer::new(
                Journal::open(
                    &transport._directory.path().join("skill-journal"),
                    Duration::from_secs(24 * 3600),
                    transport.clock.time(),
                )?,
                Arc::new(transport.clock.clone()),
                ServerOptions {
                    roles: vec![Role::Environment, Role::Node],
                    connected: true,
                },
            );
            transport.server.serve(peer.clone());
            within(transport.connect(false)).await??;
            sqlx::query("UPDATE machines SET online=true,last_seen_at=$2 WHERE id=$1")
                .bind(transport.machine)
                .bind(transport.clock.now())
                .execute(&transport.test.pool)
                .await?;
            sqlx::query("UPDATE environments e SET attached_session=COALESCE(m.session_started_at,m.last_seen_at) FROM machines m WHERE e.id=$1 AND e.machine_id=m.id").bind(transport.environment).execute(&transport.test.pool).await?;
            let request = tokio::spawn(async move {
                let request = app
                    .call(SkillCreate, &[])
                    .token(&token)
                    .body(&SkillCreateBody {
                        home_id: home,
                        from,
                        access: None,
                    });
                request.send_bytes().await
            });
            if change != "none" {
                within(peer.reached.notified()).await?;
                if change == "scope_closed" {
                    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                        .bind(key)
                        .bind(json!({"rules":[]}))
                        .execute(&transport.test.pool)
                        .await?;
                } else {
                    match kind {
                        Kind::Environment => {
                            sqlx::query(
                                "UPDATE environments SET generation=generation+1 WHERE id=$1",
                            )
                            .bind(transport.environment)
                            .execute(&transport.test.pool)
                            .await?;
                        }
                        _ => {
                            let other = resources::testing::insert_machine(
                                &mut *transport.test.pool.acquire().await?,
                                "new-unrelated-skill-host",
                                "managed",
                            )
                            .await?;
                            sqlx::query("UPDATE hosts SET machine_id=$2 WHERE id=$1")
                                .bind(host)
                                .bind(other)
                                .execute(&transport.test.pool)
                                .await?;
                        }
                    }
                }
                peer.release.notify_one();
            }
            let reply = within(request).await??;
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM skills")
                .fetch_one(&transport.test.pool)
                .await?;
            if change == "none" {
                assert_eq!(
                    reply.status,
                    http::StatusCode::CREATED,
                    "{}",
                    String::from_utf8_lossy(&reply.raw)
                );
                let body: serde_json::Value = serde_json::from_slice(&reply.raw)?;
                assert_eq!(body["projection"], "receipt");
                assert!(body.get("name").is_none());
                assert_eq!(count, 1);
                let version: Id = serde_json::from_value(body["version_id"].clone())?;
                let created: i64 =
                    sqlx::query_scalar("SELECT count(*) FROM skill_versions WHERE id=$1")
                        .bind(version)
                        .fetch_one(&transport.test.pool)
                        .await?;
                assert_eq!(created, 1);
            } else {
                assert!(!reply.status.is_success());
                assert_eq!(count, 0);
            }
            match kind {
                Kind::Environment => {
                    assert_eq!(peer.lists.load(Ordering::SeqCst), 1);
                    assert_eq!(
                        peer.files.load(Ordering::SeqCst),
                        usize::from(change == "none")
                    );
                    assert_eq!(peer.archives.load(Ordering::SeqCst), 0);
                }
                _ => assert_eq!(peer.archives.load(Ordering::SeqCst), 1),
            }
            let saved:Vec<(bool,serde_json::Value,serde_json::Value)>=sqlx::query_as("SELECT authority_required,body,login_authority FROM commands WHERE machine_id=$1 AND kind IN ('fs.list','fs.fetch','node.skill.fetch')").bind(transport.machine).fetch_all(&transport.test.pool).await?;
            assert!(!saved.is_empty());
            assert!(saved.iter().all(|(required, _, _)| *required));
            assert!(
                saved
                    .iter()
                    .all(|(_, body, proof)| !body.to_string().contains(PRIVATE_URL)
                        && !proof.to_string().contains(PRIVATE_URL))
            );
            if matches!(kind, Kind::Url) {
                let secrets: i64 =
                    sqlx::query_scalar("SELECT count(*) FROM secrets WHERE owner_kind='skill'")
                        .fetch_one(&transport.test.pool)
                        .await?;
                assert_eq!(secrets, i64::from(change == "none"));
            }
            transport.close().await?;
        }
    }
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_native_skill_sources_keep_actual_environment_or_host_raw_origin_and_once_only_supplier_work()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(self::native_skill_sources_keep_actual_environment_or_host_raw_origin_and_once_only_supplier_work())
}
