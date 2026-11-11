//! Real reviewed node/Host sources for native observations before an execution job exists.
use crate::{
    Engine, EngineError,
    commands::{CommandAuthority, Outgoing},
    harnesses::preparation as shared,
    operation_scope::{
        continuing::{Error, Source as NativeSource},
        http_origin::HttpOrigin,
        sources::Facts,
        targets,
    },
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::KeyTargetKind,
    permissions::{NodePermission, ObjectKind, Permission, PermissionName},
};
use orch_api_contract::{
    host_delete_sources as host, move_batch_sources as node, routes::Endpoint,
};
use orch_authz::{Caller, CapturedOperation, Principal};
use orch_db::{
    Conn,
    queries::{
        environments::hosting as envs,
        jobs,
        resources::{self, hosting as hosts, provider_runtime as providers},
    },
};
use orch_link_contract::commands::{CommandKind, NodeRuntimeVolumesMeasure, NodeVolumeInspect};
use orch_types::{Clock, Hash, Id, Json, Time};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, PoisonError},
};

const MINIMUM: &str = "lifecycle_planning_minimum";
const REQUIRED: &str = "lifecycle_planning_required";
fn closed() -> EngineError {
    EngineError::BeforeEffect(Box::new(Error::Refused))
}
fn native(error: Error) -> EngineError {
    match error {
        Error::Native(error) => error,
        error => EngineError::BeforeEffect(Box::new(error)),
    }
}
fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    NodeMove,
    HostDelete,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    kind: Kind,
    review: Id,
    subject: Id,
    actor: Id,
    plan: Hash,
    expires: Time,
    managed: BTreeSet<(ObjectKind, Id)>,
    used: BTreeSet<(ObjectKind, Id)>,
    physical: Hash,
    machines: BTreeSet<Id>,
    units: BTreeSet<Id>,
}
#[derive(Deserialize)]
struct NodeReview {
    #[serde(default)]
    observations: crate::jobs::measurement::SavedMeasurements,
    recipes: Vec<crate::jobs::environment_move::MoveRecipe>,
    #[serde(default)]
    portable_recipes: Vec<crate::jobs::machine_move::Recipe>,
}
#[derive(Deserialize)]
struct HostReview {
    #[serde(default)]
    observations: crate::jobs::measurement::SavedMeasurements,
    host: Id,
    environments: Vec<crate::jobs::host_delete::reviewed::EnvironmentAction>,
    providers: Vec<crate::jobs::host_delete::reviewed::ProviderAction>,
    data: Option<HostData>,
}
#[derive(Deserialize)]
struct HostData {
    custody: crate::jobs::host_delete::Custody,
    purges: Vec<(Id, i64)>,
}

