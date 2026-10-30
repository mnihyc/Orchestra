//! A thread's request to let more people reach a port, and a person's decision on its card.
//!
//! `POST /threads/{id}/port-requests` (and the agent's `open_port`, which shares it) asks for an
//! audience of one port of the thread's environment. Who may be let in is a capability of the
//! environment (`port_place`, `port_link`, `port_host`), so the request is decided as every
//! capability is: where the environment holds the audience's capability, or the policy at its home
//! allows it, the opening is made at once and keeps what admitted it, with no card; where the policy
//! denies it, or the template leaves it out, it is refused; otherwise it waits on a card, an
//! approval of kind `port` that whoever holds Ports Manage on the environment now may decide. The
//! same request asked again answers its opening and makes no second card, and a refused one is
//! refused again for an hour. The card admits the opening once, or admits it and grants the
//! audience's capability to the environment (so that later requests of that kind open without a
//! card), or refuses it. A link made for the thread is sealed for the thread's own reading; the
//! deciding person may keep it to themself instead. These are boundary requests: no harness approval
//! mode, reviewer, thread rule or Stop decides one.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::routes::ports::{
    address, check_program, environment, failure, host_candidate, opening_view, plan_of,
};
use orch_access_contract::permissions::Permission;
use orch_api_contract::approvals::{ApprovalDecision, ApprovalDecisionReply, ApprovalState};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::ports::{self as api, *};
use orch_authz::Principal;
use orch_db::Tx;
use orch_db::queries::environments::routes::EnvironmentFull;
use orch_db::queries::ports as rows;
use orch_engine::port_capabilities::{self, Gate};
use orch_engine::ports as core;
use orch_types::{Id, Json, Time, new_id};

/// Binds a thread's port request.
pub fn mount(registry: &mut Registry) {
    registry.on(api::ThreadPortRequestCreate, create);
}

/// The card's one line.
fn title_of(request: &PortOpeningRequest) -> String {
    let audience = match request.kind {
        PortOpeningKind::Place => "a named place",
        PortOpeningKind::Link => "a link's holder",
        PortOpeningKind::Host => "callers at the host",
    };
    format!("Open port {} to {audience}", request.port)
}

/// `POST /threads/{id}/port-requests`: opens the thread's request where a capability admits it,
/// records it as a waiting opening with its card otherwise, or answers the opening the same request
/// already has.
async fn create(ctx: Ctx, (): (), body: PortOpeningRequest) -> ApiResult<PortOpenReply> {
    let thread = ctx.id("id")?;
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    request_for_thread(&ctx, tx, guard.principal(), thread, body).await
}

/// Records the same thread request for HTTP and tools, after their own current authority guard.
pub(crate) async fn request_for_thread(
    ctx: &Ctx,
    tx: Tx,
    principal: &Principal,
    thread: Id,
    body: PortOpeningRequest,
) -> ApiResult<PortOpenReply> {
    request_for_thread_bounded(ctx, tx, principal, thread, body, None).await
}

