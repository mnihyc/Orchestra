//! The original creator or fork remains the authority for its one pending environment child.
use super::*;
use crate::access::ResourceAccess;
use crate::jobs::fork::operation::Preparation as ForkPreparation;
use crate::operation_scope::{Prepared, compatible_selection, creator_files, targets};
use orch_access_contract::key_scope::{KeyAction, KeyTargetKind};
use orch_access_contract::permissions::{NodePermission, ObjectKind, PermissionName};
use orch_api_contract::operations::TargetResolver;
use orch_authz::{Act, CapturedOperation, Policy};
use orch_db::Conn;
use orch_types::{Clock, Hash};
use serde::{Deserialize, Serialize};

const FIELD: &str = "creator_provision";
const COMMANDS: &str = "creator_provision_commands";
const VERSION: &str = "creator_provision_version";
pub fn is_retained(row: &JobRow) -> bool {
    row.plan.get(FIELD).is_some()
        || row.plan.get(VERSION).is_some()
        || row.plan.get(COMMANDS).is_some()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Authority {
    origin: CapturedOperation,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_standalone"
    )]
    standalone: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_thread"
    )]
    thread: Option<Id>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_fork"
    )]
    fork: Option<Box<ForkPreparation>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_configuration"
    )]
    configuration: Option<Hash>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_wake"
    )]
    wake: Option<Wake>,
    environment: Id,
    home: Id,
    job: Id,
    template: Id,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wake {
    owner: Id,
    generation: i32,
    mode: String,
    unit: Option<Id>,
    update: Option<Hash>,
}
fn present_standalone<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
    bool::deserialize(d).map(Some)
}
fn present_wake<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Wake>, D::Error> {
    Wake::deserialize(d).map(Some)
}

fn present_thread<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Id>, D::Error> {
    Id::deserialize(d).map(Some)
}
fn present_fork<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Box<ForkPreparation>>, D::Error> {
    Box::<ForkPreparation>::deserialize(d).map(Some)
}
fn present_configuration<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Hash>, D::Error> {
    Hash::deserialize(d).map(Some)
}
fn closed() -> EngineError {
    EngineError::BeforeEffect(Box::new(crate::operation_scope::continuing::Error::Refused))
}

impl Authority {
    fn same_parent(&self, other: &Self) -> bool {
        self.environment == other.environment
            && self.home == other.home
            && self.job == other.job
            && self.template == other.template
            && self.configuration == other.configuration
            && self.wake == other.wake
            && self.thread == other.thread
            && self.standalone == other.standalone
            && (self.standalone != Some(true) || self.origin == other.origin)
            && self.origin.user() == other.origin.user()
            && self.origin.operation() == other.origin.operation()
            && match (&self.fork, &other.fork) {
                (None, None) => true,
                (Some(a), Some(b)) => a.preserves(b),
                _ => false,
            }
    }
}

fn read(row: &JobRow) -> Result<Option<Authority>, EngineError> {
    let version = row.plan.get(VERSION);
    if version.is_some_and(|value| {
        value != &json!(0) && value != &json!(1) && value != &json!(2) && value != &json!(3)
    }) {
        return Err(closed());
    }
    let authority: Option<Authority> = row
        .plan
        .get(FIELD)
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| closed())?;
    if authority.is_none() && is_retained(row) {
        return Err(closed());
    }
    if let Some(authority) = &authority {
        let valid = match (&authority.thread, &authority.fork, authority.standalone) {
            (Some(_), None, None) => {
                matches!(
                    authority.origin.operation(),
                    "ThreadCreate" | "TopicCreate.with_thread"
                ) && authority.configuration.is_none()
                    && authority.wake.is_none()
                    && version.is_none_or(|value| value == &json!(0))
            }
            (None, Some(parent), None) => {
                matches!(
                    authority.origin.operation(),
                    "ThreadForkCreate.fork"
                        | "ThreadForkCreate.switch"
                        | "ThreadForkNow"
                        | "ForkBatchCreate.switch"
                ) && parent.origin().captured() == &authority.origin
                    && authority.configuration.is_some()
                    && match &authority.wake {
                        None => version == Some(&json!(1)),
                        Some(wake) => {
                            version == Some(&json!(2))
                                && matches!(wake.mode.as_str(), "make" | "start" | "update")
                                && (wake.mode == "update") == wake.update.is_some()
                                && (wake.mode == "make" || wake.unit.is_some())
                        }
                    }
            }
            (None, None, Some(true)) => {
                authority.origin.operation()
                    == orch_api_contract::environment_sources::CREATE.operation
                    && authority.configuration.is_none()
                    && authority.wake.is_none()
                    && version.is_none_or(|value| value == &json!(3))
            }
            _ => false,
        };
        if !valid {
            return Err(closed());
        }
    }
    Ok(authority)
}

