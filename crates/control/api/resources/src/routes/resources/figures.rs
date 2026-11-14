//! Native library figures and current impact share factual roots and bounded dependency names.

use super::library;

use super::library::subjects;

use crate::key_operations::native;

use crate::key_operations::snapshot_read::engine_error;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationPlan;

use crate::routes::nodes::read_sources::Sources;

use crate::routes::nodes::view as node_view;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::library::LibraryEntry;

use orch_api_contract::library::LibraryReadBody;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::operations::FacetUsage;

use orch_api_contract::resource_facts::DependentRelation;

use orch_api_contract::resource_facts::FactSubject;

use orch_api_contract::resource_facts::FactTarget;

use orch_api_contract::resource_facts::ResourceImpact;

use orch_api_contract::resource_facts::ResourceUseKind;

use orch_api_contract::resource_facts_sources as sources;

use orch_api_contract::resource_facts_sources::Source;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::DbError;

use orch_engine::resource_facts as facts;

use orch_engine::resource_facts::PreparedVisibility;

use orch_engine::resource_facts::RawFacts;

use orch_engine::resource_facts::UseRequest;

use orch_engine::resource_facts::activity;

use orch_engine::resource_facts::library::RecordedUses;

use orch_types::Id;

use orch_types::Time;

use std::collections::BTreeMap;

use std::collections::BTreeSet;

use std::collections::HashMap;

use std::collections::HashSet;
pub(super) struct Library;

pub(super) struct Impact;

pub(super) struct Figures {
    targets: LibraryReadBody,
    raw: Vec<RawFacts>,
    visibility: PreparedVisibility,
    threads: HashMap<Id, i64>,
}

pub(super) struct CurrentImpact {
    target: FactTarget,
    name: String,
    raw: Vec<RawFacts>,
    visibility: PreparedVisibility,
    activity: activity::PreparedProjection,
    stopping: bool,
    displays: BTreeMap<Id, orch_api_contract::threads::ThreadDisplay>,
    notification: Option<super::notices::PreparedNotice>,
}

fn grouped(
    values: impl IntoIterator<Item = (KeyTargetKind, KeyResolvedTarget)>,
) -> Vec<(KeyTargetKind, Vec<KeyResolvedTarget>)> {
    let mut result = BTreeMap::new();
    for (kind, target) in values {
        result.entry(kind).or_insert_with(Vec::new).push(target);
    }
    result.into_iter().collect()
}

fn roots(
    values: Vec<subjects::Subject>,
    collection: bool,
) -> ApiResult<Vec<native::Facts<Source>>> {
    if !collection {
        let [value] = values.as_slice() else {
            return Err(ApiError::internal("the native impact root is not singular"));
        };
        return Ok(vec![native::Facts::one(
            Source::Requested,
            KeyTargetKind::Object(value.object.kind),
            value.target.clone(),
        )]);
    }
    let mut parents = BTreeMap::new();
    for value in &values {
        if let Some(parent) = &value.environment {
            let id = parent
                .object
                .id
                .ok_or_else(|| ApiError::internal("the native owned parent has no identity"))?;
            parents.insert(id, parent.clone());
        }
    }
    Ok(vec![
        native::Facts::collection(
            Source::Requested,
            grouped(
                values
                    .into_iter()
                    .map(|value| (KeyTargetKind::Object(value.object.kind), value.target)),
            ),
        ),
        native::Facts::collection(
            Source::OwnedEnvironments,
            grouped(
                parents
                    .into_values()
                    .map(|target| (KeyTargetKind::Object(ObjectKind::Environment), target)),
            ),
        ),
    ])
}

