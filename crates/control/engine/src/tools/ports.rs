//! The four port tools: `list_ports`, `open_port`, `close_port` and `list_port_connections`.
//!
//! What a call does is the API's: the projection of an environment's ports and stable addresses, the opening a
//! thread asks for and the person's decision on it, the closing of an opening the thread asked for, and the live
//! callers of an environment's ports, each as the HTTP requests of 03 9.10 answer them. The engine reaches it through
//! the hook the program it lives in attaches ([`PortTools`]), as it reaches what making a unit needs; until one is
//! attached a call is answered as Orch's own failure. The thread's admitted context selects the environment: a
//! call names no other one.

use async_trait::async_trait;
use orch_db::queries::threads::ThreadRow;
use orch_types::Json;
use std::sync::{Arc, PoisonError};

use super::definitions::{Code, Refusal};
use super::{Answer, CallContext};
use crate::engine::Engine;
use crate::error::EngineError;
use orch_agent_contract::events::OrchTool;
use orch_db::Conn;

/// Which of the four a thread's current environment lets it be offered now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offer {
    /// The two reads: the environment has the ports door (Ports on at its home and on its machine, its owner
    /// holding Ports View there) and its native discovery is supported.
    pub reads: bool,
    /// `open_port` as well: at least one audience is neither left out by the template nor denied by
    /// `capability.policy`.
    pub open: bool,
}

impl Offer {
    /// Nothing but closing what the thread asked for earlier.
    pub const NONE: Self = Self {
        reads: false,
        open: false,
    };
}

/// What the thread's current environment permits offering: the port reads, and new requests where some
/// audience may be opened.
pub async fn offered(
    conn: &mut Conn,
    thread: &ThreadRow,
    now: orch_types::Time,
) -> Result<Offer, EngineError> {
    let unreadable = |error: crate::ports::Error| EngineError::unreadable(error.to_string());
    let Some(id) = thread.environment_id else {
        return Ok(Offer::NONE);
    };
    let Some(env) = orch_db::queries::environments::routes::environments(conn, &[id])
        .await?
        .into_iter()
        .next()
    else {
        return Ok(Offer::NONE);
    };
    let Some(machine) = env.machine_id else {
        return Ok(Offer::NONE);
    };
    let Some(machine) = orch_db::queries::resources::machine(conn, machine).await? else {
        return Ok(Offer::NONE);
    };
    // The native reader currently proves ownership on Linux; other platforms report unsupported.
    if machine.platform_os != "linux" {
        return Ok(Offer::NONE);
    }
    let principal = orch_authz::Principal::load(
        conn,
        orch_authz::Caller::token(thread.owner_id, orch_authz::TokenGrant::everything()),
        now,
    )
    .await?;
    if !crate::ports::holds(
        conn,
        &principal,
        id,
        orch_access_contract::permissions::Permission::PortsView,
    )
    .await
    .map_err(unreadable)?
    {
        return Ok(Offer::NONE);
    }
    let policy = crate::ports::policies(conn, id).await.map_err(unreadable)?;
    if !policy.enabled
        || !crate::ports::local_enabled(conn, &env)
            .await
            .map_err(unreadable)?
    {
        return Ok(Offer::NONE);
    }
    let mut open = false;
    for gate in crate::port_capabilities::gates(conn, id)
        .await
        .map_err(unreadable)?
    {
        open |= gate.permits();
    }
    Ok(Offer { reads: true, open })
}

/// Narrows the offered tools to what the environment permits: without the door only `close_port` stays (closing
/// what the thread asked for earlier reduces exposure and is never refused), and without an audience that may be
/// opened `open_port` goes while the reads stay.
pub fn narrow(tools: &mut Vec<OrchTool>, offer: Offer) {
    if !offer.reads {
        tools.retain(|tool| {
            !matches!(
                tool,
                OrchTool::ListPorts | OrchTool::OpenPort | OrchTool::ListPortConnections
            )
        });
    } else if !offer.open {
        tools.retain(|tool| *tool != OrchTool::OpenPort);
    }
}

/// What the program the engine lives in does for the four port tools, for the agent of `thread` with the call's
/// `arguments` (checked against the tool's schema already). The answer is the tool's one structured object, or a
/// refusal that says what to do instead.
#[async_trait]
pub trait PortTools: Send + Sync {
    /// `list_ports`.
    async fn list_ports(&self, thread: &ThreadRow, arguments: &Json) -> Result<Json, Refusal>;
    /// `open_port`: waits for a person's decision at most sixty seconds, then answers the opening as it stands.
    async fn open_port(&self, thread: &ThreadRow, arguments: &Json) -> Result<Json, Refusal>;
    /// `close_port`.
    async fn close_port(&self, thread: &ThreadRow, arguments: &Json) -> Result<Json, Refusal>;
    /// `list_port_connections`.
    async fn list_port_connections(
        &self,
        thread: &ThreadRow,
        arguments: &Json,
    ) -> Result<Json, Refusal>;
}

