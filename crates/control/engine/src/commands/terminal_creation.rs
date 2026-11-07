//! Actual accepted shell creation and its typed native commands, not a new ticket or permission model.

use super::{CommandAuthority, Outgoing};
use crate::{
    EngineError,
    operation_scope::{Prepared, targets},
};
use orch_access_contract::{
    decisions::ObjectRef, key_scope::KeyTargetKind, permissions::ObjectKind,
};
use orch_api_contract::operations::{FacetUsage, TargetResolver};
use orch_authz::{Caller, CapturedOperation, Policy, Principal};
use orch_db::{
    Conn,
    queries::{link::CommandRow, terminals},
};
use orch_link_contract::commands::{CommandKind, PtyClose, PtyOpen, PtyStatus, SerialKey};
use orch_types::{Clock, Id, Time};
use serde::{Deserialize, Serialize};

const OP: &str = "EnvironmentTerminalCreate";
const ENV: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);

/// The real accepted creator and physical environment, with no reusable target admission.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalCreationOrigin {
    origin: CapturedOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<Id>,
    environment: Id,
    machine: Id,
    generation: i32,
    opening: Option<Id>,
}
impl std::fmt::Debug for TerminalCreationOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TerminalCreationOrigin([private])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "body", rename_all = "snake_case")]
enum Expected {
    Open(PtyOpen),
    Close(PtyClose),
    Status(PtyStatus),
}

/// The exact native body beside the accepted creation, never inside the Cmd wire body.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalCreationAuthority {
    creator: TerminalCreationOrigin,
    expected: Expected,
}
impl std::fmt::Debug for TerminalCreationAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TerminalCreationAuthority([private])")
    }
}

impl TerminalCreationOrigin {
    /// Captures only a fully admitted actual creator at the native selected physical site.
    pub fn capture(
        caller: &Caller,
        session: Option<Id>,
        site: &terminals::Site,
    ) -> Result<Self, EngineError> {
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        let metadata = Prepared::for_selection(OP).map_err(EngineError::unreadable)?;
        let facets = caller.admitted_facets().unwrap_or(&[]);
        if origin.operation() != OP
            || session.is_some() && caller.grant().is_some()
            || !metadata
                .variant()
                .facets
                .iter()
                .filter(|facet| facet.usage == FacetUsage::Required)
                .all(|required| {
                    required.target == TargetResolver::Path("id")
                        && required.resources == [ENV]
                        && facets.iter().any(|fact| {
                            fact.operation == OP
                                && fact.action == required.action
                                && fact.resource_kind == ENV
                                && fact.target.object.kind == ENV
                                && fact.target.object.id == Some(site.id)
                        })
                })
            || !caller.permits_facets(facets)
        {
            return Err(EngineError::unreadable(
                "the native creator was not admitted at its actual environment",
            ));
        }
        Ok(Self {
            origin,
            session,
            environment: site.id,
            machine: site.machine_id.ok_or_else(|| {
                EngineError::unreadable("the native creator has no physical machine")
            })?,
            generation: site.generation,
            opening: None,
        })
    }

    /// Pins the generated native reservation after its first guarded INSERT.
    pub fn with_opening(mut self, opening: &crate::shells::Opening) -> Result<Self, EngineError> {
        if opening.site().id != self.environment
            || opening.site().generation != self.generation
            || opening.site().machine_id != Some(self.machine)
            || opening.person() != self.origin.user()
        {
            return Err(EngineError::unreadable(
                "the native reservation has another creator or physical source",
            ));
        }
        self.opening = Some(opening.id());
        Ok(self)
    }

