//! Open the authorized archive once; bind the native plan and recheck its original read sources.

use super::*;
use crate::key_operations::{blob_sources, native, targets};
use futures::future::BoxFuture;
use native::effects::{self, Effect, Initial};
use orch_access_contract::{
    key_scope::{KeyResolvedTarget, KeyTargetKind},
    permissions::ObjectKind,
};
use orch_api_contract::{
    import_preview_sources::{PREFLIGHT, Source},
    imports::ImportLimits,
    operation_sources::Declaration,
};
use orch_authz::{Caller, CapturedOperation, Principal, engine::Graph};
use orch_chat::following::BlobOrigin;
use orch_db::queries::imports::Table;
use orch_engine::imports::{Opened, Plan};
use orch_types::{Hash, Id};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub(super) struct Planning;

pub(super) struct Known {
    node: Id,
    request: ImportPreflightRequest,
    selected: Vec<(Hash, BlobOrigin)>,
    limits: ImportLimits,
}
pub(super) struct Fetched {
    known: Known,
    origin: CapturedOperation,
    opened: Arc<Opened>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Existing {
    Node(Id),
    Document(Id),
    Folder(Id),
}
impl Existing {
    fn native(self) -> (Table, Id) {
        match self {
            Self::Node(id) => (Table::Nodes, id),
            Self::Document(id) => (Table::Documents, id),
            Self::Folder(id) => (Table::Containers, id),
        }
    }
}

/// Native matches are retained before the public aggregate/page is made.
pub(super) struct Ready {
    fetched: Arc<Fetched>,
    plan: Option<Arc<Plan>>,
    existing: BTreeSet<Existing>,
    accounts: BTreeSet<Id>,
    targets: BTreeMap<(KeyTargetKind, Id), KeyResolvedTarget>,
}

async fn require(ctx: &Ctx, conn: &mut Conn, principal: &Principal, node: Id) -> ApiResult<()> {
    match ctx
        .app
        .engine
        .import_place_seen(conn, principal, node)
        .await
    {
        Ok(()) => Ok(()),
        Err(error) => Err(failed(conn, error).await),
    }
}

/// Keep exact completed-upload custody and the shared selected byte witnesses.
async fn inputs(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    request: ImportPreflightRequest,
    old: Option<&Fetched>,
) -> ApiResult<(Known, Vec<native::Facts<Source>>, Principal)> {
    if request.request.source.files.is_empty() {
        return Err(ApiError::invalid(
            "source.files",
            "name the uploads to import",
        ));
    }
    let node = ctx.id("id")?;
    if old.is_some_and(|old| old.known.node != node || old.known.request != request) {
        return Err(ApiError::internal(
            "the original import planning input changed",
        ));
    }
    let target = targets::node_identity(conn, node).await?;
    let mut facts = vec![native::Facts::one(
        Source::Destination,
        target.object.kind,
        target,
    )];
    let known = native::bind_known(ctx, &PREFLIGHT, &facts)?;
    let known = match old {
        Some(old) => known.with_origin(ctx, conn, caller, &old.origin).await?,
        None => known,
    };
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    require(ctx, conn, &principal, node).await?;
    for file in &request.request.source.files {
        crate::attachments::require(conn, &principal, &file.blob).await?;
    }
    let hashes = request
        .request
        .source
        .files
        .iter()
        .map(|file| file.blob)
        .collect::<Vec<_>>();
    let selected = match old {
        None => blob_sources::prepare(ctx, conn, principal.caller(), &known, &hashes, &[]).await?,
        Some(old) => {
            blob_sources::prepare_selected(
                ctx,
                conn,
                principal.caller(),
                &known,
                &hashes,
                &[],
                &old.known.selected,
            )
            .await?
        }
    };
    if old.is_some_and(|old| old.known.selected != selected.selected) {
        return Err(ApiError::not_found());
    }
    facts.extend(super::source_preview::byte_facts(&selected.bindings));
    let limits = match ctx.app.engine.import_limits(conn, node).await {
        Ok(limits) => limits,
        Err(error) => return Err(failed(conn, error).await),
    };
    Ok((
        Known {
            node,
            request,
            selected: selected.selected,
            limits,
        },
        facts,
        principal,
    ))
}

fn matches(plan: &Plan) -> ApiResult<(BTreeSet<Existing>, BTreeSet<Id>)> {
    let mut existing = BTreeSet::new();
    for found in plan
        .objects
        .iter()
        .filter_map(|object| object.existing.as_ref())
    {
        existing.insert(match found.table {
            Table::Nodes => Existing::Node(found.id),
            Table::Documents => Existing::Document(found.id),
            Table::Containers => Existing::Folder(found.id),
            // Native chats are appended and never presented as an existing-object conflict.
            Table::Chats | Table::Messages | Table::Rows => {
                return Err(ApiError::internal(
                    "the native import match has an unsupported table",
                ));
            }
        });
    }
    Ok((
        existing,
        plan.people
            .iter()
            .filter_map(|person| person.user)
            .collect(),
    ))
}

async fn context(
    conn: &mut Conn,
    principal: &Principal,
    ready: &mut Ready,
    facts: &mut Vec<native::Facts<Source>>,
) -> ApiResult<()> {
    let native = ready
        .existing
        .iter()
        .map(|subject| subject.native())
        .collect::<Vec<_>>();
    match orch_engine::imports::existing_readable(
        conn,
        principal,
        ready.fetched.known.node,
        &native,
    )
    .await
    {
        Ok(true) => {}
        Ok(false) => return Err(ApiError::not_found()),
        Err(error) => return Err(failed(conn, error).await),
    }
    let nodes = ready
        .existing
        .iter()
        .filter_map(|subject| match subject {
            Existing::Node(id) => Some(*id),
            _ => None,
        })
        .chain(std::iter::once(ready.fetched.known.node))
        .collect::<Vec<_>>();
    let mut graph = Graph::load(conn, &nodes)
        .await
        .map_err(ApiError::internal)?;
    let ids = ready
        .existing
        .iter()
        .filter_map(|subject| match subject {
            Existing::Document(id) | Existing::Folder(id) => Some(*id),
            Existing::Node(_) => None,
        })
        .collect::<Vec<_>>();
    let documents = orch_authz::documents::read_into(conn, &mut graph, &ids)
        .await
        .map_err(ApiError::internal)?;
    let mut document_targets = BTreeMap::new();
    for document in documents.into_iter().filter(|document| !document.deleted) {
        let site =
            orch_authz::documents::site_of(&graph, &document).ok_or_else(ApiError::not_found)?;
        document_targets.insert(
            (document.kind, document.id),
            targets::from_site(&graph, &site)?,
        );
    }
    let mut groups: BTreeMap<KeyTargetKind, Vec<KeyResolvedTarget>> = BTreeMap::new();
    for subject in &ready.existing {
        let target = match *subject {
            Existing::Node(id) => {
                orch_engine::operation_scope::targets::from_node_identity(&graph, id)
                    .ok_or_else(ApiError::not_found)?
            }
            Existing::Document(id) => document_targets
                .get(&(ObjectKind::Document, id))
                .cloned()
                .ok_or_else(ApiError::not_found)?,
            Existing::Folder(id) => document_targets
                .get(&(ObjectKind::DocumentContainer, id))
                .cloned()
                .ok_or_else(ApiError::not_found)?,
        };
        let id = target
            .object
            .id
            .ok_or_else(|| ApiError::internal("an import match has no identity"))?;
        ready
            .targets
            .insert((target.object.kind, id), target.clone());
        groups.entry(target.object.kind).or_default().push(target);
    }
    facts.push(native::Facts::collection(
        Source::Existing,
        groups.into_iter().collect(),
    ));
    let mut accounts = Vec::new();
    for id in &ready.accounts {
        let target = targets::account(conn, *id).await?;
        ready
            .targets
            .insert((target.object.kind, *id), target.clone());
        accounts.push(target);
    }
    facts.push(native::Facts::collection(
        Source::Accounts,
        if accounts.is_empty() {
            Vec::new()
        } else {
            vec![(
                orch_access_contract::key_scope::KeyTargetKind::Identity(
                    orch_access_contract::key_scope::KeyIdentityKind::Account,
                ),
                accounts,
            )]
        },
    ));
    let root =
        orch_engine::operation_scope::targets::from_node_identity(&graph, ready.fetched.known.node)
            .ok_or_else(ApiError::not_found)?;
    ready
        .targets
        .insert((root.object.kind, ready.fetched.known.node), root);
    Ok(())
}

impl Effect for Planning {
    type Source = Source;
    type Query = ();
    type Body = ImportPreflightRequest;
    type Value = Known;
    type Output = ImportPlan;
    fn declaration(_: &Known) -> &'static Declaration<Source> {
        &PREFLIGHT
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ImportPreflightRequest,
    ) -> BoxFuture<'a, ApiResult<(Known, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (known, facts, _) = inputs(ctx, conn, caller, body, None).await?;
            Ok((known, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        known: &'a Known,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(ctx, conn, principal, known.node))
    }
}
impl Initial for Planning {
    type Input = Known;
    type Fetched = Fetched;
    fn declaration(_: &Known) -> &'static Declaration<Source> {
        &PREFLIGHT
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ImportPreflightRequest,
    ) -> BoxFuture<'a, ApiResult<(Known, Vec<native::Facts<Source>>)>> {
        <Self as Effect>::prepare(ctx, conn, caller, (), body)
    }
    fn require_initial<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        known: &'a Known,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(ctx, conn, principal, known.node))
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        known: Known,
    ) -> BoxFuture<'a, ApiResult<Fetched>> {
        Box::pin(async move {
            let origin = CapturedOperation::capture(caller).map_err(ApiError::internal)?;
            let opened = match orch_engine::imports::open(
                &ctx.app.engine,
                &known.request.request.source,
                &known.limits,
            )
            .await
            {
                Ok(opened) => Arc::new(opened),
                Err(error) => {
                    let mut conn = ctx.app.db.acquire().await?;
                    let error = failed(&mut conn, error.into()).await;
                    return Err(if native::modern(caller) {
                        authorization::problem_without_content(error)
                    } else {
                        error
                    });
                }
            };
            Ok(Fetched {
                known,
                origin,
                opened,
            })
        })
    }
}