/// Tools additionally fence the complete projected answer before the request's transaction commits.
pub(crate) async fn request_for_thread_bounded(
    ctx: &Ctx,
    mut tx: Tx,
    principal: &Principal,
    thread: Id,
    body: PortOpeningRequest,
    budget: Option<&Json>,
) -> ApiResult<PortOpenReply> {
    let row = orch_db::queries::threads::thread(&mut tx, thread)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let id = row.environment_id.ok_or_else(|| {
        ApiError::new(
            ErrorCode::PortsUnavailable,
            "this thread has no environment whose port could be opened",
        )
    })?;
    let env = environment(&mut tx, id).await?;
    let mut asked = body.clone();
    // An omitted named place is the thread's own topic.
    if asked.kind == PortOpeningKind::Place && asked.place_id.is_none() {
        asked.place_id = Some(row.topic_id);
    }
    let resolved = match core::resolve_request(
        &mut tx,
        principal,
        &env,
        &asked,
        address(ctx, id, asked.port).is_some(),
    )
    .await
    {
        Ok(resolved) => resolved,
        Err(error) => return Err(failure(&mut tx, error).await),
    };
    let full_body = serde_json::to_value(&body).map_err(ApiError::internal)?;
    let gate = match port_capabilities::gate(&mut tx, env.id, resolved.kind).await {
        Ok(gate) => gate,
        Err(error) => return Err(failure(&mut tx, error).await),
    };
    // Held by the environment, or allowed by the policy at its home: opened now, with no card.
    if gate.automatic() {
        return automatic(
            ctx, tx, principal, &env, thread, &full_body, &resolved, budget,
        )
        .await;
    }
    let waited = match core::request_waiting(
        &mut tx,
        &env,
        thread,
        new_id(),
        &full_body,
        &resolved,
        &title_of(&resolved),
        &Json::Null,
        principal.now(),
    )
    .await
    {
        Ok(waited) => waited,
        Err(error) => return Err(failure(&mut tx, error).await),
    };
    let opening = opening_view(ctx, &mut tx, principal, &waited.opening, &env, false).await?;
    if waited.created
        && let Some(approval) = waited.opening.approval_id
    {
        if budget.is_some() {
            sqlx::query("UPDATE approvals SET turn_id=$2 WHERE id=$1")
                .bind(approval)
                .bind(row.current_turn_id)
                .execute(&mut *tx)
                .await
                .map_err(orch_db::DbError::from)?;
        }
        let card = card_of(
            ctx,
            &mut tx,
            principal,
            &env,
            &resolved,
            &gate,
            opening.clone(),
        )
        .await?;
        rows::set_card(
            &mut tx,
            approval,
            &serde_json::to_value(&card).map_err(ApiError::internal)?,
        )
        .await?;
        tell_deciders(
            &mut tx,
            &row,
            approval,
            env.id,
            &title_of(&resolved),
            principal.user(),
            principal.now(),
        )
        .await?;
    }
    let request = match waited.opening.approval_id {
        Some(approval) => Some(request_ref(&mut tx, principal, approval, &env).await?),
        None => None,
    };
    let reply = PortOpenReply {
        opening,
        request,
        link: None,
    };
    if waited.created
        && let Some(arguments) = budget
    {
        crate::port_tools::check_open_budget(ctx, &waited.opening, &reply, arguments)?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(reply)
}

/// A new link's proof: what its holder is given, its copy sealed to the opening for the one intended
/// delivery, and the digest admission compares.
struct SealedLink {
    proof: String,
    sealed: Vec<u8>,
    digest: Vec<u8>,
}

/// Makes the proof of the link of `opening`.
fn sealed_link(opening: Id) -> ApiResult<SealedLink> {
    use base64::Engine;
    /// The random bytes of a link's proof: as many as a session token has, so that it cannot be guessed.
    const LINK_PROOF_BYTES: usize = 32;
    let proof = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(rand::random::<[u8; LINK_PROOF_BYTES]>());
    let sealed = crate::routes::resources::sealing::master_key()?
        .seal(proof.as_bytes(), opening.as_bytes())
        .map_err(ApiError::internal)?;
    let digest = orch_types::Hash::of(proof.as_bytes()).as_bytes().to_vec();
    Ok(SealedLink {
        proof,
        sealed,
        digest,
    })
}

/// Opens a thread's request at once, where the environment holds the audience's capability or the
/// policy at its home allows it: no person decides it and no card is made; the opening keeps what
/// admitted it. A link goes to the thread (its first answer, or the late line), and a host opening
/// takes the number its review finds now and is bound by its node after the commit. The same request
/// asked again answers the opening it has (a waiting or refused one included).
#[expect(
    clippy::too_many_arguments,
    reason = "the canonical guarded request and its optional tool-answer bound travel together"
)]
async fn automatic(
    ctx: &Ctx,
    mut tx: Tx,
    principal: &Principal,
    env: &EnvironmentFull,
    thread: Id,
    full_body: &Json,
    resolved: &PortOpeningRequest,
    budget: Option<&Json>,
) -> ApiResult<PortOpenReply> {
    let (current, revision) = rows::program(&mut tx, env.id, resolved.port).await?;
    if let Some(desired) = &resolved.program
        && *desired != current
    {
        check_program(ctx, &mut tx, env, resolved.port, &current, desired).await?;
    }
    let preparation = match resolved.kind {
        PortOpeningKind::Place => core::Preparation::Place,
        PortOpeningKind::Link => {
            let opening = new_id();
            let link = sealed_link(opening)?;
            core::Preparation::Link {
                id: opening,
                digest: link.digest,
                sealed: link.sealed,
                recipient: PortLinkRecipient::Thread,
                person: None,
            }
        }
        PortOpeningKind::Host => {
            match host_candidate(&ctx.app, &mut tx, env, resolved, principal.user()).await? {
                Ok(candidate) => core::Preparation::Host {
                    host: candidate.host.as_ref().map(|host| host.id),
                    machine: candidate.machine.id,
                    port: candidate.host_port,
                    revision: candidate.host_revision,
                },
                Err(blocker) => {
                    let code = ErrorCode::ALL
                        .iter()
                        .copied()
                        .find(|code| code.as_str() == blocker.code)
                        .unwrap_or(ErrorCode::PortsNoRange);
                    return Err(ApiError::new(code, blocker.message));
                }
            }
        }
    };
    let produced = match core::produce_automatic(
        &mut tx,
        principal,
        env,
        thread,
        new_id(),
        full_body,
        resolved,
        revision,
        &preparation,
        &crate::sign_in::origin(ctx),
    )
    .await
    {
        Ok(produced) => produced,
        Err(error) => return Err(failure(&mut tx, error).await),
    };
    let opening = opening_view(ctx, &mut tx, principal, &produced.opening, env, false).await?;
    // A request that coalesced with an earlier one still waiting on its card answers that card.
    let request = match produced.opening.approval_id {
        Some(approval) => Some(request_ref(&mut tx, principal, approval, env).await?),
        None => None,
    };
    let reply = PortOpenReply {
        opening,
        request,
        link: None,
    };
    if produced.created
        && let Some(arguments) = budget
    {
        crate::port_tools::check_open_budget(ctx, &produced.opening, &reply, arguments)?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    if produced.created && produced.opening.kind == "host" && produced.opening.host_allocated {
        crate::host_ports::publish_in_background(&ctx.app, produced.opening.id);
    }
    Ok(reply)
}

/// Tells everyone who may decide a new card, in the same transaction: those who hold Ports
/// Manage on the environment now, but the person who asked, as an approval is told (their
/// Inbox, and each route of theirs that takes approvals).
async fn tell_deciders(
    conn: &mut orch_db::Conn,
    thread: &orch_db::queries::threads::ThreadRow,
    approval: Id,
    environment: Id,
    title: &str,
    asker: Id,
    now: Time,
) -> ApiResult<()> {
    use orch_engine::notifications::{Event, Kind, tell};
    let people: Vec<Id> = core::port_deciders(conn, environment, asker, now)
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .filter(|person| *person != asker)
        .collect();
    let source = orch_db::queries::approvals::approval(conn, approval)
        .await?
        .and_then(|row| row.turn_id);
    let people = orch_authz::approvals::request_recipients(
        conn,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Environment,
            id: environment,
        },
        &people,
        source,
        now,
    )
    .await
    .map_err(ApiError::internal)?;
    let name = if thread.name.is_empty() {
        "A thread"
    } else {
        thread.name.as_str()
    };
    let event = Event {
        kind: Kind::ApprovalRequested,
        node: Some(thread.node_id),
        thread: Some(thread.node_id),
        approval: Some(approval),
        title: format!("{name} asks to open a port"),
        body: title.to_owned(),
        data: serde_json::json!({ "approval_id": approval, "kind": "port" }),
        source: approval.to_string(),
        actor: Some(asker),
        at: now.get(),
    };
    tell(conn, &event, &people, Some(thread.org_id))
        .await
        .map_err(ApiError::internal)?;
    Ok(())
}

