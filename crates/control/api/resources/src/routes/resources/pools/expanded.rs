//! A sealed, reader-bound view of the shared resolver; inspection reserves and executes nothing.
use super::*;
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use orch_engine::{
    EngineError,
    resource_choice::{
        self, Event, Hop, Resolver,
        database::{Database, Requirements},
    },
};
use orch_provider_contract::{
    capabilities::{BackendRef, OfferingRef},
    pools::{Leaf, Target},
};

pub(super) fn mount(registry: &mut Registry) {
    registry.on_snapshot_read_with_optional_operation(
        api::PoolExpandedGet,
        native::selected::<Expanded>,
        native::selected_read::<Expanded>,
    );
}
struct Expanded;
#[derive(PartialEq, Eq)]
struct Page {
    pool: Id,
    items: Vec<api::PoolExpandedMember>,
    next: Option<Json>,
}
struct Inspect {
    model_routes: std::collections::BTreeSet<String>,
    routes: Vec<(String, String)>,
}
#[async_trait]
impl Requirements for Inspect {
    fn inspect_only(&self) -> bool {
        true
    }
    async fn model(
        &mut self,
        conn: &mut Conn,
        provider: Id,
        model: &str,
    ) -> Result<Leaf, EngineError> {
        let kind: Option<String> =
            sqlx::query_scalar("SELECT kind FROM providers WHERE id=$1 AND deleted_at IS NULL")
                .bind(provider)
                .fetch_optional(conn)
                .await
                .map_err(DbError::from)?;
        let executor = if kind.as_deref() == Some("api") {
            "model-backed"
        } else {
            "program-session"
        };
        let route = self
            .routes
            .iter()
            .find(|(_, implementation)| implementation == executor)
            .map(|(route, _)| route.clone())
            .ok_or_else(|| {
                EngineError::unreadable("no installed model route is available for inspection")
            })?;
        Ok(Leaf::Model {
            backend: BackendRef {
                route,
                offering: Some(OfferingRef {
                    provider,
                    offering: model.into(),
                }),
                config: serde_json::json!({}),
                extensions: Default::default(),
            },
        })
    }
    async fn reason(
        &mut self,
        conn: &mut Conn,
        _: &Principal,
        leaf: &Leaf,
        _: &[Hop],
    ) -> Result<Option<String>, EngineError> {
        let (kind, id) = match leaf {
            Leaf::Model { backend } => {
                let Some(offering) = backend.offering.as_ref() else {
                    return Ok(Some("member_unavailable".into()));
                };
                if self.model_routes.contains(&backend.route) {
                    let enabled: Option<bool> = sqlx::query_scalar(
                    "SELECT enabled FROM provider_models WHERE provider_id=$1 AND model_name=$2",
                )
                .bind(offering.provider)
                .bind(&offering.offering)
                .fetch_optional(&mut *conn)
                .await
                .map_err(DbError::from)?;
                    match enabled {
                        None => return Ok(Some("member_unavailable".into())),
                        Some(false) => return Ok(Some("member_disabled".into())),
                        Some(true) => {}
                    }
                }
                ("provider", offering.provider)
            }
            Leaf::Provider { id } => ("provider", *id),
            Leaf::Environment { id } => ("environment", *id),
            Leaf::Host { id } => ("host", *id),
            Leaf::EnvironmentTemplate { .. } => return Ok(Some("request_required".into())),
        };
        let current = orch_db::queries::resources::resource_facts(conn, &[id])
            .await?
            .into_iter()
            .find(|row| row.kind == kind);
        Ok(Some(
            match current {
                None => "member_unavailable",
                Some(row) if row.deleted => "member_unavailable",
                Some(row) if row.disabled => "member_disabled",
                Some(_) => "request_required",
            }
            .into(),
        ))
    }
}
fn audience(principal: &Principal, pool: Id) -> Vec<u8> {
    let mut value = b"pool-members/read".to_vec();
    value.extend_from_slice(principal.user().as_bytes());
    value.extend_from_slice(pool.as_bytes());
    if let Some(token) = principal.caller().grant().and_then(|grant| grant.token()) {
        value.extend_from_slice(token.as_bytes());
    }
    value
}
fn open(principal: &Principal, pool: Id, value: &str) -> ApiResult<resource_choice::Checkpoint> {
    let bad = || invalid("cursor", "is not a continuation for this reader and pool");
    let bytes = URL_SAFE_NO_PAD.decode(value).map_err(|_| bad())?;
    let plain = crate::routes::resources::sealing::master_key()?
        .open(&bytes, &audience(principal, pool))
        .map_err(|_| bad())?;
    serde_json::from_slice(&plain).map_err(|_| bad())
}
impl native::Selected for Expanded {
    type Source = api::Source;
    type Query = api::PoolExpandedQuery;
    type Value = Page;
    type Reply = api::PoolExpandedReply;
    const DECLARATION: &'static Declaration<api::Source> = &api::EXPANDED;
    const WATCH_ORDINARY: bool = true;
    const MOVES: native::Moves = native::Moves::Repeat;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> native::SourceFuture<'a, Page, api::Source> {
        Box::pin(async move {
            let id = ctx.id("id")?;
            let mut sources = facts(conn, id).await?;
            let known = native::bind_known(ctx, Self::DECLARATION, &sources)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            require(
                conn,
                &principal,
                ResourceKind::Pool,
                id,
                Permission::PoolView,
            )
            .await?;
            let row = rows::get(conn, id).await?.ok_or_else(ApiError::not_found)?;
            let definition: Definition =
                serde_json::from_value(row.definition).map_err(ApiError::internal)?;
            let batch = orch_settings::get::<u64>(
                conn,
                orch_settings::Place::Pool(id),
                orch_settings_contract::registry::keys::POOLS_READ_BATCH_SIZE,
            )
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::internal("the pool read batch has no default"))?;
            let routes = ctx
                .app
                .builtin_sources
                .capabilities()
                .declarations()
                .routes
                .iter()
                .filter_map(|(id, route)| {
                    (route.uses_backend
                        && route
                            .capabilities
                            .iter()
                            .any(|capability| capability == "generation"))
                    .then(|| {
                        route
                            .executor
                            .as_ref()
                            .map(|executor| (id.clone(), executor.clone()))
                    })
                    .flatten()
                })
                .collect();
            let model_routes = ctx
                .app
                .builtin_sources
                .capabilities()
                .declarations()
                .routes
                .iter()
                .filter(|(_, route)| {
                    matches!(
                        route.executor.as_deref(),
                        Some("model-backed" | "program-session")
                    )
                })
                .map(|(id, _)| id.clone())
                .collect();
            let source = Database::new(
                conn,
                &principal,
                Inspect {
                    routes,
                    model_routes,
                },
            );
            let mut resolver = match &query.cursor {
                None => {
                    Resolver::new(
                        source,
                        &Target::Pool {
                            id,
                            kind: definition.kind,
                        },
                        definition.kind,
                    )
                    .await
                }
                Some(cursor) => Resolver::resume(source, open(&principal, id, cursor)?).await,
            }
            .map_err(|error| invalid("cursor", &error.to_string()))?;
            let mut events = Vec::new();
            let mut finished = false;
            for _ in 0..batch {
                match resolver.next().await.map_err(ApiError::internal)? {
                    Some(event) => events.push(event),
                    None => {
                        finished = true;
                        break;
                    }
                }
            }
            let next = if finished {
                None
            } else {
                Some(serde_json::to_value(resolver.checkpoint()).map_err(ApiError::internal)?)
            };
            drop(resolver);
            let mut items = Vec::new();
            let mut member_sources = std::collections::BTreeMap::new();
            for event in events {
                match event {
                    Event::Candidate { leaf, path, reason } => {
                        let mut readable = true;
                        for hop in path {
                            if !scoped_rule(
                                conn,
                                caller,
                                &known,
                                &principal,
                                &MemberRule::Pool {
                                    id: hop.pool_id,
                                    kind: definition.kind,
                                },
                                &mut member_sources,
                            )
                            .await?
                            {
                                readable = false;
                                break;
                            }
                        }
                        let rule = MemberRule::Single {
                            resource: leaf.clone(),
                        };
                        readable = readable
                            && scoped_rule(
                                conn,
                                caller,
                                &known,
                                &principal,
                                &rule,
                                &mut member_sources,
                            )
                            .await?;
                        let label = if readable {
                            match &leaf {
                                Leaf::Model { backend } => backend
                                    .offering
                                    .as_ref()
                                    .map(|offering| offering.offering.clone()),
                                _ => rule_label(conn, &rule).await?,
                            }
                        } else {
                            None
                        };
                        items.push(api::PoolExpandedMember {
                            resource: readable.then_some(leaf),
                            label,
                            reason: if readable {
                                reason
                            } else {
                                Some("member_hidden".into())
                            },
                        });
                    }
                    Event::Skipped { reason, .. } => items.push(api::PoolExpandedMember {
                        resource: None,
                        label: None,
                        reason: Some(reason),
                    }),
                    Event::Enter { .. } | Event::Leave { .. } => {}
                }
            }
            sources.push(native::Facts::collection(
                api::Source::Members,
                member_sources.into_iter().collect(),
            ));
            Ok((
                Page {
                    pool: id,
                    items,
                    next,
                },
                sources,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require(
                conn,
                principal,
                ResourceKind::Pool,
                ctx.id("id")?,
                Permission::PoolView,
            )
            .await
        })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Page,
    ) -> BoxFuture<'a, ApiResult<api::PoolExpandedReply>> {
        Box::pin(async move {
            let next = value
                .next
                .as_ref()
                .map(|next| {
                    let plain = serde_json::to_vec(next).map_err(ApiError::internal)?;
                    let sealed = crate::routes::resources::sealing::master_key()?
                        .seal(&plain, &audience(principal, value.pool))
                        .map_err(ApiError::internal)?;
                    Ok::<String, ApiError>(URL_SAFE_NO_PAD.encode(sealed))
                })
                .transpose()?;
            Ok(api::PoolExpandedReply {
                items: value.items.clone(),
                next,
            })
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Self::Query,
        opened: &'a Page,
        fresh: &'a Page,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened == fresh) })
    }
}
