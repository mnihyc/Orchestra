//! Resource paragraphs through native Read, paid-refresh admission and metadata-only receipts.

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::publication;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::Registry;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::Problem;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::routes::Endpoint;

use orch_api_contract::summaries::*;

use orch_api_contract::summary_sources as declarations;

use orch_api_contract::summary_sources::ResourceSource as Role;

use orch_authz::Caller;

use orch_authz::CapturedOperation;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::DbError;

use orch_db::queries::summaries as rows;

use orch_engine::operation_scope::Prepared;

use orch_engine::summaries::options;

use orch_engine::summaries::resource;

use orch_engine::summaries::sources;

use orch_types::Id;

use orch_types::Json;

use serde_json::json;

mod attempts;

use std::collections::BTreeMap;

pub use orch_api_base::routes::summaries::View;
/// Binds the native resource paragraph read and paid refresh acts.
pub fn mount(registry: &mut Registry) {
    registry.on_snapshot_read_with_optional_operation(
        SummaryGet,
        native::selected::<Reader>,
        native::selected_read::<Reader>,
    );
    registry.on_snapshot_body_read_with_optional_operation(
        SummaryBatchGet,
        native::selected_body::<batch::Reader, (), SummaryBatchBody>,
        native::selected_body_read::<batch::Reader, (), SummaryBatchBody>,
    );
    registry.on_with_operation(SummaryRefresh, effects::prepare::<Refresher>, refresh);
}
orch_api_base::__api_bind_862439cb84!();

fn source_error(_: impl std::fmt::Display) -> ApiError {
    ApiError::not_found()
}

fn target(ctx: &Ctx) -> ApiResult<SummaryTarget> {
    let id = ctx.id("id")?;
    match ctx.param("kind")? {
        "thread" => Ok(SummaryTarget::Thread { id }),
        "topic" => Ok(SummaryTarget::Topic { id }),
        "document" => Ok(SummaryTarget::Document { id }),
        "chat" => Ok(SummaryTarget::Chat { id }),
        "chat_title" => Ok(SummaryTarget::ChatTitle { id }),
        _ => Err(ApiError::not_found()),
    }
}

fn range(query: &SummaryReadQuery, target: SummaryTarget) -> ApiResult<Option<SummaryChatRange>> {
    match (target, query.after, query.through) {
        (SummaryTarget::Chat { .. }, Some(after), Some(through))
            if after >= 0 && through >= after =>
        {
            Ok(Some(SummaryChatRange { after, through }))
        }
        (_, None, None) => Ok(None),
        _ => Err(ApiError::invalid(
            "range",
            "Use an ordered chat range with both after and through",
        )),
    }
}

async fn require_source(
    conn: &mut Conn,
    principal: &Principal,
    target: SummaryTarget,
) -> ApiResult<()> {
    if !sources::readable(conn, principal, target)
        .await
        .map_err(source_error)?
    {
        return Err(ApiError::not_found());
    }
    Ok(())
}

async fn require_disclosure(
    conn: &mut Conn,
    principal: &Principal,
    target: SummaryTarget,
) -> ApiResult<()> {
    if !sources::viewable(conn, principal, target)
        .await
        .map_err(source_error)?
    {
        return Err(ApiError::not_found());
    }
    Ok(())
}

/// The same native opening audit for stored paragraphs and protected model-input text.
/// Selection/publication retain all canonical Read facts; this once-only hook creates no
/// chat membership, cursor, draft, notification or model request.
pub(crate) async fn note_chat_openings(
    conn: &mut Conn,
    principal: &Principal,
    inputs: &[KeyResolvedTarget],
) -> ApiResult<()> {
    let origin = orch_db::audit::current_origin();
    let act = orch_chat::Acting {
        principal,
        now: principal.now().get(),
        origin: &origin,
    };
    let mut seen = std::collections::BTreeSet::new();
    for fact in inputs {
        if fact.object.kind != KeyTargetKind::Identity(KeyIdentityKind::Chat) {
            continue;
        }
        let id = fact.object.id.ok_or_else(ApiError::not_found)?;
        if seen.insert(id) {
            let loaded = orch_chat::chats::load(conn, &act, id, orch_chat::chats::Mode::Read)
                .await
                .map_err(source_error)?;
            orch_chat::inspection::note_opened(conn, &act, &loaded)
                .await
                .map_err(source_error)?;
        }
    }
    Ok(())
}
orch_api_base::__api_bind_2f44c829e4!();

fn read_facts(
    identity: &sources::Identity,
    inputs: &[KeyResolvedTarget],
    provider: &[KeyResolvedTarget],
) -> Vec<native::Facts<Role>> {
    let mut groups = BTreeMap::new();
    for input in inputs {
        groups
            .entry(input.object.kind)
            .or_insert_with(Vec::new)
            .push(input.clone());
    }
    let mut providers = BTreeMap::new();
    for input in provider {
        providers
            .entry(input.object.kind)
            .or_insert_with(Vec::new)
            .push(input.clone());
    }
    vec![
        native::Facts::one(
            Role::Target,
            identity.fact().object.kind,
            identity.fact().clone(),
        ),
        native::Facts::collection(Role::Inputs, groups.into_iter().collect()),
        native::Facts::collection(Role::Provider, providers.into_iter().collect()),
    ]
}

