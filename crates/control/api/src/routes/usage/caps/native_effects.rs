//! Cap effects keep their native typed operands through the shared guarded adapter.
use super::*;
use crate::key_operations::native::effects::{self, Apply, Effect, FirstEffect, Outcome};
use crate::key_operations::{native, publication, targets};
use crate::routes::usage::native as source_native;
use futures::future::BoxFuture;
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::usage::{UsageCapReceipt, UsageCapWriteResponse};
use orch_api_contract::usage_sources::{self as sources, Source};
use orch_authz::Caller;

pub(crate) struct Creating {
    id: Id,
    node: Option<Id>,
    provider: Option<Id>,
    checked: Checked,
}
pub(crate) struct Updating {
    cap: CapRow,
    body: UsageCapUpdateBody,
    changing: Changing,
}
pub(crate) enum ResultCap {
    Changed(CapRow),
    Stale(CapRow),
}
pub(crate) struct Create;
pub(crate) struct Update;
#[derive(Clone, Copy)]
pub(crate) enum Mutation {
    Creation,
    Change,
}
pub(crate) trait CapEffect: Apply<Source = Source, Output = ResultCap> {
    const ROW: &'static Declaration<Source>;
    const MUTATION: Mutation;
}
impl CapEffect for Create {
    const ROW: &'static Declaration<Source> = &sources::CAP_CREATE;
    const MUTATION: Mutation = Mutation::Creation;
}
impl CapEffect for Update {
    const ROW: &'static Declaration<Source> = &sources::CAP_UPDATE;
    const MUTATION: Mutation = Mutation::Change;
}

impl Creating {
    fn row<'a>(&'a self, ctx: &Ctx, user: Id) -> NewCap<'a> {
        let checked = &self.checked;
        NewCap {
            id: self.id,
            node_id: self.node,
            provider_id: self.provider,
            subject_kind: checked.subject_kind,
            subject_id: checked.subject_id,
            subject_role: checked.subject_role.as_deref(),
            split: checked.split,
            models: &checked.models,
            services: &checked.services,
            purposes: &checked.purposes,
            tiers: &checked.tiers,
            harness_kinds: &checked.harness_kinds,
            meter: checked.meter,
            period: checked.period,
            counted_from: checked.counted_from,
            amount: checked.amount.as_deref(),
            at_limit: checked.at_limit,
            warn_percent: checked.warn_percent,
            uncounted: checked.uncounted,
            note: &checked.note,
            created_by: user,
            at: ctx.now,
        }
    }
}
impl Updating {
    fn change(&self) -> Change<'_> {
        let changing = &self.changing;
        Change {
            models: changing.models.as_deref(),
            services: changing.services.as_deref(),
            purposes: changing.purposes.as_deref(),
            tiers: changing.tiers.as_deref(),
            harness_kinds: changing.harness_kinds.as_deref(),
            counted_from: changing.counted_from,
            amount: changing.amount.as_ref().map(Option::as_deref),
            at_limit: changing.at_limit,
            warn_percent: changing.warn_percent,
            uncounted: changing.uncounted,
            note: changing.note.as_deref(),
            enabled: changing.enabled,
        }
    }
}
async fn subject_nodes(
    conn: &mut Conn,
    home: Home,
    subject: Option<(&str, Option<Id>)>,
) -> ApiResult<Vec<Id>> {
    let mut nodes = Vec::new();
    if let Home::Provider {
        node: Some(node), ..
    } = home
    {
        nodes.push(node);
    }
    match subject {
        Some((subject::GROUP, Some(group))) => {
            let found = access::groups_full(conn, &[group]).await?;
            let found = found
                .first()
                .ok_or_else(|| invalid("subject.id", "names no group"))?;
            nodes.push(found.org_id);
        }
        Some((subject::ROLE | subject::MEMBERS, Some(node))) => nodes.push(node),
        _ => {}
    }
    nodes.sort_unstable();
    nodes.dedup();
    Ok(nodes)
}
async fn required_facts(
    conn: &mut Conn,
    home: Home,
    nodes: Option<&[Id]>,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let target = source_native::home(conn, home).await?;
    let mut facts = vec![native::Facts::one(
        Source::CapHome,
        target.object.kind,
        target,
    )];
    if let Some(nodes) = nodes {
        let mut named = Vec::new();
        for &node in nodes {
            named.push(targets::node(conn, node).await?)
        }
        facts.push(source_native::collection(Source::Prerequisites, named));
    }
    Ok(facts)
}

