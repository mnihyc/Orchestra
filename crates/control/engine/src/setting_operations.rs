//! Native setting subjects and continuing application of the same accepted policy edit.

use crate::{
    EngineError,
    operation_scope::{continuing, sources::Facts, targets},
};
use futures::future::BoxFuture;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::routes::{Endpoint, Route};
use orch_api_contract::settings::{NodeSettingDelete, NodeSettingSet};
use orch_api_contract::settings_sources::{self as sources, Source};
use orch_authz::{Act, Caller, Policy, Principal};
use orch_db::queries::{hierarchy, resources};
use orch_db::{Conn, DbError};
use orch_settings::{
    Place, SettingView, Target,
    validation::{self, Need, Needs},
};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Time};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;

pub trait Kind: Send + Sync + 'static {
    const ROUTE: Route;
    const DECLARATION: &'static Declaration<Source>;
}
pub struct Set;
pub struct Clear;
impl Kind for Set {
    const ROUTE: Route = NodeSettingSet::ROUTE;
    const DECLARATION: &'static Declaration<Source> = &sources::NODE_SET;
}
impl Kind for Clear {
    const ROUTE: Route = NodeSettingDelete::ROUTE;
    const DECLARATION: &'static Declaration<Source> = &sources::NODE_CLEAR;
}

/// Factual identity of a native accepted setting result; no scopes or caller are inferred from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub node: Id,
    pub key: String,
    pub fingerprint: String,
    pub lock_changed: bool,
    pub provider: Option<Id>,
}

pub fn fingerprint(view: &SettingView) -> Result<String, EngineError> {
    let bytes = serde_json::to_vec(&view.clone().into_setting(false))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

/// The original two native Node policy fields, with one source for HTTP and workers.
pub async fn policy_at(conn: &mut Conn, node: Id) -> Result<Policy, EngineError> {
    let allowed =
        orch_settings::get::<bool>(conn, Place::Node(node), keys::SHARING_ALLOW_EXTERNAL).await?;
    let holders =
        orch_settings::get::<bool>(conn, Place::Node(node), keys::SHARING_ALLOW_HOLDERS).await?;
    Ok(Policy {
        owners_name_outsiders: allowed.unwrap_or(true),
        holders_share: holders.unwrap_or(true),
    })
}

/// Actual value/lock/model subjects of the existing native setting validator.
pub async fn subject_facts(
    conn: &mut Conn,
    node: Id,
    needs: Needs,
    lock_changed: bool,
    provider: Option<Id>,
) -> Result<Vec<Facts<Source>>, EngineError> {
    let mut asks = vec![needs.value];
    if lock_changed {
        asks.push(needs.lock);
    }
    let mut objects = BTreeSet::<(ObjectKind, Id)>::new();
    for need in asks {
        match need {
            Need::OnEnvironment { environment, .. } => {
                objects.insert((ObjectKind::Environment, environment));
            }
            Need::OnProvider { provider, .. } => {
                objects.insert((ObjectKind::Provider, provider));
            }
            Need::OnNode { node: id, .. } => {
                let target = targets::node_identity(conn, id)
                    .await
                    .map_err(EngineError::unreadable)?;
                let KeyTargetKind::Object(kind) = target.object.kind else {
                    return Err(EngineError::unreadable(
                        "a setting node has no native node kind",
                    ));
                };
                objects.insert((kind, id));
            }
            Need::SystemAdmin => {
                let target = targets::node_identity(conn, node)
                    .await
                    .map_err(EngineError::unreadable)?;
                let KeyTargetKind::Object(kind) = target.object.kind else {
                    return Err(EngineError::unreadable(
                        "a setting node has no native node kind",
                    ));
                };
                objects.insert((kind, node));
            }
        }
    }
    if let Some(provider) = provider {
        objects.insert((ObjectKind::Provider, provider));
    }
    let mut groups = BTreeMap::<KeyTargetKind, Vec<_>>::new();
    for (kind, id) in objects {
        let target = if kind.node().is_some() {
            targets::node_identity(conn, id)
                .await
                .map_err(EngineError::unreadable)?
        } else {
            targets::resource_identity(conn, ObjectRef { kind, id })
                .await
                .map_err(EngineError::unreadable)?
        };
        groups
            .entry(KeyTargetKind::Object(kind))
            .or_default()
            .push(target);
    }
    Ok(vec![Facts::collection(
        Source::SettingSubjects,
        groups.into_iter().collect(),
    )])
}

/// The same native permission questions as the HTTP permit adapter; no operation scope grants a role.
pub async fn need_allowed(
    conn: &mut Conn,
    principal: &Principal,
    need: Need,
    policy: Policy,
) -> Result<bool, orch_authz::Error> {
    Ok(match need {
        Need::SystemAdmin => principal.is_system_admin(),
        Need::OnNode { node, permission } => {
            orch_authz::nodes::check(conn, principal, node, permission, Act::Use, policy)
                .await?
                .is_allowed()
        }
        Need::OnEnvironment {
            environment,
            permission,
        } => orch_authz::resources::check(
            conn,
            principal,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: environment,
            },
            permission,
            Policy::default(),
        )
        .await?
        .is_allowed(),
        Need::OnProvider {
            provider,
            permission,
        } => orch_authz::resources::check(
            conn,
            principal,
            ObjectRef {
                kind: ObjectKind::Provider,
                id: provider,
            },
            permission,
            Policy::default(),
        )
        .await?
        .is_allowed(),
    })
}

