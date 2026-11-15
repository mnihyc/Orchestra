//! One ordinary share reviewed before applying it, with current holders, effects and the rules of its place.

use super::page::NameBounds;

use super::page::Names;

use super::page::entries_named_at_bounded;

use super::page::same;

use super::previews::worded;

use super::role_create_in;

use super::subject;

use crate::key_operations::targets;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::OperationFuture;

use crate::registry::OperationPlan;

use crate::routes::nodes::invalid;

use crate::sign_in::origin;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::pages::Holder;

use orch_access_contract::pages::PageChange;

use orch_access_contract::pages::PageKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::Profile;

use orch_access_contract::roles::PlaceRef;

use orch_api_contract::access as api;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::nodes::NodeKind;

use orch_api_contract::nodes::NodeOrSystem;

use orch_api_contract::nodes::NodeRef;

use orch_api_contract::operations::TargetResolver;

use orch_api_contract::share::ShareParts;

use orch_api_contract::share::ShareTo;

use orch_api_contract::share::ShareWhat;

use orch_authz::engine::Frame;

use orch_authz::engine::PageRef;

use orch_authz::giving::Acting;

use orch_authz::giving::Giving;

use orch_authz::pages;

use orch_authz::pages::ChangeError;

use orch_authz::previews;

use orch_authz::previews::Before;

use orch_authz::previews::Preview;

use orch_authz::previews::Scope;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_authz::things;

use orch_authz::things::Thing;

use orch_db::queries::access as access_rows;

use orch_db::queries::chats;

use orch_db::queries::environments::capabilities;

use orch_db::queries::hierarchy;

use orch_db::Conn;

use orch_db::DbError;

use orch_db::Tx;

use orch_engine::sharing::PreparedAgentShare;

use orch_engine::sharing::ShareError;

use orch_engine::sharing::ShareTrial;

use orch_settings::Place;

use orch_settings::Snapshot;

use orch_settings::Wanted;

use orch_settings_contract::provenance::Source;

use orch_settings_contract::provenance::SourceScope;

use orch_settings_contract::registry;

use orch_settings_contract::registry::keys;

use orch_types::Id;

use orch_types::Json;

use orch_types::Time;

use std::collections::BTreeMap;

use std::collections::BTreeSet;
/// Fresh trial operands from the common operation checkpoint. Never retained after rollback.
struct PreparedPreview {
    agent: Option<Result<PreparedAgentShare, ApiError>>,
    names: NameBounds,
}

const NODE_FAMILIES: &[(&str, ObjectKind)] = &[
    ("orgs", ObjectKind::Org),
    ("projects", ObjectKind::Project),
    ("topics", ObjectKind::Topic),
    ("threads", ObjectKind::Thread),
];

fn role(plan: &OperationPlan, name: &str) -> ApiResult<usize> {
    plan.variant().facets.iter().position(|facet| {
        matches!(facet.target, TargetResolver::Resolved(found) | TargetResolver::ResolvedCollection(found) if found == name)
    }).ok_or_else(|| ApiError::internal("the share preview role is not declared"))
}

fn bind(
    plan: &mut OperationPlan,
    name: &str,
    kind: KeyTargetKind,
    targets: &[KeyResolvedTarget],
) -> ApiResult<()> {
    let index = role(plan, name)?;
    if targets.is_empty() {
        plan.bind_empty(index)
    } else {
        plan.bind(index, kind, targets)
    }
}

/// A missing proposed holder has no current protected metadata. Its ordinary trial still
/// owns the refusal; preserving that trial avoids changing allowed:false into an early 404.
fn current_holder(result: ApiResult<KeyResolvedTarget>) -> ApiResult<Option<KeyResolvedTarget>> {
    match result {
        Ok(target) => Ok(Some(target)),
        Err(error) if error.problem.code == ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn bind_nodes(
    plan: &mut OperationPlan,
    prefix: &str,
    targets: &[KeyResolvedTarget],
) -> ApiResult<()> {
    for (suffix, kind) in NODE_FAMILIES {
        let selected: Vec<_> = targets
            .iter()
            .filter(|target| target.object.kind == KeyTargetKind::Object(*kind))
            .cloned()
            .collect();
        bind(
            plan,
            &format!("{prefix}_{suffix}"),
            KeyTargetKind::Object(*kind),
            &selected,
        )?;
    }
    Ok(())
}

/// The existing preview keeps its own operation selection. It shares agent-source preparation,
/// not the committing endpoint's operation identity or its authority.
pub(super) fn resolve<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: api::SharePreviewBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let result = resolve_in(ctx, conn, caller, body).await;
        if caller
            .grant()
            .and_then(|grant| grant.operation_scope())
            .is_some()
        {
            result.map_err(crate::middleware::authorization::problem_without_content)
        } else {
            result
        }
    })
}

