//! Native stream frames: initial interest, then each actual selected part batch.
use super::{content, meaning, native as search, rows, stream, words};
use crate::key_operations::{native, publication};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::access::page::NameBounds;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_api_contract::{
    operation_sources::Declaration,
    search::{SearchHit, SearchListQuery},
    search_sources::{STREAM, Source},
};
use orch_authz::{Caller, CapturedOperation, Principal};
use orch_db::Conn;
use std::collections::BTreeSet;

#[derive(Clone)]
pub(super) enum Query {
    Start(SearchListQuery),
    Words {
        query: SearchListQuery,
        position: rows::candidates::Position,
        shown: BTreeSet<String>,
        origin: Option<CapturedOperation>,
    },
    History {
        query: SearchListQuery,
        origin: Option<CapturedOperation>,
    },
    MeaningScope {
        query: SearchListQuery,
        orgs: Vec<orch_types::Id>,
        origin: Option<CapturedOperation>,
    },
    MeaningHits {
        query: SearchListQuery,
        near: Vec<orch_engine::semantic::query::Near>,
        shown: BTreeSet<String>,
        origin: Option<CapturedOperation>,
    },
}

pub(super) enum Output {
    Start(Box<stream::PreparedStream>),
    Words {
        found: Vec<rows::HitRow>,
        hits: Vec<SearchHit>,
        next: Option<rows::candidates::Position>,
        request_windows: i64,
    },
    History(orch_api_contract::common::Page<SearchHit>),
    MeaningScope(meaning::PreparedScope),
    MeaningHits(Vec<SearchHit>),
}
pub(super) struct Prepared {
    origin: Option<CapturedOperation>,
    output: Output,
    frozen: Option<Frozen>,
}

#[derive(Clone, Copy)]
enum Match {
    Words,
    Meaning,
}
struct Frozen {
    query: SearchListQuery,
    source: Match,
    found: Vec<rows::HitRow>,
    governors: Vec<(KeyTargetKind, KeyResolvedTarget)>,
    names: NameBounds,
    history: Option<orch_api_contract::threads::ThreadContinuationProjection>,
}

pub(super) struct Reader;

/// The stream header contains only request/continuation metadata. It keeps namespace
/// eligibility; each actual batch separately retains and rechecks its concrete source.
pub(super) fn check_opening(ctx: &Ctx, caller: &Caller) -> ApiResult<()> {
    let plan = crate::registry::OperationPlan::new(ctx.route, STREAM.operation)?;
    plan.shared()
        .compatible(caller)
        .map_err(crate::key_operations::admission::binding_error)?;
    let roles = plan
        .variant()
        .facets
        .iter()
        .filter(|facet| facet.usage == orch_api_contract::operations::FacetUsage::Required)
        .map(|facet| orch_access_contract::key_scope::KeyEmptyFacet {
            action: facet.action,
            resources: facet.resources.clone(),
        })
        .collect::<Vec<_>>();
    if !caller.permits_empty_operation(STREAM.operation, &roles) {
        return Err(refused());
    }
    Ok(())
}
impl native::Selected for Reader {
    type Source = Source;
    type Query = Query;
    type Value = Prepared;
    type Reply = ();
    const DECLARATION: &'static Declaration<Source> = &STREAM;
    fn origin<'a>(_query: &Query, value: &'a Prepared) -> Option<&'a CapturedOperation> {
        value.origin.as_ref()
    }
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Query,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(prepare(ctx, conn, caller, query))
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
        _value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async {
            Err(ApiError::internal(
                "a staged search source has no replayed renderer",
            ))
        })
    }
}