/// Actual native downstream target; a node policy does not become permission on another home.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum TargetWork {
    Policy,
    Environment(Id),
    Provider(Id),
    Wake(Id),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Work {
    pub change: Change,
    pub target: TargetWork,
}

pub struct Continuing<K>(PhantomData<K>);
pub struct Current {
    pub needs: Needs,
    pub path: Vec<Id>,
    pub target: TargetWork,
    pub target_path: Vec<Id>,
    pub runtime: Option<(Id, Id, Id)>,
    pub thread: Option<(Id, Option<Id>)>,
    pub connector: Option<(Id, i64)>,
}

fn native(error: impl std::fmt::Display) -> continuing::Error {
    continuing::Error::Native(EngineError::unreadable(error))
}

impl<K: Kind> continuing::Source for Continuing<K> {
    type Role = Source;
    type Input = Work;
    type Value = Current;
    const ROUTE: Route = K::ROUTE;
    const DECLARATION: &'static Declaration<Source> = K::DECLARATION;
    fn prepare<'a>(
        conn: &'a mut Conn,
        _caller: &'a Caller,
        work: &'a Work,
        _now: Time,
    ) -> BoxFuture<'a, Result<(Current, Vec<Facts<Source>>), continuing::Error>> {
        Box::pin(async move {
            let change = &work.change;
            let node = hierarchy::with_ancestors(conn, &[change.node])
                .await?
                .into_iter()
                .find(|node| node.id == change.node && node.deleted_at.is_none())
                .ok_or(continuing::Error::Refused)?;
            let view = orch_settings::resolve(conn, Place::Node(change.node), &change.key)
                .await
                .map_err(native)?;
            if fingerprint(&view)? != change.fingerprint {
                return Err(continuing::Error::Refused);
            }
            let needs = validation::needs(conn, &change.key, Target::Node(change.node))
                .await
                .map_err(native)?;
            let mut value = Current {
                needs,
                path: node.path,
                target: work.target,
                target_path: Vec::new(),
                runtime: None,
                thread: None,
                connector: None,
            };
            match work.target {
                TargetWork::Policy => {}
                TargetWork::Wake(id) => {
                    let node = hierarchy::with_ancestors(conn, &[id])
                        .await?
                        .into_iter()
                        .find(|node| node.id == id && node.deleted_at.is_none())
                        .ok_or(continuing::Error::Refused)?;
                    if !node.path.contains(&change.node) {
                        return Err(continuing::Error::Refused);
                    }
                    let thread = orch_db::queries::threads::thread(conn, id)
                        .await?
                        .ok_or(continuing::Error::Refused)?;
                    value.target_path = node.path;
                    value.thread = Some((thread.active_continuation_id, thread.current_segment_id));
                }
                TargetWork::Environment(id) | TargetWork::Provider(id) => {
                    let kind = if matches!(work.target, TargetWork::Environment(_)) {
                        ObjectKind::Environment
                    } else {
                        ObjectKind::Provider
                    };
                    let fact = resources::resource_facts(conn, &[id])
                        .await?
                        .into_iter()
                        .find(|fact| fact.id == id && fact.kind == kind.as_str() && !fact.deleted)
                        .ok_or(continuing::Error::Refused)?;
                    let home = fact.home_id.ok_or(continuing::Error::Refused)?;
                    let node = hierarchy::with_ancestors(conn, &[home])
                        .await?
                        .into_iter()
                        .find(|node| node.id == home && node.deleted_at.is_none())
                        .ok_or(continuing::Error::Refused)?;
                    if !node.path.contains(&change.node) {
                        return Err(continuing::Error::Refused);
                    }
                    value.target_path = node.path;
                    if kind == ObjectKind::Environment {
                        value.connector =
                            orch_db::queries::environments::hosting::environment(conn, id)
                                .await?
                                .and_then(|environment| {
                                    environment
                                        .machine_id
                                        .map(|machine| (machine, environment.generation.into()))
                                });
                    }
                    let runtime = match work.target {
                        TargetWork::Environment(_) => {
                            orch_db::queries::environments::hosting::live_runtime(conn, id).await?
                        }
                        _ => resources::provider_runtime::live_runtime(conn, id).await?,
                    };
                    if let Some(runtime) = runtime {
                        let host = resources::hosting::host(conn, runtime.host_id)
                            .await?
                            .ok_or(continuing::Error::Refused)?;
                        value.runtime = Some((runtime.id, runtime.host_id, host.machine_id));
                    }
                }
            }
            let facts = subject_facts(
                conn,
                change.node,
                needs,
                change.lock_changed,
                change.provider,
            )
            .await?;
            Ok((value, facts))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        work: &'a Work,
        current: &'a Current,
    ) -> BoxFuture<'a, Result<(), continuing::Error>> {
        Box::pin(async move {
            let policy = policy_at(conn, work.change.node).await?;
            if !need_allowed(conn, principal, current.needs.value, policy).await?
                || (work.change.lock_changed
                    && !need_allowed(conn, principal, current.needs.lock, policy).await?)
            {
                return Err(continuing::Error::Refused);
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        work: &'a Work,
        current: &'a Current,
    ) -> BoxFuture<'a, Result<(), continuing::Error>> {
        Box::pin(async move {
            if let TargetWork::Wake(id) = work.target {
                orch_db::queries::threads::tools::lock_threads(conn, &[id]).await?;
            }
            let ids = current
                .path
                .iter()
                .chain(&current.target_path)
                .copied()
                .collect::<BTreeSet<_>>();
            for id in ids {
                hierarchy::lock(conn, id).await?;
            }
            sqlx::query("SELECT key FROM settings WHERE node_id=$1 AND key=$2 FOR SHARE")
                .bind(work.change.node)
                .bind(&work.change.key)
                .fetch_all(&mut *conn)
                .await
                .map_err(DbError::from)?;
            match work.target {
                TargetWork::Environment(id) => {
                    resources::lock_resource(conn, "environment", id).await?;
                }
                TargetWork::Provider(id) => {
                    resources::lock_resource(conn, "provider", id).await?;
                }
                _ => {}
            }
            if let Some((id, _, _)) = current.runtime {
                sqlx::query("SELECT id FROM runtimes WHERE id=$1 FOR UPDATE")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
            }
            Ok(())
        })
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _work: &'a Work,
        old: &'a Current,
        fresh: &'a Current,
    ) -> BoxFuture<'a, Result<bool, continuing::Error>> {
        Box::pin(async move {
            Ok(old.needs == fresh.needs
                && old.path == fresh.path
                && old.target == fresh.target
                && old.target_path == fresh.target_path
                && old.runtime == fresh.runtime
                && old.thread == fresh.thread
                && old.connector == fresh.connector)
        })
    }
}

