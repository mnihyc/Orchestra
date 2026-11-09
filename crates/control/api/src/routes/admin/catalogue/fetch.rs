//! One exact-version supplier and native claimant for both existing fetch doors.

use crate::key_operations::{
    native::{self, effects, reply},
    publication,
};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use futures::future::BoxFuture;
use orch_api_contract::admin::{
    HarnessBuild, HarnessBuildSource, HarnessBuildStatus, HarnessFetchReceipt,
    HarnessFetchResponse, HarnessVersionFetchBody,
};
use orch_api_contract::admin_sources::Source;
use orch_api_contract::common::{ErrorCode, ReceiptProjection};
use orch_api_contract::operation_sources::Declaration;
use orch_authz::{Caller, Principal};
use orch_db::{Conn, DbError};
use orch_engine::build_checks::{Claim, Destination, Request};
use std::{marker::PhantomData, sync::Arc};

pub(crate) trait Mode: Send + Sync + 'static {
    type Body: Clone + Send + 'static;
    const ADMIN: bool;
    fn destination(body: Self::Body) -> ApiResult<Destination>;
    fn declaration(destination: &Destination) -> &'static Declaration<Source>;
}
pub(crate) struct Fetch;
impl Mode for Fetch {
    type Body = HarnessVersionFetchBody;
    const ADMIN: bool = false;
    fn destination(body: Self::Body) -> ApiResult<Destination> {
        match (body.environment_id, body.node_id) {
            (Some(id), None) => Ok(Destination::Environment(id)),
            (None, Some(id)) => Ok(Destination::Node(id)),
            (None, None) => Ok(Destination::System),
            _ => Err(super::invalid("environment_id", "name one settings scope")),
        }
    }
    fn declaration(destination: &Destination) -> &'static Declaration<Source> {
        destination.declaration()
    }
}
pub(crate) struct Program;
impl Mode for Program {
    type Body = ();
    const ADMIN: bool = true;
    fn destination((): ()) -> ApiResult<Destination> {
        Ok(Destination::System)
    }
    fn declaration(_destination: &Destination) -> &'static Declaration<Source> {
        &orch_engine::build_checks::PROGRAM_FETCH
    }
}
pub(crate) struct Action<M>(PhantomData<M>);
pub(crate) struct Input {
    kind: String,
    version: String,
    destination: Destination,
    programs: orch_agent_programs::Programs,
    mirror: Option<String>,
    trust_mirror: bool,
    fetch: bool,
}
pub(crate) struct Fetched {
    kind: String,
    version: String,
    destination: Destination,
    builds: Option<Vec<HarnessBuild>>,
}
pub(crate) struct Prepared {
    input: Arc<Fetched>,
}
pub(crate) struct Accepted {
    input: Arc<Fetched>,
    claims: Vec<(String, HarnessBuild, Claim)>,
}

fn factual_error(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal(error)
}
async fn require(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    destination: &Destination,
) -> ApiResult<()> {
    if matches!(destination, Destination::System) {
        super::super::native::require(ctx, principal)?;
    }
    if !destination
        .may_manage(conn, principal)
        .await
        .map_err(factual_error)?
    {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "current settings management is required to fetch a build",
        ));
    }
    Ok(())
}

