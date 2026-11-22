//! Exact Restart operation and raw transport through the existing lifecycle evidence channel.
use super::Plan;
use crate::{
    EngineError,
    operation_scope::{
        continuing::{Error, Source},
        http_origin::HttpOrigin,
        sources::{self, Facts},
        targets,
    },
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::KeyTargetKind,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    environment::EnvironmentRestart,
    operation_sources::{Binding, Cardinality, Declaration},
    operations::{FacetUsage, TargetResolver},
    routes::{Endpoint, Route},
};
use orch_authz::{Caller, Policy, Principal};
use orch_db::{
    Conn,
    queries::{environments::hosting as envs, jobs, resources::hosting as hosts},
};
use orch_types::{Clock, Hash, Id, Time};
use serde::{Deserialize, Serialize};

fn closed() -> EngineError {
    EngineError::BeforeEffect(Box::new(Error::Refused))
}
fn mapped(error: Error) -> EngineError {
    match error {
        Error::Native(error) => error,
        error => EngineError::BeforeEffect(Box::new(error)),
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Environment,
}
const DECLARATION: Declaration<Role> = Declaration {
    operation: "EnvironmentRestart",
    bindings: &[Binding {
        source: Role::Environment,
        target: TargetResolver::PermissionObject,
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};

/// Initial canonical producer admission, never a replacement operation for deferred work.
pub async fn admit(
    conn: &mut Conn,
    caller: &Caller,
    environment: Id,
) -> Result<Caller, EngineError> {
    if caller
        .admitted_operation()
        .is_some_and(|operation| operation != "EnvironmentRestart")
    {
        return Err(closed());
    }
    let target = targets::resource_identity(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
    )
    .await
    .map_err(EngineError::unreadable)?;
    sources::bind_required(
        EnvironmentRestart::ROUTE,
        &DECLARATION,
        &[Facts::one(
            Role::Environment,
            KeyTargetKind::Object(ObjectKind::Environment),
            target,
        )],
    )
    .map_err(|error| EngineError::BeforeEffect(Box::new(error)))?
    .admit(caller, false)
    .map_err(|error| EngineError::BeforeEffect(Box::new(error)))
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    http: HttpOrigin,
    job: Id,
    environment: Id,
    runtime: Id,
    host: Id,
    owner: Id,
    home: Id,
    generation: i32,
    recipe: Hash,
}
impl std::fmt::Debug for Evidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RestartSource([private])")
    }
}
fn plan(row: &jobs::JobRow) -> Result<Plan, EngineError> {
    if row.kind != jobs::kind::ENVIRONMENT_RESTART {
        return Err(closed());
    }
    Ok(serde_json::from_value(row.plan.clone())?)
}
fn hash(plan: &Plan) -> Result<Hash, EngineError> {
    crate::jobs::command_identity::hash(&serde_json::to_value(plan)?)
}
impl Evidence {
    pub(crate) async fn capture(
        conn: &mut Conn,
        row: &jobs::JobRow,
        http: &HttpOrigin,
    ) -> Result<Self, EngineError> {
        let plan = plan(row)?;
        let born: bool = sqlx::query_scalar("SELECT xmin=((pg_current_xact_id()::text::numeric % 4294967296)::text)::xid FROM jobs WHERE id=$1")
            .bind(row.id).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
        if !born
            || row.state != "queued"
            || row.created_by != Some(http.origin().user())
            || http.origin().operation() != "EnvironmentRestart"
            || plan.caller != http.origin().user()
            || row.environment_id != Some(plan.environment)
        {
            return Err(closed());
        }
        let env = envs::environment(conn, plan.environment)
            .await?
            .ok_or_else(closed)?;
        Ok(Self {
            http: http.clone(),
            job: row.id,
            environment: plan.environment,
            runtime: plan.runtime,
            host: plan.host,
            owner: env.owner_id,
            home: env.home_node_id,
            generation: env.generation,
            recipe: hash(&plan)?,
        })
    }
    pub(crate) fn origin(&self) -> &HttpOrigin {
        &self.http
    }
    pub(crate) fn matches(&self, row: &jobs::JobRow) -> Result<bool, EngineError> {
        if row.id != self.job
            || row.created_by != Some(self.http.origin().user())
            || row.environment_id != Some(self.environment)
        {
            return Ok(false);
        }
        let plan = plan(row)?;
        Ok(self.http.origin().operation() == "EnvironmentRestart"
            && hash(&plan)? == self.recipe
            && plan.environment == self.environment
            && plan.runtime == self.runtime
            && plan.host == self.host
            && plan.generation == self.generation
            && plan.caller == self.http.origin().user())
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
        sqlx::query("SELECT id FROM jobs WHERE id=$1 FOR SHARE")
            .bind(self.job)
            .execute(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
        envs::lock_environment(conn, self.environment)
            .await?
            .ok_or_else(closed)?;
        Ok(())
    }
    pub(crate) async fn principal(
        &self,
        conn: &mut Conn,
        row: &jobs::JobRow,
        clock: &dyn Clock,
        selection: crate::jobs::lifecycle_commands::environment::Selection,
    ) -> Result<Principal, EngineError> {
        use crate::jobs::lifecycle_commands::environment::Selection;
        if !self.matches(row)?
            || !match selection {
                Selection::Current => true,
                Selection::Host(host) => host == self.host,
                Selection::Wake(environment) => environment == self.environment,
            }
        {
            return Err(closed());
        }
        let frame = self
            .http
            .prepare::<Native>(conn, clock, self.clone())
            .await
            .map_err(mapped)?;
        let parts = frame.into_parts();
        parts.first_effect.check(clock).map_err(mapped)?;
        Ok(parts.principal)
    }
}
#[derive(Clone, PartialEq, Eq)]
struct Value {
    environment: Id,
    runtime: Id,
    host: Id,
    owner: Id,
    home: Id,
    generation: i32,
}
struct Native;
impl Source for Native {
    type Role = Role;
    type Input = Evidence;
    type Value = Value;
    const ROUTE: Route = EnvironmentRestart::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &DECLARATION;
    fn prepare<'a>(
        conn: &'a mut Conn,
        _: &'a Caller,
        input: &'a Evidence,
        _: Time,
    ) -> BoxFuture<'a, Result<(Value, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let row = jobs::job(conn, input.job).await?.ok_or(Error::Refused)?;
            if !row.is_open() || row.cancel_requested() || !input.matches(&row)? {
                return Err(Error::Refused);
            }
            let env = envs::environment(conn, input.environment)
                .await?
                .filter(|env| env.deleted_at.is_none() && env.status != "disabled")
                .ok_or(Error::Refused)?;
            let runtime = envs::live_runtime(conn, input.environment)
                .await?
                .filter(|runtime| runtime.id == input.runtime && runtime.host_id == input.host)
                .ok_or(Error::Refused)?;
            if env.owner_id != input.owner
                || env.home_node_id != input.home
                || env.generation != input.generation
                || !orch_db::queries::runtime_controls::is_current(conn, &plan(&row)?.control)
                    .await?
                || hosts::host(conn, input.host)
                    .await?
                    .is_none_or(|host| host.disabled)
            {
                return Err(Error::Refused);
            }
            let target = targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: env.id,
                },
            )
            .await
            .map_err(EngineError::unreadable)?;
            Ok((
                Value {
                    environment: env.id,
                    runtime: runtime.id,
                    host: input.host,
                    owner: env.owner_id,
                    home: env.home_node_id,
                    generation: env.generation,
                },
                vec![Facts::one(
                    Role::Environment,
                    KeyTargetKind::Object(ObjectKind::Environment),
                    target,
                )],
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Evidence,
        value: &'a Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if !orch_authz::resources::check(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: value.environment,
                },
                Permission::EnvironmentManage,
                Policy::default(),
            )
            .await?
            .is_allowed()
            {
                return Err(Error::Refused);
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Evidence,
        _: &'a Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let row = jobs::job(conn, input.job).await?.ok_or(Error::Refused)?;
            input.lock_source(conn, &row).await?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Evidence,
        opened: &'a Value,
        fresh: &'a Value,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(opened == fresh) })
    }
}

impl crate::Engine {
    pub(crate) async fn retain_restart_source(
        &self,
        conn: &mut Conn,
        row: &jobs::JobRow,
        http: &HttpOrigin,
    ) -> Result<(), EngineError> {
        let source = Evidence::capture(conn, row, http).await?;
        crate::jobs::lifecycle_commands::environment::retain(
            conn,
            row.id,
            crate::jobs::environment_move::preparation::execution::Evidence::Restart(Box::new(
                source,
            )),
            self.inner.clock.as_ref(),
            self.inner.access.as_ref(),
        )
        .await
    }
}
