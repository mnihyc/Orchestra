//! Native chat pages, with scope selection before the existing window and retained response identities.

use super::*;

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::registry::OperationPlan;

use crate::routes::access::operations::NameRoles;

use crate::routes::access::operations::projection_names;

use crate::routes::access::page::NameBounds;

use crate::routes::access::page::NameSources;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::chat_sources as declarations;

use orch_api_contract::chat_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::engine::Graph;

use std::sync::Arc;
const CHAT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Chat);

const NODES: &[(ObjectKind, Source)] = &[
    (ObjectKind::Org, Source::NameOrgs),
    (ObjectKind::Project, Source::NameProjects),
    (ObjectKind::Topic, Source::NameTopics),
    (ObjectKind::Thread, Source::NameThreads),
];

const NAMES: NameRoles = NameRoles {
    groups: None,
    nodes: [
        (ObjectKind::Org, "message_name_orgs"),
        (ObjectKind::Project, "message_name_projects"),
        (ObjectKind::Topic, "message_name_topics"),
        (ObjectKind::Thread, "message_name_threads"),
    ],
    containers: None,
};

#[derive(Clone, PartialEq, Eq)]
struct Witness {
    root: Id,
    scope: Scope,
    participating: bool,
    q: String,
    ids: Vec<Id>,
    previews: HashMap<Id, Id>,
    source: String,
    next: Option<After>,
}

pub(super) struct Selection {
    witness: Witness,
    chats: Vec<Loaded>,
    previews: HashMap<Id, MessageRow>,
    bounds: NameBounds,
    allowed: names::AllowSet,
}

pub(super) struct Output {
    pub(super) page: Page<Chat>,
    witness: Witness,
}

pub(super) struct Lists;

fn intent(query: &OrgChatListQuery) -> ApiResult<Scope> {
    let scope = query.scope.unwrap_or(ChatListScope::Mine);
    if query.of.is_some() && scope != ChatListScope::Others {
        return Err(ApiError::invalid(
            "of",
            "only the chats of others are listed by an org",
        ));
    }
    if query.include_empty.unwrap_or(false) && scope != ChatListScope::Nodes {
        return Err(ApiError::invalid(
            "include_empty",
            "requires the nodes scope",
        ));
    }
    Ok(match scope {
        ChatListScope::Mine => Scope::Mine,
        ChatListScope::Others => Scope::Others { of: query.of },
        ChatListScope::Nodes if query.include_empty.unwrap_or(false) => Scope::EmptyNodes,
        ChatListScope::Nodes => Scope::Nodes,
    })
}

fn selected_ids(query: &OrgChatListQuery) -> ApiResult<Option<Vec<Id>>> {
    query
        .ids
        .as_ref()
        .map(|text| {
            let mut ids = text
                .split(',')
                .map(|id| {
                    id.parse::<Id>()
                        .map_err(|_| ApiError::invalid("ids", "contains an invalid chat identity"))
                })
                .collect::<ApiResult<Vec<_>>>()?;
            ids.sort_unstable();
            ids.dedup();
            Ok(ids)
        })
        .transpose()
}

fn page_query(query: &OrgChatListQuery) -> orch_api_contract::common::CollectionQuery {
    orch_api_contract::common::CollectionQuery {
        q: query.q.clone(),
        limit: query.limit,
        cursor: query.cursor.clone(),
    }
}

fn page_source(actor: Id, root: Id, query: &OrgChatListQuery, scope: Scope) -> ApiResult<String> {
    serde_json::to_string(&(
        actor,
        root,
        format!("{scope:?}"),
        query.participating.unwrap_or(false),
        selected_ids(query)?,
    ))
    .map_err(ApiError::internal)
}

fn declaration(scope: Scope) -> &'static Declaration<Source> {
    if matches!(scope, Scope::Others { .. }) {
        &declarations::LIST_INSPECT
    } else {
        &declarations::LIST_MEMBER
    }
}

