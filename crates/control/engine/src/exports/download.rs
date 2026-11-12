//! Actual archive-open sources for the shared continuing-operation kernel.

use super::{PreparedExportOpen, prepare_export_open_for_row};
use crate::operation_scope::{
    continuing::{self, Error},
    sources::Facts,
    targets::{self, TargetError},
};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyTargetKind};
use orch_api_contract::{
    artifact_sources::{EXPORT_DOWNLOAD, Source as Role},
    exports::ExportDownload,
    operation_sources::Declaration,
    routes::{Endpoint, Route},
};
use orch_authz::{Caller, Principal};
use orch_db::{Conn, queries::exports};
use orch_types::Time;

/// Actual opened native association, never an invoker or a reusable authority.
pub struct Input {
    original: PreparedExportOpen,
}
impl Input {
    /// Keep exactly the ready native archive selected by the caller's first source frame.
    pub fn capture(selected: &PreparedExportOpen) -> Self {
        Self {
            original: selected.clone(),
        }
    }
}

/// The native source supplies real rows; the shared kernel owns all operation/key admission.
pub struct Archive;
impl continuing::Source for Archive {
    type Role = Role;
    type Input = Input;
    type Value = PreparedExportOpen;
    const ROUTE: Route = ExportDownload::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &EXPORT_DOWNLOAD;

    fn prepare<'a>(
        conn: &'a mut Conn,
        caller: &'a Caller,
        input: &'a Input,
        now: Time,
    ) -> BoxFuture<'a, Result<(PreparedExportOpen, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let row = exports::get(conn, input.original.row().id).await?;
            let principal = Principal::load(conn, caller.clone(), now).await?;
            let value = prepare_export_open_for_row(row, &principal).map_err(|_| Error::Refused)?;
            let target =
                targets::from_export(conn, value.row())
                    .await
                    .map_err(|error| match error {
                        TargetError::Missing => Error::Refused,
                        TargetError::Db(error) => Error::from(error),
                        TargetError::Authz(error) => Error::from(error),
                    })?;
            let facts = vec![Facts::one(
                Role::Artifact,
                KeyTargetKind::Identity(KeyIdentityKind::Export),
                target,
            )];
            Ok((value, facts))
        })
    }

    fn require<'a>(
        _conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        value: &'a PreparedExportOpen,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if !crate::archive::may_read_custody(principal, value.row().user_id)
                || !input.original.preserves(value)
            {
                return Err(Error::Refused);
            }
            Ok(())
        })
    }

    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Input,
        value: &'a PreparedExportOpen,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            exports::lock(conn, value.row().id)
                .await?
                .ok_or(Error::Refused)?;
            Ok(())
        })
    }

    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Input,
        opened: &'a PreparedExportOpen,
        fresh: &'a PreparedExportOpen,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(input.original.preserves(opened) && opened.preserves(fresh)) })
    }
}
