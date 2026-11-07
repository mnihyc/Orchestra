//! Native stored sharing rows and factual projections use the shared adapters.
use super::*;
use crate::key_operations::native;
use futures::future::BoxFuture;
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::share_sources::{self as sources, Source};
use std::collections::BTreeSet;

fn collection(source: Source, rows: Vec<KeyResolvedTarget>) -> native::Facts<Source> {
    let mut groups = std::collections::BTreeMap::new();
    for row in rows {
        groups
            .entry(row.object.kind)
            .or_insert_with(Vec::new)
            .push(row);
    }
    native::Facts::collection(source, groups.into_iter().collect())
}
fn permits(
    ctx: &Ctx,
    caller: &Caller,
    declaration: &'static Declaration<Source>,
    role: Source,
    target: &KeyResolvedTarget,
) -> ApiResult<bool> {
    if !native::modern(caller) {
        return Ok(true);
    }
    let plan = native::bind_known(ctx, declaration, &[])?;
    let usage = declaration
        .bindings
        .iter()
        .find(|b| b.source == role)
        .ok_or_else(|| ApiError::internal("the stored share role is undeclared"))?
        .usage;
    native::permits_usage(
        &plan,
        declaration,
        caller,
        &role,
        usage,
        target.object.kind,
        target,
    )
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Root {
    Source(Subject),
    Recipient(Id),
}
fn root(ctx: &Ctx, query: &AgentShareListQuery) -> ApiResult<Root> {
    let id = ctx.id("id")?;
    Ok(
        match (ctx.param("object")?, query.side.unwrap_or_default()) {
            ("nodes", AgentShareSide::Of) => Root::Source(Subject::Node(id)),
            ("nodes", AgentShareSide::To) => Root::Recipient(id),
            ("documents", AgentShareSide::Of) => Root::Source(Subject::Document(id)),
            ("chats", AgentShareSide::Of) => Root::Source(Subject::Chat(id)),
            ("documents" | "chats", AgentShareSide::To) => {
                return Err(ApiError::invalid("side", "is for a node"));
            }
            _ => return Err(ApiError::not_found()),
        },
    )
}
async fn root_target(conn: &mut Conn, root: Root) -> ApiResult<KeyResolvedTarget> {
    match root {
        Root::Source(subject) => source(conn, &what_of(subject)).await,
        Root::Recipient(id) => targets::node(conn, id).await,
    }
}
async fn selected(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    root: Root,
) -> ApiResult<Vec<Record>> {
    let result = match root {
        Root::Source(subject) => {
            ctx.app
                .engine
                .shares_of_in(conn, principal, &origin(ctx), subject, ctx.now)
                .await
        }
        Root::Recipient(id) => {
            ctx.app
                .engine
                .shares_to_place_in(conn, principal, &origin(ctx), id, ctx.now)
                .await
        }
    };
    match result {
        Ok(records) => Ok(records),
        Err(error) => Err(failure(ctx, conn, error).await),
    }
}
async fn wanted_names(
    conn: &mut Conn,
    principal: &Principal,
    records: &[Record],
) -> ApiResult<Vec<Id>> {
    let mut names = BTreeSet::new();
    for record in records {
        let target = source(conn, &what_of(record.subject)).await?;
        names.extend(target.node_ancestors.iter().filter_map(|n| n.id));
        if matches!(target.object.kind,KeyTargetKind::Object(kind) if kind.node().is_some()) {
            names.extend(target.object.id);
        }
        names.insert(record.to);
        names.extend(record.grants.iter().map(|g| g.holder_id));
        if let Subject::Document(id) = record.subject {
            let row = document_rows::get(conn, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let listed = document_access::listed(conn, principal, &[id], Policy::default())
                .await
                .map_err(ApiError::internal)?;
            if let Some(listed) = listed.first() {
                names.extend(
                    document_views::ProjectedDocumentRef::candidates(conn, &row, listed)
                        .await?
                        .into_iter()
                        .filter_map(|source| match source {
                            document_views::WrittenLabelSource::Node(id) => Some(id),
                            _ => None,
                        }),
                );
            }
        }
    }
    Ok(names.into_iter().collect())
}
fn named_facts(names: &[KeyResolvedTarget]) -> Vec<native::Facts<Source>> {
    [
        (Source::Orgs, ObjectKind::Org),
        (Source::Projects, ObjectKind::Project),
        (Source::Topics, ObjectKind::Topic),
        (Source::Threads, ObjectKind::Thread),
    ]
    .into_iter()
    .map(|(source, kind)| {
        collection(
            source,
            names
                .iter()
                .filter(|t| t.object.kind == KeyTargetKind::Object(kind))
                .cloned()
                .collect(),
        )
    })
    .collect()
}
pub(crate) struct List;
pub(crate) struct PreparedList {
    root: Root,
    records: Vec<Record>,
    projection: ShareProjection,
}
impl native::Selected for List {
    type Source = Source;
    type Query = AgentShareListQuery;
    type Value = PreparedList;
    type Reply = Vec<AgentShare>;
    const DECLARATION: &'static Declaration<Source> = &sources::LIST;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(select(ctx, conn, caller, query))
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
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move { Ok(value.projection.shares.clone()) })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a Self::Query,
        old: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.root == fresh.root
                && old.records.iter().all(|old| {
                    fresh.records.iter().any(|new| {
                        old.id == new.id
                            && old.subject == new.subject
                            && old.to == new.to
                            && old.by == new.by
                            && old.input == new.input
                            && old.grants.iter().all(|g| {
                                new.grants.iter().any(|n| {
                                    n.entry_id == g.entry_id
                                        && n.object_kind == g.object_kind
                                        && n.object_id == g.object_id
                                        && n.holder_id == g.holder_id
                                        && n.created_by == g.created_by
                                })
                            })
                    })
                })
                && old.projection.named.is_subset(&fresh.projection.named)
                && old
                    .projection
                    .inputs
                    .iter()
                    .all(|input| fresh.projection.inputs.contains(input)))
        })
    }
}
async fn select(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: &AgentShareListQuery,
) -> ApiResult<(PreparedList, Vec<native::Facts<Source>>)> {
    let root = root(ctx, query)?;
    let target = root_target(conn, root).await?;
    let root_fact = native::Facts::one(Source::Root, target.object.kind, target);
    let known = native::bind_known(ctx, &sources::LIST, std::slice::from_ref(&root_fact))?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    let candidates = selected(ctx, conn, &principal, root).await?;
    let mut records = Vec::new();
    let mut subjects = Vec::new();
    let mut inputs = Vec::new();
    for record in candidates {
        let source = source(conn, &what_of(record.subject)).await?;
        if !permits(ctx, caller, &sources::LIST, Source::Results, &source)? {
            continue;
        }
        if let Some(id) = record.input {
            let row: Option<(Id, Id)> =
                sqlx::query_as("SELECT thread_id,continuation_id FROM thread_inputs WHERE id=$1")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
            if let Some((thread, _)) = row {
                let input = targets::node(conn, thread).await?;
                if !permits(ctx, caller, &sources::LIST, Source::Inputs, &input)? {
                    continue;
                }
                inputs.push(input);
            }
        }
        records.push(record);
        subjects.push(source);
    }
    let mut facts = vec![
        root_fact,
        collection(Source::Results, subjects),
        collection(Source::Inputs, inputs),
    ];
    let known = native::bind_known(ctx, &sources::LIST, &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    let names = wanted_names(conn, &principal, &records).await?;
    let allowed = name_bindings(ctx, conn, caller, &known, &names).await?;
    let bounds = crate::routes::access::page::NameBounds {
        limited: native::modern(caller),
        nodes: allowed.iter().filter_map(|t| t.object.id).collect(),
        containers: Default::default(),
    };
    let projection = project_shares(
        ctx,
        conn,
        &principal,
        &records,
        match root {
            Root::Recipient(id) => Some(id),
            _ => None,
        },
        Some(&bounds),
    )
    .await?;
    let actual = allowed
        .into_iter()
        .filter(|target| {
            target
                .object
                .id
                .is_some_and(|id| projection.named.contains(&id))
        })
        .collect::<Vec<_>>();
    facts.extend(named_facts(&actual));
    Ok((
        PreparedList {
            root,
            records,
            projection,
        },
        facts,
    ))
}

pub(crate) struct Update;
pub(crate) struct Updating {
    native: orch_engine::sharing::PreparedShareUpdate,
}
pub(crate) struct Updated {
    record: Record,
    control: bool,
}
fn update_declaration(control: bool) -> &'static Declaration<Source> {
    if control {
        &sources::UPDATE_READ_FIRST
    } else {
        &sources::UPDATE_GRANT
    }
}
async fn update_facts(
    conn: &mut Conn,
    record: &Record,
    control: bool,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let target = source(conn, &what_of(record.subject)).await?;
    let mut facts = vec![native::Facts::one(
        Source::Subject,
        target.object.kind,
        target,
    )];
    if control {
        let recipient = targets::node(conn, record.to).await?;
        facts.push(native::Facts::one(
            Source::Control,
            recipient.object.kind,
            recipient,
        ));
    }
    Ok(facts)
}
impl native::effects::Effect for Update {
    type Source = Source;
    type Query = ();
    type Body = ShareAgentUpdateBody;
    type Value = Updating;
    type Output = Updated;
    fn declaration(value: &Self::Value) -> &'static Declaration<Source> {
        update_declaration(value.native.controls_recipient())
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ShareAgentUpdateBody,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let record = match ctx
                .app
                .engine
                .lock_agent_share_record_in(conn, ctx.id("share_id")?)
                .await
            {
                Ok(record) => record,
                Err(error) => return Err(failure(ctx, conn, error).await),
            };
            let control = body
                .read_first
                .is_some_and(|next| next != record.read_first);
            let declaration = update_declaration(control);
            let facts = update_facts(conn, &record, control).await?;
            let known = native::bind_known(ctx, declaration, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let mut prepared = match ctx
                .app
                .engine
                .prepare_share_update_in(conn, &principal, &origin(ctx), record.id, &body)
                .await
            {
                Ok(prepared) => prepared,
                Err(error) => return Err(failure(ctx, conn, error).await),
            };
            let control = prepared.controls_recipient();
            let actual = source(conn, &what_of(prepared.record().subject)).await?;
            let declaration = update_declaration(control);
            prepared.bound_source_name(permits(
                ctx,
                caller,
                declaration,
                Source::Content,
                &actual,
            )?);
            let facts = update_facts(conn, prepared.record(), control).await?;
            Ok((Updating { native: prepared }, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            match ctx
                .app
                .engine
                .check_agent_share_source_in(
                    conn,
                    principal,
                    &origin(ctx),
                    value.native.record().subject,
                )
                .await
            {
                Ok(()) => Ok(()),
                Err(error) => Err(failure(ctx, conn, error).await),
            }
        })
    }
}
impl native::effects::ApplyOwned for Update {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Self::Value,
        before: native::effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<native::effects::Outcome<Self::Output>>> {
        Box::pin(async move {
            let control = value.native.controls_recipient();
            let result = ctx
                .app
                .engine
                .update_prepared_share_guarded(
                    tx,
                    principal.clone(),
                    &origin(ctx),
                    &value.native,
                    move || {
                        before.check().map_err(|error| {
                            orch_engine::error::EngineError::BeforeEffect(Box::new(error))
                        })
                    },
                )
                .await;
            let result = match result {
                Ok(result) => result,
                Err(error) => {
                    let mut conn = ctx.app.db.acquire().await?;
                    return Err(failure(ctx, &mut conn, error).await);
                }
            };
            let output = Updated {
                record: result.record,
                control,
            };
            Ok(if result.changed {
                native::effects::Outcome::Applied(output)
            } else {
                native::effects::Outcome::Unchanged(output)
            })
        })
    }
}
pub(crate) async fn update(
    ctx: Ctx,
    (): (),
    _body: ShareAgentUpdateBody,
) -> ApiResult<orch_api_contract::share::ShareAgentUpdateResponse> {
    use orch_api_contract::share::{ShareAgentUpdateReceipt, ShareAgentUpdateResponse};
    let (tx, _) = transaction_guard::begin(&ctx).await?;
    let done = native::effects::apply_owned::<Update>(&ctx, tx).await?;
    let receipt = ShareAgentUpdateResponse::Receipt(ShareAgentUpdateReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        id: done.output.record.id,
    });
    crate::key_operations::publication::receipt(&done.ctx, &receipt)?;
    Ok(
        native::reply::render::<Content>(&done.ctx, &done.accepted, done.output)
            .await?
            .map_or(receipt, ShareAgentUpdateResponse::full),
    )
}
struct Content;
pub(crate) struct PreparedContent {
    record: Record,
    projection: ShareProjection,
}
impl native::reply::Projection for Content {
    type Source = Source;
    type Input = Updated;
    type Subject = Record;
    type Value = PreparedContent;
    type Reply = AgentShare;
    const READ_SOURCE: Source = Source::Content;
    fn declaration(input: &Self::Input) -> &'static Declaration<Source> {
        update_declaration(input.control)
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Self::Input,
    ) -> BoxFuture<'a, ApiResult<(Record, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let record = &input.record;
            let mut facts = update_facts(conn, record, input.control).await?;
            let source = source(conn, &what_of(record.subject)).await?;
            facts.push(native::Facts::one(
                Source::Content,
                source.object.kind,
                source,
            ));
            Ok((record.clone(), facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Self::Input,
        subject: &'a Record,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            match ctx
                .app
                .engine
                .check_agent_share_source_in(conn, principal, &origin(ctx), subject.subject)
                .await
            {
                Ok(()) => {}
                Err(error) => return Err(failure(ctx, conn, error).await),
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
        _subject: &'a Record,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let record = match ctx
                .app
                .engine
                .agent_share_record_in(conn, input.record.id, ctx.app.clock.now())
                .await
            {
                Ok(record) => record,
                Err(error) => return Err(failure(ctx, conn, error).await),
            };
            if record.subject != input.record.subject
                || record.to != input.record.to
                || record.by != input.record.by
            {
                return Err(ApiError::not_found());
            }
            let mut facts = update_facts(conn, &record, input.control).await?;
            let source = source(conn, &what_of(record.subject)).await?;
            facts.push(native::Facts::one(
                Source::Content,
                source.object.kind,
                source,
            ));
            let known = native::bind_known(ctx, update_declaration(input.control), &facts)?;
            let wanted = wanted_names(conn, principal, std::slice::from_ref(&record)).await?;
            let allowed = name_bindings(ctx, conn, principal.caller(), &known, &wanted).await?;
            let bounds = crate::routes::access::page::NameBounds {
                limited: native::modern(principal.caller()),
                nodes: allowed.iter().filter_map(|t| t.object.id).collect(),
                containers: Default::default(),
            };
            let projection = project_shares(
                ctx,
                conn,
                principal,
                std::slice::from_ref(&record),
                None,
                Some(&bounds),
            )
            .await?;
            let actual = allowed
                .into_iter()
                .filter(|target| {
                    target
                        .object
                        .id
                        .is_some_and(|id| projection.named.contains(&id))
                })
                .collect::<Vec<_>>();
            Ok((PreparedContent { record, projection }, named_facts(&actual)))
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Self::Input,
        prepared: &'a native::reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<AgentShare>> {
        Box::pin(async move {
            prepared
                .value
                .projection
                .shares
                .first()
                .cloned()
                .ok_or_else(ApiError::not_found)
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Self::Input,
        old: &'a native::reply::Prepared<Self>,
        fresh: &'a native::reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.value.record.id == fresh.value.record.id
                && old.value.record.subject == fresh.value.record.subject
                && old.value.record.to == fresh.value.record.to
                && old
                    .value
                    .projection
                    .named
                    .is_subset(&fresh.value.projection.named)
                && old.value.record.grants.iter().all(|old| {
                    fresh
                        .value
                        .record
                        .grants
                        .iter()
                        .any(|new| new.entry_id == old.entry_id)
                }))
        })
    }
}