fn work(ctx: &Ctx, principal: &Principal) -> Work {
    Work {
        projection_ctx: ctx.clone(),
        principal: principal.clone(),
        origin: origin(ctx),
        now: principal.now().get(),
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the native list query and its selected operation are independent operands"
)]
async fn select_page(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    query: &OrgChatListQuery,
    root: Id,
    scope: Scope,
    base: &OperationPlan,
    frozen: Option<&Witness>,
) -> ApiResult<chats::Listed> {
    let acting = work(ctx, principal);
    let participating = query.participating.unwrap_or(false);
    let source = page_source(principal.user(), root, query, scope)?;
    let after =
        orch_api_base::collection::after::<(DateTime<Utc>, Id)>(&page_query(query), &source)?
            .map(|(at, id)| After { at, id });
    let limit = i64::from(
        query
            .limit
            .unwrap_or(orch_api_contract::common::DEFAULT_LIMIT),
    );
    if limit == 0 {
        return Err(ApiError::invalid("limit", "must be greater than zero"));
    }
    if participating && matches!(scope, Scope::Others { .. }) {
        return Ok(chats::Listed {
            chats: Vec::new(),
            next: None,
        });
    }
    if let Some(frozen) = frozen {
        let listed = chats::list_selected(
            conn,
            &acting.acting(),
            root,
            scope,
            participating,
            &frozen.ids,
        )
        .await;
        let mut listed = acting.refuse(conn, None, listed).await?;
        if listed.chats.len() != frozen.ids.len()
            || frozen
                .ids
                .iter()
                .any(|id| !listed.chats.iter().any(|row| row.id == *id))
        {
            return Err(ApiError::not_found());
        }
        listed
            .chats
            .sort_by_key(|row| frozen.ids.iter().position(|id| *id == row.id));
        listed.next = frozen.next;
        return Ok(listed);
    }
    if let Some(ids) = selected_ids(query)? {
        let listed =
            chats::list_selected(conn, &acting.acting(), root, scope, participating, &ids).await;
        let listed = acting.refuse(conn, None, listed).await?;
        let targets = targets::from_chat_rows(conn, &listed.chats).await?;
        let mut selected = Vec::new();
        for row in listed.chats {
            if after.is_some_and(|after| (row.active_at(), row.id) >= (after.at, after.id)) {
                continue;
            }
            if !native::modern(principal.caller())
                || native::permits(
                    base,
                    declaration(scope),
                    principal.caller(),
                    &Source::ListResults,
                    CHAT,
                    targets.get(&row.id).ok_or_else(ApiError::not_found)?,
                )?
            {
                selected.push(row);
            }
        }
        let wanted = usize::try_from(limit).map_err(ApiError::internal)?;
        let more = selected.len() > wanted;
        selected.truncate(wanted);
        let next = more.then(|| selected.last()).flatten().map(|row| After {
            at: row.active_at(),
            id: row.id,
        });
        return Ok(chats::Listed {
            chats: selected,
            next,
        });
    }
    if !native::modern(principal.caller()) {
        let listed = if participating {
            chats::list_participating(conn, &acting.acting(), root, scope, after, limit).await
        } else {
            chats::list(conn, &acting.acting(), root, scope, after, limit).await
        };
        return acting.refuse(conn, None, listed).await;
    }
    let wanted = usize::try_from(limit).unwrap_or(usize::MAX);
    let mut selected = Vec::new();
    let mut cursor = after;
    loop {
        let listed = if participating {
            chats::list_participating(
                conn,
                &acting.acting(),
                root,
                scope,
                cursor,
                chats::LIST_STEP,
            )
            .await
        } else {
            chats::list(
                conn,
                &acting.acting(),
                root,
                scope,
                cursor,
                chats::LIST_STEP,
            )
            .await
        };
        let listed = acting.refuse(conn, None, listed).await?;
        let targets = targets::from_chat_rows(conn, &listed.chats).await?;
        for row in listed.chats {
            let target = targets.get(&row.id).ok_or_else(ApiError::not_found)?;
            if native::permits(
                base,
                declaration(scope),
                principal.caller(),
                &Source::ListResults,
                CHAT,
                target,
            )? {
                selected.push(row);
            }
        }
        if selected.len() > wanted || listed.next.is_none() {
            let more = selected.len() > wanted;
            selected.truncate(wanted);
            let next = more.then(|| selected.last()).flatten().map(|row| After {
                at: row.active_at(),
                id: row.id,
            });
            return Ok(chats::Listed {
                chats: selected,
                next,
            });
        }
        cursor = listed.next;
    }
}

