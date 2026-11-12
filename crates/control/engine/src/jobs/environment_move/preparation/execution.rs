//! Accepted native moves keep their real source through the shared lifecycle channel.
use super::{Kind, Origin, closed};
use crate::{EngineError, access::ResourceAccess, operation_scope::http_origin::HttpOrigin};
use orch_db::{
    Conn,
    queries::{
        environments::hosting as envs,
        jobs::{self, JobRow},
        resources::hosting as hosts,
    },
};
use orch_types::{Clock, Hash, Id, Json};
use serde::{Deserialize, Serialize};

/// The actual native boundary whose complete factual roles must be prepared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    Current,
    Host(Id),
    Wake(Id),
}

/// A native lifecycle source ended; storage and engine failures keep their own categories.
#[derive(Debug, thiserror::Error)]
#[error("the original environment lifecycle source ended")]
pub struct Ended;

mod identity;
pub(super) use identity::JobIdentity;
mod parent;
pub use parent::ParentSource;
mod provider;
pub use provider::ProviderParentSource;

/// Closed native evidence is produced only from the real queued job and original confirmation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "source",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Evidence {
    Move(Box<Origin>),
    Recovery(Box<Origin>),
    Parent(Box<ParentSource>),
    ProviderParent(Box<ProviderParentSource>),
}
impl Evidence {
    pub async fn capture(
        conn: &mut Conn,
        row: &JobRow,
        source: &Origin,
        clock: &dyn Clock,
        prepared: Option<&crate::recovery::PreparedQueue>,
    ) -> Result<Self, EngineError> {
        if !matches!(source.kind(), Kind::MoveCreate | Kind::RecoverCreate) || row.state != "queued"
        {
            return Err(closed());
        }
        let inserted: bool = sqlx::query_scalar("SELECT xmin=((pg_current_xact_id()::text::numeric % 4294967296)::text)::xid FROM jobs WHERE id=$1")
            .bind(row.id).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
        if !inserted {
            return Err(closed());
        }
        let accepted = Box::new(source.for_job(conn, row, clock, prepared).await?);
        Ok(match source.kind() {
            Kind::MoveCreate => Self::Move(accepted),
            Kind::RecoverCreate => Self::Recovery(accepted),
            _ => return Err(closed()),
        })
    }
    fn source(&self) -> Result<&Origin, EngineError> {
        match self {
            Self::Move(source) | Self::Recovery(source) => Ok(source),
            Self::Parent(_) | Self::ProviderParent(_) => Err(closed()),
        }
    }
    fn identity(&self) -> Result<&JobIdentity, EngineError> {
        self.source()?.execution().ok_or_else(closed)
    }
    pub fn origin(&self) -> &HttpOrigin {
        match self {
            Self::Move(source) | Self::Recovery(source) => source.http(),
            Self::Parent(source) => source.origin(),
            Self::ProviderParent(source) => source.origin(),
        }
    }
    /// Closed producer identity; a parent can never borrow a global move/recovery selection.
    pub(crate) fn permits_operation(&self, operation: &str) -> bool {
        match self {
            Self::Move(source) => {
                operation == "EnvironmentMoveCreate" && source.kind() == Kind::MoveCreate
            }
            Self::Recovery(source) => {
                operation == "EnvironmentRecoverCreate" && source.kind() == Kind::RecoverCreate
            }
            Self::Parent(source) => operation == source.operation(),
            Self::ProviderParent(source) => operation == source.operation(),
        }
    }
    pub(crate) fn matches_row(&self, row: &JobRow) -> Result<bool, EngineError> {
        match self {
            Self::Parent(source) => source.matches(row),
            Self::ProviderParent(source) => source.matches(row),
            _ => self.identity()?.matches(row),
        }
    }
    pub(crate) async fn owns_provider_candidate(
        &self,
        conn: &mut Conn,
        row: &JobRow,
    ) -> Result<bool, EngineError> {
        match self {
            Self::ProviderParent(source) => source.owns_candidate(conn, row).await,
            _ => Ok(false),
        }
    }
    pub(crate) fn is_provider(&self) -> bool {
        matches!(self, Self::ProviderParent(_))
    }
    pub(crate) async fn current_origin(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<HttpOrigin, EngineError> {
        match self {
            Self::Parent(source) => source.current_http(conn, clock).await,
            Self::ProviderParent(source) => source.current_http(conn, clock).await,
            _ => Ok(self.origin().clone()),
        }
    }
    pub async fn lock_source(&self, conn: &mut Conn, row: &JobRow) -> Result<(), EngineError> {
        if let Self::Parent(source) = self {
            return source.lock_source(conn, row).await;
        }
        if let Self::ProviderParent(source) = self {
            return source.lock_source(conn, row).await;
        }
        if !self.identity()?.matches(row)? {
            return Err(closed());
        }
        self.source()?.lock_source(conn).await
    }
    /// The same native Source supplies the full current source and destination roles.
    pub async fn principal(
        &self,
        conn: &mut Conn,
        row: &JobRow,
        clock: &dyn Clock,
        _access: &dyn ResourceAccess,
        selection: Selection,
    ) -> Result<orch_authz::Principal, EngineError> {
        self.current_principal(conn, row, clock, selection).await
    }
    /// All classes share one current native source; provider workers need no environment service.
    pub(crate) fn current_principal<'a>(
        &'a self,
        conn: &'a mut Conn,
        row: &'a JobRow,
        clock: &'a dyn Clock,
        selection: Selection,
    ) -> futures::future::BoxFuture<'a, Result<orch_authz::Principal, EngineError>> {
        Box::pin(async move {
            if let Self::Parent(source) = self {
                return source.principal(conn, row, clock, selection).await;
            }
            if let Self::ProviderParent(source) = self {
                return source.principal(conn, row, clock, selection).await;
            }
            if !self.identity()?.matches(row)? {
                return Err(closed());
            }
            let selected_host = match selection {
                Selection::Host(host) => Some(host),
                _ => None,
            };
            match selection {
                Selection::Current => {}
                Selection::Host(_host) => {}
                Selection::Wake(environment)
                    if self.identity()?.wake_matches(conn, environment).await? => {}
                _ => return Err(closed()),
            }
            let source = self
                .source()?
                .for_execution_frame(conn, selected_host)
                .await?;
            let check = source.checkpoint(conn, clock).await?;
            let principal =
                orch_authz::Principal::load(conn, check.caller().clone(), clock.time()).await?;
            check.check(clock)?;
            Ok(principal)
        })
    }
    /// Only the exact committed old unit or registration is owned cleanup.
    pub async fn cleanup_matches(
        &self,
        conn: &mut Conn,
        row: &JobRow,
        machine: Id,
        key: Option<&str>,
        kind: &str,
        body: &Json,
    ) -> Result<bool, EngineError> {
        if let Self::Parent(source) = self {
            return source
                .cleanup_matches(conn, row, machine, key, kind, body)
                .await;
        }
        // The departing Host owns retirement of the old Provider unit.
        if matches!(self, Self::ProviderParent(_)) {
            return Ok(false);
        }
        let identity = self.identity()?;
        if !identity.matches(row)?
            || row.plan.get("committed").and_then(Json::as_bool) != Some(true)
            || crate::jobs::runner::read_progress(row).0.phase != "cleanup"
        {
            return Ok(false);
        }
        let Some(old) = identity.old_runtime() else {
            return Ok(identity.portable()
                && identity.source_machine() == Some(machine)
                && kind == "env.detach"
                && key
                    == orch_link_contract::commands::SerialKey::Environment(
                        identity.environment(),
                    )
                    .wire()
                    .as_deref()
                && body.get("environment_id") == Some(&serde_json::json!(identity.environment())));
        };
        if !self
            .source()?
            .source(identity.environment())
            .is_some_and(|source| source.runtime_matches(machine, old))
        {
            return Ok(false);
        }
        let Some(runtime) = envs::runtime(conn, old).await?.filter(|r| {
            r.environment_id == Some(identity.environment())
                && r.successor_of.is_none()
                && r.state == "retired"
        }) else {
            return Ok(false);
        };
        let Some(host) = hosts::host(conn, runtime.host_id).await? else {
            return Ok(false);
        };
        Ok(kind == "node.runtime.stop"
            && machine == host.machine_id
            && key
                == orch_link_contract::commands::SerialKey::Runtime(old)
                    .wire()
                    .as_deref()
            && body.get("runtime_id") == Some(&serde_json::json!(old)))
    }
    /// The native maker journal names the exact new provision or existing-destination start.
    pub async fn child(
        &self,
        conn: &mut Conn,
        parent: &JobRow,
        child: &JobRow,
        clock: &dyn Clock,
        _access: &dyn ResourceAccess,
    ) -> Result<Self, EngineError> {
        if let Self::Parent(source) = self {
            return Ok(Self::Parent(Box::new(
                source.child(conn, parent, child).await?,
            )));
        }
        if matches!(self, Self::ProviderParent(_)) {
            return Err(closed());
        }
        if !matches!(self, Self::Recovery(_)) || !self.identity()?.matches(parent)? {
            return Err(closed());
        }
        let source = self.source()?.for_child(conn, parent, child, clock).await?;
        Ok(Self::Recovery(Box::new(source)))
    }
}

