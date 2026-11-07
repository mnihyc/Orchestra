//! Text creation consumes the native directory plan and the existing copied-label producer.

use super::*;
use crate::key_operations::native::{self, effects, reply};
use crate::routes::access::page::NameBounds;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::common::ReceiptProjection;
use orch_api_contract::document_sources::{self as sources, TextCreationSource as Source};
use orch_api_contract::documents::{DocumentCreationReceipt, DocumentCreationResponse};
use orch_api_contract::operation_sources::Declaration;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

const DOCUMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Document);
const WRITTEN: &[(KeyTargetKind, Source)] = &[
    (KeyTargetKind::Object(ObjectKind::Org), Source::WrittenOrgs),
    (
        KeyTargetKind::Object(ObjectKind::Project),
        Source::WrittenProjects,
    ),
    (
        KeyTargetKind::Object(ObjectKind::Topic),
        Source::WrittenTopics,
    ),
    (
        KeyTargetKind::Object(ObjectKind::Thread),
        Source::WrittenThreads,
    ),
    (DOCUMENT, Source::WrittenDocuments),
    (
        KeyTargetKind::Identity(orch_access_contract::key_scope::KeyIdentityKind::Chat),
        Source::WrittenChats,
    ),
];
const NAMES: &[(ObjectKind, Source)] = &[
    (ObjectKind::Org, Source::Orgs),
    (ObjectKind::Project, Source::Projects),
    (ObjectKind::Topic, Source::Topics),
    (ObjectKind::Thread, Source::Threads),
];

pub(super) trait Destination: Send + Sync + 'static {
    const DECLARATION: &'static Declaration<Source>;
    fn directory(ctx: &Ctx) -> ApiResult<document_tree_domain::Directory>;
}
pub(super) struct Node;
pub(super) struct Container;
pub(super) struct CopyNode;
impl Destination for Node {
    const DECLARATION: &'static Declaration<Source> = &sources::TEXT_NODE_CREATE;
    fn directory(ctx: &Ctx) -> ApiResult<document_tree_domain::Directory> {
        Ok(document_tree_domain::Directory::NodeDocs(ctx.id("id")?))
    }
}
impl Destination for Container {
    const DECLARATION: &'static Declaration<Source> = &sources::TEXT_CONTAINER_CREATE;
    fn directory(ctx: &Ctx) -> ApiResult<document_tree_domain::Directory> {
        Ok(document_tree_domain::Directory::Container(ctx.id("id")?))
    }
}
impl Destination for CopyNode {
    const DECLARATION: &'static Declaration<Source> = &sources::TEXT_COPY;
    fn directory(ctx: &Ctx) -> ApiResult<document_tree_domain::Directory> {
        Node::directory(ctx)
    }
}
pub(super) struct Texts<F: Destination>(PhantomData<F>);
pub(super) type AtNode = Texts<Node>;
pub(super) type AtContainer = Texts<Container>;
pub(super) type CopiedTexts = Texts<CopyNode>;

pub(super) struct Proposal {
    native: Mutex<Option<Arc<document_tree_domain::TextCreationPlan>>>,
    facts: Vec<native::Facts<Source>>,
    title: String,
    blocks: Vec<NewBlock>,
    access: Option<NewDocumentAccess>,
    settings: DocSettings,
}
pub(super) struct Created {
    id: Id,
    facts: Vec<native::Facts<Source>>,
}
impl Created {
    pub(super) fn copied(id: Id, facts: Vec<native::Facts<Source>>) -> Self {
        Self { id, facts }
    }
}
pub(super) struct Subject {
    row: DocumentRow,
    directory: document_tree_domain::Directory,
}
pub(super) struct View {
    listed: documents::Listed,
    names: NameBounds,
    sources: Vec<build::WrittenLabelSource>,
    cut: crate::routes::document_tree::metadata::Cut,
}