/// Called only by the creator's private new-environment prefix after full scope admission.
/// The ordinary hosted writer supplies both identities; a pre-existing environment/job is excluded.
pub async fn retain(
    conn: &mut Conn,
    origin: &CapturedOperation,
    thread: Id,
    environment: Id,
    home: Id,
    job: Id,
    new_created: bool,
    now: Time,
) -> Result<(), EngineError> {
    if !new_created
        || !matches!(
            origin.operation(),
            "ThreadCreate" | "TopicCreate.with_thread"
        )
    {
        return Err(EngineError::unreadable(
            "Only this creator's new provision can retain its origin.",
        ));
    }
    let row = jobs::lock(conn, job)
        .await?
        .ok_or_else(|| EngineError::unreadable("The new provision disappeared."))?;
    let env = env_rows::environment(conn, environment)
        .await?
        .ok_or_else(|| EngineError::unreadable("The new environment disappeared."))?;
    let actual = orch_db::queries::threads::thread(conn, thread)
        .await?
        .ok_or_else(|| EngineError::unreadable("The new thread disappeared."))?;
    if row.kind != kind::ENVIRONMENT_PROVISION
        || row.environment_id != Some(environment)
        || row.state != "queued"
        || row.cancel_requested()
        || row.created_by != Some(origin.user())
        || row.plan.get("mode").and_then(Json::as_str) != Some("make")
        || row.plan.get("generation").and_then(Json::as_i64) != Some(0)
        || row
            .plan
            .get("runtime")
            .is_some_and(|value| !value.is_null())
        || env.generation != 0
        || env.placement != "managed"
        || env.deleted_at.is_some()
        || env.home_node_id != home
        || env.owner_id != origin.user()
        || actual.environment_id != Some(environment)
    {
        return Err(EngineError::unreadable(
            "The creator no longer owns this exact new-environment prefix.",
        ));
    }
    let authority = Authority {
        origin: origin.clone(),
        standalone: None,
        thread: Some(thread),
        fork: None,
        configuration: None,
        wake: None,
        environment,
        home,
        job,
        template: env
            .template_id
            .ok_or_else(|| EngineError::unreadable("The new template is missing."))?,
    };
    if let Some(previous) = read(&row)? {
        if previous != authority {
            return Err(EngineError::unreadable(
                "The new provision already belongs to another creator.",
            ));
        }
        return Ok(());
    }
    jobs::merge_plan(conn, job, &json!({FIELD:authority, VERSION:0}), now.get()).await?;
    Ok(())
}

/// Retain the real maker's source on its newly inserted child and native maker checkpoint.
/// Both rows must be in this transaction; an existing child cannot be adopted.
pub async fn retain_fork(
    conn: &mut Conn,
    parent: &ForkPreparation,
    made: &crate::environments::Made,
    home: Id,
    clock: &dyn Clock,
    access: &dyn ResourceAccess,
) -> Result<(), EngineError> {
    if !orch_db::queries::transaction_guard::in_transaction(conn) {
        return Err(closed());
    }
    let check = parent
        .current_provision(conn, clock, access, made.id)
        .await?;
    let accepted = parent.retain_on(conn, clock, &check).await?;
    let row = jobs::lock(conn, made.job.id).await?.ok_or_else(closed)?;
    let env = env_rows::environment(conn, made.id)
        .await?
        .ok_or_else(closed)?;
    // xmin is PostgreSQL's 32-bit transaction identity; xid8 includes its wraparound epoch.
    let inserted_here: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs j JOIN environments e ON e.id=j.environment_id WHERE j.id=$1 AND e.id=$2 AND j.xmin=((pg_current_xact_id()::text::numeric % 4294967296)::text)::xid AND e.xmin=j.xmin)")
        .bind(made.job.id).bind(made.id).fetch_one(&mut *conn).await.map_err(DbError::from)?;
    if !inserted_here
        || row.kind != kind::ENVIRONMENT_PROVISION
        || row.environment_id != Some(made.id)
        || row.state != "queued"
        || row.cancel_requested()
        || row.created_by != Some(check.caller().user())
        || row.plan.get("mode").and_then(Json::as_str) != Some("make")
        || row.plan.get("generation").and_then(Json::as_i64) != Some(0)
        || row
            .plan
            .get("runtime")
            .is_some_and(|value| !value.is_null())
        || env.generation != 0
        || env.status != status::PROVISIONING
        || env.placement != "managed"
        || env.deleted_at.is_some()
        || env.home_node_id != home
        || env.owner_id != check.caller().user()
    {
        return Err(closed());
    }
    let authority = Authority {
        origin: accepted.origin().captured().clone(),
        standalone: None,
        thread: None,
        fork: Some(Box::new(accepted)),
        configuration: Some(crate::jobs::command_identity::hash(&env.config)?),
        wake: None,
        environment: made.id,
        home,
        job: row.id,
        template: env.template_id.ok_or_else(closed)?,
    };
    if let Some(previous) = read(&row)? {
        if previous != authority {
            return Err(closed());
        }
    } else {
        jobs::merge_plan(
            conn,
            row.id,
            &json!({FIELD:authority, VERSION:1}),
            clock.now(),
        )
        .await?;
    }
    check.check(clock)?;
    Ok(())
}