/// What a card shows: the environment, the reviewed request, its waiting opening and review, whom a
/// link would go to, and whether it offers to grant the audience's capability to the environment.
async fn card_of(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    principal: &Principal,
    env: &EnvironmentFull,
    resolved: &PortOpeningRequest,
    gate: &Gate,
    opening: PortOpening,
) -> ApiResult<PortRequest> {
    let environment =
        crate::routes::resources::view::environment_refs(conn, &[env.id], &[], principal.now())
            .await?
            .into_iter()
            .next()
            .map(|(_, reference)| reference)
            .ok_or_else(ApiError::not_found)?;
    let link = resolved.kind == PortOpeningKind::Link;
    let plan = plan_of(
        ctx,
        conn,
        principal,
        env,
        resolved.port,
        resolved.clone(),
        link.then(|| {
            "The link becomes readable in the thread that asked, unless you keep it to yourself"
                .to_owned()
        }),
    )
    .await?;
    Ok(PortRequest {
        environment,
        request: resolved.clone(),
        opening,
        plan,
        link_recipient: PortLinkRecipient::Thread,
        recipient_warning: link.then(|| {
            "Allowing sends the link to the thread that asked: whoever reads that thread can use it"
                .to_owned()
        }),
        can_grant: gate.card_can_grant(),
    })
}

