//! One ordinary scoped read supplies backend candidates for every capability setting.

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_api_contract::common::Page;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::resources::CapabilityBackendCandidate;

use orch_api_contract::resources::CapabilityBackendQuery;

use orch_api_contract::resources::ProviderCapabilityBackendList;

use orch_api_contract::settings_sources as sources;

use orch_api_contract::settings_sources::CapabilityBackendSource as Source;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::resources;

use orch_provider_contract::capabilities::BackendRef;

use orch_provider_contract::capabilities::OfferingRef;

use orch_provider_contract::capabilities::Route;

use orch_types::Id;

use serde::Deserialize;

use serde::Serialize;

use std::collections::BTreeMap;
pub(crate) fn mount(registry: &mut Registry) {
    registry.on_snapshot_read_with_optional_operation(
        ProviderCapabilityBackendList,
        native::selected::<Choices>,
        native::selected_read::<Choices>,
    );
}

struct Choices;

#[derive(Clone)]
struct Ready {
    provider: Id,
    place: Id,
    page: Page<CapabilityBackendCandidate>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    provider: Id,
    place: Id,
    capability: String,
    query: String,
    lookup: Option<String>,
    kind: String,
    offering: String,
    route: String,
}

fn after(
    query: &CapabilityBackendQuery,
    provider: Id,
    capability: &str,
) -> ApiResult<Option<Cursor>> {
    query
        .page
        .cursor
        .as_deref()
        .map(|value| {
            let cursor: Cursor = serde_json::from_str(value)
                .map_err(|_| ApiError::invalid("cursor", "is not a cursor this list gave"))?;
            if cursor.provider != provider
                || cursor.place != query.place
                || cursor.capability != capability
                || cursor.query != query.q.as_deref().unwrap_or("").trim()
                || cursor.lookup != query.offering
                || !matches!(cursor.kind.as_str(), "model" | "service")
            {
                return Err(ApiError::invalid(
                    "cursor",
                    "belongs to a different backend selection",
                ));
            }
            Ok(cursor)
        })
        .transpose()
}

async fn facts(conn: &mut Conn, provider: Id, place: Id) -> ApiResult<Vec<native::Facts<Source>>> {
    let provider = targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Provider,
            id: provider,
        },
    )
    .await?;
    let place = targets::node_identity(conn, place).await?;
    Ok(vec![
        native::Facts::one(
            Source::Provider,
            KeyTargetKind::Object(ObjectKind::Provider),
            provider,
        ),
        native::Facts::one(Source::Place, place.object.kind, place),
    ])
}

async fn require(conn: &mut Conn, principal: &Principal, provider: Id, place: Id) -> ApiResult<()> {
    super::providers::provider_for(conn, principal, provider, Permission::ProviderView).await?;
    authorization::require(
        conn,
        principal,
        place,
        authorization::VIEW,
        orch_authz::Act::Use,
        orch_authz::Policy::default(),
    )
    .await
}

fn title(route: &Route) -> BTreeMap<String, String> {
    if route.text.is_empty() {
        BTreeMap::from([
            ("en".into(), route.title.clone()),
            ("zh-CN".into(), route.title.clone()),
        ])
    } else {
        route.text.clone()
    }
}

async fn program_label(
    conn: &mut Conn,
    provider: Id,
    capability: &str,
) -> ApiResult<BTreeMap<String, String>> {
    let Some(provider) = resources::thread_engine::provider(conn, provider).await? else {
        return Ok(BTreeMap::new());
    };
    let Some(agent) = provider.agent_id else {
        return Ok(BTreeMap::new());
    };
    let Some(agent) = resources::thread_engine::agent(conn, agent).await? else {
        return Ok(BTreeMap::new());
    };
    let Some(descriptor) =
        orch_engine::harnesses::descriptor_of(&agent).map_err(ApiError::internal)?
    else {
        return Ok(BTreeMap::new());
    };
    let program = descriptor.capability_program();
    Ok(program
        .access
        .get("native")
        .and_then(|access| access.native.get(capability))
        .and_then(|support| support.extensions.get("delegation_label"))
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(ApiError::internal)?
        .unwrap_or_else(|| {
            BTreeMap::from([
                ("en".into(), descriptor.harness.name.clone()),
                ("zh-CN".into(), descriptor.harness.name),
            ])
        }))
}