async fn select_batch(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: &OrgChatListQuery,
    frozen: Option<&Witness>,
) -> ApiResult<(Arc<Selection>, Vec<native::Facts<Source>>)> {
    let root = ctx.id("id")?;
    let scope = intent(query)?;
    let participating = query.participating.unwrap_or(false);
    let base = OperationPlan::new(ctx.route, declaration(scope).operation)?;
    let principal = native::preparation_principal(ctx, conn, caller, &base).await?;
    if frozen.as_ref().is_some_and(|old| {
        old.root != root
            || old.scope != scope
            || old.participating != participating
            || old.q != query.q.as_deref().unwrap_or("").trim().to_lowercase()
            || page_source(caller.user(), root, query, scope).as_ref().ok() != Some(&old.source)
    }) {
        return Err(ApiError::internal("the retained chat-list intent changed"));
    }
    let listed = select_page(ctx, conn, &principal, query, root, scope, &base, frozen).await?;
    let ids: Vec<_> = listed.chats.iter().map(|row| row.id).collect();
    let mut members = chats::members_of(conn, &ids, principal.now())
        .await
        .map_err(ApiError::internal)?;
    let direct = chats::direct_allowed(conn, root)
        .await
        .map_err(ApiError::internal)?;
    let mut selected = Vec::new();
    for row in listed.chats {
        let people = members.remove(&row.id).unwrap_or_default();
        let policy = if row.node_id.is_some() {
            chats::policy_of(conn, &row)
                .await
                .map_err(ApiError::internal)?
        } else {
            orch_authz::chats::ChatPolicy {
                enabled: true,
                direct,
            }
        };
        let standing = orch_authz::chats::standing(conn, &principal, &row, &people, policy)
            .await
            .map_err(ApiError::internal)?;
        if standing.reads() {
            selected.push(Loaded {
                row,
                members: people,
                standing,
                policy,
            });
        }
    }
    if frozen
        .as_ref()
        .is_some_and(|old| selected.len() != old.ids.len())
    {
        return Err(ApiError::not_found());
    }
    let ids: Vec<_> = selected.iter().map(Loaded::id).collect();
    let mut previews = HashMap::new();
    if let Some(old) = frozen {
        for (chat, message) in &old.previews {
            let row = rows::message(conn, *chat, *message)
                .await?
                .ok_or_else(ApiError::not_found)?;
            previews.insert(*chat, row);
        }
    } else {
        previews.extend(
            rows::last_messages(conn, &ids)
                .await?
                .into_iter()
                .map(|row| (row.chat_id, row)),
        );
    }
    let rows: Vec<_> = selected.iter().map(|row| row.row.clone()).collect();
    let targets = targets::from_chat_rows(conn, &rows).await?;
    let actual: Vec<_> = ids
        .iter()
        .map(|id| targets.get(id).cloned().ok_or_else(ApiError::not_found))
        .collect::<ApiResult<_>>()?;
    let primary = native::Facts::collection(
        Source::ListResults,
        if actual.is_empty() {
            Vec::new()
        } else {
            vec![(CHAT, actual)]
        },
    );
    let mut plan = native::bind_known(ctx, declaration(scope), std::slice::from_ref(&primary))?;
    let mut sources = NameSources::default();
    for row in &selected {
        sources.nodes.extend(row.row.node_id);
        if row.row.kind == rows::KIND_GROUP {
            sources.nodes.insert(row.row.home());
        }
        sources
            .nodes
            .extend(row.standing.admin_place.and_then(|place| place.node()));
        sources.nodes.extend(
            row.standing
                .admin_access
                .and_then(|reach| reach.place.node()),
        );
    }
    sources
        .nodes
        .extend(previews.values().filter_map(|row| row.author_thread_id));
    let bounds = projection_names(conn, principal.caller(), &mut plan, sources, &NAMES).await?;
    let graph = Graph::load(conn, &bounds.nodes.iter().copied().collect::<Vec<_>>())
        .await
        .map_err(ApiError::internal)?;
    let (mut facts, _) = native::node_name_facts(
        &plan,
        declaration(scope),
        principal.caller(),
        &graph,
        &bounds.nodes,
        NODES,
    )?;
    facts.push(primary);
    let allowed = bounds
        .nodes
        .iter()
        .copied()
        .map(names::NameSource::Node)
        .collect();
    let witness = Witness {
        root,
        scope,
        participating,
        q: query.q.as_deref().unwrap_or("").trim().to_lowercase(),
        ids,
        previews: previews.iter().map(|(chat, row)| (*chat, row.id)).collect(),
        source: page_source(caller.user(), root, query, scope)?,
        next: listed.next,
    };
    Ok((
        Arc::new(Selection {
            witness,
            chats: selected,
            previews,
            bounds,
            allowed,
        }),
        facts,
    ))
}

