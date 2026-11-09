//! Provider-owner placement eligibility stays independent from the accepted requester's origins.
use super::*;
use crate::commands::{Commands, Outcome, Stage};
use orch_authz::{Principal, usable::ResourcePolicy};
use orch_db::audit::Origin;
use orch_db::queries::access;

const ORDINARY_COMMANDS: &str = "provider_recreate_commands";

struct ReleaseImage(std::sync::Arc<orch_testkit::doubles::machine::ScriptedMachine>);
impl Drop for ReleaseImage {
    fn drop(&mut self) {
        self.0.handler.release();
    }
}

struct BorrowedHost {
    fixture: Fixture,
    scoped: bool,
    host: Id,
    host_owner: Id,
    owner_use_entry: Id,
}

impl BorrowedHost {
    async fn new(scoped: bool) -> TestResult<Self> {
        let fixture = if scoped {
            Fixture::new().await?
        } else {
            Fixture::ordinary().await?
        };
        let host_owner = fixture.world.person("other-host-owner").await;
        let mut tx = fixture.world.pool.begin().await?;
        let candidate = envs::runtime(&mut tx, fixture.candidate)
            .await?
            .ok_or("candidate absent")?;
        let host = candidate.host_id;
        let principal = Principal::load(
            &mut tx,
            Caller::session(fixture.world.ada).with_admin_mode(false),
            Time::new(fixture.world.clock.now()),
        )
        .await?;
        // Use the actual ownership writer: first owner and live Owners entries move
        // together. The provider, requester, keys and accepted runtime recipe do not move.
        let changed = ownership::transfer(
            &mut tx,
            &principal,
            ObjectRef {
                kind: ObjectKind::Host,
                id: host,
            },
            host_owner,
            false,
            &ResourcePolicy::default(),
            &Origin::default(),
        )
        .await
        .map_err(|error| format!("ordinary Host transfer failed: {error:?}"))?;
        assert!(changed.changed);
        assert!(
            !orch_authz::resources::check(
                &mut tx,
                &principal,
                ObjectRef {
                    kind: ObjectKind::Host,
                    id: host
                },
                Permission::HostUse,
                orch_authz::Policy::default(),
            )
            .await?
            .is_allowed(),
            "the former owner has no implicit Host Use after ordinary transfer"
        );
        let owner_use_entry = access::testing::add_entry(
            &mut tx,
            &access::PageKey::new("host", host, "live"),
            "any",
            "host.use",
            ("user", Some(fixture.world.ada), None),
            None,
            fixture.world.clock.now(),
        )
        .await?;
        orch_authz::epoch::bump(&mut tx).await?;
        tx.commit().await?;
        let result = Self {
            fixture,
            scoped,
            host,
            host_owner,
            owner_use_entry,
        };
        result.independent_rights(true).await?;
        Ok(result)
    }