async fn require_subject(
    conn: &mut Conn,
    principal: &Principal,
    checked: &Checked,
) -> ApiResult<()> {
    match (checked.subject_kind, checked.subject_id) {
        (subject::USER, Some(user)) => {
            if !refusals::user_refs(conn, &[user])
                .await?
                .contains_key(&user)
            {
                return Err(invalid("subject.id", "names nobody"));
            }
        }
        (subject::GROUP, Some(group)) => {
            let found = access::groups_full(conn, &[group]).await?;
            let found = found
                .first()
                .ok_or_else(|| invalid("subject.id", "names no group"))?;
            authorization::require(
                conn,
                principal,
                found.org_id,
                authorization::VIEW,
                Act::Use,
                Policy::default(),
            )
            .await?;
        }
        (subject::ROLE | subject::MEMBERS, Some(source)) => {
            let (_, kind) = crate::routes::nodes::placed(conn, source).await?;
            if kind == NodeKind::Thread
                || (checked.subject_kind == subject::MEMBERS && kind != NodeKind::Org)
            {
                return Err(invalid("subject.at", "is no place of that kind"));
            }
            authorization::require(
                conn,
                principal,
                source,
                authorization::VIEW,
                Act::Use,
                Policy::default(),
            )
            .await?;
        }
        _ => {}
    }
    Ok(())
}
fn private_error(principal: &Principal, error: ApiError) -> ApiError {
    if native::modern(principal.caller()) {
        authorization::problem_without_content(error)
    } else {
        error
    }
}
/// Preserve the native early ordinary gate before validation and lock contention.
/// This admits only real known Home facts for effect-free preparation, never a completed writer.
async fn preliminary_manage(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    declaration: &'static Declaration<Source>,
    home: Home,
) -> ApiResult<Principal> {
    let target = source_native::home(conn, home).await?;
    let known = native::bind_known(
        ctx,
        declaration,
        &[native::Facts::one(
            Source::CapHome,
            target.object.kind,
            target,
        )],
    )?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    require(conn, &principal, home, Need::Manage)
        .await
        .map_err(|error| private_error(&principal, error))?;
    Ok(principal)
}
impl Effect for Create {
    type Source = Source;
    type Query = ();
    type Body = CapInput;
    type Value = Creating;
    type Output = ResultCap;
    fn declaration(_value: &Creating) -> &'static Declaration<Source> {
        &sources::CAP_CREATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: CapInput,
    ) -> BoxFuture<'a, ApiResult<(Creating, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let home = Home::of(body.node_id, body.provider_id);
            let principal =
                preliminary_manage(ctx, conn, caller, &sources::CAP_CREATE, home).await?;
            let value = Creating {
                id: new_id(),
                node: body.node_id,
                provider: body.provider_id,
                checked: check(&body)?,
            };
            require_subject(conn, &principal, &value.checked)
                .await
                .map_err(|error| private_error(&principal, error))?;
            // The same native duplicate-identity lock is held through the actual insert.
            rows::lock_insert(conn, &value.row(ctx, caller.user())).await?;
            let nodes = subject_nodes(
                conn,
                home,
                Some((value.checked.subject_kind, value.checked.subject_id)),
            )
            .await?;
            let facts = required_facts(conn, home, Some(&nodes)).await?;
            Ok((value, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Creating,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require(
                conn,
                principal,
                Home::of(value.node, value.provider),
                Need::Manage,
            )
            .await
            .map_err(|error| private_error(principal, error))?;
            require_subject(conn, principal, &value.checked)
                .await
                .map_err(|error| private_error(principal, error))
        })
    }
}
impl Apply for Create {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Creating,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<ResultCap>>> {
        Box::pin(async move {
            let cap=rows::insert_before(conn,&value.row(ctx,principal.user()),move||before.check()).await?
            .ok_or_else(||ApiError::new(ErrorCode::ConflictExists,"a cap with the same node, provider, subject, split, models, tiers, harness kinds, meter and period exists"))?;
            record(
                ctx,
                conn,
                orch_db::audit::action!(AuditAction::UsageCapCreated),
                &cap,
                "added",
                about(&cap),
            )
            .await?;
            orch_db::idempotency::creation::observe_with(
                conn,
                Some(principal.user()),
                "usage_cap",
                cap.id,
                json!({"revision":cap.revision}),
            )
            .await?;
            Ok(Outcome::Applied(ResultCap::Changed(cap)))
        })
    }
}
impl Effect for Update {
    type Source = Source;
    type Query = ();
    type Body = UsageCapUpdateBody;
    type Value = Updating;
    type Output = ResultCap;
    fn declaration(_value: &Updating) -> &'static Declaration<Source> {
        &sources::CAP_UPDATE
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        _caller: &'a Caller,
        (): (),
        body: UsageCapUpdateBody,
    ) -> BoxFuture<'a, ApiResult<(Updating, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let early = rows::get(conn, _ctx.id("id")?)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let preliminary = preliminary_manage(
                _ctx,
                conn,
                _caller,
                &sources::CAP_UPDATE,
                engine::home_of(&early),
            )
            .await?;
            let cap = rows::lock(conn, _ctx.id("id")?)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if touched(&body).is_empty() {
                return Err(invalid("", "names nothing to change"));
            }
            let changing =
                check_change(&cap, &body).map_err(|error| private_error(&preliminary, error))?;
            let home = engine::home_of(&cap);
            let facts = required_facts(conn, home, None).await?;
            Ok((
                Updating {
                    cap,
                    body,
                    changing,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Updating,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require(conn, principal, engine::home_of(&value.cap), Need::Manage)
                .await
                .map_err(|error| private_error(principal, error))
        })
    }
}
impl Apply for Update {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a Updating,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<ResultCap>>> {
        Box::pin(async move {
            let outcome = rows::update_before(
                conn,
                value.cap.id,
                value.body.expected_revision,
                &value.change(),
                ctx.now,
                move || before.check(),
            )
            .await?;
            let changed = match outcome {
                Updated::Done(changed) => *changed,
                Updated::Stale(current) => {
                    return Ok(Outcome::Unchanged(ResultCap::Stale(*current)));
                }
                Updated::Exists => {
                    return Err(ApiError::new(
                        ErrorCode::ConflictExists,
                        "with these models, tiers and harness kinds it would be a cap that exists",
                    ));
                }
                Updated::Gone => return Err(ApiError::not_found()),
            };
            let mut details = about(&changed);
            details["changed"] = json!(touched(&value.body));
            details["revision"] = json!(changed.revision);
            record(
                ctx,
                conn,
                orch_db::audit::action!(AuditAction::UsageCapUpdated),
                &changed,
                "changed",
                details,
            )
            .await?;
            Ok(Outcome::Applied(ResultCap::Changed(changed)))
        })
    }
}

