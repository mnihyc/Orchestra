//! One native current-source factory for prompt-transition admission and deferred work.
use super::{
    admission,
    recipe::{Kind, Recipe},
};
use crate::{
    EngineError,
    operation_scope::{
        self,
        continuing::{self, Error, Source},
        sources::Facts,
        targets,
    },
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::{KeyResolvedTarget, KeyTargetKind},
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    operation_sources::Declaration,
    routes::{Endpoint, Route},
    thread_sources::{self as sources, Source as Role},
    threads::{ThreadContinuationActivate, ThreadPromptVersionCreate},
};
use orch_authz::{Act, Caller, Policy, Principal};
use orch_db::{
    Conn,
    queries::{jobs, threads},
};
use orch_types::{Clock, Time};
use std::marker::PhantomData;

pub trait Intent: Send + Sync + 'static {
    const ROUTE: Route;
    const DECLARATION: &'static Declaration<Role>;
    const KIND: Kind;
}
pub struct Edit;
pub struct Activate;
impl Intent for Edit {
    const ROUTE: Route = ThreadPromptVersionCreate::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &sources::PROMPT_CREATE;
    const KIND: Kind = Kind::Edit;
}
impl Intent for Activate {
    const ROUTE: Route = ThreadContinuationActivate::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &sources::CONTINUATION_ACTIVATE;
    const KIND: Kind = Kind::Activate;
}

pub fn byte_facts(bindings: &[operation_scope::blob_sources::Binding]) -> Vec<Facts<Role>> {
    use operation_scope::blob_sources::SourceKind;
    [
        (Role::ContinuationChats, SourceKind::Chat),
        (Role::ContinuationThreads, SourceKind::Thread),
        (Role::ContinuationDocuments, SourceKind::Document),
        (Role::ContinuationEnvironments, SourceKind::Environment),
        (Role::ContinuationUploads, SourceKind::RetainedUpload),
    ]
    .into_iter()
    .map(|(source, kind)| {
        let values: Vec<_> = bindings
            .iter()
            .filter(|binding| binding.source == kind)
            .map(|binding| binding.target.clone())
            .collect();
        Facts::collection(
            source,
            if values.is_empty() {
                Vec::new()
            } else {
                vec![(kind.kind(), values)]
            },
        )
    })
    .collect()
}
#[derive(Clone)]
pub struct Input {
    pub recipe: Recipe,
}
pub struct Value {
    target: KeyResolvedTarget,
    selected: Vec<(orch_types::Hash, orch_chat::following::BlobOrigin)>,
}
pub struct Work<I>(PhantomData<I>);
fn target_error(error: targets::TargetError) -> Error {
    match error {
        targets::TargetError::Missing
        | targets::TargetError::Authz(orch_authz::Error::NotFound(_)) => Error::Refused,
        targets::TargetError::Db(error) => error.into(),
        targets::TargetError::Authz(error) => error.into(),
    }
}
fn native_error(error: super::Error) -> Error {
    Error::Native(match error {
        super::Error::Internal(error) => error,
        other => EngineError::unreadable(other),
    })
}

