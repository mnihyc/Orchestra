//! One raw HTTP identity layer around a native producer's exact private command source.
use super::{CommandAuthority, Outgoing};
use crate::{
    EngineError,
    access::ResourceAccess,
    operation_scope::{continuing, http_origin::HttpOrigin},
};
use chrono::{DateTime, Utc};
use orch_authz::Caller;
use orch_db::{
    Conn,
    queries::link::{self, CommandRow, NewCommand},
};
use orch_types::{Clock, Json, Time};
use serde::{Deserialize, Serialize};

fn closed() -> EngineError {
    EngineError::BeforeEffect(Box::new(continuing::Error::Refused))
}
/// A durable conjunction of original raw identity and the actual native source; never wire data.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpCommand {
    original: HttpOrigin,
    accepted: HttpOrigin,
    native: Native,
    canonical: Json,
    deadline: Option<Time>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sealed_deadline: Option<DateTime<Utc>>,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "evidence",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum Native {
    Channel(Box<CommandAuthority>),
    Source(Box<crate::operation_scope::http_command_source::CommandEvidence>),
}
impl Native {
    fn command_kind(&self) -> &'static str {
        match self {
            Self::Channel(source) => source.command_kind(),
            Self::Source(source) => source.command_kind(),
        }
    }
    fn same_origin(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Channel(a), Self::Channel(b)) => a.same_origin(b),
            _ => self == other,
        }
    }
    async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        match self {
            Self::Channel(source) => Box::pin(source.lock_native_source(conn)).await,
            Self::Source(source) => source.lock_source(conn).await,
        }
    }
    async fn issue(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
        access: &dyn ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        match self {
            Self::Channel(source) => {
                Box::pin(source.current_issue_callers(conn, command, clock, access)).await
            }
            Self::Source(source) => {
                source
                    .current_issue_callers(conn, command, clock, access)
                    .await
            }
        }
    }
    async fn current(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
        access: &dyn ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        match self {
            Self::Channel(source) => {
                Box::pin(source.current_callers(conn, command, clock, access)).await
            }
            Self::Source(source) => source.current_callers(conn, command, clock, access).await,
        }
    }
}
impl std::fmt::Debug for HttpCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HttpCommand([private])")
    }
}
impl HttpOrigin {
    /// The domain supplies exact native evidence; an identity alone cannot admit a command.
    pub fn command(
        &self,
        command: &Outgoing,
        native: CommandAuthority,
    ) -> Result<CommandAuthority, EngineError> {
        if command.authority.is_some()
            || matches!(native, CommandAuthority::Http(_))
            || !native.requires_issue_checkpoint()
            || native.command_kind() != command.kind.as_str()
        {
            return Err(closed());
        }
        Ok(CommandAuthority::Http(Box::new(HttpCommand {
            original: self.clone(),
            accepted: self.clone(),
            native: Native::Channel(Box::new(native)),
            canonical: crate::jobs::command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
            deadline: command.deadline,
            sealed_deadline: None,
        })))
    }
}
impl HttpOrigin {
    /// A producer with its own native source uses the same raw identity channel and checkpoints.
    pub fn command_source(
        &self,
        command: &Outgoing,
        native: crate::operation_scope::http_command_source::CommandEvidence,
    ) -> Result<CommandAuthority, EngineError> {
        if command.authority.is_some() || native.command_kind() != command.kind.as_str() {
            return Err(closed());
        }
        Ok(CommandAuthority::Http(Box::new(HttpCommand {
            original: self.clone(),
            accepted: self.clone(),
            native: Native::Source(Box::new(native)),
            canonical: crate::jobs::command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
            deadline: command.deadline,
            sealed_deadline: None,
        })))
    }
}
impl HttpCommand {
    pub(crate) fn reads_before_fence(&self) -> bool {
        matches!(&self.native, Native::Channel(source) if source.reads_before_fence())
    }
    pub(crate) async fn refused_without_fence(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
    ) -> Result<bool, EngineError> {
        match &self.native {
            Native::Channel(source) => Box::pin(source.refused_without_fence(conn, command)).await,
            Native::Source(_) => Ok(false),
        }
    }
    /// Accepted native mkdir coordinates, without restoring its original sender.
    pub(crate) fn file_mkdir_receipt(
        &self,
        row: &CommandRow,
    ) -> Result<Option<super::FileMkdirInput>, EngineError> {
        let Native::Source(source) = &self.native else {
            return Ok(None);
        };
        let crate::operation_scope::http_command_source::CommandEvidence::FileMkdir(source) =
            source.as_ref()
        else {
            return Ok(None);
        };
        if !self.original.same_identity(&self.accepted)
            || self
                .sealed_deadline
                .or_else(|| self.deadline.map(Time::get))
                != Some(row.deadline_at)
            || !self.matches(&NewCommand {
                id: row.id,
                machine_id: row.machine_id,
                kind: &row.kind,
                body: &row.body,
                serial_key: row.serial_key(),
                deadline_at: row.deadline_at,
            })?
            || !source.matches_receipt(row)?
        {
            return Ok(None);
        }
        Ok(Some(source.receipt_input()))
    }

