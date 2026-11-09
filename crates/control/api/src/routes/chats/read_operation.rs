//! Current chat reads and their live-name projection, shared by HTTP and existing chat feeds.

use super::*;
use crate::key_operations::{admission::binding_error, targets};
use crate::registry::{OperationFuture, OperationPlan};
use crate::routes::access::{
    operations::{NameRoles, group_targets, projection_names},
    page::{NameBounds, NameSources, holder_list_name_sources},
};
use orch_access_contract::key_scope::{KeyIdentityKind, KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::{operations::TargetResolver, routes::Route};
use orch_authz::Caller;

const CHAT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Chat);
const GROUP: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Group);
const GROUP_NAMES: &str = "message_name_groups";
const NAMES: NameRoles = NameRoles {
    groups: Some("chat_inspection_groups"),
    nodes: [
        (ObjectKind::Org, "message_name_orgs"),
        (ObjectKind::Project, "message_name_projects"),
        (ObjectKind::Topic, "message_name_topics"),
        (ObjectKind::Thread, "message_name_threads"),
    ],
    containers: None,
};
const MESSAGE_NAMES: NameRoles = NameRoles {
    groups: None,
    nodes: NAMES.nodes,
    containers: None,
};

/// Raw facts and trusted bounds from this exact read snapshot; never a shared rich response cache.
pub(super) struct PreparedRead {
    pub(super) loaded: Loaded,
    pub(super) complete: bool,
    pub(super) has_older: bool,
    include_chat: bool,
    messages: Vec<MessageRow>,
    replies: HashMap<Id, MessageRow>,
    preview: Option<MessageRow>,
    inspection: Vec<Holder>,
    roster: Vec<agents::PreparedOrigin>,
    draft: Option<ChatDraft>,
    bounds: NameBounds,
    allowed: names::AllowSet,
    inspection_groups: Vec<Id>,
    message_groups: Vec<KeyResolvedTarget>,
}

impl PreparedRead {
    /// Actual source identities retained before rendering; public UserRefs keep their native convention.
    pub(super) fn name_nodes(&self) -> &std::collections::BTreeSet<Id> {
        &self.bounds.nodes
    }
    pub(super) fn inspection_groups(&self) -> &[Id] {
        &self.inspection_groups
    }
    pub(super) fn message_groups(&self) -> &[KeyResolvedTarget] {
        &self.message_groups
    }
}

/// The same typed output cut feeds HTTP and the existing delta envelope.
pub(crate) struct ReadProjection {
    pub(crate) chat: Option<Chat>,
    pub(crate) messages: Vec<ChatMessage>,
    pub(crate) draft: Option<ChatDraft>,
}

pub(super) struct PreparedMessageList {
    pub(super) read: PreparedRead,
    pub(super) has_newer: bool,
}

/// Keep the native exactly-one-window validation independent of scope or rows.
fn message_window(query: &ChatMessageListQuery) -> ApiResult<Window> {
    let asked: Vec<_> = [
        query.before.map(Window::Before),
        query.after.map(Window::After),
        query.around.map(Window::Around),
        query
            .pinned
            .filter(|pinned| *pinned)
            .map(|_| Window::Pinned),
        query.item_id.map(Window::Quoting),
    ]
    .into_iter()
    .flatten()
    .collect();
    let [window] = asked.as_slice() else {
        return Err(ApiError::invalid(
            "",
            "name exactly one of before, after, around, pinned and item_id",
        ));
    };
    Ok(*window)
}

pub(super) fn list<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: ChatMessageListQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        // This public input validation preceded the native Chat read as well.
        let window = message_window(&query)?;
        let result = prepare_list(ctx, conn, caller, window, query.limit).await;
        match result {
            Ok(plan) => Ok(plan),
            Err(error) => {
                let now = orch_types::Time::new(ctx.app.clock.now());
                let current = orch_authz::CapturedCaller::capture(caller)
                    .restore(conn, now)
                    .await
                    .map_err(ApiError::internal)?;
                match current {
                    Some(current)
                        if current
                            .grant()
                            .is_some_and(|grant| grant.operation_scope().is_some()) =>
                    {
                        Err(authorization::problem_without_content(error))
                    }
                    Some(_) => Err(error),
                    None => Err(ApiError::new(
                        ErrorCode::AuthInvalidToken,
                        "the credential is no longer valid",
                    )),
                }
            }
        }
    })
}

