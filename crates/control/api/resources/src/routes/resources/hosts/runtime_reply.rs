//! Optional content from the exact Host unit whose Stop request completed.

use super::*;
use crate::key_operations::native::reply;
use crate::key_operations::{native, publication, targets};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_api_contract::host_actions::{self, Source};
use orch_api_contract::operation_sources::Declaration;
use orch_link_contract::native_target::RuntimeSource;

pub(super) struct Input {
    row: RuntimeFull,
    receiver: Id,
    source: Option<RuntimeSource>,
}

impl Input {
    pub(super) async fn capture(conn: &mut Conn, row: &RuntimeFull) -> ApiResult<Self> {
        let host = orch_db::queries::resources::hosting::host(conn, row.host_id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        Ok(Self {
            row: row.clone(),
            receiver: host.machine_id,
            source: orch_db::queries::runtime_incarnations::current(conn, row.id).await?,
        })
    }
}

pub(super) async fn render(
    ctx: &Ctx,
    accepted: &orch_authz::Caller,
    selected: Input,
) -> ApiResult<api::HostRuntimeStopResponse> {
    let receipt = api::HostRuntimeStopResponse::Receipt(api::HostRuntimeStopReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        host_id: selected.row.host_id,
        runtime_id: selected.row.id,
    });
    publication::receipt(ctx, &receipt)?;
    Ok(
        match reply::render::<Unit>(ctx, accepted, selected).await? {
            Some(view) => api::HostRuntimeStopResponse::full(view),
            None => receipt,
        },
    )
}

struct Unit;

impl reply::Projection for Unit {
    type Source = Source;
    type Input = Input;
    type Subject = RuntimeFull;
    type Value = Runtime;
    type Reply = Runtime;
    const READ_SOURCE: Source = Source::Host;

    fn declaration(_: &Input) -> &'static Declaration<Source> {
        &host_actions::RUNTIME_STOP
    }

    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Input,
    ) -> native::SourceFuture<'a, RuntimeFull, Source> {
        Box::pin(async move {
            let host = orch_db::queries::resources::hosting::host(conn, input.row.host_id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let row = runtime_of_host(conn, input.row.host_id, input.row.id).await?;
            if host.machine_id != input.receiver
                || row.kind != input.row.kind
                || row.environment_id != input.row.environment_id
                || row.provider_id != input.row.provider_id
                || row.unit_name != input.row.unit_name
                || row.generation != input.row.generation
                || row.driver != input.row.driver
                || row.template_id != input.row.template_id
                || row.spec_hash != input.row.spec_hash
                || orch_db::queries::runtime_incarnations::current(conn, row.id).await?
                    != input.source
            {
                return Err(ApiError::not_found());
            }
            let target = targets::page_object(
                conn,
                ObjectRef {
                    kind: ResourceKind::Host.into(),
                    id: row.host_id,
                },
            )
            .await?;
            Ok((
                row,
                vec![native::Facts::one(
                    Source::Host,
                    KeyTargetKind::Object(ResourceKind::Host.into()),
                    target,
                )],
            ))
        })
    }

    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Input,
        subject: &'a RuntimeFull,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(
            conn,
            principal,
            ResourceKind::Host,
            subject.host_id,
            Permission::HostManage,
        ))
    }

    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        subject: &'a RuntimeFull,
    ) -> native::SourceFuture<'a, Runtime, Source> {
        Box::pin(async move {
            let view = runtime_views(ctx, conn, vec![subject.clone()])
                .await?
                .pop()
                .ok_or_else(|| ApiError::internal("a selected runtime has no projection"))?;
            Ok((view, Vec::new()))
        })
    }

    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Runtime>> {
        Box::pin(async move { Ok(prepared.value.clone()) })
    }

    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened.value == fresh.value) })
    }
}
