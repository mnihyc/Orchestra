//! The complete authorized hierarchy for every signed-in person, with a whole-system scope for administrators.

use base64::Engine;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::Page;

use orch_api_contract::everything::EVERYTHING_LIMIT_DEFAULT;

use orch_api_contract::everything::EVERYTHING_LIMIT_MAX;

use orch_api_contract::everything::EverythingAccess;

use orch_api_contract::everything::EverythingDirection;

use orch_api_contract::everything::EverythingGroup;

use orch_api_contract::everything::EverythingKind;

use orch_api_contract::everything::EverythingList;

use orch_api_contract::everything::EverythingListQuery;

use orch_api_contract::everything::EverythingRecent;

use orch_api_contract::everything::EverythingRow;

use orch_api_contract::everything::EverythingScope;

use orch_api_contract::everything::EverythingSort;

use orch_api_contract::everything::EverythingTakes;

use orch_api_contract::everything::ResourceCounts;

use orch_api_contract::everything::ResourcesSummaryGet;

use orch_api_contract::nodes::NodeKind;

use orch_api_contract::nodes::NodeRef;

use orch_authz::everything::Projection;

use orch_db::queries::everything as rows;

use orch_db::queries::everything::Cursor;

use orch_db::queries::everything::Grant;

use orch_db::queries::identity;

use orch_db::queries::settings;

use orch_settings::merge::Link;

use orch_settings::merge::OwnValues;

use orch_settings::merge::Rows;

use orch_settings::merge::Stack;

use orch_settings::merge::resolve_key;

use orch_settings_contract::registry;

use orch_settings_contract::registry::Scope;

use orch_settings_contract::registry::keys;

use orch_types::Id;

use orch_types::Time;

use serde::Deserialize;

use serde::Serialize;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;
/// Declarations count processors in thousandths of one core.
const CPU_MILLIS_PER_CORE: f64 = 1_000.0;

/// The scheduler's memory declarations are mebibytes; the inventory's sizes are bytes.
const BYTES_PER_MIB: i64 = 1 << 20;

/// The scheduler's storage declarations are gibibytes.
const BYTES_PER_GIB: i64 = 1 << 30;

/// Binds the reader's inventory.
pub fn mount(registry: &mut Registry) {
    registry.on_with_optional_operation(EverythingList, operation::list, list);
    registry.on_with_optional_operation(ResourcesSummaryGet, operation::summary, summary);
}
orch_api_base::__api_bind_c47513c681!();

async fn summary(ctx: Ctx, (): (), (): ()) -> ApiResult<ResourceCounts> {
    crate::key_operations::snapshot_read::run(ctx, summary_snapshot).await
}

async fn summary_snapshot(ctx: Ctx) -> ApiResult<ResourceCounts> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let answer = if operation::scoped(principal.caller()) {
        operation::read::<ResourceCounts>(&ctx, &mut tx, &principal).await?
    } else {
        let projection = orch_authz::everything::project(&mut tx, &principal, None)
            .await
            .map_err(ApiError::internal)?;
        summary_counts(rows::resource_counts(&mut tx, &projection.grants).await?)
    };
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(answer)
}

fn summary_counts(rows: impl IntoIterator<Item = (String, i64)>) -> ResourceCounts {
    let counts: std::collections::HashMap<_, _> = rows.into_iter().collect();
    let count = |kind: &str| counts.get(kind).copied().unwrap_or(0);
    ResourceCounts {
        providers: count("provider"),
        agents: count("agent"),
        machines: count("machine"),
        environments: count("environment"),
        templates: count("template"),
        hosts: count("host"),
        mcp: count("mcp_server"),
        mounts: count("mount"),
        skills: count("skill"),
    }
}

/// A cursor is kept with its filters so changing an order cannot silently skip rows.
#[derive(Serialize, Deserialize)]
struct Continuation {
    selection: EverythingListQuery,
    position: Cursor,
}

fn word<T: serde::de::DeserializeOwned>(value: &str) -> ApiResult<T> {
    serde_json::from_value(serde_json::Value::String(value.to_owned())).map_err(ApiError::internal)
}

