//! The exact successful copy's unfiled native definition, and only its owned inverse.

use super::CopyCommand;
use crate::{EngineError, commands::Outgoing};
use orch_authz::Caller;
use orch_db::{
    Conn, DbError,
    queries::{
        link::{CommandRow, NewCommand},
        resources::mcp,
    },
};
use orch_link_contract::commands::{CommandBody, McpForget, SerialKey};
use orch_types::{Clock, Json, Time};
use serde::{Deserialize, Serialize};

/// Native inverse custody, with no actor or renewed authority to create anything.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgetCommand {
    copy: CopyCommand,
    canonical: Json,
    until: Time,
}
impl std::fmt::Debug for ForgetCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpForgetCommand([owned])")
    }
}
impl ForgetCommand {
    /// Own only a real successful, canonical, privately recorded copy which was never filed.
    /// The ordinary native Forget budget bounds its cleanup; the ended invoker is not restored.
    pub async fn capture(
        conn: &mut Conn,
        clock: &dyn Clock,
        copy: CopyCommand,
        command: &Outgoing,
    ) -> Result<Self, EngineError> {
        if command.id == copy.id()
            || command.kind != McpForget::KIND
            || command.machine != copy.input().machine()
            || command.key != SerialKey::Machine
            || command.body
                != serde_json::to_value(McpForget {
                    server_id: copy.input().server(),
                })?
            || !copy.completed(conn).await?
            || mcp::recorded(conn, copy.input().server()).await?
        {
            return Err(EngineError::unreadable(
                "the native cleanup does not own an unfiled successful MCP copy",
            ));
        }
        Ok(Self {
            copy,
            canonical: crate::jobs::command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
            until: Time::new(
                clock.now() + chrono::Duration::seconds(i64::from(McpForget::KIND.deadline_secs())),
            ),
        })
    }

    /// The existing native Forget deadline, retained through queue and dispatch waits.
    pub fn deadline(&self) -> Time {
        self.until
    }

    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.kind != "mcp.forget"
            || command.deadline_at != self.until.get()
            || self.until.get() <= clock.now()
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
        // Filing and its owned inverse serialize on the same actual Environment.
        // A completed public server is never undone by this private copy cleanup.
        orch_db::queries::transaction_guard::lock_action(conn).await?;
        sqlx::query("SELECT id FROM environments WHERE id=$1 FOR UPDATE")
            .bind(self.copy.input().environment())
            .execute(&mut *conn)
            .await
            .map_err(DbError::from)?;
        if !self.copy.completed(conn).await?
            || mcp::recorded(conn, self.copy.input().server()).await?
            || self.until.get() <= clock.now()
        {
            return Ok(None);
        }
        Ok(Some(vec![(None, Some(self.until))]))
    }

    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current_issue_callers(
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
