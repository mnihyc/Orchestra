//! Existing file jobs retain their actual publication operation through queueing and native ACK.
//! The native supplier is the ordinary scripted Node link; no test fabricates Accepted/Over.

use super::*;
use crate::commands::{Commands, CommandsError, Info, Outcome, RetainedCommand, Stage};
use crate::operation_scope::targets;
use crate::{EngineParts, test_common as common};
use orch_access_contract::key_scope::{KeyAction, KeyFacet, KeyScope, KeyTargetKind};
use orch_api_contract::workspace::GitStatus;
use orch_authz::CapturedOperation;
use orch_db::queries::identity;
use orch_link_contract::commands::{Command, CommandKind, CommandResult};
use orch_link_contract::source::{GitAuthor, GitPushResult};
use orch_types::Hash;
use std::sync::Arc;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Fixture {
    world: common::World,
    engine: Engine,
    job: Job,
    plan: Plan,
    token: Id,
    command: Outgoing,
    siblings: Vec<Id>,
}

fn status() -> GitStatus {
    GitStatus {
        available: true,
        reason: None,
        repository_dir: Some("repo".into()),
        status_hash: Some(Hash::of(b"completed publication")),
        remotes: vec![],
        upstream: None,
        branch: Some("main".into()),
        head: Some("0123456789012345678901234567890123456789".into()),
        ahead: None,
        behind: None,
        changes: vec![],
    }
}

impl Fixture {
    async fn new(push: bool) -> TestResult<Self> {
        Self::configured(push, false).await
    }