/// A signed-in reader may see the model admitted for the summary. For a key, its
/// consuming operation must also admit the provider source before returning either name.
fn project_model_names(
    principal: &Principal,
    provider: Option<&KeyResolvedTarget>,
    view: &mut SummaryProducerView,
) {
    if principal.caller().grant().is_some() && provider.is_none() {
        view.title = "Model summarizer".into();
        view.offering = None;
    }
}

async fn readable_provider_fact(
    conn: &mut Conn,
    principal: &Principal,
    plan: &Prepared,
    id: Id,
    providers_slot: usize,
) -> ApiResult<Option<KeyResolvedTarget>> {
    let Ok(fact) = orch_engine::operation_scope::targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Provider,
            id,
        },
    )
    .await
    else {
        return Ok(None);
    };
    if !plan
        .permits_binding(
            principal.caller(),
            providers_slot,
            fact.object.kind,
            std::slice::from_ref(&fact),
        )
        .map_err(source_error)?
    {
        return Ok(None);
    }
    // A read's views name the same provider for their producers and routes: its View is decided once in the read.
    let viewable = match orch_engine::summaries::once::provider_view(principal.caller(), id) {
        Some(viewable) => viewable,
        None => {
            let viewable = orch_authz::resources::check(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Provider,
                    id,
                },
                Permission::ProviderView,
                orch_authz::Policy::default(),
            )
            .await
            .map_err(source_error)?
            .is_allowed();
            orch_engine::summaries::once::keep_provider_view(principal.caller(), id, viewable);
            viewable
        }
    };
    Ok(viewable.then_some(fact))
}

/// A view asks of its place's model for its producer, its route and its proposal: one read judges each choice once.
async fn read_view(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    target: SummaryTarget,
    wanted: Option<SummaryChatRange>,
    plan: &Prepared,
    slots: [usize; 3],
) -> ApiResult<View> {
    orch_engine::summaries::once::deciding(read_view_judged(
        ctx, conn, caller, target, wanted, plan, slots,
    ))
    .await
}

