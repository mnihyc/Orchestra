//! The real dispatcher rechecks a retained scoped publication before the first native ACK.
//! The ordinary parent peer's real CommandServer/journal observes every actual wire frame.

use super::*;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyAction;

use orch_access_contract::key_scope::KeyFacet;

use orch_access_contract::key_scope::KeyScope;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_authz::Caller;

use orch_authz::CapturedOperation;

use orch_authz::TokenGrant;

use orch_authz::ownership;

use orch_db::queries::access;

use orch_db::queries::identity;

use orch_db::queries::jobs;

use orch_engine::commands::Outgoing;

use orch_engine::jobs::source;

use orch_engine::jobs::source::Action;

use orch_engine::jobs::source::Plan;

use orch_link_contract::source::GitAuthor;

use orch_link_contract::source::GitCommit;

use orch_link_contract::source::GitPush;
async fn revoked_first_wire(push: bool) -> Proof {
    let mut fixture = Fixture::new(false).await?;
    let mut conn = fixture.test.pool.acquire().await?;
    let (owner, org): (Id, Id) =
        sqlx::query_as("SELECT owner_id,org_id FROM environments WHERE id=$1")
            .bind(fixture.environment)
            .fetch_one(&mut *conn)
            .await?;
    access::testing::insert_member(&mut conn, org, owner, org).await?;
    access::testing::name_owner(&mut conn, org, ("user", owner), fixture.clock.now()).await?;
    ownership::start_resource(
        &mut conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: fixture.environment,
        },
        owner,
        ownership::NewAccess::OnlyMe,
        fixture.clock.time(),
    )
    .await?;
    sqlx::query("UPDATE environments SET remote_write=true WHERE id=$1")
        .bind(fixture.environment)
        .execute(&mut *conn)
        .await?;
    sqlx::query("UPDATE machines SET facts=jsonb_build_object('policy',jsonb_build_object('environment.remote_write',true)) WHERE id=$1").bind(fixture.machine).execute(&mut *conn).await?;
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
    let policy: KeyScope = serde_json::from_value(json!({"rules":[{
        "target":{"kind":"environment","id":fixture.environment,"reach":"self"},
        "allow":[{"operation":operation,"actions":actions}]
    }]}))?;
    assert!(policy.validate().is_empty());
    let key = new_id();
    let expires = fixture.clock.now() + chrono::Duration::hours(1);
    identity::create_api_token_scoped(
        &mut conn,
        &identity::NewApiToken {
            id: key,
            user_id: owner,
            name: "First-wire publication",
            token_hash: Hash::of(key.as_bytes()).as_bytes(),
            scopes: &[],
            node_id: None,
            at: fixture.clock.now(),
            expires_at: Some(expires),
        },
        Some(&json!(policy)),
    )
    .await?;
    let target = orch_engine::operation_scope::targets::page_object(
        &mut conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: fixture.environment,
        },
    )
    .await?;
    let caller = Caller::token(
        owner,
        TokenGrant::from_stored_scope(&[], None, Some(policy), Some(Time::new(expires)))?
            .for_token(key),
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
    let action = if push {
        Action::Push {
            body: GitPush {
                environment_id: fixture.environment,
                job_id: Id::nil(),
                path: "repo".into(),
                remote: "origin".into(),
                expected_remote_url: "https://example.test/team/repo.git".into(),
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
                environment_id: fixture.environment,
                job_id: Id::nil(),
                path: "repo".into(),
                paths: vec!["code".into()],
                message: "Reviewed supplied change".into(),
                expected_head: None,
                expected_status: Hash::of(b"reviewed source status"),
                author: GitAuthor {
                    name: "Publisher".into(),
                    email: "publisher@orch.invalid".into(),
                },
            },
        }
    };
    drop(conn);
    let mut tx = fixture.test.pool.begin().await?;
    let row = source::begin(
        &mut tx,
        Plan {
            environment: fixture.environment,
            generation: 1,
            machine: fixture.machine,
            thread: None,
            caller: owner,
            token: Some(key),
            creator_origin: None,
            scheduled_input: None,
            action,
        },
        fixture.clock.now(),
    )
    .await?;
    let plan: Plan = serde_json::from_value(row.plan.clone())?;
    let command = match &plan.action {
        Action::Commit { body } => Outgoing::new(
            orch_engine::ids::derived("git.commit", &[row.id.as_bytes()]),
            fixture.machine,
            SerialKey::Environment(fixture.environment),
            body,
        )?,
        Action::Push { body } => Outgoing::new(
            orch_engine::ids::derived("git.push", &[row.id.as_bytes()]),
            fixture.machine,
            SerialKey::Environment(fixture.environment),
            body,
        )?,
        _ => unreachable!("publication fixture"),
    };
    jobs::merge_plan(
        &mut tx,
        row.id,
        &json!({"publication_origin":origin,"publication_plan":plan,"publication_command":command,
        "active_command":{"id":command.id,"machine":command.machine,"kind":command.kind.as_str()}}),
        fixture.clock.now(),
    )
    .await?;
    tx.commit().await?;
    let id = command.id;
    let original = StoredCommand {
        authority: None,
        id: Some(id),
        machine: command.machine,
        kind: command.kind.as_str().into(),
        key: command.key.wire(),
        deadline: None,
        body: command.body.clone(),
    };
    fixture.dispatcher.issue_stored(original.clone()).await?;
    assert!(
        fixture.server.entry(id).is_none(),
        "no peer has accepted this queued publication"
    );
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(key)
        .bind(fixture.clock.now())
        .execute(&fixture.test.pool)
        .await?;
    within(fixture.connect(false)).await??;
    fixture.completed_probe().await?;
    let retained = fixture
        .dispatcher
        .status(id)
        .await?
        .ok_or("original durable command absent")?;
    assert!(
        matches!(retained.state.as_str(), "failed" | "cancelled" | "expired"),
        "withdrawn origin reaches the actual refusal path: {}",
        retained.state
    );
    assert!(
        fixture.server.entry(id).is_none(),
        "the native journal has no Accepted entry for the revoked operation"
    );
    assert!(
        !fixture
            .native
            .wire
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|frame| frame.id == id),
        "no publication frame crosses the gateway cut after original key withdrawal"
    );
    assert_eq!(retained.body, original.body);
    assert_eq!(retained.kind, original.kind);
    assert_eq!(retained.serial_key(), original.key.as_deref());
    fixture.close().await
}

#[cfg(any(test, feature = "test-support"))]
async fn a_revoked_queued_commit_never_reaches_the_real_native_journal_on_reconnect() -> Proof {
    revoked_first_wire(false).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_revoked_queued_commit_never_reaches_the_real_native_journal_on_reconnect()
-> ::futures::future::LocalBoxFuture<
    'static,
    ::orch_api_base::backplane::command_admission_tests::Proof,
> {
    ::std::boxed::Box::pin(
        self::a_revoked_queued_commit_never_reaches_the_real_native_journal_on_reconnect(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn a_revoked_queued_push_never_reaches_the_real_native_journal_on_reconnect() -> Proof {
    revoked_first_wire(true).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_revoked_queued_push_never_reaches_the_real_native_journal_on_reconnect()
-> ::futures::future::LocalBoxFuture<
    'static,
    ::orch_api_base::backplane::command_admission_tests::Proof,
> {
    ::std::boxed::Box::pin(
        self::a_revoked_queued_push_never_reaches_the_real_native_journal_on_reconnect(),
    )
}