fn selection(query: &EverythingListQuery) -> EverythingListQuery {
    EverythingListQuery {
        cursor: None,
        limit: None,
        ..query.clone()
    }
}

fn after(query: &EverythingListQuery) -> ApiResult<Option<Cursor>> {
    query
        .cursor
        .as_ref()
        .map(|cursor| {
            let bad = || ApiError::invalid("cursor", "Use the next page of this same view.");
            let bytes = URL_SAFE_NO_PAD.decode(cursor).map_err(|_| bad())?;
            let continued: Continuation = serde_json::from_slice(&bytes).map_err(|_| bad())?;
            if continued.selection != selection(query) {
                return Err(bad());
            }
            Ok(continued.position)
        })
        .transpose()
}

fn next(query: &EverythingListQuery, position: Cursor) -> ApiResult<String> {
    Ok(URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&Continuation {
            selection: selection(query),
            position,
        })
        .map_err(ApiError::internal)?,
    ))
}

fn sort_word(sort: EverythingSort) -> &'static str {
    match sort {
        EverythingSort::Name => "name",
        EverythingSort::Kind => "kind",
        EverythingSort::Created => "created",
        EverythingSort::CreatedBy => "created_by",
        EverythingSort::Owner => "owner",
        EverythingSort::LastActivity => "last_activity",
        EverythingSort::Opened => "opened",
        EverythingSort::Where => "where",
        EverythingSort::ActivityBy => "activity_by",
        EverythingSort::State => "state",
        EverythingSort::Size => "size",
        EverythingSort::MyAccess => "my_access",
        EverythingSort::Tags => "tags",
        EverythingSort::Load => "load",
        EverythingSort::Running => "running",
        EverythingSort::Use => "use",
    }
}

fn kinds(query: &EverythingListQuery) -> ApiResult<Vec<String>> {
    let mut kinds = Vec::new();
    for kind in query
        .kinds
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|kind| !kind.is_empty())
    {
        let parsed = serde_json::from_value::<EverythingKind>(serde_json::json!(kind))
            .map_err(|_| ApiError::invalid("kinds", "Choose one of the kinds in this view."))?;
        if parsed == EverythingKind::Group {
            return Err(ApiError::invalid(
                "kinds",
                "Choose a kind of place, thing or resource.",
            ));
        }
        kinds.push(kind.to_owned());
    }
    kinds.sort_unstable();
    kinds.dedup();
    Ok(kinds)
}

fn path(row: &rows::Row, projection: &Projection, grant: &Grant) -> ApiResult<Vec<NodeRef>> {
    let tail: Option<NodeRef> = row
        .path_tail
        .clone()
        .map(serde_json::from_value)
        .transpose()
        .map_err(ApiError::internal)?;
    row.path
        .iter()
        .map(|id| {
            if let Some(step) = projection.names.get(id) {
                Ok(NodeRef {
                    thread_display: None,
                    id: *id,
                    kind: word(step.kind.as_str())?,
                    name: step.name.clone(),
                    redacted: step.name.is_none(),
                })
            } else if id.to_string() == row.id {
                Ok(NodeRef {
                    thread_display: None,
                    id: *id,
                    kind: word(&row.kind)?,
                    name: grant.named.then(|| row.name.clone()),
                    redacted: !grant.named,
                })
            } else if let Some(tail) = tail.as_ref().filter(|tail| tail.id == *id) {
                Ok(tail.clone())
            } else {
                // Every ancestor was read by the shared graph. Absence fails closed.
                Ok(NodeRef {
                    thread_display: None,
                    id: *id,
                    kind: NodeKind::Org,
                    name: None,
                    redacted: true,
                })
            }
        })
        .collect()
}

fn settings_destination(row: &rows::Row) -> Option<String> {
    match row.kind.as_str() {
        "org" => Some(format!("/org/{}/settings/general", row.id)),
        "project" => Some(format!("/project/{}/settings", row.id)),
        "topic" => Some(format!("/topic/{}?tab=settings", row.id)),
        _ => None,
    }
}