impl Engine {
    /// Persists a late port result before steering it, or leaves it for the next real input without starting a turn.
    pub async fn port_notice_in(
        &self,
        unit: &mut crate::unit::Unit<'_>,
        thread: &ThreadRow,
        opening: orch_types::Id,
        text: &str,
    ) -> Result<(), EngineError> {
        use crate::ingest::{ItemSpec, add_item};
        use serde_json::json;
        let key = format!("port-result:{opening}");
        let input = crate::ids::derived("port.outcome", &[opening.as_bytes()]);
        if let Some(segment) = thread.current_segment_id {
            let payload = json!({"parts":[{"type":"text","text":text}],"input_id":input,"delivery":"steer","source":"system"});
            add_item(
                unit,
                &ItemSpec {
                    thread: thread.node_id,
                    segment,
                    turn: None,
                    stream: "main",
                    parent: None,
                    kind: "user_message",
                    status: "completed",
                    author: None,
                    payload: &payload,
                    native_ref: Some(&key),
                },
            )
            .await?;
        }
        let turn = orch_db::queries::threads::active_turn(unit.conn, thread.node_id).await?;
        if let Some(turn) = turn
            && turn.state == "running"
            && self.agent_takes_a_steer(unit, thread).await?
        {
            self.queue_announcement_input(
                unit,
                thread.node_id,
                input,
                text,
                &[],
                crate::inputs::Arrival::Steer { turn: turn.id },
            )
            .await?;
        } else {
            let notice = json!({"id":key,"kind":"port_outcome","text":text,"at":orch_types::Time::new(unit.now())});
            if orch_db::queries::threads::notices::append(unit.conn, thread.node_id, &notice)
                .await?
            {
                let seq = unit.stamp(thread.node_id).await?;
                orch_db::queries::threads::set_seq_state(unit.conn, thread.node_id, seq).await?;
            }
        }
        Ok(())
    }

    /// Attaches what answers the four port tools. What was attached before is replaced.
    pub fn attach_port_tools(&self, tools: Arc<dyn PortTools>) {
        *self
            .inner
            .port_tools
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(tools);
    }

    /// What answers the four port tools, if it was attached.
    #[must_use]
    pub fn port_tools(&self) -> Option<Arc<dyn PortTools>> {
        self.inner
            .port_tools
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Answers a call of one of the four port tools.
    pub(crate) async fn port_tool_call(
        &self,
        call: &CallContext,
        thread: &ThreadRow,
    ) -> Result<Answer, EngineError> {
        let Some(tools) = self.port_tools() else {
            return Ok(Refusal::new(
                Code::Unsupported,
                format!(
                    "{} is not served by this system's process now; nothing was changed. Try again later.",
                    call.tool.name()
                ),
            )
            .into());
        };
        let outcome = match call.tool {
            OrchTool::ListPorts => tools.list_ports(thread, &call.arguments).await,
            OrchTool::OpenPort => tools.open_port(thread, &call.arguments).await,
            OrchTool::ClosePort => tools.close_port(thread, &call.arguments).await,
            _ => tools.list_port_connections(thread, &call.arguments).await,
        };
        Ok(match outcome {
            Ok(result) => Answer::Done(result),
            Err(refusal) => Answer::Refused(refusal),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_without_enabled_ports_can_still_reduce_its_earlier_exposure() {
        let mut tools = vec![
            OrchTool::ListShared,
            OrchTool::ListPorts,
            OrchTool::OpenPort,
            OrchTool::ClosePort,
            OrchTool::ListPortConnections,
        ];
        narrow(&mut tools, Offer::NONE);
        assert_eq!(tools, vec![OrchTool::ListShared, OrchTool::ClosePort]);
    }

    #[test]
    fn where_every_audience_is_denied_the_reads_stay_and_open_port_goes() {
        let all = vec![
            OrchTool::ListPorts,
            OrchTool::OpenPort,
            OrchTool::ClosePort,
            OrchTool::ListPortConnections,
        ];
        let mut tools = all.clone();
        narrow(
            &mut tools,
            Offer {
                reads: true,
                open: false,
            },
        );
        assert_eq!(
            tools,
            vec![
                OrchTool::ListPorts,
                OrchTool::ClosePort,
                OrchTool::ListPortConnections
            ]
        );
        let mut tools = all.clone();
        narrow(
            &mut tools,
            Offer {
                reads: true,
                open: true,
            },
        );
        assert_eq!(tools, all);
    }
}