/// Retains the actual fork on a newly queued wake/update of its existing native target.
/// Existing work is waited for under its own source; it is never adopted by this fork.
pub async fn retain_fork_wake(
    conn: &mut Conn,
    parent: &ForkPreparation,
    environment: Id,
    job: Id,
    clock: &dyn Clock,
    access: &dyn ResourceAccess,
) -> Result<(), EngineError> {
    if !orch_db::queries::transaction_guard::in_transaction(conn) {
        return Err(closed());
    }
    let check = parent
        .current_wake(conn, clock, access, environment)
        .await?;
    let accepted = parent.retain_on(conn, clock, &check).await?;
    let row = jobs::lock(conn, job).await?.ok_or_else(closed)?;
    let env = env_rows::environment(conn, environment)
        .await?
        .ok_or_else(closed)?;
    let inserted_here: bool = sqlx::query_scalar("SELECT xmin=((pg_current_xact_id()::text::numeric % 4294967296)::text)::xid FROM jobs WHERE id=$1")
        .bind(job).fetch_one(&mut *conn).await.map_err(DbError::from)?;
    if !inserted_here
        || row.state != "queued"
        || row.cancel_requested()
        || row.environment_id != Some(environment)
        || row.created_by != Some(check.caller().user())
        || env.deleted_at.is_some()
        || env.remove_at.is_some()
        || env.placement != "managed"
        || is_retained(&row)
    {
        return Err(closed());
    }
    let runtime = env_rows::live_runtime(conn, environment).await?;
    let (mode, update, template) = match row.kind.as_str() {
        kind::ENVIRONMENT_PROVISION => {
            let mode = row
                .plan
                .get("mode")
                .and_then(Json::as_str)
                .ok_or_else(closed)?;
            if !matches!(mode, "make" | "start")
                || env.status != status::PROVISIONING
                || row.plan.get("generation") != Some(&json!(env.generation))
                || mode == "start" && runtime.is_none()
                || row
                    .plan
                    .get("runtime")
                    .is_some_and(|value| !value.is_null())
            {
                return Err(closed());
            }
            let template = if mode == "start" {
                runtime
                    .as_ref()
                    .map(|runtime| runtime.template_id)
                    .ok_or_else(closed)?
            } else {
                env.template_id.ok_or_else(closed)?
            };
            (mode.to_owned(), None, template)
        }
        kind::ENVIRONMENT_RECREATE => {
            let recipe: crate::updates::Recipe = serde_json::from_value(row.plan.clone())?;
            if recipe.target != crate::updates::Target::Environment(environment)
                || !recipe.auto_start
                || !recipe.was_stopped
                || recipe.by != check.caller().user()
                || env.status != "recreating"
                || runtime.as_ref().map(|unit| unit.id) != Some(recipe.old)
                || recipe.reason != orch_api_contract::environment::UpdatePreflightReason::Update
            {
                return Err(closed());
            }
            (
                "update".to_owned(),
                Some(crate::jobs::command_identity::hash(&serde_json::to_value(
                    &recipe,
                )?)?),
                recipe.template,
            )
        }
        _ => return Err(closed()),
    };
    let authority = Authority {
        origin: accepted.origin().captured().clone(),
        standalone: None,
        thread: None,
        fork: Some(Box::new(accepted)),
        configuration: Some(crate::jobs::command_identity::hash(&env.config)?),
        wake: Some(Wake {
            owner: env.owner_id,
            generation: env.generation,
            mode,
            unit: runtime.map(|unit| unit.id),
            update,
        }),
        environment,
        home: env.home_node_id,
        job,
        template,
    };
    jobs::merge_plan(conn, job, &json!({FIELD:authority, VERSION:2}), clock.now()).await?;
    check.check(clock)?;
    Ok(())
}