fn setting_chain(row: &rows::Row, projection: &Projection) -> Vec<Link> {
    row.path
        .iter()
        .filter_map(|id| {
            let kind = projection
                .names
                .get(id)
                .map(|step| step.kind.as_str())
                .or_else(|| (id.to_string() == row.id).then_some(row.kind.as_str()))?;
            Some(Link {
                id: *id,
                scope: match kind {
                    "org" => Scope::Org,
                    "project" => Scope::Project,
                    "topic" => Scope::Topic,
                    "thread" => Scope::Thread,
                    _ => return None,
                },
            })
        })
        .collect()
}

fn may_share(row: &rows::Row, projection: &Projection) -> ApiResult<bool> {
    let Some(def) = registry::get(keys::SHARING_ALLOW_HOLDERS) else {
        return Ok(false);
    };
    let stored: Vec<settings::SettingRow> =
        serde_json::from_value(row.sharing_rows.clone()).map_err(ApiError::internal)?;
    let settings = Rows::new(stored);
    let chain = setting_chain(row, projection);
    Ok(resolve_key(
        def,
        &Stack {
            rows: &settings,
            chain: &chain,
            own: None,
        },
        None,
    )
    .value
    .and_then(|value| value.as_bool())
    .unwrap_or(false))
}

#[derive(Deserialize)]
struct Sizing {
    spec: orch_types::Json,
    config: orch_types::Json,
    revision: i64,
}

fn requirements(row: &rows::Row, projection: &Projection) -> ApiResult<Option<EverythingTakes>> {
    let Some(value) = row.takes.clone() else {
        return Ok(None);
    };
    let data: Sizing = serde_json::from_value(value).map_err(ApiError::internal)?;
    let stored: Vec<settings::SettingRow> =
        serde_json::from_value(row.sharing_rows.clone()).map_err(ApiError::internal)?;
    let settings = Rows::new(stored);
    let chain = setting_chain(row, projection);
    let own = OwnValues {
        scope: Scope::Environment,
        values: data.config,
        revision: data.revision,
    };
    let stack = Stack {
        rows: &settings,
        chain: &chain,
        own: Some(&own),
    };
    let resolve = |key| registry::get(key).and_then(|def| resolve_key(def, &stack, None).value);
    let limits = orch_engine::places::limits_of(&data.spec, resolve(keys::ENV_RESOURCES).as_ref());
    let needs =
        orch_engine::places::needs_of(&data.spec, resolve(keys::ENV_NEEDS).as_ref(), Some(&limits));
    // Zero is the scheduler's undeclared fallback, not a measurement of no use.
    let result = EverythingTakes {
        cpus: (needs.cpu_millis > 0).then_some(needs.cpu_millis as f64 / CPU_MILLIS_PER_CORE),
        memory_bytes: (needs.memory_mb > 0)
            .then(|| needs.memory_mb.checked_mul(BYTES_PER_MIB))
            .flatten(),
        storage_bytes: (needs.disk_gb > 0)
            .then(|| needs.disk_gb.checked_mul(BYTES_PER_GIB))
            .flatten(),
    };
    Ok(
        (result.cpus.is_some() || result.memory_bytes.is_some() || result.storage_bytes.is_some())
            .then_some(result),
    )
}

fn person(value: Option<orch_types::Json>) -> ApiResult<Option<orch_api_contract::users::UserRef>> {
    value
        .map(|value| {
            serde_json::from_value::<identity::UserBrief>(value)
                .map(|brief| refusals::user_ref(&brief))
                .map_err(ApiError::internal)
        })
        .transpose()
}

#[derive(Clone)]
struct ListRows {
    found: Vec<rows::Row>,
    grants: Vec<Grant>,
    continuation: Option<String>,
    kinds: Vec<String>,
    details: bool,
    rows: bool,
    facts: Vec<(orch_access_contract::key_scope::KeyTargetKind, Id)>,
    grouping: grouping::Prepared,
}

#[derive(Clone)]
struct ListSelection {
    limit: u32,
    kinds: Vec<String>,
    details: bool,
    rows: bool,
    parent: Option<String>,
    access_parent: Option<Id>,
    after: Option<Cursor>,
    system: bool,
}