impl<I: Intent> Source for Work<I> {
    type Role = Role;
    type Input = Input;
    type Value = Value;
    const ROUTE: Route = I::ROUTE;
    const DECLARATION: &'static Declaration<Role> = I::DECLARATION;
    fn prepare<'a>(
        conn: &'a mut Conn,
        caller: &'a Caller,
        input: &'a Input,
        now: Time,
    ) -> BoxFuture<'a, Result<(Value, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let recipe = &input.recipe;
            if recipe.request.kind != I::KIND || recipe.actor_id != caller.user() {
                return Err(Error::Refused);
            }
            let stored = jobs::job(conn, recipe.operation_id)
                .await?
                .ok_or(Error::Refused)?;
            let actual: Recipe =
                serde_json::from_value(stored.plan["recipe"].clone()).map_err(EngineError::from)?;
            if actual.operation_id != recipe.operation_id
                || actual.thread_id != recipe.thread_id
                || actual.actor_id != recipe.actor_id
                || actual.request_hash != recipe.request_hash
                || actual.request != recipe.request
                || actual.source_lane != recipe.source_lane
                || actual.segment_id != recipe.segment_id
                || actual.target_continuation_id != recipe.target_continuation_id
                || actual.operation_origin != recipe.operation_origin
                || actual.byte_sources != recipe.byte_sources
                || stored.node_id != Some(recipe.thread_id)
            {
                return Err(Error::Refused);
            }
            let lane = threads::segment(conn, recipe.source_lane.source.segment_id)
                .await?
                .ok_or(Error::Refused)?;
            if lane.thread_id != recipe.thread_id
                || lane.continuation_id != recipe.source_lane.continuation_id
                || lane.native_session_ref.as_deref()
                    != Some(recipe.source_lane.source.native_session_ref.as_str())
            {
                return Err(Error::Refused);
            }
            let target = targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Thread,
                    id: recipe.thread_id,
                },
            )
            .await
            .map_err(target_error)?;
            let primary = Facts::one(
                Role::Thread,
                KeyTargetKind::Object(ObjectKind::Thread),
                target.clone(),
            );
            let base = operation_scope::sources::bind_known(
                I::ROUTE,
                I::DECLARATION,
                std::slice::from_ref(&primary),
            )?;
            let preparing = base.preparation_caller(caller)?;
            let principal = Principal::load(conn, preparing, now).await?;
            let hashes = recipe
                .request
                .content
                .as_deref()
                .map(orch_chat::following::content_blobs)
                .unwrap_or_default();
            let prepared = operation_scope::blob_sources::prepare_selected(
                conn,
                &principal,
                &base,
                &hashes,
                &[],
                &recipe.byte_sources,
            )
            .await
            .map_err(EngineError::unreadable)?;
            let mut facts = vec![primary];
            facts.extend(byte_facts(&prepared.bindings));
            Ok((
                Value {
                    target,
                    selected: prepared.selected,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        _value: &'a Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            admission::require_view(conn, principal, input.recipe.thread_id)
                .await
                .map_err(native_error)?;
            if orch_authz::nodes::check(
                conn,
                principal,
                input.recipe.thread_id,
                Permission::ThreadInteract,
                Act::Use,
                Policy::default(),
            )
            .await?
            .is_allowed()
            {
                Ok(())
            } else {
                Err(Error::Refused)
            }
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Input,
        _value: &'a Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            threads::lock(conn, input.recipe.thread_id)
                .await?
                .ok_or(Error::Refused)?;
            jobs::lock(conn, input.recipe.operation_id)
                .await?
                .ok_or(Error::Refused)?;
            let hashes = input
                .recipe
                .request
                .content
                .as_deref()
                .map(orch_chat::following::content_blobs)
                .unwrap_or_default();
            orch_db::queries::uploads::lock_blobs(conn, &hashes).await?;
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
        Box::pin(async move { Ok(old.target == fresh.target && old.selected == fresh.selected) })
    }
}

/// A known original never degrades to the legacy user-only worker path.
pub(crate) async fn principal(
    conn: &mut Conn,
    clock: &dyn Clock,
    recipe: &Recipe,
) -> Result<Option<Principal>, EngineError> {
    let Some(origin) = &recipe.operation_origin else {
        return Ok(None);
    };
    let input = Input {
        recipe: recipe.clone(),
    };
    let result = match recipe.request.kind {
        Kind::Edit => continuing::prepare::<Work<Edit>>(conn, clock, origin, input)
            .await
            .map(|frame| frame.into_parts().principal),
        Kind::Activate => continuing::prepare::<Work<Activate>>(conn, clock, origin, input)
            .await
            .map(|frame| frame.into_parts().principal),
    };
    result.map(Some).map_err(|error| match error {
        Error::Native(error) => error,
        other => EngineError::BeforeEffect(Box::new(other)),
    })
}