async fn prepare_list(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    window: Window,
    limit: Option<u32>,
) -> ApiResult<OperationPlan> {
    let id = ctx.id("id")?;
    let row = rows::chat(conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let plan = base(conn, ctx.route, &row).await?;
    let current = orch_authz::CapturedCaller::capture(caller)
        .restore(conn, orch_types::Time::new(ctx.app.clock.now()))
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthInvalidToken,
                "the credential is no longer valid",
            )
        })?;
    let principal = plan.preparation_principal(ctx, conn, &current).await?;
    let work = Work {
        projection_ctx: ctx.clone(),
        now: principal.now().get(),
        principal,
        origin: origin(ctx),
    };
    let loaded = load(conn, &work, id, Mode::Read).await?;
    let limit = work
        .refuse(conn, Some(id), messages::limit_of(limit))
        .await?;
    let slice = messages::window(conn, &loaded.row, window, limit).await;
    let slice = work.refuse(conn, Some(id), slice).await?;
    let has_newer = slice.has_newer;
    let selected = messages::Synced {
        messages: slice.messages,
        // List pagination is retained separately; Sync's transport completeness
        // is not returned by this operation.
        complete: true,
        has_older: slice.has_older,
    };
    let (plan, read) = prepare_projection_with_names(
        conn,
        &work.principal,
        plan,
        loaded,
        selected,
        false,
        None,
        &MESSAGE_NAMES,
    )
    .await?;
    Ok(plan.with_data(PreparedMessageList { read, has_newer }))
}

/// No selected conversation means no message, roster, inspection or draft source.
/// The caller separately binds its genuine possibly-empty chosen-Chat collection.
pub(super) fn empty_opening(plan: &mut OperationPlan) -> ApiResult<()> {
    let roles = NAMES
        .nodes
        .iter()
        .map(|(_, role)| *role)
        .chain(NAMES.groups)
        .chain(std::iter::once(GROUP_NAMES));
    for role in roles {
        let index = plan
            .variant()
            .facets
            .iter()
            .position(|facet| facet.target == TargetResolver::ResolvedCollection(role))
            .ok_or_else(|| ApiError::internal("the conversation source is not declared"))?;
        plan.bind_empty(index)?;
    }
    Ok(())
}

async fn base(conn: &mut Conn, route: Route, row: &ChatRow) -> ApiResult<OperationPlan> {
    let mut plan = OperationPlan::new(route, route.name)?;
    let target = targets::from_chat(conn, row).await?;
    plan.bind(0, CHAT, &[target])?;
    Ok(plan)
}

/// Subscription eligibility publishes no rows. Each later delivery resolves its complete
/// actual sources again through `read_for`, under this same ChatSyncGet selection.
pub(crate) async fn root_plan(conn: &mut Conn, chat: Id) -> ApiResult<OperationPlan> {
    // Scope eligibility follows the real retained identity. The ordinary source loader still
    // decides whether its root is gone; resolving identity grants no restoration or content read.
    let target = targets::chat_identity(conn, chat).await?;
    let mut plan = OperationPlan::new(ChatSyncGet::ROUTE, ChatSyncGet::ROUTE.name)?;
    plan.bind(0, CHAT, &[target])?;
    for index in 1..plan.variant().facets.len() {
        plan.bind_empty(index)?;
    }
    Ok(plan)
}

pub(super) fn get<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(prepare_http(ctx, conn, caller, None))
}

pub(super) fn sync<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: ChatSyncGetQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(prepare_http(ctx, conn, caller, Some(query)))
}

pub(super) fn opening<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: ChatSyncGetQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        if query.since.is_some() {
            return Err(ApiError::invalid(
                "since",
                "an opening starts without a position",
            ));
        }
        prepare_http(ctx, conn, caller, Some(query)).await
    })
}

