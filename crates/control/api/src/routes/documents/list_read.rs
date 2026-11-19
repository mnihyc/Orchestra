//! Native document windows, including their separately paged private administrative rows.

use super::{
    LIST_ROUNDS_MAX, SCAN_CURSOR, WITH_PRIVATE_CURSOR, build, invalid, listing, placed,
    read_cursor, read_operation, write_cursor,
};
use crate::key_operations::native;
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, OperationPlan, OptionalOperationFuture};
use build::{ProjectedDocumentRef, WrittenLabelSource};
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::common::{ErrorCode, Listed as ListedItem, Page, PageQuery};
use orch_api_contract::document_sources::{self as sources, ListSource};
use orch_api_contract::documents::{DocumentRef, NodeDocumentListQuery};
use orch_api_contract::operation_sources::Declaration;
use orch_authz::{Caller, Policy, Principal, documents};
use orch_db::queries::documents::{self as rows, DocumentRow, ListCursor, ListWhere};
use orch_db::{Conn, DbError};
use orch_engine::operation_scope::targets;
use orch_types::Id;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

type Reply = Page<ListedItem<DocumentRef>>;

pub(super) struct Ready {
    pub(super) response: OperationPlan,
    pub(super) page: Reply,
    pub(super) visible: Vec<Id>,
    pub(super) private: Vec<Id>,
    pub(super) sources: Vec<WrittenLabelSource>,
    query: NodeDocumentListQuery,
    declaration: &'static Declaration<ListSource>,
}

fn declaration(query: &NodeDocumentListQuery) -> &'static Declaration<ListSource> {
    if query.deleted.is_some() {
        &sources::DOCUMENT_LIST.deleted
    } else {
        &sources::DOCUMENT_LIST.read
    }
}

fn refused() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the document list source is no longer admitted",
    )
}

/// The first real permitted candidate admits only effect-free ordinary preparation.
/// An exhausted selector instead supplies the complete checked-empty collection proof.
pub(super) struct Reader<'a> {
    ctx: &'a Ctx,
    caller: &'a Caller,
    declaration: &'static Declaration<ListSource>,
    selection: OperationPlan,
    principal: Option<Principal>,
}

impl<'a> Reader<'a> {
    async fn new(
        ctx: &'a Ctx,
        conn: &mut Conn,
        caller: &'a Caller,
        declaration: &'static Declaration<ListSource>,
    ) -> ApiResult<Self> {
        let selection = native::bind_known(ctx, declaration, &[])?;
        let principal = if native::modern(caller) {
            None
        } else {
            let principal = native::preparation_principal(ctx, conn, caller, &selection).await?;
            authorization::require_on_path_in(ctx, conn, &principal, "node.view").await?;
            Some(principal)
        };
        Ok(Self {
            ctx,
            caller,
            declaration,
            selection,
            principal,
        })
    }

    pub(super) async fn accept(&mut self, conn: &mut Conn, ids: &[Id]) -> ApiResult<HashSet<Id>> {
        if !native::modern(self.caller) {
            return Ok(ids.iter().copied().collect());
        }
        let found = read_operation::identities(conn, ids).await?;
        let mut allowed = HashSet::new();
        for id in ids {
            let target = found.get(id).ok_or_else(ApiError::not_found)?;
            if target.object.kind != KeyTargetKind::Object(ObjectKind::Document) {
                return Err(ApiError::not_found());
            }
            if !native::permits(
                &self.selection,
                self.declaration,
                self.caller,
                &ListSource::Documents,
                KeyTargetKind::Object(ObjectKind::Document),
                target,
            )? {
                continue;
            }
            if self.principal.is_none() {
                let known = native::bind_known(
                    self.ctx,
                    self.declaration,
                    &[native::Facts::collection(
                        ListSource::Documents,
                        vec![(
                            KeyTargetKind::Object(ObjectKind::Document),
                            vec![target.clone()],
                        )],
                    )],
                )?;
                let principal =
                    native::preparation_principal(self.ctx, conn, self.caller, &known).await?;
                authorization::require_on_path_in(self.ctx, conn, &principal, "node.view").await?;
                self.principal = Some(principal);
            }
            allowed.insert(*id);
        }
        Ok(allowed)
    }

