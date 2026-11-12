//! Current authority of real initial-copy commands held after native dispatch. The production
//! dispatcher separately preserves Accepted/terminal receipts before invoking this same guard.

use super::*;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{
    KeyAction, KeyFacet, KeyOperation, KeyReach, KeyRule, KeyScope, KeyTarget, KeyTargetKind,
};
use orch_access_contract::permissions::ObjectKind;
use orch_authz::{
    TokenGrant,
    ownership::{self, NewAccess},
};
use orch_db::queries::{identity, transaction_guard};
use orch_engine::operation_scope::targets;
use orch_link_contract::commands::Cmd;
use orch_types::new_id;

const CREATE: &str = "ThreadCreate";
const THREAD: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Thread);
const ENV: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);

fn rule(target: KeyTargetKind, id: Id, family: KeyTargetKind, action: KeyAction) -> KeyRule {
    KeyRule {
        target: KeyTarget {
            kind: target,
            id: Some(id),
            reach: KeyReach::SelfOnly,
        },
        resources: Some(vec![family]),
        allow: vec![KeyOperation {
            operation: CREATE.into(),
            actions: vec![action],
        }],
    }
}

struct Fixture {
    world: World,
    prepared: Prepared,
    made: environments::Made,
    unit: hosting::Unit,
    tokens: Vec<Id>,
    policy: KeyScope,
    machine: Id,
    command: Cmd,
}