/// The original writer owns its commit, audit and engine notification; only effect-free readback retries.
pub(super) async fn run<F>(ctx: Ctx) -> ApiResult<UsageCapWriteResponse>
where
    F: CapEffect,
{
    let mut tx = ctx.app.db.begin().await?;
    let completed = effects::apply::<F>(&ctx, &mut tx).await?;
    let ctx = completed.ctx;
    let accepted = completed.accepted;
    let (cap, stale) = match completed.output {
        ResultCap::Changed(cap) => (cap, false),
        ResultCap::Stale(cap) => (cap, true),
    };
    if stale {
        tx.rollback().await.map_err(orch_db::DbError::from)?;
        let current = project(&ctx, &accepted, cap, F::ROW, F::MUTATION, false).await?;
        let error = ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the cap was changed since it was read",
        );
        return Err(match current {
            Some(cap) => error.with("current", cap),
            None => error,
        });
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let receipt = UsageCapReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        id: cap.id,
        revision: cap.revision,
    };
    publication::receipt(&ctx, &receipt)?;
    engine_told(&ctx, cap.id).await;
    match project(&ctx, &accepted, cap, F::ROW, F::MUTATION, false).await? {
        Some(cap) => Ok(UsageCapWriteResponse::full(cap)),
        None => Ok(UsageCapWriteResponse::Receipt(receipt)),
    }
}

