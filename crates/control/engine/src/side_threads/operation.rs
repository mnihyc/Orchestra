//! Current original-operation source of a side's frozen history and native opening.
use super::*;
use crate::operation_scope::{
    continuing::{self, Error, Source},
    sources::Facts,
    targets,
};
use futures::future::BoxFuture;
use orch_access_contract::{
    key_scope::KeyTargetKind,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    operation_sources::Declaration,
    routes::{Endpoint, Route},
    thread_sources::{self as sources, Source as Role},
    threads::ThreadSideCreate,
};
use orch_authz::CapturedOperation;
use orch_db::{Conn, queries::thread_history_cuts as cuts};
use orch_types::Clock;

#[derive(Clone)]
pub struct Input {
    side: Id,
    origin: CapturedOperation,
    continuation: Id,
    segment: Id,
}
pub struct Value {
    target: orch_access_contract::key_scope::KeyResolvedTarget,
}
pub struct Work;
impl Source for Work {
    type Role = Role;
    type Input = Input;
    type Value = Value;
    const ROUTE: Route = ThreadSideCreate::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &sources::SIDE_CREATE;
    fn prepare<'a>(
        conn: &'a mut Conn,
        caller: &'a Caller,
        input: &'a Input,
        _now: Time,
    ) -> BoxFuture<'a, Result<(Value, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let (side, cut, auth) = original(conn, input.side).await?.ok_or(Error::Refused)?;
            if auth.operation_origin.as_ref() != Some(&input.origin)
                || auth.actor != caller.user()
                || auth.continuation != input.continuation
                || auth.segment != input.segment
                || side.active_continuation_id != auth.continuation
            {
                return Err(Error::Refused);
            }
            let initial = threads::segment(conn, auth.segment)
                .await?
                .ok_or(Error::Refused)?;
            if initial.thread_id != side.node_id
                || initial.continuation_id != auth.continuation
                || Some(cut.thread_id) != side.side_of
            {
                return Err(Error::Refused);
            }
            let target = targets::page_object(
                conn,
                orch_access_contract::decisions::ObjectRef {
                    kind: ObjectKind::Thread,
                    id: cut.thread_id,
                },
            )
            .await
            .map_err(|error| Error::Native(EngineError::unreadable(error)))?;
            let mut facts = vec![Facts::one(
                Role::Thread,
                KeyTargetKind::Object(ObjectKind::Thread),
                target.clone(),
            )];
            // This phase reads captured parent history, never the supplied authored-input bytes.
            facts.extend(crate::continuations::operation::byte_facts(&[]));
            Ok((Value { target }, facts))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Input,
        value: &'a Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let parent = value.target.object.id.ok_or(Error::Refused)?;
            for permission in [Permission::ThreadView, Permission::ThreadInteract] {
                if !nodes::check(
                    conn,
                    principal,
                    parent,
                    permission,
                    Act::Use,
                    orch_authz::Policy::default(),
                )
                .await?
                .is_allowed()
                {
                    return Err(Error::Refused);
                }
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Input,
        _value: &'a Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            threads::lock(conn, input.side)
                .await?
                .filter(|row| row.deleted_at.is_none() && row.archived_at.is_none())
                .ok_or(Error::Refused)?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Input,
        old: &'a Value,
        fresh: &'a Value,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(old.target == fresh.target) })
    }
}
pub(super) async fn original(
    conn: &mut Conn,
    side: Id,
) -> Result<Option<(threads::ThreadRow, cuts::Cut, history::Authorization)>, EngineError> {
    let Some(row) = threads::thread(conn, side).await?.filter(|row| {
        row.side_of.is_some() && row.deleted_at.is_none() && row.archived_at.is_none()
    }) else {
        return Ok(None);
    };
    let Some(cut) = cuts::cut(conn, side)
        .await?
        .filter(|cut| cut.owner_thread_id == Some(side) && Some(cut.thread_id) == row.side_of)
    else {
        return Ok(None);
    };
    let auth = serde_json::from_value(cut.artifacts["side_authorization"].clone())?;
    Ok(Some((row, cut, auth)))
}
pub(super) async fn principal(
    conn: &mut Conn,
    clock: &dyn Clock,
    side: Id,
) -> Result<Option<Principal>, EngineError> {
    let (_, _, auth) = original(conn, side)
        .await?
        .ok_or_else(|| EngineError::unreadable("The original side source is unavailable."))?;
    let Some(origin) = auth.operation_origin else {
        return Ok(None);
    };
    let input = Input {
        side,
        origin: origin.clone(),
        continuation: auth.continuation,
        segment: auth.segment,
    };
    continuing::prepare::<Work>(conn, clock, &origin, input)
        .await
        .map(|frame| Some(frame.into_parts().principal))
        .map_err(|error| match error {
            Error::Native(error) => error,
            other => EngineError::BeforeEffect(Box::new(other)),
        })
}
/// The common frozen-history reader can also consume the side source without inventing an actor.
pub(crate) async fn checkpoint(engine: &Engine, cut: &cuts::Cut) -> Result<(), EngineError> {
    if cut.artifacts.get("side_authorization").is_none() {
        return Ok(());
    }
    let side = cut
        .owner_thread_id
        .filter(|side| *side == cut.id)
        .ok_or_else(|| EngineError::unreadable("The side history owner changed."))?;
    let mut conn = engine.db().acquire().await?;
    if let Some(principal) = principal(&mut conn, engine.inner.clock.as_ref(), side).await? {
        if !principal
            .caller()
            .key_deadline_allows(engine.inner.clock.as_ref())
        {
            return Err(EngineError::unreadable(
                "The original side credential ended.",
            ));
        }
    }
    Ok(())
}
