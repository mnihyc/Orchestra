//! Native candidate identity and selected-row facts for the shared operation adapter.
use super::*;
use crate::key_operations::{native, targets};
use crate::routes::access::page::NameBounds;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::search_sources::{self as sources, Source};
use orch_authz::{Caller, Principal};
use orch_db::Conn;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub(super) fn collection(
    source: Source,
    targets: Vec<(KeyTargetKind, KeyResolvedTarget)>,
) -> native::Facts<Source> {
    let mut groups = BTreeMap::new();
    for (family, target) in targets {
        groups.entry(family).or_insert_with(Vec::new).push(target);
    }
    native::Facts::collection(source, groups.into_iter().collect())
}
/// NameRow.kind is the native producer's typed kind; tags retain their stored defining node.
async fn name_target(
    conn: &mut Conn,
    row: &rows::NameRow,
) -> ApiResult<(KeyTargetKind, KeyResolvedTarget)> {
    let (family, target) = match row.kind.as_str() {
        "user" => (
            KeyTargetKind::Identity(KeyIdentityKind::Account),
            targets::account(conn, row.id).await?,
        ),
        "group" => (
            KeyTargetKind::Identity(KeyIdentityKind::Group),
            targets::group(conn, row.id).await?,
        ),
        "chat" => (
            KeyTargetKind::Identity(KeyIdentityKind::Chat),
            targets::chat(conn, row.id).await?,
        ),
        "tag" => {
            let target = targets::node(conn, row.node_id.ok_or_else(ApiError::not_found)?).await?;
            (target.object.kind, target)
        }
        "machine" => (
            KeyTargetKind::Object(ObjectKind::Machine),
            targets::machine_subject(conn, row.id).await?,
        ),
        kind => {
            let kind = match kind {
                "org" => ObjectKind::Org,
                "project" => ObjectKind::Project,
                "topic" => ObjectKind::Topic,
                "thread" => ObjectKind::Thread,
                "document" => ObjectKind::Document,
                "document_container" => ObjectKind::DocumentContainer,
                "provider" => ObjectKind::Provider,
                "agent" => ObjectKind::Agent,
                "environment" => ObjectKind::Environment,
                "host" => ObjectKind::Host,
                "template" => ObjectKind::Template,
                "mcp_server" => ObjectKind::McpServer,
                "mount" => ObjectKind::Mount,
                "skill" => ObjectKind::Skill,
                _ => {
                    return Err(ApiError::internal(
                        "the native name kind has no target identity",
                    ));
                }
            };
            (
                KeyTargetKind::Object(kind),
                targets::page_object(
                    conn,
                    orch_access_contract::decisions::ObjectRef { kind, id: row.id },
                )
                .await?,
            )
        }
    };
    Ok((family, target))
}
/// Optional context keeps only labels admitted by this exact consuming operation.
pub(super) async fn name_bounds(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    declaration: &'static Declaration<Source>,
    nodes: BTreeSet<Id>,
) -> ApiResult<(NameBounds, BTreeSet<Id>, native::Facts<Source>)> {
    let graph = orch_authz::engine::Graph::load(conn, &nodes.iter().copied().collect::<Vec<_>>())
        .await
        .map_err(ApiError::internal)?;
    let plan = native::bind_known(ctx, declaration, &[])?;
    let mut bounds = NameBounds {
        limited: native::modern(caller),
        ..NameBounds::default()
    };
    let mut targets = Vec::new();
    for node in graph.nodes() {
        let Some(target) =
            orch_engine::operation_scope::targets::from_node_identity(&graph, node.id)
        else {
            continue;
        };
        if !bounds.limited
            || native::permits_usage(
                &plan,
                declaration,
                caller,
                &Source::Names,
                orch_api_contract::operations::FacetUsage::ResponseContent,
                target.object.kind,
                &target,
            )?
        {
            bounds.nodes.insert(node.id);
            targets.push((target.object.kind, target));
        }
    }
    let addresses = nodes
        .into_iter()
        .filter(|id| {
            graph
                .get(*id)
                .is_some_and(|node| node.path.iter().all(|id| bounds.nodes.contains(id)))
        })
        .collect();
    Ok((bounds, addresses, collection(Source::Names, targets)))
}

