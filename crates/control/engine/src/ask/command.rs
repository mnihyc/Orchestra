//! The retained Ask call and controller behind one exact native model command.
use super::{admission, model};
use crate::{
    EngineError,
    commands::Outgoing,
    operation_scope::{
        continuing::{self, Error, Source as NativeSource},
        sources::Facts,
        targets,
    },
};
use futures::future::BoxFuture;
use orch_access_contract::{
    key_scope::{KeyResolvedTarget, KeyTargetKind},
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    ask::{AskMessageCreate, AskModel},
    operation_sources::{Binding, Cardinality, Declaration},
    operations::{FacetUsage, TargetResolver},
    routes::{Endpoint, Route},
    threads::ContentPart,
};
use orch_authz::{Act, Caller, CapturedOperation, Principal};
use orch_db::{
    Conn, DbError,
    queries::{
        ask as rows, hierarchy,
        link::{CommandRow, NewCommand},
        threads,
    },
};
use orch_link_contract::commands::{CommandKind, ProviderGenerate, SerialKey};
use orch_types::{Clock, Hash, Id, Json, Time};
use serde::{Deserialize, Serialize};

/// Actual current request callers and their factual source deadlines.
pub type Callers = Vec<(Option<Caller>, Option<Time>)>;

/// Four missed thirty-second heartbeats are the existing controller-loss boundary.
pub const CONTROLLER_LEASE_SECONDS: i64 = 120;

/// The same lease predicate is used by the controller, recovery and command publication.
pub fn controller_alive(
    heartbeat: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    heartbeat.is_some_and(|at| at > now - chrono::Duration::seconds(CONTROLLER_LEASE_SECONDS))
}

/// Factual roles of the existing message operation, including all history sent to the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The actual conversation for both Write and required dialogue Read.
    Conversation,
    /// Actual conversation-owned attachments, or a checked empty selection.
    Attachments,
}
/// The same source declaration at HTTP preparation and native continuation.
pub const MESSAGE: Declaration<Role> = Declaration {
    operation: "AskMessageCreate",
    bindings: &[
        Binding {
            source: Role::Conversation,
            target: TargetResolver::TypedPath {
                parameter: "id",
                kind: KeyTargetKind::Object(ObjectKind::Thread),
            },
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Role::Attachments,
            target: TargetResolver::ResolvedCollection("ask_attachment_sources"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};

/// Connect only actual resolved conversation facts to the existing canonical roles.
pub fn facts(target: KeyResolvedTarget, attachments: bool) -> Vec<Facts<Role>> {
    vec![
        Facts::one(
            Role::Conversation,
            KeyTargetKind::Object(ObjectKind::Thread),
            target.clone(),
        ),
        Facts::collection(
            Role::Attachments,
            if attachments {
                vec![(KeyTargetKind::Object(ObjectKind::Thread), vec![target])]
            } else {
                Vec::new()
            },
        ),
    ]
}

/// The selected native controller; these IDs are checked against the retained call rows.
pub struct Controller {
    /// Actual conversation.
    pub thread: Id,
    /// Actual admitted turn.
    pub turn: Id,
    /// Current runner identity.
    pub runner: Id,
}

/// Private provenance, never model input or connector wire data.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandSource {
    origin: CapturedOperation,
    call: Id,
    thread: Id,
    root: Id,
    turn: Id,
    segment: Id,
    controller: Id,
    input: Id,
    input_body: Hash,
    person: Id,
    payer: Id,
    provider: Id,
    machine: Id,
    body: Hash,
    endpoint: Vec<u8>,
}
impl std::fmt::Debug for CommandSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AskCommandSource([private])")
    }
}

fn closed() -> EngineError {
    EngineError::BeforeEffect(Box::new(Error::Refused))
}
fn fingerprint(body: &Json) -> Result<Hash, EngineError> {
    let mut body = body.clone();
    body.sort_all_objects();
    Ok(Hash::of(&serde_json::to_vec(&body)?))
}

/// Actual current rows selected by the retained native call.
pub struct Selected {
    row: rows::Conversation,
    call: rows::Call,
}
/// Native ordinary prerequisites are shared by first issue and later unaccepted delivery.
pub struct Source;
impl NativeSource for Source {
    type Role = Role;
    type Input = CommandSource;
    type Value = Selected;
    const ROUTE: Route = AskMessageCreate::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &MESSAGE;