fn list_selection(query: &EverythingListQuery) -> ApiResult<ListSelection> {
    let limit = query.limit.unwrap_or(EVERYTHING_LIMIT_DEFAULT);
    if limit == 0 || limit > EVERYTHING_LIMIT_MAX {
        return Err(ApiError::invalid("limit", "Choose between 1 and 200 rows."));
    }
    if let Some(parent) = query.parent.as_deref()
        && parent != "shared"
        && parent != "resources"
        && parent.parse::<Id>().is_err()
    {
        return Err(ApiError::invalid(
            "parent",
            "Choose a place or group in this view.",
        ));
    }
    if query
        .created_from
        .zip(query.created_to)
        .is_some_and(|(from, to)| from > to)
    {
        return Err(ApiError::invalid(
            "created_to",
            "The end date must come after the start date.",
        ));
    }
    let kinds = kinds(query)?;
    let rows = query.resource_rows.unwrap_or(false);
    let details = query.resource_details.unwrap_or(false) || rows;
    let resource_filter = details
        || query.resource_state.is_some()
        || query.harness.is_some()
        || query.resource_kind.is_some()
        || query.resource_view.is_some()
        || query.home.is_some();
    if resource_filter
        && (kinds.len() != 1
            || !matches!(
                kinds[0].as_str(),
                "agent"
                    | "provider"
                    | "host"
                    | "environment"
                    | "machine"
                    | "template"
                    | "mcp_server"
                    | "mount"
                    | "skill"
            ))
    {
        return Err(ApiError::invalid(
            "kinds",
            "Choose one resource kind for these filters.",
        ));
    }
    if details && query.parent.as_deref() != Some("resources") {
        return Err(ApiError::invalid(
            "parent",
            "Open the resource list to request its columns.",
        ));
    }
    library::validate(query, &kinds)?;
    if query.harness.is_some() && kinds.first().map(String::as_str) != Some("agent") {
        return Err(ApiError::invalid(
            "harness",
            "Choose an agent list to filter its harness.",
        ));
    }
    if query.resource_kind.is_some()
        && !matches!(
            kinds.first().map(String::as_str),
            Some("template" | "environment")
        )
    {
        return Err(ApiError::invalid(
            "resource_kind",
            "Choose a template or environment list for this filter.",
        ));
    }
    if matches!(
        query.sort,
        Some(EverythingSort::Load | EverythingSort::Running)
    ) && kinds.as_slice() != ["host"]
    {
        return Err(ApiError::invalid(
            "sort",
            "Load and running order apply to hosts.",
        ));
    }
    let parent = query.parent.as_ref().map(|parent| {
        parent
            .parse::<Id>()
            .map_or_else(|_| parent.clone(), |id| id.to_string())
    });
    // Unfiltered expansions prepare just this subtree; cross-level filters and the two groups need the full inventory.
    let local = kinds.is_empty()
        && query.owner.is_none()
        && query.created_from.is_none()
        && query.created_to.is_none()
        && query.active_from.is_none()
        && query.q.as_deref().unwrap_or_default().trim().is_empty();
    let access_parent = if local {
        parent
            .as_deref()
            .and_then(|parent| parent.parse::<Id>().ok())
    } else {
        None
    };
    let after = after(query)?;
    Ok(ListSelection {
        limit,
        kinds,
        details,
        rows,
        parent,
        access_parent,
        after,
        system: query.scope.unwrap_or_default() == EverythingScope::System,
    })
}

fn system_scope(principal: &orch_authz::Principal, system: bool) -> ApiResult<()> {
    if system && !principal.is_system_admin() {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "Only a system administrator may open the whole system.",
        ));
    }
    Ok(())
}

async fn list(ctx: Ctx, query: EverythingListQuery, (): ()) -> ApiResult<Page<EverythingRow>> {
    // Keep the original validation before opening a database snapshot.
    let selected = list_selection(&query)?;
    crate::key_operations::snapshot_read::run(ctx, move |ctx| {
        list_snapshot(ctx, query.clone(), selected.clone())
    })
    .await
}

