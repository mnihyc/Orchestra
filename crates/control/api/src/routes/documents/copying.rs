//! Native text-copy operands, exact source admission and the shared completed-document reply.

use super::*;
use crate::key_operations::native::{self, effects};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::document_sources::{self as sources, TextCreationSource as Source};
use orch_api_contract::documents::{DocumentCreationResponse, NodeDocumentCopyBody};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::operations::FacetUsage;
use orch_authz::Caller;
use std::sync::{Arc, Mutex};

const DOCUMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Document);
pub(super) struct Copies;
pub(super) struct Proposal {
    native: Mutex<Option<Arc<document_tree_domain::DocumentCopying>>>,
    facts: Vec<native::Facts<Source>>,
}

fn plan(value: &Proposal) -> ApiResult<Arc<document_tree_domain::DocumentCopying>> {
    value
        .native
        .lock()
        .map_err(|_| ApiError::internal("the native copy plan is poisoned"))?
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::internal("the native copy plan was already consumed"))
}

impl effects::Effect for Copies {
    type Source = Source;
    type Query = ();
    type Body = NodeDocumentCopyBody;
    type Value = Proposal;
    type Output = text_creation::Created;
    fn declaration(_value: &Proposal) -> &'static Declaration<Source> {
        &sources::TEXT_COPY
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: NodeDocumentCopyBody,
    ) -> BoxFuture<'a, ApiResult<(Proposal, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            creation::client_id(&body.client_id)?;
            let destination = ctx.id("id")?;
            let directory = document_tree_domain::Directory::NodeDocs(destination);
            let initial = native::bind_known(
                ctx,
                &sources::TEXT_COPY,
                &[native::Facts::one(
                    Source::Destination,
                    DOCUMENT,
                    text_creation::destination(conn, directory).await?,
                )],
            )?;
            let principal = native::preparation_principal(ctx, conn, caller, &initial).await?;
            let id = rows::copy::identity(principal.user(), destination, &body.client_id, "copy");
            let title = body
                .title
                .as_deref()
                .map(|title| clean_title("title", title))
                .transpose()?;
            let request = document_tree_domain::Request {
                command: document_tree_domain::Command::Copy,
                items: vec![document_tree_domain::Item::Document(
                    body.source_document_id,
                )],
                to: Some(directory),
                name: None,
                request_ref: id.to_string(),
                version: None,
            };
            let copy = document_tree_domain::TextCopy {
                id,
                title,
                private: body.access == Some(NewDocumentAccess::Private),
            };
            let subjects = document_tree_domain::prepare_document_copy_sources_in(
                conn,
                document_tree_domain::Actor::Person(&principal),
                &request,
                &copy,
            )
            .await;
            let subjects = crate::routes::document_tree::result(conn, subjects).await?;
            if subjects.destination() != Some(directory) {
                return Err(ApiError::internal(
                    "the native copy changed its destination kind",
                ));
            }
            let ids: Vec<_> = subjects.sources().iter().map(|row| row.id).collect();
            let mut targets = read_operation::identities(conn, &ids).await?;
            let originals = ids
                .iter()
                .map(|id| {
                    targets
                        .remove(id)
                        .filter(|target| target.object.kind == DOCUMENT)
                        .ok_or_else(ApiError::not_found)
                })
                .collect::<ApiResult<Vec<_>>>()?;
            let mut facts = vec![
                native::Facts::one(
                    Source::Destination,
                    DOCUMENT,
                    text_creation::destination(conn, directory).await?,
                ),
                native::Facts::collection(
                    Source::Originals,
                    if originals.is_empty() {
                        Vec::new()
                    } else {
                        vec![(DOCUMENT, originals)]
                    },
                ),
            ];
            // Original material is trusted copied content, not new submitted label snapshots.
            facts.extend(
                sources::TEXT_COPY
                    .bindings
                    .iter()
                    .filter(|binding| {
                        binding.usage == FacetUsage::Required
                            && !matches!(binding.source, Source::Destination | Source::Originals)
                    })
                    .map(|binding| native::Facts::collection(binding.source, Vec::new())),
            );
            let known = native::bind_known(ctx, &sources::TEXT_COPY, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            authorization::require(
                conn,
                &principal,
                destination,
                exact(Permission::DocCreate),
                Act::Use,
                Policy::default(),
            )
            .await?;
            if !subjects.is_replay() {
                for source in subjects.sources() {
                    let standing =
                        build::standing(conn, &principal, source.id, Policy::default()).await?;
                    if !standing.holds(Permission::DocView) {
                        return Err(ApiError::not_found());
                    }
                }
                let settings = DocSettings::at(conn, destination).await?;
                if !settings.enabled {
                    return Err(
                        limits::disabled(conn, &principal, &settings, Policy::default()).await,
                    );
                }
            }
            let prepared = document_tree_domain::prepare_document_copy_material_in(
                conn,
                document_tree_domain::Actor::Person(&principal),
                subjects,
                principal.now(),
                crate::routes::document_tree::options(ctx),
            )
            .await;
            let prepared = crate::routes::document_tree::result(conn, prepared).await?;
            Ok((
                Proposal {
                    native: Mutex::new(Some(Arc::new(prepared))),
                    facts: facts.clone(),
                },
                facts,
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
            let prepared = plan(value)?;
            let result = document_tree_domain::require_document_copy_in(
                conn,
                document_tree_domain::Actor::Person(principal),
                &prepared,
                principal.now(),
            )
            .await;
            crate::routes::document_tree::result(conn, result).await
        })
    }
}

impl effects::ApplyOwned for Copies {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Proposal,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<text_creation::Created>>> {
        Box::pin(async move {
            let prepared = value
                .native
                .lock()
                .map_err(|_| ApiError::internal("the native copy plan is poisoned"))?
                .take()
                .ok_or_else(|| ApiError::internal("the native copy plan was already consumed"))?;
            let prepared = Arc::try_unwrap(prepared)
                .map_err(|_| ApiError::internal("the native copy plan is still in use"))?;
            let changed = !prepared.is_replay();
            let id = prepared.id();
            let deadline = before.deadline();
            let receipt = document_tree_domain::apply_document_copy_in(
                &mut tx,
                document_tree_domain::Actor::Person(principal),
                prepared,
                principal.now(),
                crate::routes::document_tree::options(ctx),
                move || before.check(),
            )
            .await?;
            crate::routes::document_tree::result(&mut tx, receipt).await?;
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
            let output = text_creation::Created::copied(id, value.facts.clone());
            Ok(if changed {
                effects::Outcome::Applied(output)
            } else {
                effects::Outcome::Unchanged(output)
            })
        })
    }
}

pub(super) async fn copy(
    ctx: Ctx,
    (): (),
    _body: NodeDocumentCopyBody,
) -> ApiResult<DocumentCreationResponse> {
    orch_db::queries::transaction_guard::request_scope(
        orch_db::queries::transaction_guard::Scope::Creation,
        async move {
            let (tx, _guard) = transaction_guard::begin(&ctx).await?;
            let completed = effects::apply_owned::<Copies>(&ctx, tx).await?;
            text_creation::finish_created::<text_creation::CopiedTexts>(completed).await
        },
    )
    .await
}
