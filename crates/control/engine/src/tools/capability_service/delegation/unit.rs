//! A real owned input admits its actual node placement; private metadata never creates a caller.
use super::*;
use orch_db::queries::{environments::hosting as runtimes, resources::hosting as hosts};
use orch_driver_contract::network::{NetworkTransport, TransportPurpose};
use orch_link_contract::{
    commands::{CommandKind, NodeProgramAdmit},
    program_work::Admission,
};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Plan {
    pub(crate) node: Id,
    pub(crate) admission: Admission,
    host: Id,
    generation: i32,
}
impl std::fmt::Debug for Plan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgramPlacement")
            .field("node", &self.node)
            .field("admission", &self.admission)
            .finish_non_exhaustive()
    }
}

fn unavailable(message: &str) -> EngineError {
    EngineError::unreadable(message)
}

fn endpoints(
    descriptor: &orch_agent_contract::descriptor::AgentDescriptor,
) -> Result<Vec<NetworkTransport>, EngineError> {
    let origins = descriptor
        .extensions
        .get("x_program_work")
        .and_then(|value| value.get("origins"))
        .and_then(Json::as_array)
        .ok_or_else(|| {
            unavailable("This program has not declared its isolated provider transports.")
        })?;
    let mut endpoints = Vec::with_capacity(origins.len());
    for origin in origins {
        let url = url::Url::parse(
            origin
                .as_str()
                .ok_or_else(|| unavailable("A program transport is invalid."))?,
        )
        .map_err(|_| unavailable("A program transport is invalid."))?;
        if !matches!(url.scheme(), "https" | "http" | "wss" | "ws")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
        {
            return Err(unavailable(
                "A program transport must name only its provider origin.",
            ));
        }
        endpoints.push(NetworkTransport {
            authority: url
                .host_str()
                .ok_or_else(|| unavailable("A program transport has no host."))?
                .to_lowercase(),
            protocol: url.scheme().into(),
            port: url
                .port_or_known_default()
                .ok_or_else(|| unavailable("A program transport has no port."))?,
            purpose: TransportPurpose::Model,
        });
    }
    endpoints.sort();
    endpoints.dedup();
    if endpoints.is_empty() {
        return Err(unavailable(
            "This program has no isolated provider transport.",
        ));
    }
    Ok(endpoints)
}

impl Plan {
    async fn capture(conn: &mut Conn, parent: &ParentLink, peer: Id) -> Result<Self, EngineError> {
        let record = parent.record(conn).await?;
        let ids:Vec<Id>=sqlx::query_scalar("SELECT id FROM runtimes WHERE machine_id=$1 AND state='running' AND successor_of IS NULL ORDER BY id")
            .bind(peer).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
        let rows = runtimes::runtimes_by(conn, &ids).await?;
        let [runtime] = rows.as_slice() else {
            return Err(unavailable(
                "This placement has no single owned node unit for isolated capability work.",
            ));
        };
        if runtime.kind == "provider" && runtime.provider_id != Some(record.provider) {
            return Err(unavailable(
                "The program unit does not serve this selected provider.",
            ));
        }
        if !matches!(runtime.kind.as_str(), "provider" | "environment") {
            return Err(unavailable(
                "The program placement cannot provide an isolated unit.",
            ));
        }
        let host = hosts::host(conn, runtime.host_id)
            .await?
            .filter(|host| {
                host.online
                    && !host.disabled
                    && host.lost_at.is_none()
                    && host.machine_status == "active"
            })
            .ok_or_else(|| unavailable("The node for isolated capability work is unavailable."))?;
        let descriptor: orch_driver_contract::descriptor::DriverDescriptor =
            serde_json::from_value(
                host.drivers
                    .get(&runtime.driver)
                    .cloned()
                    .ok_or_else(|| unavailable("The program placement's driver is unavailable."))?,
            )?;
        if !descriptor.program_units.supported {
            return Err(unavailable(
                descriptor
                    .program_units
                    .reason
                    .as_deref()
                    .unwrap_or("This placement does not offer isolated capability units."),
            ));
        }
        let agent = resources::thread_engine::agent(conn, record.agent)
            .await?
            .ok_or(EngineError::StaleToolSource)?;
        let program =
            crate::harnesses::descriptor_of(&agent)?.ok_or(EngineError::StaleToolSource)?;
        Ok(Self {
            node: host.machine_id,
            host: host.id,
            generation: runtime.generation,
            admission: Admission {
                work: parent.work,
                helper: parent.helper,
                peer,
                parent: runtime.id,
                endpoints: endpoints(&program)?,
                deadline: record.deadline,
            },
        })
    }
    pub(super) async fn current(
        &self,
        conn: &mut Conn,
        parent: &ParentLink,
        clock: &dyn Clock,
    ) -> Result<bool, EngineError> {
        if clock.time() >= self.admission.deadline {
            return Ok(false);
        }
        match Self::capture(conn, parent, self.admission.peer).await {
            Ok(current) => Ok(current == *self),
            Err(EngineError::Unreadable(_) | EngineError::StaleToolSource) => Ok(false),
            Err(error) => Err(error),
        }
    }
}