/// Only native commands in the accepted setting application, with no generic client-selected kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum NativeCommand {
    Mounts,
    Resources,
    Attach,
}
impl NativeCommand {
    fn wire(self) -> &'static str {
        match self {
            Self::Mounts => "node.mounts.apply",
            Self::Resources => "node.resources.apply",
            Self::Attach => "env.attach",
        }
    }
}

/// Private native evidence. The same complete payload/recipient remains bound through queue/send/replay.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandEvidence {
    origin: orch_authz::CapturedOperation,
    work: Work,
    runtime: Id,
    id: Id,
    machine: Id,
    kind: NativeCommand,
    key: Option<String>,
    body: orch_types::Json,
}
impl std::fmt::Debug for CommandEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SettingCommand([private])")
    }
}
impl CommandEvidence {
    pub(crate) fn capture(
        origin: &orch_authz::CapturedOperation,
        change: &Change,
        target: TargetWork,
        runtime: Id,
        command: &crate::commands::Outgoing,
    ) -> Result<Self, EngineError> {
        use orch_link_contract::commands::CommandKind;
        let kind = match command.kind {
            CommandKind::NodeMountsApply => NativeCommand::Mounts,
            CommandKind::NodeResourcesApply => NativeCommand::Resources,
            CommandKind::EnvAttach => NativeCommand::Attach,
            _ => {
                return Err(EngineError::unreadable(
                    "a setting application has an unrelated native command",
                ));
            }
        };
        if !matches!(origin.operation(), "NodeSettingSet" | "NodeSettingDelete")
            || runtime.is_nil()
            || change.node.is_nil()
        {
            return Err(EngineError::unreadable(
                "a setting command has no actual accepted operation",
            ));
        }
        Ok(Self {
            origin: origin.clone(),
            work: Work {
                change: change.clone(),
                target,
            },
            runtime,
            id: command.id,
            machine: command.machine,
            kind,
            key: command.key.wire(),
            body: command.body.clone(),
        })
    }
    pub fn command_kind(&self) -> &'static str {
        self.kind.wire()
    }
    pub(crate) fn origin(&self) -> &orch_authz::CapturedOperation {
        &self.origin
    }
    pub(crate) fn change(&self) -> &Change {
        &self.work.change
    }

    fn completed_mount(&self, row: &orch_db::queries::link::CommandRow) -> bool {
        self.kind == NativeCommand::Mounts
            && row.id == self.id
            && row.kind == self.kind.wire()
            && row.machine_id == self.machine
            && row.serial_key() == self.key.as_deref()
            && row.body == self.body
    }

    pub async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::CommandRow,
        clock: &dyn orch_types::Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current_proposed(
            conn,
            command.id,
            command.machine_id,
            &command.kind,
            command.serial_key(),
            &command.body,
            clock,
        )
        .await
    }

    /// Honest proposed native descriptor, also usable by the common pre-INSERT queue checkpoint.
    #[allow(
        clippy::too_many_arguments,
        reason = "the actual canonical proposed command and current native transaction"
    )]
    pub async fn current_proposed(
        &self,
        conn: &mut Conn,
        id: Id,
        machine: Id,
        kind: &str,
        key: Option<&str>,
        body: &orch_types::Json,
        clock: &dyn orch_types::Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if id != self.id
            || machine != self.machine
            || kind != self.kind.wire()
            || key != self.key.as_deref()
            || body != &self.body
        {
            return Ok(None);
        }
        match self.origin.operation() {
            "NodeSettingSet" => self.current::<Set>(conn, clock).await,
            "NodeSettingDelete" => self.current::<Clear>(conn, clock).await,
            _ => Ok(None),
        }
    }
    async fn current<K: Kind>(
        &self,
        conn: &mut Conn,
        clock: &dyn orch_types::Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let frame = match continuing::prepare::<Continuing<K>>(
            conn,
            clock,
            &self.origin,
            self.work.clone(),
        )
        .await
        {
            Ok(frame) => frame,
            Err(
                continuing::Error::Refused
                | continuing::Error::Binding(crate::operation_scope::BindingError::Refused),
            ) => return Ok(None),
            Err(continuing::Error::Native(error)) => return Err(error),
            Err(error) => return Err(EngineError::unreadable(error)),
        };
        let current = frame.value();
        let association = match self.kind {
            NativeCommand::Mounts | NativeCommand::Resources => {
                current.runtime.is_some_and(|(runtime, _, machine)| {
                    runtime == self.runtime && machine == self.machine
                })
            }
            NativeCommand::Attach => {
                current
                    .runtime
                    .is_some_and(|(runtime, _, _)| runtime == self.runtime)
                    && current
                        .connector
                        .is_some_and(|(machine, _)| machine == self.machine)
            }
        };
        if !association {
            return Ok(None);
        }
        let parts = frame.into_parts();
        let caller = parts.principal.caller().clone();
        if parts.first_effect.check(clock).is_err() {
            return Ok(None);
        }
        Ok(Some(vec![(Some(caller), None)]))
    }
}