    fn prepare<'a>(
        conn: &'a mut Conn,
        _caller: &'a Caller,
        input: &'a CommandSource,
        now: Time,
    ) -> BoxFuture<'a, Result<(Selected, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let call = rows::call(conn, input.call, input.machine)
                .await?
                .ok_or(Error::Refused)?;
            let row = rows::get(conn, input.thread, false)
                .await?
                .ok_or(Error::Refused)?;
            let turn = threads::turn(conn, input.turn)
                .await?
                .ok_or(Error::Refused)?;
            let authored = threads::input(conn, input.input)
                .await?
                .ok_or(Error::Refused)?;
            if call.thread_id != input.thread
                || call.root_org_id != input.root
                || call.turn_id != input.turn
                || call.segment_id != input.segment
                || call.person_id != input.person
                || call.provider_id != input.provider
                || fingerprint(&call.request)? != input.body
                || row.root_org_id != input.root
                || row.turn != Some(input.turn)
                || row.segment != Some(input.segment)
                || row.runner != Some(input.controller)
                || row.state != "running"
                || row.payer_id != Some(input.payer)
                || !controller_alive(row.heartbeat, now.get())
                || turn.thread_id != input.thread
                || turn.input_id != input.input
                || turn.segment_id != input.segment
                || turn.state != "running"
                || authored.thread_id != input.thread
                || authored.author_id != Some(input.person)
                || authored.turn_id != Some(input.turn)
                || authored.run_as.is_some()
                || fingerprint(&authored.content)? != input.input_body
                || orch_db::queries::semantic::endpoint_fingerprint(conn, input.provider)
                    .await?
                    .as_ref()
                    != Some(&input.endpoint)
            {
                return Err(Error::Refused);
            }
            let content: Vec<ContentPart> =
                serde_json::from_value(authored.content).map_err(EngineError::from)?;
            let hashes = orch_chat::following::content_blobs(&content);
            for hash in &hashes {
                if rows::file(conn, input.thread, hash).await?.is_none() {
                    return Err(Error::Refused);
                }
            }
            let target = targets::node(conn, input.thread)
                .await
                .map_err(EngineError::unreadable)?;
            Ok((Selected { row, call }, facts(target, !hashes.is_empty())))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a CommandSource,
        selected: &'a Selected,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let principal = principal.ordinary(conn).await?;
            if principal.caller().user() != input.person
                || selected.row.temporary && selected.row.user_id != input.person
                || !matches!(
                    admission::root_access(conn, &principal, input.root).await?,
                    admission::RootAccess::Allowed
                )
            {
                return Err(Error::Refused);
            }
            let policy = admission::page_policy(conn, input.thread).await?;
            for permission in [Permission::ThreadView, Permission::ThreadInteract] {
                if !orch_authz::nodes::check(
                    conn,
                    &principal,
                    input.thread,
                    permission,
                    Act::Use,
                    policy,
                )
                .await?
                .is_allowed()
                {
                    return Err(Error::Refused);
                }
            }
            let request: ProviderGenerate =
                serde_json::from_value(selected.call.request.clone()).map_err(EngineError::from)?;
            if request.provider_id != input.provider {
                return Err(Error::Refused);
            }
            let selected = match Box::pin(model::select(
                conn,
                &principal,
                input.root,
                AskModel {
                    provider_id: request.provider_id,
                    model: request.model_name.clone(),
                    params: request.params.clone(),
                },
                principal.now(),
            ))
            .await
            {
                Ok(selected) => selected,
                Err(model::Error::Denied(_)) => return Err(Error::Refused),
                Err(model::Error::Db(error)) => return Err(error.into()),
                Err(model::Error::Native(error)) => return Err(error.into()),
            };
            let tier = request.params.as_ref().and_then(|params| {
                orch_model_contract::parameters::requested_tier_for_caps(&request.protocol, params)
            });
            if !crate::caps::retained_turn_room(
                conn,
                &crate::caps::OwnCapAsk {
                    place: input.root,
                    provider: input.provider,
                    model: &request.model_name,
                    tier: tier.as_deref(),
                    purpose: "ask",
                    person: Some(input.person),
                },
                principal.now().get(),
                input.turn,
            )
            .await?
            {
                return Err(Error::Refused);
            }
            let chosen = match selected.online(conn).await {
                Ok(chosen) => chosen,
                Err(model::Error::Denied(_)) => return Err(Error::Refused),
                Err(model::Error::Db(error)) => return Err(error.into()),
                Err(model::Error::Native(error)) => return Err(error.into()),
            };
            if chosen.machine != input.machine
                || chosen.payer != input.payer
                || chosen.protocol != request.protocol
                || chosen.model.params != request.params
                || chosen.parameters != request.model_parameters
            {
                return Err(Error::Refused);
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a CommandSource,
        _selected: &'a Selected,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move { input.lock_source(conn).await.map_err(Error::from) })
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a CommandSource,
        opened: &'a Selected,
        fresh: &'a Selected,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move {
            Ok(opened.call.request == fresh.call.request
                && opened.row.root_org_id == fresh.row.root_org_id
                && opened.row.payer_id == fresh.row.payer_id
                && opened.row.runner == fresh.row.runner)
        })
    }
}

