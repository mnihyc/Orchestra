//! Actual applicable-setting disclosure uses the same typed refusal boundary as file creation.

use super::super::{
    limits::{self, DocSettings, Limit},
    live,
};
use super::*;
use crate::key_operations::{
    native::{self, refusal, reply},
    publication, targets,
};
use futures::future::BoxFuture;
use orch_access_contract::permissions::{NodePermission, PermissionName};
use orch_api_contract::{
    document_sources::{self as sources, ConversionSource as Source},
    nodes::NodeRef,
    operation_sources::Declaration,
};
use orch_authz::{Act, Caller, Policy, Principal};
use orch_db::Conn;

#[derive(Clone, Copy)]
pub(super) struct Failure {
    document: Id,
    settings: DocSettings,
    limit: Limit,
}
impl Failure {
    pub(super) fn new(document: Id, settings: DocSettings, limit: Limit) -> Self {
        Self {
            document,
            settings,
            limit,
        }
    }
    pub(super) async fn project(self, ctx: &Ctx, caller: &Caller) -> ApiResult<ApiError> {
        let sparse = limits::too_large_receipt(&self.settings, self.limit)
            .with("scope", Option::<NodeRef>::None);
        if self.settings.scope_of(self.limit).is_none() {
            publication::refusal(ctx, &sparse)?;
            return Ok(sparse);
        }
        refusal::project::<LimitReply>(ctx, caller, self, sparse).await
    }
}
struct LimitReply;
impl reply::Projection for LimitReply {
    type Source = Source;
    type Input = Failure;
    type Subject = Id;
    type Value = NodeRef;
    type Reply = ApiError;
    const READ_SOURCE: Source = Source::LimitNodes;
    fn declaration(_input: &Failure) -> &'static Declaration<Source> {
        &sources::CONVERSION_LIMIT
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Failure,
    ) -> BoxFuture<'a, ApiResult<(Id, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = live(conn, input.document).await?;
            let settings = DocSettings::at(conn, row.node_id).await?;
            if settings.scope_of(input.limit) != input.settings.scope_of(input.limit)
                || settings.value_of(input.limit) != input.settings.value_of(input.limit)
            {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the applicable conversion limit changed",
                ));
            }
            let node = settings
                .scope_of(input.limit)
                .ok_or_else(ApiError::not_found)?;
            let target = targets::node_identity(conn, node).await?;
            let kind = target.object.kind;
            Ok((
                node,
                vec![native::Facts::collection(
                    Source::LimitNodes,
                    vec![(kind, vec![target])],
                )],
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Failure,
        node: &'a Id,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let decision = orch_authz::nodes::check_named(
                conn,
                principal,
                *node,
                PermissionName::OfNode(NodePermission::View),
                Act::Use,
                Policy::default(),
            )
            .await
            .map_err(ApiError::internal)?;
            if !decision.is_allowed() {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the limit's setting place is not readable",
                ));
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Failure,
        node: &'a Id,
    ) -> BoxFuture<'a, ApiResult<(NodeRef, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let shown =
                crate::routes::nodes::view::refs(conn, principal, &[*node], Policy::default())
                    .await?
                    .remove(node)
                    .ok_or_else(ApiError::not_found)?;
            Ok((shown, Vec::new()))
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Failure,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<ApiError>> {
        Box::pin(async move {
            Ok(limits::too_large_receipt(&input.settings, input.limit)
                .with("scope", Some(prepared.value.clone())))
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Failure,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(old.subject == fresh.subject && old.value == fresh.value) })
    }
}