async fn reason_texts(
    conn: &mut Conn,
    provider: Id,
) -> ApiResult<BTreeMap<String, BTreeMap<String, String>>> {
    let Some(provider) = resources::thread_engine::provider(conn, provider).await? else {
        return Ok(BTreeMap::new());
    };
    let Some(agent) = provider.agent_id else {
        return Ok(BTreeMap::new());
    };
    let Some(agent) = resources::thread_engine::agent(conn, agent).await? else {
        return Ok(BTreeMap::new());
    };
    let Some(descriptor) =
        orch_engine::harnesses::descriptor_of(&agent).map_err(ApiError::internal)?
    else {
        return Ok(BTreeMap::new());
    };
    let program = descriptor.capability_program();
    Ok(program
        .facts
        .iter()
        .chain(
            program
                .access
                .get("native")
                .into_iter()
                .flat_map(|access| &access.facts),
        )
        .map(|fact| (fact.id.clone(), fact.text.clone()))
        .collect())
}

impl native::Selected for Choices {
    type Source = Source;
    type Query = CapabilityBackendQuery;
    type Value = Ready;
    type Reply = Page<CapabilityBackendCandidate>;
    const DECLARATION: &'static Declaration<Source> = &sources::CAPABILITY_BACKENDS;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let provider = ctx.id("id")?;
            let declared = facts(conn, provider, query.place).await?;
            let plan = native::bind_required(ctx, Self::DECLARATION, &declared)?;
            let principal = plan.preparation_principal(ctx, conn, caller).await?;
            require(conn, &principal, provider, query.place).await?;
            let declarations = ctx.app.builtin_sources.capabilities().declarations();
            let capability = declarations
                .capabilities
                .get(&query.capability)
                .filter(|cap| cap.backend_setting.is_some())
                .map(|_| query.capability.clone())
                .ok_or_else(|| {
                    ApiError::invalid("capability", "does not declare a backend setting")
                })?;
            let after = after(query, provider, &capability)?;
            let limit = usize::try_from(query.page.limit()).map_err(ApiError::internal)?;
            let search = query.q.as_deref().unwrap_or("").trim();
            let row = resources::routes::providers(conn, &[provider])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            let routes: Vec<_> = declarations
                .routes
                .iter()
                .filter(|(_, route)| route.uses_backend && route.capabilities.contains(&capability))
                .collect();
            let model_routes: Vec<_> = routes
                .iter()
                .filter(|(_, route)| match route.executor.as_deref() {
                    Some("program-session") => row.kind == "subscription",
                    Some("model-backed") => row.kind == "api",
                    _ => false,
                })
                .copied()
                .collect();
            let native_label = program_label(conn, provider, &capability).await?;
            let static_reasons = reason_texts(conn, provider).await?;
            let mut candidates = Vec::new();
            if after.as_ref().is_none_or(|cursor| cursor.kind == "model")
                && !model_routes.is_empty()
            {
                let lower = after.as_ref().map(|cursor| cursor.offering.as_str());
                let names:Vec<(String,String)>=sqlx::query_as("SELECT model_name,display_name FROM provider_models WHERE provider_id=$1 AND (enabled OR $5::text IS NOT NULL) AND ($5::text IS NULL OR model_name=$5) AND ($2::text IS NULL OR model_name >= $2) AND ($4='' OR model_name ILIKE '%'||$4||'%' OR display_name ILIKE '%'||$4||'%') ORDER BY model_name LIMIT $3")
                    .bind(provider).bind(lower).bind(i64::try_from(limit.saturating_add(2)).map_err(ApiError::internal)?)
                    .bind(search).bind(query.offering.as_deref()).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
                for (offering, display) in names {
                    for (id, route) in &model_routes {
                        if after.as_ref().is_some_and(|cursor| {
                            cursor.kind == "model"
                                && offering == cursor.offering
                                && id.as_str() <= cursor.route.as_str()
                        }) {
                            continue;
                        }
                        let backend = BackendRef {
                            route: (*id).clone(),
                            offering: Some(OfferingRef {
                                provider,
                                offering: offering.clone(),
                            }),
                            ..BackendRef::default()
                        };
                        let availability = ctx
                            .app
                            .engine
                            .review_capability_backend(
                                conn,
                                &principal,
                                query.place,
                                &capability,
                                backend.clone(),
                            )
                            .await
                            .map_err(ApiError::internal)?;
                        candidates.push((
                            "model",
                            CapabilityBackendCandidate {
                                backend,
                                offering_name: if display.is_empty() {
                                    offering.clone()
                                } else {
                                    display.clone()
                                },
                                route_text: title(route),
                                program_text: if route.executor.as_deref()
                                    == Some("program-session")
                                {
                                    native_label.clone()
                                } else {
                                    BTreeMap::new()
                                },
                                eligible: availability.allowed,
                                reason_text: availability
                                    .reason
                                    .as_ref()
                                    .and_then(|reason| static_reasons.get(reason))
                                    .cloned()
                                    .unwrap_or_default(),
                                reason: availability.reason,
                            },
                        ));
                        if candidates.len() > limit {
                            break;
                        }
                    }
                    if candidates.len() > limit {
                        break;
                    }
                }
            }
            if candidates.len() <= limit && row.kind == "api" {
                let offered = super::provider_services::views(
                    &ctx.app.builtin_sources,
                    &row.upstreams,
                    &serde_json::from_value(row.service_offerings).map_err(ApiError::internal)?,
                )?;
                for offering in offered.into_iter().filter(|offering| {
                    (offering.enabled || query.offering.as_deref() == Some(offering.id.as_str()))
                        && offering.capabilities.contains(&capability)
                        && query
                            .offering
                            .as_deref()
                            .is_none_or(|name| name == offering.id)
                        && (search.is_empty()
                            || offering.id.to_lowercase().contains(&search.to_lowercase())
                            || offering
                                .title
                                .to_lowercase()
                                .contains(&search.to_lowercase()))
                }) {
                    for (id, route) in routes
                        .iter()
                        .filter(|(_, route)| route.executor.as_deref() == Some("api-backed"))
                    {
                        if after.as_ref().is_some_and(|cursor| {
                            cursor.kind == "service"
                                && (offering.id.as_str(), id.as_str())
                                    <= (cursor.offering.as_str(), cursor.route.as_str())
                        }) {
                            continue;
                        }
                        let backend = BackendRef {
                            route: (*id).clone(),
                            offering: Some(OfferingRef {
                                provider,
                                offering: offering.id.clone(),
                            }),
                            ..BackendRef::default()
                        };
                        let availability = ctx
                            .app
                            .engine
                            .review_capability_backend(
                                conn,
                                &principal,
                                query.place,
                                &capability,
                                backend.clone(),
                            )
                            .await
                            .map_err(ApiError::internal)?;
                        candidates.push((
                            "service",
                            CapabilityBackendCandidate {
                                backend,
                                offering_name: offering.title.clone(),
                                route_text: title(route),
                                program_text: BTreeMap::new(),
                                eligible: availability.allowed,
                                reason_text: availability
                                    .reason
                                    .as_ref()
                                    .and_then(|reason| static_reasons.get(reason))
                                    .cloned()
                                    .unwrap_or_default(),
                                reason: availability.reason,
                            },
                        ));
                        if candidates.len() > limit {
                            break;
                        }
                    }
                    if candidates.len() > limit {
                        break;
                    }
                }
            }
            let more = candidates.len() > limit;
            candidates.truncate(limit);
            let next_cursor = if more {
                candidates
                    .last()
                    .map(|(kind, candidate)| {
                        let offering = candidate
                            .backend
                            .offering
                            .as_ref()
                            .ok_or_else(ApiError::not_found)?;
                        serde_json::to_string(&Cursor {
                            provider,
                            place: query.place,
                            capability: capability.clone(),
                            query: search.into(),
                            lookup: query.offering.clone(),
                            kind: (*kind).into(),
                            offering: offering.offering.clone(),
                            route: candidate.backend.route.clone(),
                        })
                        .map_err(ApiError::internal)
                    })
                    .transpose()?
            } else {
                None
            };
            let page = Page {
                items: candidates
                    .into_iter()
                    .map(|(_, candidate)| candidate)
                    .collect(),
                next_cursor,
            };
            Ok((
                Ready {
                    provider,
                    place: query.place,
                    page,
                },
                declared,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            super::providers::provider_for(conn, principal, ctx.id("id")?, Permission::ProviderView)
                .await
                .map(|_| ())
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move {
            require(conn, principal, value.provider, value.place).await?;
            Ok(value.page.clone())
        })
    }
}