impl Fixture {
    async fn new() -> Self {
        let world = World::new().await;
        hosting::serve(&world);
        let host = hosting::ScriptedHost::on(&world, "creator-initial-files", 16384).await;
        let prepared = source(&world).await;
        let created_thread = world.thread("Initial copy target").await;
        let template = hosting::template(&world).await;
        let mut tx = world.pool.begin().await.unwrap();
        let now = world.clock.now();
        let principal = Principal::load(&mut tx, Caller::session(world.ada), Time::new(now))
            .await
            .unwrap();
        let mut files = initial_files::plan(
            &mut tx,
            &principal,
            &NewEnvironmentFiles::FromThread {
                from_thread: prepared.thread,
                selection: Some(FileSelection::Selection {
                    include: vec!["uploads".into(), "shares".into()],
                    exclude: None,
                }),
            },
        )
        .await
        .unwrap();
        let made = environments::create(
            &mut tx,
            now,
            &principal,
            &Request {
                owner: world.ada,
                name: Some("Creator initial files"),
                home: world.topic,
                template: Some(template),
                host: Some(Some(host.id)),
                config: &json!({}),
            },
        )
        .await
        .unwrap();
        ownership::start_resource(
            &mut tx,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: made.id,
            },
            world.ada,
            NewAccess::Defaults,
            Time::new(now),
        )
        .await
        .unwrap();
        // The same existing hosting fixture bind used by ordinary hosted-engine tests.
        sqlx::query("UPDATE threads SET environment_id=$2, arrangement='separated', executor='orch-exec', tool_set='plain' WHERE node_id=$1")
            .bind(created_thread).bind(made.id).execute(&mut *tx).await.unwrap();
        let policy = KeyScope {
            rules: vec![
                rule(
                    KeyTargetKind::Object(ObjectKind::Topic),
                    world.topic,
                    THREAD,
                    KeyAction::Write,
                ),
                rule(
                    KeyTargetKind::Object(ObjectKind::Topic),
                    world.topic,
                    ENV,
                    KeyAction::Write,
                ),
                rule(THREAD, prepared.thread, THREAD, KeyAction::Read),
                rule(ENV, prepared.source, ENV, KeyAction::Read),
            ],
            ..KeyScope::default()
        };
        let mut callers = Vec::new();
        let mut tokens = Vec::new();
        for name in ["Accepted first boundary", "Accepted second boundary"] {
            let token = new_id();
            let expires = now + chrono::Duration::hours(1);
            let hash = Hash::of(token.as_bytes());
            identity::create_api_token_scoped(
                &mut tx,
                &identity::NewApiToken {
                    id: token,
                    user_id: world.ada,
                    name,
                    token_hash: hash.as_bytes(),
                    scopes: &[],
                    node_id: None,
                    at: now,
                    expires_at: Some(expires),
                },
                Some(&json!(policy)),
            )
            .await
            .unwrap();
            callers.push(Caller::token(
                world.ada,
                TokenGrant::from_stored_scope(
                    &[],
                    None,
                    Some(policy.clone()),
                    Some(Time::new(expires)),
                )
                .unwrap()
                .for_token(token),
            ));
            tokens.push(token);
        }
        let caller = callers[0].intersect_restrictions(&callers[1]).unwrap();
        let parent = targets::node(&mut tx, world.topic).await.unwrap();
        let source_thread = targets::node(&mut tx, prepared.thread).await.unwrap();
        let source_environment = targets::page_object(
            &mut tx,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: prepared.source,
            },
        )
        .await
        .unwrap();
        let caller = caller
            .admit_facets(&[
                KeyFacet {
                    operation: CREATE.into(),
                    action: KeyAction::Write,
                    resource_kind: THREAD,
                    target: parent.clone(),
                },
                KeyFacet {
                    operation: CREATE.into(),
                    action: KeyAction::Write,
                    resource_kind: ENV,
                    target: parent,
                },
                KeyFacet {
                    operation: CREATE.into(),
                    action: KeyAction::Read,
                    resource_kind: THREAD,
                    target: source_thread,
                },
                KeyFacet {
                    operation: CREATE.into(),
                    action: KeyAction::Read,
                    resource_kind: ENV,
                    target: source_environment,
                },
            ])
            .unwrap()
            .into_caller();
        let target = envs::environment(&mut tx, made.id).await.unwrap().unwrap();
        files
            .capture_creator(&caller, created_thread, made.id, target.generation)
            .unwrap();
        jobs::merge_plan(&mut tx, made.job.id, &json!({"initial_files": files}), now)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        world.engine().kick();
        let (unit, _) = enrolled(&world, &host, &made, 2).await;
        common::eventually(
            "the creator copy reaches its retained materialize command",
            || async {
                let mut conn = world.pool.acquire().await.unwrap();
                let row = jobs::job(&mut conn, made.job.id).await.unwrap().unwrap();
                assert!(
                    row.is_open(),
                    "creator initial files failed: {:?}",
                    row.error
                );
                unit.began(CommandKind::FsMaterialize) > 0
            },
        )
        .await;
        let (machine, command) = world
            .commands
            .issued_commands("fs.materialize")
            .into_iter()
            .find(|(_, command)| command.body["environment_id"] == json!(made.id))
            .unwrap();
        Self {
            world,
            prepared,
            made,
            unit,
            tokens,
            policy,
            machine,
            command,
        }
    }

    async fn allowed_in(&self, conn: &mut orch_db::Conn, machine: Id, command: &Cmd) -> bool {
        initial_files::command_authorized(
            conn,
            command.id,
            machine,
            command.key.as_deref(),
            &command.kind,
            &command.body,
            Time::new(self.world.clock.now()),
        )
        .await
        .unwrap()
    }

    async fn allowed(&self) -> bool {
        let mut tx = self.world.pool.begin().await.unwrap();
        transaction_guard::lock_action(&mut tx).await.unwrap();
        let allowed = self.allowed_in(&mut tx, self.machine, &self.command).await;
        tx.rollback().await.unwrap();
        allowed
    }

    fn stop(&self) {
        self.world.engine().stop();
        self.unit.let_go();
    }
}

#[tokio::test]
async fn both_accepted_key_boundaries_remain_required_for_initial_copy() {
    let s = Fixture::new().await;
    assert!(s.allowed().await);
    for token in &s.tokens {
        sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
            .bind(token)
            .bind(s.world.clock.now())
            .execute(s.world.pool.pool())
            .await
            .unwrap();
        assert!(
            !s.allowed().await,
            "revoking either accepted boundary stops a new dispatch"
        );
        sqlx::query("UPDATE api_tokens SET revoked_at=NULL WHERE id=$1")
            .bind(token)
            .execute(s.world.pool.pool())
            .await
            .unwrap();
        assert!(s.allowed().await);
    }
    let mut narrowed = s.policy.clone();
    narrowed
        .rules
        .retain(|rule| rule.target.id != Some(s.prepared.source));
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.tokens[1])
        .bind(json!(narrowed))
        .execute(s.world.pool.pool())
        .await
        .unwrap();
    assert!(
        !s.allowed().await,
        "current source-read scope intersects the accepted policy"
    );
    let mut narrowed = s.policy.clone();
    narrowed.rules.remove(0); // This fixture's exact parent ThreadCreate Write rule.
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.tokens[1])
        .bind(json!(narrowed))
        .execute(s.world.pool.pool())
        .await
        .unwrap();
    assert!(
        !s.allowed().await,
        "the current creator destination remains required"
    );
    s.stop();
}