    pub(super) fn principal(&self) -> ApiResult<&Principal> {
        self.principal.as_ref().ok_or_else(|| {
            ApiError::internal("the native list has no selected preparation principal")
        })
    }

    async fn complete_empty(&mut self, conn: &mut Conn) -> ApiResult<()> {
        if self.principal.is_none() {
            let complete = bind_sources(self.ctx, conn, self.declaration, &[], &[]).await?;
            let principal =
                native::preparation_principal(self.ctx, conn, self.caller, &complete).await?;
            authorization::require_on_path_in(self.ctx, conn, &principal, "node.view").await?;
            self.principal = Some(principal);
        }
        Ok(())
    }
}

fn node_source(kind: KeyTargetKind) -> ApiResult<ListSource> {
    match kind {
        KeyTargetKind::Object(ObjectKind::Org) => Ok(ListSource::Orgs),
        KeyTargetKind::Object(ObjectKind::Project) => Ok(ListSource::Projects),
        KeyTargetKind::Object(ObjectKind::Topic) => Ok(ListSource::Topics),
        KeyTargetKind::Object(ObjectKind::Thread) => Ok(ListSource::Threads),
        _ => Err(ApiError::internal(
            "the document list has an undeclared name source",
        )),
    }
}

async fn name_targets(
    conn: &mut Conn,
    sources: &[WrittenLabelSource],
    require_all: bool,
) -> ApiResult<Vec<(ListSource, KeyResolvedTarget)>> {
    let ids: BTreeSet<_> = sources
        .iter()
        .filter_map(|source| match source {
            WrittenLabelSource::Node(id) => Some(*id),
            _ => None,
        })
        .collect();
    if ids.len() != sources.len() {
        return Err(ApiError::internal(
            "the document list has an unexpected reference source",
        ));
    }
    let graph = orch_authz::engine::Graph::load(conn, &ids.into_iter().collect::<Vec<_>>())
        .await
        .map_err(ApiError::internal)?;
    let mut found = Vec::new();
    for source in sources {
        let WrittenLabelSource::Node(id) = source else {
            return Err(refused());
        };
        let Some(target) = targets::from_node_identity(&graph, *id) else {
            if require_all {
                return Err(refused());
            }
            continue;
        };
        found.push((node_source(target.object.kind)?, target));
    }
    Ok(found)
}

