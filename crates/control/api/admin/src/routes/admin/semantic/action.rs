//! One native administrative settings/generation writer for browser and key requests.

use super::org_ref;

use super::view;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::native::reply;

use crate::key_operations::publication;

use crate::key_operations::targets;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use futures::future::BoxFuture;

use orch_api_contract::admin::SemanticSearchKeepBody;

use orch_api_contract::admin::SemanticSearchOrg;

use orch_api_contract::admin::SemanticSearchReceipt;

use orch_api_contract::admin::SemanticSearchResponse;

use orch_api_contract::admin_sources as sources;

use orch_api_contract::admin_sources::Source;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::DbError;

use orch_db::queries::semantic as rows;

use orch_engine::semantic::administration::Request;

use orch_settings::write::Write;

use orch_settings::write::WriteError;

use orch_settings::write::write_before;

use orch_settings_contract::registry::keys;

use orch_types::Id;

use orch_types::Json;

use serde_json::json;

use std::marker::PhantomData;
pub(super) trait Mode: Send + Sync + 'static {
    type Body: Send + 'static;
    const ROW: &'static Declaration<Source>;
    fn keep(body: Self::Body) -> Option<Id>;
}

pub(super) struct Rebuild;

impl Mode for Rebuild {
    type Body = ();
    const ROW: &'static Declaration<Source> = &sources::SEMANTIC_REBUILD;
    fn keep((): ()) -> Option<Id> {
        None
    }
}

pub(super) struct Keep;

impl Mode for Keep {
    type Body = SemanticSearchKeepBody;
    const ROW: &'static Declaration<Source> = &sources::SEMANTIC_KEEP;
    fn keep(body: Self::Body) -> Option<Id> {
        Some(body.generation_id)
    }
}

pub(super) struct Action<M>(PhantomData<M>);