/// Each source gets its own isolated operation operands; the original HTTP cursor is not replayed.
pub(super) async fn read(ctx: Ctx, query: Query) -> ApiResult<(Ctx, Caller, Output)> {
    native::staged::read::<Reader, _, _, _>(ctx, query, |frame| async move {
        let (ctx, accepted, mut value) = frame.into_parts();
        if let Output::MeaningScope(scope) = &mut value.output {
            scope.origins = std::mem::take(&mut scope.origins)
                .into_iter()
                .map(|(org, origin)| {
                    origin
                        .narrowed_with(&accepted)
                        .map(|origin| (org, origin))
                        .map_err(ApiError::internal)
                })
                .collect::<ApiResult<_>>()?;
        }
        if let Some(frozen) = value.frozen {
            publication::retain(&ctx, &accepted, frozen, |ctx, conn, reader, frozen| {
                Box::pin(check_parts(ctx, conn, reader, frozen))
            })?;
        }
        Ok((ctx, accepted, value.output))
    })
    .await
}

async fn current_caller(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    origin: Option<&CapturedOperation>,
) -> ApiResult<Caller> {
    let plan = native::bind_known(ctx, &STREAM, &[])?;
    let plan = match origin {
        Some(origin) => {
            // A stored cursor's missing original credential is an authentication failure.
            // The shared origin handoff still owns all operation/account/scope admission.
            origin
                .restore_current(conn, ctx.app.clock.as_ref())
                .await
                .map_err(crate::middleware::authorization::restoration_error)?
                .ok_or_else(|| {
                    ApiError::new(
                        orch_api_contract::common::ErrorCode::AuthInvalidToken,
                        "the search cursor's original key has ended",
                    )
                })?;
            plan.with_origin(ctx, conn, caller, origin).await?
        }
        None => plan,
    };
    Ok(native::preparation_principal(ctx, conn, caller, &plan)
        .await?
        .caller()
        .clone())
}