async fn fork_checkpoint(
    authority: &Authority,
    conn: &mut Conn,
    clock: &dyn Clock,
    access: &dyn ResourceAccess,
) -> Result<crate::jobs::fork::operation::Checkpoint, EngineError> {
    let parent = authority.fork.as_ref().ok_or_else(closed)?;
    if authority.wake.is_some() {
        parent
            .current_wake(conn, clock, access, authority.environment)
            .await
    } else {
        parent
            .current_provision(conn, clock, access, authority.environment)
            .await
    }
}

/// Acquire the parent before any child/command row. The locked child must retain this association.
pub(super) async fn lock_parent(
    conn: &mut Conn,
    row: &JobRow,
    clock: &dyn Clock,
    access: &dyn ResourceAccess,
) -> Result<(), EngineError> {
    if let Some(authority) = read(row)? {
        if authority.fork.is_some() {
            fork_checkpoint(&authority, conn, clock, access)
                .await?
                .check(clock)?;
        } else if authority.standalone == Some(true) {
            orch_db::queries::transaction_guard::lock_native_actor(conn, authority.origin.user())
                .await?
                .ok_or_else(closed)?;
        }
    }
    Ok(())
}
pub(super) fn preserves_parent(previous: &JobRow, current: &JobRow) -> Result<(), EngineError> {
    match (read(previous)?, read(current)?) {
        (None, None) => Ok(()),
        (Some(a), Some(b)) if a.same_parent(&b) => Ok(()),
        _ => Err(closed()),
    }
}

pub(super) async fn principal(
    conn: &mut Conn,
    row: &JobRow,
    clock: &dyn Clock,
    access: &dyn ResourceAccess,
) -> Result<Option<Principal>, EngineError> {
    Ok(current(conn, row, clock, access)
        .await?
        .map(|checked| checked.principal))
}

pub struct Checkpoint {
    principal: Principal,
    until: Option<Time>,
}
impl Checkpoint {
    pub fn principal(&self) -> &Principal {
        &self.principal
    }
    pub fn until(&self) -> Option<Time> {
        self.until
    }
    pub fn check(&self, clock: &dyn Clock) -> Result<(), EngineError> {
        if !self.principal.caller().key_deadline_allows(clock)
            || self.until.is_some_and(|until| clock.time() >= until)
        {
            return Err(closed());
        }
        Ok(())
    }
}
pub async fn current(
    conn: &mut Conn,
    row: &JobRow,
    clock: &dyn Clock,
    access: &dyn ResourceAccess,
) -> Result<Option<Checkpoint>, EngineError> {
    current_selected(conn, row, clock, access, None).await
}