async fn response_plan(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    cap: &CapRow,
    declaration: &'static Declaration<Source>,
    mutation: Mutation,
) -> ApiResult<(
    crate::registry::OperationPlan,
    NameSources,
    Vec<Id>,
    NameBounds,
)> {
    let home = engine::home_of(cap);
    let subject = matches!(mutation, Mutation::Creation)
        .then_some((cap.subject_kind.as_str(), cap.subject_id));
    let nodes = subject_nodes(conn, home, subject).await?;
    let mut facts = required_facts(
        conn,
        home,
        matches!(mutation, Mutation::Creation).then_some(nodes.as_slice()),
    )
    .await?;
    let (names, providers) = Known::sources(&[cap], &[]);
    let groups = crate::routes::access::operations::group_targets(
        conn,
        &names.groups.iter().copied().collect::<Vec<_>>(),
    )
    .await?;
    facts.push(source_native::collection(Source::Groups, groups));
    let (bounds, named) =
        source_native::names(ctx, conn, caller, declaration, names.clone()).await?;
    facts.push(named);
    Ok((
        native::bind(ctx, declaration, &facts)?,
        names,
        providers,
        bounds,
    ))
}
async fn same_cap(conn: &mut Conn, cap: &CapRow) -> ApiResult<()> {
    let current = rows::get(conn, cap.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if (
        current.node_id,
        current.provider_id,
        &current.subject_kind,
        current.subject_id,
        &current.subject_role,
    ) != (
        cap.node_id,
        cap.provider_id,
        &cap.subject_kind,
        cap.subject_id,
        &cap.subject_role,
    ) {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "the stored cap source association changed",
        ));
    }
    Ok(())
}
async fn project(
    ctx: &Ctx,
    accepted: &Caller,
    cap: CapRow,
    declaration: &'static Declaration<Source>,
    mutation: Mutation,
    replaying: bool,
) -> ApiResult<Option<UsageCap>> {
    if !native::modern(accepted) && !replaying {
        return native::completed(ctx, accepted, move |_ctx, conn, reader| {
            Box::pin(async move {
                Ok(Some(
                    Known::load(conn, reader, &[&cap], &[])
                        .await?
                        .cap(&cap, true)?,
                ))
            })
        })
        .await;
    }
    let retained = cap.clone();
    let projected = native::completed(ctx, accepted, move |ctx, conn, reader| {
        Box::pin(async move {
            same_cap(conn, &cap).await?;
            let (plan, names, providers, bounds) =
                response_plan(ctx, conn, reader.caller(), &cap, declaration, mutation).await?;
            let earlier = plan.clone();
            let projection = response_projection(&plan, replaying);
            let result = plan
                .read_response_projection(
                    ctx,
                    conn,
                    reader.caller(),
                    projection,
                    move |conn, principal| {
                        Box::pin(async move {
                            require(conn, principal, engine::home_of(&cap), Need::View).await?;
                            let can_edit = if replaying {
                                may(conn, principal, engine::home_of(&cap), Need::Manage).await?
                            } else {
                                true
                            };
                            Known::project(conn, principal, &names, &providers, Some(&bounds))
                                .await?
                                .cap(&cap, can_edit)
                        })
                    },
                )
                .await?;
            Ok(result.map(|reply| (reply, earlier)))
        })
    })
    .await?;
    let Some((reply, earlier)) = projected else {
        return Ok(None);
    };
    publication::retain(
        ctx,
        accepted,
        (retained, earlier, declaration, mutation, replaying),
        |ctx, conn, reader, (cap, earlier, declaration, mutation, replaying)| {
            Box::pin(async move {
                same_cap(conn, cap).await?;
                let (fresh, _, _, _) =
                    response_plan(ctx, conn, reader.caller(), cap, declaration, *mutation).await?;
                if !fresh.shared().covers_sources(earlier.shared()) {
                    return Err(ApiError::new(
                        ErrorCode::AuthzDenied,
                        "an earlier cap response source is no longer admitted",
                    ));
                }
                let home = engine::home_of(cap);
                let projection = response_projection(&fresh, *replaying);
                fresh
                    .read_response_projection(
                        ctx,
                        conn,
                        reader.caller(),
                        projection,
                        move |conn, principal| {
                            Box::pin(
                                async move { require(conn, principal, home, Need::View).await },
                            )
                        },
                    )
                    .await?
                    .ok_or_else(|| {
                        ApiError::new(
                            ErrorCode::AuthzDenied,
                            "the cap response is no longer readable",
                        )
                    })
            })
        },
    )?;
    Ok(Some(reply))
}