async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    intent: &Query,
) -> ApiResult<(Prepared, Vec<native::Facts<Source>>)> {
    let mut cancellation = rows::cancel_on_drop(&ctx.app.db, conn).await?;
    if let Query::Start(query) = intent {
        current_caller(ctx, conn, caller, None).await?;
        let value = stream::prepare(ctx, conn, stream::validate(query.clone())?).await?;
        let origin = value.origin().cloned();
        let current = current_caller(ctx, conn, caller, origin.as_ref()).await?;
        let (_, _, _, interest) =
            search::content_frame(ctx, conn, &current, &STREAM, value.selection()).await?;
        cancellation.disarm();
        return Ok((
            Prepared {
                origin,
                output: Output::Start(Box::new(value)),
                frozen: None,
            },
            vec![
                search::collection(Source::Results, interest),
                search::collection(Source::Names, Vec::new()),
            ],
        ));
    }
    let (query, origin) = match intent {
        Query::Start(_) => unreachable!(),
        Query::Words { query, origin, .. }
        | Query::History { query, origin }
        | Query::MeaningScope { query, origin, .. }
        | Query::MeaningHits { query, origin, .. } => (query, origin),
    };
    let current = current_caller(ctx, conn, caller, origin.as_ref()).await?;
    let (principal, scopes, exhausted, _interest) =
        search::content_frame(ctx, conn, &current, &STREAM, query).await?;
    let boundary = search::ContentBoundary {
        caller: &current,
        declaration: &STREAM,
        scopes,
    };
    if let Query::MeaningScope { orgs, .. } = intent {
        let mut eligible = Vec::new();
        let mut interests = Vec::new();
        let mut origins = std::collections::BTreeMap::new();
        for org in orgs {
            let mut scope = query.clone();
            if let Some(place) = query.scope {
                if orch_db::queries::semantic::org_of(conn, place).await? != Some(*org) {
                    continue;
                }
            } else {
                scope.scope = Some(*org);
            }
            let (_, _, empty, facts) =
                search::content_frame(ctx, conn, &current, &STREAM, &scope).await?;
            let belongs = facts.iter().any(|(_, target)| {
                target
                    .node_ancestors
                    .iter()
                    .chain(std::iter::once(&target.object))
                    .any(|object| {
                        object.kind
                            == KeyTargetKind::Object(
                                orch_access_contract::permissions::ObjectKind::Org,
                            )
                            && object.id == Some(*org)
                    })
            });
            if !empty && belongs {
                eligible.push(*org);
                if let Some(origin) = origin {
                    origins.insert(
                        *org,
                        orch_engine::semantic::query_origin::QueryOrigin::new(
                            origin.clone(),
                            *org,
                            facts.iter().map(|(_, target)| target.clone()).collect(),
                        )
                        .map_err(ApiError::internal)?,
                    );
                }
                interests.extend(facts);
            }
        }
        let mut value = meaning::prepare_scope(
            ctx,
            conn,
            &principal,
            query,
            eligible,
            Some(&boundary.scopes),
        )
        .await?;
        let selected_orgs: BTreeSet<_> = value.orgs.iter().copied().collect();
        origins.retain(|org, _| selected_orgs.contains(org));
        value.origins = origins;
        cancellation.disarm();
        return Ok((
            Prepared {
                origin: origin.clone(),
                output: Output::MeaningScope(value),
                frozen: None,
            },
            vec![
                search::collection(Source::Results, interests),
                search::collection(Source::Names, Vec::new()),
            ],
        ));
    }
    let (mut found, source, next, request_windows, history_next, history) = match intent {
        Query::Words { position, .. } => {
            let value = if exhausted {
                words::PreparedWindow {
                    found: Vec::new(),
                    next: None,
                    request_windows: 1,
                }
            } else {
                words::prepare_window(
                    ctx,
                    conn,
                    &principal,
                    query,
                    position.clone(),
                    Some(&boundary),
                )
                .await?
            };
            (
                value.found,
                Match::Words,
                value.next,
                value.request_windows,
                None,
                None,
            )
        }
        Query::History { .. } => {
            let checked = content::validate(query)?;
            let value = if exhausted {
                content::PreparedContent::empty(query.clone())
            } else {
                content::prepare(ctx, conn, &principal, query, checked, Some(&boundary)).await?
            };
            (
                value.hits,
                Match::Words,
                None,
                0,
                value.next_cursor,
                value.history,
            )
        }
        Query::MeaningHits { near, .. } => {
            let found = meaning::prepare_hits(ctx, conn, &principal, query, near).await?;
            (
                boundary.filter(ctx, conn, found).await?,
                Match::Meaning,
                None,
                0,
                None,
                None,
            )
        }
        _ => unreachable!(),
    };
    if let Query::Words { shown, .. } | Query::MeaningHits { shown, .. } = intent {
        found.retain(|row| !shown.contains(&part_identity(row)));
    }
    let mut governors = Vec::new();
    for row in &found {
        governors.push(search::content_target(conn, row).await?);
    }
    let (bounds, names) =
        search::content_names(ctx, conn, &current, &principal, &STREAM, &found, query).await?;
    let hits = content::hydrate_with_names(conn, &principal, &found, query, ctx.now, Some(&bounds))
        .await?;
    let output = match intent {
        Query::Words { .. } => Output::Words {
            found: found.clone(),
            hits,
            next,
            request_windows,
        },
        Query::History { .. } => Output::History(orch_api_contract::common::Page {
            items: hits,
            next_cursor: history_next,
        }),
        Query::MeaningHits { .. } => Output::MeaningHits(hits),
        _ => unreachable!(),
    };
    let facts = vec![
        search::collection(Source::Results, governors.clone()),
        names,
    ];
    cancellation.disarm();
    Ok((
        Prepared {
            origin: origin.clone(),
            output,
            frozen: Some(Frozen {
                query: query.clone(),
                source,
                found,
                governors,
                names: bounds,
                history,
            }),
        },
        facts,
    ))
}