async fn prepare_http(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: Option<ChatSyncGetQuery>,
) -> ApiResult<OperationPlan> {
    let id = ctx.id("id")?;
    let row = rows::chat(conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let primary = base(conn, ctx.route, &row).await?;
    let principal = primary.preparation_principal(ctx, conn, caller).await?;
    let work = Work {
        projection_ctx: ctx.clone(),
        now: principal.now().get(),
        principal,
        origin: origin(ctx),
    };
    let loaded = load(conn, &work, id, Mode::Read).await?;
    let own_draft = ctx.route.name == ChatOpeningGet::ROUTE.name;
    let (plan, prepared) = prepare_window(conn, &work, ctx.route, loaded, query, own_draft).await?;
    Ok(plan.with_data(prepared))
}

async fn prepare_window(
    conn: &mut Conn,
    work: &Work,
    route: Route,
    loaded: Loaded,
    query: Option<ChatSyncGetQuery>,
    own_draft: bool,
) -> ApiResult<(OperationPlan, PreparedRead)> {
    let plan = base(conn, route, &loaded.row).await?;
    prepare_window_with_plan(conn, work, plan, loaded, query, own_draft).await
}

/// The same conversation sources under the consuming operation's own canonical plan.
/// Its root and chosen Chat have already been bound by that operation's producer.
pub(super) async fn prepare_opening_with_plan(
    conn: &mut Conn,
    work: &Work,
    plan: OperationPlan,
    loaded: Loaded,
    limit: Option<u32>,
) -> ApiResult<(OperationPlan, PreparedRead)> {
    prepare_window_with_plan(
        conn,
        work,
        plan,
        loaded,
        Some(ChatSyncGetQuery { since: None, limit }),
        true,
    )
    .await
}

async fn prepare_window_with_plan(
    conn: &mut Conn,
    work: &Work,
    plan: OperationPlan,
    loaded: Loaded,
    query: Option<ChatSyncGetQuery>,
    own_draft: bool,
) -> ApiResult<(OperationPlan, PreparedRead)> {
    let id = loaded.id();
    let synced = if let Some(query) = query {
        if query.since.is_some_and(|since| since < 0) {
            return Err(ApiError::invalid(
                "since",
                "is a position in the chat, from 0",
            ));
        }
        let limit = work
            .refuse(conn, Some(id), messages::limit_of(query.limit))
            .await?;
        let synced = messages::sync(conn, &loaded.row, query.since, limit).await;
        work.refuse(conn, Some(id), synced).await?
    } else {
        messages::Synced {
            messages: Vec::new(),
            complete: true,
            has_older: false,
        }
    };
    let draft = if own_draft && !loaded.standing.inspects() {
        let draft = following::draft(conn, &work.acting(), id).await;
        work.refuse(conn, Some(id), draft).await?
    } else {
        None
    };
    let (plan, prepared) =
        prepare_projection_with_plan(conn, &work.principal, plan, loaded, synced, true, draft)
            .await?;
    Ok((plan, prepared))
}

pub(super) fn opening_view(
    prepared: &PreparedRead,
    projected: ReadProjection,
) -> ApiResult<ChatOpening> {
    let chat = projected
        .chat
        .ok_or_else(|| ApiError::internal("the opening read has no chat projection"))?;
    Ok(ChatOpening {
        sync: ChatSync {
            seq: chat.seq,
            chat,
            since: None,
            complete: prepared.complete,
            messages: projected.messages,
            has_older: prepared.has_older,
        },
        draft: projected.draft,
    })
}

/// A live feed passes only the actual raw rows of its `(after, upto]` selection. No caller or
/// credential is reconstructed; this is the same source preparation and projection as HTTP.
pub(crate) async fn read_for(
    conn: &mut Conn,
    principal: &Principal,
    loaded: &Loaded,
    messages: &[MessageRow],
    include_chat: bool,
) -> ApiResult<ReadProjection> {
    let synced = messages::Synced {
        messages: messages.to_vec(),
        complete: true,
        has_older: false,
    };
    let (plan, prepared) = prepare_projection(
        conn,
        principal,
        ChatSyncGet::ROUTE,
        loaded.clone(),
        synced,
        include_chat,
        None,
    )
    .await?;
    plan.admit(principal.caller())?;
    project(conn, principal, &prepared).await
}

async fn prepare_projection(
    conn: &mut Conn,
    principal: &Principal,
    route: Route,
    loaded: Loaded,
    synced: messages::Synced,
    include_chat: bool,
    draft: Option<ChatDraft>,
) -> ApiResult<(OperationPlan, PreparedRead)> {
    let plan = base(conn, route, &loaded.row).await?;
    prepare_projection_with_plan(conn, principal, plan, loaded, synced, include_chat, draft).await
}

pub(super) async fn prepare_projection_with_plan(
    conn: &mut Conn,
    principal: &Principal,
    plan: OperationPlan,
    loaded: Loaded,
    synced: messages::Synced,
    include_chat: bool,
    draft: Option<ChatDraft>,
) -> ApiResult<(OperationPlan, PreparedRead)> {
    prepare_projection_with_names(
        conn,
        principal,
        plan,
        loaded,
        synced,
        include_chat,
        draft,
        &NAMES,
    )
    .await
}

/// The shared completed-response adapter has already admitted this operation's primary Read.
/// Collect the same native sources without re-admitting its completed Write/Manage.
pub(super) async fn prepare_completed_projection(
    conn: &mut Conn,
    principal: &Principal,
    plan: OperationPlan,
    loaded: Loaded,
) -> ApiResult<(OperationPlan, PreparedRead)> {
    collect_projection_with_names(
        conn,
        principal,
        plan,
        loaded,
        messages::Synced {
            messages: Vec::new(),
            complete: true,
            has_older: false,
        },
        true,
        None,
        &NAMES,
    )
    .await
}

#[allow(
    clippy::too_many_arguments,
    reason = "the canonical current read supplies its raw rows, existing transport metadata and explicit projection-role contract"
)]
async fn prepare_projection_with_names(
    conn: &mut Conn,
    principal: &Principal,
    plan: OperationPlan,
    loaded: Loaded,
    synced: messages::Synced,
    include_chat: bool,
    draft: Option<ChatDraft>,
    roles: &NameRoles,
) -> ApiResult<(OperationPlan, PreparedRead)> {
    // This proves the current primary scope before looking at content, while retaining the
    // original ordinary principal. The complete footprint is admitted before projection.
    plan.shared()
        .preparation_caller(principal.caller())
        .map_err(binding_error)?;
    collect_projection_with_names(
        conn,
        principal,
        plan,
        loaded,
        synced,
        include_chat,
        draft,
        roles,
    )
    .await
}