/// The same checkpoint at the native selector's actual chosen Host before its placement write.
pub(super) async fn selected(
    conn: &mut Conn,
    row: &JobRow,
    host: Id,
    clock: &dyn Clock,
    access: &dyn ResourceAccess,
) -> Result<Option<Checkpoint>, EngineError> {
    current_selected(conn, row, clock, access, Some(host)).await
}
async fn current_selected(
    conn: &mut Conn,
    row: &JobRow,
    clock: &dyn Clock,
    access: &dyn ResourceAccess,
    selected_host: Option<Id>,
) -> Result<Option<Checkpoint>, EngineError> {
    if read(row)?.is_none() {
        return Ok(None);
    }
    lock_parent(conn, row, clock, access).await?;
    let actual = jobs::lock(conn, row.id).await?.ok_or_else(closed)?;
    preserves_parent(row, &actual)?;
    let row = &actual;
    let mut authority = read(row)?.ok_or_else(closed)?;
    let parent = if let Some(source) = &authority.fork {
        let check = fork_checkpoint(&authority, conn, clock, access).await?;
        let accepted = source.retain_on(conn, clock, &check).await?;
        authority.origin = accepted.origin().captured().clone();
        authority.fork = Some(Box::new(accepted));
        Some(check)
    } else {
        None
    };
    if row.id != authority.job
        || !matches!(
            row.kind.as_str(),
            kind::ENVIRONMENT_PROVISION | kind::ENVIRONMENT_RECREATE
        )
        || !row.is_open()
        || row.cancel_requested()
        || crate::jobs::runner::abort_reason(row)?.is_some()
        || row.environment_id != Some(authority.environment)
        || row.created_by != Some(authority.origin.user())
    {
        return Err(EngineError::unreadable(
            "The accepted creator provision no longer owns its work.",
        ));
    }
    let env = env_rows::environment(conn, authority.environment)
        .await?
        .filter(|env| {
            env.deleted_at.is_none()
                && env.remove_at.is_none()
                && env.home_node_id == authority.home
                && match &authority.wake {
                    None => {
                        env.generation == 0
                            && env.status == status::PROVISIONING
                            && env.template_id == Some(authority.template)
                            && (authority.fork.is_none() && authority.standalone.is_none()
                                || env.owner_id == authority.origin.user())
                    }
                    Some(wake) => {
                        env.generation == wake.generation
                            && env.owner_id == wake.owner
                            && env.status
                                == if wake.mode == "update" {
                                    "recreating"
                                } else {
                                    status::PROVISIONING
                                }
                    }
                }
        })
        .ok_or_else(|| EngineError::unreadable("The accepted new-environment binding changed."))?;
    match &authority.wake {
        None => {
            if row.kind != kind::ENVIRONMENT_PROVISION
                || row.plan.get("mode").and_then(Json::as_str) != Some("make")
                || row.plan.get("generation") != Some(&json!(0))
            {
                return Err(closed());
            }
        }
        Some(wake) if wake.mode == "update" => {
            let recipe: crate::updates::Recipe = serde_json::from_value(row.plan.clone())?;
            if row.kind != kind::ENVIRONMENT_RECREATE
                || Some(crate::jobs::command_identity::hash(&serde_json::to_value(
                    &recipe,
                )?)?)
                    != wake.update
                || recipe.target != crate::updates::Target::Environment(env.id)
                || Some(recipe.old) != wake.unit
                || recipe.template != authority.template
            {
                return Err(closed());
            }
        }
        Some(wake) => {
            if row.kind != kind::ENVIRONMENT_PROVISION
                || row.plan.get("mode").and_then(Json::as_str) != Some(wake.mode.as_str())
                || row.plan.get("generation") != Some(&json!(wake.generation))
            {
                return Err(closed());
            }
            if wake.mode == "start"
                && env_rows::live_runtime(conn, env.id)
                    .await?
                    .as_ref()
                    .map(|unit| unit.id)
                    != wake.unit
            {
                return Err(closed());
            }
        }
    }
    if let Some(expected) = authority.configuration {
        if crate::jobs::command_identity::hash(&env.config)? != expected {
            return Err(closed());
        }
    }
    let until = parent.as_ref().and_then(|check| check.until());
    let principal = match &parent {
        Some(check) => check.principal().clone(),
        None if authority.standalone == Some(true) => {
            crate::environment_creation::principal_current(conn, row, clock, selected_host).await?
        }
        None => {
            let caller = authority
                .origin
                .restore_current(conn, clock)
                .await?
                .ok_or_else(|| {
                    EngineError::unreadable("The creator credential is no longer valid.")
                })?;
            let plan = Prepared::for_selection(authority.origin.operation())
                .map_err(EngineError::unreadable)?;
            if !compatible_selection(&caller, plan.variant()) {
                return Err(EngineError::unreadable(
                    "The creator operation is no longer permitted.",
                ));
            }
            let mut facets = creator_files::parent_facets(
                conn,
                authority.origin.operation(),
                authority.thread.ok_or_else(closed)?,
                authority.environment,
            )
            .await?
            .ok_or_else(|| EngineError::unreadable("The creator parent changed."))?;
            let home = targets::node(conn, env.home_node_id)
                .await
                .map_err(EngineError::unreadable)?;
            facets.extend(creator_files::required_facet(
                &plan,
                TargetResolver::ResolvedCollection("creator_new_environment_homes"),
                KeyAction::Write,
                KeyTargetKind::Object(ObjectKind::Environment),
                &home,
            )?);
            let caller = caller
                .admit_facets(&facets)
                .map_err(EngineError::unreadable)?
                .into_caller();
            Principal::load(conn, caller, clock.time()).await?
        }
    };
    if authority.thread.is_some()
        && !orch_authz::nodes::check_named(
            conn,
            &principal,
            env.home_node_id,
            PermissionName::OfNode(NodePermission::Interact),
            Act::Use,
            Policy::default(),
        )
        .await?
        .is_allowed()
    {
        return Err(EngineError::unreadable(
            "The creator no longer has the environment home's Interact permission.",
        ));
    }
    if let Some(wake) = &authority.wake
        && (wake.mode == "start" || wake.mode == "update")
    {
        let old = env_rows::runtime(conn, wake.unit.ok_or_else(closed)?)
            .await?
            .ok_or_else(closed)?;
        let host = host_rows::host(conn, old.host_id)
            .await?
            .filter(|host| !host.disabled)
            .ok_or_else(closed)?;
        if old.environment_id != Some(env.id)
            || old.generation != wake.generation
            || host.lost_at.is_some()
        {
            return Err(closed());
        }
        let checked = Checkpoint { principal, until };
        checked.check(clock)?;
        jobs::merge_plan(conn, row.id, &json!({FIELD:authority}), clock.now()).await?;
        checked.check(clock)?;
        return Ok(Some(checked));
    }
    let template = host_rows::template(conn, authority.template)
        .await?
        .filter(|template| {
            template.enabled && template.deleted_at.is_none() && template.kind == "environment"
        })
        .ok_or_else(|| EngineError::unreadable("The admitted template is unavailable."))?;
    if let Some(runtime) = row
        .plan
        .get("runtime")
        .and_then(Json::as_str)
        .and_then(|runtime| runtime.parse::<Id>().ok())
    {
        let current = env_rows::live_runtime(conn, authority.environment)
            .await?
            .filter(|candidate| {
                candidate.id == runtime
                    && candidate.environment_id == Some(authority.environment)
                    && candidate.template_id == authority.template
                    && candidate.successor_of.is_none()
                    // This is the same existing Make candidate predicate used by source files;
                    // publication still stamps the actual runtime's generation, never a guess.
                    && env.generation.checked_add(1) == Some(candidate.generation)
                    && matches!(candidate.state.as_str(), "creating" | "running")
            })
            .ok_or_else(|| EngineError::unreadable("The admitted provision candidate changed."))?;
        if row.plan.get("host") != Some(&json!(current.host_id)) {
            return Err(EngineError::unreadable(
                "The admitted provision host changed.",
            ));
        }
        let wanted = places::wanted(conn, env.home_node_id, template, &env.config).await?;
        let minutes_idle = setting::<i64>(
            conn,
            SettingsPlace::Environment(env.id),
            keys::ENV_RECYCLE_IDLE_MINUTES,
        )
        .await?
        .unwrap_or(IDLE_MINUTES_DEFAULT);
        let ask = Ask {
            home: env.home_node_id,
            wanted: &wanted,
            current: None,
            minutes_idle,
            now: clock.now(),
        };
        let places = if authority.wake.is_some() && env.generation > 0 {
            places::retained_host_places(conn, &principal, &ask, env.id, current.host_id).await?
        } else {
            places::host_places_for_environment(conn, &principal, &ask, env.id).await?
        };
        // created proves current HostUse/TemplateUse including their actual options. Later load
        // or physical capacity warnings do not invent a new phase-authorization condition.
        if !places.iter().any(|place| {
            place.host.id == current.host_id
                && !place.host.disabled
                && (authority.wake.is_some() && env.generation > 0 || place.created.is_some())
        }) {
            return Err(EngineError::unreadable(
                "The creator no longer has the candidate host/template Use.",
            ));
        }
    }
    let checked = Checkpoint { principal, until };
    checked.check(clock)?;
    if authority.fork.is_some() {
        jobs::merge_plan(conn, row.id, &json!({FIELD:authority}), clock.now()).await?;
        checked.check(clock)?;
    }
    Ok(Some(checked))
}

