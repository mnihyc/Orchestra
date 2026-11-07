//! Every native tree intent has the same real first-effect and durable replay boundary.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Case {
    area: Area,
    request: domain::Request,
    confirmation: Option<domain::MembershipStamp>,
    attached: Attached,
}
impl Case {
    async fn new(command: domain::Command, changed_version: bool) -> TestResult<Self> {
        let area = Area::new(true).await?;
        let folder = area.folder("source/").await?;
        let document = area
            .document("source/original.md", "Native original material")
            .await?;
        let destination = area.folder("destination/").await?;
        let mut request = request(
            command,
            match command {
                domain::Command::CreateFolder => Vec::new(),
                domain::Command::Rename | domain::Command::RestoreVersion => {
                    vec![domain::Item::Document(document)]
                }
                _ => vec![domain::Item::Container(folder)],
            },
            matches!(
                command,
                domain::Command::CreateFolder | domain::Command::Copy | domain::Command::Move
            )
            .then_some(domain::Directory::Container(destination)),
            &format!("native-staged-{command:?}"),
        );
        if matches!(
            command,
            domain::Command::CreateFolder | domain::Command::Rename
        ) {
            request.name = Some("Prepared native name".into());
        }
        if command == domain::Command::RestoreVersion {
            let mut tx = area.world.pool.begin().await?;
            let row = documents::lock(&mut tx, document)
                .await?
                .ok_or("native version source missing")?;
            request.version = Some(
                documents::history::cut(
                    &mut tx,
                    &row,
                    VersionCause::Restore,
                    area.world.clock.now(),
                )
                .await?,
            );
            tx.commit().await?;
        }
        let attached = attach(&area.world, document).await?;
        if command == domain::Command::RestoreVersion && !changed_version {
            let mut tx = area.world.pool.begin().await?;
            let row = documents::lock(&mut tx, document)
                .await?
                .ok_or("native no-change version source missing")?;
            request.version = Some(
                documents::history::cut(
                    &mut tx,
                    &row,
                    VersionCause::Restore,
                    area.world.clock.now(),
                )
                .await?,
            );
            tx.commit().await?;
        }
        if command == domain::Command::Restore {
            remove(
                &area.world,
                area.agent,
                domain::Item::Container(folder),
                "native-stage-retain",
            )
            .await?;
        }
        let confirmation = if command == domain::Command::Delete {
            let mut tx = area.world.pool.begin().await?;
            let plan = domain::plan_delete_in(
                &mut tx,
                domain::Actor::Agent { thread: area.agent },
                &request,
                Time::new(area.world.clock.now()),
                domain::Options::for_engine(&area.world.engine()),
            )
            .await?;
            tx.commit().await?;
            Some(plan.stamp)
        } else {
            None
        };
        Ok(Self {
            area,
            request,
            confirmation,
            attached: attached,
        })
    }
    async fn prepare(&self, tx: &mut orch_db::Tx) -> TestResult<domain::Operation> {
        let actor = domain::Actor::Agent {
            thread: self.area.agent,
        };
        let sources = domain::prepare_operation_sources_in(tx, actor, &self.request).await?;
        domain::require_operation_sources_in(
            tx,
            actor,
            &sources,
            Time::new(self.area.world.clock.now()),
        )
        .await?;
        Ok(domain::complete_operation_in(
            tx,
            actor,
            sources,
            self.confirmation
                .as_ref()
                .map(domain::DeletionConfirmation::Membership),
            Time::new(self.area.world.clock.now()),
            domain::Options::for_engine(&self.area.world.engine()),
        )
        .await?)
    }
}

#[tokio::test]
async fn each_native_tree_intent_rejects_at_its_first_actual_write_and_rolls_back_every_ledger()
-> TestResult {
    for command in [
        domain::Command::CreateFolder,
        domain::Command::Copy,
        domain::Command::Move,
        domain::Command::Rename,
        domain::Command::Delete,
        domain::Command::Restore,
        domain::Command::RestoreVersion,
    ] {
        let case = Case::new(command, true).await?;
        let area = &case.area;
        let before = state(&area.world, &[area.topic]).await?;
        let pins_before = pins(&area.world, case.attached.hash).await?;
        let mut tx = area.world.pool.begin().await?;
        let prepared = case.prepare(&mut tx).await?;
        assert_eq!(prepared.command(), command);
        assert!(!prepared.is_replay());
        let calls = AtomicUsize::new(0);
        let refused = domain::apply_operation_in(
            &mut tx,
            domain::Actor::Agent { thread: area.agent },
            prepared,
            Time::new(area.world.clock.now()),
            domain::Options::for_engine(&area.world.engine()),
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(orch_engine::EngineError::BeforeEffect(Box::new(
                    std::io::Error::other("stop at the actual native first effect"),
                )))
            },
        )
        .await;
        assert!(
            matches!(
                refused,
                Err(domain::Error::Engine(
                    orch_engine::EngineError::BeforeEffect(_)
                ))
            ),
            "{command:?}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1, "{command:?}");
        tx.commit().await?;
        assert_eq!(
            state(&area.world, &[area.topic]).await?,
            before,
            "{command:?}: no header, content, pin, history, page, audit or operation result may survive"
        );
        assert_eq!(pins(&area.world, case.attached.hash).await?, pins_before);
    }
    Ok(())
}

