//! Real native capability decisions retain scope while an owned live application is still unaccepted.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a control fails by panicking"
)]
#[path = "../../tests/hosting/mod.rs"]
mod hosting;
#[path = "../../tests/native_tools/mod.rs"]
mod native_tools;
use crate::jobs::{
    capability_change::Applying,
    runner::{Job, Phased, Stop, Undone},
};
use crate::operation_scope::{Prepared, targets};
use crate::{Engine, EngineParts, test_common::World};
use orch_access_contract::{decisions::ObjectRef, key_scope::KeyScope, permissions::ObjectKind};
use orch_api_contract::{
    approvals::{ApprovalDecide, ApprovalDecision, CapabilityDecision},
    routes::Endpoint,
};
use orch_authz::{Caller, CapturedOperation, Principal, TokenGrant};
use orch_db::queries::{approvals, identity, jobs};
use orch_link_contract::commands::{CommandKind, CommandResult, NodeCapabilitiesApplyResult};
use orch_types::{Clock, Hash, Id, Json, Time, new_id};
use serde_json::json;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
struct Fixture {
    world: World,
    engine: Engine,
    host: hosting::ScriptedHost,
    _unit: hosting::Unit,
    environment: Id,
    runtime: Id,
    approval: Id,
    token: Id,
    origin: CapturedOperation,
    job: Job,
}
impl Fixture {
    async fn new() -> TestResult<Self> {
        let world = World::observed().await;
        sqlx::query("INSERT INTO capability_defs(key,label,description,risk,driver_patch,default_decision,default_approver_scope) VALUES('kvm','KVM virtualisation','Existing prepared-live authority control','high','{}','ask','host')")
            .execute(world.pool.pool()).await?;
        let (host, environment, runtime, unit) =
            hosting::capabilities::prepared_live_environment(&world).await;
        native_tools::workspace_on(&world, &unit.added.machine, environment);
        let thread = world
            .engine()
            .create_thread(
                &Caller::session(world.ada),
                crate::binding::NewThread {
                    topic: world.topic,
                    name: "Pending live capability authority".into(),
                    agent: world.agent,
                    provider: world.provider,
                    environment,
                    directory: "web".into(),
                    arrangement: None,
                    model: Some(crate::test_common::MODEL.into()),
                    effort: None,
                },
            )
            .await?
            .thread;
        // The setup uses the existing enrolled scripted unit. Its completed lifecycle is not
        // part of this authority claim. An unstarted executor below makes queue/claim deterministic.
        world.engine().stop();
        let engine = Engine::new(EngineParts {
            db: world.pool.clone(),
            clock: world.clock.clone(),
            config: crate::test_common::config(),
            commands: world.commands.as_engine(),
            access: world.access.clone(),
            live: world.live.clone(),
        });
        let (token, approval) = (new_id(), new_id());
        let policy: KeyScope = serde_json::from_value(
            json!({"rules":[{"target":{"kind":"host","id":host.id,"reach":"self"},"resources":["host"],"allow":[{"operation":"ApprovalDecide.capability","actions":["manage"]}]}]}),
        )?;
        assert!(policy.validate().is_empty());
        let expires = world.clock.now() + chrono::Duration::hours(1);
        let mut tx = world.pool.begin().await?;
        identity::create_api_token_scoped(
            &mut tx,
            &identity::NewApiToken {
                id: token,
                user_id: world.ada,
                name: "Live approval origin",
                token_hash: Hash::of(token.as_bytes()).as_bytes(),
                scopes: &[],
                node_id: None,
                at: world.clock.now(),
                expires_at: Some(expires),
            },
            Some(&serde_json::to_value(&policy)?),
        )
        .await?;
        let caller = Caller::token(
            world.ada,
            TokenGrant::from_stored_scope(&[], None, Some(policy), Some(Time::new(expires)))?
                .for_token(token),
        );
        let target = targets::page_object(
            &mut tx,
            ObjectRef {
                kind: ObjectKind::Host,
                id: host.id,
            },
        )
        .await?;
        let mut prepared = Prepared::new(ApprovalDecide::ROUTE, "ApprovalDecide.capability")?;
        prepared.bind(0, target.object.kind, &[target])?;
        let caller = prepared.admit(&caller, false)?;
        let origin = CapturedOperation::capture(&caller)?;
        let principal = Principal::load(&mut tx, caller, Time::new(world.clock.now())).await?;
        assert!(!principal.is_system_admin());
        let env = orch_db::queries::environments::thread_engine::environment(&mut tx, environment)
            .await?
            .ok_or("environment absent")?;
        let environment_ref =
            crate::views::environment_ref_json(&mut tx, &env, Time::new(world.clock.now())).await?;
        let host_address = orch_authz::resources::addresses(&mut tx, &[host.id])
            .await?
            .into_iter()
            .find(|(id, _)| *id == host.id)
            .ok_or("host address absent")?
            .1;
        let request = json!({"capability":"kvm","label":"KVM virtualisation","risk":"high","reason":"Apply the declared prepared live capability","environment":environment_ref,"host":{"id":host.id,"name":"live-capabilities","address":host_address},"consequences":["Existing live application, without replacement"]});
        serde_json::from_value::<orch_api_contract::approvals::CapabilityRequest>(request.clone())?;
        let row = approvals::capability::insert(
            &mut tx,
            &approvals::capability::NewCapabilityApproval {
                id: approval,
                thread_id: thread,
                turn_id: None,
                environment_id: environment,
                title: "Give the prepared capability",
                request: &request,
                approver_scope: "host",
                expires_at: expires,
                at: world.clock.now(),
            },
        )
        .await?
        .ok_or("approval absent")?;
        assert!(
            engine
                .may_decide_capability_request(principal.caller(), &row)
                .await?
        );
        assert!(
            engine
                .capability_is_live(&mut tx, environment, "kvm")
                .await?
        );
        let accepted = engine
            .decide_capability_guarded(
                tx,
                principal,
                approval,
                &ApprovalDecision::Capability(CapabilityDecision {
                    approve: true,
                    note: None,
                }),
            )
            .await?;
        assert_eq!(accepted.state, "approved");
        let mut conn = world.pool.acquire().await?;
        let row:jobs::JobRow=sqlx::query_as("SELECT * FROM jobs WHERE environment_id=$1 AND kind='capability_change' ORDER BY created_at DESC,id DESC LIMIT 1").bind(environment).fetch_one(&mut *conn).await?;
        assert_eq!(row.plan["runtime"], json!(runtime));
        assert_eq!(row.plan["approval"], json!(approval));
        // A stopped old generation cannot send work. Release only a stale fixture lease
        // if its last sweep claimed this row, then take the real canonical claim ourselves.
        if let Some(old) = row
            .plan
            .pointer("/runner/id")
            .and_then(Json::as_str)
            .and_then(|id| id.parse::<Id>().ok())
        {
            jobs::release(&mut conn, row.id, old, world.clock.now()).await?;
        }
        let runner = new_id();
        assert!(jobs::claim(&mut conn, row.id, runner, expires, world.clock.now()).await?);
        drop(conn);
        host.answer(CommandKind::NodeCapabilitiesApply, |_| {
            Ok(CommandResult::NodeCapabilitiesApply(
                NodeCapabilitiesApplyResult {
                    applied: true,
                    restarted: false,
                    reason: None,
                },
            ))
        });
        let job = Job::claimed(engine.clone(), row.id, runner);
        assert_eq!(host.count(CommandKind::NodeCapabilitiesApply), 0);
        Ok(Self {
            world,
            engine,
            host,
            _unit: unit,
            environment,
            runtime,
            approval,
            token,
            origin,
            job,
        })
    }
    async fn applying(&self) -> TestResult<Applying> {
        Ok(Applying::of(self.engine.clone(), &self.job.row().await?)?)
    }
    async fn revoke(&self) -> TestResult {
        assert!(
            identity::revoke_api_token(
                &mut *self.world.pool.acquire().await?,
                self.token,
                self.world.ada,
                self.world.clock.now()
            )
            .await?
        );
        Ok(())
    }
}
#[tokio::test]
async fn native_live_decision_retains_its_exact_accepted_key_operation() -> TestResult {
    let s = Fixture::new().await?;
    let row = s.job.row().await?;
    assert!(
        row.plan["approval_origin"].is_object(),
        "the normal native producer must retain known key provenance instead of only decided_by"
    );
    let captured: CapturedOperation = serde_json::from_value(row.plan["approval_origin"].clone())?;
    assert_eq!(captured, s.origin);
    assert_eq!(captured.operation(), "ApprovalDecide.capability");
    assert_eq!(row.plan["runtime"], json!(s.runtime));
    s.engine.stop();
    Ok(())
}
#[tokio::test]
async fn revoked_key_refuses_unaccepted_live_apply_and_no_grant_is_published() -> TestResult {
    let s = Fixture::new().await?;
    s.revoke().await?;
    let applying = s.applying().await?;
    let result = applying.run(&s.job, "apply").await;
    let forward = s.host.count(CommandKind::NodeCapabilitiesApply);
    // Clean the actual unexpected native mutation on a failing Before, before asserting it.
    if result.is_ok() {
        let undone = applying.undo(&s.job, &Stop::Cancelled).await;
        assert!(
            matches!(undone, Undone::Everything),
            "the native fixture compensation did not restore its exact prior state"
        );
    }
    let granted: Vec<String> =
        sqlx::query_scalar("SELECT granted_capabilities FROM environments WHERE id=$1")
            .bind(s.environment)
            .fetch_one(s.world.pool.pool())
            .await?;
    assert!(!granted.contains(&"kvm".into()));
    assert!(
        matches!(&result,Err(Stop::Failed(failure)) if failure.code=="access.denied"),
        "new forward work used a revoked approval key: result={result:?}, forward_commands={forward}"
    );
    assert_eq!(forward, 0);
    assert_eq!(
        approvals::approval(&mut *s.world.pool.acquire().await?, s.approval)
            .await?
            .ok_or("approval absent")?
            .state,
        "approved"
    );
    s.engine.stop();
    Ok(())
}
#[tokio::test]
async fn actual_accepted_live_apply_replays_and_publishes_its_fact_after_key_revocation()
-> TestResult {
    let s = Fixture::new().await?;
    let applying = s.applying().await?;
    let applied = applying.run(&s.job, "apply").await;
    assert!(
        applied.is_ok(),
        "the ordinary live apply must reach its real scripted node: {applied:?}"
    );
    assert_eq!(s.host.count(CommandKind::NodeCapabilitiesApply), 1);
    s.revoke().await?;
    let replay = applying.run(&s.job, "apply").await;
    assert!(
        replay.is_ok(),
        "the exact retained successful result must not be re-decided: {replay:?}"
    );
    assert_eq!(s.host.count(CommandKind::NodeCapabilitiesApply), 1);
    let committed = applying.run(&s.job, "commit").await;
    assert!(
        committed.is_ok(),
        "publication completes the accepted native fact: {committed:?}"
    );
    let granted: Vec<String> =
        sqlx::query_scalar("SELECT granted_capabilities FROM environments WHERE id=$1")
            .bind(s.environment)
            .fetch_one(s.world.pool.pool())
            .await?;
    assert!(granted.contains(&"kvm".into()));
    s.engine.stop();
    Ok(())
}

#[path = "live_controls/authority.rs"]
mod authority;