async fn check_parts(
    ctx: &Ctx,
    conn: &mut Conn,
    reader: &Principal,
    frozen: &Frozen,
) -> ApiResult<()> {
    // Restore this source's required operation before using the ordinary projector.
    // The frozen native governor set cannot be lent by another matching window.
    let mut governors = Vec::new();
    for row in &frozen.found {
        governors.push(search::content_target(conn, row).await?);
    }
    if governors != frozen.governors {
        return Err(refused());
    }
    let known = native::bind_known(
        ctx,
        &STREAM,
        &[search::collection(Source::Results, governors.clone())],
    )?;
    let principal = native::preparation_principal(ctx, conn, reader.caller(), &known).await?;
    if let Some(opened) = &frozen.history {
        let thread = frozen.query.scope.ok_or_else(refused)?;
        let fresh = orch_db::queries::thread_continuations::projection(
            conn,
            thread,
            frozen.query.continuation_id,
        )
        .await?
        .ok_or_else(refused)?;
        let fresh = orch_engine::continuations::projection::scope(&fresh);
        if opened.viewed_continuation_id != fresh.viewed_continuation_id
            || opened.projection_revision != fresh.projection_revision
            || opened.follows_active != fresh.follows_active
        {
            return Err(refused());
        }
    }
    let selected = rows::CandidateSet::from_content(&frozen.found);
    let found = match frozen.source {
        Match::Words => {
            words::prepare_selected(ctx, conn, &principal, &frozen.query, &selected).await?
        }
        Match::Meaning => {
            meaning::prepare_selected(ctx, conn, &principal, &frozen.query, &selected).await?
        }
    };
    let identities: BTreeSet<_> = found
        .iter()
        .map(|row| (&row.kind, row.object_id, row.id, row.block_id))
        .collect();
    if !frozen
        .found
        .iter()
        .all(|row| identities.contains(&(&row.kind, row.object_id, row.id, row.block_id)))
    {
        return Err(refused());
    }
    let (bounds, names) = search::content_names(
        ctx,
        conn,
        principal.caller(),
        &principal,
        &STREAM,
        &frozen.found,
        &frozen.query,
    )
    .await?;
    if !frozen.names.nodes.is_subset(&bounds.nodes)
        || !frozen.names.containers.is_subset(&bounds.containers)
    {
        return Err(refused());
    }
    let plan = native::bind(
        ctx,
        &STREAM,
        &[search::collection(Source::Results, governors), names],
    )?;
    plan.read_operation(ctx, conn, principal.caller(), |_conn, _principal| {
        Box::pin(async { Ok(()) })
    })
    .await?
    .ok_or_else(refused)
}

fn refused() -> ApiError {
    ApiError::new(
        orch_api_contract::common::ErrorCode::AuthzDenied,
        "the original search source is no longer readable",
    )
}

/// Same stable identity as the native stream renderer, before payload hydration.
pub(super) fn part_identity(row: &rows::HitRow) -> String {
    match row.kind.as_str() {
        "name" => format!("name:{}", row.object_id),
        "chat_message" => format!("chat:{}", row.id),
        "document" => format!(
            "doc:{}:{}",
            row.object_id,
            row.block_id
                .map_or_else(|| "title".to_owned(), |id| id.to_string())
        ),
        _ => format!("item:{}", row.id),
    }
}

/// Typed registration row for the original query; the owned start uses the staged kernel.
pub(super) struct Opening;
impl native::Selected for Opening {
    type Source = Source;
    type Query = SearchListQuery;
    type Value = Prepared;
    type Reply = ();
    const DECLARATION: &'static Declaration<Source> = &STREAM;
    fn origin<'a>(_query: &SearchListQuery, value: &'a Prepared) -> Option<&'a CapturedOperation> {
        value.origin.as_ref()
    }
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a SearchListQuery,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move { prepare(ctx, conn, caller, &Query::Start(query.clone())).await })
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
        _value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async {
            Err(ApiError::internal(
                "search starts only outside its source snapshot",
            ))
        })
    }
}
