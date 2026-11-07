//! One consumed body, admitted saved-byte witness and native first-custody writer.

use super::{DocSettings, Limit, attachments, limits, reference};
use crate::key_operations::{
    blob_sources,
    native::{self, effects, raw},
    publication,
};
use crate::middleware::{
    authorization,
    requests::{ApiError, ApiResult},
    transaction_guard,
};
use crate::registry::{Ctx, OperationPlan};
use crate::routes::documents::{build, live, live_locked, read_operation};
use axum::{body::Body, response::Response};
use futures::future::BoxFuture;
use http::StatusCode;
use orch_access_contract::{
    key_scope::KeyTargetKind,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    common::ReceiptProjection,
    document_file_sources::{self as sources, Source},
    documents::{
        DocumentFileChange, DocumentFileChangeKind, DocumentFileCreateQuery,
        DocumentFileCreateReceipt, DocumentFileCreateResponse,
    },
    operation_sources::Declaration,
};
use orch_authz::{Caller, Policy, Principal};
use orch_chat::following::BlobOrigin;
use orch_db::{
    Conn, DbError,
    queries::{
        documents::{self as rows, HeldFile, NewFile},
        uploads,
    },
};
use orch_documents::versions::Writer;
use orch_types::{Hash, Id, new_id};
use std::sync::{Arc, Mutex};

pub(super) mod projection;
pub(super) struct Files;
pub(super) struct Known {
    document: Id,
    query: DocumentFileCreateQuery,
    settings: DocSettings,
    direct: bool,
}
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Input {
    document: Id,
    overwrite: bool,
    staged: attachments::Staged,
}
pub(super) struct Initial {
    input: Arc<Input>,
    settings: DocSettings,
    selected: Vec<(Hash, BlobOrigin)>,
}
pub(super) struct Fetched {
    initial: Initial,
    content: Content,
}
enum Content {
    Ready(attachments::Taken, attachments::Described),
    TooLarge,
}
pub(super) struct Proposal {
    fetched: Arc<Fetched>,
    facts: Vec<native::Facts<Source>>,
    decision: Decision,
}
enum Decision {
    Refused(projection::Refusal),
    Write {
        id: Id,
        replaced: Option<HeldFile>,
        settings: DocSettings,
        retention: Mutex<Option<Arc<attachments::Retention>>>,
    },
}
pub(super) enum Output {
    Written(projection::Written),
    Refused(projection::Refusal),
}
const DOCUMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Document);