/// Keeps the original accepted operation in the one server-private command channel.
pub(crate) fn command(
    origin: &orch_authz::CapturedOperation,
    change: &Change,
    target: TargetWork,
    runtime: Id,
    outgoing: crate::commands::Outgoing,
) -> Result<crate::commands::Outgoing, EngineError> {
    let evidence = CommandEvidence::capture(origin, change, target, runtime, &outgoing)?;
    Ok(outgoing.with_authority(crate::commands::CommandAuthority::setting(evidence)))
}

/// A pending native directory refresh keeps its exact completed mount command's private origin.
/// Canonical legacy None remains ordinary; malformed or unrelated present evidence never becomes None.
pub(crate) async fn last_mount_origin(
    conn: &mut Conn,
    runtime: Id,
) -> Result<Option<(orch_authz::CapturedOperation, Change)>, EngineError> {
    let id: Option<Id> = sqlx::query_scalar("SELECT id FROM commands WHERE kind='node.mounts.apply' AND body->>'runtime_id'=$1::uuid::text AND state='succeeded' ORDER BY created_at DESC,id DESC LIMIT 1")
        .bind(runtime).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
    let Some(id) = id else {
        return Ok(None);
    };
    let saved = orch_db::queries::link::command_with_authority(conn, id)
        .await?
        .ok_or_else(|| {
            EngineError::unreadable("the completed mount command lost its native record")
        })?;
    let Some(authority) = saved.authority else {
        return Ok(None);
    };
    let authority: crate::commands::CommandAuthority = serde_json::from_value(authority)?;
    match authority {
        crate::commands::CommandAuthority::Setting(evidence)
            if evidence.completed_mount(&saved.command) =>
        {
            Ok(Some((evidence.origin().clone(), evidence.change().clone())))
        }
        _ => Err(EngineError::unreadable(
            "the native mount refresh has mismatched original command evidence",
        )),
    }
}

