//! Complete native deletion sources, one audience plan and independent protected disclosure.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::native::reply;

use crate::key_operations::publication;

use crate::middleware::authorization;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::document_sources as sources;

use orch_api_contract::document_sources::TreeManagementSource as Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::Policy;

use std::collections::BTreeMap;

use std::collections::BTreeSet;

use std::sync::Arc;

use std::sync::Mutex;
pub(super) struct Reviews;

pub(super) struct Proposal {
    sources: Mutex<Option<Arc<domain::DeletionSources>>>,
    request: domain::Request,
}

pub(super) struct Review {
    plan: domain::Plan,
    request: domain::Request,
    actor: Caller,
}

pub(super) struct Subject {
    sources: Arc<domain::DeletionSources>,
}

fn refused() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the original deletion preview is no longer readable",
    )
}

async fn role_facts(
    conn: &mut Conn,
    rows: &[orch_db::queries::documents::containers::EntryRow],
    source: Source,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let ids = rows.iter().map(|row| row.id).collect::<Vec<_>>();
    let mut actual = crate::routes::documents::read_operation::identities(conn, &ids).await?;
    let mut groups: BTreeMap<KeyTargetKind, Vec<_>> = BTreeMap::new();
    for row in rows {
        let kind = match row.kind.as_str() {
            "document" => ObjectKind::Document,
            "container" => ObjectKind::DocumentContainer,
            _ => {
                return Err(ApiError::internal(
                    "the native deletion has an invalid entry kind",
                ));
            }
        };
        let target = actual.remove(&row.id).ok_or_else(ApiError::not_found)?;
        let family = KeyTargetKind::Object(kind);
        if target.object.kind != family {
            return Err(ApiError::not_found());
        }
        groups.entry(family).or_default().push(target);
    }
    Ok(vec![native::Facts::collection(
        source,
        groups.into_iter().collect(),
    )])
}

async fn facts(
    conn: &mut Conn,
    selected: &domain::DeletionSources,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let mut facts = role_facts(conn, selected.roots(), Source::Selected).await?;
    facts.extend(role_facts(conn, selected.entries(), Source::Affected).await?);
    Ok(facts)
}

fn prepared(value: &Proposal) -> ApiResult<Arc<domain::DeletionSources>> {
    value
        .sources
        .lock()
        .map_err(|_| ApiError::internal("the native deletion source is poisoned"))?
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::internal("the native deletion source was consumed"))
}

impl effects::Effect for Reviews {
    type Source = Source;
    type Query = ();
    type Body = DocumentTreeOperationBody;
    type Value = Proposal;
    type Output = Arc<Review>;
    fn declaration(_value: &Proposal) -> &'static Declaration<Source> {
        &sources::TREE_DELETE_PREVIEW
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: DocumentTreeOperationBody,
    ) -> BoxFuture<'a, ApiResult<(Proposal, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            if body.command != DocumentTreeCommand::Delete || body.deletion_plan.is_some() {
                return Err(ApiError::invalid(
                    "command",
                    "only deletion has a fresh preview",
                ));
            }
            let request = super::request(&body)?;
            let roots = domain::deletion_roots_in(conn, &request).await;
            let roots = super::result(conn, roots).await?;
            let known = native::bind_known(
                ctx,
                &sources::TREE_DELETE_PREVIEW,
                &role_facts(conn, &roots, Source::Selected).await?,
            )?;
            // Selected roots admit only this native preparation prefix. The complete
            // recursively locked set remains a separate mandatory, unresolved role.
            let actor = native::preparation_principal(ctx, conn, caller, &known).await?;
            let selected =
                domain::prepare_deletion_sources_in(conn, domain::Actor::Person(&actor), &request)
                    .await;
            let selected = super::result(conn, selected).await?;
            let facts = facts(conn, &selected).await?;
            Ok((
                Proposal {
                    sources: Mutex::new(Some(Arc::new(selected))),
                    request,
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
            let selected = prepared(value)?;
            let required = domain::require_deletion_sources_in(
                conn,
                domain::Actor::Person(principal),
                &selected,
                principal.now(),
            )
            .await;
            super::result(conn, required).await
        })
    }
}