pub async fn phase(job: &Job, phase: &str) -> Result<(), Stop> {
    if !is_retained(&job.row().await?) {
        return Ok(());
    }
    let engine = job.engine();
    let snapshot = job.row().await?;
    if phase == "commit"
        && snapshot
            .plan
            .get("provision_committed")
            .or_else(|| snapshot.plan.get("committed"))
            .and_then(Json::as_bool)
            == Some(true)
    {
        return Ok(());
    }
    let mut tx = engine.db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    lock_parent(
        &mut tx,
        &snapshot,
        engine.inner.clock.as_ref(),
        engine.inner.access.as_ref(),
    )
    .await?;
    let owner = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    preserves_parent(&snapshot, &owner)?;
    if read(&owner)?.is_none() {
        return Ok(());
    }
    if phase == "commit"
        && owner
            .plan
            .get("provision_committed")
            .or_else(|| owner.plan.get("committed"))
            .and_then(Json::as_bool)
            == Some(true)
    {
        return Ok(());
    }
    let checked = current(
        &mut tx,
        &owner,
        engine.inner.clock.as_ref(),
        engine.inner.access.as_ref(),
    )
    .await?
    .ok_or_else(closed)?;
    checked.check(engine.inner.clock.as_ref())?;
    tx.commit().await.map_err(DbError::from)?;
    checked.check(engine.inner.clock.as_ref())?;
    Ok(())
}

