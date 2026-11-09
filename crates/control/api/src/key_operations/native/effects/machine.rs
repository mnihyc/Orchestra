//! Native E entry for an authenticated connected machine's current owner.
use super::*;
use crate::{
    middleware::authentication::{Authed, Caller as TransportCaller, Via},
    state::App,
};
use orch_api_contract::{resources::McpServerDelete, routes::Endpoint};
use orch_db::queries::{identity, resources, transaction_guard as rows};
use orch_types::Id;

/// Transaction-pinned actual machine/owner authority, never an HTTP credential or new lease.
pub(super) struct Authority {
    machine: Id,
    owner: Id,
    principal: Principal,
}
impl Authority {
    pub(super) fn check(&self) -> ApiResult<()> {
        if self.machine.is_nil()
            || self.owner.is_nil()
            || self.principal.user() != self.owner
            || self.principal.caller().native_machine() != Some(self.machine)
            || self.principal.caller().as_admin()
            || self.principal.caller().admitted_operation() != Some("McpServerDelete")
        {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "The native machine-owner deletion boundary is closed.",
            ));
        }
        Ok(())
    }
}
async fn owner(conn: &mut Conn, machine: Id) -> ApiResult<orch_db::queries::identity::UserRow> {
    let _ = sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
        .bind(machine)
        .fetch_optional(&mut *conn)
        .await
        .map_err(DbError::from)?;
    let current = resources::machine(conn, machine)
        .await?
        .filter(|row| row.placement == "connected" && row.is_active())
        .ok_or_else(|| {
            ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "Only an active connected machine acts as its current owner.",
            )
        })?;
    let user = current.owner_id.ok_or_else(|| {
        ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "The machine has no current owner.",
        )
    })?;
    let _ = sqlx::query("SELECT id FROM users WHERE id=$1 FOR SHARE")
        .bind(user)
        .fetch_optional(&mut *conn)
        .await
        .map_err(DbError::from)?;
    identity::user_by_id(conn, user)
        .await?
        .filter(|user| user.status == "active")
        .ok_or_else(|| {
            ApiError::new(
                orch_api_contract::common::ErrorCode::AuthSuspended,
                "The machine's current owner is unavailable.",
            )
        })
}
async fn imported_here(conn: &mut Conn, machine: Id, server: Id) -> ApiResult<()> {
    let current = resources::mcp::servers(conn, &[server])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    if !current.imported
        || !resources::mcp::offerings(conn, &[server])
            .await?
            .iter()
            .any(|copy| copy.machine_id == machine && copy.state == "ready")
    {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "This imported server is not canonically recorded on the requesting machine.",
        ));
    }
    Ok(())
}

/// The machine id comes only from the authenticated link's Handler argument, never its body.
/// The domain supplies its SAME Delete producer/writer; all facts and ordinary gates are fresh.
/// Body/query have no invoking-account/HTTP-token authority. This entry is closed to the one
/// native imported-server deletion act, and keeps the actual caller-owned transaction.
pub(crate) async fn apply_owned<F: ApplyOwned>(
    app: &App,
    mut tx: orch_db::Tx,
    machine: Id,
    server: Id,
    request_id: Id,
    query: F::Query,
    body: F::Body,
) -> ApiResult<Completed<F::Output>> {
    if machine.is_nil() || server.is_nil() || request_id.is_nil() || !rows::in_transaction(&mut tx)
    {
        return Err(declaration_error());
    }
    rows::lock_authority(&mut tx).await?;
    let user = owner(&mut tx, machine).await?;
    let account = user.id;
    imported_here(&mut tx, machine, server).await?;
    let transaction: i64 = sqlx::query_scalar("SELECT txid_current()::bigint")
        .fetch_one(&mut *tx)
        .await
        .map_err(DbError::from)?;
    let caller = Caller::machine_owner(machine, account, transaction);
    // This DTO exists only for existing projection/audit/account-policy inputs. The authority
    // is the marked machine caller above; no nil token enters an HTTP credential guard.
    let display = Authed {
        user,
        as_admin: false,
        via: Via::Token {
            token_id: Id::nil(),
            scopes: vec!["*".into()],
            node_id: None,
            operation_scope: None,
            expires_at: None,
        },
    };
    let mut ctx = Ctx::for_tool(
        app.clone(),
        TransportCaller::User(Arc::new(display)),
        McpServerDelete::ROUTE,
        server,
    );
    ctx.request_id = format!("machine-native:{machine}:{request_id}");
    ctx.now = app.clock.now();
    let (value, facts) = F::prepare(&ctx, &mut tx, &caller, query, body).await?;
    if F::declaration(&value).operation != "McpServerDelete" {
        return Err(declaration_error());
    }
    let fresh = owner(&mut tx, machine).await?;
    if fresh.id != account {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "The machine changed owner while preparing its deletion.",
        ));
    }
    imported_here(&mut tx, machine, server).await?;
    let response = bind_required(&ctx, F::declaration(&value), &facts)?;
    let admitted = response
        .shared()
        .admit(&caller, false)
        .map_err(admission::binding_error)?;
    let principal = authorization::load_principal(&mut tx, admitted, app.clock.time()).await?;
    let principal = crate::sign_in::second_step::hold(&mut tx, ctx.user()?, principal).await?;
    F::require(&ctx, &mut tx, &principal, &value).await?;
    let authority = Authority {
        machine,
        owner: account,
        principal,
    };
    authority.check()?;
    let checked = AtomicBool::new(false);
    let before = FirstEffect {
        boundary: Boundary::Machine(&authority),
        checked: &checked,
    };
    let outcome = F::apply_owned(&ctx, tx, &authority.principal, &value, before).await?;
    finish_native(ctx, authority.principal.caller().clone(), &checked, outcome)
}
