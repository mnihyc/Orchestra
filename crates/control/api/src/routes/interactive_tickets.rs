//! Same native ticket bindings at initial admission and current delivery, from stored Environment.

use crate::{
    key_operations::{admission::Prepared, targets},
    middleware::requests::{ApiError, ApiResult},
    registry::Ctx,
};
use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
use orch_db::Conn;
use orch_types::Id;

pub(crate) async fn core(
    conn: &mut Conn,
    selection: &str,
    environment: Id,
) -> ApiResult<orch_engine::operation_scope::Prepared> {
    let target = targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
    )
    .await?;
    let mut plan = orch_engine::operation_scope::Prepared::for_selection(selection)
        .map_err(ApiError::internal)?;
    let count = plan.variant().facets.len();
    for index in 0..count {
        plan.bind(index, target.object.kind, std::slice::from_ref(&target))
            .map_err(ApiError::internal)?;
    }
    Ok(plan)
}

pub(super) async fn plan(
    ctx: &Ctx,
    conn: &mut Conn,
    selection: &str,
    environment: Id,
) -> ApiResult<Prepared> {
    let core = core(conn, selection, environment).await?;
    if !core.matches_route(&ctx.route) {
        return Err(ApiError::internal(
            "ticket binding belongs to another operation",
        ));
    }
    Ok(Prepared::from_shared(core).with_data(environment))
}

/// A once-only native capability's shape; its existing store supplies the actual identity.
pub(crate) trait Creator: Send + Sync + 'static {
    type Body: Send + 'static;
    type Full: Send;
    type Reply: Send + 'static;
    const KIND: &'static str;
    fn expires(value: &Self::Full) -> orch_types::Time;
    fn receipt(value: orch_api_contract::common::TicketCreateReceipt) -> Self::Reply;
}
pub(crate) struct BrowserTickets;
impl Creator for BrowserTickets {
    type Body = orch_api_contract::browsers::BrowserTicketCreateBody;
    type Full = orch_api_contract::browsers::BrowserTicketCreateReply;
    type Reply = orch_api_contract::browsers::BrowserTicketCreateResponse;
    const KIND: &'static str = "browser_ticket";
    fn expires(value: &Self::Full) -> orch_types::Time {
        value.ticket_expires_at
    }
    fn receipt(value: orch_api_contract::common::TicketCreateReceipt) -> Self::Reply {
        Self::Reply::Receipt(value)
    }
}
pub(crate) struct DisplayTickets;
impl Creator for DisplayTickets {
    type Body = orch_api_contract::displays::DisplayTicketCreateBody;
    type Full = orch_api_contract::displays::DisplayTicketCreateReply;
    type Reply = orch_api_contract::displays::DisplayTicketCreateResponse;
    const KIND: &'static str = "display_ticket";
    fn expires(value: &Self::Full) -> orch_types::Time {
        value.ticket_expires_at
    }
    fn receipt(value: orch_api_contract::common::TicketCreateReceipt) -> Self::Reply {
        Self::Reply::Receipt(value)
    }
}
pub(crate) struct TerminalTickets;
impl Creator for TerminalTickets {
    type Body = ();
    type Full = orch_api_contract::terminals::TerminalTicket;
    type Reply = orch_api_contract::terminals::TerminalTicketCreateResponse;
    const KIND: &'static str = "terminal_ticket";
    fn expires(value: &Self::Full) -> orch_types::Time {
        value.ticket_expires_at
    }
    fn receipt(value: orch_api_contract::common::TicketCreateReceipt) -> Self::Reply {
        Self::Reply::Receipt(value)
    }
}
pub(crate) struct AgentTerminalTickets;
impl Creator for AgentTerminalTickets {
    type Body = orch_api_contract::terminals::AgentTerminalTicketCreateBody;
    type Full = orch_api_contract::terminals::TerminalTicket;
    type Reply = orch_api_contract::terminals::TerminalTicketCreateResponse;
    const KIND: &'static str = "agent_terminal_ticket";
    fn expires(value: &Self::Full) -> orch_types::Time {
        value.ticket_expires_at
    }
    fn receipt(value: orch_api_contract::common::TicketCreateReceipt) -> Self::Reply {
        Self::Reply::Receipt(value)
    }
}
pub(crate) struct PortUses;
impl Creator for PortUses {
    type Body = orch_api_contract::ports::EnvironmentPortUseCreateBody;
    type Full = orch_api_contract::ports::PortUse;
    type Reply = orch_api_contract::ports::PortUseCreateResponse;
    const KIND: &'static str = "port_use";
    fn expires(value: &Self::Full) -> orch_types::Time {
        value.ticket_expires_at
    }
    fn receipt(value: orch_api_contract::common::TicketCreateReceipt) -> Self::Reply {
        Self::Reply::Receipt(value)
    }
}
pub(crate) struct PortEntries;
impl Creator for PortEntries {
    type Body = orch_api_contract::ports::EnvironmentPortEnterBody;
    type Full = orch_api_contract::ports::EnvironmentPortEnterReply;
    type Reply = orch_api_contract::ports::EnvironmentPortEnterResponse;
    const KIND: &'static str = "port_entry";
    fn expires(value: &Self::Full) -> orch_types::Time {
        value.expires_at
    }
    fn receipt(value: orch_api_contract::common::TicketCreateReceipt) -> Self::Reply {
        Self::Reply::Receipt(value)
    }
}
pub(crate) fn creation<C: Creator>()
-> crate::middleware::idempotency::ObservedCreation<(), C::Body, C::Reply> {
    crate::middleware::idempotency::ObservedCreation {
        kind: C::KIND,
        replay: |_ctx, (), _body, created| {
            Box::pin(async move {
                if created.kind != C::KIND {
                    return Err(ApiError::internal(
                        "the native ticket belongs to another creation kind",
                    ));
                }
                let expires = serde_json::from_value(created.data["ticket_expires_at"].clone())
                    .map_err(ApiError::internal)?;
                Ok(C::receipt(orch_api_contract::common::TicketCreateReceipt {
                    projection: orch_api_contract::common::ReceiptProjection::Receipt,
                    ticket_id: created.id,
                    ticket_expires_at: expires,
                }))
            })
        },
    }
}
pub(crate) async fn record<C: Creator>(
    ticket: &mut crate::middleware::idempotency::memory::Reservation<C::Full>,
    ctx: &Ctx,
    conn: &mut Conn,
) -> ApiResult<()> {
    let expires = C::expires(ticket.value()?);
    ticket
        .record(
            ctx,
            conn,
            C::KIND,
            serde_json::json!({"ticket_expires_at":expires}),
        )
        .await
}
