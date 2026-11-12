//! Bounded native first pages keep one aggregate operation and all producing source witnesses.

use super::invalid;

use super::list_read;

use crate::key_operations::native;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::OperationPlan;

use crate::registry::OptionalOperationFuture;

use crate::routes::document_tree::metadata;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::Problem;

use orch_api_contract::document_sources as sources;

use orch_api_contract::documents::NodeDocumentListQuery;

use orch_api_contract::documents::NodeDocumentPage;

use orch_api_contract::documents::NodeDocumentPagesGetQuery;

use orch_authz::Caller;

use orch_authz::CapturedCaller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::DbError;

use orch_types::Id;

use std::collections::BTreeSet;

use std::sync::Arc;
enum Part {
    Page {
        node: Id,
        ready: Arc<list_read::Ready>,
    },
    Error {
        node: Id,
        problem: Box<Problem>,
    },
}

struct Failure {
    cut: metadata::Cut,
    caller: CapturedCaller,
}

struct Ready {
    response: OperationPlan,
    parts: Vec<Part>,
    failures: Vec<Failure>,
}

fn refused() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the aggregate document page source is no longer readable",
    )
}

fn nodes(ctx: &Ctx, query: &NodeDocumentPagesGetQuery) -> ApiResult<Vec<Id>> {
    let ids: Vec<Id> = query
        .node_ids
        .split(',')
        .map(|id| {
            id.parse()
                .map_err(|_| invalid("node_ids", "must contain node identifiers"))
        })
        .collect::<ApiResult<_>>()?;
    if ids.is_empty()
        || ids.len() > orch_api_contract::common::MAX_LIMIT as usize
        || ids.first() != Some(&ctx.id("id")?)
    {
        return Err(invalid(
            "node_ids",
            "must begin with the path node and fit one page",
        ));
    }
    Ok(ids)
}

async fn select(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: NodeDocumentPagesGetQuery,
) -> ApiResult<Ready> {
    let ids = nodes(ctx, &query)?;
    let mut parts = Vec::with_capacity(ids.len());
    let mut failures = Vec::new();
    let mut documents = BTreeSet::new();
    let mut labels = BTreeSet::new();
    for node in ids {
        // for_id changes only the native requested path, never its operation/route.
        let child = ctx.for_id(node);
        let page = list_read::select_for(
            &child,
            conn,
            caller,
            NodeDocumentListQuery {
                limit: query.limit,
                ..Default::default()
            },
            &sources::DOCUMENT_PAGES,
        )
        .await;
        match page {
            Ok(ready) => {
                documents.extend(ready.visible.iter().chain(&ready.private).copied());
                labels.extend(ready.sources.iter().copied());
                parts.push(Part::Page {
                    node,
                    ready: Arc::new(ready),
                });
            }
            Err(error)
                if error.is_database_retry()
                    || error.problem.status >= 500
                    || error.problem.status == 401 =>
            {
                return Err(error);
            }
            Err(error) => {
                let error = if native::modern(caller) {
                    // A failed member has no successful document source or named-refusal projection.
                    error.without_content()
                } else {
                    let empty = native::bind_known(ctx, &sources::DOCUMENT_PAGES, &[])?;
                    let principal =
                        native::preparation_principal(ctx, conn, caller, &empty).await?;
                    match metadata::cut(conn, &principal, node, principal.now()).await {
                        Ok(cut) => {
                            failures.push(Failure {
                                cut,
                                caller: CapturedCaller::capture(principal.caller()),
                            });
                            error
                        }
                        Err(missing) if missing.code() == ErrorCode::NotFound => {
                            error.without_content()
                        }
                        Err(other) => return Err(other),
                    }
                };
                parts.push(Part::Error {
                    node,
                    problem: error.problem,
                });
            }
        }
    }
    let facts = list_read::source_facts(
        conn,
        &documents.into_iter().collect::<Vec<_>>(),
        &labels.into_iter().collect::<Vec<_>>(),
    )
    .await?;
    let response = native::bind(ctx, &sources::DOCUMENT_PAGES, &facts)?;
    Ok(Ready {
        response,
        parts,
        failures,
    })
}

pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: NodeDocumentPagesGetQuery,
    (): (),
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !native::modern(caller) {
            return Ok(None);
        }
        let ready = select(ctx, conn, caller, query).await?;
        Ok(Some(ready.response.clone().with_data(ready)))
    })
}

fn pages(ready: &Ready) -> Vec<NodeDocumentPage> {
    ready
        .parts
        .iter()
        .map(|part| match part {
            Part::Page { node, ready } => NodeDocumentPage {
                node_id: *node,
                page: Some(ready.page.clone()),
                error: None,
            },
            Part::Error { node, problem } => NodeDocumentPage {
                node_id: *node,
                page: None,
                error: Some((**problem).clone()),
            },
        })
        .collect()
}

async fn check_sources(
    ctx: &Ctx,
    conn: &mut Conn,
    reader: &Principal,
    ready: &Ready,
) -> ApiResult<()> {
    let mut documents = BTreeSet::new();
    let mut labels = BTreeSet::new();
    for part in &ready.parts {
        if let Part::Page { node, ready } = part {
            documents.extend(ready.visible.iter().chain(&ready.private).copied());
            labels.extend(ready.sources.iter().copied());
            list_read::check_sources(&ctx.for_id(*node), conn, reader, ready).await?;
        }
    }
    let facts = list_read::source_facts(
        conn,
        &documents.into_iter().collect::<Vec<_>>(),
        &labels.into_iter().collect::<Vec<_>>(),
    )
    .await?;
    let fresh = native::bind(ctx, &sources::DOCUMENT_PAGES, &facts)?;
    if !fresh.shared().covers_sources(ready.response.shared()) {
        return Err(refused());
    }
    if native::modern(reader.caller()) {
        fresh.admit(reader.caller())?;
    }
    for failure in &ready.failures {
        // Rich ordinary/legacy failures retain their real producing authority/placement cut.
        if CapturedCaller::capture(reader.caller()) != failure.caller {
            return Err(refused());
        }
        match metadata::unchanged(conn, reader, &failure.cut, reader.now()).await {
            Ok(()) => {}
            Err(error) if error.code() == ErrorCode::ConflictStaleRevision => return Err(refused()),
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(super) async fn get(
    ctx: Ctx,
    query: NodeDocumentPagesGetQuery,
    (): (),
) -> ApiResult<Vec<NodeDocumentPage>> {
    let (value, ready, actual) = native::read(ctx, move |ctx, conn, principal| {
        let query = query.clone();
        Box::pin(async move {
            let ready = if native::modern(principal.caller()) {
                ctx.operation_data::<Ready>()?
            } else {
                Arc::new(select(ctx, conn, principal.caller(), query).await?)
            };
            let value = if native::modern(principal.caller()) {
                let projection = ctx.clone();
                let producing = Arc::clone(&ready);
                ready
                    .response
                    .clone()
                    .read_operation(ctx, conn, principal.caller(), move |conn, reader| {
                        Box::pin(async move {
                            check_sources(&projection, conn, reader, &producing).await?;
                            Ok(pages(&producing))
                        })
                    })
                    .await?
                    .ok_or_else(refused)?
            } else {
                check_sources(ctx, conn, principal, &ready).await?;
                pages(&ready)
            };
            Ok((value, ready, ctx.clone()))
        })
    })
    .await?;
    let (mut tx, guard) = transaction_guard::begin(&actual).await?;
    check_sources(&actual, &mut tx, guard.principal(), &ready).await?;
    guard.check_deadline(&actual)?;
    tx.commit().await.map_err(DbError::from)?;
    guard.check_deadline(&actual)?;
    let accepted = match actual.admitted_caller() {
        Some(caller) => caller.clone(),
        None => authorization::caller_of(actual.user()?)?,
    };
    crate::key_operations::publication::retain(
        &actual,
        &accepted,
        ready,
        |ctx, conn, reader, ready| Box::pin(check_sources(ctx, conn, reader, ready)),
    )?;
    Ok(value)
}