#[tokio::test]
async fn completed_native_tree_intents_replay_without_material_or_first_effect_and_keep_the_original_result()
-> TestResult {
    for command in [
        domain::Command::CreateFolder,
        domain::Command::Copy,
        domain::Command::Move,
        domain::Command::Rename,
        domain::Command::Delete,
        domain::Command::Restore,
        domain::Command::RestoreVersion,
    ] {
        let case = Case::new(command, true).await?;
        let area = &case.area;
        let calls = AtomicUsize::new(0);
        let mut tx = area.world.pool.begin().await?;
        let prepared = case.prepare(&mut tx).await?;
        let written = domain::apply_operation_in(
            &mut tx,
            domain::Actor::Agent { thread: area.agent },
            prepared,
            Time::new(area.world.clock.now()),
            domain::Options::for_engine(&area.world.engine()),
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .await?;
        tx.commit().await?;
        assert!(!written.entries.is_empty(), "{command:?}");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let completed = state(&area.world, &[area.topic]).await?;
        let pins_before = pins(&area.world, case.attached.hash).await?;
        let mut tx = area.world.pool.begin().await?;
        let replay = case.prepare(&mut tx).await?;
        assert!(replay.is_replay(), "{command:?}");
        let repeated = domain::apply_operation_in(
            &mut tx,
            domain::Actor::Agent { thread: area.agent },
            replay,
            Time::new(area.world.clock.now()),
            domain::Options::for_engine(&area.world.engine()),
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(orch_engine::EngineError::BeforeEffect(Box::new(
                    std::io::Error::other("replay cannot start another act"),
                )))
            },
        )
        .await?;
        tx.commit().await?;
        assert_eq!(written.operation, repeated.operation, "{command:?}");
        assert_eq!(written.entries, repeated.entries, "{command:?}");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "{command:?}");
        assert_eq!(
            state(&area.world, &[area.topic]).await?,
            completed,
            "{command:?}"
        );
        assert_eq!(pins(&area.world, case.attached.hash).await?, pins_before);
    }
    Ok(())
}

#[tokio::test]
async fn unchanged_version_tree_intents_check_before_their_real_audit_and_cannot_cross_actor_or_transaction()
-> TestResult {
    let case = Case::new(domain::Command::RestoreVersion, false).await?;
    let area = &case.area;
    let before = state(&area.world, &[area.topic]).await?;
    let calls = AtomicUsize::new(0);
    let mut tx = area.world.pool.begin().await?;
    let prepared = case.prepare(&mut tx).await?;
    let owner = orch_authz::Principal::load(
        &mut tx,
        orch_authz::Caller::session(area.world.ada),
        Time::new(area.world.clock.now()),
    )
    .await?;
    let wrong = domain::apply_operation_in(
        &mut tx,
        domain::Actor::Person(&owner),
        prepared,
        Time::new(area.world.clock.now()),
        domain::Options::for_engine(&area.world.engine()),
        || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    )
    .await;
    assert!(matches!(wrong, Err(domain::Error::Denied)));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let prepared = case.prepare(&mut tx).await?;
    tx.rollback().await?;
    let mut tx = area.world.pool.begin().await?;
    let wrong = domain::apply_operation_in(
        &mut tx,
        domain::Actor::Agent { thread: area.agent },
        prepared,
        Time::new(area.world.clock.now()),
        domain::Options::for_engine(&area.world.engine()),
        || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    )
    .await;
    assert!(matches!(wrong, Err(domain::Error::Stale)));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    tx.commit().await?;
    assert_eq!(state(&area.world, &[area.topic]).await?, before);
    let mut tx = area.world.pool.begin().await?;
    let prepared = case.prepare(&mut tx).await?;
    let refused = domain::apply_operation_in(
        &mut tx,
        domain::Actor::Agent { thread: area.agent },
        prepared,
        Time::new(area.world.clock.now()),
        domain::Options::for_engine(&area.world.engine()),
        || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(orch_engine::EngineError::BeforeEffect(Box::new(
                std::io::Error::other("unchanged material still cannot insert an unaccepted audit"),
            )))
        },
    )
    .await;
    assert!(matches!(
        refused,
        Err(domain::Error::Engine(
            orch_engine::EngineError::BeforeEffect(_)
        ))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    tx.commit().await?;
    assert_eq!(state(&area.world, &[area.topic]).await?, before);
    Ok(())
}
