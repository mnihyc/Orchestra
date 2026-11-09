//! The immutable native lifecycle request and the common raw HTTP/source checkpoint.
use super::*;
use crate::{
    harnesses::preparation as startup,
    operation_scope::{
        continuing::{self, Error, Source as NativeSource},
        http_origin::HttpOrigin,
        sources::Facts,
        targets,
    },
    updates,
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::KeyTargetKind,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    environment_sources::lifecycle::{self as rows, Source as Role},
    operation_sources::Declaration,
    routes::Route,
};
use orch_authz::{Policy, Principal};
use orch_db::queries::{environments::hosting as envs, jobs::forks::PreflightRow, resources};
use orch_types::{Hash, Id, Json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    MovePreflight,
    MoveCreate,
    RecoverPreflight,
    RecoverCreate,
}
impl Kind {
    pub fn row(self) -> &'static rows::Row {
        match self {
            Self::MovePreflight => &rows::MOVE_PREFLIGHT,
            Self::MoveCreate => &rows::MOVE_CREATE,
            Self::RecoverPreflight => &rows::RECOVER_PREFLIGHT,
            Self::RecoverCreate => &rows::RECOVER_CREATE,
        }
    }
}
fn mapped(error: Error) -> EngineError {
    match error {
        Error::Native(e) => e,
        other => EngineError::BeforeEffect(Box::new(other)),
    }
}

/// Only an actual native saved review supplies its identity and immutable content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Review {
    id: Id,
    kind: String,
    hash: Hash,
    expires: Time,
}

/// Physical source identity; incidental liveness/counters cannot substitute for its unit or files.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    pub id: Id,
    owner: Id,
    home: Id,
    generation: i32,
    revision: i64,
    machine: Option<Id>,
    root: String,
    config: Hash,
    runtime: Option<Id>,
    runtime_host: Option<Id>,
    runtime_machine: Option<Id>,
    volumes: Json,
    threads: Vec<Thread>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Thread {
    id: Id,
    association: Hash,
    placement: Hash,
}
impl Environment {
    pub async fn load(conn: &mut Conn, id: Id) -> Result<Self, EngineError> {
        let row = envs::environment(conn, id)
            .await?
            .filter(|r| r.deleted_at.is_none())
            .ok_or_else(closed)?;
        let runtime = envs::live_runtime(conn, id).await?;
        let host = match &runtime {
            Some(r) => resources::hosting::host(conn, r.host_id).await?,
            None => None,
        };
        let ids = orch_db::queries::jobs::changes::threads(conn, Some(id), None, false).await?;
        let mut threads = Vec::new();
        for t in orch_db::queries::threads::threads_among(conn, &ids).await? {
            threads.push(Thread {
                id: t.node_id,
                placement: crate::jobs::command_identity::hash(&serde_json::json!([
                    t.node_id,
                    t.topic_id,
                    t.workspace_dir,
                    t.environment_id
                ]))?,
                association: crate::jobs::command_identity::hash(&serde_json::json!([
                    t.node_id,
                    t.active_continuation_id,
                    t.current_segment_id,
                    t.execution_revision,
                    t.topic_id,
                    t.workspace_dir,
                    t.provider_id,
                    t.agent_id,
                    t.environment_id,
                ]))?,
            });
        }
        threads.sort_by_key(|t| t.id);
        Ok(Self {
            id,
            owner: row.owner_id,
            home: row.home_node_id,
            generation: row.generation,
            revision: row.revision,
            machine: row.machine_id,
            root: crate::sessions::workspace_root(&row.workspace_root, &row.facts, row.generation)
                .into(),
            config: crate::jobs::command_identity::hash(&row.config)?,
            runtime: runtime.as_ref().map(|r| r.id),
            runtime_host: runtime.as_ref().map(|r| r.host_id),
            runtime_machine: host.map(|h| h.machine_id),
            volumes: runtime.map_or_else(|| serde_json::json!({}), |r| r.volumes),
            threads,
        })
    }
    pub(super) fn same_execution(
        &self,
        current: &Self,
        owned_candidate: bool,
        owned_recovery: bool,
    ) -> bool {
        let mut previous = self.clone();
        let mut current = current.clone();
        if (previous.runtime.is_none() && owned_candidate) || owned_recovery {
            // This move's real unpublished unit is a destination, not a new source.
            current.runtime = previous.runtime;
            current.runtime_host = previous.runtime_host;
            current.runtime_machine = previous.runtime_machine;
            current.volumes = previous.volumes.clone();
        }
        // A running native move may drain a turn without rebinding its thread or workspace.
        for thread in previous
            .threads
            .iter_mut()
            .chain(current.threads.iter_mut())
        {
            thread.association = thread.placement;
        }
        previous == current
    }
    pub(super) fn machine_matches(&self, machine: Id) -> bool {
        self.machine == Some(machine) || self.runtime_machine == Some(machine)
    }
    pub(super) fn runtime_matches(&self, machine: Id, runtime: Id) -> bool {
        self.runtime == Some(runtime) && self.runtime_machine == Some(machine)
    }
    pub(super) fn volumes(&self) -> &Json {
        &self.volumes
    }

