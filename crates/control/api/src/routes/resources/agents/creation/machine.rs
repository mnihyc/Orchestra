//! Machine-backed registration consumes stored native reports and the shared profile writer.

use super::*;
use crate::routes::resources::machines;
use orch_api_contract::resources::{MachineAgentCreateBody, MachineAgentCreateResponse};
use orch_db::queries::resources as machine_rows;

pub(in crate::routes::resources::agents) struct MachineProfiles;

pub(super) async fn machine_fact(conn: &mut Conn, machine: Id) -> ApiResult<native::Facts<Source>> {
    let target = crate::key_operations::targets::machine_subjects(conn, &[machine])
        .await?
        .remove(&machine)
        .ok_or_else(ApiError::not_found)?;
    Ok(native::Facts::one(
        Source::Machine,
        KeyTargetKind::Object(ObjectKind::Machine),
        target,
    ))
}
async fn facts(conn: &mut Conn, machine: Id, home: Id) -> ApiResult<Vec<native::Facts<Source>>> {
    Ok(vec![
        machine_fact(conn, machine).await?,
        native::Facts::one(
            Source::Home,
            AGENT,
            targets::node(conn, home).await.map_err(target_error)?,
        ),
    ])
}
async fn require(
    conn: &mut Conn,
    principal: &Principal,
    machine: Id,
    home: Id,
    harness: &str,
) -> ApiResult<machine_rows::MachineRow> {
    let row = machine_rows::machine(conn, machine)
        .await?
        .filter(|row| row.deleted_at.is_none() && row.is_active())
        .ok_or_else(ApiError::not_found)?;
    machines::require_machine(conn, principal, machine, Permission::MachineView).await?;
    machines::require_machine(conn, principal, machine, Permission::MachineUse).await?;
    crate::backplane::require_facts(&row.facts)?;
    super::super::registration_home(conn, principal, home, "home_id").await?;
    if !row.roles.iter().any(|role| role == "agent")
        || !super::super::machine_reports(&row.facts, harness)
    {
        return Err(crate::routes::nodes::invalid(
            "harness_kind",
            "this machine did not report that it can run this harness",
        ));
    }
    Ok(row)
}
impl effects::Effect for MachineProfiles {
    type Source = Source;
    type Query = ();
    type Body = MachineAgentCreateBody;
    type Value = Proposal;
    type Output = Created;
    fn declaration(_value: &Proposal) -> &'static Declaration<Source> {
        &sources::MACHINE_CREATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: MachineAgentCreateBody,
    ) -> BoxFuture<'a, ApiResult<(Proposal, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let machine = ctx.id("id")?;
            let name = clean_name("name", &body.name)?;
            let harness = clean_harness_kind("harness_kind", &body.harness_kind)?;
            let profile =
                clean_profile("profile", body.profile.as_ref().unwrap_or(&json!({})), true)?;
            let initial = facts(conn, machine, body.home_id).await?;
            let known = native::bind_known(ctx, &sources::MACHINE_CREATE, &initial)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            require(conn, &principal, machine, body.home_id, &harness).await?;
            sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR SHARE")
                .bind(body.home_id)
                .fetch_one(&mut *conn)
                .await
                .map_err(DbError::from)?;
            sqlx::query("SELECT id FROM machines WHERE id=$1 FOR UPDATE")
                .bind(machine)
                .fetch_one(&mut *conn)
                .await
                .map_err(DbError::from)?;
            sqlx::query("SELECT id FROM hosts WHERE machine_id=$1 FOR SHARE")
                .bind(machine)
                .fetch_all(&mut *conn)
                .await
                .map_err(DbError::from)?;
            let actual = facts(conn, machine, body.home_id).await?;
            let known = native::bind_known(ctx, &sources::MACHINE_CREATE, &actual)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let row = require(conn, &principal, machine, body.home_id, &harness).await?;
            let place =
                super::super::registration_home(conn, &principal, body.home_id, "home_id").await?;
            Ok((
                Proposal {
                    id: new_id(),
                    home: body.home_id,
                    root: place.org,
                    name,
                    harness,
                    profile,
                    machine: Some((machine, row.name)),
                },
                actual,
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Proposal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let (machine, _) = value.machine.as_ref().ok_or_else(|| {
                ApiError::internal("the native machine registration has no machine")
            })?;
            require(conn, principal, *machine, value.home, &value.harness).await?;
            Ok(())
        })
    }
}
impl effects::ApplyOwned for MachineProfiles {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Proposal,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Created>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            let made =
                super::insert(ctx, &mut tx, principal, value, move || before.check()).await?;
            deadline.check()?;
            let identity = transaction_guard::current_http_identity(
                ctx,
                &mut tx,
                transaction_guard::HttpIdentityMode::Read,
            )
            .await?;
            identity.check_deadline(&ctx.app)?;
            deadline.check()?;
            tx.commit().await.map_err(DbError::from)?;
            Ok(effects::Outcome::Applied(made))
        })
    }
}
pub(in crate::routes::resources::agents) async fn create(
    ctx: Ctx,
    (): (),
    _body: MachineAgentCreateBody,
) -> ApiResult<MachineAgentCreateResponse> {
    let (tx, _initial) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply_owned::<MachineProfiles>(&ctx, tx).await?;
    Ok(match super::finish_created(completed).await? {
        OrgAgentCreateResponse::Full(agent) => MachineAgentCreateResponse::Full(agent),
        OrgAgentCreateResponse::Receipt(receipt) => MachineAgentCreateResponse::Receipt(receipt),
    })
}