async fn resolve_in(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    body: api::SharePreviewBody,
) -> ApiResult<OperationPlan> {
    let id = ctx.id("id")?;
    let object = ctx.param("object")?;
    if let api::SharePreviewBody::Agents { share } = &body
        && !source_matches(object, id, share)
    {
        return Err(invalid("share.what", "must name the thing in the path"));
    }
    let holders = selected(&body)?;
    let copying = matches!(&body, api::SharePreviewBody::Agents { share } if share.mode == orch_api_contract::threads::ShareMode::Snapshot);
    let (source, place) = if object == "chats" {
        if !matches!(&body, api::SharePreviewBody::Agents { .. }) {
            return Err(ApiError::not_found());
        }
        let row = chats::chat(conn, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        (targets::from_chat(conn, &row).await?, Some(row.org_id))
    } else {
        let thing = subject::named(ctx)?.thing;
        let (graph, site) = things::site(conn, thing)
            .await
            .map_err(|error| match error {
                orch_authz::Error::NotFound(_) => ApiError::not_found(),
                other => ApiError::internal(other),
            })?;
        (targets::from_site(&graph, &site)?, site.role_place)
    };
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    bind(
        &mut plan,
        "preview_source",
        source.object.kind,
        std::slice::from_ref(&source),
    )?;
    bind(
        &mut plan,
        "preview_source_manage",
        source.object.kind,
        if copying {
            &[]
        } else {
            std::slice::from_ref(&source)
        },
    )?;
    let principal = plan.preparation_principal(ctx, conn, caller).await?;
    if !matches!(&body, api::SharePreviewBody::Agents { .. }) {
        let thing = subject::named(ctx)?.thing;
        let (_, site) = things::site(conn, thing)
            .await
            .map_err(ApiError::internal)?;
        let policy = subject::policy_of(conn, thing, &site).await?;
        subject::require_view(conn, &principal, thing, policy).await?;
        if !previews::may_share(conn, &principal, thing, policy)
            .await
            .map_err(ApiError::internal)?
        {
            return Err(ApiError::new(
                ErrorCode::AuthzDenied,
                "this thing cannot currently be shared by this caller",
            ));
        }
    }
    let (mut plan, agent) = if let api::SharePreviewBody::Agents { share } = &body {
        let (plan, prepared) = crate::routes::share::operation::prepare_agents_trial(
            ctx,
            conn,
            principal.caller(),
            share,
            plan,
        )
        .await?;
        (plan, Some(prepared))
    } else {
        // These declarations describe no agent or copy effects in a Page/Role trial.
        for (index, facet) in plan.variant().facets.clone().iter().enumerate() {
            if let TargetResolver::ResolvedCollection(name) = facet.target
                && (name.starts_with("share_")
                    || name.starts_with("snapshot_")
                    || name == "viewer_grant_sources")
            {
                plan.bind_empty(index)?;
            }
        }
        (plan, None)
    };
    let mut candidates: BTreeSet<Id> = source.node_path().unwrap_or_default().into_iter().collect();
    candidates.extend(place);
    let mut held_nodes = Vec::new();
    let mut held_groups = Vec::new();
    let mut users = Vec::new();
    for holder in &holders {
        match *holder {
            Holder::Group { id } => {
                if let Some(target) = current_holder(targets::group(conn, id).await)? {
                    candidates.extend(target.node_path().unwrap_or_default());
                    held_groups.push(target);
                }
            }
            Holder::Node { id } | Holder::Agents { id } | Holder::Members { id } => {
                if let Some(target) = current_holder(targets::node(conn, id).await)? {
                    candidates.extend(target.node_path().unwrap_or_default());
                    held_nodes.push(target);
                }
            }
            Holder::Role { at, .. } => {
                if let Some(id) = at.or(place)
                    && let Some(target) = current_holder(targets::node(conn, id).await)?
                {
                    candidates.extend(target.node_path().unwrap_or_default());
                    held_nodes.push(target);
                }
            }
            Holder::User { id } => users.push(id),
            Holder::Everyone => {}
        }
    }
    bind_nodes(&mut plan, "preview_holder", &held_nodes)?;
    bind(
        &mut plan,
        "preview_groups",
        KeyTargetKind::Identity(KeyIdentityKind::Group),
        &held_groups,
    )?;
    let principal = plan
        .preparation_principal(ctx, conn, principal.caller())
        .await?;
    // Domain-derived addresses and inherited rule provenance use the same optional node-name bound.
    for (_, _, domain, _) in access_rows::holder_user_addresses(conn, &users).await? {
        candidates.insert(domain);
    }
    let all = hierarchy::with_ancestors(conn, &candidates.into_iter().collect::<Vec<_>>()).await?;
    let mut names = NameBounds {
        limited: principal
            .caller()
            .grant()
            .and_then(|grant| grant.operation_scope())
            .is_some(),
        ..NameBounds::default()
    };
    let mut named = Vec::new();
    for node in all {
        let target = targets::node_identity(conn, node.id).await?;
        let Some((suffix, kind)) = NODE_FAMILIES
            .iter()
            .find(|(_, kind)| target.object.kind == KeyTargetKind::Object(*kind))
        else {
            continue;
        };
        let index = role(&plan, &format!("preview_name_{suffix}"))?;
        if plan.permits_binding(
            principal.caller(),
            index,
            KeyTargetKind::Object(*kind),
            std::slice::from_ref(&target),
        )? {
            names.nodes.insert(node.id);
            named.push(target);
        }
    }
    bind_nodes(&mut plan, "preview_name", &named)?;
    Ok(plan.with_data(PreparedPreview { agent, names }))
}

enum TrialError {
    Page(Box<ChangeError>),
    Api(ApiError),
    Agent(Box<ShareError>),
}

fn empty_effects() -> api::PagePreview {
    api::PagePreview {
        server_host_use: None,
        owner_reads: None,
        pages: 0,
        orgs: None,
        gaining: Vec::new(),
        losing: Vec::new(),
        more: 0,
    }
}

fn source_matches(object: &str, id: Id, share: &orch_api_contract::share::ShareWithAgents) -> bool {
    match share.what {
        ShareWhat::Node(ref source) => object == "nodes" && source.node_id == id,
        ShareWhat::Document(ref source) => object == "documents" && source.document_id == id,
        ShareWhat::Chat(ref source) => object == "chats" && source.chat_id == id,
    }
}

fn selected(body: &api::SharePreviewBody) -> ApiResult<Vec<Holder>> {
    let mut holders = Vec::new();
    match body {
        api::SharePreviewBody::Page { change } => {
            let change: PageChange = crate::routes::notifications::person::page_change(change)?.0;
            if change.owners.is_some()
                || change.whole.is_some()
                || change.rows.iter().flatten().any(|row| {
                    row.inherit.is_some()
                        || row.locked.is_some()
                        || row.update.is_some()
                        || row.remove.is_some()
                })
            {
                return Err(invalid(
                    "change",
                    "a share preview only adds holders; use the permission-page preview for management edits",
                ));
            }
            for name in change.name.into_iter().flatten() {
                holders.extend(name.holders);
            }
            for row in change.rows.into_iter().flatten() {
                holders.extend(row.add.into_iter().flatten().map(|entry| entry.holder));
            }
        }
        api::SharePreviewBody::Role { give } => holders.push(match give.subject.kind {
            api::SubjectKind::User => Holder::User {
                id: give.subject.id,
            },
            api::SubjectKind::Group => Holder::Group {
                id: give.subject.id,
            },
            api::SubjectKind::Members => Holder::Members {
                id: give.subject.id,
            },
            api::SubjectKind::Role => Holder::Role {
                at: Some(give.subject.id),
                role: same(
                    &give
                        .subject
                        .role
                        .ok_or_else(|| invalid("give.subject.role", "is required"))?,
                )
                .ok_or_else(|| invalid("give.subject.role", "is not a role"))?,
            },
        }),
        api::SharePreviewBody::Agents { share } => match &share.to {
            ShareTo::Threads(threads) => {
                holders.extend(threads.iter().map(|to| Holder::Agents { id: to.thread_id }))
            }
            ShareTo::Nodes(nodes) => {
                holders.extend(nodes.iter().map(|to| Holder::Agents { id: to.node_id }))
            }
        },
    }
    let mut unique = Vec::new();
    for holder in holders {
        if !unique.contains(&holder) {
            unique.push(holder);
        }
    }
    Ok(unique)
}

async fn requested(
    conn: &mut Conn,
    principal: &Principal,
    thing: Thing,
    policy: Policy,
    body: &api::SharePreviewBody,
) -> ApiResult<Vec<Permission>> {
    let (graph, site) = things::site(conn, thing)
        .await
        .map_err(ApiError::internal)?;
    let extra = match thing {
        Thing::Node(id) => vec![PageRef::defaults_of(PlaceRef::Node(id))],
        _ => Vec::new(),
    };
    let frame = Frame::load_with(
        conn,
        principal,
        policy,
        graph,
        std::slice::from_ref(&site),
        &extra,
        Permission::ALL,
    )
    .await
    .map_err(ApiError::internal)?;
    let giving = Giving::of(&frame, Some(&site), PageKind::Live, Acting::Ordinary);
    let mut permissions = Vec::new();
    match body {
        api::SharePreviewBody::Page { change } => {
            let change: PageChange = crate::routes::notifications::person::page_change(change)?.0;
            for name in change.name.into_iter().flatten() {
                permissions.extend(name.choice.permissions(site.kind));
                if name.export.unwrap_or(true) {
                    permissions.extend(
                        name.choice
                            .exports(site.kind)
                            .into_iter()
                            .filter(|permission| giving.can_give(*permission, Profile::Any)),
                    );
                }
            }
            for row in change.rows.into_iter().flatten() {
                if let Ok(permission) = row.permission.parse::<Permission>() {
                    permissions.push(permission);
                }
            }
        }
        api::SharePreviewBody::Role { give } => {
            let role = same(&give.role).ok_or_else(|| invalid("give.role", "is not a role"))?;
            permissions = giving.role_permissions(role);
        }
        api::SharePreviewBody::Agents { .. } => {}
    }
    permissions.sort_unstable();
    permissions.dedup();
    Ok(permissions)
}

/// Captures both the place itself and, for a wider share, what follows it inside.
async fn before(
    conn: &mut Conn,
    principal: &Principal,
    thing: Thing,
    policy: Policy,
    inside: bool,
) -> ApiResult<Vec<Before>> {
    let mut out = vec![
        Before::take(conn, principal, Scope::Page(thing), policy)
            .await
            .map_err(ApiError::internal)?,
    ];
    if inside && let Thing::Node(node) = thing {
        let (_, site) = things::site(conn, thing)
            .await
            .map_err(ApiError::internal)?;
        if site.kind.node().and_then(|kind| kind.place()).is_none() {
            return Ok(out);
        }
        let limit = i64::try_from(previews::PAGES_MOST).map_err(ApiError::internal)?;
        let checked =
            orch_db::queries::access::everything::inside(conn, Some(node), limit + 1).await?;
        if checked.nodes.len() > previews::PAGES_MOST
            || checked.resources.len() > previews::PAGES_MOST
            || checked.documents.len() > previews::PAGES_MOST
        {
            return Err(ApiError::new(
                ErrorCode::RequestTooLarge,
                "this share is too large for a complete review",
            )
            .with("limit", limit));
        }
        out.push(
            Before::take(conn, principal, Scope::Inside(PlaceRef::Node(node)), policy)
                .await
                .map_err(ApiError::internal)?,
        );
    }
    Ok(out)
}

pub(crate) async fn effects(
    conn: &mut Conn,
    principal: &Principal,
    before: &[Before],
) -> ApiResult<api::PagePreview> {
    let mut out = empty_effects();
    let mut gains = BTreeMap::new();
    let mut losses = BTreeMap::new();
    for scope in before {
        let changed = scope
            .finish(conn, principal)
            .await
            .map_err(ApiError::internal)?;
        let preview = worded(conn, changed).await?;
        out.pages += preview.pages;
        out.more += preview.more;
        for (rows, target) in [(preview.gaining, &mut gains), (preview.losing, &mut losses)] {
            for person in rows {
                let value = target
                    .entry(person.user.id)
                    .or_insert_with(|| api::PreviewPerson {
                        user: person.user.clone(),
                        permissions: Vec::new(),
                        pages: 0,
                    });
                value.pages += person.pages;
                for permission in person.permissions {
                    if !value.permissions.contains(&permission) {
                        value.permissions.push(permission);
                    }
                }
            }
        }
    }
    out.gaining = gains.into_values().collect();
    out.losing = losses.into_values().collect();
    let limit = usize::try_from(api::PREVIEW_MAX_PEOPLE).map_err(ApiError::internal)?;
    for people in [&mut out.gaining, &mut out.losing] {
        people.sort_by(|one, other| {
            other
                .pages
                .cmp(&one.pages)
                .then(one.user.id.cmp(&other.user.id))
        });
        out.more += i64::try_from(people.len().saturating_sub(limit)).unwrap_or(i64::MAX);
        people.truncate(limit);
    }
    Ok(out)
}

async fn rules(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    place: Option<Id>,
    ports: bool,
    copying: bool,
    bounds: &NameBounds,
) -> ApiResult<Vec<api::SharePreviewRule>> {
    let names = [
        keys::SHARING_ALLOW_HOLDERS,
        keys::SHARING_ALLOW_EXTERNAL,
        keys::CAPABILITY_POLICY,
        keys::SHARING_SNAPSHOT_MAX_BYTES,
    ];
    let defs: Vec<_> = names.iter().filter_map(|key| registry::get(key)).collect();
    let snapshot = Snapshot::load(
        conn,
        place.map_or(Place::System, Place::Node),
        Wanted::Keys(&defs),
    )
    .await
    .map_err(ApiError::internal)?;
    let mut out = Vec::new();
    for def in defs {
        if def.key == keys::CAPABILITY_POLICY {
            continue;
        }
        if def.key == keys::SHARING_SNAPSHOT_MAX_BYTES && !copying {
            continue;
        }
        let resolved = snapshot.resolve(def);
        let source = rule_source(conn, principal, &resolved.source, bounds).await?;
        out.push(api::SharePreviewRule {
            key: def.key.to_owned(),
            value: resolved.value.unwrap_or(Json::Null),
            source,
            sharing: !copying || def.key == keys::SHARING_SNAPSHOT_MAX_BYTES,
        });
    }
    if ports && let Some(home) = place {
        let resolved = snapshot.resolve(
            registry::get(keys::CAPABILITY_POLICY)
                .ok_or_else(|| ApiError::internal("capability policy is not registered"))?,
        );
        for definition in capabilities::definitions(conn)
            .await?
            .into_iter()
            .filter(|def| def.key.starts_with("port_"))
        {
            let policy = ctx
                .app
                .engine
                .capability_policy(conn, home, &definition)
                .await
                .map_err(ApiError::internal)?;
            let source = match resolved
                .parts
                .iter()
                .rev()
                .find(|part| part.value.get(&definition.key).is_some())
            {
                Some(part) => rule_source(conn, principal, &part.source, bounds).await?,
                None => None,
            };
            out.push(api::SharePreviewRule {key:format!("capability.policy.{}",definition.key),value:serde_json::json!({"decision":policy.decision,"approver_scope":policy.approver_scope}),source,sharing:false});
        }
    }
    Ok(out)
}

async fn rule_source(
    conn: &mut Conn,
    principal: &Principal,
    source: &Source<Id>,
    bounds: &NameBounds,
) -> ApiResult<Option<NodeOrSystem>> {
    match source.node {
        Some(id) => Ok(bounds
            .refs(conn, principal, &[id])
            .await?
            .remove(&id)
            .map(NodeOrSystem::Node)),
        None => Ok((source.scope == SourceScope::System).then_some(NodeOrSystem::SYSTEM)),
    }
}

async fn agents(
    conn: &mut Conn,
    principal: &Principal,
    trial: ShareTrial,
    now: Time,
    bounds: &NameBounds,
) -> ApiResult<Vec<api::SharePreviewAgent>> {
    let ids: Vec<_> = trial.destinations.iter().map(|(id, _)| *id).collect();
    let names = bounds.refs(conn, principal, &ids).await?;
    let mut viewer_entries = BTreeMap::new();
    for record in &trial.live {
        viewer_entries.insert(
            record.to,
            crate::routes::share::viewer_entries_bounded(conn, principal, record, bounds).await?,
        );
    }
    let mut current =
        match orch_engine::sharing::current::project(conn, &trial.live, None, now).await {
            Ok(current) => current,
            Err(error) => return Err(crate::routes::share::failed(conn, error).await),
        };
    Ok(trial
        .destinations
        .into_iter()
        .map(|(id, future_threads)| {
            let live = trial.live.iter().find(|record| record.to == id);
            let old = trial.before.iter().find(|record| {
                record.to == id
                    && record.inside == trial.inside
                    && record.by == Some(principal.user())
            });
            api::SharePreviewAgent {
                effective_before: trial.effective_before.get(&id).cloned(),
                viewer_entries: viewer_entries.get(&id).cloned().unwrap_or_default(),
                to: names.get(&id).cloned().unwrap_or(NodeRef {
                    thread_display: None,
                    id,
                    kind: NodeKind::Thread,
                    name: None,
                    redacted: true,
                }),
                inside: trial.inside,
                parts: live.map_or_else(
                    || trial.copy_parts.get(&id).copied().unwrap_or(trial.parts),
                    |record| record.parts,
                ),
                already_shared: old.map_or(ShareParts::default(), |record| record.parts),
                current: live.and_then(|record| current.remove(&record.id)),
                download: live.is_none_or(crate::routes::share::downloads),
                future_threads,
                copy: trial.copies.iter().find(|(to, _, _, _)| *to == id).map(
                    |(_, files, bytes, already_sent)| api::SharePreviewCopy {
                        files: *files,
                        bytes: *bytes,
                        already_sent: *already_sent,
                    },
                ),
            }
        })
        .collect())
}

async fn rows(
    conn: &mut Conn,
    principal: &Principal,
    thing: Thing,
    policy: Policy,
    inside: bool,
    selected: &[Holder],
    bounds: &NameBounds,
) -> ApiResult<Vec<api::SharePreviewRow>> {
    let (_, site) = things::site(conn, thing)
        .await
        .map_err(ApiError::internal)?;
    let mut targets = vec![(false, thing.target())];
    if inside
        && let Thing::Node(id) = thing
        && site.kind.node().and_then(|kind| kind.place()).is_some()
    {
        targets.push((true, pages::Target::Defaults(PlaceRef::Node(id))));
    }
    let mut out = Vec::new();
    for (defaults, target) in targets {
        let page = pages::read(conn, principal, target, policy)
            .await
            .map_err(ApiError::internal)?;
        let entries: Vec<_> = page
            .rows
            .iter()
            .flat_map(|row| row.entries.iter())
            .filter(|entry| selected.contains(&entry.holder))
            .cloned()
            .collect();
        let named = entries_named_at_bounded(
            conn,
            principal,
            site.root,
            site.role_place,
            &entries,
            bounds,
        )
        .await?;
        for row in page.rows {
            let mut entries: Vec<_> = row
                .entries
                .iter()
                .filter_map(|entry| named.get(&entry.id).cloned())
                .collect();
            entries.sort_by_key(|entry| entry.id);
            out.push(api::SharePreviewRow {
                defaults,
                permission: row.permission.as_str().to_owned(),
                profile: same(&row.profile)
                    .ok_or_else(|| ApiError::internal("a row has an unknown profile"))?,
                before: entries,
                after: Vec::new(),
            });
        }
    }
    Ok(out)
}

/// The preview is rolled back before either a successful response or an ordinary refusal is returned.
pub async fn create(
    mut ctx: Ctx,
    (): (),
    body: api::SharePreviewBody,
) -> ApiResult<api::SharePreview> {
    let id = ctx.id("id")?;
    let object = ctx.param("object")?.to_owned();
    if let api::SharePreviewBody::Agents { share } = &body
        && !source_matches(&object, id, share)
    {
        return Err(invalid("share.what", "must name the thing in the path"));
    }
    let selected = selected(&body)?;
    let copying = matches!(&body,api::SharePreviewBody::Agents {share} if share.mode == orch_api_contract::threads::ShareMode::Snapshot);
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let prepared = guard.operation_data::<PreparedPreview>()?;
    let bounds = &prepared.names;
    let principal = guard.into_principal();
    let thing = if object == "chats" {
        None
    } else {
        Some(subject::named(&ctx)?.thing)
    };
    let (policy, revision, scope, permissions, ports) = if let Some(thing) = thing {
        let (_, site) = things::site(&mut tx, thing)
            .await
            .map_err(|error| match error {
                orch_authz::Error::NotFound(_) => ApiError::not_found(),
                other => ApiError::internal(other),
            })?;
        let policy = subject::policy_of(&mut tx, thing, &site).await?;
        subject::require_view(&mut tx, &principal, thing, policy).await?;
        if !copying
            && !previews::may_share(&mut tx, &principal, thing, policy)
                .await
                .map_err(ApiError::internal)?
        {
            return Err(ApiError::new(
                ErrorCode::AuthzDenied,
                "this thing cannot currently be shared by this caller",
            ));
        }
        let page = pages::read(&mut tx, &principal, thing.target(), policy)
            .await
            .map_err(ApiError::internal)?;
        let scope = match thing {
            Thing::Node(id) => Some(id),
            Thing::Other(object) if object.kind.is_document() => site.beside,
            _ => site.role_place,
        };
        let permissions = requested(&mut tx, &principal, thing, policy, &body).await?;
        (
            policy,
            Some(page.revision),
            scope,
            permissions,
            site.kind == ObjectKind::Environment,
        )
    } else {
        if !matches!(body, api::SharePreviewBody::Agents { .. }) {
            return Err(ApiError::not_found());
        }
        let row = chats::chat(&mut tx, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        (Policy::default(), None, Some(row.org_id), Vec::new(), false)
    };
    let read_rules = if thing.is_some() {
        rules(&ctx, &mut tx, &principal, scope, ports, copying, bounds).await?
    } else {
        Vec::new()
    };
    let inside = match &body {
        api::SharePreviewBody::Role { .. } => true,
        api::SharePreviewBody::Agents { share } => share.inside.unwrap_or(true),
        _ => false,
    };
    let before = match (thing, copying) {
        (Some(thing), false) => before(&mut tx, &principal, thing, policy, inside).await?,
        _ => Vec::new(),
    };
    let already = match (thing, &body) {
        (Some(thing), api::SharePreviewBody::Page { .. } | api::SharePreviewBody::Role { .. }) => {
            previews::already_held(&mut tx, &principal, thing, policy, &selected, &permissions)
                .await
                .map_err(ApiError::internal)?
        }
        _ => (Vec::new(), 0),
    };
    let old_rows = match (thing, copying) {
        (Some(thing), false) => {
            rows(
                &mut tx, &principal, thing, policy, inside, &selected, bounds,
            )
            .await?
        }
        _ => Vec::new(),
    };
    let anchor = match thing {
        Some(Thing::Node(id)) => Some(id),
        _ => None,
    };
    let server_hosts = match (&body, thing) {
        (api::SharePreviewBody::Page { change }, Some(Thing::Other(object))) => {
            crate::server_host_use::Before::take(&mut tx, &principal, object, change).await?
        }
        _ => None,
    };
    let disclosure = match &body {
        api::SharePreviewBody::Page { change } => {
            crate::thread_read_tools::sharing::Before::take(
                &ctx.app,
                &mut tx,
                anchor,
                &("page", anchor, change),
                false,
                true,
            )
            .await?
        }
        api::SharePreviewBody::Role { give } => {
            crate::thread_read_tools::sharing::Before::take(
                &ctx.app,
                &mut tx,
                anchor,
                &("role", anchor, give),
                false,
                true,
            )
            .await?
        }
        api::SharePreviewBody::Agents { share } => {
            crate::thread_read_tools::sharing::Before::take(
                &ctx.app,
                &mut tx,
                anchor,
                &("agents", share),
                share
                    .parts
                    .as_ref()
                    .is_some_and(|parts| parts.timeline.is_some()),
                share.inside.unwrap_or(true),
            )
            .await?
        }
    };
    let attempted = apply(
        &ctx,
        &mut tx,
        &principal,
        thing,
        policy,
        &body,
        prepared.agent.as_ref(),
    )
    .await;
    let mut answer = api::SharePreview {
        owner_reads: None,
        allowed: false,
        revision,
        permissions: permissions
            .iter()
            .map(|permission| permission.as_str().to_owned())
            .collect(),
        holders: Vec::new(),
        effects: empty_effects(),
        already: Vec::new(),
        already_more: 0,
        agents: Vec::new(),
        rows: Vec::new(),
        rules: read_rules,
        refusal: None,
    };
    match attempted {
        Ok(trial) => {
            answer.owner_reads = disclosure.disclosure(&ctx.app, &mut tx).await?;
            answer.allowed = true;
            answer.effects = effects(&mut tx, &principal, &before).await?;
            if let Some(before) = server_hosts {
                answer.effects.server_host_use = before.disclosure(&mut tx, &principal).await?;
            }
            let already_view = worded(
                &mut tx,
                Preview {
                    gaining: already.0,
                    ..Preview::default()
                },
            )
            .await?;
            answer.already = already_view.gaining;
            answer.already_more = already.1 + already_view.more;
            answer.holders =
                Names::holders_bounded(&mut tx, &principal, None, scope, &selected, bounds).await?;
            if let Some(thing) = thing
                && !copying
            {
                let new_rows = rows(
                    &mut tx, &principal, thing, policy, inside, &selected, bounds,
                )
                .await?;
                for mut old in old_rows {
                    if let Some(new) = new_rows.iter().find(|new| {
                        new.defaults == old.defaults
                            && new.permission == old.permission
                            && new.profile == old.profile
                    }) {
                        old.after = new.before.clone();
                        if old.before != old.after {
                            answer.rows.push(old);
                        }
                    }
                }
            }
            if let Some(trial) = trial {
                answer.agents =
                    agents(&mut tx, &principal, trial, Time::new(ctx.now), bounds).await?;
            }
            tx.rollback().await.map_err(DbError::from)?;
        }
        Err(error) => {
            tx.rollback().await.map_err(DbError::from)?;
            let mut conn = ctx.app.db.acquire().await?;
            let mut refusal = match error {
                TrialError::Api(error) => error,
                TrialError::Page(error) => refusals::change_failed(&mut conn, *error).await,
                TrialError::Agent(error) => {
                    crate::routes::share::failed_projected(&mut conn, *error, bounds.limited).await
                }
            };
            if principal
                .caller()
                .grant()
                .and_then(|grant| grant.operation_scope())
                .is_some()
            {
                refusal = crate::middleware::authorization::problem_without_content(refusal);
            }
            // A chat's inspection is not membership or sharing authority: no preview is returned to that reader.
            if thing.is_none() || copying {
                return Err(refusal);
            }
            refusal.problem.request_id = ctx.request_id.clone();
            answer.refusal = Some(*refusal.problem);
        }
    }
    Ok(answer)
}

async fn apply(
    ctx: &Ctx,
    tx: &mut Tx,
    principal: &Principal,
    thing: Option<Thing>,
    policy: Policy,
    body: &api::SharePreviewBody,
    prepared: Option<&Result<PreparedAgentShare, ApiError>>,
) -> Result<Option<ShareTrial>, TrialError> {
    match body {
        api::SharePreviewBody::Page { change } => {
            let thing = thing.ok_or_else(|| TrialError::Api(ApiError::not_found()))?;
            crate::routes::access::link_origin::check(tx, principal, change.link_origin)
                .await
                .map_err(TrialError::Api)?;
            let change = crate::routes::notifications::person::page_change(change)
                .map_err(TrialError::Api)?
                .0;
            match thing {
                Thing::Node(_) => {
                    pages::change(tx, principal, thing.target(), &change, policy, &origin(ctx))
                        .await
                        .map_err(|error| TrialError::Page(Box::new(error)))?
                }
                Thing::Other(object) => {
                    let (_, site) = things::site(tx, thing)
                        .await
                        .map_err(|error| TrialError::Api(ApiError::internal(error)))?;
                    let resource_policy = subject::resource_policy(tx, thing, &site)
                        .await
                        .map_err(TrialError::Api)?;
                    pages::change_resource_under(
                        tx,
                        principal,
                        object,
                        &change,
                        policy,
                        &resource_policy,
                        &origin(ctx),
                    )
                    .await
                    .map_err(|error| TrialError::Page(Box::new(error)))?
                }
            };
            Ok(None)
        }
        api::SharePreviewBody::Role { give } => {
            let Some(Thing::Node(node)) = thing else {
                return Err(TrialError::Api(invalid(
                    "kind",
                    "roles are given on places",
                )));
            };
            role_create_in(ctx, tx, principal, node, give)
                .await
                .map_err(TrialError::Api)?;
            Ok(None)
        }
        api::SharePreviewBody::Agents { share } => ctx
            .app
            .engine
            .preview_prepared_agent_share_in(
                tx,
                principal,
                &origin(ctx),
                share,
                prepared
                    .ok_or_else(|| {
                        TrialError::Api(ApiError::internal(
                            "the agent preview has no fresh preparation",
                        ))
                    })?
                    .as_ref()
                    .map_err(|error| TrialError::Api(error.clone()))?,
            )
            .await
            .map(Some)
            .map_err(|error| TrialError::Agent(Box::new(error))),
    }
}