async fn list_snapshot(
    ctx: Ctx,
    query: EverythingListQuery,
    selected: ListSelection,
) -> ApiResult<Page<EverythingRow>> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    system_scope(&principal, selected.system)?;
    let page = if operation::scoped(principal.caller()) {
        operation::list_read(&ctx, &mut tx, &principal).await?
    } else {
        let mut read_ctx = ctx.clone();
        read_ctx.now = principal.now().get();
        let mut projection =
            orch_authz::everything::project(&mut tx, &principal, selected.access_parent)
                .await
                .map_err(ApiError::internal)?;
        let mut rows = list_rows(
            &read_ctx,
            &mut tx,
            &principal,
            &query,
            &selected,
            &mut projection,
        )
        .await?;
        rows.grouping = grouping::prepare(
            &read_ctx,
            &mut tx,
            &principal,
            &rows.found,
            &rows.grants,
            None,
        )
        .await?;
        render_rows(
            &read_ctx,
            &mut tx,
            &principal,
            rows,
            &projection,
            PreparedViews::default(),
        )
        .await?
    };
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(page)
}

fn row_fact(
    row: &rows::Row,
    grant: &Grant,
) -> ApiResult<Option<(orch_access_contract::key_scope::KeyTargetKind, Id)>> {
    if row.kind == "group" || !(grant.view || grant.listed || grant.named) {
        return Ok(None);
    }
    let family = if row.kind == "chat" {
        orch_access_contract::key_scope::KeyTargetKind::Identity(
            orch_access_contract::key_scope::KeyIdentityKind::Chat,
        )
    } else {
        orch_access_contract::key_scope::KeyTargetKind::Object(
            crate::routes::access::subject::kind_of_word(&row.kind).ok_or_else(|| {
                ApiError::internal("the native inventory row has an unknown family")
            })?,
        )
    };
    Ok(Some((family, row.id.parse().map_err(ApiError::internal)?)))
}

async fn list_rows(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    query: &EverythingListQuery,
    selected: &ListSelection,
    projection: &mut Projection,
) -> ApiResult<ListRows> {
    let limit = selected.limit;
    let kinds = &selected.kinds;
    let details = selected.details;
    let thin_rows = selected.rows;
    let parent = &selected.parent;
    let after = &selected.after;
    let system = selected.system;
    resources::prepare(conn, query, projection).await?;
    library::prepare(conn, principal, query, projection, Time::new(ctx.now)).await?;
    let mut found = rows::level(
        conn,
        &rows::Query {
            navigation: rows::NavigationFilter {
                recent: query.recent.map(|recent| match recent {
                    EverythingRecent::Opened => "opened",
                    EverythingRecent::Active => "active",
                }),
                enabled: query.navigation.unwrap_or(false),
                show_archived: query.show_archived.unwrap_or(true),
                hide_done: query.hide_done.unwrap_or(false),
                keep: query.keep,
            },
            resource: rows::ResourceFilter {
                state: query.resource_state.as_deref(),
                harness: query.harness.as_deref(),
                kind: query.resource_kind.as_deref(),
                provider_scope: query.resource_view.is_some(),
                home: query.home,
                details,
            },
            admin_access: query.admin_access,
            participating: query.participating.unwrap_or(false),
            user: principal.user(),
            groups: principal.groups(),
            member_roots: &projection.member_roots,
            grants: &projection.grants,
            parent: parent.as_deref(),
            system,
            text: query.q.as_deref().unwrap_or_default().trim(),
            kinds,
            owner: query.owner,
            created_from: query.created_from.map(Time::get),
            created_to: query.created_to.map(Time::get),
            active_from: query.active_from.map(Time::get),
            sort: sort_word(query.sort.unwrap_or(match query.recent {
                Some(EverythingRecent::Opened) => EverythingSort::Opened,
                Some(EverythingRecent::Active) => EverythingSort::LastActivity,
                None => EverythingSort::Name,
            })),
            descending: query.direction.unwrap_or(if query.recent.is_some() {
                EverythingDirection::Desc
            } else {
                EverythingDirection::Asc
            }) == EverythingDirection::Desc,
            after: after.as_ref(),
            limit: i64::from(limit) + 1,
            at: ctx.now,
        },
    )
    .await?;
    let more = found.len() > limit as usize;
    let lookahead = if operation::scoped(principal.caller()) {
        found
            .get(limit as usize)
            .map(|row| {
                let grant =
                    serde_json::from_value(row.grant.clone()).map_err(ApiError::internal)?;
                row_fact(row, &grant)
            })
            .transpose()?
            .flatten()
    } else {
        None
    };
    found.truncate(limit as usize);
    let continuation = if more {
        found
            .last()
            .map(|row| next(query, row.cursor()))
            .transpose()?
    } else {
        None
    };
    let grants: Vec<Grant> = found
        .iter()
        .map(|row| serde_json::from_value(row.grant.clone()).map_err(ApiError::internal))
        .collect::<ApiResult<_>>()?;
    let mut facts = found
        .iter()
        .zip(&grants)
        .filter_map(|(row, grant)| row_fact(row, grant).transpose())
        .collect::<ApiResult<Vec<_>>>()?;
    facts.extend(lookahead);
    if operation::scoped(principal.caller()) {
        let mut names = super::nodes::read_sources::Sources::default();
        for (row, grant) in found.iter().zip(&grants) {
            for node in path(row, projection, grant)? {
                names.node(&node);
            }
            if grant.view {
                let tags: Vec<orch_api_contract::nodes::Tag> =
                    serde_json::from_value(row.tags.clone()).map_err(ApiError::internal)?;
                for tag in tags {
                    names.node(&tag.node);
                }
            }
        }
        facts.extend(names.objects().map(|object| {
            (
                orch_access_contract::key_scope::KeyTargetKind::Object(object.kind),
                object.id,
            )
        }));
    }
    facts.sort_unstable();
    facts.dedup();
    Ok(ListRows {
        found,
        grants,
        continuation,
        kinds: kinds.clone(),
        details,
        rows: thin_rows,
        facts,
        grouping: grouping::Prepared::default(),
    })
}