    async fn configured(push: bool, siblings: bool) -> TestResult<Self> {
        let world = common::World::observed().await;
        let siblings = if siblings {
            vec![
                world.thread("First sharing directory").await,
                world.thread("Second sharing directory").await,
            ]
        } else {
            vec![]
        };
        world.engine().stop();
        let engine = Engine::new(EngineParts {
            db: world.pool.clone(),
            clock: world.clock.clone(),
            config: common::config(),
            commands: world.commands.as_engine(),
            access: world.access.clone(),
            live: world.live.clone(),
        });
        let operation = if push {
            "EnvironmentGitPush"
        } else {
            "EnvironmentGitCommit"
        };
        let actions = if push {
            vec![KeyAction::Manage, KeyAction::Read]
        } else {
            vec![KeyAction::Manage]
        };
        let token = new_id();
        let policy: KeyScope = serde_json::from_value(json!({"rules":[{
            "target":{"kind":"environment","id":world.environment,"reach":"self"},
            "allow":[{"operation":operation,"actions":actions}]
        }]}))?;
        assert!(policy.validate().is_empty());
        let mut tx = world.pool.begin().await?;
        if let Some(sibling) = siblings.get(1) {
            sqlx::query("UPDATE threads SET workspace_dir='review' WHERE node_id=$1")
                .bind(sibling)
                .execute(&mut *tx)
                .await?;
        }
        let host = new_id();
        sqlx::query(
            "INSERT INTO hosts(id,machine_id,name) VALUES($1,$2,'Publication fixture host')",
        )
        .bind(host)
        .bind(world.machine_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE environments SET remote_write=true,config=config||jsonb_build_object('env.host_id',$2::text) WHERE id=$1")
            .bind(world.environment).bind(host).execute(&mut *tx).await?;
        sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy,environment.remote_write}','true') WHERE id=$1")
            .bind(world.machine_id).execute(&mut *tx).await?;
        let expires = world.clock.now() + chrono::Duration::hours(1);
        identity::create_api_token_scoped(
            &mut tx,
            &identity::NewApiToken {
                id: token,
                user_id: world.ada,
                name: operation,
                token_hash: Hash::of(token.as_bytes()).as_bytes(),
                scopes: &[],
                node_id: None,
                at: world.clock.now(),
                expires_at: Some(expires),
            },
            Some(&json!(policy)),
        )
        .await?;
        let target = targets::page_object(
            &mut tx,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: world.environment,
            },
        )
        .await?;
        let caller = Caller::token(
            world.ada,
            TokenGrant::from_stored_scope(&[], None, Some(policy), Some(Time::new(expires)))?
                .for_token(token),
        )
        .admit_facets(
            &actions
                .iter()
                .map(|action| KeyFacet {
                    operation: operation.into(),
                    action: *action,
                    resource_kind: KeyTargetKind::Object(ObjectKind::Environment),
                    target: target.clone(),
                })
                .collect::<Vec<_>>(),
        )?
        .into_caller();
        let origin = CapturedOperation::capture(&caller)?;
        let environment = environments::workspace::environment(&mut tx, world.environment)
            .await?
            .ok_or("environment absent")?;
        let action = if push {
            Action::Push {
                body: GitPush {
                    environment_id: world.environment,
                    job_id: Id::nil(),
                    path: "repo".into(),
                    remote: "origin".into(),
                    expected_remote_url: "https://example.test/team/repository.git".into(),
                    branch: "main".into(),
                    expected_head: "0123456789012345678901234567890123456789".into(),
                    set_upstream: false,
                    credential: None,
                    credential_repository_url: None,
                },
            }
        } else {
            Action::Commit {
                body: GitCommit {
                    environment_id: world.environment,
                    job_id: Id::nil(),
                    path: "repo".into(),
                    paths: vec!["code".into()],
                    message: "Reviewed local change".into(),
                    expected_head: None,
                    expected_status: Hash::of(b"reviewed state"),
                    author: GitAuthor {
                        name: "Publisher".into(),
                        email: "publisher@orch.invalid".into(),
                    },
                },
            }
        };
        let row = begin(
            &mut tx,
            Plan {
                environment: world.environment,
                generation: environment.generation,
                machine: world.machine_id,
                thread: None,
                caller: world.ada,
                token: Some(token),
                creator_origin: None,
                scheduled_input: None,
                action,
            },
            world.clock.now(),
        )
        .await?;
        let plan: Plan = serde_json::from_value(row.plan.clone())?;
        rows::merge_plan(
            &mut tx,
            row.id,
            &json!({"publication_origin":origin,"publication_plan":plan}),
            world.clock.now(),
        )
        .await?;
        let runner = new_id();
        assert!(
            rows::claim(
                &mut tx,
                row.id,
                runner,
                world.clock.now() + chrono::Duration::hours(1),
                world.clock.now()
            )
            .await?
        );
        tx.commit().await?;
        let command = match &plan.action {
            Action::Commit { body } => Outgoing::new(
                crate::ids::derived("git.commit", &[row.id.as_bytes()]),
                plan.machine,
                SerialKey::Environment(plan.environment),
                body,
            )?,
            Action::Push { body } => Outgoing::new(
                crate::ids::derived("git.push", &[row.id.as_bytes()]),
                plan.machine,
                SerialKey::Environment(plan.environment),
                body,
            )?,
            _ => unreachable!("publication fixture"),
        };
        world.machine.handler.on(CommandKind::GitCommit, |command| {
            assert!(matches!(command, Command::GitCommit(_)));
            Ok(CommandResult::GitCommit(
                orch_api_contract::source::GitCommitResult {
                    head: "0123456789012345678901234567890123456789".into(),
                    status: status(),
                },
            ))
        });
        world.machine.handler.on(CommandKind::GitPush, |command| {
            assert!(matches!(command, Command::GitPush(_)));
            Ok(CommandResult::GitPush(GitPushResult {
                status: status(),
                base_branch: None,
            }))
        });
        world.machine.handler.on(CommandKind::GitStatus, |command| {
            assert!(matches!(command, Command::GitStatus(_)));
            Ok(CommandResult::GitStatus(status()))
        });
        Ok(Self {
            world,
            engine: engine.clone(),
            job: Job::claimed(engine, row.id, runner),
            plan,
            token,
            command,
            siblings,
        })
    }

    async fn reserve(&self) -> TestResult {
        let mut tx = self.world.pool.begin().await?;
        rows::merge_plan(&mut tx,self.job.id(),&json!({"publication_command":self.command,"active_command":{
            "id":self.command.id,"machine":self.command.machine,"kind":self.command.kind.as_str(),
        }}),self.world.clock.now()).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn revoke(&self) -> TestResult {
        sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
            .bind(self.token)
            .bind(self.world.clock.now())
            .execute(self.world.pool.pool())
            .await?;
        Ok(())
    }

    async fn wire(&self, command: &Outgoing) -> TestResult<bool> {
        let mut tx = self.world.pool.begin().await?;
        orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
        let permitted = creator_command_authorized(
            &mut tx,
            command.id,
            command.machine,
            command.key.wire().as_deref(),
            command.kind.as_str(),
            &command.body,
            Time::new(self.world.clock.now()),
        )
        .await?;
        tx.commit().await?;
        Ok(permitted)
    }
}