    pub(super) fn runtime(&self) -> Option<Id> {
        self.runtime
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Destination {
    pub object: ObjectRef,
    pub permission: Option<Permission>,
    fingerprint: Hash,
}
impl Destination {
    async fn load(
        conn: &mut Conn,
        object: ObjectRef,
        permission: Option<Permission>,
    ) -> Result<Self, EngineError> {
        // Only stable declared configuration is frozen; free-space/load/heartbeat reports are observations.
        let valid = matches!(
            (object.kind, permission),
            (ObjectKind::Host, Some(Permission::HostUse))
                | (ObjectKind::Template, Some(Permission::TemplateUse))
                | (ObjectKind::Machine, Some(Permission::MachineUse))
                | (ObjectKind::Environment, Some(Permission::EnvironmentUse))
                | (
                    ObjectKind::Org | ObjectKind::Project | ObjectKind::Topic | ObjectKind::Thread,
                    None
                )
        );
        if !valid {
            return Err(closed());
        }
        let value:Option<Json>=match object.kind {
            ObjectKind::Host=>sqlx::query_scalar("SELECT jsonb_build_array(machine_id,drivers,schedulable,disabled) FROM hosts WHERE id=$1").bind(object.id).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?,
            ObjectKind::Template=>sqlx::query_scalar("SELECT jsonb_build_array(driver,spec,revision) FROM env_templates WHERE id=$1").bind(object.id).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?,
            ObjectKind::Machine=>{
                let row=resources::machine(conn,object.id).await?.ok_or_else(closed)?;
                let policy=orch_db::queries::environment_facts::machine(conn,object.id,None).await?.and_then(|f|f.policy);
                let descriptor=orch_db::queries::environment_facts::on_machine(conn,object.id,None).await?;
                Some(serde_json::json!([row.owner_id,row.host_id,row.runtime_id,row.status,descriptor,policy]))
            },
            ObjectKind::Environment=>Some(serde_json::to_value(Environment::load(conn,object.id).await?)?),
            ObjectKind::Org|ObjectKind::Project|ObjectKind::Topic=>{
                let rows=orch_db::queries::hierarchy::with_ancestors(conn,&[object.id]).await?;
                rows.into_iter().find(|r|r.id==object.id&&r.deleted_at.is_none()).map(|r|serde_json::json!([r.id,r.org_id,r.parent_id,r.path,r.archived_at]))
            },
            _=>return Err(closed()),
        };
        Ok(Self {
            object,
            permission,
            fingerprint: crate::jobs::command_identity::hash(&value.ok_or_else(closed)?)?,
        })
    }
}

/// One actual original operation; no actor-only session or reusable admission is serialized.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    http: HttpOrigin,
    minimum: CapturedOperation,
    until: Option<Time>,
    pub kind: Kind,
    request: Hash,
    environments: Vec<Environment>,
    destinations: Vec<Destination>,
    destination_selected: bool,
    review: Option<Review>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    execution: Option<super::execution::JobIdentity>,
}
impl std::fmt::Debug for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LifecyclePreparation([private])")
    }
}
impl Origin {
    pub async fn capture(
        conn: &mut Conn,
        http: HttpOrigin,
        kind: Kind,
        request: Hash,
        environments: &[Id],
        review: Option<&PreflightRow>,
    ) -> Result<Self, EngineError> {
        if matches!(kind, Kind::MoveCreate | Kind::RecoverCreate) != review.is_some() {
            return Err(closed());
        }
        if kind == Kind::MoveCreate
            && review.is_some_and(|r| {
                environments.len() != 1 || r.environment_id != environments.first().copied()
            })
        {
            return Err(closed());
        }
        if http.origin().operation() != kind.row().declaration.operation {
            return Err(closed());
        }
        let mut selected = Vec::new();
        for id in environments.iter().copied().collect::<BTreeSet<_>>() {
            selected.push(Environment::load(conn, id).await?);
        }
        let review = match review {
            Some(row) => {
                let expected = match kind {
                    Kind::MoveCreate => "environment_move",
                    Kind::RecoverCreate => "recover_batch",
                    _ => return Err(closed()),
                };
                if row.kind != expected || row.created_by != http.origin().user() {
                    return Err(closed());
                }
                Some(Review {
                    id: row.id,
                    kind: row.kind.clone(),
                    hash: crate::jobs::command_identity::hash(&row.plan)?,
                    expires: Time::new(row.expires_at),
                })
            }
            None => None,
        };
        Ok(Self {
            minimum: http.origin().clone(),
            until: http.until(),
            http,
            kind,
            request,
            environments: selected,
            destinations: Vec::new(),
            destination_selected: false,
            review,
            execution: None,
        })
    }
    pub(super) fn kind(&self) -> Kind {
        self.kind
    }
    pub(super) fn http(&self) -> &HttpOrigin {
        &self.http
    }
    pub(super) fn execution(&self) -> Option<&super::execution::JobIdentity> {
        self.execution.as_ref()
    }
    pub(super) async fn for_job(
        &self,
        conn: &mut Conn,
        row: &orch_db::queries::jobs::JobRow,
        clock: &dyn Clock,
        prepared: Option<&crate::recovery::PreparedQueue>,
    ) -> Result<Self, EngineError> {
        if !matches!(self.kind, Kind::MoveCreate | Kind::RecoverCreate)
            || self.review.is_none()
            || self.execution.is_some()
        {
            return Err(closed());
        }
        let identity = super::execution::JobIdentity::capture(row)?;
        if identity.actor() != self.minimum.user()
            || !self.environments().any(|id| id == identity.environment())
        {
            return Err(closed());
        }
        if self.kind == Kind::MoveCreate && !identity.is_move() {
            return Err(closed());
        }
        let needs = identity.destinations(conn, None).await?;
        for &(object, permission) in &needs {
            if object.kind == ObjectKind::Environment && permission == Permission::EnvironmentManage
            {
                if !self.environments().any(|id| id == object.id) {
                    return Err(closed());
                }
                continue;
            }
            let ordinary = if matches!(
                object.kind,
                ObjectKind::Org | ObjectKind::Project | ObjectKind::Topic | ObjectKind::Thread
            ) {
                None
            } else {
                Some(permission)
            };
            if !self.destinations.iter().any(|destination| {
                destination.object == object && destination.permission == ordinary
            }) {
                return Err(closed());
            }
        }
        let raw = self.http.current(conn, clock).await.map_err(mapped)?;
        let previous = self
            .minimum
            .restore_current(conn, clock)
            .await?
            .ok_or_else(closed)?;
        let caller = raw
            .caller()
            .intersect_restrictions(&previous)
            .map_err(EngineError::unreadable)?;
        let mut accepted = Self {
            http: self.http.accepted(&caller, raw.until()).map_err(mapped)?,
            minimum: CapturedOperation::capture(&caller).map_err(EngineError::unreadable)?,
            until: self.until().into_iter().chain(raw.until()).min(),
            environments: self
                .environments
                .iter()
                .filter(|env| {
                    env.id == identity.environment()
                        || needs.iter().any(|(object, permission)| {
                            object.kind == ObjectKind::Environment
                                && *permission == Permission::EnvironmentManage
                                && object.id == env.id
                        })
                })
                .cloned()
                .collect(),
            execution: Some(identity.clone()),
            ..self.clone()
        };
        if let Some(prepared) = prepared {
            if !identity.is_recovery()
                || prepared.recipe().by != row.created_by.ok_or_else(closed)?
                || row.plan.get("recovery") != Some(&serde_json::to_value(prepared.recipe())?)
            {
                return Err(closed());
            }
            let (id, before, after) = prepared.config_change()?;
            let source = accepted
                .environments
                .iter_mut()
                .find(|env| env.id == id)
                .ok_or_else(closed)?;
            if source.config != before || Environment::load(conn, id).await?.config != after {
                return Err(closed());
            }
            source.config = after;
        } else if identity.is_recovery() {
            return Err(closed());
        }
        if let Some(thread) = identity.thread() {
            for environment in &mut accepted.environments {
                let original_source = environment.id == identity.environment();
                environment
                    .threads
                    .retain(|member| original_source && member.id == thread);
            }
        }
        // The original saved review still exists here. Native queue state changes may drain
        // a turn, but cannot lend a new environment, workspace, runtime or destination.
        if !accepted.same_source(conn, clock).await? {
            return Err(closed());
        }
        accepted.review = None;
        Ok(accepted)
    }