/// Rechecks the same retained creator source on a native child mutation transaction.
pub async fn guard_in(conn: &mut Conn, job: &Job) -> Result<(), Stop> {
    let snapshot = jobs::job(conn, job.id()).await?.ok_or(Stop::Lost)?;
    if !is_retained(&snapshot) {
        return Ok(());
    }
    let engine = job.engine();
    lock_parent(
        conn,
        &snapshot,
        engine.inner.clock.as_ref(),
        engine.inner.access.as_ref(),
    )
    .await?;
    let owner = job.lock_owned(conn).await?.ok_or(Stop::Lost)?;
    preserves_parent(&snapshot, &owner)?;
    current(
        conn,
        &owner,
        engine.inner.clock.as_ref(),
        engine.inner.access.as_ref(),
    )
    .await?
    .ok_or_else(closed)?
    .check(engine.inner.clock.as_ref())?;
    Ok(())
}

fn body_hash(body: &Json) -> Result<Hash, serde_json::Error> {
    // Issued bodies pass through JSONB before first send. Object key order is
    // transport representation, so preserve the same effect across that roundtrip.
    let mut body = body.clone();
    body.sort_all_objects();
    Ok(Hash::of(&serde_json::to_vec(&body)?))
}

fn canonical(command: &Outgoing) -> Result<Json, EngineError> {
    Ok(
        json!({"id":command.id,"machine":command.machine,"kind":command.kind.as_str(),
        "key":command.key.wire(),"body_hash":body_hash(&command.body)?.to_string()}),
    )
}

/// Runs at the job runner's canonical post-augmentation point for only retained creator jobs.
pub async fn remember_command(job: &Job, command: &Outgoing) -> Result<(), Stop> {
    if !is_retained(&job.row().await?) {
        return Ok(());
    }
    let engine = job.engine();
    let snapshot = job.row().await?;
    let mut tx = engine.db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    lock_parent(
        &mut tx,
        &snapshot,
        engine.inner.clock.as_ref(),
        engine.inner.access.as_ref(),
    )
    .await?;
    let owner = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    preserves_parent(&snapshot, &owner)?;
    if read(&owner)?.is_none() {
        return Ok(());
    }
    let checked = current(
        &mut tx,
        &owner,
        engine.inner.clock.as_ref(),
        engine.inner.access.as_ref(),
    )
    .await?
    .ok_or_else(closed)?;
    let native = canonical(command)?;
    let mut records = owner
        .plan
        .get(COMMANDS)
        .cloned()
        .unwrap_or_else(|| json!({}));
    let map = records
        .as_object_mut()
        .ok_or_else(|| EngineError::unreadable("The creator command journal is invalid."))?;
    if map
        .get(&command.id.to_string())
        .is_some_and(|old| old != &native)
    {
        return Err(EngineError::unreadable("The creator command identity changed.").into());
    }
    map.insert(command.id.to_string(), native);
    // IDs derive from this job and its finite template/phase operands. Restart repeats IDs;
    // existing Busy retry policy keeps its original deadline. No new retry counter or TTL exists.
    jobs::merge_plan(&mut tx, job.id(), &json!({COMMANDS:records}), job.now()).await?;
    checked.check(engine.inner.clock.as_ref())?;
    tx.commit().await.map_err(DbError::from)?;
    checked.check(engine.inner.clock.as_ref())?;
    Ok(())
}