pub(crate) struct PreparedNames {
    pub selected: Vec<rows::NameRow>,
    pub admin_places: HashMap<(String, Id), orch_access_contract::roles::AdminReach>,
    pub participation: HashSet<(String, Id)>,
    pub partial: bool,
    pub bounds: NameBounds,
    pub addresses: BTreeSet<Id>,
}
pub(super) struct NamesReader;
impl native::Selected for NamesReader {
    type Source = Source;
    type Query = NameSearchListQuery;
    type Value = PreparedNames;
    type Reply = NameSearchPage;
    const DECLARATION: &'static Declaration<Source> = &sources::NAME_SEARCH;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(select_names(ctx, conn, caller, query))
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(super::render_names(ctx, conn, principal, value))
    }
}
async fn select_names(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: &NameSearchListQuery,
) -> ApiResult<(PreparedNames, Vec<native::Facts<Source>>)> {
    let limit = query.limit.unwrap_or(NAME_SEARCH_LIMIT_DEFAULT);
    if limit == 0 || limit > NAME_SEARCH_LIMIT_MAX {
        return Err(invalid("limit", "is outside the names page limits"));
    }
    let batch = i64::from(limit.max(NAME_SEARCH_LIMIT_DEFAULT)) * NAME_BATCH_PAGES;
    let limit = usize::try_from(limit).map_err(ApiError::internal)?;
    let kinds = kinds(query.kinds.as_deref(), NAME_KINDS)?;
    let text = query.q.as_deref().unwrap_or_default().trim();
    if !text.is_empty() && text.chars().count() < NAME_QUERY_MIN_CHARS {
        return Err(invalid(
            "q",
            "type at least two characters, or leave it empty for recent visits",
        ));
    }
    if !text.is_empty() && rows::Query::parse(text).terms().is_empty() {
        return Err(invalid("q", "give at least one word or quoted phrase"));
    }
    // This principal supplies only native membership/holder discovery. It authorizes no result.
    let discovery = authorization::load_principal(conn, caller.clone(), ctx.now.into()).await?;
    let principal = crate::sign_in::second_step::hold(conn, ctx.user()?, discovery).await?;
    let predicate = native::bind_known(ctx, &sources::NAME_SEARCH, &[])?;
    // The identities the roots are found from, as kept between requests (a system admin's roots are all of them).
    let held = if principal.is_system_admin() {
        None
    } else {
        Some(
            orch_authz::holders::held(conn, principal.user(), orch_types::Time::new(ctx.now))
                .await
                .map_err(ApiError::internal)?,
        )
    };
    let roots = rows::name_roots(
        conn,
        principal.user(),
        principal.is_system_admin(),
        ctx.now,
        held.as_deref().map(|sets| &sets.held),
    )
    .await?;
    let mut offset = 0;
    let mut selected = Vec::new();
    let mut selected_targets = std::collections::HashMap::new();
    let mut admin_places = std::collections::HashMap::new();
    let mut participation = std::collections::HashSet::new();
    let mut partial = false;
    for _ in 0..NAME_BATCHES_MAX {
        let candidates = rows::names(
            conn,
            &Names {
                identifiers: None,
                starred: query.starred.unwrap_or(false),
                at: ctx.now,
                user: principal.user(),
                trees: principal.member_of(),
                roots: &roots,
                system_admin: principal.is_system_admin(),
                text,
                kinds: &kinds,
                org: query.org_id,
                offset,
                limit: batch,
            },
        )
        .await?;
        let count = i64::try_from(candidates.len()).map_err(ApiError::internal)?;
        let mut admitted = Vec::new();
        let mut targets_for_batch = Vec::new();
        for row in candidates {
            let (family, target) = match name_target(conn, &row).await {
                Err(error)
                    if error.problem.code == orch_api_contract::common::ErrorCode::NotFound =>
                {
                    continue;
                }
                result => result?,
            };
            if !native::modern(caller)
                || native::permits(
                    &predicate,
                    &sources::NAME_SEARCH,
                    caller,
                    &Source::Results,
                    family,
                    &target,
                )?
            {
                selected_targets.insert((row.kind.clone(), row.id), (family, target.clone()));
                targets_for_batch.push((family, target));
                admitted.push(row);
            }
        }
        let candidates = admitted;
        if candidates.is_empty() {
            partial = count == batch;
            if count < batch {
                break;
            }
            offset += count;
            continue;
        }
        let prepared = native::bind_known(
            ctx,
            &sources::NAME_SEARCH,
            &[collection(Source::Results, targets_for_batch)],
        )?;
        let principal = native::preparation_principal(ctx, conn, caller, &prepared).await?;
        let chat_ids: Vec<Id> = candidates
            .iter()
            .filter(|row| row.kind == "chat")
            .map(|row| row.id)
            .collect();
        let chats = orch_db::queries::chats::chats(conn, &chat_ids).await?;
        let members = rows::participant_chats(conn, principal.user(), &chat_ids).await?;
        let searchable_chats: std::collections::HashSet<Id> = chats
            .iter()
            .filter(|chat| {
                chat.node_id.is_some()
                    || members.iter().any(|member| {
                        member.id == chat.id
                            && !principal.suspension().applies(&[member.org_id])
                            && (principal.is_in_tree(member.org_id) || member.guest)
                            && principal
                                .caller()
                                .permits(orch_access_contract::permissions::Permission::ChatView)
                            && principal.caller().reaches(Some(&[member.org_id]))
                    })
            })
            .map(|chat| chat.id)
            .collect();
        let visible = orch_engine::projections::visible_names(conn, &principal, &candidates)
            .await
            .map_err(ApiError::internal)?;
        admin_places.extend(
            visible
                .admin_access
                .iter()
                .map(|(key, place)| (key.clone(), *place)),
        );
        participation.extend(visible.participating.iter().cloned());
        selected.extend(candidates.into_iter().filter(|row| {
            visible.contains(&(row.kind.clone(), row.id))
                && (row.kind != "chat" || searchable_chats.contains(&row.id))
                && (!query.participating.unwrap_or(false)
                    || visible.participating.contains(&(row.kind.clone(), row.id)))
        }));
        partial = count == batch || selected.len() > limit;
        if selected.len() >= limit || count < batch {
            break;
        }
        offset += count;
    }
    selected.truncate(limit);
    let results = selected
        .iter()
        .map(|row| {
            selected_targets
                .get(&(row.kind.clone(), row.id))
                .cloned()
                .ok_or_else(ApiError::not_found)
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let mut nodes: std::collections::BTreeSet<_> =
        selected.iter().filter_map(|row| row.node_id).collect();
    nodes.extend(selected.iter().filter_map(|row| {
        admin_places
            .get(&(row.kind.clone(), row.id))
            .and_then(|reach| reach.place.node())
    }));
    let (bounds, addresses, names) =
        name_bounds(ctx, conn, caller, &sources::NAME_SEARCH, nodes).await?;
    Ok((
        PreparedNames {
            selected,
            admin_places,
            participation,
            partial,
            bounds,
            addresses,
        },
        vec![collection(Source::Results, results), names],
    ))
}

struct RelatedAccess<'a> {
    ctx: &'a Ctx,
    caller: &'a Caller,
    document: KeyResolvedTarget,
}
impl orch_engine::links::SourceAccess for RelatedAccess<'_> {
    async fn readable(
        &self,
        conn: &mut Conn,
        rows: &[orch_db::queries::links::LiveSource],
    ) -> Result<HashSet<Id>, orch_engine::EngineError> {
        let prepared: ApiResult<_> = async {
            let plan = native::bind_known(
                self.ctx,
                &sources::RELATED,
                &[native::Facts::one(
                    Source::Document,
                    self.document.object.kind,
                    self.document.clone(),
                )],
            )?;
            let mut selected = Vec::new();
            let mut facts = Vec::new();
            for row in rows {
                let (family, target) = match related_target(conn, &row.kind, row.source_id).await {
                    Err(error)
                        if error.problem.code == orch_api_contract::common::ErrorCode::NotFound =>
                    {
                        continue;
                    }
                    result => result?,
                };
                if !native::modern(self.caller)
                    || native::permits(
                        &plan,
                        &sources::RELATED,
                        self.caller,
                        &Source::Results,
                        family,
                        &target,
                    )?
                {
                    selected.push(row.clone());
                    facts.push((family, target));
                }
            }
            let plan = native::bind_known(
                self.ctx,
                &sources::RELATED,
                &[
                    native::Facts::one(
                        Source::Document,
                        self.document.object.kind,
                        self.document.clone(),
                    ),
                    collection(Source::Results, facts),
                ],
            )?;
            let principal =
                native::preparation_principal(self.ctx, conn, self.caller, &plan).await?;
            orch_engine::links::readable_by_person(conn, &principal, &selected)
                .await
                .map_err(ApiError::internal)
        }
        .await;
        prepared.map_err(|error| orch_engine::EngineError::unreadable(error.to_string()))
    }
}
async fn related_target(
    conn: &mut Conn,
    kind: &str,
    id: Id,
) -> ApiResult<(KeyTargetKind, KeyResolvedTarget)> {
    match kind {
        "document" => Ok((
            KeyTargetKind::Object(ObjectKind::Document),
            targets::page_object(
                conn,
                orch_access_contract::decisions::ObjectRef {
                    kind: ObjectKind::Document,
                    id,
                },
            )
            .await?,
        )),
        "chat" => Ok((
            KeyTargetKind::Identity(KeyIdentityKind::Chat),
            targets::chat(conn, id).await?,
        )),
        "conversation" => Ok((
            KeyTargetKind::Object(ObjectKind::Thread),
            targets::node(conn, id).await?,
        )),
        _ => Err(ApiError::internal("the native related kind is undeclared")),
    }
}
pub(crate) struct PreparedRelated {
    pub page: orch_engine::links::RelatedPage,
    pub bounds: NameBounds,
}
pub(super) struct RelatedReader;
impl native::Selected for RelatedReader {
    type Source = Source;
    type Query = orch_api_contract::common::PageQuery;
    type Value = PreparedRelated;
    type Reply = orch_api_contract::search::RelatedItems;
    const DECLARATION: &'static Declaration<Source> = &sources::RELATED;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let mut cancellation = rows::cancel_on_drop(&ctx.app.db, conn).await?;
            let document = ctx.id("id")?;
            let target = targets::page_object(
                conn,
                orch_access_contract::decisions::ObjectRef {
                    kind: ObjectKind::Document,
                    id: document,
                },
            )
            .await?;
            let mut facts = vec![native::Facts::one(
                Source::Document,
                target.object.kind,
                target.clone(),
            )];
            let plan = native::bind_known(ctx, Self::DECLARATION, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            let continued = query
                .cursor
                .as_deref()
                .map(crate::routes::links::related_after)
                .transpose()?;
            let after = continued.as_ref().map(|(position, _)| *position);
            let access = RelatedAccess {
                ctx,
                caller,
                document: target,
            };
            let page = ctx
                .app
                .engine
                .related_page_for_access(
                    conn,
                    &principal,
                    document,
                    after,
                    query.limit.map(|limit| {
                        usize::try_from(limit.clamp(1, orch_api_contract::common::MAX_LIMIT))
                            .unwrap_or(1)
                    }),
                    &access,
                )
                .await
                .map_err(ApiError::internal)?
                .ok_or_else(ApiError::not_found)?;
            if continued.is_some_and(|(_, fingerprint)| fingerprint != page.fingerprint) {
                return Err(invalid("cursor", "the related list changed; start again"));
            }
            let mut results = Vec::new();
            for item in &page.sources {
                results.push(related_target(conn, &item.kind, item.source_id).await?)
            }
            facts.push(collection(Source::Results, results));
            let nodes = page.sources.iter().map(|item| item.node_id).collect();
            let (mut bounds, _, names) =
                name_bounds(ctx, conn, caller, Self::DECLARATION, nodes).await?;
            facts.push(names);
            let documents: Vec<_> = page
                .sources
                .iter()
                .filter(|row| row.kind == "document")
                .map(|row| row.source_id)
                .collect();
            let folders =
                orch_db::queries::documents::containers::ancestors_many(conn, &documents).await?;
            let mut selected = Vec::new();
            for folder in folders {
                let target = targets::page_object(
                    conn,
                    orch_access_contract::decisions::ObjectRef {
                        kind: ObjectKind::DocumentContainer,
                        id: folder.id,
                    },
                )
                .await?;
                if !bounds.limited
                    || native::permits_usage(
                        &plan,
                        Self::DECLARATION,
                        caller,
                        &Source::Folders,
                        orch_api_contract::operations::FacetUsage::ResponseContent,
                        KeyTargetKind::Object(ObjectKind::DocumentContainer),
                        &target,
                    )?
                {
                    bounds.containers.insert(folder.id);
                    selected.push((KeyTargetKind::Object(ObjectKind::DocumentContainer), target));
                }
            }
            facts.push(collection(Source::Folders, selected));
            cancellation.disarm();
            Ok((PreparedRelated { page, bounds }, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(super::render_related(ctx, conn, principal, value))
    }
}