impl<M: Mode> effects::Initial for Action<M> {
    type Input = Input;
    type Fetched = Fetched;
    fn declaration(input: &Input) -> &'static Declaration<Source> {
        M::declaration(&input.destination)
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _caller: &'a Caller,
        (): (),
        body: M::Body,
    ) -> BoxFuture<'a, ApiResult<(Input, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let destination = M::destination(body)?;
            let kind = ctx.param("kind")?.to_owned();
            let version = ctx.param("version")?.to_owned();
            if !super::is_word(&version, &['-', '_', '.', '+']) {
                return Err(super::invalid("version", "must be a version"));
            }
            if let Destination::Node(id) = &destination {
                let (_, kind) = crate::routes::nodes::placed(conn, *id).await?;
                if !matches!(
                    kind,
                    orch_api_contract::nodes::NodeKind::Org
                        | orch_api_contract::nodes::NodeKind::Project
                ) {
                    return Err(super::invalid("node_id", "must be an org or project"));
                }
            }
            let facts = vec![destination.facts(conn).await.map_err(factual_error)?];
            let known = super::super::programs::exact_builds(ctx, conn, &kind, &version).await?;
            let fetch = !known
                .iter()
                .any(|build| build.kind == kind && build.version == version);
            let programs = orch_db::programs::registry(conn).await?;
            let mirror = if fetch {
                orch_settings::get::<String>(
                    conn,
                    orch_settings::Place::System,
                    orch_settings_contract::registry::keys::AGENTS_HARNESS_MIRROR,
                )
                .await
                .map_err(factual_error)?
            } else {
                None
            };
            let trust_mirror = crate::harness_channels::trusts_mirror(conn).await?;
            Ok((
                Input {
                    kind,
                    version,
                    destination,
                    programs,
                    mirror,
                    trust_mirror,
                    fetch,
                },
                facts,
            ))
        })
    }
    fn require_initial<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(ctx, conn, principal, &input.destination))
    }
    fn fetch<'a>(
        _ctx: &'a Ctx,
        _caller: &'a Caller,
        input: Input,
    ) -> BoxFuture<'a, ApiResult<Fetched>> {
        Box::pin(async move {
            let builds = if input.fetch {
                Some(
                    crate::harness_channels::read_exact_configured(
                        &reqwest::Client::new(),
                        &input.programs,
                        input.mirror.as_deref(),
                        input.trust_mirror,
                        &input.kind,
                        &input.version,
                    )
                    .await
                    .map_err(|reason| super::invalid("version", &reason))?,
                )
            } else {
                None
            };
            Ok(Fetched {
                kind: input.kind,
                version: input.version,
                destination: input.destination,
                builds,
            })
        })
    }
}