async fn named(
    conn: &mut Conn,
    caller: &Caller,
    plan: &OperationPlan,
    declaration: &'static Declaration<Source>,
    objects: impl IntoIterator<Item = ObjectRef>,
) -> ApiResult<(HashSet<(ObjectKind, Id)>, native::Facts<Source>)> {
    let targets = Sources::from_objects(objects)
        .identity_targets(conn)
        .await?;
    let mut allowed = HashSet::new();
    let mut selected = Vec::new();
    for (object, target) in targets {
        let business = KeyTargetKind::Object(object.kind);
        if !native::modern(caller)
            || native::permits_usage(
                plan,
                declaration,
                caller,
                &Source::Names,
                FacetUsage::ResponseContent,
                business,
                &target,
            )?
        {
            allowed.insert((object.kind, object.id));
            selected.push((business, target));
        }
    }
    Ok((
        allowed,
        native::Facts::collection(Source::Names, grouped(selected)),
    ))
}

type Membership = (
    ObjectRef,
    String,
    DependentRelation,
    Option<ObjectRef>,
    Option<ObjectRef>,
);

fn memberships(raw: &[RawFacts], visibility: &PreparedVisibility) -> HashSet<Membership> {
    visibility
        .named_edges(raw)
        .filter_map(|(subject, edge)| {
            let FactSubject::Resource { object } = subject else {
                return None;
            };
            Some((
                *object,
                edge.key.clone(),
                edge.relation,
                edge.visibility,
                edge.object,
            ))
        })
        .collect()
}

async fn library_raw(
    conn: &mut Conn,
    body: &LibraryReadBody,
    at: Time,
) -> ApiResult<Vec<RawFacts>> {
    let mut seen = HashSet::new();
    let mut subjects = Vec::new();
    let mut requested = Vec::new();
    for target in &body.targets {
        if let Some(version) = target.version_id {
            requested.push(UseRequest {
                object: target.object,
                version_id: Some(version),
                kind: ResourceUseKind::SkillInvocation,
            });
        } else if seen.insert(target.object) {
            subjects.push(FactSubject::Resource {
                object: target.object,
            });
        }
    }
    let parents: HashMap<_, _> = facts::read_with(conn, &subjects, at, Some(&RecordedUses))
        .await
        .map_err(engine_error)?
        .into_iter()
        .filter_map(|row| match &row.subject {
            FactSubject::Resource { object } => Some((*object, row)),
            _ => None,
        })
        .collect();
    let versions: HashMap<_, _> = facts::reported_uses(conn, &requested, at, Some(&RecordedUses))
        .await
        .map_err(engine_error)?
        .into_iter()
        .map(|(request, uses)| ((request.object, request.version_id), uses))
        .collect();
    let mut raw =
        body.targets
            .iter()
            .map(|target| {
                if target.version_id.is_some() {
                    Ok(RawFacts {
                        subject: FactSubject::Resource {
                            object: target.object,
                        },
                        as_of: at,
                        dependents: Vec::new(),
                        dependents_complete: false,
                        uses: versions.get(&(target.object, target.version_id)).cloned(),
                    })
                } else {
                    parents.get(&target.object).cloned().ok_or_else(|| {
                        ApiError::internal("the native resource figures were not read")
                    })
                }
            })
            .collect::<ApiResult<Vec<_>>>()?;
    crate::routes::everything::library::select_relations(&mut raw);
    Ok(raw)
}