    pub(super) async fn for_execution_frame(
        &self,
        conn: &mut Conn,
        selected_host: Option<Id>,
    ) -> Result<Self, EngineError> {
        let identity = self.execution.as_ref().ok_or_else(closed)?;
        let needs = identity.destinations(conn, selected_host).await?;
        let mut frame = self.clone();
        frame.destinations.clear();
        frame.destination_selected = false;
        for (object, permission) in &needs {
            if object.kind == ObjectKind::Environment
                && *permission == Permission::EnvironmentManage
            {
                if !frame.environments().any(|id| id == object.id) {
                    return Err(closed());
                }
            } else {
                frame
                    .select_destinations(conn, &[(*object, *permission)])
                    .await?;
            }
        }
        frame.destination_selected = true;
        Ok(frame)
    }
    pub(super) async fn for_child(
        &self,
        conn: &mut Conn,
        parent: &orch_db::queries::jobs::JobRow,
        child: &orch_db::queries::jobs::JobRow,
        clock: &dyn Clock,
    ) -> Result<Self, EngineError> {
        if self.kind != Kind::RecoverCreate || self.review.is_some() {
            return Err(closed());
        }
        let identity = self
            .execution
            .as_ref()
            .ok_or_else(closed)?
            .child(conn, parent, child)
            .await?;
        let source = Self {
            execution: Some(identity),
            ..self.clone()
        }
        .for_execution_frame(conn, None)
        .await?;
        let check = source.checkpoint(conn, clock).await?;
        let source = source.accepted(
            CapturedOperation::capture(check.caller()).map_err(EngineError::unreadable)?,
            check.until(),
        )?;
        check.check(clock)?;
        Ok(source)
    }

