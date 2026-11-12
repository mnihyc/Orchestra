//! Held AFTER component controls for the same real producer and claimed job.
//! The leaf checks here are the publication predicate; final gateway/send coverage is separate.
use super::*;
use crate::commands::Outgoing;
use crate::jobs::capability_change::{Plan, authority};
use orch_link_contract::commands::{NodeCapabilitiesApply, SerialKey};

async fn command(s: &Fixture) -> TestResult<Outgoing> {
    let row = s.job.row().await?;
    let plan: Plan = serde_json::from_value(row.plan)?;
    Ok(Outgoing::new(
        crate::ids::derived("capability.live.apply", &[row.id.as_bytes()]),
        s.host.machine,
        SerialKey::Runtime(s.runtime),
        &NodeCapabilitiesApply {
            runtime_id: s.runtime,
            capabilities: json!(plan.after),
        },
    )?)
}
async fn permitted(
    s: &Fixture,
    command: &Outgoing,
) -> TestResult<Option<Vec<(Option<Caller>, Option<Time>)>>> {
    Ok(authority::command_current_callers(
        &mut *s.world.pool.acquire().await?,
        command.id,
        command.machine,
        command.key.wire().as_deref(),
        command.kind.as_str(),
        &command.body,
        s.world.clock.as_ref(),
    )
    .await?)
}

#[tokio::test]
async fn exact_queued_copy_of_the_native_apply_follows_current_key_bounds_and_keeps_real_callers()
-> TestResult {
    let s = Fixture::new().await?;
    let command = command(&s).await?;
    let remembered = authority::before_issue(&s.job, &command, true).await;
    assert!(
        remembered.is_ok(),
        "normal canonical issue must be retained: {remembered:?}"
    );
    let callers = permitted(&s, &command)
        .await?
        .ok_or("current native apply refused")?;
    assert_eq!(callers.len(), 1);
    assert_eq!(
        callers[0]
            .0
            .as_ref()
            .and_then(|caller| caller.grant())
            .and_then(|grant| grant.token()),
        Some(s.token)
    );
    assert_eq!(
        callers[0]
            .0
            .as_ref()
            .and_then(|caller| caller.admitted_operation()),
        Some("ApprovalDecide.capability")
    );
    assert!(
        callers[0].1.is_some(),
        "the real owned runner deadline must survive to send"
    );
    let old: Json = sqlx::query_scalar("SELECT operation_scope FROM api_tokens WHERE id=$1")
        .bind(s.token)
        .fetch_one(s.world.pool.pool())
        .await?;
    let mut reduced = old.clone();
    reduced["rules"][0]["allow"][0]["actions"] = json!(["read"]);
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.token)
        .bind(reduced)
        .execute(s.world.pool.pool())
        .await?;
    assert!(permitted(&s, &command).await?.is_none());
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.token)
        .bind(old)
        .execute(s.world.pool.pool())
        .await?;
    assert!(permitted(&s, &command).await?.is_some());
    assert_eq!(s.host.count(CommandKind::NodeCapabilitiesApply), 0);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn known_queued_apply_never_becomes_maintenance_when_active_or_origin_is_lost() -> TestResult
{
    let s = Fixture::new().await?;
    let command = command(&s).await?;
    let remembered = authority::before_issue(&s.job, &command, true).await;
    assert!(
        remembered.is_ok(),
        "normal canonical issue must be retained: {remembered:?}"
    );
    assert!(permitted(&s, &command).await?.is_some());
    let original = s.job.row().await?.plan;
    sqlx::query("UPDATE jobs SET plan=plan-'active_command' WHERE id=$1")
        .bind(s.job.id())
        .execute(s.world.pool.pool())
        .await?;
    assert!(
        permitted(&s, &command).await?.is_none(),
        "retained journal still recognizes the owned command"
    );
    sqlx::query("UPDATE jobs SET plan=$2 WHERE id=$1")
        .bind(s.job.id())
        .bind(original)
        .execute(s.world.pool.pool())
        .await?;
    sqlx::query("UPDATE jobs SET plan=plan-'approval_origin' WHERE id=$1")
        .bind(s.job.id())
        .execute(s.world.pool.pool())
        .await?;
    assert!(
        permitted(&s, &command).await?.is_none(),
        "known missing origin cannot adopt an unrestricted actor"
    );
    assert_eq!(s.host.count(CommandKind::NodeCapabilitiesApply), 0);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn actual_success_does_not_loan_its_receipt_to_a_changed_private_journal() -> TestResult {
    let s = Fixture::new().await?;
    let applying = s.applying().await?;
    let result = applying.run(&s.job, "apply").await;
    assert!(result.is_ok(), "real native apply must succeed: {result:?}");
    assert_eq!(s.host.count(CommandKind::NodeCapabilitiesApply), 1);
    let command = command(&s).await?;
    let before = s.job.row().await?.plan;
    sqlx::query("UPDATE jobs SET plan=jsonb_set(plan,ARRAY['capability_commands',$2,'body_hash'],'\"changed journal\"'::jsonb) WHERE id=$1")
        .bind(s.job.id()).bind(command.id.to_string()).execute(s.world.pool.pool()).await?;
    let replay = applying.run(&s.job, "apply").await;
    // Restore the fixture's exact immutable evidence before normal inverse cleanup.
    sqlx::query("UPDATE jobs SET plan=$2 WHERE id=$1")
        .bind(s.job.id())
        .bind(before)
        .execute(s.world.pool.pool())
        .await?;
    let undone = applying.undo(&s.job, &Stop::Cancelled).await;
    assert!(
        matches!(undone, Undone::Everything),
        "owned cleanup failed: {undone:?}"
    );
    assert!(
        matches!(replay, Err(Stop::Failed(ref failure)) if failure.code=="capability_identity"),
        "Accepted status borrowed foreign journal evidence: {replay:?}"
    );
    assert_eq!(s.host.count(CommandKind::NodeCapabilitiesApply), 2);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn accepted_live_apply_can_remove_its_exact_owned_grant_after_the_key_is_revoked()
-> TestResult {
    let s = Fixture::new().await?;
    let applying = s.applying().await?;
    let result = applying.run(&s.job, "apply").await;
    assert!(result.is_ok(), "real native apply must succeed: {result:?}");
    s.revoke().await?;
    let undone = applying.undo(&s.job, &Stop::Cancelled).await;
    assert!(
        matches!(undone, Undone::Everything),
        "key revocation blocked owned inverse cleanup: {undone:?}"
    );
    assert_eq!(s.host.count(CommandKind::NodeCapabilitiesApply), 2);
    let granted: Vec<String> =
        sqlx::query_scalar("SELECT granted_capabilities FROM environments WHERE id=$1")
            .bind(s.environment)
            .fetch_one(s.world.pool.pool())
            .await?;
    assert!(!granted.contains(&"kvm".into()));
    s.engine.stop();
    Ok(())
}