pub(super) async fn destination(
    conn: &mut Conn,
    directory: document_tree_domain::Directory,
) -> ApiResult<KeyResolvedTarget> {
    match directory {
        document_tree_domain::Directory::NodeDocs(id) => {
            crate::key_operations::targets::node(conn, id).await
        }
        document_tree_domain::Directory::Container(id) => read_operation::identities(conn, &[id])
            .await?
            .remove(&id)
            .filter(|target| {
                target.object.kind == KeyTargetKind::Object(ObjectKind::DocumentContainer)
            })
            .ok_or_else(ApiError::not_found),
    }
}
fn copied_facts(
    sources: &[(build::WrittenLabelSource, KeyResolvedTarget)],
) -> Vec<native::Facts<Source>> {
    WRITTEN
        .iter()
        .map(|(kind, source)| {
            let targets: Vec<_> = sources
                .iter()
                .filter(|(_, target)| target.object.kind == *kind)
                .map(|(_, target)| target.clone())
                .collect();
            native::Facts::collection(
                *source,
                if targets.is_empty() {
                    Vec::new()
                } else {
                    vec![(*kind, targets)]
                },
            )
        })
        .collect()
}
fn plan(value: &Proposal) -> ApiResult<Arc<document_tree_domain::TextCreationPlan>> {
    value
        .native
        .lock()
        .map_err(|_| ApiError::internal("the native text plan is poisoned"))?
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::internal("the native text plan was already consumed"))
}

impl<F: Destination> effects::Effect for Texts<F> {
    type Source = Source;
    type Query = ();
    type Body = NodeDocumentCreateBody;
    type Value = Proposal;
    type Output = Created;
    fn declaration(_value: &Proposal) -> &'static Declaration<Source> {
        F::DECLARATION
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a orch_authz::Caller,
        (): (),
        body: NodeDocumentCreateBody,
    ) -> BoxFuture<'a, ApiResult<(Proposal, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let directory = F::directory(ctx)?;
            let initial = native::bind_known(
                ctx,
                F::DECLARATION,
                &[native::Facts::one(
                    Source::Destination,
                    DOCUMENT,
                    destination(conn, directory).await?,
                )],
            )?;
            let principal = native::preparation_principal(ctx, conn, caller, &initial).await?;
            let parent = creation::parent(ctx, conn, &principal).await?;
            if parent != directory {
                return Err(ApiError::internal(
                    "the native text destination changed its kind",
                ));
            }
            let node = match parent {
                document_tree_domain::Directory::NodeDocs(id) => id,
                document_tree_domain::Directory::Container(id) => {
                    rows::containers::get(conn, id)
                        .await?
                        .ok_or_else(ApiError::not_found)?
                        .node_id
                }
            };
            placed(conn, node).await?;
            let settings = DocSettings::at(conn, node).await?;
            if !settings.enabled {
                return Err(limits::disabled(conn, &principal, &settings, Policy::default()).await);
            }
            let title = clean_title("title", &body.title)?;
            let blocks = initial_blocks(conn, body.blocks, body.markdown).await?;
            let prepared = document_tree_domain::prepare_text_creation_in(
                conn,
                document_tree_domain::Actor::Person(&principal),
                parent,
                orch_types::new_id(),
                &title,
                None,
                principal.now(),
            )
            .await;
            let prepared = crate::routes::document_tree::result(conn, prepared).await?;
            let actual = native::Facts::one(
                Source::Destination,
                DOCUMENT,
                destination(conn, parent).await?,
            );
            let known = native::bind_known(ctx, F::DECLARATION, std::slice::from_ref(&actual))?;
            let (labels, copied) =
                operation::prepare_written(ctx, conn, caller, known, Some(&blocks)).await?;
            let blocks = labels.apply(blocks)?;
            let mut facts = vec![actual];
            facts.extend(copied_facts(&copied));
            Ok((
                Proposal {
                    native: Mutex::new(Some(Arc::new(prepared))),
                    facts: facts.clone(),
                    title,
                    blocks,
                    access: body.access,
                    settings,
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
            let checked = document_tree_domain::require_text_creation_in(
                conn,
                document_tree_domain::Actor::Person(principal),
                &prepared,
                principal.now(),
            )
            .await;
            crate::routes::document_tree::result(conn, checked).await
        })
    }
}