/// Search only the same public name projection the person receives, then freeze one final native page.
async fn select_named(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: &OrgChatListQuery,
) -> ApiResult<(Arc<Selection>, Vec<native::Facts<Source>>)> {
    let frozen = ctx.optional_retained_source::<Witness>()?;
    let words = orch_db::queries::search::Query::parse(query.q.as_deref().unwrap_or(""));
    if frozen.is_some() || words.terms().is_empty() {
        return select_batch(ctx, conn, caller, query, frozen.as_deref()).await;
    }
    let wanted = usize::try_from(
        query
            .limit
            .unwrap_or(orch_api_contract::common::DEFAULT_LIMIT)
            .max(1),
    )
    .map_err(ApiError::internal)?;
    let mut scan = query.clone();
    scan.limit = Some(
        query
            .limit
            .unwrap_or(orch_api_contract::common::DEFAULT_LIMIT)
            .max(orch_api_contract::common::DEFAULT_LIMIT),
    );
    let mut ids = Vec::new();
    let mut previews = HashMap::new();
    let mut last = None;
    let mut more = false;
    loop {
        let (selection, facts) = select_batch(ctx, conn, caller, &scan, None).await?;
        let plan = native::bind_known(ctx, declaration(selection.witness.scope), &facts)?;
        let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
        let output = <Lists as native::Selected>::render(ctx, conn, &principal, &selection).await?;
        for chat in output.page.items {
            let people = chat
                .members
                .as_ref()
                .into_iter()
                .flatten()
                .map(|user| format!("{} {}", user.display_name, user.username))
                .collect::<Vec<_>>()
                .join(" ");
            let node = chat
                .base
                .node
                .as_ref()
                .filter(|node| !node.redacted)
                .and_then(|node| node.name.as_deref())
                .unwrap_or("");
            if !words.matches(&format!(
                "{} {people} {node}",
                chat.base.name.as_deref().unwrap_or("")
            )) {
                continue;
            }
            if ids.len() == wanted {
                more = true;
                break;
            }
            let loaded = selection
                .chats
                .iter()
                .find(|row| row.id() == chat.base.id)
                .ok_or_else(|| {
                    ApiError::internal("a projected conversation lost its native row")
                })?;
            last = Some(After {
                at: loaded.row.active_at(),
                id: loaded.id(),
            });
            ids.push(loaded.id());
            if let Some(preview) = selection.witness.previews.get(&loaded.id()) {
                previews.insert(loaded.id(), *preview);
            }
        }
        if more {
            break;
        }
        match output.page.next_cursor {
            Some(next) => scan.cursor = Some(next),
            None => break,
        }
    }
    let witness = Witness {
        root: ctx.id("id")?,
        scope: intent(query)?,
        participating: query.participating.unwrap_or(false),
        q: query.q.as_deref().unwrap_or("").trim().to_lowercase(),
        ids,
        previews,
        source: page_source(caller.user(), ctx.id("id")?, query, intent(query)?)?,
        next: if more { last } else { None },
    };
    select_batch(ctx, conn, caller, query, Some(&witness)).await
}

