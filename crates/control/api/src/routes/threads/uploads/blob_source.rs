//! The original native stored-byte planner supplies the upload reader's one retained source.

use crate::key_operations::{blob_sources, native};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationPlan};
use futures::future::BoxFuture;
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::threads::UploadBlobGetQuery;
use orch_api_contract::upload_sources::{self as declarations, Source};
use orch_authz::{Caller, Principal};
use orch_chat::following::BlobOrigin;
use orch_db::Conn;
use orch_types::Hash;
use std::sync::Arc;

#[derive(Clone, PartialEq, Eq)]
struct Witness {
    hash: Hash,
    selected: Vec<(Hash, BlobOrigin)>,
}

pub(super) struct Selection {
    pub(super) hash: Hash,
    pub(super) name: String,
    witness: Witness,
}

pub(super) fn retain(ctx: &Ctx, selected: &Selection) -> ApiResult<()> {
    ctx.retain_source(selected.witness.clone())
}

fn facts(bindings: &[blob_sources::Binding]) -> Vec<native::Facts<Source>> {
    use blob_sources::SourceKind;
    [
        (Source::ReadChats, SourceKind::Chat),
        (Source::ReadThreads, SourceKind::Thread),
        (Source::ReadDocuments, SourceKind::Document),
        (Source::ReadEnvironments, SourceKind::Environment),
        (Source::ReadUploads, SourceKind::RetainedUpload),
    ]
    .into_iter()
    .map(|(source, kind)| {
        let targets: Vec<_> = bindings
            .iter()
            .filter(|binding| binding.source == kind)
            .map(|binding| binding.target.clone())
            .collect();
        native::Facts::collection(
            source,
            if targets.is_empty() {
                Vec::new()
            } else {
                vec![(kind.kind(), targets)]
            },
        )
    })
    .collect()
}

pub(super) struct Blobs;
impl native::Selected for Blobs {
    type Source = Source;
    type Query = UploadBlobGetQuery;
    type Value = Arc<Selection>;
    type Reply = Arc<Selection>;
    const DECLARATION: &'static Declaration<Source> = &declarations::BLOB_GET;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let hash: Hash = ctx
                .param("hash")?
                .parse()
                .map_err(|_| ApiError::not_found())?;
            let name = crate::uploads::clean_name("name", &query.name)?;
            let base = OperationPlan::new(ctx.route, Self::DECLARATION.operation)?;
            let frozen = ctx.optional_retained_source::<Witness>()?;
            if frozen.as_ref().is_some_and(|old| old.hash != hash) {
                return Err(ApiError::internal("the retained upload hash changed"));
            }
            // The partial base grants no byte source. The native planner scope-checks candidates,
            // then picks one under the same caller's ordinary authority before binding the final read.
            let prepared = if let Some(old) = frozen.as_deref() {
                blob_sources::prepare_selected(
                    ctx,
                    conn,
                    caller,
                    &base,
                    &[hash],
                    &[],
                    &old.selected,
                )
                .await?
            } else {
                blob_sources::prepare(ctx, conn, caller, &base, &[hash], &[]).await?
            };
            let facts = facts(&prepared.bindings);
            let witness = Witness {
                hash,
                selected: prepared.selected,
            };
            Ok((
                Arc::new(Selection {
                    hash,
                    name,
                    witness,
                }),
                facts,
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if ctx.user()?.id() != principal.user() {
                return Err(ApiError::internal("the upload reader changed account"));
            }
            Ok(())
        })
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move { Ok(Arc::clone(value)) })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a Self::Query,
        opened: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened.witness == fresh.witness) })
    }
}