impl crate::Engine {
    /// Keep a reviewed Replace/Make Provider child on its real Host confirmation.
    /// Leave remains the parent's own Host control and is refused by this child port.
    pub async fn retain_host_delete_provider_child(
        &self,
        conn: &mut Conn,
        parent: &JobRow,
        child: Id,
        reviewed: &crate::jobs::host_delete::reviewed::ProviderAction,
    ) -> Result<(), EngineError> {
        let child_row = jobs::job(conn, child).await?.ok_or_else(closed)?;
        let source = ProviderParentSource::capture(
            conn,
            parent,
            &child_row,
            reviewed,
            self.inner.clock.as_ref(),
        )
        .await?;
        crate::jobs::lifecycle_commands::environment::retain(
            conn,
            child,
            Evidence::ProviderParent(Box::new(source)),
            self.inner.clock.as_ref(),
            self.inner.access.as_ref(),
        )
        .await
    }
    /// Keep the actual Host confirmation on a newly queued environment move/recovery child.
    pub async fn retain_host_delete_environment_child(
        &self,
        conn: &mut Conn,
        parent: &JobRow,
        child: Id,
    ) -> Result<(), EngineError> {
        let child_row = jobs::job(conn, child).await?.ok_or_else(closed)?;
        let source =
            ParentSource::capture_host(conn, parent, &child_row, self.inner.clock.as_ref()).await?;
        crate::jobs::lifecycle_commands::environment::retain(
            conn,
            child,
            Evidence::Parent(Box::new(source)),
            self.inner.clock.as_ref(),
            self.inner.access.as_ref(),
        )
        .await
    }
    /// Retain a node move through the same lifecycle channel, without a global-operation alias.
    pub async fn retain_node_environment_move_source(
        &self,
        conn: &mut Conn,
        row: &JobRow,
    ) -> Result<(), EngineError> {
        let source = ParentSource::capture_node(conn, row, self.inner.clock.as_ref()).await?;
        crate::jobs::lifecycle_commands::environment::retain(
            conn,
            row.id,
            Evidence::Parent(Box::new(source)),
            self.inner.clock.as_ref(),
            self.inner.access.as_ref(),
        )
        .await
    }
    /// Retain one actual native recovery member on its original queue transaction.
    pub async fn retain_recovery_lifecycle_source(
        &self,
        conn: &mut Conn,
        job: Id,
        source: &Origin,
        prepared: Option<&crate::recovery::PreparedQueue>,
    ) -> Result<(), EngineError> {
        let row = jobs::job(conn, job).await?.ok_or_else(closed)?;
        let evidence =
            Evidence::capture(conn, &row, source, self.inner.clock.as_ref(), prepared).await?;
        crate::jobs::lifecycle_commands::environment::retain(
            conn,
            job,
            evidence,
            self.inner.clock.as_ref(),
            self.inner.access.as_ref(),
        )
        .await
    }
    /// Retain the actual newly queued move in the existing lifecycle command family.
    pub async fn retain_environment_move_source(
        &self,
        conn: &mut Conn,
        row: &JobRow,
        source: &Origin,
    ) -> Result<(), EngineError> {
        let evidence =
            Evidence::capture(conn, row, source, self.inner.clock.as_ref(), None).await?;
        crate::jobs::lifecycle_commands::environment::retain(
            conn,
            row.id,
            evidence,
            self.inner.clock.as_ref(),
            self.inner.access.as_ref(),
        )
        .await
    }
}