#[tokio::test]
async fn queued_commit_restores_the_original_operation_and_current_environment_authority()
-> TestResult {
    let s = Fixture::new(false).await?;
    let result = check(&s.job, &s.plan).await;
    assert!(
        result.is_ok(),
        "the accepted Manage publication is still authorized: {result:?}"
    );
    assert_eq!(s.world.commands.issued_of("git.commit"), 0);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn queued_push_restores_its_own_read_and_manage_without_reconstructing_a_session()
-> TestResult {
    let s = Fixture::new(true).await?;
    let result = check(&s.job, &s.plan).await;
    assert!(
        result.is_ok(),
        "the accepted Read+Manage publication is still authorized: {result:?}"
    );
    assert_eq!(s.world.commands.issued_of("git.push"), 0);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn queued_publication_revocation_and_narrowing_stop_before_native_acceptance() -> TestResult {
    for push in [false, true] {
        for revoked in [false, true] {
            let s = Fixture::new(push).await?;
            s.reserve().await?;
            if revoked {
                s.revoke().await?;
            } else {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(s.token)
                    .bind(json!({"rules":[]}))
                    .execute(s.world.pool.pool())
                    .await?;
            }
            assert!(
                !s.wire(&s.command).await?,
                "the dispatcher must repeat original credential authority before ACK"
            );
            assert_eq!(s.world.commands.issued_of(s.command.kind.as_str()), 0);
            s.engine.stop();
        }
    }
    Ok(())
}

#[tokio::test]
async fn publication_native_gate_requires_exact_command_body_kind_recipient_and_serial_lane()
-> TestResult {
    let s = Fixture::new(false).await?;
    s.reserve().await?;
    assert!(
        s.wire(&s.command).await?,
        "the exact current command is permitted before wire"
    );
    for difference in ["body", "kind", "recipient", "key"] {
        let mut command = s.command.clone();
        match difference {
            "body" => command.body["message"] = json!("another publication"),
            "kind" => command.kind = CommandKind::GitPush,
            "recipient" => command.machine = new_id(),
            "key" => command.key = SerialKey::Thread(new_id()),
            _ => unreachable!(),
        }
        assert!(
            !s.wire(&command).await?,
            "{difference} cannot borrow this job's scope"
        );
    }
    s.engine.stop();
    Ok(())
}

struct ReleaseReplay {
    inner: Arc<dyn Commands>,
    command: Id,
    machine: Arc<orch_testkit::doubles::machine::ScriptedMachine>,
}

#[async_trait]
impl Commands for ReleaseReplay {
    async fn issue(&self, command: Outgoing) -> Result<(), CommandsError> {
        let replay = command.id == self.command;
        self.inner.issue(command).await?;
        if replay {
            self.machine.handler.release();
        }
        Ok(())
    }
    async fn retained_body(&self, id: Id) -> Result<Option<Json>, CommandsError> {
        self.inner.retained_body(id).await
    }
    async fn retained_command(&self, id: Id) -> Result<Option<RetainedCommand>, CommandsError> {
        self.inner.retained_command(id).await
    }
    async fn status(&self, id: Id) -> Result<Option<Info>, CommandsError> {
        self.inner.status(id).await
    }
    async fn wait(&self, id: Id) -> Result<Outcome, CommandsError> {
        self.inner.wait(id).await
    }
    async fn cancel(&self, id: Id) -> Result<bool, CommandsError> {
        self.inner.cancel(id).await
    }
    async fn stopped(&self, id: Id) -> Result<bool, CommandsError> {
        self.inner.stopped(id).await
    }
}

async fn received(push: bool, terminal: bool, difference: Option<&str>) -> TestResult {
    let s = Fixture::new(push).await?;
    s.reserve().await?;
    let mut actual = s.command.clone();
    let holder = if difference == Some("recipient") {
        let foreign = s
            .world
            .add_machine("Foreign accepted publication peer", &["environment"])
            .await;
        actual.machine = foreign.id;
        foreign
            .machine
            .handler
            .on(CommandKind::GitCommit, |command| {
                assert!(matches!(command, Command::GitCommit(_)));
                Ok(CommandResult::GitCommit(
                    orch_api_contract::source::GitCommitResult {
                        head: "0123456789012345678901234567890123456789".into(),
                        status: status(),
                    },
                ))
            });
        foreign.machine
    } else {
        s.world.machine.clone()
    };
    match difference {
        Some("body") => actual.body["message"] = json!("foreign accepted publication"),
        Some("kind") => {
            actual.kind = CommandKind::GitPush;
            actual.body = json!(GitPush {
                environment_id: s.plan.environment,
                job_id: s.job.id(),
                path: "repo".into(),
                remote: "origin".into(),
                expected_remote_url: "https://example.test/team/repository.git".into(),
                branch: "main".into(),
                expected_head: "0123456789012345678901234567890123456789".into(),
                set_upstream: false,
                credential: None,
                credential_repository_url: None,
            });
        }
        Some("key") => actual.key = SerialKey::Thread(new_id()),
        Some("recipient") => {}
        Some("journal") => {
            let mut changed = s.command.clone();
            changed.body["message"] = json!("foreign journal publication");
            let mut tx = s.world.pool.begin().await?;
            rows::merge_plan(
                &mut tx,
                s.job.id(),
                &json!({"publication_command":changed}),
                s.world.clock.now(),
            )
            .await?;
            tx.commit().await?;
        }
        None => {}
        Some(_) => unreachable!("receipt control"),
    }
    holder.handler.hold(actual.kind);
    let commands = s.world.commands.as_engine();
    commands.issue(actual.clone()).await?;
    common::eventually("the actual publication frame is Accepted", || {
        let commands = commands.clone();
        async move {
            commands
                .status(actual.id)
                .await
                .is_ok_and(|info| info.is_some_and(|info| info.stage == Stage::Accepted))
        }
    })
    .await;
    holder.handler.started(actual.id).await;
    assert_eq!(
        commands
            .status(actual.id)
            .await?
            .ok_or("native receipt absent")?
            .stage,
        Stage::Accepted
    );
    let (_, frame) = s
        .world
        .commands
        .issued_commands(actual.kind.as_str())
        .into_iter()
        .find(|(_, frame)| frame.id == actual.id)
        .ok_or("actual issued frame absent")?;
    assert_eq!(frame.kind, actual.kind.as_str());
    assert_eq!(frame.key, actual.key.wire());
    assert_eq!(frame.body, actual.body);
    if terminal {
        holder.handler.release();
        assert!(matches!(
            commands.wait(actual.id).await?,
            Outcome::Succeeded(_)
        ));
    }
    s.revoke().await?;
    let engine = Engine::new(EngineParts {
        db: s.world.pool.clone(),
        clock: s.world.clock.clone(),
        config: common::config(),
        commands: Arc::new(ReleaseReplay {
            inner: commands,
            command: actual.id,
            machine: holder.clone(),
        }),
        access: s.world.access.clone(),
        live: s.world.live.clone(),
    });
    let row = s.job.row().await?;
    let runner = serde_json::from_value(row.plan["runner"]["id"].clone())?;
    let job = Job::claimed(engine.clone(), row.id, runner);
    let preparing = Preparing::of(engine.clone(), &row)?;
    let result = preparing.run(&job, "apply").await;
    holder.handler.release();
    if difference.is_none() {
        assert!(
            result.is_ok(),
            "an exact actual receipt reconciles after revocation: {result:?}"
        );
        assert!(job.plan().await?.get("file_result").is_some());
        assert_eq!(
            s.world.commands.issued_of(actual.kind.as_str()),
            1,
            "the retained native ID is not another effect"
        );
    } else {
        assert!(
            result.is_err(),
            "a foreign actual receipt is not this publication: {difference:?}"
        );
        assert!(job.plan().await?.get("file_result").is_none());
    }
    engine.stop();
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn exact_accepted_commit_and_terminal_push_reconcile_after_key_revocation() -> TestResult {
    received(false, false, None).await?;
    received(true, true, None).await
}

#[tokio::test]
async fn accepted_publication_never_borrows_foreign_body_kind_recipient_key_or_journal()
-> TestResult {
    for difference in ["body", "kind", "recipient", "key", "journal"] {
        received(false, false, Some(difference)).await?;
    }
    Ok(())
}

async fn aftermath(replaced: bool) -> TestResult {
    let s = Fixture::configured(false, true).await?;
    s.reserve().await?;
    let commands = s.world.commands.as_engine();
    commands.issue(s.command.clone()).await?;
    assert!(matches!(
        commands.wait(s.command.id).await?,
        Outcome::Succeeded(_)
    ));
    s.revoke().await?;
    if replaced {
        sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
            .bind(s.plan.environment)
            .execute(s.world.pool.pool())
            .await?;
    }
    let row = s.job.row().await?;
    let preparing = Preparing::of(s.engine.clone(), &row)?;
    for phase in ["apply", "verify", "commit"] {
        let result = preparing.run(&s.job, phase).await;
        assert!(
            result.is_ok(),
            "accepted {phase} retains the factual outcome after revoke: {result:?}"
        );
    }
    let saved = s.job.row().await?;
    assert!(saved.plan.get("file_result").is_some());
    assert_eq!(
        saved.plan.pointer("/file_publications/git"),
        Some(&json!(true))
    );
    let audit:i64=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_kind='environment' AND object_id=$1 AND details->>'job_id'=$2")
        .bind(s.plan.environment).bind(s.job.id().to_string()).fetch_one(s.world.pool.pool()).await?;
    assert_eq!(
        audit, 1,
        "completed publication is recorded once despite key withdrawal"
    );
    let refreshes = s.world.commands.issued_commands("git.status");
    assert_eq!(refreshes.len(), if replaced { 0 } else { 2 });
    for (machine, command) in refreshes {
        assert_eq!(machine, s.plan.machine);
        assert_eq!(command.body["environment_id"], json!(s.plan.environment));
        assert!(
            matches!(command.body["path"].as_str(), Some("web" | "review")),
            "only the actual same-environment sharing directories are refreshed"
        );
    }
    for thread in s.siblings {
        let row = threads::thread(&mut *s.world.pool.acquire().await?, thread)
            .await?
            .ok_or("sharing thread absent")?;
        assert_eq!(
            row.work_observed.is_some(),
            !replaced,
            "changed physical source defers watcher refresh instead of relabeling old facts"
        );
    }
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn completed_publication_records_its_fact_and_refreshes_only_its_original_environment_after_revoke()
-> TestResult {
    aftermath(false).await
}

#[tokio::test]
async fn completed_publication_keeps_result_and_audit_without_refreshing_a_replacement_generation()
-> TestResult {
    aftermath(true).await
}