impl native::Selected for Planning {
    type Source = Source;
    type Query = ((), ImportPreflightRequest);
    type Value = Ready;
    type Reply = ImportPlan;
    const DECLARATION: &'static Declaration<Source> = &PREFLIGHT;
    fn origin<'a>(
        _: &((), ImportPreflightRequest),
        value: &'a Ready,
    ) -> Option<&'a CapturedOperation> {
        Some(&value.fetched.origin)
    }
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let fetched = effects::initial_value::<Self>(ctx)?;
            let (_, mut facts, principal) =
                inputs(ctx, conn, caller, query.1.clone(), Some(&fetched)).await?;
            let plan = match orch_engine::imports::build(
                &ctx.app.engine,
                conn,
                &principal,
                fetched.known.node,
                &fetched.known.request.request,
                Arc::clone(&fetched.opened),
            )
            .await
            {
                Ok(plan) => plan,
                Err(error) => return Err(failed(conn, error).await),
            };
            let (existing, accounts) = matches(&plan)?;
            let mut ready = Ready {
                fetched,
                plan: Some(Arc::new(plan)),
                existing,
                accounts,
                targets: BTreeMap::new(),
            };
            if native::modern(caller) {
                context(conn, &principal, &mut ready, &mut facts).await?;
            }
            Ok((ready, facts))
        })
    }
    fn refresh<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
        opened: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (_, mut facts, principal) =
                inputs(ctx, conn, caller, query.1.clone(), Some(&opened.fetched)).await?;
            let mut ready = Ready {
                fetched: Arc::clone(&opened.fetched),
                plan: None,
                existing: opened.existing.clone(),
                accounts: opened.accounts.clone(),
                targets: BTreeMap::new(),
            };
            context(conn, &principal, &mut ready, &mut facts).await?;
            Ok((ready, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { require(ctx, conn, principal, ctx.id("id")?).await })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        value: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<ImportPlan>> {
        Box::pin(async move {
            let plan = value
                .plan
                .as_ref()
                .ok_or_else(|| ApiError::internal("the native import plan was not prepared"))?;
            match ctx
                .app
                .engine
                .plan_view(conn, plan, &value.fetched.known.request.view)
                .await
            {
                Ok(reply) => Ok(reply),
                Err(error) => Err(failed(conn, error).await),
            }
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Self::Query,
        opened: &'a Ready,
        fresh: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened.targets == fresh.targets) })
    }
}
