//! One bounded stored-content source reader. It never calls a model or borrows another route's admission.

use super::*;

use std::collections::BTreeSet;
/// The ordinary IO default, independently configurable at system scope for mixed-root selections.
const BATCH_SIZE: u32 = 100;

pub(super) struct View {
    targets: Vec<sources::Identity>,
    inputs: Vec<KeyResolvedTarget>,
    providers: Vec<KeyResolvedTarget>,
    reply: SummaryBatchReply,
}

pub(super) struct Reader;

fn groups(facts: Vec<KeyResolvedTarget>) -> Vec<(KeyTargetKind, Vec<KeyResolvedTarget>)> {
    let mut groups: BTreeMap<KeyTargetKind, Vec<KeyResolvedTarget>> = BTreeMap::new();
    for fact in facts {
        let rows = groups.entry(fact.object.kind).or_default();
        if !rows.contains(&fact) {
            rows.push(fact);
        }
    }
    groups.into_iter().collect()
}

impl native::Selected for Reader {
    type Source = Role;
    type Query = ((), SummaryBatchBody);
    type Value = View;
    type Reply = SummaryBatchReply;
    const DECLARATION: &'static Declaration<Role> = &declarations::RESOURCE_BATCH;
    const WATCH_ORDINARY: bool = true;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(View, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let plan = Prepared::new(
                SummaryBatchGet::ROUTE,
                declarations::RESOURCE_BATCH.operation,
            )
            .map_err(source_error)?;
            let principal = Principal::load(
                conn,
                plan.preparation_caller(caller).map_err(source_error)?,
                ctx.app.clock.time(),
            )
            .await
            .map_err(ApiError::internal)?;
            let limit = orch_settings::get::<std::num::NonZeroU32>(
                conn,
                orch_settings::Place::System,
                orch_settings_contract::registry::keys::SUMMARY_READ_BATCH_SIZE,
            )
            .await
            .map_err(ApiError::internal)?
            .map_or(BATCH_SIZE, std::num::NonZeroU32::get);
            let mut seen = BTreeSet::new();
            let distinct: Vec<_> = query
                .1
                .targets
                .iter()
                .copied()
                .filter(|target| seen.insert((target.kind(), target.id())))
                .collect();
            let start = usize::try_from(query.1.offset)
                .unwrap_or(usize::MAX)
                .min(distinct.len());
            let end = start
                .saturating_add(usize::try_from(limit).unwrap_or(usize::MAX))
                .min(distinct.len());
            let selected = &distinct[start..end];
            let mut unavailable = Vec::new();
            let mut targets = Vec::new();
            for target in selected {
                let identity = match sources::identify(conn, *target).await {
                    Ok(identity) => identity,
                    Err(sources::SourceError::Unavailable) => {
                        unavailable.push(*target);
                        continue;
                    }
                    Err(error) => return Err(ApiError::internal(error)),
                };
                if !plan
                    .permits_binding(
                        principal.caller(),
                        0,
                        identity.fact().object.kind,
                        std::slice::from_ref(identity.fact()),
                    )
                    .map_err(source_error)?
                {
                    unavailable.push(*target);
                    continue;
                }
                if let Err(error) = require_disclosure(conn, &principal, *target).await {
                    if error.problem.code == ErrorCode::NotFound
                        || error.problem.code == ErrorCode::AuthzDenied
                    {
                        unavailable.push(*target);
                        continue;
                    }
                    return Err(error);
                }
                targets.push(identity);
            }
            let ids: Vec<_> = targets.iter().map(|identity| identity.target()).collect();
            let caches = rows::get_many(conn, &ids, principal.caller().user()).await?;
            let mut inputs = Vec::new();
            let mut providers = Vec::new();
            let mut items = Vec::new();
            for identity in &targets {
                let target = identity.target();
                let personal = caches.iter().find(|row| {
                    rows::is_target(row, target)
                        && row.viewer_id == Some(principal.caller().user())
                        && row.text.is_some()
                });
                let shared = caches
                    .iter()
                    .find(|row| rows::is_target(row, target) && row.viewer_id.is_none());
                let checked = cache_read::guard(
                    conn,
                    &principal,
                    target,
                    &plan,
                    None,
                    personal.or(shared).cloned(),
                )
                .await?;
                inputs.extend(checked.inputs);
                let (producer, facts) =
                    cache_read::producer(conn, &principal, &plan, checked.cache.as_ref()).await?;
                providers.extend(facts);
                // Only current native root relationship is returned; no root name or root Read is inferred.
                let root = orch_db::queries::hierarchy::node(conn, identity.place())
                    .await?
                    .ok_or_else(ApiError::not_found)?
                    .org_id;
                items.push(SummaryProjection {
                    target,
                    root_org_id: root,
                    inspecting: checked.inspecting,
                    producer,
                    text: checked.cache.as_ref().and_then(|row| row.text.clone()),
                    updated_at: checked
                        .cache
                        .as_ref()
                        .and_then(|row| row.completed_at)
                        .map(Into::into),
                });
            }
            let facts = vec![
                native::Facts::collection(
                    Role::Target,
                    groups(
                        targets
                            .iter()
                            .map(|identity| identity.fact().clone())
                            .collect(),
                    ),
                ),
                native::Facts::collection(Role::Inputs, groups(inputs.clone())),
                native::Facts::collection(Role::Provider, groups(providers.clone())),
            ];
            let next_offset = if end < distinct.len() {
                Some(u32::try_from(end).map_err(ApiError::internal)?)
            } else {
                None
            };
            Ok((
                View {
                    targets,
                    inputs,
                    providers,
                    reply: SummaryBatchReply {
                        items,
                        unavailable,
                        next_offset,
                    },
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        // Each returned row is selected through the native disclosure producer before its own
        // collection is bound. A fully filtered empty list still belongs to this actual reader.
        Box::pin(async move {
            ctx.user()?;
            Ok(())
        })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        value: &'a View,
    ) -> BoxFuture<'a, ApiResult<SummaryBatchReply>> {
        Box::pin(async move { Ok(value.reply.clone()) })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Self::Query,
        old: &'a View,
        fresh: &'a View,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.reply == fresh.reply
                && old.inputs == fresh.inputs
                && old.providers == fresh.providers
                && old
                    .targets
                    .iter()
                    .map(|identity| identity.fact())
                    .eq(fresh.targets.iter().map(|identity| identity.fact())))
        })
    }
    fn after_read<'a>(
        ctx: &'a Ctx,
        _: &'a Caller,
        _: &'a Self::Query,
        value: &'a View,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let (mut conn, guard) = transaction_guard::begin(ctx).await?;
            let mut inputs: Vec<_> = value
                .targets
                .iter()
                .map(|identity| identity.fact().clone())
                .collect();
            inputs.extend(value.inputs.iter().cloned());
            note_chat_openings(&mut conn, guard.principal(), &inputs).await?;
            conn.commit().await.map_err(DbError::from)?;
            Ok(())
        })
    }
}
