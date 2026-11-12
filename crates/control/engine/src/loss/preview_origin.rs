//! The original saved review remains a separate current operation at confirmation.

use super::*;
use crate::operation_scope::{
    continuing::{self, Error, Source},
    sources::Facts,
    targets,
};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_api_contract::{
    loss_sources::{PREFLIGHT, Source as Role},
    operation_sources::Declaration,
    routes::{Endpoint, Route},
};
use orch_authz::Caller;
use orch_types::Clock;

pub(super) struct Preview;
pub(super) struct Input {
    row: jobs::forks::PreflightRow,
    target: LossTarget,
}
pub(super) fn refused(error: Error) -> LossError {
    match error {
        Error::Refused | Error::Binding(_) => LossError::Denied,
        Error::Native(error) => LossError::Internal(error),
    }
}
pub(super) async fn prepare(
    conn: &mut Conn,
    clock: &dyn Clock,
    origin: &crate::operation_scope::http_origin::HttpOrigin,
    row: &jobs::forks::PreflightRow,
    target: LossTarget,
) -> Result<continuing::Frame<Preview>, LossError> {
    continuing::checkpoint::<Preview>(
        conn,
        clock,
        origin,
        Input {
            row: row.clone(),
            target,
        },
    )
    .await
    .map_err(refused)
}
impl Source for Preview {
    type Role = Role;
    type Input = Input;
    type Value = KeyResolvedTarget;
    const ROUTE: Route = LossPreflight::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &PREFLIGHT;
    fn prepare<'a>(
        conn: &'a mut Conn,
        caller: &'a Caller,
        input: &'a Input,
        now: Time,
    ) -> BoxFuture<'a, Result<(KeyResolvedTarget, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let row = jobs::forks::preflight(conn, input.row.id)
                .await?
                .ok_or(Error::Refused)?;
            if row != input.row || row.created_by != caller.user() || row.expires_at <= now.get() {
                return Err(Error::Refused);
            }
            let object = input.target.object();
            let target = if object.kind == ObjectKind::Machine {
                targets::machine_subject(conn, object.id).await
            } else {
                targets::resource_identity(conn, object).await
            }
            .map_err(|error| Error::Native(EngineError::unreadable(error)))?;
            Ok((
                target.clone(),
                vec![Facts::one(
                    Role::Target,
                    KeyTargetKind::Object(object.kind),
                    target,
                )],
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        _: &'a KeyResolvedTarget,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if allowed(conn, principal, input.target)
                .await
                .map_err(|error| Error::Native(EngineError::unreadable(error)))?
            {
                Ok(())
            } else {
                Err(Error::Refused)
            }
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Input,
        _: &'a KeyResolvedTarget,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            jobs::changes::lock_plan(conn, input.row.id)
                .await?
                .ok_or(Error::Refused)?;
            if !orch_db::queries::resources::lock_resource(
                conn,
                input.target.kind(),
                input.target.id(),
            )
            .await?
            {
                return Err(Error::Refused);
            }
            Ok(())
        })
    }
    fn check_opened<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        opened: &'a KeyResolvedTarget,
        fresh: &'a KeyResolvedTarget,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(opened == fresh) })
    }
}
