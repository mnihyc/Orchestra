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

struct Site {
    node: Id,
    host: Id,
    runtime: Id,
    generation: i32,
}

async fn site(
    conn: &mut Conn,
    provider: Id,
    peer: Option<Id>,
    preview: bool,
    capability: &str,
) -> Result<Result<Site, CapabilityFailure>, EngineError> {
    // A stopped provider may be offered using its retained host and driver. An actual
    // launch must find the running parent on the very peer which will receive the ticket.
    let ids: Vec<Id> = sqlx::query_scalar(
        "SELECT id FROM runtimes WHERE successor_of IS NULL AND remove_at IS NULL AND (
            (machine_id=$1 AND state='running' AND (kind='environment' OR (kind='provider' AND provider_id=$2)))
            OR ($3 AND kind='provider' AND provider_id=$2 AND state IN ('stopped','starting','creating','running')
                AND (machine_id IS NULL OR $1 IS NULL OR machine_id=$1))) ORDER BY id"
    ).bind(peer).bind(provider).bind(preview).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
    let rows = runtimes::runtimes_by(conn, &ids).await?;
    let no_node = || {
        CapabilityFailure::unavailable(
            capability,
            "program_unit_unavailable",
            "This placement has no available node for an isolated provider unit.",
        )
    };
    let [runtime] = rows.as_slice() else {
        return Ok(Err(no_node()));
    };
    let Some(host) = hosts::host(conn, runtime.host_id).await?.filter(|host| {
        host.online && !host.disabled && host.lost_at.is_none() && host.machine_status == "active"
    }) else {
        return Ok(Err(no_node()));
    };
    let Some(driver) = host.drivers.get(&runtime.driver) else {
        return Ok(Err(no_node()));
    };
    let support: orch_driver_contract::descriptor::OperationSupport = driver
        .get("program_units")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    if !support.supported {
        return Ok(Err(CapabilityFailure::unavailable(
            capability,
            "program_unit_network_unavailable",
            support.reason.as_deref().unwrap_or(
                "This placement needs a restricted provider proxy and an isolated network.",
            ),
        )));
    }
    Ok(Ok(Site {
        node: host.machine_id,
        host: host.id,
        runtime: runtime.id,
        generation: runtime.generation,
    }))
}

pub(super) async fn availability(
    conn: &mut Conn,
    descriptor: &orch_agent_contract::descriptor::AgentDescriptor,
    provider: Id,
    peer: Option<Id>,
    capability: &str,
) -> Result<Result<(), CapabilityFailure>, EngineError> {
    if endpoints(descriptor).is_err() {
        return Ok(Err(CapabilityFailure::unavailable(
            capability,
            "program_execution_unavailable",
            "The program has no verified provider transports for isolated work.",
        )));
    }
    Ok(site(conn, provider, peer, true, capability)
        .await?
        .map(|_| ()))
}