impl native::Selected for Library {
    type Source = Source;
    type Query = ((), LibraryReadBody);
    type Value = Figures;
    type Reply = Vec<LibraryEntry>;
    const DECLARATION: &'static Declaration<Source> = &sources::LIBRARY;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (_, body): &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Figures, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            if body.targets.len()
                > usize::try_from(orch_api_contract::common::MAX_LIMIT)
                    .map_err(ApiError::internal)?
            {
                return Err(ApiError::invalid(
                    "targets",
                    "Read resource figures one page at a time.",
                ));
            }
            let objects: Vec<_> = body.targets.iter().map(|target| target.object).collect();
            let mut bound = roots(subjects::select(conn, &objects).await?, true)?;
            let known = native::bind_known(ctx, &sources::LIBRARY, &bound)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            library::require_targets(conn, &principal, &body.targets).await?;
            let raw = library_raw(conn, body, principal.now()).await?;
            let mut visibility =
                facts::prepare_projection(conn, &principal, Policy::default(), &raw)
                    .await
                    .map_err(engine_error)?;
            let (allowed, names) = named(
                conn,
                caller,
                &known,
                &sources::LIBRARY,
                visibility.sources(),
            )
            .await?;
            visibility.restrict_to(&allowed);
            bound.push(names);
            let providers: Vec<_> = body
                .targets
                .iter()
                .filter(|target| target.object.kind == ObjectKind::Provider)
                .map(|target| target.object.id)
                .collect();
            let threads =
                crate::routes::everything::library::threads(conn, &providers, principal.now())
                    .await?;
            Ok((
                Figures {
                    targets: body.clone(),
                    raw,
                    visibility,
                    threads,
                },
                bound,
            ))
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
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Figures,
    ) -> BoxFuture<'a, ApiResult<Vec<LibraryEntry>>> {
        Box::pin(async move {
            library::require_targets(conn, principal, &value.targets.targets).await?;
            let projected = value.visibility.project(&value.raw);
            Ok(value
                .targets
                .targets
                .iter()
                .cloned()
                .zip(projected)
                .map(|(target, facts)| {
                    let threads = value.threads.get(&target.object.id).copied();
                    LibraryEntry {
                        target,
                        facts,
                        threads,
                    }
                })
                .collect())
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a Self::Query,
        opened: &'a Figures,
        fresh: &'a Figures,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.targets == fresh.targets
                && memberships(&opened.raw, &opened.visibility)
                    .is_subset(&memberships(&fresh.raw, &fresh.visibility)))
        })
    }
}

async fn require_impact(
    conn: &mut Conn,
    principal: &Principal,
    target: &FactTarget,
) -> ApiResult<String> {
    if target.object.kind == ObjectKind::McpServer {
        let row = orch_db::queries::resources::mcp::servers(conn, &[target.object.id])
            .await?
            .pop()
            .ok_or_else(ApiError::not_found)?;
        super::mcp::require_row(conn, principal, &row, false).await?;
        return Ok(row.name);
    }
    let source = facts::authorities(conn, principal, &[target.object.id])
        .await
        .map_err(engine_error)?
        .into_iter()
        .find(|row| row.resource.object() == target.object && (row.readable || row.manageable))
        .ok_or_else(ApiError::not_found)?;
    if let Some(model) = target.model_id {
        sqlx::query_scalar("SELECT COALESCE(NULLIF(display_name,''),model_name) FROM provider_models WHERE provider_id=$1 AND id=$2")
            .bind(target.object.id).bind(model).fetch_optional(&mut *conn).await.map_err(DbError::from)?.ok_or_else(ApiError::not_found)
    } else {
        Ok(source.resource.name)
    }
}

type WorkMembership = (String, Id, Id, Option<Id>, Option<Id>);

fn work_memberships(value: &activity::PreparedProjection) -> HashSet<WorkMembership> {
    value
        .named_work()
        .map(|row| {
            (
                row.kind.clone(),
                row.id,
                row.thread,
                row.segment,
                row.author,
            )
        })
        .collect()
}