    fn journal_field(&self) -> &'static str {
        if self.scoped {
            COMMANDS
        } else {
            ORDINARY_COMMANDS
        }
    }

    async fn independent_rights(&self, expected_host_use: bool) -> TestResult {
        let s = &self.fixture;
        let mut tx = s.world.pool.begin().await?;
        let provider = providers::state(&mut tx, s.world.provider)
            .await?
            .ok_or("provider absent")?;
        assert_eq!(provider.owner_id, s.world.ada);
        let host = hosts::host(&mut tx, self.host)
            .await?
            .ok_or("host absent")?;
        assert_eq!(host.owner_id, Some(self.host_owner));
        let owner = identity::user_by_id(&mut tx, self.host_owner)
            .await?
            .ok_or("host owner absent")?;
        assert_eq!(owner.status, "active");
        let row = jobs::job(&mut tx, s.job.id()).await?.ok_or("job absent")?;
        let recipe: Recipe = serde_json::from_value(row.plan.clone())?;
        assert_eq!(recipe.by, s.world.ada);
        if self.scoped {
            assert_eq!(row.plan[ORIGINS], json!(s.origins));
        } else {
            assert!(s.origins.is_empty());
            assert!(s.policies.is_empty());
            assert_eq!(row.batch_id, None);
            for field in [ORIGINS, BINDING, COMMANDS, NOTICES] {
                assert!(
                    row.plan.get(field).is_none(),
                    "ordinary jobs never acquire scoped metadata {field}"
                );
            }
        }
        let old = envs::runtime(&mut tx, s.old)
            .await?
            .ok_or("old runtime absent")?;
        let mut actors = Vec::new();
        for origin in &s.origins {
            actors.push(
                restore_origin(
                    &mut tx,
                    origin,
                    s.world.provider,
                    Time::new(s.world.clock.now()),
                )
                .await?
                .ok_or("the original requester operation lost its ordinary Provider Manage")?,
            );
        }
        if !self.scoped {
            // This fixture's actual ordinary requester is recipe.by throughout;
            // no accepted key origin is erased or replaced by another identity.
            actors.push(
                Principal::load(
                    &mut tx,
                    Caller::session(s.world.ada).with_admin_mode(false),
                    Time::new(s.world.clock.now()),
                )
                .await?,
            );
        }
        assert_eq!(actors.len(), if self.scoped { 2 } else { 1 });
        for actor in actors {
            assert_eq!(actor.user(), s.world.ada);
            assert!(crate::updates::may_manage(&mut tx, &actor, recipe.target).await?);
            assert!(
                crate::updates::template_usable(&mut tx, &actor, recipe.template, old.template_id,)
                    .await?
            );
            let object = ObjectRef {
                kind: ObjectKind::Host,
                id: self.host,
            };
            assert_eq!(
                orch_authz::resources::check(
                    &mut tx,
                    &actor,
                    object,
                    Permission::HostUse,
                    orch_authz::Policy::default(),
                )
                .await?
                .is_allowed(),
                expected_host_use
            );
            assert!(
                !orch_authz::resources::check(
                    &mut tx,
                    &actor,
                    object,
                    Permission::HostManage,
                    orch_authz::Policy::default(),
                )
                .await?
                .is_allowed(),
                "Host Manage is not donated by Provider Manage or Host Use"
            );
        }
        tx.rollback().await?;
        Ok(())
    }

    async fn withdraw(&self) -> TestResult {
        let s = &self.fixture;
        let mut tx = s.world.pool.begin().await?;
        assert!(
            access::remove_entry(
                &mut tx,
                self.owner_use_entry,
                self.host_owner,
                s.world.clock.now(),
            )
            .await?
        );
        orch_authz::epoch::bump(&mut tx).await?;
        tx.commit().await?;
        self.independent_rights(false).await
    }

    async fn restore(&mut self) -> TestResult {
        let s = &self.fixture;
        let mut tx = s.world.pool.begin().await?;
        self.owner_use_entry = access::testing::add_entry(
            &mut tx,
            &access::PageKey::new("host", self.host, "live"),
            "any",
            "host.use",
            ("user", Some(s.world.ada), None),
            None,
            s.world.clock.now(),
        )
        .await?;
        orch_authz::epoch::bump(&mut tx).await?;
        tx.commit().await?;
        self.independent_rights(true).await
    }
}

#[tokio::test]
async fn queued_member_requires_only_its_actual_owners_current_use_of_the_selected_host()
-> TestResult {
    queued_owner_use(true).await
}

#[tokio::test]
async fn ordinary_queued_provider_requires_its_owners_current_use_of_the_selected_host()
-> TestResult {
    queued_owner_use(false).await
}

async fn queued_owner_use(scoped: bool) -> TestResult {
    let mut b = BorrowedHost::new(scoped).await?;
    let journal_field = b.journal_field();
    let image = b.fixture.image()?;
    b.fixture.reserve(&image).await?;
    let before = b.fixture.job.row().await?.plan;
    assert!(b.fixture.current().await?);
    assert!(b.fixture.allows(&image).await?);
    b.withdraw().await?;
    assert!(
        !b.fixture.current().await?,
        "requester rights cannot replace provider-owner Host Use"
    );
    assert!(
        !b.fixture.allows(&image).await?,
        "a queued frame rechecks the exact selected host"
    );
    let refused = b.fixture.job.command(image.clone()).await;
    assert!(matches!(refused,Err(Stop::Failed(ref failure)) if failure.code=="access.denied"));
    assert_eq!(b.fixture.world.commands.issued_of("node.image.ensure"), 0);
    let denied = b.fixture.job.row().await?.plan;
    for field in [ORIGINS, BINDING, COMMANDS, NOTICES, ORDINARY_COMMANDS] {
        assert_eq!(
            denied.get(field),
            before.get(field),
            "withdrawal must not rewrite {field}"
        );
    }
    b.restore().await?;
    assert!(b.fixture.current().await?);
    assert!(
        b.fixture.allows(&image).await?,
        "the same canonical pending frame is valid again"
    );
    let restored = b.fixture.job.row().await?.plan;
    for field in [ORIGINS, BINDING, COMMANDS, NOTICES, ORDINARY_COMMANDS] {
        assert_eq!(restored.get(field), before.get(field));
    }
    assert!(
        restored[journal_field].get(image.id.to_string()).is_some(),
        "the ordinary and scoped paths retain their own canonical command journal"
    );
    assert_eq!(b.fixture.world.commands.issued_of("node.image.ensure"), 0);
    b.fixture.engine.stop();
    Ok(())
}

#[tokio::test]
async fn owner_use_withdrawal_keeps_the_exact_accepted_receipt_and_owned_candidate_cleanup()
-> TestResult {
    accepted_owner_use(true).await
}