fn declaration(direct: bool) -> &'static Declaration<Source> {
    if direct {
        &sources::DIRECT
    } else {
        &sources::REFERENCE
    }
}
async fn document_fact(conn: &mut Conn, id: Id) -> ApiResult<native::Facts<Source>> {
    Ok(native::Facts::one(
        Source::Document,
        DOCUMENT,
        read_operation::identity(conn, id).await?,
    ))
}
async fn require_document(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    permission: Permission,
) -> ApiResult<()> {
    authorization::require_document(conn, principal, id, permission, Policy::default()).await
}
fn source_facts(bindings: &[blob_sources::Binding]) -> Vec<native::Facts<Source>> {
    [
        (blob_sources::SourceKind::Chat, Source::Chats),
        (blob_sources::SourceKind::Thread, Source::Threads),
        (blob_sources::SourceKind::Document, Source::Documents),
        (blob_sources::SourceKind::Environment, Source::Environments),
        (blob_sources::SourceKind::RetainedUpload, Source::Uploads),
    ]
    .into_iter()
    .map(|(kind, role)| {
        let targets = bindings
            .iter()
            .filter(|binding| binding.source == kind)
            .map(|binding| binding.target.clone())
            .collect::<Vec<_>>();
        native::Facts::collection(
            role,
            if targets.is_empty() {
                Vec::new()
            } else {
                vec![(kind.kind(), targets)]
            },
        )
    })
    .collect()
}
async fn root(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    input: &Input,
) -> ApiResult<(OperationPlan, Principal, Vec<native::Facts<Source>>)> {
    live(conn, input.document).await?;
    let facts = vec![document_fact(conn, input.document).await?];
    let plan = native::bind_known(ctx, declaration(input.staged.supplied_bytes()), &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
    require_document(conn, &principal, input.document, Permission::DocEdit).await?;
    Ok((plan, principal, facts))
}

impl raw::Staged for Files {
    type Known = Known;
    type Input = Input;
    fn known_declaration(known: &Known) -> &'static Declaration<Source> {
        declaration(known.direct)
    }
    fn known<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _caller: &'a Caller,
        query: DocumentFileCreateQuery,
    ) -> BoxFuture<'a, ApiResult<(Known, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let document = ctx.id("id")?;
            let row = live(conn, document).await?;
            let settings = DocSettings::at(conn, row.node_id).await?;
            Ok((
                Known {
                    document,
                    query,
                    settings,
                    direct: crate::routes::threads::uploads::filename_of(ctx).is_some(),
                },
                vec![document_fact(conn, document).await?],
            ))
        })
    }
    fn require_known<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        known: &'a Known,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_document(
            conn,
            principal,
            known.document,
            Permission::DocEdit,
        ))
    }
    fn take<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        known: Known,
        body: Body,
    ) -> BoxFuture<'a, ApiResult<Input>> {
        Box::pin(async move {
            let staged =
                match attachments::stage(ctx, body, known.settings.file_max_bytes.value, reference)
                    .await
                {
                    Ok(staged) => staged,
                    Err(attachments::Refused::Other(error)) => return Err(error),
                    Err(attachments::Refused::TooLarge {
                        limit,
                        by_request: true,
                    }) => return Err(attachments::over_a_request(limit)),
                    Err(attachments::Refused::TooLarge { .. }) => {
                        return Err(projection::Refusal::limit(
                            known.document,
                            known.direct,
                            known.settings,
                            Limit::FileBytes,
                        )
                        .project(ctx, caller)
                        .await?);
                    }
                };
            if staged.supplied_bytes() != known.direct {
                return Err(ApiError::internal(
                    "the native file input changed its body branch",
                ));
            }
            let input = Input {
                document: known.document,
                overwrite: known.query.overwrite.unwrap_or(false),
                staged,
            };
            effects::initial::<Files>(ctx, caller, known.query, Arc::new(input.clone())).await?;
            Ok(input)
        })
    }
}

impl effects::Initial for Files {
    type Input = Initial;
    type Fetched = Fetched;
    fn declaration(input: &Initial) -> &'static Declaration<Source> {
        declaration(input.input.staged.supplied_bytes())
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: DocumentFileCreateQuery,
        input: Arc<Input>,
    ) -> BoxFuture<'a, ApiResult<(Initial, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            if query.overwrite.unwrap_or(false) != input.overwrite
                || ctx.id("id")? != input.document
            {
                return Err(ApiError::internal(
                    "the native file input changed its destination or intent",
                ));
            }
            let (plan, _principal, mut facts) = root(ctx, conn, caller, &input).await?;
            let row = live(conn, input.document).await?;
            let settings = DocSettings::at(conn, row.node_id).await?;
            let selected = if input.staged.supplied_bytes() {
                Vec::new()
            } else {
                let sources =
                    blob_sources::prepare(ctx, conn, caller, &plan, &[input.staged.hash()], &[])
                        .await?;
                facts.extend(source_facts(&sources.bindings));
                sources.selected
            };
            Ok((
                Initial {
                    input,
                    settings,
                    selected,
                },
                facts,
            ))
        })
    }
    fn require_initial<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Initial,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require_document(conn, principal, input.input.document, Permission::DocEdit).await?;
            if !input.input.staged.supplied_bytes() {
                attachments::require(conn, principal, &input.input.staged.hash()).await?;
            }
            Ok(())
        })
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        _caller: &'a Caller,
        initial: Initial,
    ) -> BoxFuture<'a, ApiResult<Fetched>> {
        Box::pin(async move {
            let content = match initial
                .input
                .staged
                .measure(ctx, initial.settings.file_max_bytes.value)
                .await
            {
                Ok(taken) => {
                    let described = attachments::describe(ctx, &taken.meta, &taken.name).await?;
                    Content::Ready(taken, described)
                }
                Err(attachments::Refused::TooLarge { .. }) => Content::TooLarge,
                Err(attachments::Refused::Other(error)) => return Err(error),
            };
            Ok(Fetched { initial, content })
        })
    }
}