impl native::Selected for Impact {
    type Source = Source;
    type Query = ((), FactTarget);
    type Value = CurrentImpact;
    type Reply = ResourceImpact;
    const DECLARATION: &'static Declaration<Source> = &sources::IMPACT;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (_, target): &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(CurrentImpact, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            if !matches!(
                target.object.kind,
                ObjectKind::Provider
                    | ObjectKind::Agent
                    | ObjectKind::Environment
                    | ObjectKind::McpServer
            ) || target.model_id.is_some() && target.object.kind != ObjectKind::Provider
            {
                return Err(ApiError::invalid(
                    "object",
                    "must be a provider, profile, environment, MCP server or a model of its provider",
                ));
            }
            let selected = subjects::select(conn, &[target.object]).await?;
            let parents = selected
                .iter()
                .filter_map(|subject| subject.environment.clone())
                .collect::<Vec<_>>();
            let mut bound = roots(selected, false)?;
            bound.push(native::Facts::collection(
                Source::OwnedEnvironments,
                if parents.is_empty() {
                    Vec::new()
                } else {
                    vec![(KeyTargetKind::Object(ObjectKind::Environment), parents)]
                },
            ));
            let known = native::bind_known(ctx, &sources::IMPACT, &bound)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let name = require_impact(conn, &principal, target).await?;
            let at = principal.now();
            let raw = vec![
                facts::target_dependents(conn, target, at)
                    .await
                    .map_err(engine_error)?,
            ];
            let mut visibility =
                facts::prepare_projection(conn, &principal, Policy::default(), &raw)
                    .await
                    .map_err(engine_error)?;
            let work = activity::read(conn, target).await.map_err(engine_error)?;
            let mut activity = activity::prepare_projection(conn, &principal, target, &work, at)
                .await
                .map_err(engine_error)?;
            let objects: Vec<_> = visibility
                .sources()
                .chain(activity.name_sources())
                .collect();
            let (allowed, names) = named(conn, caller, &known, &sources::IMPACT, objects).await?;
            visibility.restrict_to(&allowed);
            activity.bound_sources(&allowed);
            bound.push(names);
            let threads: BTreeSet<_> = visibility
                .named_edges(&raw)
                .filter(|(_, edge)| {
                    matches!(
                        edge.relation,
                        DependentRelation::Binding | DependentRelation::Attachment
                    )
                })
                .filter_map(|(_, edge)| {
                    edge.object
                        .filter(|object| object.kind == ObjectKind::Thread)
                        .map(|object| object.id)
                })
                .collect();
            let notification =
                super::notices::prepare(conn, &principal, target, &raw[0], &threads).await?;
            let displays = node_view::refs(
                conn,
                &principal,
                &threads.into_iter().collect::<Vec<_>>(),
                Policy::default(),
            )
            .await?
            .into_iter()
            .filter_map(|(id, row)| row.thread_display.map(|display| (id, *display)))
            .collect();
            let stopping = activity::stopping(conn, target)
                .await
                .map_err(engine_error)?;
            Ok((
                CurrentImpact {
                    target: target.clone(),
                    name,
                    raw,
                    visibility,
                    activity,
                    stopping,
                    displays,
                    notification,
                },
                bound,
            ))
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
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a CurrentImpact,
    ) -> BoxFuture<'a, ApiResult<ResourceImpact>> {
        Box::pin(async move {
            require_impact(conn, principal, &value.target).await?;
            let facts = value
                .visibility
                .project(&value.raw)
                .into_iter()
                .next()
                .ok_or_else(|| ApiError::internal("the native impact figures are missing"))?;
            Ok(ResourceImpact {
                target: value.target.clone(),
                name: value.name.clone(),
                facts,
                activity: value.activity.project(),
                stopping: value.stopping,
                thread_displays: value.displays.clone(),
                notification: match &value.notification {
                    Some(notification) => Some(notification.project(conn).await?),
                    None => None,
                },
            })
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _query: &'a Self::Query,
        opened: &'a CurrentImpact,
        fresh: &'a CurrentImpact,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            if opened.target != fresh.target
                || !memberships(&opened.raw, &opened.visibility)
                    .is_subset(&memberships(&fresh.raw, &fresh.visibility))
                || !work_memberships(&opened.activity).is_subset(&work_memberships(&fresh.activity))
            {
                return Ok(false);
            }
            if let Some(notification) = &opened.notification
                && !fresh
                    .notification
                    .as_ref()
                    .is_some_and(|fresh| notification.preserves(fresh))
            {
                return Ok(false);
            }
            let ids: Vec<_> = opened.displays.keys().copied().collect();
            let refs = node_view::refs(conn, principal, &ids, Policy::default()).await?;
            Ok(ids
                .iter()
                .all(|id| refs.get(id).is_some_and(|row| row.thread_display.is_some())))
        })
    }
}
