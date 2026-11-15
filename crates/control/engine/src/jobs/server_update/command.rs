//! The exact accepted server-update job and original system administrator at every native command boundary.
use super::{KIND, Request};
use crate::{
    EngineError,
    commands::Outgoing,
    operation_scope::{continuing, http_origin::HttpOrigin},
};
use orch_authz::Caller;
use orch_db::{
    Conn, DbError,
    queries::{
        jobs,
        link::{CommandRow, NewCommand},
    },
};
use orch_link_contract::commands::{CommandKind, NodeServerUpdate, SerialKey};
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    job: Id,
    request: Request,
    canonical: Json,
}
impl Source {
    pub fn of(job: Id, request: &Request, command: &Outgoing) -> Result<Self, EngineError> {
        let body: NodeServerUpdate = serde_json::from_value(command.body.clone())?;
        if command.kind != CommandKind::NodeServerUpdate
            || command.key != SerialKey::Machine
            || command.machine != request.machine
            || body.job_id != job
            || body.release != request.release
            || body.timeout_secs != request.prepare_timeout_secs
        {
            return Err(EngineError::unreadable(
                "the server update changed its native job or selected release",
            ));
        }
        Ok(Self {
            job,
            request: request.clone(),
            canonical: crate::jobs::command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
        })
    }
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        sqlx::query("SELECT id FROM jobs WHERE id=$1 FOR SHARE")
            .bind(self.job)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?;
        Ok(())
    }
    async fn current(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.kind != "node.server.update"
            || crate::jobs::command_identity::canonical(
                command.id,
                command.machine_id,
                command.serial_key,
                command.kind,
                command.body,
            )? != self.canonical
        {
            return Ok(None);
        }
        let Some(row) = jobs::job(conn, self.job).await? else {
            return Ok(None);
        };
        if row.kind != KIND
            || !row.is_open()
            || serde_json::from_value::<Request>(row.plan)? != self.request
        {
            return Ok(None);
        }
        let origin: &HttpOrigin = &self.request.origin;
        let current = match origin.current(conn, clock).await {
            Ok(current) => current,
            Err(continuing::Error::Refused) => return Ok(None),
            Err(error) => return Err(EngineError::unreadable(error)),
        };
        if !current.user().is_system_admin
            || !matches!(
                origin.credential(),
                crate::operation_scope::request_origin::Credential::Session { .. }
            )
        {
            return Ok(None);
        }
        let bundled: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM hosts h JOIN machines m ON m.id=h.machine_id WHERE h.id=$1 AND h.machine_id=$2 AND h.bundled_with_server AND m.deleted_at IS NULL AND m.status='active')")
            .bind(self.request.host).bind(self.request.machine).fetch_one(&mut *conn).await.map_err(DbError::from)?;
        if !bundled {
            return Ok(None);
        }
        current.check(clock).map_err(EngineError::unreadable)?;
        Ok(Some(vec![(
            Some(current.caller().clone()),
            current.until(),
        )]))
    }
    pub async fn current_issue(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current(conn, command, clock).await
    }
    pub async fn current_stored(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current(
            conn,
            &NewCommand {
                id: command.id,
                machine_id: command.machine_id,
                kind: &command.kind,
                body: &command.body,
                serial_key: command.serial_key(),
                deadline_at: command.deadline_at,
            },
            clock,
        )
        .await
    }
}