/// Use the same cap content sources for a new credential's completed read.
/// Native creation prerequisites have already been satisfied by the recorded effect.
fn response_projection(
    plan: &crate::registry::OperationPlan,
    replaying: bool,
) -> crate::key_operations::admission::ResponseProjection {
    use crate::key_operations::admission::ResponseProjection;
    if replaying {
        ResponseProjection::Replay(
            plan.variant()
                .facets
                .iter()
                .enumerate()
                .filter_map(|(index, facet)| {
                    (facet.usage == orch_api_contract::operations::FacetUsage::ResponseContent)
                        .then_some(index)
                })
                .collect(),
        )
    } else {
        ResponseProjection::CompletedContent
    }
}

pub(crate) fn replay_binding()
-> crate::middleware::idempotency::Creation<(), CapInput, UsageCapWriteResponse> {
    crate::middleware::idempotency::Creation {
        kind: "usage_cap",
        replay,
        identity: |reply| {
            let (id, revision) = match reply {
                UsageCapWriteResponse::Full(cap) => (cap.id, cap.revision),
                UsageCapWriteResponse::Receipt(cap) => (cap.id, cap.revision),
            };
            Ok(orch_db::idempotency::creation::Created {
                kind: "usage_cap".into(),
                id,
                data: json!({"revision":revision}),
            })
        },
    }
}
fn replay(
    ctx: Ctx,
    (): (),
    _body: CapInput,
    created: orch_db::idempotency::creation::Created,
) -> BoxFuture<'static, ApiResult<UsageCapWriteResponse>> {
    Box::pin(async move {
        let receipt = UsageCapWriteResponse::Receipt(UsageCapReceipt {
            projection: orch_api_contract::common::ReceiptProjection::Receipt,
            id: created.id,
            revision: serde_json::from_value(
                created
                    .data
                    .get("revision")
                    .cloned()
                    .ok_or_else(|| ApiError::internal("the created cap has no revision"))?,
            )
            .map_err(ApiError::internal)?,
        });
        publication::receipt(&ctx, &receipt)?;
        let ctx = ctx.for_completed_replay_projection();
        let result = async {
            let accepted = authorization::caller_of(ctx.user()?)?;
            let cap = rows::get(&mut *ctx.app.db.acquire().await?, created.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            project(
                &ctx,
                &accepted,
                cap,
                &sources::CAP_CREATE,
                Mutation::Creation,
                true,
            )
            .await
        }
        .await;
        match result {
            Ok(Some(cap)) => Ok(UsageCapWriteResponse::full(cap)),
            Ok(None) => Ok(receipt),
            Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => Ok(receipt),
            Err(error) => Err(error),
        }
    })
}