impl Selection {
    async fn capture(
        conn: &mut Conn,
        origin: &HttpOrigin,
        review: Id,
        subject: Id,
        kind: Kind,
        clock: &dyn Clock,
    ) -> Result<(Self, crate::jobs::measurement::SavedMeasurements), EngineError> {
        let operation = match kind {
            Kind::NodeMove => node::CREATE.operation,
            Kind::HostDelete => host::CREATE.operation,
        };
        if origin.origin().operation() != operation {
            return Err(closed());
        }
        let row = jobs::forks::preflight(conn, review)
            .await?
            .ok_or_else(closed)?;
        if row.created_by != origin.origin().user()
            || Time::new(row.expires_at) <= clock.time()
            || row.kind
                != match kind {
                    Kind::NodeMove => "move_batch",
                    Kind::HostDelete => "host_delete",
                }
            || kind == Kind::NodeMove && row.node_id != Some(subject)
        {
            return Err(closed());
        }
        let (managed, used, observations) = match kind {
            Kind::NodeMove => {
                let plan: NodeReview = serde_json::from_value(row.plan.clone())?;
                let mut managed = BTreeSet::new();
                let mut used = BTreeSet::new();
                for recipe in plan.recipes {
                    managed.insert((ObjectKind::Environment, recipe.replacement.target.id()));
                    used.insert((ObjectKind::Host, recipe.destination));
                    used.insert((ObjectKind::Template, recipe.replacement.template));
                }
                for recipe in plan.portable_recipes {
                    managed.insert((ObjectKind::Environment, recipe.environment));
                    used.extend(
                        recipe
                            .destination_access()
                            .into_iter()
                            .map(|(object, _)| (object.kind, object.id)),
                    );
                }
                (managed, used, plan.observations)
            }
            Kind::HostDelete => {
                let plan: HostReview = serde_json::from_value(row.plan.clone())?;
                if plan.host != subject {
                    return Err(closed());
                }
                let mut footprint = crate::jobs::host_delete::reviewed::Footprint::of(
                    &plan.environments,
                    &plan.providers,
                    plan.data
                        .as_ref()
                        .map(|data| (&data.custody, data.purges.as_slice())),
                );
                for machine in &footprint.copy_targets {
                    match hosts::host_of_machine(conn, *machine).await? {
                        Some(host) => {
                            footprint.used.insert((ObjectKind::Host, host));
                        }
                        None => {
                            footprint.used.insert((ObjectKind::Machine, *machine));
                        }
                    }
                }
                (footprint.managed, footprint.used, plan.observations)
            }
        };
        let anchor = (kind == Kind::HostDelete).then_some(subject);
        let (physical, machines, units) = physical(conn, &managed, &used, anchor).await?;
        Ok((
            Self {
                kind,
                review,
                subject,
                actor: origin.origin().user(),
                plan: super::command_identity::hash(&row.plan)?,
                expires: Time::new(row.expires_at),
                managed,
                used,
                physical,
                machines,
                units,
            },
            observations,
        ))
    }
    async fn current(
        &self,
        conn: &mut Conn,
        target: &Target,
        now: Time,
    ) -> Result<(), EngineError> {
        if now >= self.expires || !self.machines.contains(&target.machine) || !allowed(target.kind)
        {
            return Err(closed());
        }
        match jobs::forks::preflight(conn, self.review).await? {
            Some(row)
                if row.created_by == self.actor
                    && Time::new(row.expires_at) == self.expires
                    && row.kind
                        == match self.kind {
                            Kind::NodeMove => "move_batch",
                            Kind::HostDelete => "host_delete",
                        }
                    && (self.kind != Kind::NodeMove || row.node_id == Some(self.subject))
                    && super::command_identity::hash(&row.plan)? == self.plan => {}
            // An accepted observation owns its immutable request after a review is consumed.
            None if target.job.is_some() => {}
            _ => return Err(closed()),
        }
        if self.kind == Kind::NodeMove {
            let under = envs::environments_under(conn, self.subject).await?;
            if self
                .managed
                .iter()
                .any(|(kind, id)| *kind != ObjectKind::Environment || !under.contains(id))
            {
                return Err(closed());
            }
        }
        if physical(
            conn,
            &self.managed,
            &self.used,
            (self.kind == Kind::HostDelete).then_some(self.subject),
        )
        .await?
        .0 != self.physical
        {
            return Err(closed());
        }
        if let Some(id) = target.job {
            let row = jobs::job(conn, id).await?.ok_or_else(closed)?;
            if row.kind != crate::jobs::measurement::KIND
                || row.created_by != Some(self.actor)
                || row.plan["command_kind"] != json!(target.kind.as_str())
            {
                return Err(closed());
            }
            let machine = if target.phase == Some(shared::MeasurementPhase::FingerprintTarget) {
                let input: crate::jobs::measurement::FingerprintInput =
                    serde_json::from_value(row.plan["body"].clone())?;
                input.target_machine
            } else {
                serde_json::from_value(row.plan["machine"].clone())?
            };
            if machine != target.machine {
                return Err(closed());
            }
        }
        Ok(())
    }
    async fn ordinary(&self, conn: &mut Conn, principal: &Principal) -> Result<(), Error> {
        if self.kind == Kind::NodeMove
            && !orch_authz::nodes::check_named(
                conn,
                principal,
                self.subject,
                PermissionName::OfNode(NodePermission::View),
                orch_authz::Act::Use,
                orch_authz::Policy::default(),
            )
            .await?
            .is_allowed()
        {
            return Err(Error::Refused);
        }
        let mut permissions = Vec::new();
        if self.kind == Kind::HostDelete {
            permissions.push((ObjectKind::Host, self.subject, Permission::HostManage));
        }
        for (kind, id) in &self.managed {
            permissions.push((
                *kind,
                *id,
                match kind {
                    ObjectKind::Environment => Permission::EnvironmentManage,
                    ObjectKind::Provider => Permission::ProviderManage,
                    _ => return Err(Error::Refused),
                },
            ));
        }
        for (kind, id) in &self.used {
            permissions.push((
                *kind,
                *id,
                match kind {
                    ObjectKind::Host => Permission::HostUse,
                    ObjectKind::Template => Permission::TemplateUse,
                    ObjectKind::Machine => Permission::MachineUse,
                    ObjectKind::Mount => Permission::MountUse,
                    _ => return Err(Error::Refused),
                },
            ));
        }
        for (kind, id, permission) in permissions {
            crate::operation_access::review(conn, principal, ObjectRef { kind, id }, permission)
                .await
                .map_err(|e| Error::Native(EngineError::BeforeEffect(Box::new(e))))?;
        }
        Ok(())
    }
    async fn locks(&self, conn: &mut Conn) -> Result<(), Error> {
        // These are effect-free infrastructure observations. Their creator's native preview
        // may hold speculative placement/grant writes until rollback; taking lifecycle locks
        // here would wait on that same caller. The two current source reads still surround
        // this immutable review fence. The actual creator/execution keeps its full writer locks.
        self.lock_review(conn).await
    }
    async fn lock_review(&self, conn: &mut Conn) -> Result<(), Error> {
        if let Some(row) = jobs::changes::lock_plan(conn, self.review).await?
            && (row.created_by != self.actor
                || super::command_identity::hash(&row.plan)? != self.plan)
        {
            return Err(Error::Refused);
        }
        Ok(())
    }
}
async fn physical(
    conn: &mut Conn,
    managed: &BTreeSet<(ObjectKind, Id)>,
    used: &BTreeSet<(ObjectKind, Id)>,
    anchor: Option<Id>,
) -> Result<(Hash, BTreeSet<Id>, BTreeSet<Id>), EngineError> {
    let mut facts = Vec::new();
    let mut machines = BTreeSet::new();
    let mut units = BTreeSet::new();
    if let Some(id) = anchor {
        let host = hosts::host(conn, id).await?.ok_or_else(closed)?;
        machines.insert(host.machine_id);
        facts.push(json!(["host_controller", id, host.machine_id]));
    }
    for (kind, id) in managed {
        match kind {
            ObjectKind::Environment => {
                let state = envs::environment(conn, *id).await?.ok_or_else(closed)?;
                facts.push(json!([
                    kind,
                    id,
                    state.owner_id,
                    state.home_node_id,
                    state.generation,
                    state.machine_id,
                    state.workspace_root,
                    state.config
                ]));
                machines.extend(state.machine_id);
                if let Some(unit) = envs::live_runtime(conn, *id).await? {
                    let host = hosts::host(conn, unit.host_id).await?.ok_or_else(closed)?;
                    machines.insert(host.machine_id);
                    units.insert(unit.id);
                    facts.push(json!([unit.id, unit.host_id, unit.driver, unit.volumes]));
                }
            }
            ObjectKind::Provider => {
                let state = providers::state(conn, *id).await?.ok_or_else(closed)?;
                facts.push(json!([
                    kind,
                    id,
                    state.owner_id,
                    state.home_id,
                    state.machine_id
                ]));
                machines.extend(state.machine_id);
                if let Some(unit) = providers::live_runtime(conn, *id).await? {
                    let host = hosts::host(conn, unit.host_id).await?.ok_or_else(closed)?;
                    machines.insert(host.machine_id);
                    units.insert(unit.id);
                    facts.push(json!([unit.id, unit.host_id, unit.driver, unit.volumes]));
                }
            }
            _ => return Err(closed()),
        }
    }
    for (kind, id) in used {
        match kind {
            ObjectKind::Host => {
                let host = hosts::host(conn, *id).await?.ok_or_else(closed)?;
                machines.insert(host.machine_id);
                facts.push(json!([kind, id, host.machine_id]));
            }
            ObjectKind::Machine => {
                let machine = resources::machine(conn, *id).await?.ok_or_else(closed)?;
                machines.insert(machine.id);
                facts.push(json!([kind, id, machine.owner_id]));
            }
            ObjectKind::Template => {
                let template = hosts::template(conn, *id).await?.ok_or_else(closed)?;
                if !template.enabled || template.deleted_at.is_some() {
                    return Err(closed());
                }
                facts.push(json!([
                    kind,
                    id,
                    template.owner_id,
                    template.home_id,
                    template.driver,
                    template.spec,
                    template.revision
                ]));
            }
            ObjectKind::Mount => {
                let _ = targets::resource_identity(
                    conn,
                    ObjectRef {
                        kind: *kind,
                        id: *id,
                    },
                )
                .await
                .map_err(EngineError::unreadable)?;
            }
            _ => return Err(closed()),
        }
    }
    Ok((
        super::command_identity::hash(&json!(facts))?,
        machines,
        units,
    ))
}
async fn objects<R>(
    conn: &mut Conn,
    role: R,
    objects: impl IntoIterator<Item = (ObjectKind, Id)>,
) -> Result<Facts<R>, Error> {
    let mut groups = BTreeMap::new();
    for (kind, id) in objects {
        let target = match kind {
            ObjectKind::Machine => targets::machine_subject(conn, id).await,
            _ => targets::resource_identity(conn, ObjectRef { kind, id }).await,
        }
        .map_err(|error| Error::Native(EngineError::unreadable(error)))?;
        groups
            .entry(KeyTargetKind::Object(kind))
            .or_insert_with(Vec::new)
            .push(target);
    }
    Ok(Facts::collection(role, groups.into_iter().collect()))
}
fn allowed(kind: CommandKind) -> bool {
    matches!(
        kind,
        CommandKind::NodeResourcesList
            | CommandKind::NodeRuntimeVolumesMeasure
            | CommandKind::FsDataMeasure
            | CommandKind::FsManifest
            | CommandKind::NodeVolumeInspect
            | CommandKind::NodeMountFingerprint
    )
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    job: Option<Id>,
    machine: Id,
    kind: CommandKind,
    identity: Hash,
    phase: Option<shared::MeasurementPhase>,
}
impl Target {
    fn of(target: shared::Target) -> Result<Self, EngineError> {
        match target {
            shared::Target::Measurement {
                job,
                machine,
                kind,
                identity,
                phase,
            } => Ok(Self {
                job,
                machine,
                kind,
                identity,
                phase,
            }),
            _ => Err(closed()),
        }
    }
}
#[derive(Clone)]
struct Input {
    selection: Selection,
    target: Target,
}
struct NodeSource;
struct HostSource;
macro_rules! source {
    ($type:ty,$role:ty,$route:ty,$declaration:expr,$facts:expr) => {
        impl NativeSource for $type {
            type Role = $role;
            type Input = Input;
            type Value = Hash;
            const ROUTE: orch_api_contract::routes::Route = <$route>::ROUTE;
            const DECLARATION: &'static orch_api_contract::operation_sources::Declaration<
                Self::Role,
            > = &$declaration;
            fn prepare<'a>(
                conn: &'a mut Conn,
                _: &'a Caller,
                input: &'a Input,
                now: Time,
            ) -> BoxFuture<'a, Result<(Hash, Vec<Facts<Self::Role>>), Error>> {
                Box::pin(async move {
                    input.selection.current(conn, &input.target, now).await?;
                    let facts = ($facts)(conn, &input.selection).await?;
                    Ok((input.selection.physical, facts))
                })
            }
            fn require<'a>(
                conn: &'a mut Conn,
                principal: &'a Principal,
                input: &'a Input,
                _: &'a Hash,
            ) -> BoxFuture<'a, Result<(), Error>> {
                Box::pin(async move { input.selection.ordinary(conn, principal).await })
            }
            fn locks<'a>(
                conn: &'a mut Conn,
                _: &'a Principal,
                input: &'a Input,
                _: &'a Hash,
            ) -> BoxFuture<'a, Result<(), Error>> {
                Box::pin(async move { input.selection.locks(conn).await })
            }
            fn check_opened<'a>(
                _: &'a mut Conn,
                _: &'a Principal,
                _: &'a Input,
                opened: &'a Hash,
                fresh: &'a Hash,
            ) -> BoxFuture<'a, Result<bool, Error>> {
                Box::pin(async move { Ok(opened == fresh) })
            }
        }
    };
}
async fn node_facts(
    conn: &mut Conn,
    selection: &Selection,
) -> Result<Vec<Facts<node::Source>>, Error> {
    Ok(vec![
        objects(
            conn,
            node::Source::Environments,
            selection.managed.iter().copied(),
        )
        .await?,
        objects(
            conn,
            node::Source::Destinations,
            selection.used.iter().copied(),
        )
        .await?,
    ])
}
async fn host_facts(
    conn: &mut Conn,
    selection: &Selection,
) -> Result<Vec<Facts<host::Source>>, Error> {
    let host = targets::resource_identity(
        conn,
        ObjectRef {
            kind: ObjectKind::Host,
            id: selection.subject,
        },
    )
    .await
    .map_err(|error| Error::Native(EngineError::unreadable(error)))?;
    Ok(vec![
        Facts::one(
            host::Source::Host,
            KeyTargetKind::Object(ObjectKind::Host),
            host,
        ),
        objects(
            conn,
            host::Source::Managed,
            selection.managed.iter().copied(),
        )
        .await?,
        objects(conn, host::Source::Used, selection.used.iter().copied()).await?,
    ])
}
source!(
    NodeSource,
    node::Source,
    orch_api_contract::environment::NodeEnvironmentMoveCreate,
    node::CREATE,
    node_facts
);
source!(
    HostSource,
    host::Source,
    orch_api_contract::loss::HostDeleteCreate,
    host::CREATE,
    host_facts
);