#[allow(
    clippy::too_many_arguments,
    reason = "the same native conversation factory receives its selected rows, output window and canonical name roles"
)]
async fn collect_projection_with_names(
    conn: &mut Conn,
    principal: &Principal,
    mut plan: OperationPlan,
    loaded: Loaded,
    synced: messages::Synced,
    include_chat: bool,
    draft: Option<ChatDraft>,
    roles: &NameRoles,
) -> ApiResult<(OperationPlan, PreparedRead)> {
    let replies = messages::replies_of(conn, loaded.id(), &synced.messages)
        .await
        .map_err(ApiError::internal)?;
    let all: Vec<_> = synced.messages.iter().chain(replies.values()).collect();
    let mut sources = NameSources::default();
    sources.nodes.extend(views::threads_of(&all));
    sources.nodes.extend(views::role_places_of(&all));
    let mut groups = views::groups_of(&all);
    if let Some(draft) = &draft {
        // Only current labels of the caller's typed mentions are looked up. Stored text,
        // forwarded snapshots and reply content remain the author's unchanged draft.
        for source in names::candidate_sources(conn, &[], &draft.parts, None).await? {
            match source {
                names::NameSource::Node(id) => {
                    sources.nodes.insert(id);
                }
                names::NameSource::Group(id) => groups.push(id),
            }
        }
    }
    let group_slot = plan
        .variant()
        .facets
        .iter()
        .position(|facet| facet.target == TargetResolver::ResolvedCollection(GROUP_NAMES))
        .ok_or_else(|| ApiError::internal("the optional chat group-name source is not declared"))?;
    let mut optional_groups = Vec::new();
    for target in group_targets(conn, &groups).await? {
        if plan.permits_binding(
            principal.caller(),
            group_slot,
            GROUP,
            std::slice::from_ref(&target),
        )? {
            sources
                .nodes
                .extend(target.node_path().into_iter().flatten());
            optional_groups.push(target);
        }
    }
    if optional_groups.is_empty() {
        plan.bind_empty(group_slot)?;
    } else {
        plan.bind(group_slot, GROUP, &optional_groups)?;
    }
    let preview = if include_chat {
        rows::last_messages(conn, &[loaded.id()])
            .await?
            .into_iter()
            .next()
    } else {
        None
    };
    // A preview uses stored excerpt text and a current author only; it never dereferences quotes.
    sources
        .nodes
        .extend(preview.as_ref().and_then(|row| row.author_thread_id));
    let mut roster = if include_chat {
        let visible = agents::visible_origins(conn, principal, &loaded.row).await?;
        agents::prepare(conn, principal, &loaded.row, &visible).await?
    } else {
        Vec::new()
    };
    sources.nodes.extend(roster.iter().map(|row| row.thread));
    let inspection = if include_chat && loaded.row.node_id.is_none() {
        readers::holders(conn, principal, &loaded.row).await?
    } else {
        Vec::new()
    };
    if include_chat {
        sources.nodes.extend(loaded.row.node_id);
        if loaded.row.kind == rows::KIND_GROUP {
            sources.nodes.insert(loaded.row.home());
        }
        sources
            .nodes
            .extend(loaded.standing.admin_place.and_then(|place| place.node()));
        sources.nodes.extend(
            loaded
                .standing
                .admin_access
                .and_then(|reach| reach.place.node()),
        );
        if loaded.row.node_id.is_none() && !inspection.is_empty() {
            let holders = holder_list_name_sources(
                std::slice::from_ref(&inspection),
                Some(loaded.row.org_id),
            );
            sources.nodes.extend(holders.nodes);
            sources.groups.extend(holders.groups);
            sources.users.extend(holders.users);
        }
    }
    let inspection_groups = sources.groups.iter().copied().collect();
    let bounds = projection_names(conn, principal.caller(), &mut plan, sources, roles).await?;
    roster.retain(|row| !bounds.limited || bounds.nodes.contains(&row.thread));
    let mut allowed: names::AllowSet = bounds
        .nodes
        .iter()
        .copied()
        .map(names::NameSource::Node)
        .collect();
    allowed.extend(
        optional_groups
            .iter()
            .filter_map(|target| target.object.id)
            .map(names::NameSource::Group),
    );
    Ok((
        plan,
        PreparedRead {
            loaded,
            complete: synced.complete,
            has_older: synced.has_older,
            include_chat,
            messages: synced.messages,
            replies,
            preview,
            inspection,
            roster,
            draft,
            bounds,
            allowed,
            inspection_groups,
            message_groups: optional_groups,
        },
    ))
}