async fn read_view_judged(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    target: SummaryTarget,
    wanted: Option<SummaryChatRange>,
    plan: &Prepared,
    slots: [usize; 3],
) -> ApiResult<View> {
    let principal = Principal::load(
        conn,
        plan.preparation_caller(caller).map_err(source_error)?,
        ctx.app.clock.time(),
    )
    .await
    .map_err(ApiError::internal)?;
    let identity = sources::identify(conn, target)
        .await
        .map_err(source_error)?;
    require_disclosure(conn, &principal, target).await?;
    let policy = options::resolve(conn, &identity)
        .await
        .map_err(ApiError::engine)?;
    let personal = rows::get(conn, target, Some(principal.caller().user())).await?;
    let shared = rows::get(conn, target, None).await?;
    let latest = match personal.as_ref().or(shared.as_ref()) {
        Some(cache) => rows::latest(conn, cache.id).await?,
        None => None,
    };
    let cache = personal
        .as_ref()
        .filter(|row| row.text.is_some())
        .or(shared.as_ref())
        .cloned();
    let checked =
        cache_read::guard(conn, &principal, target, plan, wanted, cache, slots[1]).await?;
    let cache = checked.cache;
    let paragraph_inputs = checked.inputs.clone();
    let mut inputs = checked.inputs;
    let mut proposal_inputs = Vec::new();
    let inspecting = checked.inspecting;
    let fresh = if let Some(cache) = &cache {
        let cursor = if matches!(target, SummaryTarget::ChatTitle { .. }) {
            None
        } else {
            cache
                .source_snapshot
                .as_ref()
                .and_then(|source| source.get("range"))
                .cloned()
                .map(serde_json::from_value::<Option<SummaryChatRange>>)
                .transpose()
                .map_err(ApiError::internal)?
                .flatten()
        };
        let scope = sources::ReadScope::new(plan, slots[0], slots[1]).map_err(source_error)?;
        let previous = cache
            .source_snapshot
            .as_ref()
            .map(sources::context_plan)
            .transpose()
            .map_err(source_error)?
            .flatten();
        let budget = previous.as_ref().map_or(
            sources::Budget {
                tokens: policy.context_tokens.get(),
                frame: 0,
            },
            |plan| plan.budget,
        );
        let material = sources::disclosure_material(
            conn, &principal, target, &scope, cursor, budget, previous,
        )
        .await
        .map_err(source_error)?;
        cache
            .source_snapshot
            .as_ref()
            .is_some_and(|kept| sources::same_content(kept, &material.snapshot))
            && cache.completed_dirty_revision == Some(cache.dirty_revision)
    } else {
        false
    };
    let (producer, mut providers) =
        cache_read::producer(conn, &principal, plan, cache.as_ref(), slots[2]).await?;
    let attempt = attempts::read(
        conn,
        &principal,
        target,
        plan,
        wanted,
        latest.as_ref(),
        &slots,
    )
    .await?;
    inputs.extend(attempt.inputs);
    if let Some(fact) = attempt.provider
        && !providers.contains(&fact)
    {
        providers.push(fact);
    }
    let program = cache_read::program(
        conn,
        &principal,
        plan,
        latest.as_ref(),
        slots,
        &mut inputs,
        &mut providers,
    )
    .await?;
    let manual = if policy.producer.is_none() {
        Some(
            resource::manual_default(
                conn,
                &principal,
                identity.place(),
                resource::field_for(target),
                ctx.app.clock.time(),
            )
            .await
            .map_err(ApiError::engine)?,
        )
    } else {
        None
    };
    let manual_producer = manual
        .as_ref()
        .and_then(|manual| manual.config.as_ref())
        .map(|config| {
            Ok::<_, ApiError>(options::Producer {
                kind: "model".into(),
                config: serde_json::to_value(config).map_err(ApiError::internal)?,
                writer: None,
            })
        })
        .transpose()?;
    let pool = resource::pool::configured(&policy).map_err(ApiError::engine)?;
    let pool_member = if let Some(pool) = pool {
        resource::pool::preview(conn, &principal, identity.place(), &policy, target, pool)
            .await
            .map_err(ApiError::engine)?
            .map(|(config, member)| -> ApiResult<_> {
                Ok((
                    options::Producer {
                        kind: "model".into(),
                        config: serde_json::to_value(config).map_err(ApiError::internal)?,
                        writer: policy.producer.as_ref().and_then(|p| p.writer),
                    },
                    member,
                ))
            })
            .transpose()?
    } else {
        None
    };
    let preview_producer = if pool.is_some() {
        pool_member.as_ref().map(|(producer, _)| producer)
    } else {
        policy.producer.as_ref().or(manual_producer.as_ref())
    };
    let default_provider = preview_producer
        .filter(|producer| producer.kind == "model")
        .and_then(|producer| producer.config.pointer("/backend/offering/provider"))
        .and_then(Json::as_str)
        .and_then(|id| id.parse::<Id>().ok());
    let default_fact = match default_provider.filter(|_| pool.is_none()) {
        Some(id) => readable_provider_fact(conn, &principal, plan, id, slots[2]).await?,
        None => None,
    };
    // A pool already checked a native route. Its representative is private until acceptance.
    let (route, unavailable) = if pool.is_some() {
        (None, pool_member.is_none())
    } else {
        let route = resource::preview_route(
            conn,
            &principal,
            identity.place(),
            preview_producer,
            default_fact.is_some(),
        )
        .await
        .ok()
        .flatten();
        let unavailable = route
            .as_ref()
            .is_none_or(|resolution| resolution.route.is_none());
        (route, unavailable)
    };
    let refresh_proposal = if !inspecting && !unavailable {
        if let Some(producer) = preview_producer.filter(|producer| producer.kind == "model") {
            let scope = sources::ReadScope::new(plan, slots[0], slots[1]).map_err(source_error)?;
            match resource::preview_selection(
                conn,
                &principal,
                target,
                wanted,
                &scope,
                policy.clone(),
                producer,
                pool_member.as_ref().map(|(_, member)| member),
            )
            .await
            {
                Ok(selected) => {
                    let budget = if pool.is_some() {
                        None
                    } else {
                        Some(
                            resource::proposal_budget(conn, &selected)
                                .await
                                .map_err(ApiError::engine)?,
                        )
                    };
                    let owner = principal.caller().user();
                    let mut people =
                        crate::middleware::authorization::refusals::user_refs(conn, &[owner])
                            .await?;
                    let revision = orch_types::Hash::of(&serde_json::to_vec(&json!({"source":selected.material.snapshot,"policy":selected.policy.fingerprint,"config":selected.config,"owner":owner})).map_err(ApiError::internal)?);
                    for fact in &selected.material.inputs {
                        if !inputs.iter().any(|old| old.object == fact.object) {
                            inputs.push(fact.clone());
                        }
                    }
                    proposal_inputs.clone_from(&selected.material.inputs);
                    let model = if pool.is_some() {
                        SummaryProducerView {
                            kind: "pool".into(),
                            title: "Model pool".into(),
                            provider: None,
                            offering: None,
                        }
                    } else {
                        let offering = selected.config.offering().map_err(ApiError::internal)?;
                        let model = orch_db::queries::resources::thread_engine::model(
                            conn,
                            offering.provider,
                            &offering.offering,
                        )
                        .await?;
                        let display = model
                            .map(|model| model.display_name)
                            .filter(|name| !name.is_empty())
                            .unwrap_or_else(|| offering.offering.clone());
                        let mut model = SummaryProducerView {
                            kind: "model".into(),
                            title: display,
                            provider: route.as_ref().and_then(|route| route.provider.clone()),
                            offering: Some(offering.offering.clone()),
                        };
                        project_model_names(&principal, default_fact.as_ref(), &mut model);
                        model
                    };
                    Some(orch_api_contract::summary_facts::SummaryRefreshProposal {
                        revision,
                        producer: model,
                        usage_owner: people.remove(&owner),
                        input_estimate_tokens: budget.map(|(input, _)| input),
                        output_allowance_tokens: budget.map(|(_, output)| output),
                        estimate_basis: orch_api_contract::forks::EstimateBasis::Assumed,
                        problem: None,
                    })
                }
                Err(orch_engine::EngineError::BeforeEffect(_)) => None,
                Err(error) => return Err(ApiError::engine(error)),
            }
        } else {
            None
        }
    } else {
        None
    };
    let route_names_provider = route.as_ref().is_some_and(|route| {
        route.provider.is_some() || route.offering.is_some() || !route.prices.is_empty()
    });
    let key_names_model = principal.caller().grant().is_some()
        && refresh_proposal
            .as_ref()
            .is_some_and(|proposal| proposal.producer.offering.is_some());
    if (route_names_provider || key_names_model)
        && let Some(fact) = default_fact
        && !providers.contains(&fact)
    {
        providers.push(fact);
    }
    let follow = follow::load(
        conn,
        identity.place(),
        latest.as_ref(),
        ctx.now,
        policy.follow_interval_secs,
    )
    .await?;
    let failed = latest.as_ref().is_some_and(|row| row.state == "failed");
    let changed = latest.as_ref().is_some_and(|row| {
        matches!(
            row.problem
                .as_ref()
                .and_then(|problem| problem.get("reason"))
                .and_then(Json::as_str),
            Some("source_changed" | "content_changing" | "settings_changed")
        )
    });
    let state = if follow.scheduled {
        SummaryState::Scheduled
    } else if follow.updating {
        SummaryState::Updating
    } else if changed {
        if cache.as_ref().and_then(|row| row.text.as_ref()).is_some() {
            SummaryState::Stale
        } else {
            SummaryState::Empty
        }
    } else if failed {
        SummaryState::Failed
    } else if policy.producer.is_none() && manual_producer.is_none() {
        SummaryState::NoSummarizer
    } else if cache.as_ref().and_then(|row| row.text.as_ref()).is_none() {
        if unavailable {
            SummaryState::NoSummarizer
        } else {
            SummaryState::Empty
        }
    } else if fresh {
        SummaryState::Fresh
    } else {
        SummaryState::Stale
    };
    let root_org_id = orch_db::queries::hierarchy::node(conn, identity.place())
        .await?
        .ok_or_else(ApiError::not_found)?
        .org_id;
    let snapshot = SummarySnapshot {
        root_org_id,
        target,
        inspecting,
        revision: personal
            .as_ref()
            .or(shared.as_ref())
            .map_or(0, |row| row.revision),
        state,
        text: cache.as_ref().and_then(|row| row.text.clone()),
        producer,
        program,
        refresh_proposal,
        last_attempt: attempt.attempt,
        result_facts: cache
            .as_ref()
            .and_then(|row| row.result_facts.clone())
            .map(serde_json::from_value)
            .transpose()
            .map_err(ApiError::internal)?,
        manual_problem: manual.and_then(|manual| manual.problem),
        wait: follow.wait,
        poll_after_ms: follow.poll_after_ms,
        input: cache
            .as_ref()
            .and_then(|row| row.source_snapshot.as_ref())
            .and_then(|source| source.get("context"))
            .and_then(|context| context.get("facts"))
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(ApiError::internal)?,
        updated_at: cache
            .as_ref()
            .and_then(|row| row.completed_at)
            .map(Into::into),
        route,
        problem: ((failed || changed) && !follow.updating).then(|| {
            // What the failed request stored: the thing that stopped it, by a stable word the page
            // keys its sentence by, and that sentence in English for readers that know no word.
            let stored = latest.as_ref().and_then(|row| row.problem.as_ref());
            let reason = stored
                .and_then(|problem| problem.get("reason"))
                .and_then(Json::as_str);
            let message = stored
                .and_then(|problem| problem.get("message"))
                .and_then(Json::as_str)
                .unwrap_or("The summary could not be refreshed.");
            let detail = if cache.as_ref().is_some_and(|row| row.text.is_some()) {
                format!("{message} The previous paragraph is kept.")
            } else {
                message.to_owned()
            };
            let problem = Problem::new(
                if changed {
                    ErrorCode::ConflictStaleRevision
                } else {
                    ErrorCode::ProviderUnavailable
                },
                detail,
            );
            match reason {
                Some(reason) => problem.with("reason", reason),
                None => problem,
            }
        }),
    };
    Ok(View {
        identity,
        inputs,
        paragraph_inputs,
        proposal_inputs,
        providers,
        snapshot,
        policy: policy.fingerprint,
    })
}