    /// Current actual reservation/owned close, before another native creation step.
    pub(crate) async fn given(
        &self,
        conn: &mut Conn,
        row: &terminals::SessionRow,
        clock: &dyn Clock,
    ) -> Result<Option<(Principal, Option<Time>)>, EngineError> {
        let Some(opening) = self.opening else {
            return Ok(None);
        };
        let Some(parent) = terminals::get(conn, opening).await? else {
            return Ok(None);
        };
        if parent.state != terminals::state::OPENING
            || row.id == opening
            || row.environment_id != self.environment
            || row.opened_by != self.origin.user()
        {
            return Ok(None);
        }
        self.current(conn, clock).await
    }

    /// The actual retained caller, current ordinary door and native physical gates.
    pub async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<(Principal, Option<Time>)>, EngineError> {
        let current = self.at(conn, clock.time()).await?;
        Ok(current.filter(|(principal, deadline)| {
            principal.caller().key_deadline_allows(clock)
                && deadline.is_none_or(|end| clock.time().get() < end.get())
        }))
    }

    async fn at(
        &self,
        conn: &mut Conn,
        now: Time,
    ) -> Result<Option<(Principal, Option<Time>)>, EngineError> {
        if self.origin.operation() != OP {
            return Ok(None);
        }
        let Some(session) =
            super::authority::session_state(conn, &self.origin, self.session, now).await?
        else {
            return Ok(None);
        };
        let Some(caller) = self.origin.restore(conn, now).await? else {
            return Ok(None);
        };
        let enabled = caller.as_admin() && session.is_some_and(|(admin, _)| admin);
        let caller = caller.with_admin_mode(enabled);
        let Some(site) = terminals::site(conn, self.environment).await? else {
            return Ok(None);
        };
        if site.machine_id != Some(self.machine) || site.generation != self.generation {
            return Ok(None);
        }
        let target = match targets::resource_identity(
            conn,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: self.environment,
            },
        )
        .await
        {
            Ok(target) => target,
            Err(targets::TargetError::Missing) => return Ok(None),
            Err(targets::TargetError::Db(error)) => return Err(error.into()),
            Err(targets::TargetError::Authz(error)) => return Err(error.into()),
        };
        let mut plan = Prepared::for_selection(OP).map_err(EngineError::unreadable)?;
        for index in 0..plan.variant().facets.len() {
            let facet = &plan.variant().facets[index];
            if facet.usage != FacetUsage::Required
                || facet.resources != [ENV]
                || facet.target != TargetResolver::Path("id")
            {
                return Err(EngineError::unreadable(
                    "the native creation declaration changed",
                ));
            }
            plan.bind(index, ENV, std::slice::from_ref(&target))
                .map_err(EngineError::unreadable)?;
        }
        let Ok(caller) = plan.admit(&caller, false) else {
            return Ok(None);
        };
        let principal = match Principal::load(conn, caller, now).await {
            Ok(principal) => principal,
            Err(
                orch_authz::Error::Suspended { .. }
                | orch_authz::Error::NoSuchUser(_)
                | orch_authz::Error::NotFound(_),
            ) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !orch_authz::environments::may_open_shell(
            conn,
            &principal,
            self.environment,
            Policy::default(),
        )
        .await?
        .is_allowed()
        {
            return Ok(None);
        }
        let through = if let Some(opening) = self.opening {
            let Some(row) = terminals::get(conn, opening).await? else {
                return Ok(None);
            };
            if row.environment_id != self.environment
                || row.opened_by != self.origin.user()
                || row.env_generation != self.generation
            {
                return Ok(None);
            }
            row.thread_id
        } else {
            None
        };
        if crate::shells::Shells::opening_gates(
            conn,
            &site,
            crate::shells::place_of(through, self.environment),
        )
        .await?
        .is_some()
        {
            return Ok(None);
        }
        Ok(Some((principal, session.map(|(_, end)| end))))
    }

