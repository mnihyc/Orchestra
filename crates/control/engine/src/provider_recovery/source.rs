//! A newly accepted provider recovery keeps its real confirmation through native execution.
use super::*;
use crate::operation_scope::{
    continuing::{self, Error, PreparedFuture},
    http_origin::HttpOrigin,
    sources::Facts,
    targets,
};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_api_contract::{
    operation_sources::{Binding, Cardinality, Declaration},
    operations::{FacetUsage, TargetResolver},
    resources::ProviderRuntimeUpdateCreate,
    routes::{Endpoint, Route},
};
use orch_authz::Caller;
use orch_types::{Clock, Hash, Time};

fn closed() -> EngineError {
    EngineError::BeforeEffect(Box::new(continuing::Error::Refused))
}
fn mapped(error: Error) -> EngineError {
    match error {
        Error::Native(error) => error,
        error => EngineError::BeforeEffect(Box::new(error)),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    owner: Id,
    home: Id,
    config: Hash,
    runtime: Option<Id>,
}
impl Snapshot {
    async fn load(conn: &mut Conn, provider: Id) -> Result<Self, EngineError> {
        let row = rows::state(conn, provider)
            .await?
            .filter(|row| row.deleted_at.is_none())
            .ok_or_else(closed)?;
        Ok(Self {
            owner: row.owner_id,
            home: row.home_id,
            config: crate::jobs::command_identity::hash(&json!([
                row.kind,
                row.protocols,
                row.harness_kind,
                row.agent_id,
                row.config,
                row.runtime_config
            ]))?,
            runtime: rows::live_runtime(conn, provider)
                .await?
                .map(|runtime| runtime.id),
        })
    }
}
/// Private factual source; it is captured only from the actual confirmation and newly queued job.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    http: HttpOrigin,
    job: Id,
    kind: String,
    recipe: Recipe,
    snapshot: Snapshot,
}
impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProviderRecoverySource([private])")
    }
}
impl Source {
    pub(crate) fn origin(&self) -> &HttpOrigin {
        &self.http
    }
    pub(crate) fn operation(&self) -> &'static str {
        "ProviderRuntimeUpdateCreate"
    }
    pub(crate) fn matches(&self, row: &jobs::JobRow) -> Result<bool, EngineError> {
        Ok(row.id == self.job
            && row.kind == self.kind
            && row.kind == jobs::kind::PROVIDER_PROVISION
            && row.provider_id == Some(self.recipe.provider)
            && row.environment_id.is_none()
            && row.created_by == Some(self.http.origin().user())
            && self.recipe.by == self.http.origin().user()
            && self.http.origin().operation() == self.operation()
            && row.plan.get("mode").and_then(Json::as_str) == Some("make")
            && row.plan.get("provider_recovery") == Some(&serde_json::to_value(&self.recipe)?))
    }
    pub(crate) async fn lock_source(
        &self,
        conn: &mut Conn,
        row: &jobs::JobRow,
    ) -> Result<(), EngineError> {
        if !self.matches(row)? {
            return Err(closed());
        }
        self.http.lock_identity(conn).await?;
        rows::lock(conn, self.recipe.provider)
            .await?
            .ok_or_else(closed)?;
        Ok(())
    }
    async fn physical(&self, conn: &mut Conn, row: &jobs::JobRow) -> Result<Snapshot, EngineError> {
        if !self.matches(row)? || !row.is_open() || row.cancel_requested() {
            return Err(closed());
        }
        let current = Snapshot::load(conn, self.recipe.provider).await?;
        if current.owner != self.snapshot.owner
            || current.home != self.snapshot.home
            || current.config != self.snapshot.config
        {
            return Err(closed());
        }
        let planned = row
            .plan
            .get("runtime")
            .filter(|value| !value.is_null())
            .map(|value| serde_json::from_value::<Id>(value.clone()))
            .transpose()
            .map_err(|_| closed())?;
        if planned != current.runtime {
            return Err(closed());
        }
        if current.runtime != self.snapshot.runtime {
            let id = current.runtime.ok_or_else(closed)?;
            if row.plan.get("runtime") != Some(&json!(id)) {
                return Err(closed());
            }
            let runtime = orch_db::queries::environments::hosting::runtime(conn, id)
                .await?
                .ok_or_else(closed)?;
            if runtime.provider_id != Some(self.recipe.provider)
                || runtime.environment_id.is_some()
                || runtime.kind != "provider"
                || Some(runtime.host_id)
                    != super::selected_host(conn, row, &self.recipe.host).await?
                || runtime.template_id != self.recipe.template
                || runtime.successor_of.is_some()
                || matches!(runtime.state.as_str(), "retired" | "removed")
            {
                return Err(closed());
            }
        }
        Ok(current)
    }
    pub(crate) async fn principal(
        &self,
        conn: &mut Conn,
        row: &jobs::JobRow,
        clock: &dyn Clock,
        selection: crate::jobs::environment_move::preparation::execution::Selection,
    ) -> Result<Principal, EngineError> {
        use crate::jobs::environment_move::preparation::execution::Selection;
        match selection {
            Selection::Current => {}
            Selection::Host(host)
                if super::selected_host(conn, row, &self.recipe.host).await? == Some(host) => {}
            _ => return Err(closed()),
        }
        self.physical(conn, row).await?;
        let frame = continuing::checkpoint::<Native>(
            conn,
            clock,
            &self.http,
            Input {
                source: self.clone(),
            },
        )
        .await
        .map_err(mapped)?;
        frame.check_deadline(clock).map_err(mapped)?;
        Ok(frame.principal().clone())
    }
    async fn capture(
        conn: &mut Conn,
        row: &jobs::JobRow,
        http: &HttpOrigin,
        clock: &dyn Clock,
    ) -> Result<Self, EngineError> {
        if !orch_db::queries::transaction_guard::in_transaction(conn) || row.state != "queued" {
            return Err(closed());
        }
        let made_here:bool=sqlx::query_scalar("SELECT xmin=((pg_current_xact_id()::text::numeric % 4294967296)::text)::xid FROM jobs WHERE id=$1")
            .bind(row.id).fetch_one(&mut *conn).await.map_err(DbError::from)?;
        if !made_here {
            return Err(closed());
        }
        let recipe: Recipe = serde_json::from_value(
            row.plan
                .get("provider_recovery")
                .cloned()
                .ok_or_else(closed)?,
        )?;
        let source = Self {
            http: http.clone(),
            job: row.id,
            kind: row.kind.clone(),
            snapshot: Snapshot::load(conn, recipe.provider).await?,
            recipe,
        };
        if source.snapshot.runtime.is_some() || !source.matches(row)? {
            return Err(closed());
        }
        source
            .principal(
                conn,
                row,
                clock,
                crate::jobs::environment_move::preparation::execution::Selection::Current,
            )
            .await?;
        Ok(source)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Provider,
}
const DECLARATION: Declaration<Role> = Declaration {
    operation: "ProviderRuntimeUpdateCreate",
    bindings: &[Binding {
        source: Role::Provider,
        target: TargetResolver::PermissionObject,
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};
struct Input {
    source: Source,
}
struct Native;
impl continuing::Source for Native {
    type Role = Role;
    type Input = Input;
    type Value = Snapshot;
    const ROUTE: Route = ProviderRuntimeUpdateCreate::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &DECLARATION;
    fn prepare<'a>(
        conn: &'a mut Conn,
        _: &'a Caller,
        input: &'a Input,
        _: Time,
    ) -> PreparedFuture<'a, Snapshot, Role> {
        Box::pin(async move {
            let row = jobs::job(conn, input.source.job)
                .await?
                .ok_or(Error::Refused)?;
            let snapshot = input.source.physical(conn, &row).await?;
            let target = targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Provider,
                    id: input.source.recipe.provider,
                },
            )
            .await
            .map_err(EngineError::unreadable)?;
            Ok((
                snapshot,
                vec![Facts::one(
                    Role::Provider,
                    KeyTargetKind::Object(ObjectKind::Provider),
                    target,
                )],
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        _: &'a Snapshot,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let mut needs = vec![
                (
                    ObjectKind::Provider,
                    input.source.recipe.provider,
                    Permission::ProviderManage,
                ),
                (
                    ObjectKind::Template,
                    input.source.recipe.template,
                    Permission::TemplateUse,
                ),
            ];
            let row = jobs::job(conn, input.source.job)
                .await?
                .ok_or(Error::Refused)?;
            let host = super::selected_host(conn, &row, &input.source.recipe.host).await?;
            match &input.source.recipe.host {
                Target::Single {
                    resource: Leaf::Host { id },
                } => needs.push((ObjectKind::Host, *id, Permission::HostUse)),
                Target::Pool {
                    id,
                    kind: Kind::Host,
                } => needs.push((ObjectKind::Pool, *id, Permission::PoolUse)),
                _ => return Err(Error::Refused),
            }
            for (kind, id, permission) in needs {
                match operation_access::review(conn, principal, ObjectRef { kind, id }, permission)
                    .await
                {
                    Ok(None) => {}
                    Err(UpdateError::Internal(error)) => return Err(Error::Native(error)),
                    _ => return Err(Error::Refused),
                }
            }
            if matches!(input.source.recipe.host, Target::Pool { .. }) {
                if let Some(host) = host
                    && !crate::jobs::provider_provision::current_pool_placement(
                        conn, principal, &row, host,
                    )
                    .await?
                {
                    return Err(Error::Refused);
                }
            } else if let Some(host) = host
                && !crate::updates::provider_owner_host_usable(
                    conn,
                    input.source.recipe.provider,
                    host,
                    principal.now(),
                )
                .await?
            {
                return Err(Error::Refused);
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Input,
        _: &'a Snapshot,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let row = jobs::job(conn, input.source.job)
                .await?
                .ok_or(Error::Refused)?;
            input.source.lock_source(conn, &row).await?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        opened: &'a Snapshot,
        fresh: &'a Snapshot,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(opened == fresh) })
    }
}

impl Engine {
    /// Retain the real singleton confirmation on its actual newly queued provider recovery.
    pub async fn retain_provider_recovery_source(
        &self,
        conn: &mut Conn,
        row: &jobs::JobRow,
        http: &HttpOrigin,
    ) -> Result<(), EngineError> {
        let current = jobs::job(conn, row.id).await?.ok_or_else(closed)?;
        let source = Source::capture(conn, &current, http, self.inner.clock.as_ref()).await?;
        crate::jobs::lifecycle_commands::environment::retain(
            conn,
            row.id,
            crate::jobs::environment_move::preparation::execution::Evidence::ProviderRecovery(
                Box::new(source),
            ),
            self.inner.clock.as_ref(),
            self.inner.access.as_ref(),
        )
        .await
    }
}