/// What of two views of one summary differs, by the name of the member. The wait for a request that is due
/// at a time counts down with the clock: it moves with the time of the read, not with the summary, and is
/// the same wait while the time it ends at is the same.
fn moved(old: &View, fresh: &View) -> Vec<&'static str> {
    let (old_snapshot, fresh_snapshot) = (&old.snapshot, &fresh.snapshot);
    let counting_down = old_snapshot
        .wait
        .as_ref()
        .zip(fresh_snapshot.wait.as_ref())
        .is_some_and(|(old, fresh)| old.until.is_some() && old.until == fresh.until);
    [
        ("identity", old.identity.fact() != fresh.identity.fact()),
        ("target", old_snapshot.target != fresh_snapshot.target),
        ("inputs", old.inputs != fresh.inputs),
        ("providers", old.providers != fresh.providers),
        ("policy", old.policy != fresh.policy),
        ("state", old_snapshot.state != fresh_snapshot.state),
        ("revision", old_snapshot.revision != fresh_snapshot.revision),
        ("text", old_snapshot.text != fresh_snapshot.text),
        ("wait", old_snapshot.wait != fresh_snapshot.wait),
        (
            "poll_after_ms",
            !counting_down && old_snapshot.poll_after_ms != fresh_snapshot.poll_after_ms,
        ),
        (
            "updated_at",
            old_snapshot.updated_at != fresh_snapshot.updated_at,
        ),
        ("route", old_snapshot.route != fresh_snapshot.route),
        ("producer", old_snapshot.producer != fresh_snapshot.producer),
        ("problem", old_snapshot.problem != fresh_snapshot.problem),
        ("input", old_snapshot.input != fresh_snapshot.input),
        (
            "last_attempt",
            old_snapshot.last_attempt != fresh_snapshot.last_attempt,
        ),
        (
            "refresh_proposal",
            old_snapshot.refresh_proposal != fresh_snapshot.refresh_proposal,
        ),
        (
            "result_facts",
            old_snapshot.result_facts != fresh_snapshot.result_facts,
        ),
        (
            "manual_problem",
            old_snapshot.manual_problem != fresh_snapshot.manual_problem,
        ),
        (
            "inspecting",
            old_snapshot.inspecting != fresh_snapshot.inspecting,
        ),
        (
            "root",
            old_snapshot.root_org_id != fresh_snapshot.root_org_id,
        ),
    ]
    .into_iter()
    .filter_map(|(member, differs)| differs.then_some(member))
    .collect()
}