    pub fn captured(&self) -> &CapturedOperation {
        &self.minimum
    }
    pub fn until(&self) -> Option<Time> {
        self.until.into_iter().chain(self.http.until()).min()
    }
    pub fn environments(&self) -> impl Iterator<Item = Id> + '_ {
        self.environments.iter().map(|s| s.id)
    }
    pub fn destinations(&self) -> impl Iterator<Item = (ObjectRef, Option<Permission>)> + '_ {
        self.destinations.iter().map(|d| (d.object, d.permission))
    }
    pub(super) fn source(&self, id: Id) -> Option<&Environment> {
        self.environments.iter().find(|s| s.id == id)
    }
    pub async fn select_destinations(
        &mut self,
        conn: &mut Conn,
        needs: &[(ObjectRef, Permission)],
    ) -> Result<(), EngineError> {
        // Only the actual native selector calls this, including its proven empty selection.
        self.destination_selected = true;
        for &(object, permission) in needs {
            if object.kind == ObjectKind::Environment && permission == Permission::EnvironmentManage
            {
                let selected = Environment::load(conn, object.id).await?;
                if let Some(old) = self.environments.iter().find(|s| s.id == object.id) {
                    if old != &selected {
                        return Err(closed());
                    }
                } else {
                    self.environments.push(selected);
                    self.environments.sort_by_key(|s| s.id);
                }
                continue;
            }
            let allowed = matches!(
                (object.kind, permission),
                (ObjectKind::Host, Permission::HostUse)
                    | (ObjectKind::Template, Permission::TemplateUse)
                    | (ObjectKind::Machine, Permission::MachineUse)
                    | (ObjectKind::Environment, Permission::EnvironmentUse)
                    | (ObjectKind::Org, Permission::OrgView)
                    | (ObjectKind::Project, Permission::ProjectView)
                    | (ObjectKind::Topic, Permission::TopicView)
                    | (ObjectKind::Thread, Permission::ThreadView)
            );
            if !allowed {
                return Err(closed());
            }
            let ordinary = if matches!(
                object.kind,
                ObjectKind::Org | ObjectKind::Project | ObjectKind::Topic | ObjectKind::Thread
            ) {
                None
            } else {
                Some(permission)
            };
            let selected = Destination::load(conn, object, ordinary).await?;
            if let Some(old) = self.destinations.iter().find(|d| d.object == object) {
                if old != &selected {
                    return Err(closed());
                }
            } else {
                self.destinations.push(selected);
            }
        }
        self.destinations
            .sort_by_key(|d| (d.object.kind, d.object.id));
        Ok(())
    }
    pub fn accepted(
        &self,
        origin: CapturedOperation,
        until: Option<Time>,
    ) -> Result<Self, EngineError> {
        if origin.operation() != self.minimum.operation() || origin.user() != self.minimum.user() {
            return Err(closed());
        }
        Ok(Self {
            minimum: self
                .minimum
                .intersect(&origin)
                .map_err(EngineError::unreadable)?,
            until: self.until().into_iter().chain(until).min(),
            ..self.clone()
        })
    }
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        self.http.lock_identity(conn).await?;
        for source in &self.environments {
            let _ = envs::lock_environment(conn, source.id).await?;
            let threads = source.threads.iter().map(|t| t.id).collect::<Vec<_>>();
            if !threads.is_empty() {
                sqlx::query("SELECT t.node_id FROM threads t JOIN nodes n ON n.id=t.node_id WHERE t.node_id=ANY($1) ORDER BY t.node_id FOR SHARE OF t,n").bind(threads).execute(&mut *conn).await.map_err(orch_db::DbError::from)?;
            }

            if let Some(id) = source.runtime {
                sqlx::query("SELECT id FROM runtimes WHERE id=$1 FOR SHARE")
                    .bind(id)
                    .execute(&mut *conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
            }
            sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR SHARE")
                .bind(source.home)
                .execute(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
        }
        for (table, query) in [
            (
                "nodes",
                "SELECT id FROM nodes WHERE id=ANY($1) ORDER BY id FOR SHARE",
            ),
            (
                "hosts",
                "SELECT id FROM hosts WHERE id=ANY($1) ORDER BY id FOR SHARE",
            ),
            (
                "machines",
                "SELECT id FROM machines WHERE id=ANY($1) ORDER BY id FOR SHARE",
            ),
            (
                "env_templates",
                "SELECT id FROM env_templates WHERE id=ANY($1) ORDER BY id FOR SHARE",
            ),
        ] {
            let ids = self
                .destinations
                .iter()
                .filter(|d| match table {
                    "nodes" => matches!(
                        d.object.kind,
                        ObjectKind::Org
                            | ObjectKind::Project
                            | ObjectKind::Topic
                            | ObjectKind::Thread
                    ),
                    "hosts" => d.object.kind == ObjectKind::Host,
                    "machines" => d.object.kind == ObjectKind::Machine,
                    "env_templates" => d.object.kind == ObjectKind::Template,
                    _ => false,
                })
                .map(|d| d.object.id)
                .collect::<Vec<_>>();
            if !ids.is_empty() {
                sqlx::query(sqlx::AssertSqlSafe(query))
                    .bind(ids)
                    .execute(&mut *conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
            }
        }
        if let Some(review) = &self.review {
            let _ = orch_db::queries::jobs::changes::lock_plan(conn, review.id).await?;
        }
        Ok(())
    }
    pub async fn same_source(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<bool, EngineError> {
        if let Some(execution) = &self.execution
            && !execution.current(conn).await?
        {
            return Ok(false);
        }
        for old in &self.environments {
            let mut current = Environment::load(conn, old.id).await?;
            if let Some(thread) = self
                .execution
                .as_ref()
                .and_then(|identity| identity.thread())
            {
                let source = self.execution.as_ref().ok_or_else(closed)?.environment();
                let original_source = current.id == source;
                current
                    .threads
                    .retain(|member| original_source && member.id == thread);
            }
            let owned_candidate = match (&self.execution, current.runtime) {
                (Some(execution), Some(runtime)) if old.runtime.is_none() => {
                    execution.owns_candidate(conn, runtime).await?
                }
                _ => false,
            };
            if if self.execution.is_some() {
                let owned_recovery = match &self.execution {
                    Some(identity) => {
                        identity
                            .owns_recovery_runtime(conn, current.runtime)
                            .await?
                    }
                    None => false,
                };
                !old.same_execution(&current, owned_candidate, owned_recovery)
            } else {
                current != *old
            } {
                return Ok(false);
            }
        }
        for old in &self.destinations {
            if self.execution.is_none()
                && Destination::load(conn, old.object, old.permission).await? != *old
            {
                return Ok(false);
            }
        }
        if let Some(review) = &self.review {
            let Some(row) = orch_db::queries::jobs::forks::preflight(conn, review.id).await? else {
                return Ok(false);
            };
            if row.kind != review.kind
                || row.created_by != self.minimum.user()
                || Time::new(row.expires_at) != review.expires
                || clock.time() >= review.expires
                || crate::jobs::command_identity::hash(&row.plan)? != review.hash
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub async fn require(&self, conn: &mut Conn, principal: &Principal) -> Result<(), EngineError> {
        for source in &self.environments {
            if !updates::may_manage(conn, principal, updates::Target::Environment(source.id))
                .await?
            {
                return Err(closed());
            }
        }
        for destination in &self.destinations {
            let Some(permission) = destination.permission else {
                continue;
            };
            if !orch_authz::resources::check(
                conn,
                principal,
                destination.object,
                permission,
                Policy::default(),
            )
            .await?
            .is_allowed()
            {
                return Err(closed());
            }
        }
        Ok(())
    }
    pub async fn facts(&self, conn: &mut Conn, complete: bool) -> Result<Vec<Facts<Role>>, Error> {
        let mut facts = vec![
            collection(
                conn,
                Role::Environments,
                self.environments.iter().map(|e| ObjectRef {
                    kind: ObjectKind::Environment,
                    id: e.id,
                }),
            )
            .await?,
        ];
        if complete || self.destination_selected {
            facts.push(
                collection(
                    conn,
                    Role::Destinations,
                    self.destinations.iter().map(|d| d.object),
                )
                .await?,
            );
        }
        Ok(facts)
    }
    pub async fn checkpoint(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<startup::Checkpoint, EngineError> {
        let current = self.http.current(conn, clock).await.map_err(mapped)?;
        let restored = self
            .minimum
            .restore_current(conn, clock)
            .await?
            .ok_or_else(closed)?;
        let caller = current
            .caller()
            .intersect_restrictions(&restored)
            .map_err(EngineError::unreadable)?;
        macro_rules! checked {
            ($n:expr) => {{
                let frame = self
                    .http
                    .scope(continuing::preparation::prepare_with_caller::<Native<$n>>(
                        conn,
                        clock,
                        &self.minimum,
                        &caller,
                        self.clone(),
                    ))
                    .await
                    .map_err(mapped)?;
                startup::Checkpoint::from_preparation(
                    frame,
                    self.until().into_iter().chain(current.until()).min(),
                    Box::new(|| Ok(())),
                )
            }};
        }
        Ok(match self.kind {
            Kind::MovePreflight => checked!(0),
            Kind::MoveCreate => checked!(1),
            Kind::RecoverPreflight => checked!(2),
            Kind::RecoverCreate => checked!(3),
        })
    }
}
async fn collection(
    conn: &mut Conn,
    role: Role,
    objects: impl Iterator<Item = ObjectRef>,
) -> Result<Facts<Role>, Error> {
    let mut groups = BTreeMap::new();
    for (kind, id) in objects.map(|o| (o.kind, o.id)).collect::<BTreeSet<_>>() {
        let object = ObjectRef { kind, id };
        let target = targets::page_object(conn, object)
            .await
            .map_err(|e| match e {
                targets::TargetError::Missing => Error::Refused,
                targets::TargetError::Db(e) => e.into(),
                targets::TargetError::Authz(e) => e.into(),
            })?;
        groups
            .entry(KeyTargetKind::Object(
                if role == Role::Destinations
                    && matches!(
                        object.kind,
                        ObjectKind::Org
                            | ObjectKind::Project
                            | ObjectKind::Topic
                            | ObjectKind::Thread
                    )
                {
                    ObjectKind::Environment
                } else {
                    object.kind
                },
            ))
            .or_insert_with(Vec::new)
            .push(target);
    }
    Ok(Facts::collection(role, groups.into_iter().collect()))
}
struct Native<const N: u8>;
impl<const N: u8> NativeSource for Native<N> {
    type Role = Role;
    type Input = Origin;
    type Value = Origin;
    const ROUTE: Route = match N {
        0 => rows::MOVE_PREFLIGHT.route,
        1 => rows::MOVE_CREATE.route,
        2 => rows::RECOVER_PREFLIGHT.route,
        _ => rows::RECOVER_CREATE.route,
    };
    const DECLARATION: &'static Declaration<Role> = match N {
        0 => &rows::MOVE_PREFLIGHT.source_phase,
        1 => &rows::MOVE_CREATE.source_phase,
        2 => &rows::RECOVER_PREFLIGHT.source_phase,
        _ => &rows::RECOVER_CREATE.source_phase,
    };
    fn declaration(input: &Origin, _: &Origin) -> &'static Declaration<Role> {
        let row = input.kind.row();
        if input.destination_selected {
            &row.destination_phase
        } else {
            &row.source_phase
        }
    }
    fn prepare<'a>(
        conn: &'a mut Conn,
        _: &'a Caller,
        input: &'a Origin,
        now: Time,
    ) -> BoxFuture<'a, Result<(Origin, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            #[derive(Debug)]
            struct Fixed(Time);
            impl Clock for Fixed {
                fn now(&self) -> chrono::DateTime<chrono::Utc> {
                    self.0.get()
                }
            }
            if !input.same_source(conn, &Fixed(now)).await? {
                return Err(Error::Refused);
            }
            Ok((input.clone(), input.facts(conn, false).await?))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        p: &'a Principal,
        input: &'a Origin,
        _: &'a Origin,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move { input.require(conn, p).await.map_err(Error::Native) })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Origin,
        _: &'a Origin,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move { input.lock_source(conn).await.map_err(Error::Native) })
    }
    fn check_opened<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Origin,
        a: &'a Origin,
        b: &'a Origin,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(a == b) })
    }
}