/// The short reference of a card, as its reader may act on it.
pub(crate) async fn request_ref(
    conn: &mut orch_db::Conn,
    principal: &Principal,
    approval: Id,
    env: &EnvironmentFull,
) -> ApiResult<PortRequestRef> {
    let row = orch_db::queries::approvals::approval(conn, approval)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let state = serde_json::from_value::<ApprovalState>(serde_json::json!(row.state))
        .unwrap_or(ApprovalState::Pending);
    Ok(PortRequestRef {
        id: row.id,
        state,
        can_decide: state == ApprovalState::Pending
            && core::holds(conn, principal, env.id, Permission::PortsManage)
                .await
                .map_err(ApiError::internal)?,
        expires_at: row.expires_at.map(Time::new),
    })
}

/// Decides a port card, by whoever holds Ports Manage on its environment now, for the opening
/// revision they reviewed: `once` admits this opening; `grant` admits it and grants the audience's
/// capability to the environment, which also needs the capability's approvers by its policy (the
/// same people as shipped), so that later requests of that kind open without a card; `deny` refuses
/// it. The answer carries the opening as the decision left it; a link the deciding person keeps to
/// themself (For me only) is in it once and nowhere else, and the thread never reads it.
pub(crate) async fn decide(
    ctx: &Ctx,
    mut tx: Tx,
    principal: Principal,
    id: Id,
    body: &ApprovalDecision,
) -> ApiResult<ApprovalDecisionReply> {
    let ApprovalDecision::Port(decision) = body else {
        return Err(ApiError::invalid(
            "decision",
            "a port request is decided with once, grant or deny",
        ));
    };
    let row = orch_db::queries::approvals::approval(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if row.state != "pending" {
        return Err(ApiError::new(
            ErrorCode::ApprovalAlreadyDecided,
            "this port request was decided already",
        ));
    }
    let waiting = rows::opening_of_approval(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let env = environment(&mut tx, waiting.environment_id).await?;
    if !core::holds(&mut tx, &principal, env.id, Permission::PortsManage)
        .await
        .map_err(ApiError::internal)?
    {
        return Err(ApiError::new(
            ErrorCode::PortsNotAllowed,
            "current Ports Manage on the environment decides its port requests",
        ));
    }
    if waiting.revision != decision.expected_revision {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the request changed since it was reviewed",
        ));
    }
    let card: PortRequest = serde_json::from_value(row.request.clone()).map_err(|_| {
        ApiError::new(
            ErrorCode::PortsUnavailable,
            "this card is not ready yet; read it again",
        )
    })?;
    let now = principal.now();
    let origin = crate::sign_in::origin(ctx);
    let decided = serde_json::to_value(decision).map_err(ApiError::internal)?;
    // The proof of a link the deciding person keeps: given back in this answer alone.
    let mut kept_proof = None;
    match decision.decision {
        PortDecisionChoice::Deny => {
            core::refuse_waiting(&mut tx, &waiting, "refused by a Ports manager", now)
                .await
                .map_err(ApiError::internal)?;
            rows::settle_approval(
                &mut tx,
                id,
                "denied",
                Some(principal.user()),
                &decided,
                now.get(),
            )
            .await?;
        }
        PortDecisionChoice::Once | PortDecisionChoice::Grant => {
            // The grant first, so that the opening is admitted under it: refused unless the template
            // allows the capability, the policy does not deny it and this person is among its approvers.
            let admission = if decision.decision == PortDecisionChoice::Grant {
                if let Err(error) = port_capabilities::grant(
                    &mut tx,
                    &principal,
                    env.id,
                    card.request.kind,
                    &origin,
                )
                .await
                {
                    return Err(failure(&mut tx, error).await);
                }
                // What admits the opening is the grant as it stands now: this person's, or the one
                // somebody gave while the card waited.
                port_capabilities::gate(&mut tx, env.id, card.request.kind)
                    .await
                    .map_err(ApiError::internal)?
                    .admission()
            } else {
                None
            };
            let preparation = match card.request.kind {
                PortOpeningKind::Place => core::Preparation::Place,
                PortOpeningKind::Link => {
                    let recipient = decision.link_recipient.unwrap_or(PortLinkRecipient::Thread);
                    let link = sealed_link(waiting.id)?;
                    // For me only: the link is the deciding person's, given back once below; the
                    // thread that asked is told the opening and never the link.
                    let person = (recipient == PortLinkRecipient::Person).then(|| principal.user());
                    if person.is_some() {
                        kept_proof = Some(link.proof);
                    }
                    core::Preparation::Link {
                        id: waiting.id,
                        digest: link.digest,
                        sealed: link.sealed,
                        recipient,
                        person,
                    }
                }
                PortOpeningKind::Host => {
                    // The number the card was reviewed with, exactly; a taken one needs a new card.
                    let candidate = card.plan.candidate.as_ref().ok_or_else(|| {
                        ApiError::new(
                            ErrorCode::PortsNoRange,
                            "the card found no host number: the thread asks again",
                        )
                    })?;
                    core::Preparation::Host {
                        host: candidate.host.as_ref().map(|host| host.id),
                        machine: candidate.machine.id,
                        port: candidate.host_port,
                        revision: candidate.host_revision,
                    }
                }
            };
            if let Err(error) = core::grant_waiting(
                &mut tx,
                &principal,
                crate::routes::ports::decision_credential(ctx, &principal)?,
                waiting.id,
                &card.request,
                card.plan.program_revision,
                &preparation,
                admission,
                &origin,
            )
            .await
            {
                return Err(failure(&mut tx, error).await);
            }
            rows::settle_approval(
                &mut tx,
                id,
                "approved",
                Some(principal.user()),
                &decided,
                now.get(),
            )
            .await?;
        }
    }
    // Nobody has anything left to do about the card: what asked them is marked read.
    orch_db::queries::notifications::inbox::settle_approval(&mut tx, id, now.get()).await?;
    let settled = orch_db::queries::approvals::approval(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    // The opening as this decision left it, its decision, and a kept link once.
    let decided_opening = rows::opening(&mut tx, waiting.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let opening = opening_view(ctx, &mut tx, &principal, &decided_opening, &env, false).await?;
    let request = request_ref(&mut tx, &principal, id, &env).await?;
    let link = match kept_proof {
        Some(proof) => {
            let port = opening.port;
            let helper = format!(
                "{}/api/v1/ports/links/{}#{proof}",
                ctx.app.config.system.public_url.trim_end_matches('/'),
                opening.id
            );
            let browser = if opening.entry_methods.contains(&PortEntryMethod::Browser) {
                address(ctx, env.id, port).map(|address| format!("{address}/?orch_token={proof}"))
            } else {
                None
            };
            // Given now, and never again: the sealed copy goes, and no read shows the proof.
            rows::delivered(&mut tx, opening.id).await?;
            Some(PortLinkDelivery {
                recipient: PortLinkRecipient::Person,
                url_once: Some(helper.clone()),
                browser_url_once: browser,
                helper_link_once: opening
                    .entry_methods
                    .contains(&PortEntryMethod::Helper)
                    .then_some(helper),
                delivered_to: "the deciding person, once".into(),
            })
        }
        None => None,
    };
    let readers = core::port_deciders(&mut tx, env.id, principal.user(), now)
        .await
        .map_err(ApiError::internal)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    // An admitted host opening is bound by its node after the commit.
    ctx.app.engine.notify_approval_changes(&readers);
    if decided_opening.kind == "host" && decided_opening.host_allocated {
        crate::host_ports::publish_in_background(&ctx.app, decided_opening.id);
    }
    let mut conn = ctx.app.db.acquire().await?;
    let approval = crate::routes::approvals::views(
        &mut conn,
        &principal,
        &[settled],
        ctx.now,
        &ctx.app.engine,
    )
    .await?
    .into_iter()
    .next()
    .ok_or_else(ApiError::not_found)?;
    Ok(ApprovalDecisionReply {
        approval,
        opening_result: Some(PortOpenReply {
            opening,
            request: Some(request),
            link,
        }),
    })
}