#[tokio::test]
async fn ordinary_owner_use_withdrawal_keeps_the_exact_accepted_receipt_and_owned_candidate_cleanup()
-> TestResult {
    accepted_owner_use(false).await
}

async fn accepted_owner_use(scoped: bool) -> TestResult {
    let b = BorrowedHost::new(scoped).await?;
    let journal_field = b.journal_field();
    let s = &b.fixture;
    let image = s.image()?;
    s.reserve(&image).await?;
    let original_journal = s.job.row().await?.plan[journal_field].clone();
    s.target_host
        .machine
        .handler
        .hold(CommandKind::NodeImageEnsure);
    let _release = ReleaseImage(std::sync::Arc::clone(&s.target_host.machine));
    s.target_host
        .machine
        .handler
        .on(CommandKind::NodeImageEnsure, |_| {
            Ok(CommandResult::NodeImageEnsure(NodeImageEnsureResult {
                digest: Some("accepted-before-owner-use-withdrawal".into()),
                built: None,
                programs: std::collections::BTreeMap::new(),
            }))
        });
    let transport = s.world.commands.as_engine();
    // The original journal was admitted above; this is the fixture's real Node issuer.
    Commands::issue(transport.as_ref(), image.clone()).await?;
    crate::test_common::eventually(
        "the original image has a real native acknowledgement",
        || async {
            Commands::status(transport.as_ref(), image.id)
                .await
                .is_ok_and(|info| info.is_some_and(|info| info.stage == Stage::Accepted))
        },
    )
    .await;
    s.target_host.machine.handler.started(image.id).await;
    b.withdraw().await?;
    let current = s.current().await?;
    let wire = s.allows(&image).await?;
    // Drain the actual accepted operation even if a following eligibility assertion fails.
    s.target_host.machine.handler.release();
    let native = Commands::wait(transport.as_ref(), image.id).await?;
    assert!(
        !current && !wire,
        "withdrawn owner Use refuses only new forward decisions"
    );
    let Outcome::Succeeded(native) = native else {
        return Err("the accepted image did not succeed".into());
    };
    let replay = s
        .job
        .command(image.clone())
        .await
        .map_err(|error| format!("{error:?}"))?;
    assert_eq!(replay, native);
    assert_eq!(
        replay["digest"],
        json!("accepted-before-owner-use-withdrawal")
    );
    assert_eq!(s.target_host.machine.handler.runs_of(image.id), 1);
    assert_eq!(s.world.commands.issued_of("node.image.ensure"), 1);
    assert!(original_journal.get(image.id.to_string()).is_some());
    assert_eq!(s.job.row().await?.plan[journal_field], original_journal);
    assert!(
        matches!(s.job.command(s.old_stop()?).await,Err(Stop::Failed(ref failure)) if failure.code=="access.denied")
    );
    assert_eq!(s.world.commands.issued_of("node.runtime.stop"), 0);

    let why = Stop::Failed(Failure::new(
        "access.denied",
        "The provider owner lost destination Host Use.",
    ));
    s.job
        .begin_undo(&why)
        .await?
        .ok_or("owned cleanup claim absent")?;
    s.target_host
        .machine
        .handler
        .on(CommandKind::NodeRuntimeRemove, |_| {
            Ok(CommandResult::NodeRuntimeRemove(Empty {}))
        });
    s.old_host
        .machine
        .handler
        .on(CommandKind::NodeRuntimeStart, |_| {
            Ok(CommandResult::NodeRuntimeStart(Empty {}))
        });
    let recreate = Recreating::of(&s.job.row().await?)?;
    assert_eq!(
        recreate
            .rollback(&s.job)
            .await
            .map_err(|error| format!("{error:?}"))?,
        None
    );
    let removed = s.world.commands.issued_commands("node.runtime.remove");
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].0, s.target_host.id);
    assert_eq!(removed[0].1.body["runtime_id"], json!(s.candidate));
    let resumed = s.world.commands.issued_commands("node.runtime.start");
    assert_eq!(resumed.len(), 1);
    assert_eq!(resumed[0].0, s.old_host.id);
    assert_eq!(resumed[0].1.body["runtime_id"], json!(s.old));
    let mut conn = s.world.pool.acquire().await?;
    assert_eq!(
        providers::live_runtime(&mut conn, s.world.provider)
            .await?
            .ok_or("old runtime absent")?
            .id,
        s.old
    );
    assert_eq!(
        envs::runtime(&mut conn, s.candidate)
            .await?
            .ok_or("candidate absent")?
            .state,
        "removed"
    );
    assert_eq!(
        providers::state(&mut conn, s.world.provider)
            .await?
            .ok_or("provider absent")?
            .owner_id,
        s.world.ada
    );
    s.engine.stop();
    Ok(())
}