/// The source is request-local; only its native private packet is serialized by the existing channel.
#[derive(Clone)]
pub struct Planning {
    selection: Selection,
    observations: Arc<crate::jobs::measurement::SavedMeasurements>,
    initial: HttpOrigin,
    retained: Arc<Mutex<HttpOrigin>>,
}
tokio::task_local! {static ACTIVE:Planning;}
pub(crate) fn active() -> Option<Planning> {
    ACTIVE.try_with(Clone::clone).ok()
}
impl Planning {
    pub async fn node(
        conn: &mut Conn,
        origin: HttpOrigin,
        review: Id,
        node: Id,
        clock: &dyn Clock,
    ) -> Result<Self, EngineError> {
        Self::new(conn, origin, review, node, Kind::NodeMove, clock).await
    }
    pub async fn host(
        conn: &mut Conn,
        origin: HttpOrigin,
        review: Id,
        host: Id,
        clock: &dyn Clock,
    ) -> Result<Self, EngineError> {
        Self::new(conn, origin, review, host, Kind::HostDelete, clock).await
    }
    async fn new(
        conn: &mut Conn,
        origin: HttpOrigin,
        review: Id,
        subject: Id,
        kind: Kind,
        clock: &dyn Clock,
    ) -> Result<Self, EngineError> {
        let (selection, observations) =
            Selection::capture(conn, &origin, review, subject, kind, clock).await?;
        Ok(Self {
            selection,
            observations: Arc::new(observations),
            initial: origin.clone(),
            retained: Arc::new(Mutex::new(origin)),
        })
    }
    pub async fn scope<T>(&self, future: impl std::future::Future<Output = T>) -> T {
        ACTIVE.scope(self.clone(), future).await
    }
    pub fn retained(&self) -> HttpOrigin {
        lock(&self.retained).clone()
    }
    fn retain(&self, origin: &HttpOrigin) -> Result<(), EngineError> {
        let mut current = lock(&self.retained);
        *current = current.intersect(origin).map_err(native)?;
        Ok(())
    }
    pub(crate) fn measurement(&self) -> Result<crate::jobs::measurement::Preparation, EngineError> {
        Ok(crate::jobs::measurement::Preparation::new(
            super::command_identity::hash(&json!([self.selection, self.initial]))?,
            shared::Preparation::new(self.clone()),
        ))
    }
    pub(crate) async fn reuse_measurement(
        &self,
        engine: &Engine,
        request: &crate::jobs::measurement::Request,
    ) -> Result<crate::jobs::measurement::Count, EngineError> {
        let reference = self.observations.reference(request)?.ok_or_else(closed)?;
        let source = self.measurement()?;
        let observed = engine
            .completed_measurement_observation(request, &reference, &source)
            .await?;
        let count = match observed {
            Some(observation) => observation.replay(request, &source)?,
            None => {
                // An unproven or pending reference stays data-only and non-actionable.
                // Refresh the complete NEW source after the actual reference-row wait.
                let mut tx = engine.db().begin().await?;
                let target = Target::of(shared::Target::Measurement {
                    job: None,
                    machine: request.machine,
                    kind: request.kind,
                    identity: crate::jobs::measurement::SavedMeasurements::identity(request)?,
                    phase: None,
                })?;
                let (check, origin) = self
                    .checkpoint_on(&mut tx, engine.inner.clock.as_ref(), target)
                    .await?;
                let tail = check.deadline();
                check.check(engine.inner.clock.as_ref())?;
                tx.commit().await.map_err(orch_db::DbError::from)?;
                tail.check(engine.inner.clock.as_ref())?;
                crate::jobs::measurement::Count::unavailable(reference, &origin)
            }
        };
        let (origin, until) = count.source().ok_or_else(closed)?;
        if origin.user() != self.initial.origin().user()
            || origin.operation() != self.initial.origin().operation()
        {
            return Err(closed());
        }
        let mut tx = engine.db().begin().await?;
        let caller = origin
            .restore_current(&mut tx, engine.inner.clock.as_ref())
            .await?
            .ok_or_else(closed)?;
        let current = self
            .retained()
            .with_ceiling(&caller, until)
            .map_err(native)?;
        self.retain(&current)?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        if engine.inner.clock.time() >= self.selection.expires
            || !caller.key_deadline_allows(engine.inner.clock.as_ref())
            || self
                .retained()
                .until()
                .is_some_and(|until| engine.inner.clock.time() >= until)
        {
            return Err(closed());
        }
        Ok(count)
    }
    async fn checkpoint_on(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        target: Target,
    ) -> Result<(shared::Checkpoint, HttpOrigin), EngineError> {
        let mut origin = self.retained();
        if origin.origin().user() != self.selection.actor
            || origin.origin().operation()
                != match self.selection.kind {
                    Kind::NodeMove => node::CREATE.operation,
                    Kind::HostDelete => host::CREATE.operation,
                }
        {
            return Err(closed());
        }
        if let Some(job) = target.job {
            let row = jobs::job(conn, job).await?.ok_or_else(closed)?;
            match (row.plan.get(MINIMUM), row.plan.get(REQUIRED)) {
                (Some(value), Some(version)) if version == &json!(1) => {
                    let minimum: Minimum =
                        serde_json::from_value(value.clone()).map_err(|_| closed())?;
                    if minimum.selection != self.selection {
                        return Err(closed());
                    }
                    origin = origin.intersect(&minimum.origin).map_err(native)?;
                }
                (None, None) => {
                    let made: bool = sqlx::query_scalar(
                        "SELECT xmin=pg_current_xact_id()::xid FROM jobs WHERE id=$1",
                    )
                    .bind(job)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
                    if !made {
                        return Err(closed());
                    }
                }
                _ => return Err(closed()),
            }
        }
        let input = Input {
            selection: self.selection.clone(),
            target,
        };
        macro_rules! prepare {
            ($type:ty) => {{
                let frame = origin
                    .prepare::<$type>(conn, clock, input)
                    .await
                    .map_err(native)?;
                let accepted = frame.http_origin().cloned().ok_or_else(closed)?;
                let until = accepted.until();
                self.retain(&accepted)?;
                let parts = frame.into_parts();
                let caller = parts.principal.caller().clone();
                let expires = self.selection.expires;
                (
                    shared::Checkpoint::from_check(
                        caller,
                        until,
                        Box::new(move |clock| {
                            if clock.time() >= expires {
                                return Err(closed());
                            }
                            parts.first_effect.check(clock).map_err(native)
                        }),
                    ),
                    accepted,
                )
            }};
        }
        Ok(match self.selection.kind {
            Kind::NodeMove => prepare!(NodeSource),
            Kind::HostDelete => prepare!(HostSource),
        })
    }
}
impl shared::Source for Planning {
    fn origin(&self) -> &CapturedOperation {
        self.initial.origin()
    }
    fn until(&self) -> Option<Time> {
        self.initial.until()
    }
    fn checkpoint<'a>(
        &'a self,
        conn: &'a mut Conn,
        clock: &'a dyn Clock,
        target: shared::Target,
    ) -> BoxFuture<'a, Result<shared::Checkpoint, EngineError>> {
        Box::pin(async move {
            Ok(self
                .checkpoint_on(conn, clock, Target::of(target)?)
                .await?
                .0)
        })
    }
    fn accepted(
        &self,
        origin: CapturedOperation,
        until: Option<Time>,
    ) -> Result<Arc<dyn shared::Source>, EngineError> {
        let mut accepted = self.clone();
        accepted.initial = self.retained();
        if accepted.initial.origin() != &origin || accepted.initial.until() != until {
            return Err(closed());
        }
        Ok(Arc::new(accepted))
    }
    fn command<'a>(
        &'a self,
        conn: &'a mut Conn,
        clock: &'a dyn Clock,
        target: shared::Target,
        command: &'a Outgoing,
    ) -> BoxFuture<'a, Result<CommandAuthority, EngineError>> {
        Box::pin(async move {
            let target = Target::of(target)?;
            let (check, origin) = self.checkpoint_on(conn, clock, target.clone()).await?;
            let packet = Packet::new(
                self.selection.clone(),
                origin.clone(),
                target.clone(),
                command,
            )?;
            check.check(clock)?;
            if let Some(job) = target.job {
                jobs::merge_plan(conn,job,&json!({MINIMUM:Minimum{selection:self.selection.clone(),origin:origin.clone()},REQUIRED:1}),clock.now()).await?;
            }
            let authority =
                CommandAuthority::Lifecycle(Box::new(super::Evidence::Reviewed(Box::new(packet))));
            if target.job.is_some() {
                Ok(authority)
            } else {
                origin.command(command, authority)
            }
        })
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Minimum {
    selection: Selection,
    origin: HttpOrigin,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
enum Version {
    #[serde(rename = "lifecycle_review_v1")]
    One,
}
/// Strict private native planning packet in the one lifecycle authority family.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Packet {
    version: Version,
    selection: Selection,
    origin: HttpOrigin,
    target: Target,
    canonical: Json,
}
impl Packet {
    fn new(
        selection: Selection,
        origin: HttpOrigin,
        target: Target,
        command: &Outgoing,
    ) -> Result<Self, EngineError> {
        if command.machine != target.machine || command.kind != target.kind {
            return Err(closed());
        }
        match command.kind {
            CommandKind::NodeRuntimeVolumesMeasure => {
                let body: NodeRuntimeVolumesMeasure = serde_json::from_value(command.body.clone())?;
                if !selection.units.contains(&body.runtime_id) {
                    return Err(closed());
                }
            }
            CommandKind::NodeVolumeInspect => {
                let body: NodeVolumeInspect = serde_json::from_value(command.body.clone())?;
                if !selection
                    .managed
                    .contains(&(ObjectKind::Environment, body.environment_id))
                {
                    return Err(closed());
                }
            }
            CommandKind::FsDataMeasure | CommandKind::FsManifest => {
                let id: Id = serde_json::from_value(command.body["environment_id"].clone())?;
                if !selection.managed.contains(&(ObjectKind::Environment, id)) {
                    return Err(closed());
                }
            }
            CommandKind::NodeResourcesList | CommandKind::NodeMountFingerprint => {}
            _ => return Err(closed()),
        }
        Ok(Self {
            version: Version::One,
            selection,
            origin,
            target,
            canonical: super::command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
        })
    }
    pub fn command_kind(&self) -> &'static str {
        self.target.kind.as_str()
    }
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        self.origin.lock_identity(conn).await?;
        Ok(())
    }
    async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        wire: super::Wire<'_>,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if self.canonical
            != super::command_identity::canonical(
                wire.id,
                wire.machine,
                wire.key,
                wire.kind,
                wire.body,
            )?
        {
            return Ok(None);
        }
        let planning = Planning {
            selection: self.selection.clone(),
            observations: Arc::new(crate::jobs::measurement::SavedMeasurements::default()),
            initial: self.origin.clone(),
            retained: Arc::new(Mutex::new(self.origin.clone())),
        };
        match planning
            .checkpoint_on(conn, clock, self.target.clone())
            .await
        {
            Ok((check, origin)) => {
                let caller = check.caller().clone();
                let until = if self.target.job.is_some() {
                    check
                        .until()
                        .into_iter()
                        .chain(Some(self.selection.expires))
                        .min()
                } else {
                    check.until()
                };
                check.check(clock)?;
                if let Some(job) = self.target.job {
                    jobs::merge_plan(conn,job,&json!({MINIMUM:Minimum{selection:self.selection.clone(),origin},REQUIRED:1}),clock.now()).await?;
                }
                Ok(Some(vec![(Some(caller), until)]))
            }
            Err(EngineError::BeforeEffect(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }
    pub async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::NewCommand<'_>,
        clock: &dyn Clock,
        _: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current(
            conn,
            clock,
            super::Wire {
                id: command.id,
                machine: command.machine_id,
                kind: command.kind,
                key: command.serial_key,
                body: command.body,
            },
        )
        .await
    }
    pub async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::CommandRow,
        clock: &dyn Clock,
        _: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current(
            conn,
            clock,
            super::Wire {
                id: command.id,
                machine: command.machine_id,
                kind: &command.kind,
                key: command.serial_key(),
                body: &command.body,
            },
        )
        .await
    }
    pub async fn reprepare_measurement(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
        origin: &CapturedOperation,
        until: Option<Time>,
        command: &Outgoing,
    ) -> Result<(CommandAuthority, CapturedOperation, Option<Time>), EngineError> {
        let serial = command.key.wire();
        let proposed = orch_db::queries::link::NewCommand {
            id: command.id,
            machine_id: command.machine,
            kind: command.kind.as_str(),
            body: &command.body,
            serial_key: serial.as_deref(),
            deadline_at: command.deadline.ok_or_else(closed)?.get(),
        };
        let callers = self
            .current_issue_callers(conn, &proposed, clock, access)
            .await?
            .ok_or_else(closed)?;
        let (origin, until) = crate::jobs::measurement::preparation::retain_current_callers(
            conn, clock, origin, until, &callers,
        )
        .await?;
        Ok((
            CommandAuthority::Lifecycle(Box::new(super::Evidence::Reviewed(Box::new(
                self.clone(),
            )))),
            origin,
            until,
        ))
    }
}