#[tokio::test]
async fn queued_initial_copy_keeps_its_exact_command_and_active_owner() {
    let s = Fixture::new().await;
    assert!(s.allowed().await);
    let mut tx = s.world.pool.begin().await.unwrap();
    transaction_guard::lock_action(&mut tx).await.unwrap();
    assert!(!s.allowed_in(&mut tx, new_id(), &s.command).await);
    let mut changed = s.command.clone();
    changed.body["target_root"] = json!("/another/root");
    assert!(!s.allowed_in(&mut tx, s.machine, &changed).await);
    changed = s.command.clone();
    changed.body["manifest_blob"] = json!(Hash::of(b"different contents"));
    assert!(!s.allowed_in(&mut tx, s.machine, &changed).await);
    changed = s.command.clone();
    changed.key = orch_link_contract::commands::SerialKey::Environment(s.prepared.source).wire();
    assert!(!s.allowed_in(&mut tx, s.machine, &changed).await);
    jobs::merge_plan(
        &mut tx,
        s.made.job.id,
        &json!({"active_command": null}),
        s.world.clock.now(),
    )
    .await
    .unwrap();
    assert!(
        !s.allowed_in(&mut tx, s.machine, &s.command).await,
        "a retained command stays recognized after active_command is cleared"
    );
    tx.rollback().await.unwrap();
    assert!(s.allowed().await);
    let mut tx = s.world.pool.begin().await.unwrap();
    transaction_guard::lock_action(&mut tx).await.unwrap();
    jobs::merge_plan(
        &mut tx,
        s.made.job.id,
        &json!({"cancel": true}),
        s.world.clock.now(),
    )
    .await
    .unwrap();
    assert!(
        !s.allowed_in(&mut tx, s.machine, &s.command).await,
        "cancellation cannot authorize an unaccepted native copy"
    );
    tx.rollback().await.unwrap();
    let (_, earlier) = s
        .world
        .commands
        .issued_commands("fs.snapshot")
        .into_iter()
        .find(|(_, cmd)| cmd.body["environment_id"] == json!(s.prepared.source))
        .unwrap();
    let mut tx = s.world.pool.begin().await.unwrap();
    transaction_guard::lock_action(&mut tx).await.unwrap();
    assert!(
        !s.allowed_in(&mut tx, s.world.machine_id, &earlier).await,
        "a late unacknowledged earlier phase cannot borrow the new phase's active claim"
    );
    tx.rollback().await.unwrap();
    s.stop();
}

#[tokio::test]
async fn initial_copy_rechecks_the_source_and_accepted_target_generation() {
    let s = Fixture::new().await;
    assert!(s.allowed().await);
    for environment in [s.prepared.source, s.made.id] {
        let mut tx = s.world.pool.begin().await.unwrap();
        transaction_guard::lock_action(&mut tx).await.unwrap();
        sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
            .bind(environment)
            .execute(&mut *tx)
            .await
            .unwrap();
        assert!(!s.allowed_in(&mut tx, s.machine, &s.command).await);
        tx.rollback().await.unwrap();
    }
    let mut tx = s.world.pool.begin().await.unwrap();
    transaction_guard::lock_action(&mut tx).await.unwrap();
    jobs::merge_plan(
        &mut tx,
        s.made.job.id,
        &json!({"runtime": new_id()}),
        s.world.clock.now(),
    )
    .await
    .unwrap();
    assert!(
        !s.allowed_in(&mut tx, s.machine, &s.command).await,
        "a different provisioning candidate cannot acquire the accepted copy"
    );
    tx.rollback().await.unwrap();
    let mut tx = s.world.pool.begin().await.unwrap();
    transaction_guard::lock_action(&mut tx).await.unwrap();
    sqlx::query("UPDATE threads SET workspace_dir='changed-source' WHERE node_id=$1")
        .bind(s.prepared.thread)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(!s.allowed_in(&mut tx, s.machine, &s.command).await);
    tx.rollback().await.unwrap();
    assert!(s.allowed().await);
    s.stop();
}