impl effects::Apply for Reviews {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Proposal,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Arc<Review>>>> {
        Box::pin(async move {
            let selected = value
                .sources
                .lock()
                .map_err(|_| ApiError::internal("the native deletion source is poisoned"))?
                .take()
                .ok_or_else(|| ApiError::internal("the native deletion source was consumed"))?;
            let selected = Arc::try_unwrap(selected)
                .map_err(|_| ApiError::internal("the native deletion source is still in use"))?;
            let deadline = before.deadline();
            deadline.check()?;
            // The native audience is computed once. A preview has no first business effect.
            let plan = domain::complete_deletion_plan_in(
                conn,
                domain::Actor::Person(principal),
                selected,
                principal.now(),
                super::options(ctx),
            )
            .await;
            let plan = super::result(conn, plan).await?;
            deadline.check()?;
            Ok(effects::Outcome::Unchanged(Arc::new(Review {
                plan,
                request: value.request.clone(),
                actor: principal.caller().clone(),
            })))
        })
    }
}

async fn current(
    conn: &mut Conn,
    principal: &Principal,
    input: &Review,
    source: &domain::DeletionSources,
) -> ApiResult<()> {
    let matches = domain::check_deletion_plan_in(
        conn,
        domain::Actor::Person(principal),
        source,
        &input.plan.stamp,
        principal.now(),
    )
    .await;
    if !super::result(conn, matches).await? {
        return Err(refused());
    }
    if native::modern(principal.caller()) {
        let retained = source
            .entries()
            .iter()
            .filter(|row| row.deleted_at.is_some())
            .map(|row| row.id)
            .collect::<Vec<_>>();
        orch_db::queries::trace::read_scope(&retained, async {
            for row in source.entries() {
                authorization::require_document(
                    conn,
                    principal,
                    row.id,
                    Permission::DocView,
                    Policy::default(),
                )
                .await?;
            }
            Ok::<(), ApiError>(())
        })
        .await?;
    }
    Ok(())
}

impl reply::Projection for Reviews {
    type Source = Source;
    type Input = Arc<Review>;
    type Subject = Subject;
    type Value = ();
    type Reply = DocumentTreePlan;
    const READ_SOURCE: Source = Source::Affected;
    fn declaration(_input: &Arc<Review>) -> &'static Declaration<Source> {
        &sources::TREE_DELETE_PREVIEW
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Arc<Review>,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            // The actual accepted credential supplies identity for this factual lookup only.
            // Current permission is checked separately on the framework's fresh reader below.
            let actor =
                authorization::load_principal(conn, input.actor.clone(), ctx.app.clock.time())
                    .await?;
            let selected = domain::prepare_deletion_sources_in(
                conn,
                domain::Actor::Person(&actor),
                &input.request,
            )
            .await;
            let selected = super::result(conn, selected).await?;
            let facts = facts(conn, &selected).await?;
            Ok((
                Subject {
                    sources: Arc::new(selected),
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Arc<Review>,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(current(conn, principal, input, &subject.sources))
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Arc<Review>,
        _subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Arc<Review>,
        _prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<DocumentTreePlan>> {
        Box::pin(async move { super::convert(input.plan.clone()) })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Arc<Review>,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let old = opened
                .subject
                .sources
                .entries()
                .iter()
                .map(|row| (row.kind.as_str(), row.id))
                .collect::<BTreeSet<_>>();
            let current = fresh
                .subject
                .sources
                .entries()
                .iter()
                .map(|row| (row.kind.as_str(), row.id))
                .collect::<BTreeSet<_>>();
            Ok(old == current)
        })
    }
}

pub(super) async fn preview(
    ctx: Ctx,
    (): (),
    _body: DocumentTreeOperationBody,
) -> ApiResult<DocumentTreeDeletePreviewResponse> {
    let (mut tx, initial) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply::<Reviews>(&ctx, &mut tx).await?;
    initial.check_deadline(&completed.ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    initial.check_deadline(&completed.ctx)?;
    let safe = DocumentTreeDeletePreviewResponse::Receipt(DocumentTreeDeletePreviewReceipt {
        projection: ReceiptProjection::Receipt,
        deletion_plan: super::confirmation::seal(&completed.output.plan.stamp)?,
    });
    publication::receipt(&completed.ctx, &safe)?;
    Ok(
        match reply::render::<Reviews>(&completed.ctx, &completed.accepted, completed.output)
            .await?
        {
            Some(plan) => DocumentTreeDeletePreviewResponse::full(plan),
            None => safe,
        },
    )
}