    /// Only the typed native close/open/status factory can choose one of these bodies.
    pub fn command(&self, command: &Outgoing) -> Result<CommandAuthority, EngineError> {
        if command.machine != self.machine || command.key != SerialKey::None {
            return Err(EngineError::unreadable(
                "the native creation command has another physical source",
            ));
        }
        let expected = match command.kind {
            CommandKind::PtyOpen => Expected::Open(serde_json::from_value(command.body.clone())?),
            CommandKind::PtyClose => Expected::Close(serde_json::from_value(command.body.clone())?),
            CommandKind::PtyStatus => {
                Expected::Status(serde_json::from_value(command.body.clone())?)
            }
            _ => {
                return Err(EngineError::unreadable(
                    "this is not a native shell creation command",
                ));
            }
        };
        Ok(CommandAuthority::terminal_creation(
            TerminalCreationAuthority {
                creator: self.clone(),
                expected,
            },
        ))
    }
}

impl TerminalCreationAuthority {
    pub(super) fn command_kind(&self) -> &'static str {
        match &self.expected {
            Expected::Open(_) => "pty.open",
            Expected::Close(_) => "pty.close",
            Expected::Status(_) => "pty.status",
        }
    }

    pub(super) async fn authorized(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        now: Time,
    ) -> Result<bool, EngineError> {
        if command.machine_id != self.creator.machine
            || command.kind != self.command_kind()
            || command.serial_key().is_some()
        {
            return Ok(false);
        }
        let Some((_, _)) = self.creator.at(conn, now).await? else {
            return Ok(false);
        };
        match &self.expected {
            Expected::Open(body) => {
                if self.creator.opening != Some(body.terminal_id)
                    || body.environment_id != self.creator.environment
                    || command.id
                        != crate::ids::derived(
                            "pty.open",
                            &[body.terminal_id.as_bytes().as_slice()],
                        )
                    || command.body != serde_json::to_value(body)?
                {
                    return Ok(false);
                }
                let Some(row) = terminals::get(conn, body.terminal_id).await? else {
                    return Ok(false);
                };
                let Some(site) = terminals::site(conn, self.creator.environment).await? else {
                    return Ok(false);
                };
                let absolute = if body.cwd.is_empty() {
                    site.workspace_root
                } else {
                    format!("{}/{}", site.workspace_root.trim_end_matches('/'), body.cwd)
                };
                Ok(row.state == terminals::state::OPENING
                    && row.cols == i32::from(body.cols)
                    && row.rows == i32::from(body.rows)
                    && row.cwd == absolute)
            }
            Expected::Close(body) => {
                if command.id
                    != crate::ids::derived("pty.close", &[body.terminal_id.as_bytes().as_slice()])
                    || command.body != serde_json::to_value(body)?
                {
                    return Ok(false);
                }
                let Some(opening) = self.creator.opening else {
                    return Ok(false);
                };
                let Some(parent) = terminals::get(conn, opening).await? else {
                    return Ok(false);
                };
                let Some(row) = terminals::get(conn, body.terminal_id).await? else {
                    return Ok(false);
                };
                Ok(parent.state == terminals::state::OPENING
                    && row.environment_id == self.creator.environment
                    && row.opened_by == self.creator.origin.user()
                    && row.env_generation == self.creator.generation
                    && row.id != opening)
            }
            Expected::Status(body) => {
                if body.environment_id != self.creator.environment
                    || body.terminal_ids.is_empty()
                    || body.terminal_ids.len() > 200
                    || command.body != serde_json::to_value(body)?
                {
                    return Ok(false);
                }
                let rows = terminals::get_many(conn, &body.terminal_ids).await?;
                Ok(body.terminal_ids.iter().all(|id| {
                    rows.iter().any(|row| {
                        row.id == *id
                            && row.environment_id == self.creator.environment
                            && row.opened_by == self.creator.origin.user()
                            && row.env_generation == self.creator.generation
                    })
                }))
            }
        }
    }

    pub(super) async fn current_callers(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        Ok(self
            .creator
            .current(conn, clock)
            .await?
            .map(|(principal, deadline)| vec![(Some(principal.caller().clone()), deadline)]))
    }
}