#[derive(Default)]
struct PreparedViews<'a> {
    resources: Option<&'a resources::scoped::Prepared>,
    library: Option<&'a library::Prepared>,
    sharing: Option<&'a std::collections::HashMap<Id, resources::PreparedSharing>>,
}

async fn render_rows(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    rows: ListRows,
    projection: &Projection,
    prepared: PreparedViews<'_>,
) -> ApiResult<Page<EverythingRow>> {
    let ListRows {
        found,
        grants,
        continuation,
        kinds,
        details,
        rows: thin,
        facts: _,
        grouping,
    } = rows;
    let mut resource_views = if details {
        match prepared.resources {
            Some(prepared) => prepared.render(ctx, conn, principal).await?,
            None => {
                resources::page(ctx, conn, principal, &kinds[0], &found, &grants, projection)
                    .await?
            }
        }
    } else {
        std::collections::HashMap::new()
    };
    let mut library_facts = if details {
        match prepared.library {
            Some(prepared) => prepared.render(),
            None => library::page(conn, principal, &found, &grants, Time::new(ctx.now)).await?,
        }
    } else {
        std::collections::HashMap::new()
    };
    let library_threads = if details && kinds[0] == "provider" {
        library::threads(
            conn,
            &found
                .iter()
                .filter_map(|row| row.id.parse().ok())
                .collect::<Vec<_>>(),
            Time::new(ctx.now),
        )
        .await?
    } else {
        std::collections::HashMap::new()
    };
    let marker_places: std::collections::HashMap<_, _> = found
        .iter()
        .zip(&grants)
        .filter_map(|(row, grant)| {
            grant
                .admin_access
                .map(|place| ((row.kind.clone(), row.id.clone()), place))
        })
        .collect();
    let markers =
        crate::routes::nodes::view::named_admin_access(conn, principal, &marker_places).await?;
    let display_ids: Vec<Id> = found
        .iter()
        .zip(&grants)
        .filter(|(row, grant)| row.kind == "thread" && grant.view)
        .filter_map(|(row, _)| row.id.parse().ok())
        .collect();
    let displays = orch_engine::views::thread_displays(
        conn,
        &display_ids,
        Some(principal.user()),
        principal.now(),
    )
    .await
    .map_err(ApiError::internal)?;
    let mut shared_homes = std::collections::HashSet::new();
    let mut items = Vec::with_capacity(found.len());
    for (row, grant) in found.into_iter().zip(grants) {
        let group = match row.id.as_str() {
            "shared" if row.kind == "group" => Some(EverythingGroup::Shared),
            "resources" if row.kind == "group" => Some(EverythingGroup::Resources),
            _ => None,
        };
        let path = path(&row, projection, &grant)?;
        let place_label = path
            .iter()
            .filter(|step| step.id.to_string() != row.id)
            .map(|step| step.name.as_deref().unwrap_or("…"))
            .collect::<Vec<_>>()
            .join(" / ");
        let place_label = if place_label.is_empty() {
            row.root_address.clone().unwrap_or_default()
        } else {
            place_label
        };
        let href = grant
            .view
            .then(|| {
                super::destinations::href(
                    &row.kind,
                    &row.id,
                    if row.kind == "chat" {
                        row.chat_node_id
                    } else {
                        row.parent_id.as_deref().and_then(|id| id.parse().ok())
                    },
                    row.org_id,
                    &path,
                    None,
                )
            })
            .flatten();
        let settings_href = grant.settings.then(|| settings_destination(&row)).flatten();
        let can_share = grant.view
            && row.kind != "machine"
            && if row.kind == "chat" {
                grant.share
            } else {
                grant.share || may_share(&row, projection)?
            };
        let takes = if grant.view {
            requirements(&row, projection)?
        } else {
            None
        };
        let group_metadata = if grant.view {
            grouping.resource(&row.kind, &row.id, &path)?
        } else {
            None
        };
        let resource = resource_views.remove(&row.id);
        let resource_row = if thin {
            resource.as_ref().map(resources::list_row)
        } else {
            None
        };
        let sharing = if thin
            && let Some(home) = group_metadata
                .as_ref()
                .and_then(|group| group.home.as_ref())
            && !home.redacted
            && home.kind != NodeKind::Thread
            && shared_homes.insert(home.id)
        {
            Some(
                resources::sharing(
                    ctx,
                    conn,
                    principal,
                    home.id,
                    &row.kind,
                    prepared.sharing.and_then(|sharing| sharing.get(&home.id)),
                )
                .await,
            )
        } else {
            None
        };
        items.push(EverythingRow {
            sharing,
            resource_row,
            grouping: group_metadata,
            library: library_facts.remove(&row.id),
            library_threads: row
                .id
                .parse()
                .ok()
                .and_then(|id| library_threads.get(&id).copied()),
            thread_display: row
                .id
                .parse::<Id>()
                .ok()
                .and_then(|id| displays.get(&id).cloned()),
            resource: if thin { None } else { resource },
            admin_access: markers.get(&(row.kind.clone(), row.id.clone())).cloned(),
            participating: grant.participating,
            kind: word(&row.kind)?,
            group,
            name: grant.named.then(|| row.name.clone()),
            place_label: (!place_label.is_empty()).then_some(place_label),
            has_children: row.has_children,
            parent_id: row.parent_id.clone(),
            path,
            created_at: grant
                .view
                .then_some(row.created_at)
                .flatten()
                .map(Time::new),
            created_by: if grant.view {
                person(row.creator_brief)?
            } else {
                None
            },
            owner: if grant.view {
                person(row.owner_brief)?
            } else {
                None
            },
            last_activity: grant
                .view
                .then_some(row.last_activity)
                .flatten()
                .map(Time::new),
            last_opened: grant
                .view
                .then_some(row.last_opened)
                .flatten()
                .map(Time::new),
            activity_by: if grant.view {
                person(row.activity_by_brief)?
            } else {
                None
            },
            document_unread: grant.view.then_some(row.document_unread).flatten(),
            state: if grant.view { row.state } else { None },
            size_bytes: if grant.view { row.size_bytes } else { None },
            takes,
            my_access: word::<EverythingAccess>(if grant.view {
                &grant.access
            } else {
                "path_only"
            })?,
            tags: if grant.view {
                serde_json::from_value(row.tags).map_err(ApiError::internal)?
            } else {
                Vec::new()
            },
            href,
            settings_href,
            can_share,
            id: row.id,
        });
    }
    Ok(Page {
        items,
        next_cursor: continuation,
    })
}

#[path = "everything/grouping.rs"]
pub mod grouping;

#[path = "everything/library.rs"]
pub mod library;

#[path = "everything/operation.rs"]
pub mod operation;

#[path = "everything/resources.rs"]
pub mod resources;