impl CommandSource {
    /// Only the producer's actual admitted operation and retained call can create this evidence.
    pub async fn capture(
        conn: &mut Conn,
        caller: &Caller,
        controller: Controller,
        command: &Outgoing,
    ) -> Result<Self, EngineError> {
        if caller.admitted_operation() != Some(MESSAGE.operation)
            || command.kind != CommandKind::ProviderGenerate
            || command.key != SerialKey::None
        {
            return Err(closed());
        }
        let call = rows::call(conn, command.id, command.machine)
            .await?
            .ok_or_else(closed)?;
        let row = rows::get(conn, controller.thread, false)
            .await?
            .ok_or_else(closed)?;
        let turn = threads::turn(conn, controller.turn)
            .await?
            .ok_or_else(closed)?;
        let authored = threads::input(conn, turn.input_id)
            .await?
            .ok_or_else(closed)?;
        let request: ProviderGenerate = serde_json::from_value(call.request.clone())?;
        if call.thread_id != controller.thread
            || call.turn_id != controller.turn
            || call.person_id != caller.user()
            || call.request != command.body
            || row.turn != Some(controller.turn)
            || row.runner != Some(controller.runner)
            || row.root_org_id != call.root_org_id
            || row.segment != Some(call.segment_id)
            || turn.thread_id != controller.thread
            || authored.thread_id != controller.thread
            || authored.author_id != Some(caller.user())
            || authored.turn_id != Some(controller.turn)
            || authored.run_as.is_some()
            || request.provider_id != call.provider_id
        {
            return Err(closed());
        }
        Ok(Self {
            origin: CapturedOperation::capture(caller).map_err(EngineError::unreadable)?,
            call: call.id,
            thread: call.thread_id,
            root: call.root_org_id,
            turn: call.turn_id,
            segment: call.segment_id,
            controller: controller.runner,
            input: turn.input_id,
            input_body: fingerprint(&authored.content)?,
            person: call.person_id,
            payer: row.payer_id.ok_or_else(closed)?,
            provider: call.provider_id,
            machine: command.machine,
            body: fingerprint(&command.body)?,
            endpoint: orch_db::queries::semantic::endpoint_fingerprint(conn, call.provider_id)
                .await?
                .ok_or_else(closed)?,
        })
    }
    /// The actual accepted operation, to be joined to the original raw HTTP identity.
    pub fn origin(&self) -> &CapturedOperation {
        &self.origin
    }
    /// Exact call, request, machine and serial identity before constructing private evidence.
    pub fn command(
        &self,
        command: &Outgoing,
    ) -> Result<crate::operation_scope::http_command_source::CommandEvidence, EngineError> {
        if !self.matches(
            command.id,
            command.machine,
            command.kind.as_str(),
            command.key.wire().as_deref(),
            &command.body,
        )? {
            return Err(closed());
        }
        Ok(
            crate::operation_scope::http_command_source::CommandEvidence::Ask(Box::new(
                self.clone(),
            )),
        )
    }
    fn matches(
        &self,
        id: Id,
        machine: Id,
        kind: &str,
        serial: Option<&str>,
        body: &Json,
    ) -> Result<bool, EngineError> {
        Ok(id == self.call
            && machine == self.machine
            && kind == "provider.generate"
            && serial.is_none()
            && fingerprint(body)? == self.body)
    }
    /// Native controller and physical source locks precede the canonical command row.
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        let mut nodes = hierarchy::with_ancestors(conn, &[self.root, self.thread])
            .await?
            .into_iter()
            .map(|node| node.id)
            .collect::<Vec<_>>();
        nodes.sort_unstable();
        nodes.dedup();
        let _: Vec<Id> =
            sqlx::query_scalar("SELECT id FROM nodes WHERE id=ANY($1) ORDER BY id FOR SHARE")
                .bind(&nodes)
                .fetch_all(&mut *conn)
                .await
                .map_err(DbError::from)?;
        sqlx::query("SELECT node_id FROM threads WHERE node_id=$1 FOR UPDATE")
            .bind(self.thread)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?
            .ok_or_else(closed)?;
        sqlx::query("SELECT id FROM providers WHERE id=$1 FOR SHARE")
            .bind(self.provider)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?
            .ok_or_else(closed)?;
        sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
            .bind(self.machine)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?
            .ok_or_else(closed)?;
        sqlx::query("SELECT id FROM ask_calls WHERE id=$1 FOR SHARE")
            .bind(self.call)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?
            .ok_or_else(closed)?;
        Ok(())
    }
    /// Proposed and stored commands share this same native current-source verifier.
    pub async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Callers>, EngineError> {
        if !self.matches(
            command.id,
            command.machine_id,
            command.kind,
            command.serial_key,
            command.body,
        )? {
            return Ok(None);
        }
        let frame = match Box::pin(continuing::prepare::<Source>(
            conn,
            clock,
            &self.origin,
            self.clone(),
        ))
        .await
        {
            Ok(frame) => frame,
            Err(Error::Refused | Error::Binding(_)) => return Ok(None),
            Err(Error::Native(error)) => return Err(error),
        };
        let parts = frame.into_parts();
        if parts.first_effect.check(clock).is_err() {
            return Ok(None);
        }
        Ok(Some(vec![(Some(parts.principal.caller().clone()), None)]))
    }
    /// Verify the same unaccepted command again at its actual native delivery.
    pub async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Callers>, EngineError> {
        self.current_issue_callers(
            conn,
            &NewCommand {
                id: command.id,
                machine_id: command.machine_id,
                kind: &command.kind,
                serial_key: command.serial_key(),
                body: &command.body,
                deadline_at: command.deadline_at,
            },
            clock,
        )
        .await
    }
}
