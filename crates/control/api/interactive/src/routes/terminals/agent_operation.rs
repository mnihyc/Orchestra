//! The stored agent-terminal subject, with once-only native ticket issuance.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::native::effects::Apply;

use crate::key_operations::native::effects::Effect;

use crate::key_operations::native::effects::FirstEffect;

use crate::key_operations::native::effects::Outcome;

use crate::key_operations::targets;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::terminal_sources as sources;

use orch_api_contract::terminal_sources::Source;

use orch_authz::Caller;
pub(super) struct Agent;

pub(super) struct Subject {
    thread: Id,
    environment: Id,
    agent: Option<Id>,
    control: bool,
}

impl Effect for Agent {
    type Source = Source;
    type Query = ();
    type Body = AgentTerminalTicketCreateBody;
    type Value = Subject;
    type Output = crate::middleware::idempotency::memory::Reservation<TerminalTicket>;

    fn declaration(value: &Subject) -> &'static Declaration<Source> {
        if value.control {
            &sources::AGENT_CONTROL
        } else {
            &sources::AGENT_VIEW
        }
    }

    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _caller: &'a Caller,
        (): (),
        body: AgentTerminalTicketCreateBody,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let thread = ctx.id("id")?;
            let row = threads::thread(conn, thread)
                .await?
                .filter(|row| row.deleted_at.is_none())
                .ok_or_else(ApiError::not_found)?;
            let environment = row.environment_id.ok_or_else(ApiError::not_found)?;
            let target = targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: environment,
                },
            )
            .await?;
            Ok((
                Subject {
                    thread,
                    environment,
                    agent: row.agent_id,
                    control: body.control.unwrap_or(false),
                },
                vec![native::Facts::one(
                    Source::AgentEnvironment,
                    KeyTargetKind::Object(ObjectKind::Environment),
                    target,
                )],
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            authorization::require_resource(
                conn,
                principal,
                ResourceKind::Environment,
                value.environment,
                Permission::TerminalView,
                Policy::default(),
            )
            .await?;
            if value.control {
                authorization::require_resource(
                    conn,
                    principal,
                    ResourceKind::Environment,
                    value.environment,
                    Permission::TerminalManage,
                    Policy::default(),
                )
                .await?;
            }
            let screen = ctx
                .app
                .engine
                .shells()
                .agent_screen(conn, value.thread)
                .await
                .map_err(ApiError::internal)?;
            let fits = match screen.map(|screen| screen.declared) {
                Some(AgentTerminal::Interactive) => true,
                Some(AgentTerminal::View) => !value.control,
                Some(AgentTerminal::None) | None => false,
            };
            if !fits {
                return Err(ApiError::new(
                    ErrorCode::CapabilityUnsupported,
                    if value.control {
                        "the agent declares no terminal that can be typed into"
                    } else {
                        "the agent declares no terminal of its own"
                    },
                ));
            }
            Ok(())
        })
    }
}

impl Apply for Agent {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a Subject,
        before: FirstEffect<'a>,
    ) -> BoxFuture<
        'a,
        ApiResult<Outcome<crate::middleware::idempotency::memory::Reservation<TerminalTicket>>>,
    > {
        Box::pin(async move {
            before.check()?;
            let mut ticket = ctx.app.realtime.terminals().issue_agent_pending(
                ctx,
                value.thread,
                value.environment,
                value.agent,
                value.control,
            )?;
            crate::routes::interactive_tickets::record::<
                crate::routes::interactive_tickets::AgentTerminalTickets,
            >(&mut ticket, ctx, conn)
            .await?;
            Ok(Outcome::Applied(ticket))
        })
    }
}

pub(super) async fn run(ctx: Ctx) -> ApiResult<api::TerminalTicketCreateResponse> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    guard.check_deadline(&ctx)?;
    if threads::lock(&mut tx, ctx.id("id")?).await?.is_none() {
        return Err(ApiError::not_found());
    }
    let completed = effects::apply::<Agent>(&ctx, &mut tx).await?;
    let active = completed.ctx;
    let ticket = completed.output;
    let (current, final_guard) = transaction_guard::after_wait(&active, &mut tx).await?;
    final_guard.check_deadline(&current)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    final_guard.check_deadline(&current)?;
    Ok(api::TerminalTicketCreateResponse::Full(Box::new(
        ticket.publish()?,
    )))
}