impl<F: Destination> effects::ApplyOwned for Texts<F> {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Proposal,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Created>>> {
        Box::pin(async move {
            let prepared = value
                .native
                .lock()
                .map_err(|_| ApiError::internal("the native text plan is poisoned"))?
                .take()
                .ok_or_else(|| ApiError::internal("the native text plan was already consumed"))?;
            let prepared = Arc::try_unwrap(prepared)
                .map_err(|_| ApiError::internal("the native text plan is still in use"))?;
            let created = document_tree_domain::apply_text_creation_in(
                &mut tx,
                document_tree_domain::Actor::Person(principal),
                prepared,
                principal.now(),
                move || before.check(),
            )
            .await?;
            let created = crate::routes::document_tree::result(&mut tx, created).await?;
            if created.created {
                install_created(
                    ctx,
                    &mut tx,
                    principal,
                    CreationMaterial {
                        id: created.document.id,
                        node: created.document.node_id,
                        title: &value.title,
                        access: value.access,
                        blocks: value.blocks.clone(),
                        copied_files: &[],
                        receipt_warnings: &[],
                        settings: value.settings,
                    },
                )
                .await?;
            }
            let id = created.document.id;
            let identity = transaction_guard::current_http_identity(
                ctx,
                &mut tx,
                transaction_guard::HttpIdentityMode::Read,
            )
            .await?;
            identity.check_deadline(&ctx.app)?;
            tx.commit().await.map_err(DbError::from)?;
            let output = Created {
                id,
                facts: value.facts.clone(),
            };
            Ok(if created.created {
                effects::Outcome::Applied(output)
            } else {
                effects::Outcome::Unchanged(output)
            })
        })
    }
}

async fn response_facts(
    conn: &mut Conn,
    names: &NameBounds,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let mut nodes = Vec::new();
    for id in &names.nodes {
        nodes.push(crate::key_operations::targets::node_identity(conn, *id).await?);
    }
    let mut facts = Vec::new();
    for (kind, source) in NAMES {
        let kind = KeyTargetKind::Object(*kind);
        let targets: Vec<_> = nodes
            .iter()
            .filter(|target| target.object.kind == kind)
            .cloned()
            .collect();
        facts.push(native::Facts::collection(
            *source,
            if targets.is_empty() {
                Vec::new()
            } else {
                vec![(kind, targets)]
            },
        ));
    }
    let ids: Vec<_> = names.containers.iter().copied().collect();
    let mut found = read_operation::identities(conn, &ids).await?;
    let targets = ids
        .iter()
        .map(|id| found.remove(id).ok_or_else(ApiError::not_found))
        .collect::<ApiResult<Vec<_>>>()?;
    facts.push(native::Facts::collection(
        Source::Containers,
        if targets.is_empty() {
            Vec::new()
        } else {
            vec![(
                KeyTargetKind::Object(ObjectKind::DocumentContainer),
                targets,
            )]
        },
    ));
    Ok(facts)
}

