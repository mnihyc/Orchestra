//! Native location endpoint, production durable dispatcher and file-backed volume journal.

use super::*;

use crate::registry::Registry;

use crate::state::AppState;

use crate::testing::TestApp;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyScope;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::mounts::MountKeptPurge;

use orch_api_contract::mounts::MountLocationDelete;

use orch_api_contract::mounts::MountLocationDeleteQuery;

use orch_api_contract::mounts::MountLocationSet;

use orch_api_contract::mounts::MountLocationSetBody;

use orch_authz::ownership;

use orch_db::queries::access;

use orch_db::queries::identity;

use orch_db::queries::resources::mounts;

use tokio::sync::Notify;
struct VolumePeer {
    native: Arc<Native>,
    mount: Id,
    reached: Notify,
    release: Notify,
    hold: bool,
    ensures: AtomicUsize,
    hold_remove: bool,
    removes: AtomicUsize,
}

#[async_trait::async_trait]
impl CommandHandler for VolumePeer {
    async fn handle(
        &self,
        context: CommandContext,
        command: Command,
    ) -> Result<CommandResult, CommandError> {
        match command {
            Command::NodeVolumeEnsure(body) => {
                if body.driver != "volume-fixture"
                    || body.name != format!("orch-mount-{}", self.mount)
                    || body.storage.as_deref() != Some("fast")
                    || body.labels.get("orch.mount_id") != Some(&self.mount.to_string())
                    || context.key != SerialKey::Volume(body.name.clone()).wire()
                {
                    return Err(CommandError::new(
                        ErrorCode::Conflict,
                        "another volume source",
                    ));
                }
                tokio::fs::create_dir(self.native._directory.path().join(&body.name))
                    .await
                    .map_err(|_| CommandError::new(ErrorCode::Conflict, "volume already exists"))?;
                self.ensures.fetch_add(1, Ordering::SeqCst);
                if self.hold {
                    self.reached.notify_one();
                    self.release.notified().await;
                }
                Ok(CommandResult::NodeVolumeEnsure(Empty {}))
            }
            Command::NodeVolumeRemove(body) => {
                if body.driver != "volume-fixture"
                    || body.name != format!("orch-mount-{}", self.mount)
                    || body.storage.as_deref() != Some("fast")
                    || body.labels.get("orch.mount_id") != Some(&self.mount.to_string())
                    || context.key != SerialKey::Volume(body.name.clone()).wire()
                {
                    return Err(CommandError::new(
                        ErrorCode::Conflict,
                        "another removed volume source",
                    ));
                }
                tokio::fs::remove_dir(self.native._directory.path().join(&body.name))
                    .await
                    .map_err(|_| {
                        CommandError::new(ErrorCode::NotFound, "the original volume is absent")
                    })?;
                self.removes.fetch_add(1, Ordering::SeqCst);
                if self.hold_remove {
                    self.reached.notify_one();
                    self.release.notified().await;
                }
                Ok(CommandResult::NodeVolumeRemove(Empty {}))
            }
            command => self.native.handle(context, command).await,
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
async fn mount_ensure_records_only_its_exact_accepted_volume_and_keeps_a_write_only_receipt_after_issuer_loss()
-> Proof {
    prove_volume_flow(After::Keep).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_mount_ensure_records_only_its_exact_accepted_volume_and_keeps_a_write_only_receipt_after_issuer_loss()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(self::mount_ensure_records_only_its_exact_accepted_volume_and_keeps_a_write_only_receipt_after_issuer_loss())
}

#[cfg(any(test, feature = "test-support"))]
async fn mount_delete_removes_only_its_accepted_original_volume_and_completes_bookkeeping_after_issuer_loss()
-> Proof {
    prove_volume_flow(After::Delete).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_mount_delete_removes_only_its_accepted_original_volume_and_completes_bookkeeping_after_issuer_loss()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(self::mount_delete_removes_only_its_accepted_original_volume_and_completes_bookkeeping_after_issuer_loss())
}

#[cfg(any(test, feature = "test-support"))]
async fn kept_purge_uses_the_exact_retained_row_and_keeps_a_current_location_on_another_machine()
-> Proof {
    prove_volume_flow(After::Purge).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_kept_purge_uses_the_exact_retained_row_and_keeps_a_current_location_on_another_machine()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(self::kept_purge_uses_the_exact_retained_row_and_keeps_a_current_location_on_another_machine())
}

#[derive(Clone, Copy)]
enum After {
    Keep,
    Delete,
    Purge,
}

async fn prove_volume_flow(after: After) -> Proof {
    let delete = !matches!(after, After::Keep);
    for change in ["none", "scope_closed", "raw_rotated"] {
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
        let mut conn = transport.test.pool.acquire().await?;
        let (owner, home): (Id, Id) =
            sqlx::query_as("SELECT owner_id,home_node_id FROM environments WHERE id=$1")
                .bind(transport.environment)
                .fetch_one(&mut *conn)
                .await?;
        access::testing::insert_member(&mut conn, home, owner, home).await?;
        access::testing::name_owner(&mut conn, home, ("user", owner), transport.clock.now())
            .await?;
        sqlx::query(r#"UPDATE machines SET owner_id=$2,facts=jsonb_build_object('environment_descriptor',jsonb_build_object('name','volume-fixture','data_storage',jsonb_build_object('made_by_orch','volume')),'storages','[{"name":"fast"}]'::jsonb) WHERE id=$1"#).bind(transport.machine).bind(owner).execute(&mut *conn).await?;
        resources::routes::machine_home(&mut conn, transport.machine, Some(home)).await?;
        ownership::start_resource(
            &mut conn,
            ObjectRef {
                kind: ObjectKind::Machine,
                id: transport.machine,
            },
            owner,
            ownership::NewAccess::OnlyMe,
            transport.clock.time(),
        )
        .await?;
        let mount = new_id();
        mounts::insert(
            &mut conn,
            &mounts::MountRow {
                id: mount,
                name: "native-volume".into(),
                description: String::new(),
                org_id: Some(home),
                home_id: Some(home),
                owner_id: Some(owner),
                target: "/data/native-volume".into(),
                read_only: true,
                enabled: true,
                default_fate: "as_is".into(),
                revision: 1,
            },
            transport.clock.now(),
        )
        .await?;
        ownership::start_resource(
            &mut conn,
            ObjectRef {
                kind: ObjectKind::Mount,
                id: mount,
            },
            owner,
            ownership::NewAccess::OnlyMe,
            transport.clock.time(),
        )
        .await?;
        let policy: KeyScope = serde_json::from_value(
            json!({"rules":[{"target":{"kind":"mount","id":mount,"reach":"self"},"allow":[{"operation":"MountLocationSet","actions":["write"]}]},{"target":{"kind":"machine","id":transport.machine,"reach":"self"},"allow":[{"operation":"MountLocationSet","actions":["manage"]}]}]}),
        )?;
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
                name: "Native mount ensure",
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
        let peer = Arc::new(VolumePeer {
            native: transport.native.clone(),
            mount,
            reached: Notify::new(),
            release: Notify::new(),
            hold: !delete && change != "none",
            ensures: AtomicUsize::new(0),
            hold_remove: delete && change != "none",
            removes: AtomicUsize::new(0),
        });
        transport.server = CommandServer::new(
            Journal::open(
                &transport._directory.path().join("volume-journal"),
                Duration::from_secs(24 * 3600),
                transport.clock.time(),
            )?,
            Arc::new(transport.clock.clone()),
            ServerOptions {
                roles: vec![Role::Environment],
                connected: true,
            },
        );
        transport.server.serve(peer.clone());
        within(transport.connect(false)).await??;
        let machine = transport.machine;
        let request_app = app.clone();
        let secret = token.clone();
        let request = tokio::spawn(async move {
            let request = request_app
                .call(MountLocationSet, &[&mount, &machine])
                .token(&secret)
                .body(&MountLocationSetBody::Volume {
                    storage: Some("fast".into()),
                });
            request.send_bytes().await
        });
        if !delete && change != "none" {
            within(peer.reached.notified()).await?;
            if change == "scope_closed" {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(id)
                    .bind(json!({"rules":[]}))
                    .execute(&transport.test.pool)
                    .await?;
            } else {
                sqlx::query("UPDATE api_tokens SET token_hash=$2 WHERE id=$1")
                    .bind(id)
                    .bind(crate::middleware::authentication::token_hash(
                        "synthetic replacement bearer",
                    ))
                    .execute(&transport.test.pool)
                    .await?;
            }
            peer.release.notify_one();
        }
        let reply = within(request).await??;
        assert_eq!(
            reply.status,
            http::StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&reply.raw)?,
            json!({"projection":"receipt","mount_id":mount,"machine_id":machine})
        );
        assert_eq!(peer.ensures.load(Ordering::SeqCst), 1);
        assert!(
            transport
                ._directory
                .path()
                .join(format!("orch-mount-{mount}"))
                .is_dir()
        );
        let current =
            mounts::locations(&mut *transport.test.pool.acquire().await?, &[mount]).await?;
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].machine_id, machine);
        assert_eq!(current[0].source, format!("orch-mount-{mount}"));
        let commands:Vec<(Id,bool)>=sqlx::query_as("SELECT id,authority_required FROM commands WHERE machine_id=$1 AND kind='node.volume.ensure'").bind(machine).fetch_all(&transport.test.pool).await?;
        assert_eq!(commands.len(), 1);
        assert!(commands[0].1);
        if delete {
            let kept = if matches!(after, After::Purge) {
                let kept = new_id();
                let location = &current[0];
                let mut conn = transport.test.pool.acquire().await?;
                mounts::keep(
                    &mut conn,
                    &mounts::KeptRow {
                        id: kept,
                        mount_id: mount,
                        machine_id: machine,
                        kind: location.kind.clone(),
                        source: location.source.clone(),
                        storage: location.storage.clone(),
                        data_kind: location.data_kind.clone(),
                        file_count: None,
                        total_bytes: None,
                        job_id: None,
                        kept_by: owner,
                        kept_at: transport.clock.now(),
                    },
                )
                .await?;
                mounts::remove_location(&mut conn, mount, machine).await?;
                let other = resources::testing::insert_machine(
                    &mut conn,
                    "unrelated-current-machine",
                    "connected",
                )
                .await?;
                mounts::set_location(
                    &mut conn,
                    &mounts::LocationRow {
                        mount_id: mount,
                        machine_id: other,
                        host_id: None,
                        host_name: None,
                        kind: "path".into(),
                        source: "/data/current-unrelated-location".into(),
                        storage: None,
                        data_kind: None,
                        created_by: owner,
                        created_at: transport.clock.now(),
                    },
                )
                .await?;
                let wrong: KeyScope = serde_json::from_value(
                    json!({"rules":[{"target":{"kind":"mount","id":mount,"reach":"self"},"allow":[{"operation":"MountKeptPurge","actions":["write"]}]},{"target":{"kind":"machine","id":other,"reach":"self"},"allow":[{"operation":"MountKeptPurge","actions":["manage"]}]}]}),
                )?;
                assert!(wrong.validate().is_empty());
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(id)
                    .bind(json!(wrong))
                    .execute(&mut *conn)
                    .await?;
                drop(conn);
                let request = app
                    .call(MountKeptPurge, &[&mount, &kept])
                    .token(&token)
                    .query(&MountLocationDeleteQuery {
                        confirm: Some("native-volume".into()),
                    });
                assert_eq!(
                    request.send_bytes().await.status,
                    http::StatusCode::FORBIDDEN
                );
                assert_eq!(peer.removes.load(Ordering::SeqCst), 0);
                Some(kept)
            } else {
                None
            };
            let operation = if kept.is_some() {
                "MountKeptPurge"
            } else {
                "MountLocationDelete"
            };
            let scope: KeyScope = serde_json::from_value(
                json!({"rules":[{"target":{"kind":"mount","id":mount,"reach":"self"},"allow":[{"operation":operation,"actions":["write"]}]},{"target":{"kind":"machine","id":machine,"reach":"self"},"allow":[{"operation":operation,"actions":["manage"]}]}]}),
            )?;
            assert!(scope.validate().is_empty());
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(id)
                .bind(json!(scope))
                .execute(&transport.test.pool)
                .await?;
            let request = tokio::spawn(async move {
                if let Some(kept) = kept {
                    let request = app
                        .call(MountKeptPurge, &[&mount, &kept])
                        .token(&token)
                        .query(&MountLocationDeleteQuery {
                            confirm: Some("native-volume".into()),
                        });
                    request.send_bytes().await
                } else {
                    let request = app
                        .call(MountLocationDelete, &[&mount, &machine])
                        .token(&token)
                        .query(&MountLocationDeleteQuery {
                            confirm: Some("native-volume".into()),
                        });
                    request.send_bytes().await
                }
            });
            if change != "none" {
                within(peer.reached.notified()).await?;
                if change == "scope_closed" {
                    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                        .bind(id)
                        .bind(json!({"rules":[]}))
                        .execute(&transport.test.pool)
                        .await?;
                } else {
                    sqlx::query("UPDATE api_tokens SET token_hash=$2 WHERE id=$1")
                        .bind(id)
                        .bind(crate::middleware::authentication::token_hash(
                            "synthetic replacement bearer",
                        ))
                        .execute(&transport.test.pool)
                        .await?;
                }
                peer.release.notify_one();
            }
            let reply = within(request).await??;
            assert_eq!(
                reply.status,
                http::StatusCode::NO_CONTENT,
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert!(reply.raw.is_empty());
            assert_eq!(peer.ensures.load(Ordering::SeqCst), 1);
            assert_eq!(peer.removes.load(Ordering::SeqCst), 1);
            assert!(
                !transport
                    ._directory
                    .path()
                    .join(format!("orch-mount-{mount}"))
                    .exists()
            );
            let remaining =
                mounts::locations(&mut *transport.test.pool.acquire().await?, &[mount]).await?;
            if kept.is_some() {
                assert_eq!(remaining.len(), 1);
                assert_ne!(remaining[0].machine_id, machine);
                assert_eq!(remaining[0].source, "/data/current-unrelated-location");
                assert!(
                    mounts::kept(&mut *transport.test.pool.acquire().await?, &[mount])
                        .await?
                        .is_empty()
                );
            } else {
                assert!(remaining.is_empty());
            }
            let commands:Vec<(Id,bool)>=sqlx::query_as("SELECT id,authority_required FROM commands WHERE machine_id=$1 AND kind='node.volume.remove'").bind(machine).fetch_all(&transport.test.pool).await?;
            assert_eq!(commands.len(), 1);
            assert!(commands[0].1);
        }
        transport.close().await?;
    }
    Ok(())
}