pub(super) struct Prepared {
    org: Id,
    generations: Vec<rows::GenerationRow>,
    keep: Option<Id>,
    writes: Vec<(&'static str, Json, bool)>,
}

pub(super) struct Accepted {
    org: Id,
    request: Id,
}

impl<M: Mode> effects::Effect for Action<M> {
    type Source = Source;
    type Query = ();
    type Body = M::Body;
    type Value = Prepared;
    type Output = Accepted;
    fn declaration(_value: &Prepared) -> &'static Declaration<Source> {
        M::ROW
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: M::Body,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let org = ctx.id("id")?;
            let keep = M::keep(body);
            // The root also serializes a first index row; no model/vendor IO holds these locks.
            sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
                .bind(org)
                .fetch_optional(&mut *conn)
                .await
                .map_err(DbError::from)?
                .ok_or_else(ApiError::not_found)?;
            if rows::root_org(conn, org).await?.is_none() {
                return Err(ApiError::not_found());
            }
            let claimed: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
                "SELECT claimed_until FROM search_indexes WHERE org_id=$1 FOR UPDATE",
            )
            .bind(org)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?
            .flatten();
            if keep.is_some() && claimed.is_some_and(|until| until > ctx.now) {
                return Err(ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "The indexer is finishing a batch; try again when it has finished.",
                ));
            }
            sqlx::query("SELECT id FROM search_generations WHERE org_id=$1 ORDER BY id FOR UPDATE")
                .bind(org)
                .fetch_all(&mut *conn)
                .await
                .map_err(DbError::from)?;
            let generations = rows::generations(conn, org).await?;
            let target = targets::node(conn, org).await?;
            let facts = vec![native::Facts::one(Source::Org, target.object.kind, target)];
            let known = native::bind_required(ctx, M::ROW, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            super::super::native::require(ctx, &principal)?;
            let mut writes = Vec::new();
            if let Some(id) = keep {
                let active = generations
                    .iter()
                    .find(|row| row.id == id && row.state == "active")
                    .ok_or_else(|| {
                        ApiError::new(
                            ErrorCode::ConflictStaleRevision,
                            "The active index changed; read the current model before choosing.",
                        )
                    })?;
                let chunk_tokens = active
                    .chunking
                    .rsplit_once(':')
                    .and_then(|(_, size)| size.parse::<i64>().ok())
                    .ok_or_else(|| {
                        ApiError::internal("the active index's chunk size is unreadable")
                    })?;
                // Both possibly absent setting rows are written together. The native table lock
                // also excludes competing first inserts while the original callback is prepared.
                if orch_db::queries::transaction_guard::in_transaction(conn) {
                    sqlx::query("LOCK TABLE settings IN SHARE ROW EXCLUSIVE MODE")
                        .execute(&mut *conn)
                        .await
                        .map_err(DbError::from)?;
                }
                let mut named = json!({"provider_id":active.provider_id,"model_name":active.model_name,
                    "query_prefix":active.query_prefix,"document_prefix":active.document_prefix});
                if let Some(dimensions) = active.asked_dimensions {
                    named["dimensions"] = json!(dimensions);
                }
                let mut tuning = orch_settings::get::<Json>(
                    conn,
                    orch_settings::Place::Node(org),
                    keys::SEARCH_SEMANTIC_TUNING,
                )
                .await
                .map_err(ApiError::internal)?
                .ok_or_else(|| ApiError::internal("search tuning has no value"))?;
                tuning["chunk_tokens"] = json!(chunk_tokens);
                let policy = crate::routes::nodes::policy_at(conn, org).await?;
                let administration = ctx.administrative_node(&principal, org)?;
                for (key, value) in [
                    (keys::SEARCH_SEMANTIC_EMBEDDINGS, named),
                    (keys::SEARCH_SEMANTIC_TUNING, tuning),
                ] {
                    let may_lock = crate::routes::settings::permit_admin_operation(
                        conn,
                        &administration,
                        key,
                        policy,
                    )
                    .await?;
                    writes.push((key, value, may_lock));
                }
            }
            if orch_db::queries::transaction_guard::in_transaction(conn)
                && generations
                    .iter()
                    .any(|row| keep.is_none() || row.state == "building")
            {
                rows::lock_vector_tables(conn).await?;
            }
            Ok((
                Prepared {
                    org,
                    generations,
                    keep,
                    writes,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
        _value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { super::super::native::require(ctx, principal) })
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
            let origin = crate::sign_in::origin(ctx);
            for (key, setting, may_lock) in &value.writes {
                let result = write_before(
                    conn,
                    &Write {
                        actor: orch_db::audit::Actor::User(principal.user()),
                        target: orch_settings::Target::Node(value.org),
                        key,
                        value: setting.clone(),
                        locked: None,
                        expected_revision: None,
                        may_lock: *may_lock,
                        origin: &origin,
                    },
                    ctx.now,
                    || {
                        before.take().map_or(Ok(()), |cut| {
                            cut.check().map_err(|_| WriteError::OperationRefused)
                        })
                    },
                )
                .await;
                if let Err(error) = result {
                    return Err(crate::routes::settings::write_failed(
                        conn, principal, value.org, error,
                    )
                    .await);
                }
            }
            for generation in &value.generations {
                if value.keep.is_none() || generation.state == "building" {
                    if let Some(cut) = before.take() {
                        cut.check()?;
                    }
                    rows::drop_generation(conn, generation.id).await?;
                }
            }
            let chosen = orch_engine::semantic::choice(conn, value.org)
                .await
                .map_err(ApiError::internal)?;
            let request = Request::capture(conn, principal.caller(), &chosen)
                .await
                .map_err(ApiError::internal)?;
            if let Some(id) = value.keep {
                if let Some(cut) = before.take() {
                    cut.check()?;
                }
                sqlx::query(
                    "UPDATE search_generations SET writer_id=$2,administration_id=$3 WHERE id=$1",
                )
                .bind(id)
                .bind(principal.user())
                .bind(request.id())
                .execute(&mut *conn)
                .await
                .map_err(DbError::from)?;
            }
            if let Some(cut) = before.take() {
                cut.check()?;
            }
            request
                .store(conn, orch_types::Time::new(ctx.now))
                .await
                .map_err(ApiError::internal)?;
            rows::reset_index(conn, value.org, ctx.now).await?;
            if value.keep.is_some() {
                orch_db::queries::hierarchy::stamp(conn, value.org).await?;
            }
            orch_db::idempotency::creation::observe_with(
                conn,
                Some(principal.user()),
                "semantic_administration",
                request.id(),
                json!({"org":value.org}),
            )
            .await?;
            Ok(effects::Outcome::Applied(Accepted {
                org: value.org,
                request: request.id(),
            }))
        })
    }
}

