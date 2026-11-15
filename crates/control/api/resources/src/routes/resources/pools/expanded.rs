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
        _: &mut Conn,
        _: &Principal,
        _: &Leaf,
        _: &[Hop],
    ) -> Result<Option<String>, EngineError> {
        Ok(Some("request_required".into()))
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
            let sources = facts(conn, id).await?;
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
            let source = Database::new(conn, &principal, Inspect { routes });
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
            for event in events {
                match event {
                    Event::Candidate { leaf, path, reason } => {
                        let mut readable = true;
                        for hop in path {
                            if !rule_readable(
                                conn,
                                &principal,
                                &MemberRule::Pool {
                                    id: hop.pool_id,
                                    kind: definition.kind,
                                },
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
                        readable = readable && rule_readable(conn, &principal, &rule).await?;
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
