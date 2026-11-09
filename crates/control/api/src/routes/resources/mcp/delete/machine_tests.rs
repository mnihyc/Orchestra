//! Native E-entry controls over the real Delete producer, canonical import and custody writer.
//! Authenticated CLI/link routing and physical-copy end-to-end proofs belong to that consumer.
use super::*;
use crate::{places::person, support::World};
use orch_authz::{CapturedOperation, ownership};
use orch_db::queries::{link, resources, transaction_guard as authority_rows};
use orch_engine::commands::CommandAuthority;
use orch_types::new_id;
use std::time::Duration;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
struct Fixture {
    world: World,
    owner: Id,
    machine: Id,
    server: Id,
}
impl Fixture {
    async fn new(imported: bool, here: bool) -> TestResult<Self> {
        let world = World::new().await;
        let by = person(&world, "native-machine-owner").await;
        let mut conn = world.conn().await;
        let machine =
            resources::testing::insert_machine(&mut conn, "Actual native requester", "connected")
                .await?;
        sqlx::query(
            "UPDATE machines SET owner_id=$2,online=true,roles=ARRAY['environment'] WHERE id=$1",
        )
        .bind(machine)
        .bind(by.id())
        .execute(&mut *conn)
        .await?;
        let copy_machine = if here {
            machine
        } else {
            let other =
                resources::testing::insert_machine(&mut conn, "Other native offering", "connected")
                    .await?;
            sqlx::query("UPDATE machines SET owner_id=$2,online=true WHERE id=$1")
                .bind(other)
                .bind(by.id())
                .execute(&mut *conn)
                .await?;
            other
        };
        let server = new_id();
        let source = json!({"source":"owned-native-source","scope":"user","name":"native-tool","digest":"actual-recorded-copy"});
        rows::insert(
            &mut conn,
            &McpRow {
                imported,
                id: server,
                name: "native-tool".into(),
                description: "Native E-entry fixture".into(),
                org_id: None,
                home_id: None,
                owner_id: Some(by.id()),
                machine_id: imported.then_some(copy_machine),
                environment_id: None,
                spec: if imported {
                    json!({"transport":"stdio","imported":source})
                } else {
                    json!({"transport":"stdio","command":["native-tool"]})
                },
                enabled: true,
                revision: 1,
            },
            world.now(),
        )
        .await?;
        ownership::start_resource(
            &mut conn,
            ObjectRef {
                kind: ResourceKind::McpServer.into(),
                id: server,
            },
            by.id(),
            ownership::NewAccess::OnlyMe,
            Time::new(world.now()),
        )
        .await?;
        // The source is system-scoped; ordinary Manage is explicit, not inferred from owner_id.
        orch_db::queries::access::testing::add_entry(
            &mut conn,
            &orch_db::queries::access::PageKey::new("mcp_server", server, "live"),
            "any",
            "mcp.manage",
            ("user", Some(by.id()), None),
            None,
            world.now(),
        )
        .await?;
        if imported {
            rows::offer(&mut conn, server, copy_machine, None, &source, world.now()).await?;
        }
        drop(conn);
        Ok(Self {
            world,
            owner: by.id(),
            machine,
            server,
        })
    }
    async fn apply(&self) -> ApiResult<effects::Completed<Output>> {
        let tx = self.world.app.app.db.begin().await?;
        effects::apply_owned_from_machine::<Delete>(
            &self.world.app.app,
            tx,
            self.machine,
            self.server,
            new_id(),
            (),
            (),
        )
        .await
    }
    async fn live(&self) -> TestResult<bool> {
        Ok(
            sqlx::query_scalar("SELECT deleted_at IS NULL FROM mcp_servers WHERE id=$1")
                .bind(self.server)
                .fetch_one(&self.world.db.pool)
                .await?,
        )
    }
    async fn commands(&self) -> TestResult<i64> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM commands WHERE kind='mcp.forget' AND body->>'server_id'=$1",
        )
        .bind(self.server.to_string())
        .fetch_one(&self.world.db.pool)
        .await?)
    }
}
#[tokio::test]
async fn native_machine_deletion_uses_real_canonical_facets_and_queues_owned_custody_on_the_same_commit()
-> TestResult {
    let s = Fixture::new(true, true).await?;
    let done = s.apply().await?;
    assert_eq!(done.accepted.native_machine(), Some(s.machine));
    assert_eq!(done.accepted.user(), s.owner);
    assert_eq!(done.accepted.admitted_operation(), Some("McpServerDelete"));
    assert!(!done.accepted.as_admin());
    assert!(
        done.accepted
            .grant()
            .is_some_and(|grant| grant.token_ids().next().is_none())
    );
    assert!(!s.live().await?);
    assert_eq!(s.commands().await?, 1);
    let command = done
        .output
        .commands
        .first()
        .ok_or("the native deletion did not journal its actual copy")?;
    let mut conn = s.world.conn().await;
    let saved = link::command_with_authority(&mut conn, command.id)
        .await?
        .ok_or("canonical native cleanup missing")?;
    let metadata = saved.authority.clone().ok_or("native custody missing")?;
    assert!(!metadata.to_string().contains("session"));
    let custody: CommandAuthority = serde_json::from_value(metadata)?;
    // Provenance can be audited, but cannot become an owner lease for a fresh transaction.
    let captured = CapturedOperation::capture(&done.accepted)?;
    let mut tx = s.world.app.app.db.begin().await?;
    assert!(
        captured
            .restore_current(&mut tx, s.world.app.app.clock.as_ref())
            .await?
            .is_none()
    );
    let other = crate::places::person(&s.world, "new-machine-owner").await;
    sqlx::query("UPDATE machines SET owner_id=$2 WHERE id=$1")
        .bind(s.machine)
        .bind(other.id())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let mut tx = s.world.app.app.db.begin().await?;
    assert!(
        custody
            .current_callers(
                &mut tx,
                &saved.command,
                s.world.app.app.clock.as_ref(),
                &crate::thread_access::AuthzAccess::new(s.world.app.app.clock.clone())
            )
            .await?
            .is_some(),
        "actual accepted deletion cleanup stays independent of the invoker after commit"
    );
    tx.commit().await?;
    Ok(())
}
#[tokio::test]
async fn native_machine_deletion_refuses_presets_and_another_machines_canonical_import()
-> TestResult {
    for (imported, here) in [(false, true), (true, false)] {
        let s = Fixture::new(imported, here).await?;
        assert!(s.apply().await.is_err());
        assert!(s.live().await?);
        assert_eq!(s.commands().await?, 0);
    }
    Ok(())
}