    pub(crate) fn paid_service(
        mut self,
        evidence: crate::tools::capability_service::ServiceCommand,
        attribution: crate::tools::capability_service::PaidAttribution,
    ) -> Result<CommandAuthority, EngineError> {
        use crate::tools::capability_service::NativeSource;
        let source = match self.native {
            Native::Channel(source) => {
                if matches!(
                    source.as_ref(),
                    CommandAuthority::Http(_) | CommandAuthority::CapabilityService(_)
                ) || !source.requires_issue_checkpoint()
                {
                    return Err(closed());
                }
                NativeSource::Channel(source)
            }
            Native::Source(source) => NativeSource::HttpSource(source),
        };
        self.native = Native::Channel(Box::new(CommandAuthority::CapabilityService(Box::new(
            evidence.bind_native(source, attribution),
        ))));
        Ok(CommandAuthority::Http(Box::new(self)))
    }
    pub(crate) fn paid_evidence(
        &self,
    ) -> Option<&crate::tools::capability_service::ServiceCommand> {
        match &self.native {
            Native::Channel(source) => source.paid_service(),
            Native::Source(_) => None,
        }
    }
    pub(crate) fn paid_evidence_mut(
        &mut self,
    ) -> Option<&mut crate::tools::capability_service::ServiceCommand> {
        match &mut self.native {
            Native::Channel(source) => source.paid_service_mut(),
            Native::Source(_) => None,
        }
    }
    pub(crate) fn command_kind(&self) -> &'static str {
        self.native.command_kind()
    }
    pub(crate) fn same_origin(&self, other: &Self) -> bool {
        self.original == other.original
            && self.native.same_origin(&other.native)
            && self.canonical == other.canonical
            && self.deadline == other.deadline
            && match (self.sealed_deadline, other.sealed_deadline) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
    }
    fn matches(&self, command: &NewCommand<'_>) -> Result<bool, EngineError> {
        Ok(crate::jobs::command_identity::canonical(
            command.id,
            command.machine_id,
            command.serial_key,
            command.kind,
            command.body,
        )? == self.canonical
            && self
                .deadline
                .is_none_or(|deadline| deadline.get() == command.deadline_at)
            && self
                .sealed_deadline
                .is_none_or(|deadline| deadline == command.deadline_at))
    }
    /// Factual one-layer receipt view. It restores no raw identity and exposes no forward grant.
    pub(crate) fn receipt_native(
        &self,
        row: &CommandRow,
    ) -> Result<Option<&CommandAuthority>, EngineError> {
        let Native::Channel(native) = &self.native else {
            return Ok(None);
        };
        if matches!(native.as_ref(), CommandAuthority::Http(_))
            || !self.original.same_identity(&self.accepted)
            || self
                .sealed_deadline
                .or_else(|| self.deadline.map(Time::get))
                != Some(row.deadline_at)
            || !self.matches(&NewCommand {
                id: row.id,
                machine_id: row.machine_id,
                kind: &row.kind,
                body: &row.body,
                serial_key: row.serial_key(),
                deadline_at: row.deadline_at,
            })?
        {
            return Ok(None);
        }
        Ok(Some(
            native.paid_service_native().unwrap_or(native.as_ref()),
        ))
    }
    pub(crate) async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        self.original.lock_identity(conn).await?;
        self.native.lock_source(conn).await
    }
    async fn retained(
        &self,
        conn: &mut Conn,
        id: orch_types::Id,
        deadline: DateTime<Utc>,
    ) -> Result<(HttpOrigin, Option<Json>), EngineError> {
        self.retained_with_lock(conn, id, deadline, true).await
    }
    async fn retained_with_lock(
        &self,
        conn: &mut Conn,
        id: orch_types::Id,
        deadline: DateTime<Utc>,
        lock: bool,
    ) -> Result<(HttpOrigin, Option<Json>), EngineError> {
        let mut origin = self
            .original
            .intersect(&self.accepted)
            .map_err(|_| closed())?;
        let old = if lock {
            link::lock_command_with_authority(conn, id).await?
        } else {
            link::command_with_authority(conn, id).await?
        };
        let Some(old) = old else {
            return Ok((origin, None));
        };
        let value = old.authority.ok_or_else(closed)?;
        let CommandAuthority::Http(stored) = serde_json::from_value(value.clone())? else {
            return Err(closed());
        };
        if !self.same_origin(&stored)
            || old.command.deadline_at != deadline
            || stored
                .sealed_deadline
                .is_some_and(|sealed| sealed != deadline)
        {
            return Err(closed());
        }
        origin = origin
            .intersect(&stored.original)
            .map_err(|_| closed())?
            .intersect(&stored.accepted)
            .map_err(|_| closed())?;
        Ok((origin, Some(value)))
    }
    async fn save(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        id: orch_types::Id,
        previous: Option<Json>,
        origin: HttpOrigin,
        deadline: DateTime<Utc>,
    ) -> Result<(), EngineError> {
        if let Some(previous) = previous {
            let next = serde_json::to_value(CommandAuthority::Http(Box::new(Self {
                accepted: origin,
                sealed_deadline: Some(deadline),
                ..self.clone()
            })))?;
            if previous != next
                && !link::narrow_command_authority(conn, id, &previous, &next, clock.now()).await?
            {
                return Err(closed());
            }
        }
        Ok(())
    }
    async fn checked(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        stored: Option<&CommandRow>,
        clock: &dyn Clock,
        access: &dyn ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if !self.matches(command)?
            || matches!(&self.native,Native::Channel(source) if matches!(source.as_ref(),CommandAuthority::Http(_)))
        {
            return Ok(None);
        }
        let read_first = self.reads_before_fence();
        let (origin, previous) = self
            .retained_with_lock(conn, command.id, command.deadline_at, !read_first)
            .await?;
        let raw = if read_first {
            None
        } else {
            match origin.current(conn, clock).await {
                Ok(raw) => Some(raw),
                Err(continuing::Error::Refused) => return Ok(None),
                Err(error) => return Err(EngineError::BeforeEffect(Box::new(error))),
            }
        };
        let current = match stored {
            Some(row) => {
                origin
                    .scope(Box::pin(self.native.current(conn, row, clock, access)))
                    .await?
            }
            None => {
                origin
                    .scope(Box::pin(self.native.issue(conn, command, clock, access)))
                    .await?
            }
        };
        let Some(mut callers) = current else {
            return Ok(None);
        };
        let raw = match raw {
            Some(raw) => raw,
            None => {
                // The native frame now holds the source and raw identity. Take the command row
                // in that order and check that its retained restrictions did not move during the read.
                let (fresh, saved) = self.retained(conn, command.id, command.deadline_at).await?;
                if fresh != origin || saved != previous {
                    return Err(orch_db::DbError::Retry.into());
                }
                match origin.current(conn, clock).await {
                    Ok(raw) => raw,
                    Err(continuing::Error::Refused) => return Ok(None),
                    Err(error) => return Err(EngineError::BeforeEffect(Box::new(error))),
                }
            }
        };
        let mut accepted = origin.clone();
        let mut primary = false;
        for (caller, until) in &mut callers {
            *until = (*until).into_iter().chain(raw.until()).min();
            if let Some(proof) = caller
                && proof.user() == origin.origin().user()
                && proof.admitted_operation() == Some(origin.origin().operation())
            {
                let narrowed = raw
                    .caller()
                    .narrow_source_admission(proof)
                    .map_err(|_| closed())?;
                accepted = accepted.accepted(&narrowed, *until).map_err(|_| closed())?;
                *proof = narrowed;
                primary = true;
            }
        }
        if !primary {
            return Ok(None);
        }
        raw.check(clock).map_err(|_| closed())?;
        self.save(
            conn,
            clock,
            command.id,
            previous,
            accepted,
            command.deadline_at,
        )
        .await?;
        raw.check(clock).map_err(|_| closed())?;
        if callers.iter().any(|(caller, until)| {
            caller
                .as_ref()
                .is_some_and(|c| !c.key_deadline_allows(clock))
                || until.is_some_and(|end| clock.time() >= end)
        }) {
            return Ok(None);
        }
        Ok(Some(callers))
    }
    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
        access: &dyn ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.checked(conn, command, None, clock, access).await
    }
    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
        access: &dyn ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.checked(
            conn,
            &NewCommand {
                id: command.id,
                machine_id: command.machine_id,
                kind: &command.kind,
                body: &command.body,
                serial_key: command.serial_key(),
                deadline_at: command.deadline_at,
            },
            Some(command),
            clock,
            access,
        )
        .await
    }
    /// Delivery restores identity/ceilings and leaves completed effects completed.
    /// The consumer still checks its actual byte/read source or truthful receipt boundary.
    pub(crate) async fn delivery(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        id: orch_types::Id,
    ) -> Result<HttpOrigin, EngineError> {
        // Completed effect sources may be gone. Only the actual raw identity is restored here;
        // the consumer separately checks response Read/source or its truthful accepted receipt.
        self.original.lock_identity(conn).await?;
        let row = link::command(conn, id).await?.ok_or_else(closed)?;
        let (origin, previous) = self.retained(conn, id, row.deadline_at).await?;
        if !self.matches(&NewCommand {
            id: row.id,
            machine_id: row.machine_id,
            kind: &row.kind,
            body: &row.body,
            serial_key: row.serial_key(),
            deadline_at: row.deadline_at,
        })? {
            return Err(closed());
        }
        let raw = origin
            .current(conn, clock)
            .await
            .map_err(|error| EngineError::BeforeEffect(Box::new(error)))?;
        let origin = origin
            .with_ceiling(raw.caller(), raw.until())
            .map_err(|_| closed())?;
        raw.check(clock).map_err(|_| closed())?;
        self.save(conn, clock, id, previous, origin.clone(), row.deadline_at)
            .await?;
        raw.check(clock).map_err(|_| closed())?;
        Ok(origin)
    }
}