/// Materialize protected names only after the same operation admitted its complete footprint.
pub(super) async fn project(
    conn: &mut Conn,
    principal: &Principal,
    prepared: &PreparedRead,
) -> ApiResult<ReadProjection> {
    let allowed = prepared.bounds.limited.then_some(&prepared.allowed);
    let all: Vec<_> = prepared
        .messages
        .iter()
        .chain(prepared.replies.values())
        .collect();
    let captured = names::capture(conn, principal, &all, &[], None, allowed).await?;
    let messages = views::messages(&prepared.messages, &prepared.replies, &captured.lookups);
    let chat = if prepared.include_chat {
        let loaded = &prepared.loaded;
        let mut chat = chat_base(
            conn,
            principal,
            &loaded.row,
            &loaded.members,
            &loaded.standing,
            allowed,
        )
        .await?;
        chat.agents = Some(
            agents::project(
                conn,
                principal,
                &prepared.roster,
                prepared.bounds.limited.then_some(&prepared.bounds),
            )
            .await?,
        );
        if loaded.row.node_id.is_none() {
            chat.inspection_holders = Some(
                readers::project(
                    conn,
                    principal,
                    &loaded.row,
                    &prepared.inspection,
                    prepared.bounds.limited.then_some(&prepared.bounds),
                )
                .await?,
            );
        }
        if let Some(preview) = &prepared.preview {
            let row = [preview];
            let lookups = names::lookups(
                conn,
                principal,
                &views::people_of(&row),
                &views::threads_of(&row),
                &views::groups_of(&row),
                &[],
                allowed,
            )
            .await?;
            chat.preview = Some(views::preview(preview, &lookups.lookups));
        }
        Some(chat)
    } else {
        None
    };
    let mut draft = prepared.draft.clone();
    if let Some(draft) = &mut draft {
        mentions::draft_names_bounded(conn, principal, draft, allowed).await?;
    }
    Ok(ReadProjection {
        chat,
        messages,
        draft,
    })
}