pub(super) async fn source_facts(
    conn: &mut Conn,
    ids: &[Id],
    sources: &[WrittenLabelSource],
) -> ApiResult<Vec<native::Facts<ListSource>>> {
    let mut found = read_operation::identities(conn, ids).await?;
    let documents = ids
        .iter()
        .map(|id| {
            found
                .remove(id)
                .filter(|target| target.object.kind == KeyTargetKind::Object(ObjectKind::Document))
                .ok_or_else(ApiError::not_found)
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let mut groups: HashMap<KeyTargetKind, Vec<KeyResolvedTarget>> = HashMap::new();
    for (_, target) in name_targets(conn, sources, true).await? {
        groups.entry(target.object.kind).or_default().push(target);
    }
    let mut facts = vec![native::Facts::collection(
        ListSource::Documents,
        if documents.is_empty() {
            Vec::new()
        } else {
            vec![(KeyTargetKind::Object(ObjectKind::Document), documents)]
        },
    )];
    for kind in [
        ObjectKind::Org,
        ObjectKind::Project,
        ObjectKind::Topic,
        ObjectKind::Thread,
    ] {
        let family = KeyTargetKind::Object(kind);
        facts.push(native::Facts::collection(
            node_source(family)?,
            groups
                .remove(&family)
                .map(|targets| vec![(family, targets)])
                .unwrap_or_default(),
        ));
    }
    Ok(facts)
}

async fn bind_sources(
    ctx: &Ctx,
    conn: &mut Conn,
    declaration: &'static Declaration<ListSource>,
    ids: &[Id],
    sources: &[WrittenLabelSource],
) -> ApiResult<OperationPlan> {
    native::bind(ctx, declaration, &source_facts(conn, ids, sources).await?)
}

async fn select(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: NodeDocumentListQuery,
) -> ApiResult<Ready> {
    let declaration = declaration(&query);
    select_for(ctx, conn, caller, query, declaration).await
}

pub(super) async fn select_for(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: NodeDocumentListQuery,
    declaration: &'static Declaration<ListSource>,
) -> ApiResult<Ready> {
    let node = ctx.id("id")?;
    let paging = PageQuery {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let limit = i64::from(paging.limit());
    let private_offset = listing::offset(query.cursor.as_deref())?;
    if query.participating.unwrap_or(false) && private_offset.is_some() {
        return Err(invalid(
            "cursor",
            "is not a cursor of the ordinary participation list",
        ));
    }
    let mut have_private = query
        .cursor
        .as_deref()
        .is_some_and(|cursor| cursor.starts_with(WITH_PRIVATE_CURSOR));
    let raw_readable_cursor = query
        .cursor
        .as_deref()
        .map(|cursor| cursor.strip_prefix(WITH_PRIVATE_CURSOR).unwrap_or(cursor));
    let (readable_cursor, mut first_offset) = if let Some(scan) =
        raw_readable_cursor.and_then(|cursor| cursor.strip_prefix(SCAN_CURSOR))
    {
        let (count, base) = scan
            .split_once('|')
            .ok_or_else(|| invalid("cursor", "is not a cursor of this list"))?;
        let offset = count
            .parse::<i64>()
            .ok()
            .filter(|value| *value >= 0)
            .ok_or_else(|| invalid("cursor", "is not a cursor of this list"))?;
        (if base.is_empty() { None } else { Some(base) }, offset)
    } else {
        (raw_readable_cursor, 0)
    };
    let mut scanned = first_offset;
    let mut after = if private_offset.is_none() {
        read_cursor("cursor", readable_cursor)?.map(|(at, id)| ListCursor { at, id })
    } else {
        None
    };
    let want_deleted = query.deleted.is_some();
    placed(conn, node).await?;
    let mut reader = Reader::new(ctx, conn, caller, declaration).await?;
    let policy = Policy::default();
    let mut chosen = Vec::new();
    let mut more = false;
    let mut exhausted = false;
    if private_offset.is_some() && want_deleted {
        return Err(invalid("cursor", "is not a cursor of this list"));
    }
    if private_offset.is_none() {
        for _ in 0..LIST_ROUNDS_MAX {
            let found = rows::list_beside(
                conn,
                &ListWhere {
                    node,
                    under: query.under.unwrap_or(false),
                    deleted: want_deleted,
                    after,
                    offset: std::mem::take(&mut first_offset),
                    limit: limit + 1,
                },
            )
            .await?;
            exhausted = i64::try_from(found.len()).unwrap_or(i64::MAX) <= limit;
            let ids: Vec<_> = found.iter().map(|row| row.id).collect();
            let allowed = reader.accept(conn, &ids).await?;
            let ids: Vec<_> = ids.into_iter().filter(|id| allowed.contains(id)).collect();
            let standing = if ids.is_empty() {
                HashMap::new()
            } else {
                documents::listed(conn, reader.principal()?, &ids, policy)
                    .await
                    .map_err(ApiError::internal)?
                    .into_iter()
                    .map(|each| (each.document.id, each))
                    .collect::<HashMap<_, _>>()
            };
            have_private |= !want_deleted
                && standing.values().any(|held| {
                    !held.holds(Permission::DocView) && held.holds(Permission::AccessManage)
                });
            for row in found {
                scanned = scanned
                    .checked_add(1)
                    .ok_or_else(|| invalid("cursor", "is not a cursor of this list"))?;
                after = Some(ListCursor {
                    at: row.updated_at,
                    id: row.id,
                });
                let needs = if want_deleted {
                    Permission::DocManage
                } else {
                    Permission::DocView
                };
                let Some(held) = standing.get(&row.id).filter(|each| {
                    each.holds(needs)
                        && (!query.participating.unwrap_or(false) || each.participating)
                }) else {
                    continue;
                };
                if i64::try_from(chosen.len()).unwrap_or(i64::MAX) == limit {
                    more = true;
                    break;
                }
                chosen.push((row, held.clone()));
            }
            if more || exhausted {
                break;
            }
        }
    }
    let next_cursor = if more {
        chosen
            .last()
            .map(|(row, _)| write_cursor(row.updated_at, row.id))
    } else if exhausted {
        None
    } else if let Some((row, _)) = chosen.last() {
        Some(write_cursor(row.updated_at, row.id))
    } else {
        Some(format!(
            "{SCAN_CURSOR}{scanned}|{}",
            readable_cursor.unwrap_or_default()
        ))
    };
    let next_cursor = next_cursor.map(|cursor| {
        if have_private {
            format!("{WITH_PRIVATE_CURSOR}{cursor}")
        } else {
            cursor
        }
    });
    let readable_end = chosen.last().map(|(row, _)| {
        format!(
            "{WITH_PRIVATE_CURSOR}{}",
            write_cursor(row.updated_at, row.id)
        )
    });
    let private = if let Some(offset) = private_offset {
        Some(
            listing::select(
                conn,
                &mut reader,
                listing::PrivateList {
                    node,
                    under: query.under.unwrap_or(false),
                    limit,
                    offset,
                },
                0,
            )
            .await?,
        )
    } else if !want_deleted
        && have_private
        && next_cursor.is_none()
        && !query.participating.unwrap_or(false)
    {
        Some(
            listing::select(
                conn,
                &mut reader,
                listing::PrivateList {
                    node,
                    under: query.under.unwrap_or(false),
                    limit,
                    offset: 0,
                },
                chosen.len(),
            )
            .await?,
        )
    } else {
        None
    };
    reader.complete_empty(conn).await?;
    let principal = reader.principal()?;
    let candidates: Vec<_> = ProjectedDocumentRef::candidates_many(conn, &chosen)
        .await?
        .into_values()
        .flatten()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut allowed = Vec::new();
    if native::modern(caller) {
        for (source, target) in name_targets(conn, &candidates, false).await? {
            let usage = declaration
                .bindings
                .iter()
                .find(|binding| binding.source == source)
                .map(|binding| binding.usage)
                .ok_or_else(|| ApiError::internal("a native document label role is undeclared"))?;
            if native::permits_usage(
                &reader.selection,
                declaration,
                caller,
                &source,
                usage,
                target.object.kind,
                &target,
            )? {
                let id = target.object.id.ok_or_else(ApiError::not_found)?;
                allowed.push(WrittenLabelSource::Node(id));
            }
        }
    }
    let (visible, sources) = if native::modern(caller) {
        ProjectedDocumentRef::capture_many_allowed(
            conn,
            principal,
            &chosen,
            principal.now().get(),
            policy,
            &allowed,
        )
        .await?
    } else {
        ProjectedDocumentRef::capture_many(conn, principal, &chosen, principal.now().get(), policy)
            .await?
    };
    let mut page = Page {
        items: visible.into_iter().map(ListedItem::visible).collect(),
        next_cursor,
    };
    let mut private_ids = Vec::new();
    if let Some(private) = private {
        private_ids.extend(private.rows.iter().map(|row| row.id));
        page.items
            .extend(listing::render(conn, &private.rows).await?);
        page.next_cursor = private.next_cursor;
        if private_offset.is_none()
            && i64::try_from(page.items.len()).unwrap_or(i64::MAX) == limit
            && page.items.iter().all(|row| row.as_visible().is_some())
            && page.next_cursor.is_some()
        {
            page.next_cursor = readable_end;
        }
    }
    let visible: Vec<_> = chosen.iter().map(|(row, _)| row.id).collect();
    let ids: Vec<_> = visible.iter().chain(&private_ids).copied().collect();
    let response = bind_sources(ctx, conn, declaration, &ids, &sources).await?;
    Ok(Ready {
        response,
        page,
        visible,
        private: private_ids,
        sources,
        query,
        declaration,
    })
}

pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: NodeDocumentListQuery,
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

async fn require_rows(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    ready: &Ready,
) -> ApiResult<()> {
    authorization::require_on_path_in(ctx, conn, principal, "node.view").await?;
    let ids: Vec<_> = ready
        .visible
        .iter()
        .chain(&ready.private)
        .copied()
        .collect();
    let mut standing = documents::listed(conn, principal, &ids, Policy::default())
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .map(|held| (held.document.id, held))
        .collect::<HashMap<_, _>>();
    let node = ctx.id("id")?;
    let graph = if ready.query.under.unwrap_or(false) {
        Some(
            orch_authz::engine::Graph::load(
                conn,
                &standing
                    .values()
                    .map(|held| held.document.node)
                    .collect::<Vec<_>>(),
            )
            .await
            .map_err(ApiError::internal)?,
        )
    } else {
        None
    };
    let in_native_query = |held: &documents::Listed| match &graph {
        Some(graph) => graph
            .get(held.document.node)
            .is_some_and(|current| current.path.contains(&node)),
        None => held.document.node == node,
    };
    for id in &ready.visible {
        let held = standing.remove(id).ok_or_else(refused)?;
        let needed = if ready.query.deleted.is_some() {
            Permission::DocManage
        } else {
            Permission::DocView
        };
        if held.document.deleted != ready.query.deleted.is_some()
            || !in_native_query(&held)
            || !held.holds(needed)
            || ready.query.participating.unwrap_or(false) && !held.participating
        {
            return Err(refused());
        }
    }
    for id in &ready.private {
        let held = standing.get(id).ok_or_else(refused)?;
        if held.document.deleted
            || !in_native_query(held)
            || !(held.holds(Permission::AccessManage) || held.holds(Permission::DocView))
        {
            return Err(refused());
        }
    }
    Ok(())
}

pub(super) async fn read(ctx: Ctx, query: NodeDocumentListQuery, (): ()) -> ApiResult<Reply> {
    let (page, actual, ready) = native::read(ctx, move |ctx, conn, principal| {
        let query = query.clone();
        Box::pin(async move {
            let ready = if native::modern(principal.caller()) {
                ctx.operation_data::<Ready>()?
            } else {
                Arc::new(select(ctx, conn, principal.caller(), query).await?)
            };
            let page = if native::modern(principal.caller()) {
                let projection = ctx.clone();
                let value = Arc::clone(&ready);
                ready
                    .response
                    .clone()
                    .read_operation(ctx, conn, principal.caller(), move |conn, reader| {
                        Box::pin(async move {
                            require_rows(&projection, conn, reader, &value).await?;
                            Ok(value.page.clone())
                        })
                    })
                    .await?
                    .ok_or_else(refused)?
            } else {
                require_rows(ctx, conn, principal, &ready).await?;
                ready.page.clone()
            };
            Ok((page, ctx.clone(), ready))
        })
    })
    .await?;
    #[cfg(test)]
    if let Some(document) = ready.visible.first() {
        crate::realtime::history_read_tests::document_response_cut(
            *document,
            actual.admitted_caller().is_some_and(native::modern),
            ready
                .sources
                .iter()
                .filter_map(|source| match source {
                    WrittenLabelSource::Node(id) => Some(*id),
                    _ => None,
                })
                .collect(),
            ready.sources.clone(),
        )
        .await;
    }
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
    Ok(page)
}

pub(super) async fn check_sources(
    ctx: &Ctx,
    conn: &mut Conn,
    reader: &Principal,
    ready: &Ready,
) -> ApiResult<()> {
    let ids: Vec<_> = ready
        .visible
        .iter()
        .chain(&ready.private)
        .copied()
        .collect();
    let plan = bind_sources(ctx, conn, ready.declaration, &ids, &ready.sources).await?;
    if native::modern(reader.caller()) {
        plan.clone().admit(reader.caller())?;
    }
    let current = native::preparation_principal(ctx, conn, reader.caller(), &plan).await?;
    let reader = &current;
    require_rows(ctx, conn, reader, ready).await?;
    let rows = rows::get_many(conn, &ready.visible).await?;
    let listed = documents::listed(conn, reader, &ready.visible, Policy::default())
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .map(|held| (held.document.id, held))
        .collect::<HashMap<_, _>>();
    let entries = rows
        .into_iter()
        .map(|row: DocumentRow| {
            let held = listed.get(&row.id).cloned().ok_or_else(refused)?;
            Ok((row, held))
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let (_, fresh) = ProjectedDocumentRef::capture_many_allowed(
        conn,
        reader,
        &entries,
        reader.now().get(),
        Policy::default(),
        &ready.sources,
    )
    .await?;
    if ready.sources.iter().any(|source| !fresh.contains(source)) {
        return Err(refused());
    }
    Ok(())
}
