//! Stored native recording metadata; its supplier and read audit run after snapshot success.

use super::*;
use crate::key_operations::{native, targets};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_api_contract::{
    operation_sources::Declaration,
    terminal_sources::{self as sources, Source},
};
use orch_authz::{
    Caller,
    environments::{RecordingAuthority, recording_authority},
};

pub(super) struct Recording;

#[derive(Clone)]
pub(super) struct Subject {
    pub(super) row: SessionRow,
    pub(super) authority: RecordingAuthority,
}

pub(super) struct Read {
    pub(super) subject: Subject,
    pub(super) caller: Caller,
}

fn shell(row: &SessionRow) -> Shell {
    Shell {
        environment: row.environment_id,
        thread: row.thread_id,
        opened_by: row.opened_by,
    }
}

impl native::Selected for Recording {
    type Source = Source;
    type Query = ();
    type Value = Subject;
    type Reply = Read;
    const DECLARATION: &'static Declaration<Source> = &sources::RECORDING;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a (),
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = rows::get(conn, ctx.id("terminal_id")?)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if row.is_live() || row.recording().is_none() {
                return Err(ApiError::not_found());
            }
            let target = targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: row.environment_id,
                },
            )
            .await?;
            let mut facts = vec![native::Facts::one(
                Source::SessionEnvironment,
                KeyTargetKind::Object(ObjectKind::Environment),
                target,
            )];
            let known = native::bind_known(ctx, &sources::RECORDING, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let first = recording_authority(conn, &principal, &shell(&row), Policy::default())
                .await
                .map_err(ApiError::internal)?;
            let authority = match first {
                Some(RecordingAuthority::Opener | RecordingAuthority::Environment) => {
                    facts.push(native::Facts::collection(
                        Source::RecordingThread,
                        Vec::new(),
                    ));
                    first.ok_or_else(ApiError::not_found)?
                }
                Some(RecordingAuthority::Thread) | None => {
                    let thread = row.thread_id.ok_or_else(ApiError::not_found)?;
                    let target = targets::node(conn, thread).await?;
                    facts.push(native::Facts::collection(
                        Source::RecordingThread,
                        vec![(KeyTargetKind::Object(ObjectKind::Thread), vec![target])],
                    ));
                    let known = native::bind_required(ctx, &sources::RECORDING, &facts)?;
                    let principal =
                        native::preparation_principal(ctx, conn, caller, &known).await?;
                    match recording_authority(conn, &principal, &shell(&row), Policy::default())
                        .await
                        .map_err(ApiError::internal)?
                    {
                        Some(RecordingAuthority::Thread) => RecordingAuthority::Thread,
                        _ => return Err(ApiError::not_found()),
                    }
                }
            };
            Ok((Subject { row, authority }, facts))
        })
    }

    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<Read>> {
        Box::pin(async move {
            if recording_authority(conn, principal, &shell(&value.row), Policy::default())
                .await
                .map_err(ApiError::internal)?
                != Some(value.authority)
            {
                return Err(ApiError::not_found());
            }
            Ok(Read {
                subject: value.clone(),
                caller: principal.caller().clone(),
            })
        })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a (),
        opened: &'a Subject,
        fresh: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened.row == fresh.row && opened.authority == fresh.authority) })
    }
}

/// Modern reads already retain the producing Selected value in the shared adapter.
/// The ordinary byte supplier also keeps its exact successful native source and origin.
pub(super) fn retain_ordinary(ctx: &Ctx, read: &Read) -> ApiResult<()> {
    if native::modern(&read.caller) {
        return Ok(());
    }
    crate::key_operations::publication::retain(
        ctx,
        &read.caller,
        read.subject.clone(),
        |ctx, conn, current, opened| {
            Box::pin(async move {
                let (fresh, _) =
                    <Recording as native::Selected>::select(ctx, conn, current.caller(), &())
                        .await?;
                if fresh.row != opened.row || fresh.authority != opened.authority {
                    return Err(ApiError::not_found());
                }
                Ok(())
            })
        },
    )
}