pub(crate) struct Ready {
    pub value: std::sync::Arc<Current>,
    pub first_effect: continuing::FirstEffect,
}

async fn prepare_kind<K: Kind>(
    conn: &mut Conn,
    clock: &dyn orch_types::Clock,
    origin: &orch_authz::CapturedOperation,
    work: Work,
) -> Result<Ready, EngineError> {
    let frame = continuing::prepare::<Continuing<K>>(conn, clock, origin, work)
        .await
        .map_err(EngineError::unreadable)?;
    let parts = frame.into_parts();
    Ok(Ready {
        value: parts.value,
        first_effect: parts.first_effect,
    })
}

pub(crate) async fn prepare_work(
    conn: &mut Conn,
    clock: &dyn orch_types::Clock,
    origin: &orch_authz::CapturedOperation,
    work: Work,
) -> Result<Ready, EngineError> {
    match origin.operation() {
        "NodeSettingSet" => prepare_kind::<Set>(conn, clock, origin, work).await,
        "NodeSettingDelete" => prepare_kind::<Clear>(conn, clock, origin, work).await,
        _ => Err(EngineError::unreadable(
            "a setting application changed its captured operation",
        )),
    }
}

impl crate::Engine {
    /// The native existing followups retain the real accepted setting key and actual selected work.
    pub async fn setting_changed(
        &self,
        origin: &orch_authz::CapturedOperation,
        change: Change,
    ) -> Result<(), EngineError> {
        match change.key.as_str() {
            keys::ENV_MOUNTS => {
                let environments = orch_db::queries::environments::hosting::environments_under(
                    &mut *self.db().acquire().await?,
                    change.node,
                )
                .await?;
                for environment in environments {
                    self.apply_environment_mounts_for_setting(environment, origin, &change)
                        .await?;
                }
            }
            keys::ENV_RESOURCES | keys::RUNTIME_RESOURCES => {
                let environments = orch_db::queries::environments::hosting::environments_under(
                    &mut *self.db().acquire().await?,
                    change.node,
                )
                .await?;
                for environment in environments {
                    self.apply_runtime_resources_for_setting(
                        crate::updates::Target::Environment(environment),
                        origin,
                        &change,
                    )
                    .await?;
                }
                let providers: Vec<Id> = sqlx::query_scalar("SELECT p.id FROM providers p JOIN nodes n ON n.id=p.home_id WHERE $1=ANY(n.path) AND p.deleted_at IS NULL AND EXISTS(SELECT 1 FROM runtimes r WHERE r.provider_id=p.id AND r.state NOT IN ('retired','removed'))")
                    .bind(change.node).fetch_all(&mut *self.db().acquire().await?).await.map_err(DbError::from)?;
                for provider in providers {
                    self.apply_runtime_resources_for_setting(
                        crate::updates::Target::Provider(provider),
                        origin,
                        &change,
                    )
                    .await?;
                }
            }
            keys::AUTH_SECOND_STEP => {
                let mut conn = self.db().acquire().await?;
                let held = orch_db::queries::threads::threads_held_for_second_step(&mut conn, None)
                    .await?;
                let nodes = hierarchy::with_ancestors(&mut conn, &held).await?;
                let held = held
                    .into_iter()
                    .filter(|id| {
                        nodes
                            .iter()
                            .any(|node| node.id == *id && node.path.contains(&change.node))
                    })
                    .collect::<Vec<_>>();
                drop(conn);
                for thread in held {
                    let mut tx = self.db().begin().await?;
                    let ready = prepare_work(
                        &mut tx,
                        self.inner.clock.as_ref(),
                        origin,
                        Work {
                            change: change.clone(),
                            target: TargetWork::Wake(thread),
                        },
                    )
                    .await?;
                    ready
                        .first_effect
                        .check(self.inner.clock.as_ref())
                        .map_err(EngineError::unreadable)?;
                    // This is only the existing signal. Its input's own retained author still governs preflight.
                    self.wake(thread);
                    tx.commit().await.map_err(orch_db::DbError::from)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}
