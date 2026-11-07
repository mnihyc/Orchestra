//! Actual queued creator-source commands retain their accepted key and current file footprint.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod common;

use common::World;
use orch_access_contract::key_scope::{
    KeyAction, KeyFacet, KeyOperation, KeyReach, KeyRule, KeyScope, KeyTarget, KeyTargetKind,
};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::source::{RepositorySource, SourceDestination};
use orch_authz::{Caller, CapturedOperation, TokenGrant};
use orch_db::queries::{identity, jobs, threads};
use orch_engine::{
    jobs::source::{self, Action, Plan},
    operation_scope::{creator_files, targets},
};
use orch_link_contract::commands::{FsStat, SerialKey};
use orch_types::{Hash, Id, Json, Time, new_id};
use serde_json::json;

const CREATE: &str = "ThreadCreate";
const ENV: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);
const THREAD: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Thread);

fn rule(kind: KeyTargetKind, id: Id, family: KeyTargetKind, actions: Vec<KeyAction>) -> KeyRule {
    KeyRule {
        target: KeyTarget {
            kind,
            id: Some(id),
            reach: KeyReach::SelfOnly,
        },
        resources: Some(vec![family]),
        allow: vec![KeyOperation {
            operation: CREATE.into(),
            actions,
        }],
    }
}

struct Fixture {
    world: World,
    thread: Id,
    token: Id,
    job: Id,
    plan: Plan,
    command: Id,
    body: Json,
}