async fn command_owner(
    conn: &mut Conn,
    command: Id,
    clock: &dyn Clock,
    access: &dyn ResourceAccess,
) -> Result<Option<JobRow>, EngineError> {
    let ids = jobs::creator_provision_command_owners(conn, command).await?;
    if ids.is_empty() {
        return Ok(None);
    }
    if ids.len() != 1 {
        return Err(closed());
    }
    let snapshot = jobs::job(conn, ids[0]).await?.ok_or_else(closed)?;
    lock_parent(conn, &snapshot, clock, access).await?;
    let ids_now = jobs::lock_creator_provision_command_owners(conn, command).await?;
    if ids_now != ids {
        return Err(closed());
    }
    let owner = jobs::job(conn, ids[0]).await?.ok_or_else(closed)?;
    preserves_parent(&snapshot, &owner)?;
    Ok(Some(owner))
}

/// Factual parent/child fences before the dispatcher's canonical command lock.
/// This makes no current-authority decision, so actual Accepted receipts stay native.
pub async fn lock_command_source(conn: &mut Conn, command: Id) -> Result<(), EngineError> {
    let ids = jobs::creator_provision_command_owners(conn, command).await?;
    if ids.len() > 1 {
        return Err(closed());
    }
    for id in ids {
        let snapshot = jobs::job(conn, id).await?.ok_or_else(closed)?;
        if let Some(authority) = read(&snapshot)? {
            if let Some(parent) = authority.fork {
                parent.lock_source(conn).await?;
            } else if authority.standalone == Some(true) {
                // This fence pins identity only; current() checks active standing for NEW work.
                let _: Option<Id> =
                    sqlx::query_scalar("SELECT id FROM users WHERE id=$1 FOR KEY SHARE")
                        .bind(authority.origin.user())
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(DbError::from)?;
            }
        }
        let owner = jobs::lock(conn, id).await?.ok_or_else(closed)?;
        preserves_parent(&snapshot, &owner)?;
    }
    Ok(())
}

/// First-send validation. The dispatcher preserves actual Accepted/terminal receipts before this call.
#[allow(
    clippy::too_many_arguments,
    reason = "the canonical native command fields and real source runtime ports are checked together"
)]
pub async fn command_authorized(
    conn: &mut Conn,
    command: Id,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
    clock: &dyn Clock,
    access: &dyn ResourceAccess,
) -> Result<bool, EngineError> {
    let owner = match command_owner(conn, command, clock, access).await {
        Ok(Some(owner)) => owner,
        Ok(None) => return Ok(true),
        Err(EngineError::BeforeEffect(_) | EngineError::Unreadable(_)) => return Ok(false),
        Err(error) => return Err(error),
    };
    if owner.plan.pointer("/active_command/id") != Some(&json!(command))
        || owner.plan.pointer("/active_command/machine") != Some(&json!(machine))
        || owner
            .plan
            .pointer("/active_command/kind")
            .and_then(Json::as_str)
            != Some(kind)
    {
        return Ok(false);
    }
    let native = json!({"id":command,"machine":machine,"kind":kind,"key":key,
        "body_hash":body_hash(body)?.to_string()});
    if owner
        .plan
        .get(COMMANDS)
        .and_then(Json::as_object)
        .and_then(|map| map.get(&command.to_string()))
        != Some(&native)
    {
        return Ok(false);
    }
    match current(conn, &owner, clock, access).await {
        Ok(principal) => Ok(principal.is_some()),
        // A withdrawn creator or changed retained binding refuses this new effect.
        // Database/access-engine failures retain their actual error category.
        Err(EngineError::Unreadable(_) | EngineError::BeforeEffect(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Current identity/time metadata for NEW first-send only; no completed-result reauthorization.
#[allow(
    clippy::too_many_arguments,
    reason = "the canonical native command fields and real source runtime ports are checked together"
)]
pub async fn command_current_callers(
    conn: &mut Conn,
    command: Id,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
    clock: &dyn orch_types::Clock,
    access: &dyn ResourceAccess,
) -> Result<Option<Vec<(Option<orch_authz::Caller>, Option<Time>)>>, EngineError> {
    if !command_authorized(conn, command, machine, key, kind, body, clock, access).await? {
        return Ok(None);
    }
    let Some(owner) = command_owner(conn, command, clock, access).await? else {
        return Ok(Some(Vec::new()));
    };
    match current(conn, &owner, clock, access).await {
        Ok(Some(checked)) => {
            checked.check(clock)?;
            Ok(Some(vec![(
                Some(checked.principal.caller().clone()),
                checked.until,
            )]))
        }
        Ok(None) | Err(EngineError::Unreadable(_) | EngineError::BeforeEffect(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests;