impl Plan {
    async fn capture(conn: &mut Conn, parent: &ParentLink, peer: Id) -> Result<Self, EngineError> {
        let record = parent.record(conn).await?;
        let site = site(conn, record.provider, Some(peer), false, &record.capability)
            .await?
            .map_err(EngineError::from)?;
        let agent = resources::thread_engine::agent(conn, record.agent)
            .await?
            .ok_or(EngineError::StaleToolSource)?;
        let program =
            crate::harnesses::descriptor_of(&agent)?.ok_or(EngineError::StaleToolSource)?;
        Ok(Self {
            node: site.node,
            host: site.host,
            generation: site.generation,
            admission: Admission {
                work: parent.work,
                helper: parent.helper,
                peer,
                parent: site.runtime,
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
            Err(
                EngineError::Capability(_)
                | EngineError::Unreadable(_)
                | EngineError::StaleToolSource,
            ) => Ok(false),
            Err(error) => Err(error),
        }
    }
}

pub(crate) struct FrozenInput {
    pub(crate) text: crate::instructions::InputText,
    pub(crate) recorded: Option<String>,
    unit_files: Option<Vec<Hash>>,
}

impl ParentLink {
    pub(crate) fn of_input(input: &threads::InputRow) -> Result<Option<Self>, EngineError> {
        Ok(
            crate::inputs::saved_authority(input.submission_authority.as_ref())?
                .and_then(|authority| authority.capability_parent().cloned()),
        )
    }

    async fn frozen_record(
        &self,
        conn: &mut Conn,
        thread: Id,
        input: &threads::InputRow,
    ) -> Result<Option<(Record, bool)>, EngineError> {
        let record = self.record(conn).await?;
        let unit = codecs::installed()
            .map_err(EngineError::unreadable)?
            .get(&record.execution)?
            .requires_unit();
        if !unit && !record.execution.confined_tools {
            return Ok(None);
        }
        if thread != record.helper
            || input.thread_id != thread
            || input.author_id != Some(record.actor)
            || !self.owns_input(conn, input.id).await?
            || serde_json::to_value(&record.content)? != input.content
        {
            return Err(EngineError::StaleToolSource);
        }
        Ok(Some((record, unit)))
    }

    /// Only producer-owned initial inputs with a verified confinement boundary bypass workspace preparation.
    pub(crate) async fn uses_frozen_input(
        conn: &mut Conn,
        thread: Id,
        input: &threads::InputRow,
    ) -> Result<bool, EngineError> {
        let Some(parent) = Self::of_input(input)? else {
            return Ok(false);
        };
        Ok(parent.frozen_record(conn, thread, input).await?.is_some())
    }
}

#[derive(sqlx::FromRow)]
struct PriorContext {
    work: Id,
    machine: Id,
    close: Option<Id>,
    closed: bool,
    state: Option<String>,
    updated_at: Option<chrono::DateTime<chrono::Utc>>,
    native_ref: Option<String>,
}

async fn prior_context(
    conn: &mut Conn,
    snapshot: &crate::sessions::StartSnapshot,
) -> Result<Option<PriorContext>, EngineError> {
    Ok(sqlx::query_as("SELECT w.id AS work,s.machine_id AS machine,w.idle_close_command_id AS close,w.context_closed_at IS NOT NULL AS closed,c.state,c.updated_at,s.native_session_ref AS native_ref
        FROM delegated_program_work w JOIN turns u ON u.thread_id=w.helper_id AND u.input_id=w.initial_input_id
        JOIN segments s ON s.id=u.segment_id LEFT JOIN commands c ON c.id=w.idle_close_command_id
        WHERE w.helper_id=$1 AND u.segment_id=$2 AND u.ordinal<$3 AND s.machine_id IS NOT NULL")
        .bind(snapshot.thread.node_id).bind(snapshot.segment.id).bind(snapshot.turn.ordinal).fetch_optional(conn).await.map_err(orch_db::DbError::from)?)
}

pub(super) async fn forget_context(
    conn: &mut Conn,
    segment: Id,
    seq: orch_types::Seq,
) -> Result<(), EngineError> {
    sqlx::query("UPDATE segments SET native_session_ref=NULL,seq_updated=$2 WHERE id=$1")
        .bind(segment)
        .bind(seq)
        .execute(conn)
        .await
        .map_err(orch_db::DbError::from)?;
    Ok(())
}

impl Engine {
    /// Journal the unit's close in the transaction which frees its turn for another input.
    pub(crate) async fn retire_unit_turn(
        &self,
        unit: &mut crate::unit::Unit<'_>,
        thread: &threads::ThreadRow,
        turn: &threads::TurnRow,
        seq: orch_types::Seq,
    ) -> Result<(), EngineError> {
        if thread.current_segment_id != Some(turn.segment_id)
            || thread.current_turn_id != Some(turn.id)
        {
            return Ok(());
        }
        let binding: Option<(Id,Id)> = sqlx::query_as(
            "SELECT w.id,s.machine_id FROM delegated_program_work w JOIN segments s ON s.thread_id=w.helper_id
             WHERE w.helper_id=$1 AND w.initial_input_id=$2 AND w.unit_admission IS NOT NULL
               AND w.idle_close_command_id IS NULL AND s.id=$3 AND s.machine_id IS NOT NULL")
            .bind(thread.node_id).bind(turn.input_id).bind(turn.segment_id).fetch_optional(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        let Some((work, machine)) = binding else {
            return Ok(());
        };
        let command = crate::ids::derived(
            "capability.unit.session.close",
            &[work.as_bytes(), turn.segment_id.as_bytes()],
        );
        let close = crate::commands::session_close(
            unit.conn,
            command,
            machine,
            thread.node_id,
            Some(turn.segment_id),
        )
        .await?;
        let now = unit.now();
        crate::archive::persist(unit.conn, now, close).await?;
        capability_work::retain_idle_close(unit.conn, work, command).await?;
        // Its scratch conversation goes away with the unit. A later independent input
        // keeps this helper's product history and opens a fresh ordinary native conversation.
        forget_context(unit.conn, turn.segment_id, seq).await
    }

    /// Another input may reuse the warm provider, but must open its own conversation after
    /// the previous private one closes. Lifecycle delivery owns the actual close and retries.
    pub(crate) async fn program_context_released_for_start(
        &self,
        snapshot: &crate::sessions::StartSnapshot,
    ) -> Result<bool, EngineError> {
        if snapshot.turn.ordinal == 1 {
            return Ok(true);
        }
        let mut tx = self.db().begin().await?;
        let Some(previous) = prior_context(&mut tx, snapshot).await? else {
            return Ok(true);
        };
        if previous.closed {
            return Ok(previous.native_ref == snapshot.segment.native_session_ref);
        }
        if previous
            .state
            .as_deref()
            .is_some_and(|state| matches!(state, "queued" | "sent" | "accepted"))
        {
            return Ok(false);
        }
        let Some(thread) = threads::lock(&mut tx, snapshot.thread.node_id).await? else {
            return Ok(false);
        };
        if thread.current_turn_id != Some(snapshot.turn.id)
            || thread.current_segment_id != Some(snapshot.segment.id)
        {
            return Ok(false);
        }
        let Some(previous) = prior_context(&mut tx, snapshot).await? else {
            return Ok(false);
        };
        let mut unit = crate::unit::Unit::new(&mut tx, self.inner.clock.now());
        let outgoing = match (previous.close, previous.state.as_deref()) {
            (None, None) => {
                let command = crate::ids::derived(
                    "capability.context.close",
                    &[
                        previous.work.as_bytes(),
                        snapshot.segment.id.as_bytes(),
                        previous.work.as_bytes(),
                    ],
                );
                let outgoing = crate::commands::session_close(
                    unit.conn,
                    command,
                    previous.machine,
                    thread.node_id,
                    Some(snapshot.segment.id),
                )
                .await?;
                let seq = unit.stamp(thread.node_id).await?;
                forget_context(unit.conn, snapshot.segment.id, seq).await?;
                outgoing
            }
            (Some(id), None) if !previous.closed => {
                let retry = crate::ids::derived(
                    "capability.unit.session.close.retry",
                    &[previous.work.as_bytes(), id.as_bytes()],
                );
                crate::commands::session_close(
                    unit.conn,
                    retry,
                    previous.machine,
                    thread.node_id,
                    Some(snapshot.segment.id),
                )
                .await?
            }
            (Some(id), Some("failed" | "expired" | "cancelled")) => {
                let pause = chrono::Duration::from_std(crate::commands::BUSY_RETRY_PAUSE)
                    .map_err(EngineError::unreadable)?;
                if previous
                    .updated_at
                    .is_none_or(|at| self.inner.clock.now() - at < pause)
                {
                    return Ok(false);
                }
                let original = link::command(unit.conn, id)
                    .await?
                    .ok_or(EngineError::ToolCallIdentity)?;
                Outgoing {
                    id: crate::ids::derived(
                        "capability.unit.session.close.retry",
                        &[previous.work.as_bytes(), id.as_bytes()],
                    ),
                    machine: original.machine_id,
                    key: SerialKey::Thread(thread.node_id),
                    kind: original.kind.parse().map_err(EngineError::unreadable)?,
                    body: original.body,
                    deadline: None,
                    authority: None,
                }
            }
            _ => return Ok(false),
        };
        let id = outgoing.id;
        let now = unit.now();
        crate::archive::persist(unit.conn, now, outgoing).await?;
        capability_work::retain_idle_close(unit.conn, previous.work, id).await?;
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        Ok(false)
    }

    /// Only the producer-owned initial input can supply the material for a confined unit.
    pub(crate) async fn frozen_program_input(
        &self,
        snapshot: &crate::sessions::StartSnapshot,
    ) -> Result<Option<FrozenInput>, EngineError> {
        let Some(parent) = ParentLink::of_input(&snapshot.input)? else {
            return Ok(None);
        };
        let mut conn = self.db().acquire().await?;
        let Some((record, unit)) = parent
            .frozen_record(&mut conn, snapshot.thread.node_id, &snapshot.input)
            .await?
        else {
            return Ok(None);
        };
        if snapshot.turn.input_id != snapshot.input.id {
            return Err(EngineError::StaleToolSource);
        }
        if !unit
            && (!record.material.is_empty()
                || record
                    .content
                    .iter()
                    .any(|part| !matches!(part, ContentPart::Text { .. })))
        {
            return Err(EngineError::ToolCallIdentity);
        }
        let (limit, image_source, image_origin) =
            crate::turns::native_image_policy(&mut conn, record.helper).await?;
        let images_pass = snapshot.descriptor.capabilities.input_images
            && snapshot.model_parameters()?.takes_images();
        if let origin::Producer::Ask { source, .. } = &record.origin {
            source
                .readable(&mut conn, self.inner.clock.as_ref())
                .await?;
            let mut images = Vec::new();
            for part in source
                .request
                .input
                .messages
                .iter()
                .flat_map(|message| &message.content)
            {
                if let orch_model_contract::generation::GenerationPart::ImageRef {
                    blob,
                    size,
                    media_type,
                    name,
                    ..
                } = part
                {
                    if !images_pass || *size <= 0 || *size > limit {
                        return Err(EngineError::StaleToolSource);
                    }
                    images.push(orch_link_contract::commands::TurnImage {
                        blob: *blob,
                        media_type: media_type.clone(),
                        name: name.clone(),
                        workspace_path: None,
                    });
                }
            }
            let text = crate::ask::dialogue::prompt(&source.request.input, source.request.format)?;
            return Ok(Some(FrozenInput {
                text: crate::instructions::InputText {
                    image_delivery: None,
                    text,
                    images,
                    left_out: Vec::new(),
                },
                recorded: Some("Ask model request".into()),
                unit_files: None,
            }));
        }
        let mut placement = crate::delivery::Placement::default();
        let mut files = std::collections::BTreeSet::new();
        let mut material = record.material.iter();
        for (index, part) in record.content.iter().enumerate() {
            match part {
                ContentPart::Text { .. } => {}
                ContentPart::Image { blob, .. } => {
                    let Some(orch_model_contract::generation::GenerationPart::ImageRef {
                        blob: saved,
                        size,
                        ..
                    }) = material.next()
                    else {
                        return Err(EngineError::ToolCallIdentity);
                    };
                    if saved != blob || *size <= 0 {
                        return Err(EngineError::ToolCallIdentity);
                    }
                    let path = (!images_pass || *size > limit).then(|| {
                        files.insert(*blob);
                        orch_link_contract::program_work::input_path(*blob)
                    });
                    placement.unit_image(index, *size, path);
                }
                _ => return Err(EngineError::ToolCallIdentity),
            }
        }
        if material.next().is_some() {
            return Err(EngineError::ToolCallIdentity);
        }
        let mut text = crate::instructions::input_text(
            &record.content,
            None,
            &crate::instructions::InputRules {
                rendered_announcement: false,
                attribute_to: None,
                names_people: false,
                images_pass,
                native_image_bytes: Some(limit),
                directory: "/work",
                placement: &placement,
            },
        );
        text.image_delivery = crate::turns::capture_image_origin(
            crate::instructions::image_delivery_receipt(
                &record.content,
                images_pass,
                limit,
                &placement,
                image_source,
                self.inner.clock.time(),
            ),
            &image_origin,
        );
        Ok(Some(FrozenInput {
            text,
            recorded: None,
            unit_files: unit.then(|| files.into_iter().collect()),
        }))
    }

    pub(crate) async fn prepare_program_body(
        &self,
        snapshot: &crate::sessions::StartSnapshot,
        body: &mut orch_link_contract::commands::SessionOpen,
        frozen: &FrozenInput,
    ) -> Result<(), EngineError> {
        if let Some(files) = &frozen.unit_files {
            let plan = self
                .prepare_program_unit(snapshot)
                .await?
                .ok_or(EngineError::StaleToolSource)?;
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
                input_files: files.clone(),
                ticket: orch_link_contract::commands::TicketSlot::Asked(
                    orch_link_contract::commands::TicketAsk {
                        ticket: orch_link_contract::commands::TicketWanted {
                            kind: orch_link_contract::tunnels::TunnelKind::Program,
                            more,
                        },
                    },
                ),
            });
            body.workspace_root = "/work".into();
            body.directory = "/work".into();
            body.cwd = "/work".into();
        } else if body.side_reading != Some(orch_link_contract::commands::SideReading::NoTools) {
            return Err(EngineError::StaleToolSource);
        }
        // The owned initial request is complete. No inherited private topic material becomes another input.
        body.mode = orch_link_contract::commands::SessionMode::New;
        body.native_session_ref = None;
        body.archive = None;
        body.history = None;
        body.upto_turn_ordinal = None;
        body.raw_events = false;
        body.instructions.clear();
        body.memory = None;
        body.mounts.clear();
        body.mcp.clear();
        body.skills.clear();
        body.orch_tools.clear();
        body.orch_tool_descriptions.clear();
        body.orch_tool_schemas.clear();
        body.workspace_configuration = None;
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
            // Session admission and ordinary preflight both lock the helper before restoring
            // the requesting turn. The command authority rechecks current Use before commit.
            if threads::lock(&mut tx, parent.helper).await?.is_none()
                || !parent.owns_input(&mut tx, snapshot.input.id).await?
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