/// The actual native producer calls this before its existing once-only direct observation.
pub(crate) async fn observe(
    engine: &Engine,
    mut command: Outgoing,
) -> Result<Outgoing, EngineError> {
    let Some(source) = active() else {
        return Ok(command);
    };
    let allowance = command.deadline.unwrap_or_else(|| {
        Time::new(
            engine.inner.clock.now()
                + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())),
        )
    });
    command.deadline = Some(allowance.min(source.selection.expires));
    let identity = super::command_identity::hash(&json!([
        command.machine,
        command.kind.as_str(),
        command.body
    ]))?;
    shared::Preparation::new(source)
        .command(
            engine,
            shared::Target::Measurement {
                job: None,
                machine: command.machine,
                kind: command.kind,
                identity,
                phase: None,
            },
            command,
        )
        .await
}

/// Restore the native command's accepted transport minimum after its once-only IO completes.
pub(crate) async fn delivered(engine: &Engine, command: &Outgoing) -> Result<(), EngineError> {
    let (Some(source), Some(authority)) = (active(), command.authority.as_ref()) else {
        return Ok(());
    };
    let mut tx = engine.db().begin().await?;
    let origin = authority
        .http_delivery(&mut tx, engine.inner.clock.as_ref(), command.id)
        .await?;
    source.retain(&origin)?;
    let current = source
        .retained()
        .current(&mut tx, engine.inner.clock.as_ref())
        .await
        .map_err(native)?;
    current.check(engine.inner.clock.as_ref()).map_err(native)?;
    Ok(())
}