pub(crate) struct Delete;
async fn retained_subject(conn: &mut Conn, subject: Subject) -> ApiResult<KeyResolvedTarget> {
    match subject {
        Subject::Node(id) => targets::node_identity(conn, id).await,
        Subject::Chat(id) => targets::chat_identity(conn, id).await,
        Subject::Document(id) => {
            crate::routes::nodes::read_sources::Sources::from_objects([ObjectRef {
                kind: ObjectKind::Document,
                id,
            }])
            .identity_targets(conn)
            .await?
            .remove(&ObjectRef {
                kind: ObjectKind::Document,
                id,
            })
            .ok_or_else(ApiError::not_found)
        }
    }
}
impl native::Stored for Delete {
    type Source = Source;
    type Value = Record;
    const ROWS: &'static [Declaration<Source>] = &[sources::DELETE];
    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(Record, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let record = match ctx
                .app
                .engine
                .lock_agent_share_record_in(conn, ctx.id("share_id")?)
                .await
            {
                Ok(record) => record,
                Err(error) => return Err(failure(ctx, conn, error).await),
            };
            let target = match retained_subject(conn, record.subject).await {
                Ok(target) => target,
                Err(error) if error.problem.status == 404 => {
                    return Err(ApiError::new(
                        ErrorCode::AuthzDenied,
                        "this key cannot resolve the removed share's current source identity",
                    ));
                }
                Err(error) => return Err(error),
            };
            let facts = vec![native::Facts::one(
                Source::Subject,
                target.object.kind,
                target,
            )];
            Ok((record, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _value: &'a Record,
    ) -> BoxFuture<'a, ApiResult<()>> {
        // The same native removal preparer repeats its own live/copy/creator authority below.
        Box::pin(async { Ok(()) })
    }
}
pub(crate) async fn delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    if let Err(error) = ctx
        .app
        .engine
        .lock_agent_share_record_in(&mut tx, ctx.id("share_id")?)
        .await
    {
        return Err(failure(&ctx, &mut tx, error).await);
    }
    let (current, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    let mut prepared = match ctx
        .app
        .engine
        .prepare_share_removal_in(
            &mut tx,
            guard.principal(),
            &origin(&current),
            current.id("share_id")?,
        )
        .await
    {
        Ok(prepared) => prepared,
        Err(error) => return Err(failure(&current, &mut tx, error).await),
    };
    prepared.bound_source_name(!native::modern(guard.principal().caller()));
    let principal = guard.principal().clone();
    let checking = current.clone();
    let result = current
        .app
        .engine
        .revoke_prepared_share_guarded(
            tx,
            principal,
            &origin(&current),
            &prepared,
            Some(Box::new(move || {
                guard
                    .check_deadline(&checking)
                    .map_err(|error| orch_engine::error::EngineError::BeforeEffect(Box::new(error)))
            })),
        )
        .await;
    match result {
        Ok(_) => Ok(()),
        Err(error) => {
            let mut conn = current.app.db.acquire().await?;
            Err(failure(&current, &mut conn, error).await)
        }
    }
}