impl<M: Mode> effects::Effect for Action<M> {
    type Source = Source;
    type Query = ();
    type Body = M::Body;
    type Value = Prepared;
    type Output = Accepted;
    fn declaration(value: &Prepared) -> &'static Declaration<Source> {
        M::declaration(&value.input.destination)
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _caller: &'a Caller,
        (): (),
        body: M::Body,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let input = effects::initial_value::<Self>(ctx)?;
            if M::destination(body)? != input.destination
                || ctx.param("kind")? != input.kind
                || ctx.param("version")? != input.version
            {
                return Err(ApiError::internal(
                    "the original exact-version request changed",
                ));
            }
            // This rare short writer covers existing and absent platform rows together. Supplier
            // IO already ended; no network, model or connector wait holds this native footprint.
            if orch_db::queries::transaction_guard::in_transaction(conn) {
                sqlx::query("LOCK TABLE settings IN SHARE ROW EXCLUSIVE MODE")
                    .execute(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
            }
            let facts = vec![input.destination.facts(conn).await.map_err(factual_error)?];
            Ok((Prepared { input }, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(ctx, conn, principal, &value.input.destination))
    }
}
impl<M: Mode> effects::Apply for Action<M> {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Accepted>>> {
        Box::pin(async move {
            let mut before = Some(before);
            let written = if let Some(builds) = &value.input.builds {
                crate::harness_channels::keep_before(conn, builds, ctx.now, || {
                    before.take().map_or(Ok(()), |cut| cut.check())
                })
                .await?
            } else {
                0
            };
            let input = Arc::clone(&value.input);
            let known =
                super::super::programs::exact_builds(ctx, conn, &input.kind, &input.version)
                    .await?;
            if !known
                .iter()
                .any(|build| build.kind == input.kind && build.version == input.version)
            {
                return Err(super::invalid(
                    "version",
                    "the requested build is no longer listed; fetch it again",
                ));
            }
            let request = Request::capture(principal.caller(), input.destination.clone())
                .map_err(factual_error)?;
            let mut claims = Vec::new();
            for build in known.into_iter().filter(|build| {
                build.kind == input.kind
                    && build.version == input.version
                    && matches!(
                        build.source,
                        HarnessBuildSource::Channel | HarnessBuildSource::Mirror
                    )
                    && build.status == HarnessBuildStatus::Untested
                    && build.check.is_none()
            }) {
                let key = crate::harness_channels::channel_key(&build);
                if let Some(claim) = crate::harness_checks::claim_in_before(
                    conn,
                    &key,
                    ctx.now,
                    Some(request.clone()),
                    || before.take().map_or(Ok(()), |cut| cut.check()),
                )
                .await?
                {
                    claims.push((key, build, claim));
                }
            }
            let changed = written > 0 || !claims.is_empty();
            let accepted = Accepted { input, claims };
            Ok(if changed {
                effects::Outcome::Applied(accepted)
            } else {
                effects::Outcome::Unchanged(accepted)
            })
        })
    }
}

pub(crate) async fn run<M: Mode>(ctx: Ctx) -> ApiResult<HarnessFetchResponse> {
    let (mut tx, _) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Action<M>>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    let receipt = HarnessFetchResponse::Receipt(HarnessFetchReceipt {
        projection: ReceiptProjection::Receipt,
        kind: done.output.input.kind.clone(),
        version: done.output.input.version.clone(),
    });
    publication::receipt(&done.ctx, &receipt)?;
    let Accepted { input, claims } = done.output;
    for (key, build, claim) in claims {
        let app = ctx.app.clone();
        tokio::spawn(async move {
            if let Err(error) = crate::harness_checks::finish(&app, &key, &build, claim).await {
                tracing::warn!(%error, "an explicitly requested build could not be checked");
            }
        });
    }
    Ok(
        reply::render::<Content<M>>(&done.ctx, &done.accepted, input)
            .await?
            .map_or(receipt, HarnessFetchResponse::full),
    )
}
struct Content<M>(PhantomData<M>);
impl<M: Mode> reply::Projection for Content<M> {
    type Source = Source;
    type Input = Arc<Fetched>;
    type Subject = Destination;
    type Value = Vec<HarnessBuild>;
    type Reply = Vec<HarnessBuild>;
    const READ_SOURCE: Source = Source::Catalogue;
    fn declaration(input: &Arc<Fetched>) -> &'static Declaration<Source> {
        M::declaration(&input.destination)
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Arc<Fetched>,
    ) -> BoxFuture<'a, ApiResult<(Destination, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let system = orch_engine::operation_scope::targets::system();
            Ok((
                input.destination.clone(),
                vec![
                    input.destination.facts(conn).await.map_err(factual_error)?,
                    native::Facts::one(Source::Catalogue, system.object.kind, system),
                ],
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Arc<Fetched>,
        _subject: &'a Destination,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if M::ADMIN {
                super::super::native::standing(ctx, principal)?;
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Arc<Fetched>,
        _subject: &'a Destination,
    ) -> BoxFuture<'a, ApiResult<(Vec<HarnessBuild>, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let builds = if M::ADMIN {
                super::super::programs::exact_builds(ctx, conn, &input.kind, &input.version).await?
            } else {
                super::all_builds(ctx, conn).await?
            };
            Ok((builds, Vec::new()))
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Arc<Fetched>,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Vec<HarnessBuild>>> {
        Box::pin(async move { Ok(prepared.value.clone()) })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Arc<Fetched>,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject == fresh.subject
                && old.value.iter().all(|old| {
                    fresh.value.iter().any(|now| {
                        old.kind == now.kind
                            && old.version == now.version
                            && old.os == now.os
                            && old.arch == now.arch
                            && old.sha256 == now.sha256
                            && old.source == now.source
                    })
                }))
        })
    }
}