pub(super) async fn run<M: Mode>(ctx: Ctx) -> ApiResult<SemanticSearchResponse> {
    let (mut tx, _) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Action<M>>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    let receipt = SemanticSearchResponse::Receipt(SemanticSearchReceipt {
        projection: ReceiptProjection::Receipt,
        org_id: done.output.org,
        request_id: done.output.request,
    });
    publication::receipt(&done.ctx, &receipt)?;
    ctx.app.engine.kick();
    Ok(
        reply::render::<Content<M>>(&done.ctx, &done.accepted, done.output)
            .await?
            .map_or(receipt, SemanticSearchResponse::full),
    )
}

struct Content<M>(PhantomData<M>);

impl<M: Mode> reply::Projection for Content<M> {
    type Source = Source;
    type Input = Accepted;
    type Subject = Id;
    type Value = SemanticSearchOrg;
    type Reply = SemanticSearchOrg;
    const READ_SOURCE: Source = Source::Content;
    fn declaration(_input: &Accepted) -> &'static Declaration<Source> {
        M::ROW
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Accepted,
    ) -> BoxFuture<'a, ApiResult<(Id, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let target = targets::node(conn, input.org).await?;
            Ok((
                input.org,
                vec![
                    native::Facts::one(Source::Org, target.object.kind, target.clone()),
                    native::Facts::one(Source::Content, target.object.kind, target),
                ],
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Accepted,
        _subject: &'a Id,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { super::super::native::standing(ctx, principal) })
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Accepted,
        subject: &'a Id,
    ) -> BoxFuture<'a, ApiResult<(SemanticSearchOrg, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let work = orch_engine::semantic::administration::Work::load(conn, *subject)
                .await
                .map_err(ApiError::internal)?;
            if work.as_ref().map(|work| work.request_id()) != Some(input.request) {
                return Err(ApiError::not_found());
            }
            let name = rows::root_org(conn, *subject)
                .await?
                .map(|(_, name)| name)
                .ok_or_else(ApiError::not_found)?;
            let state = orch_engine::Engine::semantic_state_in(conn, *subject)
                .await
                .map_err(ApiError::internal)?;
            Ok((view(org_ref(*subject, name), state), Vec::new()))
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Accepted,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<SemanticSearchOrg>> {
        Box::pin(async move { Ok(prepared.value.clone()) })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Accepted,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject == fresh.subject
                && old.value.active == fresh.value.active
                && old.value.building == fresh.value.building
                && old.value.provider_id == fresh.value.provider_id
                && old.value.model_name == fresh.value.model_name
                && old.value.dimensions == fresh.value.dimensions)
        })
    }
}

pub(super) fn creation<M: Mode>()
-> crate::middleware::idempotency::ObservedCreation<(), M::Body, SemanticSearchResponse> {
    crate::middleware::idempotency::ObservedCreation {
        kind: "semantic_administration",
        replay: |ctx, (), _body, created| {
            Box::pin(async move {
                let org: Id = serde_json::from_value(created.data["org"].clone())
                    .map_err(ApiError::internal)?;
                if created.kind != "semantic_administration" || ctx.id("id")? != org {
                    return Err(ApiError::not_found());
                }
                let receipt = SemanticSearchResponse::Receipt(SemanticSearchReceipt {
                    projection: ReceiptProjection::Receipt,
                    org_id: org,
                    request_id: created.id,
                });
                publication::receipt(&ctx, &receipt)?;
                match reply::render_completed_replay::<Content<M>>(
                    &ctx,
                    ctx.user()?.id(),
                    Accepted {
                        org,
                        request: created.id,
                    },
                )
                .await
                {
                    Ok(Some(value)) => Ok(SemanticSearchResponse::full(value)),
                    Ok(None) => Ok(receipt),
                    Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
                        Ok(receipt)
                    }
                    Err(error) => Err(error),
                }
            })
        },
    }
}