impl effects::Effect for Files {
    type Source = Source;
    type Query = DocumentFileCreateQuery;
    type Body = Arc<Input>;
    type Value = Proposal;
    type Output = Output;
    fn declaration(value: &Proposal) -> &'static Declaration<Source> {
        declaration(value.fetched.initial.input.staged.supplied_bytes())
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: DocumentFileCreateQuery,
        input: Arc<Input>,
    ) -> BoxFuture<'a, ApiResult<(Proposal, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let fetched = effects::initial_value::<Files>(ctx)?;
            if *fetched.initial.input != *input
                || query.overwrite.unwrap_or(false) != input.overwrite
                || ctx.id("id")? != input.document
            {
                return Err(ApiError::internal(
                    "the consumed file body changed before its native writer",
                ));
            }
            let _known = root(ctx, conn, caller, &input).await?;
            let row = live_locked(conn, input.document).await?;
            let settings = DocSettings::at(conn, row.node_id).await?;
            let old = match &fetched.content {
                Content::Ready(taken, _) => {
                    rows::file_named(conn, input.document, &taken.name).await?
                }
                Content::TooLarge => None,
            };
            if let Content::Ready(taken, _) = &fetched.content {
                let mut hashes = vec![taken.meta.hash];
                hashes.extend(old.as_ref().map(|file| file.blob));
                // One native sorted lock set avoids reversing two replacements' pin order.
                uploads::lock_blobs(conn, &hashes).await?;
            }
            let mut facts = vec![document_fact(conn, input.document).await?];
            let plan = native::bind_known(ctx, declaration(input.staged.supplied_bytes()), &facts)?;
            if !input.staged.supplied_bytes() {
                let selected = blob_sources::prepare_selected(
                    ctx,
                    conn,
                    caller,
                    &plan,
                    &[input.staged.hash()],
                    &[],
                    &fetched.initial.selected,
                )
                .await?;
                facts.extend(source_facts(&selected.bindings));
            }
            let complete =
                native::bind_required(ctx, declaration(input.staged.supplied_bytes()), &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &complete).await?;
            require_document(conn, &principal, input.document, Permission::DocEdit).await?;
            let direct = input.staged.supplied_bytes();
            let decision = match &fetched.content {
                Content::TooLarge => Decision::Refused(projection::Refusal::limit(
                    input.document,
                    direct,
                    fetched.initial.settings,
                    Limit::FileBytes,
                )),
                Content::Ready(taken, _) => {
                    if let Some(found) = old.as_ref().filter(|_| !input.overwrite) {
                        Decision::Refused(
                            projection::Refusal::conflict(conn, input.document, direct, found)
                                .await?,
                        )
                    } else if taken.meta.size > settings.file_max_bytes.value {
                        Decision::Refused(projection::Refusal::limit(
                            input.document,
                            direct,
                            settings,
                            Limit::FileBytes,
                        ))
                    } else if (row.files_bytes - old.as_ref().map_or(0, |found| found.file.size))
                        .saturating_add(taken.meta.size)
                        > settings.files_max_bytes.value
                    {
                        Decision::Refused(projection::Refusal::limit(
                            input.document,
                            direct,
                            settings,
                            Limit::FilesBytes,
                        ))
                    } else {
                        let retention =
                            attachments::prepare_retention(conn, &principal, taken).await?;
                        Decision::Write {
                            id: old.as_ref().map_or_else(new_id, |found| found.file.id),
                            replaced: old,
                            settings,
                            retention: Mutex::new(Some(Arc::new(retention))),
                        }
                    }
                }
            };
            Ok((
                Proposal {
                    fetched,
                    facts: facts.clone(),
                    decision,
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
            require_document(
                conn,
                principal,
                value.fetched.initial.input.document,
                Permission::DocEdit,
            )
            .await?;
            if let Decision::Write { retention, .. } = &value.decision {
                let prepared = retention
                    .lock()
                    .map_err(|_| ApiError::internal("the native custody plan is poisoned"))?
                    .as_ref()
                    .cloned()
                    .ok_or_else(|| ApiError::internal("the native custody plan was consumed"))?;
                attachments::require_retention(conn, principal, &prepared).await?;
            }
            Ok(())
        })
    }
}