impl Fixture {
    async fn new() -> Self {
        let world = World::observed().await;
        let thread = world.thread("Accepted creator source").await;
        world.engine().stop();
        let token = new_id();
        let expires = world.clock.now() + chrono::Duration::hours(1);
        let policy = KeyScope {
            rules: vec![
                rule(
                    KeyTargetKind::Object(ObjectKind::Topic),
                    world.topic,
                    THREAD,
                    vec![KeyAction::Write],
                ),
                rule(ENV, world.environment, ENV, vec![KeyAction::Manage]),
            ],
            ..KeyScope::default()
        };
        let mut tx = world.pool.begin().await.unwrap();
        let hash = Hash::of(token.as_bytes());
        identity::create_api_token_scoped(
            &mut tx,
            &identity::NewApiToken {
                id: token,
                user_id: world.ada,
                name: "Creator source boundary",
                token_hash: hash.as_bytes(),
                scopes: &[],
                node_id: None,
                at: world.clock.now(),
                expires_at: Some(expires),
            },
            Some(&json!(policy)),
        )
        .await
        .unwrap();
        let caller = Caller::token(
            world.ada,
            TokenGrant::from_stored_scope(&[], None, Some(policy), Some(Time::new(expires)))
                .unwrap()
                .for_token(token),
        );
        let parent = targets::node(&mut tx, world.topic).await.unwrap();
        let env = targets::page_object(
            &mut tx,
            orch_access_contract::decisions::ObjectRef {
                kind: ObjectKind::Environment,
                id: world.environment,
            },
        )
        .await
        .unwrap();
        let admitted = caller
            .admit_facets(&[
                KeyFacet {
                    operation: CREATE.into(),
                    action: KeyAction::Write,
                    resource_kind: THREAD,
                    target: parent,
                },
                KeyFacet {
                    operation: CREATE.into(),
                    action: KeyAction::Manage,
                    resource_kind: ENV,
                    target: env,
                },
            ])
            .unwrap()
            .into_caller();
        let origin = CapturedOperation::capture(&admitted).unwrap();
        let row = threads::thread(&mut tx, thread).await.unwrap().unwrap();
        let env =
            orch_db::queries::environments::workspace::environment(&mut tx, world.environment)
                .await
                .unwrap()
                .unwrap();
        let job = source::begin(
            &mut tx,
            Plan {
                environment: world.environment,
                generation: env.generation,
                machine: env.machine_id.unwrap(),
                thread: Some(thread),
                caller: world.ada,
                token: Some(token),
                creator_origin: Some(origin),
                scheduled_input: None,
                action: Action::Prepare {
                    directory: row.workspace_dir.clone(),
                    source: RepositorySource {
                        repository_url: None,
                        branch: None,
                        secret_name: None,
                    },
                    credential: None,
                    destination: SourceDestination::MissingDirectory,
                    effect_id: None,
                },
            },
            world.clock.now(),
        )
        .await
        .unwrap();
        let plan: Plan = serde_json::from_value(job.plan.clone()).unwrap();
        let command = new_id();
        let body = json!(FsStat {
            environment_id: world.environment,
            mount: None,
            path: row.workspace_dir
        });
        jobs::merge_plan(
            &mut tx,
            job.id,
            &json!({
                "active_command": {"id": command, "machine": plan.machine, "kind": "fs.stat"},
                "creator_command": {"id": command, "plan": plan, "runtime": null},
            }),
            world.clock.now(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        Self {
            world,
            thread,
            token,
            job: job.id,
            plan,
            command,
            body,
        }
    }

    async fn allowed(&self) -> bool {
        let mut tx = self.world.pool.begin().await.unwrap();
        orch_db::queries::transaction_guard::lock_action(&mut tx)
            .await
            .unwrap();
        let allowed = source::creator_command_authorized(
            &mut tx,
            self.command,
            self.plan.machine,
            SerialKey::Environment(self.plan.environment)
                .wire()
                .as_deref(),
            "fs.stat",
            &self.body,
            Time::new(self.world.clock.now()),
        )
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        allowed
    }
}

#[tokio::test]
async fn queued_creator_source_rechecks_revocation_without_changing_its_frozen_recipe() {
    let s = Fixture::new().await;
    assert!(
        s.allowed().await,
        "the admitted original operation permits its unchanged file effect"
    );
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(s.token)
        .bind(s.world.clock.now())
        .execute(s.world.pool.pool())
        .await
        .unwrap();
    assert!(
        !s.allowed().await,
        "a durable queued command cannot discard its revoked origin"
    );
    let mut conn = s.world.pool.acquire().await.unwrap();
    let current = threads::thread(&mut conn, s.thread).await.unwrap().unwrap();
    assert_eq!(current.source.unwrap()["native_plan"], json!(s.plan));
    assert_eq!(
        s.world.commands.issued_of("fs.stat"),
        0,
        "these checks admit no transport side effect"
    );
}

#[tokio::test]
async fn continuing_creator_requires_its_current_parent_and_environment_generation() {
    let s = Fixture::new().await;
    assert!(s.allowed().await);
    let narrowed = KeyScope {
        rules: vec![rule(ENV, s.plan.environment, ENV, vec![KeyAction::Manage])],
        ..KeyScope::default()
    };
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.token)
        .bind(json!(narrowed))
        .execute(s.world.pool.pool())
        .await
        .unwrap();
    assert!(
        !s.allowed().await,
        "Environment Manage cannot replace the original parent bound"
    );
    // The accepted snapshot remains conjunctive with this current row; expanding the row again
    // can restore its original authority but cannot change the captured source/environment.
    let restored = KeyScope {
        rules: vec![
            rule(
                KeyTargetKind::Object(ObjectKind::Topic),
                s.world.topic,
                THREAD,
                vec![KeyAction::Write],
            ),
            rule(ENV, s.plan.environment, ENV, vec![KeyAction::Manage]),
        ],
        ..KeyScope::default()
    };
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.token)
        .bind(json!(restored))
        .execute(s.world.pool.pool())
        .await
        .unwrap();
    assert!(s.allowed().await);
    let other_topic = s.world.topic("Outside the accepted creator parent").await;
    let mut tx = s.world.pool.begin().await.unwrap();
    orch_db::queries::hierarchy::move_under(&mut tx, s.thread, other_topic, s.world.clock.now())
        .await
        .unwrap();
    sqlx::query("UPDATE threads SET topic_id=$2 WHERE node_id=$1")
        .bind(s.thread)
        .bind(other_topic)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(
        !s.allowed().await,
        "the actual moved parent is resolved again, never the accepted old path"
    );
    let mut tx = s.world.pool.begin().await.unwrap();
    orch_db::queries::hierarchy::move_under(&mut tx, s.thread, s.world.topic, s.world.clock.now())
        .await
        .unwrap();
    sqlx::query("UPDATE threads SET topic_id=$2 WHERE node_id=$1")
        .bind(s.thread)
        .bind(s.world.topic)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(s.allowed().await);
    sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
        .bind(s.plan.environment)
        .execute(s.world.pool.pool())
        .await
        .unwrap();
    assert!(
        !s.allowed().await,
        "a replacement environment is not the admitted file target"
    );
}