impl native::Selected for Lists {
    type Source = Source;
    type Query = OrgChatListQuery;
    type Value = Arc<Selection>;
    type Reply = Output;
    const DECLARATION: &'static Declaration<Source> = &declarations::LIST_MEMBER;

    fn declaration(_query: &Self::Query, value: &Self::Value) -> &'static Declaration<Source> {
        declaration(value.witness.scope)
    }

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(select_named(ctx, conn, caller, query))
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if ctx.user()?.id() != principal.user() {
                return Err(ApiError::internal("the chat-list reader changed account"));
            }
            Ok(())
        })
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Output>> {
        Box::pin(async move {
            let allowed = value.bounds.limited.then_some(&value.allowed);
            let mut items = Vec::new();
            for loaded in &value.chats {
                let mut chat = chat_base(
                    conn,
                    principal,
                    &loaded.row,
                    &loaded.members,
                    &loaded.standing,
                    allowed,
                )
                .await?;
                if let Some(preview) = value.previews.get(&loaded.id()) {
                    let people: Vec<_> = preview.author_id.into_iter().collect();
                    let threads: Vec<_> = preview.author_thread_id.into_iter().collect();
                    let lookups =
                        names::lookups(conn, principal, &people, &threads, &[], &[], allowed)
                            .await?;
                    chat.preview = Some(views::preview(preview, &lookups.lookups));
                }
                items.push(chat);
            }
            Ok(Output {
                page: Page {
                    items,
                    next_cursor: value
                        .witness
                        .next
                        .as_ref()
                        .map(|after| {
                            orch_api_base::collection::cursor(
                                &orch_api_contract::common::CollectionQuery {
                                    q: Some(value.witness.q.clone()),
                                    ..Default::default()
                                },
                                &value.witness.source,
                                (after.at, after.id),
                            )
                        })
                        .transpose()?,
                },
                witness: value.witness.clone(),
            })
        })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a Self::Query,
        opened: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.witness.root == fresh.witness.root
                && opened.witness.scope == fresh.witness.scope
                && opened.witness.q == fresh.witness.q
                && opened.witness.source == fresh.witness.source
                && opened.chats.iter().all(|old| {
                    fresh.chats.iter().any(|new| {
                        old.id() == new.id()
                            && old.row.org_id == new.row.org_id
                            && old.row.kind == new.row.kind
                            && old.row.node_id == new.row.node_id
                            && old.row.home_id == new.row.home_id
                            && old.standing == new.standing
                            && old.policy == new.policy
                    })
                })
                && opened.previews.iter().all(|(chat, row)| {
                    fresh
                        .previews
                        .get(chat)
                        .is_some_and(|new| row.id == new.id && row.deleted_at == new.deleted_at)
                }))
        })
    }
}

pub(super) async fn read(ctx: Ctx, query: OrgChatListQuery, (): ()) -> ApiResult<Page<Chat>> {
    let output = native::selected_value::<Lists>(ctx.clone(), query, ()).await?;
    ctx.retain_source(output.witness)?;
    Ok(output.page)
}