/// Whether a read shows what the one before it showed. A difference is a summary that moved while it was
/// read (a request taken up or finished, a paragraph written, a source that has grown); the read is then
/// made again. It says nothing about who may read what: that is `same_sources`.
fn same_view(old: &View, fresh: &View) -> bool {
    let differing = moved(old, fresh);
    if differing.is_empty() {
        return true;
    }
    tracing::debug!(
        target_kind = ?old.snapshot.target,
        moved = ?differing,
        "a summary read moved between its read and its check"
    );
    false
}

/// Whether everything that the earlier read was made from may still be read by the reader, whatever else
/// about the summary has moved: the same target in the same root, read in the same way, and every source
/// and provider that the paragraph was made from still among those the reader may read. A paragraph made
/// from a source the reader can no longer read is never given.
fn same_sources(old: &View, fresh: &View) -> bool {
    let held = old.identity.fact() == fresh.identity.fact()
        && old.snapshot.root_org_id == fresh.snapshot.root_org_id
        && (old.snapshot.inspecting || !fresh.snapshot.inspecting)
        && old.inputs.iter().all(|input| fresh.inputs.contains(input))
        && old
            .providers
            .iter()
            .all(|provider| fresh.providers.contains(provider));
    if !held {
        tracing::info!(
            target_kind = ?old.snapshot.target,
            "a summary read is refused: what it was made from can no longer be read by the person who asked"
        );
    }
    held
}

/// The summary source an input's fact names, as the sources identify it.
fn source_of(input: &KeyResolvedTarget) -> Option<SummaryTarget> {
    let id = input.object.id?;
    match input.object.kind {
        KeyTargetKind::Object(ObjectKind::Thread) => Some(SummaryTarget::Thread { id }),
        KeyTargetKind::Object(ObjectKind::Topic) => Some(SummaryTarget::Topic { id }),
        KeyTargetKind::Object(ObjectKind::Document) => Some(SummaryTarget::Document { id }),
        KeyTargetKind::Identity(KeyIdentityKind::Chat) => Some(SummaryTarget::Chat { id }),
        _ => None,
    }
}