impl<F: Destination> reply::Projection for Texts<F> {
    type Source = Source;
    type Input = Created;
    type Subject = Subject;
    type Value = View;
    type Reply = Document;
    const READ_SOURCE: Source = Source::Created;
    fn declaration(_input: &Created) -> &'static Declaration<Source> {
        F::DECLARATION
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Created,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = live(conn, input.id).await?;
            let directory = stored_directory(conn, &row).await?;
            let mut facts = input.facts.clone();
            facts.push(native::Facts::one(
                Source::Created,
                DOCUMENT,
                read_operation::identity(conn, row.id).await?,
            ));
            Ok((Subject { row, directory }, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Created,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            authorization::require_document(
                conn,
                principal,
                subject.row.id,
                Permission::DocView,
                Policy::default(),
            )
            .await?;
            Ok(())
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Created,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<(View, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let listed =
                build::standing(conn, principal, subject.row.id, Policy::default()).await?;
            let mut facts = input.facts.clone();
            facts.push(native::Facts::one(
                Source::Created,
                DOCUMENT,
                read_operation::identity(conn, subject.row.id).await?,
            ));
            let mut known = native::bind_known(ctx, F::DECLARATION, &facts)?;
            let (names, sources) = read_operation::completed_sources(
                conn,
                principal,
                &mut known,
                &subject.row,
                &listed,
            )
            .await?;
            let facts = response_facts(conn, &names).await?;
            let cut = crate::routes::document_tree::metadata::cut(
                conn,
                principal,
                subject.row.node_id,
                principal.now(),
            )
            .await?;
            Ok((
                View {
                    listed,
                    names,
                    sources,
                    cut,
                },
                facts,
            ))
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Created,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Document>> {
        Box::pin(render_readback(
            conn,
            principal,
            &prepared.subject,
            &prepared.value,
        ))
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Created,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(check_readback(
            conn,
            principal,
            &opened.subject,
            &opened.value,
            &fresh.subject,
            &fresh.value,
        ))
    }
}

async fn render_readback(
    conn: &mut Conn,
    principal: &Principal,
    subject: &Subject,
    view: &View,
) -> ApiResult<Document> {
    build::document_bounded(
        conn,
        principal,
        &subject.row,
        &view.listed,
        principal.now().get(),
        Policy::default(),
        Some(&view.names),
        Some(&view.sources),
    )
    .await
}

/// Keep the native publication check concrete; it is independent of the creation destination.
async fn check_readback(
    conn: &mut Conn,
    principal: &Principal,
    opened: &Subject,
    view: &View,
    fresh: &Subject,
    current: &View,
) -> ApiResult<bool> {
    if opened.row.id != fresh.row.id
        || opened.row.node_id != fresh.row.node_id
        || opened.directory != fresh.directory
        || !view.names.nodes.is_subset(&current.names.nodes)
        || !view.names.containers.is_subset(&current.names.containers)
        || view
            .sources
            .iter()
            .any(|source| !current.sources.contains(source))
    {
        return Ok(false);
    }
    match crate::routes::document_tree::metadata::unchanged(
        conn,
        principal,
        &view.cut,
        principal.now(),
    )
    .await
    {
        Ok(()) => Ok(true),
        Err(error) if error.problem.code == ErrorCode::ConflictStaleRevision => Ok(false),
        Err(error) => Err(error),
    }
}

pub(super) async fn create<F>(
    ctx: Ctx,
    (): (),
    _body: NodeDocumentCreateBody,
) -> ApiResult<DocumentCreationResponse>
where
    F: effects::ApplyOwned<Body = NodeDocumentCreateBody, Output = Created>
        + reply::Projection<Input = Created, Reply = Document>,
{
    orch_db::queries::transaction_guard::request_scope(
        orch_db::queries::transaction_guard::Scope::Creation,
        create_body::<F>(ctx),
    )
    .await
}
async fn create_body<F>(ctx: Ctx) -> ApiResult<DocumentCreationResponse>
where
    F: effects::ApplyOwned<Body = NodeDocumentCreateBody, Output = Created>
        + reply::Projection<Input = Created, Reply = Document>,
{
    let (tx, _guard) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply_owned::<F>(&ctx, tx).await?;
    finish_created::<F>(completed).await
}

/// The same real completed-document projection for ordinary creation and native copying.
pub(super) async fn finish_created<F>(
    completed: effects::Completed<Created>,
) -> ApiResult<DocumentCreationResponse>
where
    F: reply::Projection<Input = Created, Reply = Document>,
{
    let receipt = DocumentCreationResponse::Receipt(DocumentCreationReceipt {
        projection: ReceiptProjection::Receipt,
        document_id: completed.output.id,
    });
    crate::key_operations::publication::receipt(&completed.ctx, &receipt)?;
    Ok(
        match reply::render::<F>(&completed.ctx, &completed.accepted, completed.output).await? {
            Some(document) => DocumentCreationResponse::Full(Box::new(document)),
            None => receipt,
        },
    )
}