impl Engine {
    pub(crate) async fn prepare_program_body(
        &self,
        snapshot: &crate::sessions::StartSnapshot,
        body: &mut orch_link_contract::commands::SessionOpen,
    ) -> Result<(), EngineError> {
        if snapshot
            .descriptor
            .extensions
            .get("x_program_work")
            .is_none()
        {
            return Ok(());
        }
        let Some(plan) = self.prepare_program_unit(snapshot).await? else {
            return Ok(());
        };
        let mut more = serde_json::Map::new();
        more.insert(
            "service".into(),
            json!(
                orch_link_contract::tunnels::Service::Program {
                    work_id: plan.admission.work
                }
                .to_string()
            ),
        );
        more.insert("dst".into(), json!(plan.node));
        more.insert("thread_id".into(), json!(plan.admission.helper));
        more.insert("expires_at".into(), json!(plan.admission.deadline));
        body.program_work = Some(orch_link_contract::program_work::Session {
            work: plan.admission.work,
            helper: plan.admission.helper,
            node: plan.node,
            ticket: orch_link_contract::commands::TicketSlot::Asked(
                orch_link_contract::commands::TicketAsk {
                    ticket: orch_link_contract::commands::TicketWanted {
                        kind: orch_link_contract::tunnels::TunnelKind::Program,
                        more,
                    },
                },
            ),
        });
        // The owned initial request is complete. No inherited private topic material becomes another input.
        body.instructions.clear();
        body.memory = None;
        body.mounts.clear();
        body.mcp.clear();
        body.skills.clear();
        body.orch_tools.clear();
        body.orch_tool_descriptions.clear();
        body.orch_tool_schemas.clear();
        body.workspace_configuration = None;
        body.workspace_root = "/work".into();
        body.directory = "/work".into();
        body.cwd = "/work".into();
        body.instruction_root_marker = false;
        body.exec.ticket = None;
        body.exec.executor = None;
        body.exec.tool_set = None;
        body.approval_mode = orch_link_contract::commands::ApprovalMode::Ask;
        body.sandbox_mode = orch_link_contract::commands::SandboxMode::ReadOnly;
        body.background_wake = false;
        Ok(())
    }

    /// The actual initial input is the only caller of this producer; no public work ids can admit it.
    pub(crate) async fn prepare_program_unit(
        &self,
        snapshot: &crate::sessions::StartSnapshot,
    ) -> Result<Option<Plan>, EngineError> {
        let Some(parent) =
            crate::inputs::saved_authority(snapshot.input.submission_authority.as_ref())?
                .and_then(|authority| authority.capability_parent().cloned())
        else {
            return Ok(None);
        };
        if snapshot.thread.node_id != parent.helper
            || snapshot.input.thread_id != parent.helper
            || snapshot.turn.thread_id != parent.helper
            || snapshot.turn.input_id != snapshot.input.id
        {
            return Err(EngineError::StaleToolSource);
        }
        let (plan, command) = {
            let mut tx = self.db().begin().await?;
            if !parent.owns_input(&mut tx, snapshot.input.id).await?
                || !parent
                    .current_use(
                        &mut tx,
                        self.inner.clock.as_ref(),
                        self.inner.access.as_ref(),
                    )
                    .await?
            {
                return Err(EngineError::StaleToolSource);
            }
            let proposed = Plan::capture(&mut tx, &parent, snapshot.machine.id).await?;
            let kept:Json=sqlx::query_scalar("UPDATE delegated_program_work SET unit_admission=COALESCE(unit_admission,$2) WHERE id=$1 AND initial_input_id=$3 RETURNING unit_admission")
                .bind(parent.work).bind(serde_json::to_value(&proposed)?).bind(snapshot.input.id).fetch_one(&mut *tx).await.map_err(orch_db::DbError::from)?;
            let plan: Plan = serde_json::from_value(kept)?;
            if proposed != plan {
                return Err(unavailable(
                    "The original capability unit placement changed; it was not replaced.",
                ));
            }
            let mut command = Outgoing::new(
                crate::ids::derived("capability.unit.admit", &[parent.work.as_bytes()]),
                plan.node,
                SerialKey::Runtime(parent.work),
                &NodeProgramAdmit {
                    runtime_id: parent.work,
                    work: plan.admission.clone(),
                },
            )?;
            command.deadline = Some(plan.admission.deadline);
            let authority = SessionAuthority {
                parent: parent.clone(),
                input: snapshot.input.id,
                turn: snapshot.turn.id,
                segment: snapshot.segment.id,
                command: command.id,
                machine: command.machine,
                peer: snapshot.machine.id,
                unit: Some(plan.clone()),
                body: source::hash_json(command.body.clone())?,
                kind: CommandKind::NodeProgramAdmit.as_str().into(),
                deadline: plan.admission.deadline,
            };
            authority
                .current_issue_callers(
                    &mut tx,
                    &link::NewCommand {
                        id: command.id,
                        machine_id: command.machine,
                        kind: command.kind.as_str(),
                        body: &command.body,
                        serial_key: command.key.wire().as_deref(),
                        deadline_at: plan.admission.deadline.get(),
                    },
                    self.inner.clock.as_ref(),
                    self.inner.access.as_ref(),
                )
                .await?
                .ok_or(EngineError::StaleToolSource)?;
            command.authority = Some(CommandAuthority::CapabilitySession(Box::new(authority)));
            tx.commit().await.map_err(orch_db::DbError::from)?;
            (plan, command)
        };
        match crate::commands::run(self.inner.commands.as_ref(), command).await? {
            crate::commands::Outcome::Succeeded(_) => {
                let mut tx = self.db().begin().await?;
                if !parent
                    .current_use(
                        &mut tx,
                        self.inner.clock.as_ref(),
                        self.inner.access.as_ref(),
                    )
                    .await?
                    || !plan
                        .current(&mut tx, &parent, self.inner.clock.as_ref())
                        .await?
                {
                    return Err(EngineError::StaleToolSource);
                }
                tx.commit().await.map_err(orch_db::DbError::from)?;
                Ok(Some(plan))
            }
            _ => Err(unavailable(
                "The node could not admit this isolated capability work.",
            )),
        }
    }
}