/// [`same_sources`] asked of what the view names, without making the view again: its target is where it was (the
/// same identity and root), bound by the operation and disclosed to the reader, and readable when the view was a
/// reading; every input is where it was, bound and viewable, its stored paragraph's inputs readable when the view
/// was a reading and its proposal's inputs readable as a model's input is; every provider it names is still one the
/// reader may view. Its default model, route, proposal and freshness are not read again: what moved about the summary
/// while it was read changes nothing about who may read what it was made from.
pub(crate) async fn sources_held(
    conn: &mut Conn,
    caller: &Caller,
    principal: &Principal,
    plan: &Prepared,
    view: &View,
    slots: [usize; 3],
) -> ApiResult<bool> {
    let target = view.identity.target();
    let Ok(identity) = sources::identify(conn, target).await else {
        return Ok(false);
    };
    if identity.fact() != view.identity.fact()
        || !plan
            .permits_binding(
                caller,
                slots[0],
                identity.fact().object.kind,
                std::slice::from_ref(identity.fact()),
            )
            .map_err(source_error)?
    {
        return Ok(false);
    }
    if !sources::viewable(conn, principal, target)
        .await
        .map_err(source_error)?
    {
        return Ok(false);
    }
    let root = orch_db::queries::hierarchy::node(conn, identity.place())
        .await?
        .map(|node| node.org_id);
    if root != Some(view.snapshot.root_org_id) {
        return Ok(false);
    }
    let reading = !view.snapshot.inspecting;
    if reading
        && !sources::readable(conn, principal, target)
            .await
            .map_err(source_error)?
    {
        return Ok(false);
    }
    for input in &view.inputs {
        let Some(source) = source_of(input) else {
            return Ok(false);
        };
        let Ok(current) = sources::identify(conn, source).await else {
            return Ok(false);
        };
        if current.fact() != input
            || !plan
                .permits_binding(
                    principal.caller(),
                    slots[1],
                    input.object.kind,
                    std::slice::from_ref(input),
                )
                .map_err(source_error)?
            || !sources::viewable(conn, principal, source)
                .await
                .map_err(source_error)?
        {
            return Ok(false);
        }
        let read = (reading && view.paragraph_inputs.contains(input))
            || view.proposal_inputs.contains(input);
        if read
            && !sources::readable(conn, principal, source)
                .await
                .map_err(source_error)?
        {
            return Ok(false);
        }
    }
    for provider in &view.providers {
        let Some(id) = provider.object.id else {
            return Ok(false);
        };
        if readable_provider_fact(conn, principal, plan, id, slots[2])
            .await?
            .as_ref()
            != Some(provider)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The same question answered by making the view again and asking [`same_sources`] of the two, as the openings
/// did before they checked sources directly. The comparison case calls this beside [`sources_held`].
pub(crate) async fn sources_held_by_rebuilding(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    plan: &Prepared,
    view: &View,
    wanted: Option<SummaryChatRange>,
    slots: [usize; 3],
) -> ApiResult<bool> {
    let target = view.identity.target();
    let Ok(identity) = sources::identify(conn, target).await else {
        return Ok(false);
    };
    if !plan
        .permits_binding(
            caller,
            slots[0],
            identity.fact().object.kind,
            std::slice::from_ref(identity.fact()),
        )
        .map_err(source_error)?
    {
        return Ok(false);
    }
    match read_view(ctx, conn, caller, target, wanted, plan, slots).await {
        Ok(fresh) => Ok(same_sources(view, &fresh)),
        Err(error) if matches!(error.problem.status, 403 | 404 | 410) => Ok(false),
        Err(error) => Err(error),
    }
}

struct Reader;

impl native::Selected for Reader {
    type Source = Role;
    type Query = SummaryReadQuery;
    type Value = View;
    type Reply = SummarySnapshot;
    const DECLARATION: &'static Declaration<Role> = &declarations::RESOURCE_READ;
    const WATCH_ORDINARY: bool = true;
    const MOVES: native::Moves = native::Moves::Publish;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a SummaryReadQuery,
    ) -> BoxFuture<'a, ApiResult<(View, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let target = target(ctx)?;
            let wanted = range(query, target)?;
            let plan = Prepared::new(SummaryGet::ROUTE, declarations::RESOURCE_READ.operation)
                .map_err(source_error)?;
            let view = read_view(ctx, conn, caller, target, wanted, &plan, [0, 1, 2]).await?;
            let facts = read_facts(&view.identity, &view.inputs, &view.providers);
            Ok((view, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { require_disclosure(conn, principal, target(ctx)?).await })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        value: &'a View,
    ) -> BoxFuture<'a, ApiResult<SummarySnapshot>> {
        Box::pin(async move { Ok(value.snapshot.clone()) })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a SummaryReadQuery,
        old: &'a View,
        fresh: &'a View,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(same_view(old, fresh)) })
    }
    fn access_held<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a SummaryReadQuery,
        old: &'a View,
        fresh: &'a View,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(same_sources(old, fresh)) })
    }
    fn after_read<'a>(
        ctx: &'a Ctx,
        _: &'a Caller,
        _: &'a SummaryReadQuery,
        value: &'a View,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let (mut conn, guard) = transaction_guard::begin(ctx).await?;
            let mut inputs = vec![value.identity.fact().clone()];
            inputs.extend(value.inputs.iter().cloned());
            note_chat_openings(&mut conn, guard.principal(), &inputs).await?;
            conn.commit().await.map_err(DbError::from)?;
            Ok(())
        })
    }
}