#[tokio::test]
async fn creator_wire_admission_cannot_lend_one_recipe_to_a_different_native_command() {
    let s = Fixture::new().await;
    assert!(s.allowed().await);
    let mut tx = s.world.pool.begin().await.unwrap();
    let key = SerialKey::Environment(s.plan.environment).wire();
    for (machine, kind, body) in [
        (new_id(), "fs.stat", s.body.clone()),
        (s.plan.machine, "fs.mkdir", s.body.clone()),
        (
            s.plan.machine,
            "fs.stat",
            json!({"environment_id":s.plan.environment,"mount":null,"path":"different-directory"}),
        ),
    ] {
        assert!(
            !source::creator_command_authorized(
                &mut tx,
                s.command,
                machine,
                key.as_deref(),
                kind,
                &body,
                Time::new(s.world.clock.now())
            )
            .await
            .unwrap()
        );
    }
    jobs::merge_plan(
        &mut tx,
        s.job,
        &json!({"active_command":null}),
        s.world.clock.now(),
    )
    .await
    .unwrap();
    assert!(
        !source::creator_command_authorized(
            &mut tx,
            s.command,
            s.plan.machine,
            key.as_deref(),
            "fs.stat",
            &s.body,
            Time::new(s.world.clock.now())
        )
        .await
        .unwrap(),
        "retained creator evidence cannot fall through after active ownership was cleared"
    );
    jobs::merge_plan(
        &mut tx,
        s.job,
        &json!({"active_command":{"id":s.command,"machine":s.plan.machine,"kind":"fs.stat"}}),
        s.world.clock.now(),
    )
    .await
    .unwrap();
    jobs::merge_plan(
        &mut tx,
        s.job,
        &json!({"creator_command":{"id":new_id(),"plan":s.plan,"runtime":null}}),
        s.world.clock.now(),
    )
    .await
    .unwrap();
    assert!(
        !source::creator_command_authorized(
            &mut tx,
            s.command,
            s.plan.machine,
            key.as_deref(),
            "fs.stat",
            &s.body,
            Time::new(s.world.clock.now())
        )
        .await
        .unwrap()
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn a_missing_scoped_creator_origin_cannot_be_rebuilt_as_a_legacy_file_grant() {
    let s = Fixture::new().await;
    assert!(s.allowed().await);
    let mut tx = s.world.pool.begin().await.unwrap();
    let mut missing = s.plan.clone();
    missing.creator_origin = None;
    jobs::merge_plan(
        &mut tx,
        s.job,
        &json!({
            "creator_origin":null,
            "creator_command":{"id":s.command,"plan":missing,"runtime":null},
        }),
        s.world.clock.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE threads SET source=jsonb_set(source,'{native_plan}',$2) - 'creator_origin' WHERE node_id=$1")
        .bind(s.thread).bind(json!(missing)).execute(&mut *tx).await.unwrap();
    let key = SerialKey::Environment(s.plan.environment).wire();
    assert!(
        !source::creator_command_authorized(
            &mut tx,
            s.command,
            s.plan.machine,
            key.as_deref(),
            "fs.stat",
            &s.body,
            Time::new(s.world.clock.now())
        )
        .await
        .unwrap()
    );
    jobs::merge_plan(
        &mut tx,
        s.job,
        &json!({"creator_command":null}),
        s.world.clock.now(),
    )
    .await
    .unwrap();
    assert!(
        !source::creator_command_authorized(
            &mut tx,
            s.command,
            s.plan.machine,
            key.as_deref(),
            "fs.stat",
            &s.body,
            Time::new(s.world.clock.now())
        )
        .await
        .unwrap(),
        "an uncaptured modern file job cannot bypass the wire checkpoint as historical work"
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn a_worktree_source_read_is_not_granted_by_the_creator_environment_manage_effect() {
    let s = Fixture::new().await;
    let mut conn = s.world.pool.acquire().await.unwrap();
    let origin = s.plan.creator_origin.as_ref().unwrap();
    assert!(
        creator_files::restore(
            &mut conn,
            origin,
            s.thread,
            s.plan.environment,
            false,
            Time::new(s.world.clock.now())
        )
        .await
        .unwrap()
        .is_some()
    );
    assert!(
        creator_files::restore(
            &mut conn,
            origin,
            s.thread,
            s.plan.environment,
            true,
            Time::new(s.world.clock.now())
        )
        .await
        .unwrap()
        .is_none(),
        "the original Manage selection does not contain independent source Read"
    );
}