impl effects::ApplyOwned for Files {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Proposal,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Output>>> {
        Box::pin(async move {
            let Decision::Write {
                id,
                replaced,
                settings,
                retention,
            } = &value.decision
            else {
                let Decision::Refused(error) = &value.decision else {
                    return Err(ApiError::internal("the native file decision is missing"));
                };
                return Ok(effects::Outcome::Unchanged(Output::Refused(error.clone())));
            };
            let Content::Ready(taken, described) = &value.fetched.content else {
                return Err(ApiError::internal("the native file content is missing"));
            };
            let retained = retention
                .lock()
                .map_err(|_| ApiError::internal("the native custody plan is poisoned"))?
                .take()
                .ok_or_else(|| ApiError::internal("the native custody plan was consumed"))?;
            let retained = Arc::try_unwrap(retained)
                .map_err(|_| ApiError::internal("the native custody plan is still in use"))?;
            let deadline = before.deadline();
            attachments::apply_retention(ctx, &mut tx, principal, retained, move || before.check())
                .await??;
            // The original native pin-before-unpin order also preserves identical replacements.
            ctx.app.blobs.pin(&mut tx, &[taken.meta.hash]).await?;
            let document = value.fetched.initial.input.document;
            let file = NewFile {
                id: *id,
                document,
                blob: taken.meta.hash,
                name: &taken.name,
                media_type: &described.media_type,
                size: taken.meta.size,
                is_text: described.is_text,
                added_by: Some(principal.user()),
                added_by_thread: None,
                at: ctx.now,
            };
            if let Some(old) = replaced {
                rows::replace_file(&mut tx, &file).await?;
                ctx.app.blobs.unpin(&mut tx, &[old.blob], ctx.now).await?;
            } else {
                rows::add_file(&mut tx, &file).await?;
            }
            rows::history::record_file(
                &mut tx,
                document,
                &Writer::User(principal.user()),
                DocumentFileChange {
                    name: taken.name.clone(),
                    kind: if replaced.is_some() {
                        DocumentFileChangeKind::Replaced
                    } else {
                        DocumentFileChangeKind::Added
                    },
                },
                settings.version_rules(ctx.app.config.documents.version_window_secs),
                ctx.now,
            )
            .await?;
            rows::touch(&mut tx, document).await?;
            let output = projection::Written::capture(
                &mut tx,
                document,
                *id,
                value.fetched.initial.input.staged.supplied_bytes(),
                value.facts.clone(),
            )
            .await?;
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
            Ok(effects::Outcome::Applied(Output::Written(output)))
        })
    }
}

pub(super) async fn create(
    ctx: Ctx,
    _query: DocumentFileCreateQuery,
    _input: Arc<Input>,
) -> ApiResult<Response> {
    let (tx, _guard) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply_owned::<Files>(&ctx, tx).await?;
    let response = match completed.output {
        Output::Refused(error) => {
            return Err(error.project(&completed.ctx, &completed.accepted).await?);
        }
        Output::Written(written) => {
            let receipt = DocumentFileCreateResponse::Receipt(DocumentFileCreateReceipt {
                projection: ReceiptProjection::Receipt,
                document_id: written.document(),
                file_id: written.file(),
            });
            publication::receipt(&completed.ctx, &receipt)?;
            match native::reply::render::<projection::FileReply>(
                &completed.ctx,
                &completed.accepted,
                written,
            )
            .await?
            {
                Some(file) => DocumentFileCreateResponse::full(file),
                None => receipt,
            }
        }
    };
    crate::routes::workspace::json_reply(StatusCode::OK, &response)
}