struct Refresh {
    selected: resource::Selection,
    body: SummaryRefreshBody,
}

struct Refresher;

impl effects::Effect for Refresher {
    type Source = Role;
    type Query = ();
    type Body = SummaryRefreshBody;
    type Value = Refresh;
    type Output = SummaryReceipt;
    fn declaration(_: &Refresh) -> &'static Declaration<Role> {
        &declarations::RESOURCE_REFRESH
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _: (),
        body: SummaryRefreshBody,
    ) -> BoxFuture<'a, ApiResult<(Refresh, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let target = target(ctx)?;
            if body.range.is_some() && !matches!(target, SummaryTarget::Chat { .. }) {
                return Err(ApiError::invalid(
                    "range",
                    "Only a chat has a catch-up range",
                ));
            }
            let range = if let SummaryTarget::Chat { id } = target {
                match body.range {
                    Some(range) => Some(range),
                    None => {
                        let stage = Prepared::new(
                            SummaryRefresh::ROUTE,
                            declarations::RESOURCE_REFRESH.operation,
                        )
                        .map_err(source_error)?;
                        let principal = Principal::load(
                            conn,
                            stage.preparation_caller(caller).map_err(source_error)?,
                            ctx.app.clock.time(),
                        )
                        .await
                        .map_err(ApiError::internal)?;
                        Some(
                            sources::catch_up_range(conn, &principal, id)
                                .await
                                .map_err(source_error)?,
                        )
                    }
                }
            } else {
                None
            };
            let stage = Prepared::new(
                SummaryRefresh::ROUTE,
                declarations::RESOURCE_REFRESH.operation,
            )
            .map_err(source_error)?;
            let principal = Principal::load(
                conn,
                stage.preparation_caller(caller).map_err(source_error)?,
                ctx.app.clock.time(),
            )
            .await
            .map_err(ApiError::internal)?;
            let identity = sources::identify(conn, target)
                .await
                .map_err(source_error)?;
            if !stage
                .permits_binding(
                    principal.caller(),
                    0,
                    identity.fact().object.kind,
                    std::slice::from_ref(identity.fact()),
                )
                .map_err(source_error)?
            {
                return Err(ApiError::not_found());
            }
            require_source(conn, &principal, target).await?;
            if let Some(range) = range {
                let chat = orch_db::queries::chats::chat(conn, target.id())
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                if range.after < 0
                    || range.through < range.after
                    || range.through > chat.last_message_seq
                {
                    return Err(ApiError::invalid(
                        "range",
                        "Use an ordered range inside this chat",
                    ));
                }
                if range.after == range.through {
                    // Unread material is already known to be empty. It needs no model proposal,
                    // provider permission, request row or paid completion.
                    return Err(ApiError::new(
                        ErrorCode::SummaryNothingNew,
                        "There is nothing new to summarize",
                    ));
                }
            }
            if let Some(model) = &body.model
                && (model.backend.route.trim().is_empty()
                    || model
                        .backend
                        .offering
                        .as_ref()
                        .is_none_or(|offering| offering.offering.trim().is_empty())
                    || model
                        .params
                        .as_ref()
                        .is_some_and(|params| !params.is_object()))
            {
                return Err(ApiError::invalid(
                    "model",
                    "Choose a named model with object options",
                ));
            }
            let fallback = if body.model.is_none()
                && options::resolve(conn, &identity)
                    .await
                    .map_err(ApiError::engine)?
                    .producer
                    .is_none()
            {
                let picked = resource::manual_default(
                    conn,
                    &principal,
                    identity.place(),
                    resource::field_for(target),
                    ctx.app.clock.time(),
                )
                .await
                .map_err(ApiError::engine)?;
                let config=picked.config.ok_or_else(||ApiError::new(ErrorCode::ProviderUnavailable,if picked.problem==Some(orch_api_contract::summaries::SummaryManualProblem::CapReached) {"A usage cap has no room for summaries"} else {"No permitted API model is available; check provider access and model allowlists"}))?;
                Some(orch_api_contract::summaries::SummaryModelChoice {
                    backend: config.backend,
                    params: Some(config.params),
                })
            } else {
                None
            };
            let choice = body.model.as_ref().or(fallback.as_ref());
            let initial =
                resource::select(conn, caller, target, range, choice, ctx.app.clock.time())
                    .await
                    .map_err(ApiError::engine)?;
            resource::lock_selection(conn, &initial)
                .await
                .map_err(ApiError::engine)?;
            let selected = resource::select_with_snapshot(
                conn,
                caller,
                target,
                range,
                choice,
                ctx.app.clock.time(),
                Some(&initial.material.snapshot),
            )
            .await
            .map_err(ApiError::engine)?;
            // A source with nothing in it is answered here, with its own code: no request is made, no
            // model is asked and nothing is booked, where it would only end as a request that failed.
            if selected.material.activity.is_empty() {
                return Err(ApiError::new(
                    ErrorCode::SummaryNothingNew,
                    "There is nothing new to summarize",
                ));
            }
            let facts = selected.facts();
            Ok((Refresh { selected, body }, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Refresh,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require_source(conn, principal, value.selected.material.identity.target()).await?;
            value
                .selected
                .model_entry(conn, principal)
                .await
                .map_err(ApiError::engine)?;
            if value.body.model.is_none()
                && resource::pool::configured(&value.selected.policy)
                    .map_err(ApiError::engine)?
                    .is_some()
            {
                // The accepting transaction checks each actual member's cap, including its tier.
                return Ok(());
            }
            let offering = value
                .selected
                .config
                .offering()
                .map_err(ApiError::internal)?;
            if !ctx
                .app
                .engine
                .room_for_own_use(
                    conn,
                    &orch_engine::caps::OwnCapAsk {
                        place: value.selected.material.identity.place(),
                        provider: offering.provider,
                        model: &offering.offering,
                        tier: None,
                        purpose: "summary",
                        person: Some(principal.caller().user()),
                    },
                )
                .await
                .map_err(ApiError::engine)?
            {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "A usage cap has no room for summaries",
                ));
            }
            Ok(())
        })
    }
}

