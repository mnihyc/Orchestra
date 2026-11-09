//! Actual node confirmations and Host-cascade environment children keep their own operation.

use super::*;
use crate::operation_scope::{
    http_origin,
    sources::{self, Facts},
    targets,
};
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::KeyTargetKind,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{host_delete_sources as host, loss::HostDeleteCreate, routes::Endpoint};
use orch_authz::{Policy, Principal};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "parent", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Parent {
    Node {
        job: Id,
    },
    Host {
        job: Id,
        host: Id,
        machine: Id,
        recipe: Hash,
        first_child: Id,
    },
}

/// Closed source data captured from real newly queued native jobs, never a global move alias.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParentSource {
    parent: Parent,
    http: HttpOrigin,
    identity: JobIdentity,
    environment: Option<super::super::origin::Environment>,
}

impl ParentSource {
    pub(super) fn origin(&self) -> &HttpOrigin {
        &self.http
    }
    pub(super) fn operation(&self) -> &'static str {
        match self.parent {
            Parent::Node { .. } => "NodeEnvironmentMoveCreate",
            Parent::Host { .. } => "HostDeleteCreate",
        }
    }
    pub(super) fn matches(&self, row: &JobRow) -> Result<bool, EngineError> {
        Ok(self.http.origin().operation() == self.operation()
            && self.identity.matches(row)?
            && self.http.origin().user() == row.created_by.ok_or_else(closed)?)
    }
    pub(super) async fn capture_node(
        conn: &mut Conn,
        row: &JobRow,
        clock: &dyn Clock,
    ) -> Result<Self, EngineError> {
        made_here(conn, row).await?;
        let row = jobs::job(conn, row.id).await?.ok_or_else(closed)?;
        if row.kind != jobs::kind::ENVIRONMENT_MOVE {
            return Err(closed());
        }
        crate::jobs::environment_move::authority::principal(conn, &row, clock).await?;
        let row = jobs::job(conn, row.id).await?.ok_or_else(closed)?;
        let http = crate::jobs::environment_move::authority::retained_http(&row)?;
        let value = Self {
            parent: Parent::Node { job: row.id },
            http,
            identity: JobIdentity::capture(&row)?,
            environment: None,
        };
        if !value.matches(&row)? {
            return Err(closed());
        }
        Ok(value)
    }
    pub(super) async fn capture_host(
        conn: &mut Conn,
        parent: &JobRow,
        child: &JobRow,
        clock: &dyn Clock,
    ) -> Result<Self, EngineError> {
        let (parent, http) = capture_host_parent(conn, parent, child, clock).await?;
        let identity = JobIdentity::capture(child)?;
        let source = super::super::origin::Environment::load(conn, identity.environment()).await?;
        let value = Self {
            parent,
            http,
            identity,
            environment: Some(source),
        };
        if !value.matches(child)? {
            return Err(closed());
        }
        Ok(value)
    }
    pub(super) async fn current_http(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<HttpOrigin, EngineError> {
        current_http(self.parent, &self.http, conn, clock).await
    }
    pub(super) async fn lock_source(
        &self,
        conn: &mut Conn,
        row: &JobRow,
    ) -> Result<(), EngineError> {
        if !self.matches(row)? {
            return Err(closed());
        }
        self.http.lock_identity(conn).await?;
        let parent = match self.parent {
            Parent::Node { job } | Parent::Host { job, .. } => job,
        };
        sqlx::query("SELECT id FROM jobs WHERE id=$1 FOR SHARE")
            .bind(parent)
            .execute(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
        if let Parent::Host { host, .. } = self.parent {
            orch_db::queries::resources::lock_resource(conn, "host", host).await?;
        }
        envs::lock_environment(conn, self.identity.environment())
            .await?
            .ok_or_else(closed)?;
        Ok(())
    }
    pub(super) async fn principal(
        &self,
        conn: &mut Conn,
        row: &JobRow,
        clock: &dyn Clock,
        selection: Selection,
    ) -> Result<Principal, EngineError> {
        if !self.matches(row)? || !self.identity.current(conn).await? {
            return Err(closed());
        }
        let selected_host = match selection {
            Selection::Current => None,
            Selection::Host(host) => Some(host),
            Selection::Wake(environment)
                if self.identity.wake_matches(conn, environment).await? =>
            {
                None
            }
            _ => return Err(closed()),
        };
        let needs = self.identity.destinations(conn, selected_host).await?;
        let http = self.current_http(conn, clock).await?;
        let raw = http.current(conn, clock).await.map_err(raw_error)?;
        let principal = match self.parent {
            Parent::Node { job } => {
                let owner = jobs::job(conn, job).await?.ok_or_else(closed)?;
                let principal =
                    crate::jobs::environment_move::authority::principal(conn, &owner, clock)
                        .await?
                        .within_source(raw.caller())
                        .map_err(|_| closed())?;
                let required = principal.caller().admitted_facets().unwrap_or(&[]).to_vec();
                crate::operation_scope::continuing::hold_principal(
                    conn,
                    raw.user(),
                    http.session(),
                    principal,
                    &required,
                )
                .await
                .map_err(raw_error)?
            }
            Parent::Host { .. } => {
                let original = self.environment.as_ref().ok_or_else(closed)?;
                let current =
                    super::super::origin::Environment::load(conn, self.identity.environment())
                        .await?;
                let candidate = match current.runtime() {
                    Some(runtime) => self.identity.owns_candidate(conn, runtime).await?,
                    None => false,
                };
                let recovery = self
                    .identity
                    .owns_recovery_runtime(conn, current.runtime())
                    .await?;
                if !original.same_execution(&current, candidate, recovery) {
                    return Err(closed());
                }
                let mut managed = vec![ObjectRef {
                    kind: ObjectKind::Environment,
                    id: self.identity.environment(),
                }];
                for (object, permission) in &needs {
                    if object.kind == ObjectKind::Environment
                        && *permission == Permission::EnvironmentManage
                    {
                        managed.push(*object);
                    }
                }
                host_principal(
                    conn,
                    self.parent,
                    &raw,
                    http.session(),
                    managed,
                    &needs,
                    clock,
                )
                .await?
            }
        };
        raw.check(clock).map_err(raw_error)?;
        if !principal.caller().key_deadline_allows(clock) {
            return Err(closed());
        }
        Ok(principal)
    }
    pub(super) async fn child(
        &self,
        conn: &mut Conn,
        parent: &JobRow,
        child: &JobRow,
    ) -> Result<Self, EngineError> {
        if !matches!(self.parent, Parent::Host { .. }) || !self.matches(parent)? {
            return Err(closed());
        }
        made_here(conn, child).await?;
        let identity = self.identity.child(conn, parent, child).await?;
        Ok(Self {
            identity,
            ..self.clone()
        })
    }
    pub(super) async fn cleanup_matches(
        &self,
        conn: &mut Conn,
        row: &JobRow,
        machine: Id,
        key: Option<&str>,
        kind: &str,
        body: &Json,
    ) -> Result<bool, EngineError> {
        if !self.matches(row)? {
            return Ok(false);
        }
        if matches!(self.parent, Parent::Node { .. }) {
            return crate::jobs::environment_move::authority::cleanup_matches(
                conn, row, machine, key, kind, body,
            )
            .await;
        }
        if row.plan.get("committed").and_then(Json::as_bool) != Some(true)
            || crate::jobs::runner::read_progress(row).0.phase != "cleanup"
        {
            return Ok(false);
        }
        let Some(old) = self.identity.old_runtime() else {
            return Ok(self.identity.portable()
                && self.identity.source_machine() == Some(machine)
                && kind == "env.detach"
                && key
                    == orch_link_contract::commands::SerialKey::Environment(
                        self.identity.environment(),
                    )
                    .wire()
                    .as_deref()
                && body.get("environment_id")
                    == Some(&serde_json::json!(self.identity.environment())));
        };
        if !self
            .environment
            .as_ref()
            .is_some_and(|source| source.runtime_matches(machine, old))
        {
            return Ok(false);
        }
        let Some(runtime) = envs::runtime(conn, old).await?.filter(|runtime| {
            runtime.environment_id == Some(self.identity.environment())
                && runtime.successor_of.is_none()
                && runtime.state == "retired"
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
}

async fn made_here(conn: &mut Conn, row: &JobRow) -> Result<(), EngineError> {
    if row.state != "queued" || !orch_db::queries::transaction_guard::in_transaction(conn) {
        return Err(closed());
    }
    let current:bool=sqlx::query_scalar("SELECT xmin=((pg_current_xact_id()::text::numeric % 4294967296)::text)::xid FROM jobs WHERE id=$1")
        .bind(row.id).fetch_one(conn).await.map_err(orch_db::DbError::from)?;
    if !current {
        return Err(closed());
    }
    Ok(())
}
pub(super) async fn collection(
    conn: &mut Conn,
    role: host::Source,
    objects: Vec<ObjectRef>,
) -> Result<Facts<host::Source>, EngineError> {
    let mut groups = BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    for object in objects {
        if !seen.insert((object.kind, object.id)) {
            continue;
        }
        let target = match object.kind {
            ObjectKind::Machine => targets::machine_subject(conn, object.id).await,
            _ => targets::resource_identity(conn, object).await,
        }
        .map_err(EngineError::unreadable)?;
        groups
            .entry(KeyTargetKind::Object(object.kind))
            .or_insert_with(Vec::new)
            .push(target);
    }
    Ok(Facts::collection(role, groups.into_iter().collect()))
}

pub(super) fn raw_error(error: crate::operation_scope::continuing::Error) -> EngineError {
    match error {
        crate::operation_scope::continuing::Error::Native(error) => error,
        error => EngineError::BeforeEffect(Box::new(error)),
    }
}

pub(super) async fn current_http(
    parent: Parent,
    original: &HttpOrigin,
    conn: &mut Conn,
    clock: &dyn Clock,
) -> Result<HttpOrigin, EngineError> {
    let parent_job = match parent {
        Parent::Node { job } | Parent::Host { job, .. } => job,
    };
    let row = jobs::job(conn, parent_job).await?.ok_or_else(closed)?;
    if !row.is_open()
        || row.cancel_requested()
        || crate::jobs::runner::abort_reason(&row)?.is_some()
    {
        return Err(closed());
    }
    let parent_http = match parent {
        Parent::Node { .. } => crate::jobs::environment_move::authority::retained_http(&row)?,
        Parent::Host {
            host,
            machine,
            recipe,
            first_child,
            ..
        } => {
            let native: crate::jobs::host_delete::Recipe =
                serde_json::from_value(row.plan["cascade"].clone())?;
            if row.kind != crate::jobs::host_delete::KIND
                || native.host != host
                || native.machine != machine
                || native.by != original.origin().user()
                || !native.children.contains(&first_child)
                || crate::jobs::command_identity::hash(&serde_json::to_value(&native)?)? != recipe
            {
                return Err(closed());
            }
            crate::jobs::host_delete::authority::retained_http(&row)?
        }
    };
    let mut http = original.intersect(&parent_http).map_err(raw_error)?;
    if let Some(active) = http_origin::active() {
        http = http.intersect(&active).map_err(raw_error)?;
    }
    let current = http.current(conn, clock).await.map_err(raw_error)?;
    http.accepted(current.caller(), current.until())
        .map_err(raw_error)
}

pub(super) async fn capture_host_parent(
    conn: &mut Conn,
    parent: &JobRow,
    child: &JobRow,
    clock: &dyn Clock,
) -> Result<(Parent, HttpOrigin), EngineError> {
    made_here(conn, parent).await?;
    made_here(conn, child).await?;
    let parent = jobs::job(conn, parent.id).await?.ok_or_else(closed)?;
    if parent.kind != crate::jobs::host_delete::KIND {
        return Err(closed());
    }
    crate::jobs::host_delete::authority::principal(conn, &parent, clock).await?;
    let parent = jobs::job(conn, parent.id).await?.ok_or_else(closed)?;
    let recipe: crate::jobs::host_delete::Recipe =
        serde_json::from_value(parent.plan["cascade"].clone())?;
    if !recipe.children.contains(&child.id) || child.created_by != Some(recipe.by) {
        return Err(closed());
    }
    let root = Parent::Host {
        job: parent.id,
        host: recipe.host,
        machine: recipe.machine,
        recipe: crate::jobs::command_identity::hash(&serde_json::to_value(&recipe)?)?,
        first_child: child.id,
    };
    Ok((
        root,
        crate::jobs::host_delete::authority::retained_http(&parent)?,
    ))
}

pub(super) async fn host_principal(
    conn: &mut Conn,
    parent: Parent,
    raw: &crate::operation_scope::http_origin::Current,
    session: Option<Id>,
    managed: Vec<ObjectRef>,
    needs: &[(ObjectRef, Permission)],
    clock: &dyn Clock,
) -> Result<Principal, EngineError> {
    let Parent::Host {
        host: host_id,
        machine,
        ..
    } = parent
    else {
        return Err(closed());
    };
    let held = hosts::host(conn, host_id)
        .await?
        .filter(|host| host.machine_id == machine)
        .ok_or_else(closed)?;
    let used = needs
        .iter()
        .filter_map(|(object, _)| {
            matches!(
                object.kind,
                ObjectKind::Host | ObjectKind::Template | ObjectKind::Machine | ObjectKind::Mount
            )
            .then_some(*object)
        })
        .collect();
    let facts = vec![
        Facts::one(
            host::Source::Host,
            KeyTargetKind::Object(ObjectKind::Host),
            targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Host,
                    id: held.id,
                },
            )
            .await
            .map_err(EngineError::unreadable)?,
        ),
        collection(conn, host::Source::Managed, managed.clone()).await?,
        collection(conn, host::Source::Used, used).await?,
    ];
    let plan = sources::bind_required(HostDeleteCreate::ROUTE, &host::CREATE, &facts)
        .map_err(EngineError::unreadable)?;
    let required = plan.required_facets().map_err(EngineError::unreadable)?;
    let caller = plan.admit(raw.caller(), false).map_err(|_| closed())?;
    let principal = Principal::load(conn, caller, clock.time()).await?;
    let principal = crate::operation_scope::continuing::hold_principal(
        conn,
        raw.user(),
        session,
        principal,
        &required,
    )
    .await
    .map_err(raw_error)?;
    if crate::operation_access::review(
        conn,
        &principal,
        ObjectRef {
            kind: ObjectKind::Host,
            id: host_id,
        },
        Permission::HostManage,
    )
    .await
    .map_err(|_| closed())?
    .is_some()
    {
        return Err(closed());
    }
    for object in managed {
        let target = match object.kind {
            ObjectKind::Environment => crate::updates::Target::Environment(object.id),
            ObjectKind::Provider => crate::updates::Target::Provider(object.id),
            _ => return Err(closed()),
        };
        if !crate::updates::may_manage(conn, &principal, target).await? {
            return Err(closed());
        }
    }
    for (object, permission) in needs {
        if matches!(
            object.kind,
            ObjectKind::Host | ObjectKind::Template | ObjectKind::Machine | ObjectKind::Mount
        ) && !orch_authz::resources::check(
            conn,
            &principal,
            *object,
            *permission,
            Policy::default(),
        )
        .await?
        .is_allowed()
        {
            return Err(closed());
        }
    }
    Ok(principal)
}