#[tokio::test]
async fn a_retained_ready_copy_does_not_let_its_machine_forget_another_machines_canonical_import()
-> TestResult {
    let s = Fixture::new(true, false).await?;
    let mut conn = s.world.conn().await;
    // A retained copy is real, but its machine is not the import's canonical source.
    let source = rows::servers(&mut conn, &[s.server])
        .await?
        .pop()
        .ok_or("canonical import missing")?
        .spec["imported"]
        .clone();
    rows::offer(&mut conn, s.server, s.machine, None, &source, s.world.now()).await?;
    drop(conn);
    let refused = s
        .apply()
        .await
        .err()
        .ok_or("a retained copy was admitted as the origin")?;
    assert_eq!(refused.code(), ErrorCode::AuthzDenied);
    assert!(s.live().await?);
    assert_eq!(s.commands().await?, 0);
    Ok(())
}
#[tokio::test]
async fn native_machine_deletion_reloads_current_owner_after_the_actual_authority_wait()
-> TestResult {
    for changed in [false, true] {
        let s = Fixture::new(true, true).await?;
        let other = person(&s.world, "replacement-owner").await;
        let mut held = s.world.app.app.db.begin().await?;
        authority_rows::lock_authority(&mut held).await?;
        if changed {
            sqlx::query("UPDATE machines SET owner_id=$2 WHERE id=$1")
                .bind(s.machine)
                .bind(other.id())
                .execute(&mut *held)
                .await?;
        }
        let app = s.world.app.app.clone();
        let machine = s.machine;
        let server = s.server;
        let (identified, backend) = tokio::sync::oneshot::channel();
        let work = tokio::spawn(async move {
            let mut tx = app.db.begin().await?;
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *tx)
                .await
                .map_err(DbError::from)?;
            let _ = identified.send(pid);
            effects::apply_owned_from_machine::<Delete>(&app, tx, machine, server, new_id(), (), ())
                .await
        });
        let backend = tokio::time::timeout(Duration::from_secs(30), backend).await??;
        tokio::time::timeout(Duration::from_secs(30),async {
            loop {
                let waited:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory' AND pid=$1)").bind(backend).fetch_one(&s.world.db.pool).await?;
                if waited {break;}
                tokio::task::yield_now().await;
            }
            Ok::<(),orch_db::DbError>(())
        }).await??;
        held.commit().await?;
        let result = tokio::time::timeout(Duration::from_secs(30), work).await??;
        assert_eq!(result.is_ok(), !changed);
        assert_eq!(s.live().await?, changed);
        assert_eq!(s.commands().await?, i64::from(!changed));
    }
    Ok(())
}