impl effects::ApplyOwned for Refresher {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Refresh,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<SummaryReceipt>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            let target = value.selected.material.identity.target();
            let origin =
                CapturedOperation::capture(principal.caller()).map_err(ApiError::internal)?;
            let http = native::http_origin::capture_operation(ctx, &mut tx, &origin).await?;
            let accepted = serde_json::to_value(http.origin()).map_err(ApiError::internal)?;
            let arguments = serde_json::to_value(&value.body).map_err(ApiError::internal)?;
            let selection = value
                .selected
                .backend_selection()
                .map_err(ApiError::engine)?;
            let source = json!({"target":target,"range":value.selected.pinned_range(),"snapshot":value.selected.material.snapshot,"policy":value.selected.policy.fingerprint,"http":http,
                "selection":selection});
            before.check()?;
            let cache = rows::ensure(
                &mut tx,
                target,
                Some(principal.caller().user()),
                ctx.app.clock.now(),
            )
            .await?
            .ok_or_else(ApiError::not_found)?;
            let accepted = rows::enqueue(
                &mut tx,
                cache.id,
                rows::Origin::Person {
                    issuer: principal.caller().user(),
                    client_id: value.body.client_id,
                    accepted_operation: &accepted,
                },
                &arguments,
                &source,
                ctx.app.clock.now(),
                ctx.app.clock.now(),
            )
            .await?;
            let (work, changed) = match accepted {
                rows::Enqueued::New(row) => {
                    if value.body.model.is_none() {
                        resource::pool::accept(
                            &ctx.app.engine,
                            &mut tx,
                            principal,
                            &row,
                            &value.selected,
                        )
                        .await
                        .map_err(ApiError::engine)?;
                    }
                    (row, true)
                }
                rows::Enqueued::Replay(row) => (row, false),
                rows::Enqueued::Busy => {
                    return Err(ApiError::new(
                        ErrorCode::ConflictExists,
                        "A summary refresh is already in progress",
                    ));
                }
                rows::Enqueued::Conflict | rows::Enqueued::WrongOrigin => {
                    return Err(ApiError::new(
                        ErrorCode::ConflictIdempotencyMismatch,
                        "This refresh identity was used for a different request",
                    ));
                }
            };
            let revision = rows::get(&mut tx, target, Some(principal.caller().user()))
                .await?
                .map_or(cache.revision, |cache| cache.revision);
            deadline.check()?;
            tx.commit().await.map_err(DbError::from)?;
            let receipt = SummaryReceipt {
                projection: ReceiptProjection::Receipt,
                id: work.id,
                target,
                revision,
            };
            Ok(if changed {
                effects::Outcome::Applied(receipt)
            } else {
                effects::Outcome::Unchanged(receipt)
            })
        })
    }
}

async fn refresh(ctx: Ctx, _: (), _: SummaryRefreshBody) -> ApiResult<SummaryReceipt> {
    let (tx, _) = transaction_guard::begin(&ctx).await?;
    let done = effects::apply_owned::<Refresher>(&ctx, tx).await?;
    publication::receipt(&done.ctx, &done.output)?;
    Ok(done.output)
}

#[path = "summaries/batch.rs"]
pub mod batch;

#[path = "summaries/cache_read.rs"]
pub mod cache_read;

#[path = "summaries/embedded.rs"]
pub mod embedded;

#[path = "summaries/follow.rs"]
pub mod follow;
