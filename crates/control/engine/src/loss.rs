//! Reviewed permanent loss closes unreachable work atomically and preserves its recoverable identity.

use crate::{Engine, EngineError};
use orch_access_contract::{
    decisions::ObjectRef,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    loss::*,
    nodes::{NodeKind, NodeRef},
};
use orch_authz::{Act, Policy, Principal};
use orch_db::{
    Conn, DbError,
    queries::{jobs, loss as rows},
};
use orch_types::{Hash, Id, Time, new_id};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::HashSet, sync::Arc};

mod preview_origin;

/// An additional projection bound over native names; it supplies no ordinary permission.
pub struct NameBounds {
    allowed: HashSet<ObjectRef>,
}
impl NameBounds {
    /// Keep only actual typed name subjects selected by the consuming operation.
    pub fn new(allowed: impl IntoIterator<Item = ObjectRef>) -> Self {
        Self {
            allowed: allowed.into_iter().collect(),
        }
    }
}

fn name_allowed(bounds: Option<&NameBounds>, kind: ObjectKind, id: Id) -> bool {
    bounds.is_none_or(|bounds| bounds.allowed.contains(&ObjectRef { kind, id }))
}

/// Actual potentially named subjects of this native dependency cut, before name projection.
pub async fn preview_name_candidates(
    conn: &mut Conn,
    inventory: &rows::Inventory,
) -> Result<HashSet<ObjectRef>, LossError> {
    let mut subjects = inventory
        .environments
        .iter()
        .map(|row| ObjectRef {
            kind: ObjectKind::Environment,
            id: row.id,
        })
        .chain(inventory.providers.iter().map(|id| ObjectRef {
            kind: ObjectKind::Provider,
            id: *id,
        }))
        .chain(inventory.mounts.iter().map(|id| ObjectRef {
            kind: ObjectKind::Mount,
            id: *id,
        }))
        .collect::<HashSet<_>>();
    let environments = inventory
        .environments
        .iter()
        .map(|row| row.id)
        .collect::<Vec<_>>();
    for thread in orch_db::queries::environments::threads_in(conn, &environments).await? {
        subjects.insert(ObjectRef {
            kind: ObjectKind::Thread,
            id: thread.thread_id,
        });
    }
    subjects.extend(inventory.provider_threads.iter().map(|id| ObjectRef {
        kind: ObjectKind::Thread,
        id: *id,
    }));
    Ok(subjects)
}

/// Why a permanent-loss request cannot be accepted.
#[derive(Debug, thiserror::Error)]
pub enum LossError {
    /// The resource or reviewed plan is not available to this caller.
    #[error("the resource or loss plan is not available")]
    NotFound,
    /// The caller does not manage the resource.
    #[error("only this resource's managers or the machine's owner may declare it lost")]
    Denied,
    /// Loss cannot be declared while the resource answers.
    #[error("the resource still answers; stop or remove it through its ordinary controls")]
    Online,
    /// A dependency or authority changed after the preview.
    #[error("what is on this resource changed; review a fresh loss plan")]
    Stale,
    /// Typed names must match exactly.
    #[error("type the resource's exact name to declare it lost")]
    Confirmation,
    /// The database could not answer.
    #[error(transparent)]
    Db(#[from] DbError),
    /// The orchestration infrastructure failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

#[derive(Serialize, Deserialize)]
struct Stored {
    plan: LossPlan,
    fingerprint: Hash,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preview_origin: Option<crate::operation_scope::http_origin::HttpOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    named: Option<HashSet<ObjectRef>>,
}

/// The actual native review record and audience before its once-only persistence.
pub struct PreparedLossPreview {
    target: LossTarget,
    plan: LossPlan,
    inventory: rows::Inventory,
    fingerprint: Hash,
    audience: Vec<crate::notifications::OperationRecipient>,
    creator: Id,
    created_at: chrono::DateTime<chrono::Utc>,
    named: HashSet<ObjectRef>,
}
impl PreparedLossPreview {
    /// The actual native input subject, independent of the rendered plan.
    pub fn target(&self) -> LossTarget {
        self.target
    }
    /// Native response operands, not a source of inferred scope targets.
    pub fn plan(&self) -> &LossPlan {
        &self.plan
    }
    /// The actual actor whose native review is being saved.
    pub fn creator(&self) -> Id {
        self.creator
    }
    /// The actual selected native dependency cut, before response formatting.
    pub fn inventory(&self) -> &rows::Inventory {
        &self.inventory
    }
    /// Actual native names emitted by this preparation, recorded before wire formatting.
    pub fn named_sources(&self) -> &HashSet<ObjectRef> {
        &self.named
    }
}

/// The exact reviewed native cascade, after its complete preparation and before mutation.
pub struct PreparedLossDeclaration {
    preview: Arc<PreparedLossPreview>,
    affected: Vec<ObjectRef>,
    operation: crate::notifications::NotificationOperation,
    now: chrono::DateTime<chrono::Utc>,
    preview_frame: Option<crate::operation_scope::continuing::Frame<preview_origin::Preview>>,
}
impl PreparedLossDeclaration {
    /// The original native content and source cut, independent of the response shape.
    pub fn preview(&self) -> Arc<PreparedLossPreview> {
        Arc::clone(&self.preview)
    }
    /// Every actual resource changed by this native cascade.
    pub fn affected(&self) -> &[ObjectRef] {
        &self.affected
    }
    /// The real physical connections closed only after the declaration commits.
    pub fn machines(&self) -> &[Id] {
        &self.preview.inventory.machines
    }
    fn check_deadline(&self, clock: &dyn orch_types::Clock) -> Result<(), LossError> {
        if self.preview.plan.expires_at.get() <= clock.now() {
            return Err(LossError::Stale);
        }
        if let Some(frame) = &self.preview_frame {
            frame
                .check_deadline(clock)
                .map_err(preview_origin::refused)?;
        }
        Ok(())
    }
    /// Confirm this accepted effect's immutable journal and exact changed rows, without replay.
    pub async fn current(&self, conn: &mut Conn) -> Result<bool, LossError> {
        if orch_db::queries::notifications::operations::get(conn, self.operation.id).await?
            != Some(serde_json::to_value(&self.operation).map_err(EngineError::from)?)
        {
            return Ok(false);
        }
        for environment in &self.preview.inventory.environments {
            let at: Option<chrono::DateTime<chrono::Utc>> =
                sqlx::query_scalar("SELECT lost_at FROM environments WHERE id=$1")
                    .bind(environment.id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(DbError::from)?
                    .flatten();
            if at != Some(self.now) {
                return Ok(false);
            }
        }
        for machine in &self.preview.inventory.machines {
            let at: Option<chrono::DateTime<chrono::Utc>> =
                sqlx::query_scalar("SELECT lost_at FROM machines WHERE id=$1")
                    .bind(machine)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(DbError::from)?
                    .flatten();
            if at != Some(self.now) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// Select the cascade's actual native resources, before scope admission or response formatting.
async fn declaration_objects(
    conn: &mut Conn,
    target: LossTarget,
    inventory: &rows::Inventory,
) -> Result<Vec<ObjectRef>, LossError> {
    let mut objects = HashSet::new();
    for (kind, ids) in [
        (
            ObjectKind::Environment,
            inventory.environments.iter().map(|row| row.id).collect(),
        ),
        (ObjectKind::Machine, inventory.machines.clone()),
        (ObjectKind::Provider, inventory.providers.clone()),
        (ObjectKind::Mount, inventory.mounts.clone()),
        (
            ObjectKind::Thread,
            inventory
                .threads
                .iter()
                .chain(&inventory.provider_threads)
                .copied()
                .collect(),
        ),
    ] {
        objects.extend(ids.into_iter().map(|id| ObjectRef { kind, id }));
    }
    if !matches!(target, LossTarget::Environment { .. })
        && let Some(id) = inventory.host
    {
        objects.insert(ObjectRef {
            kind: ObjectKind::Host,
            id,
        });
    }
    let servers: Vec<Id> = sqlx::query_scalar("SELECT DISTINCT server_id FROM mcp_server_offerings WHERE machine_id=ANY($1) AND state<>'lost' ORDER BY server_id")
        .bind(&inventory.machines).fetch_all(conn).await.map_err(DbError::from)?;
    objects.extend(servers.into_iter().map(|id| ObjectRef {
        kind: ObjectKind::McpServer,
        id,
    }));
    objects.remove(&target.object());
    let mut objects = objects.into_iter().collect::<Vec<_>>();
    objects.sort_by_key(|object| (object.kind, object.id));
    Ok(objects)
}

/// Read the actual private review subject without using the submitted nonce as scope authority.
pub fn declaration_target(row: &jobs::forks::PreflightRow) -> Result<LossTarget, LossError> {
    let stored: Stored = serde_json::from_value(row.plan.clone()).map_err(EngineError::from)?;
    Ok(stored.plan.target)
}

/// The original native preview with only currently readable recorded names retained.
pub struct ProjectedLossPreview {
    plan: LossPlan,
    named: HashSet<ObjectRef>,
}
impl ProjectedLossPreview {
    /// The same frozen result, ready for its one transport formatter.
    pub fn plan(&self) -> &LossPlan {
        &self.plan
    }
    /// Actual recorded name sources still emitted by this native projection.
    pub fn named_sources(&self) -> &HashSet<ObjectRef> {
        &self.named
    }
}

/// Narrow recorded names without reopening inventory, audience, a selector window or planning.
pub async fn project_loss_preview(
    conn: &mut Conn,
    principal: &Principal,
    prepared: &PreparedLossPreview,
    bounds: Option<&NameBounds>,
) -> Result<ProjectedLossPreview, LossError> {
    let mut named = HashSet::new();
    for object in &prepared.named {
        if name_allowed(bounds, object.kind, object.id)
            && name_visible(conn, principal, *object).await?
        {
            named.insert(*object);
        }
    }
    let mut plan = prepared.plan.clone();
    for environment in &mut plan.environments {
        if !named.contains(&ObjectRef {
            kind: ObjectKind::Environment,
            id: environment.environment.id,
        }) {
            environment.environment.name = None;
        }
        let before = environment.threads.len();
        environment.threads.retain(|thread| {
            named.contains(&ObjectRef {
                kind: ObjectKind::Thread,
                id: thread.id,
            })
        });
        environment.threads_hidden += before.saturating_sub(environment.threads.len()) as i64;
    }
    for (kind, resources) in [
        (ObjectKind::Provider, &mut plan.providers),
        (ObjectKind::Mount, &mut plan.mounts),
    ] {
        for resource in resources {
            if !named.contains(&ObjectRef {
                kind,
                id: resource.id,
            }) {
                resource.name = None;
            }
        }
    }
    let before = plan.provider_threads.len();
    plan.provider_threads.retain(|thread| {
        named.contains(&ObjectRef {
            kind: ObjectKind::Thread,
            id: thread.id,
        })
    });
    plan.provider_threads_hidden += before.saturating_sub(plan.provider_threads.len()) as i64;
    Ok(ProjectedLossPreview { plan, named })
}

/// The actual loss subject of the privately kept native preview, before audience formatting.
pub fn recipient_subject(plan: &orch_types::Json) -> Result<ObjectRef, EngineError> {
    let stored: Stored = serde_json::from_value(plan.clone())?;
    Ok(stored.plan.target.object())
}

/// Checks the source actor without borrowing authority over environments that merely run there.
pub async fn allowed(
    conn: &mut Conn,
    principal: &Principal,
    target: LossTarget,
) -> Result<bool, LossError> {
    if let LossTarget::Machine { id } = target {
        if orch_db::queries::resources::hosting::host_of_machine(conn, id)
            .await?
            .is_some()
        {
            return Ok(orch_authz::resources::check_machine(
                conn,
                principal,
                id,
                Permission::MachineManage,
                Policy::default(),
            )
            .await
            .map_err(EngineError::from)?
            .is_allowed());
        }
        let owns = orch_db::queries::resources::machine(conn, id)
            .await?
            .is_some_and(|row| row.owner_id == Some(principal.user()));
        return Ok((owns
            || crate::operation_access::can_bypass(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Machine,
                    id,
                },
                Permission::MachineManage,
            )
            .await?)
            && orch_authz::resources::check(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Machine,
                    id,
                },
                Permission::MachineManage,
                Policy::default(),
            )
            .await
            .map_err(EngineError::from)?
            .is_allowed());
    }
    let (kind, permission) = match target {
        LossTarget::Host { .. } => (ObjectKind::Host, Permission::HostManage),
        _ => (ObjectKind::Environment, Permission::EnvironmentManage),
    };
    Ok(orch_authz::resources::check(
        conn,
        principal,
        ObjectRef {
            kind,
            id: target.id(),
        },
        permission,
        Policy::default(),
    )
    .await
    .map_err(EngineError::from)?
    .is_allowed())
}

async fn visible(
    conn: &mut Conn,
    principal: &Principal,
    kind: ObjectKind,
    id: Id,
    permission: Permission,
) -> Result<bool, LossError> {
    Ok(orch_authz::resources::check(
        conn,
        principal,
        ObjectRef { kind, id },
        permission,
        Policy::default(),
    )
    .await
    .map_err(EngineError::from)?
    .is_allowed())
}

async fn name_visible(
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
) -> Result<bool, LossError> {
    let permission = match object.kind {
        ObjectKind::Environment => Permission::EnvironmentView,
        ObjectKind::Provider => Permission::ProviderView,
        ObjectKind::Mount => Permission::MountView,
        ObjectKind::Thread => {
            return Ok(orch_authz::nodes::check(
                conn,
                principal,
                object.id,
                Permission::ThreadView,
                Act::Use,
                Policy::default(),
            )
            .await
            .map_err(EngineError::from)?
            .is_allowed());
        }
        _ => return Ok(false),
    };
    visible(conn, principal, object.kind, object.id, permission).await
}

async fn shown(
    conn: &mut Conn,
    principal: &Principal,
    target: LossTarget,
    inventory: &rows::Inventory,
    now: chrono::DateTime<chrono::Utc>,
    bounds: Option<&NameBounds>,
) -> Result<(LossPlan, HashSet<ObjectRef>), LossError> {
    let mut named = HashSet::new();
    let mut environments = Vec::new();
    let mut kept_bytes = inventory.mount_bytes;
    for environment in &inventory.environments {
        let mut threads = Vec::new();
        let mut hidden = 0;
        for thread in orch_db::queries::environments::threads_in(conn, &[environment.id]).await? {
            if name_allowed(bounds, ObjectKind::Thread, thread.thread_id)
                && name_visible(
                    conn,
                    principal,
                    ObjectRef {
                        kind: ObjectKind::Thread,
                        id: thread.thread_id,
                    },
                )
                .await?
            {
                named.insert(ObjectRef {
                    kind: ObjectKind::Thread,
                    id: thread.thread_id,
                });
                threads.push(NodeRef {
                    thread_display: None,
                    id: thread.thread_id,
                    kind: NodeKind::Thread,
                    name: Some(thread.name),
                    redacted: false,
                });
            } else {
                hidden += 1;
            }
        }
        let name = (name_allowed(bounds, ObjectKind::Environment, environment.id)
            && name_visible(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: environment.id,
                },
            )
            .await?)
            .then(|| environment.name.clone());
        if name.is_some() {
            named.insert(ObjectRef {
                kind: ObjectKind::Environment,
                id: environment.id,
            });
        }
        environments.push(LossEnvironment {
            environment: LossResource {
                id: environment.id,
                name,
            },
            threads,
            threads_hidden: hidden,
            kept_bytes: environment.retained_bytes,
        });
        if environment.retained {
            kept_bytes = kept_bytes
                .zip(environment.retained_bytes)
                .map(|(a, b)| a.saturating_add(b));
        }
    }
    let mut providers = Vec::new();
    for id in &inventory.providers {
        let name = if name_allowed(bounds, ObjectKind::Provider, *id)
            && name_visible(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Provider,
                    id: *id,
                },
            )
            .await?
        {
            orch_db::queries::resources::thread_engine::provider(conn, *id)
                .await?
                .map(|p| p.name)
        } else {
            None
        };
        if name.is_some() {
            named.insert(ObjectRef {
                kind: ObjectKind::Provider,
                id: *id,
            });
        }
        providers.push(LossResource { id: *id, name });
    }
    let mut mounts = Vec::new();
    for (id, name) in
        orch_db::queries::resources::hosting::mount_names(conn, &inventory.mounts).await?
    {
        let name = (name_allowed(bounds, ObjectKind::Mount, id)
            && name_visible(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Mount,
                    id,
                },
            )
            .await?)
            .then_some(name);
        if name.is_some() {
            named.insert(ObjectRef {
                kind: ObjectKind::Mount,
                id,
            });
        }
        mounts.push(LossResource { id, name });
    }
    let mut provider_threads = Vec::new();
    let mut provider_threads_hidden = 0;
    for thread in &inventory.provider_threads {
        if name_allowed(bounds, ObjectKind::Thread, *thread)
            && name_visible(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Thread,
                    id: *thread,
                },
            )
            .await?
        {
            if let Some(row) = orch_db::queries::threads::thread(conn, *thread).await? {
                named.insert(ObjectRef {
                    kind: ObjectKind::Thread,
                    id: *thread,
                });
                provider_threads.push(NodeRef {
                    thread_display: None,
                    id: *thread,
                    kind: NodeKind::Thread,
                    name: Some(row.name),
                    redacted: false,
                });
            }
        } else {
            provider_threads_hidden += 1;
        }
    }
    Ok((
        LossPlan {
            admin_grants: Vec::new(),
            notification: Default::default(),
            preflight_id: Id::nil(),
            expires_at: Time::new(crate::updates::expires(now)),
            target,
            name: inventory.name.clone(),
            environments,
            provider_threads,
            provider_threads_hidden,
            mounts,
            providers,
            jobs: inventory.jobs.len() as i64,
            kept_bytes,
        },
        named,
    ))
}

/// Expands current resource managers through the same roles and groups as permission decisions.
pub(crate) async fn managers(
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
    wanted: Permission,
) -> Result<Vec<Id>, EngineError> {
    let (object, wanted) = if object.kind == ObjectKind::Machine {
        orch_authz::resources::machine_permission(conn, object.id, wanted).await?
    } else {
        (object, wanted)
    };
    let (graph, _, site) = orch_authz::resources::site(conn, object).await?;
    let mut asked = vec![wanted];
    asked.extend(wanted.held_through());
    let frame = orch_authz::engine::Frame::load(
        conn,
        principal,
        Policy::default(),
        graph,
        std::slice::from_ref(&site),
        &asked,
    )
    .await?;
    Ok(orch_authz::holders::who_holds_for_notifications(conn, &frame, &site, wanted).await?)
}

impl Engine {
    /// The original native management and availability gate over actual loss dependencies.
    pub async fn loss_preview_inventory(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        target: LossTarget,
    ) -> Result<rows::Inventory, LossError> {
        if !allowed(conn, principal, target).await? {
            return Err(LossError::Denied);
        }
        let inventory = rows::inventory(conn, target.kind(), target.id())
            .await?
            .ok_or(LossError::NotFound)?;
        if inventory.online || inventory.lost {
            return Err(LossError::Online);
        }
        Ok(inventory)
    }

    /// Plans permanent loss, with names filtered under the caller's current authority.
    pub async fn plan_loss(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        target: LossTarget,
    ) -> Result<LossPlan, LossError> {
        let prepared = self
            .prepare_loss_preview(conn, principal, target, new_id())
            .await?;
        self.save_loss_preview(conn, &prepared).await?;
        Ok(prepared.plan)
    }

    /// Effect-free native loss plan and actual audience; the supplied nonce is never an authority target.
    pub async fn prepare_loss_preview(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        target: LossTarget,
        preview: Id,
    ) -> Result<PreparedLossPreview, LossError> {
        self.prepare_loss_preview_with_names(conn, principal, target, preview, None)
            .await
    }

    /// The same native planner with an additional typed bound over independently protected names.
    pub async fn prepare_loss_preview_with_names(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        target: LossTarget,
        preview: Id,
        bounds: Option<&NameBounds>,
    ) -> Result<PreparedLossPreview, LossError> {
        let inventory = self.loss_preview_inventory(conn, principal, target).await?;
        let (mut plan, named) = shown(
            conn,
            principal,
            target,
            &inventory,
            self.inner.clock.now(),
            bounds,
        )
        .await?;
        let mut audience = crate::notifications::operation_audience(
            conn,
            principal,
            &inventory
                .environments
                .iter()
                .map(|environment| environment.id)
                .collect::<Vec<_>>(),
            &inventory.providers,
        )
        .await?;
        if let LossTarget::Machine { id } = target {
            let owner = orch_db::queries::resources::machine(conn, id)
                .await?
                .and_then(|row| row.owner_id);
            if owner != Some(principal.user()) {
                plan.admin_grants.push(OperationAdminGrant {
                    object: ObjectRef {
                        kind: ObjectKind::Machine,
                        id,
                    },
                    permission: Permission::MachineManage,
                    name: plan.name.clone(),
                });
            }
        } else {
            if let Some(grant) = crate::operation_access::review(
                conn,
                principal,
                ObjectRef {
                    kind: if matches!(target, LossTarget::Host { .. }) {
                        ObjectKind::Host
                    } else {
                        ObjectKind::Environment
                    },
                    id: target.id(),
                },
                if matches!(target, LossTarget::Host { .. }) {
                    Permission::HostManage
                } else {
                    Permission::EnvironmentManage
                },
            )
            .await
            .map_err(|error| match error {
                crate::updates::UpdateError::Denied => LossError::Denied,
                other => LossError::Internal(EngineError::unreadable(other)),
            })? {
                plan.admin_grants.push(grant);
            }
        }
        enrich_audience(conn, principal, target, &inventory, &mut audience).await?;
        let notification_place = match target {
            LossTarget::Environment { id } => orch_settings::Place::Environment(id),
            LossTarget::Host { id } => orch_db::queries::resources::hosting::host(conn, id)
                .await?
                .and_then(|host| host.home_id)
                .map_or(orch_settings::Place::System, orch_settings::Place::Node),
            LossTarget::Machine { .. } => orch_settings::Place::System,
        };
        plan.preflight_id = preview;
        plan.notification = OperationNotificationPlan {
            preview_id: Some(plan.preflight_id),
            people: audience.len() as i64,
            default_on: crate::notifications::operation_default(
                conn,
                notification_place,
                !inventory.threads.is_empty() || !inventory.providers.is_empty(),
            )
            .await?,
        };
        let fingerprint = inventory.fingerprint()?;
        Ok(PreparedLossPreview {
            target,
            plan,
            inventory,
            fingerprint,
            audience,
            creator: principal.user(),
            created_at: self.inner.clock.now(),
            named,
        })
    }

    /// Save exactly the prepared native preview and its audience on the caller's guarded transaction.
    /// Serialization is effect-free; the first SQL is the native preflight insert.
    pub async fn save_loss_preview(
        &self,
        conn: &mut Conn,
        prepared: &PreparedLossPreview,
    ) -> Result<(), LossError> {
        self.save_loss_preview_origin(conn, prepared, None).await
    }

    /// Retain the actual accepted preview operation beside its frozen native review.
    pub async fn save_loss_preview_with_origin(
        &self,
        conn: &mut Conn,
        prepared: &PreparedLossPreview,
        origin: &crate::operation_scope::http_origin::HttpOrigin,
    ) -> Result<(), LossError> {
        if origin.origin().operation() != orch_api_contract::loss_sources::PREFLIGHT.operation
            || origin.origin().user() != prepared.creator
        {
            return Err(
                EngineError::unreadable("the loss preview changed its native origin").into(),
            );
        }
        self.save_loss_preview_origin(conn, prepared, Some(origin.clone()))
            .await
    }

    async fn save_loss_preview_origin(
        &self,
        conn: &mut Conn,
        prepared: &PreparedLossPreview,
        preview_origin: Option<crate::operation_scope::http_origin::HttpOrigin>,
    ) -> Result<(), LossError> {
        let stored = Stored {
            plan: prepared.plan.clone(),
            fingerprint: prepared.fingerprint,
            preview_origin,
            named: Some(prepared.named.clone()),
        };
        let body = serde_json::to_value(stored).map_err(EngineError::from)?;
        jobs::forks::save_preflight(
            conn,
            &jobs::forks::PreflightRow {
                id: prepared.plan.preflight_id,
                kind: "loss".into(),
                node_id: None,
                environment_id: match prepared.plan.target {
                    LossTarget::Environment { id } => Some(id),
                    _ => None,
                },
                created_by: prepared.creator,
                plan: body,
                expires_at: prepared.plan.expires_at.get(),
                created_at: prepared.created_at,
            },
        )
        .await?;
        orch_db::queries::notifications::operations::preview(
            conn,
            prepared.plan.preflight_id,
            &prepared
                .audience
                .iter()
                .map(|person| person.user)
                .collect::<Vec<_>>(),
        )
        .await?;
        Ok(())
    }

    /// Declares an unchanged reviewed inventory lost inside the caller's mutation transaction.
    pub async fn declare_loss(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        body: &LossDeclareBody,
    ) -> Result<LossPlan, LossError> {
        let prepared = self.prepare_loss_declaration(conn, principal, body).await?;
        self.apply_loss_declaration(conn, principal, &prepared)
            .await?;
        Ok(prepared.preview.plan.clone())
    }

    /// Check the native reviewed inventory, confirmation and full lock footprint without writing.
    pub async fn prepare_loss_declaration(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        body: &LossDeclareBody,
    ) -> Result<PreparedLossDeclaration, LossError> {
        if !orch_db::queries::transaction_guard::in_transaction(conn) {
            return Err(EngineError::unreadable("loss declaration requires a transaction").into());
        }
        orch_db::queries::transaction_guard::lock_authority(conn).await?;
        let row = jobs::changes::lock_plan(conn, body.preflight_id)
            .await?
            .filter(|row| row.kind == "loss" && row.created_by == principal.user())
            .ok_or(LossError::NotFound)?;
        let stored: Stored = serde_json::from_value(row.plan.clone()).map_err(EngineError::from)?;
        let target = stored.plan.target;
        if !allowed(conn, principal, target).await? {
            return Err(LossError::Denied);
        }
        if row.expires_at <= self.inner.clock.now() {
            return Err(LossError::Stale);
        }
        if body.confirm != stored.plan.name {
            return Err(LossError::Confirmation);
        }
        orch_db::queries::resources::lock_resource(conn, target.kind(), target.id()).await?;
        let before = rows::inventory(conn, target.kind(), target.id())
            .await?
            .ok_or(LossError::NotFound)?;
        for environment in &before.environments {
            orch_db::queries::environments::hosting::lock_environment(conn, environment.id).await?;
        }
        for machine in &before.machines {
            orch_db::queries::resources::reenrollment::lock(conn, *machine).await?;
        }
        let inventory = rows::inventory(conn, target.kind(), target.id())
            .await?
            .ok_or(LossError::NotFound)?;
        if inventory.online {
            return Err(LossError::Online);
        }
        if inventory.fingerprint()? != stored.fingerprint {
            return Err(LossError::Stale);
        }
        let now = self.inner.clock.now();
        let mut audience = crate::notifications::operation_audience(
            conn,
            principal,
            &inventory
                .environments
                .iter()
                .map(|environment| environment.id)
                .collect::<Vec<_>>(),
            &inventory.providers,
        )
        .await?;
        enrich_audience(conn, principal, target, &inventory, &mut audience).await?;
        if !orch_db::queries::notifications::operations::matches_preview(
            conn,
            body.preflight_id,
            &audience
                .iter()
                .map(|person| person.user)
                .collect::<Vec<_>>(),
        )
        .await?
        {
            return Err(LossError::Stale);
        }
        let operation = crate::notifications::NotificationOperation {
            id: body.preflight_id,
            kind: "lost".into(),
            name: inventory.name.clone(),
            notify: body.notify,
            path: format!("/resources/{}s/{}", target.kind(), target.id()),
            subject: Some((target.kind().into(), target.id())),
            recipients: audience,
        };
        let preview_frame = match &stored.preview_origin {
            Some(origin) => Some(
                preview_origin::prepare(conn, self.inner.clock.as_ref(), origin, &row, target)
                    .await?,
            ),
            None => None,
        };
        let named = match &stored.named {
            Some(named) => named.clone(),
            None => {
                shown(conn, principal, target, &inventory, now, None)
                    .await?
                    .1
            }
        };
        let affected = declaration_objects(conn, target, &inventory).await?;
        let preview = Arc::new(PreparedLossPreview {
            target,
            plan: stored.plan,
            fingerprint: stored.fingerprint,
            inventory,
            audience: Vec::new(),
            creator: row.created_by,
            created_at: row.created_at,
            named,
        });
        Ok(PreparedLossDeclaration {
            preview,
            affected,
            operation,
            now,
            preview_frame,
        })
    }

    /// Apply only the prepared native loss cascade, starting with its existing operation insert.
    pub async fn apply_loss_declaration(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        prepared: &PreparedLossDeclaration,
    ) -> Result<(), LossError> {
        if principal.user() != prepared.preview.creator {
            return Err(LossError::NotFound);
        }
        prepared.check_deadline(self.inner.clock.as_ref())?;
        let inventory = &prepared.preview.inventory;
        let target = prepared.preview.target;
        let now = prepared.now;
        let operation = &prepared.operation;
        let stored = &prepared.preview;
        orch_db::queries::notifications::operations::save(
            conn,
            operation.id,
            &serde_json::to_value(operation).map_err(EngineError::from)?,
            now,
        )
        .await?;
        crate::operation_access::accept(
            conn,
            principal,
            &stored.plan.admin_grants,
            &orch_db::audit::Origin::default(),
        )
        .await
        .map_err(|error| match error {
            crate::updates::UpdateError::Denied => LossError::Denied,
            other => LossError::Internal(EngineError::unreadable(other)),
        })?;
        rows::close(conn, inventory, principal.user(), now).await?;
        for job in &inventory.jobs {
            crate::recovery::release_history_pins(conn, *job, now).await?;
        }
        let mut unit = crate::unit::Unit::new(conn, now);
        for id in inventory.threads.iter().chain(&inventory.provider_threads) {
            for approval in orch_db::queries::approvals::pending_of_thread(unit.conn, *id).await? {
                if orch_db::queries::approvals::cancel_pending(unit.conn, approval.id, now).await? {
                    orch_db::queries::notifications::inbox::settle_approval(
                        unit.conn,
                        approval.id,
                        now,
                    )
                    .await?;
                }
            }
            let seq = unit.stamp(*id).await?;
            // A lost source can never confirm delivery. Keep attempted words visibly failed for their author.
            for input in orch_db::queries::threads::delivering_inputs(unit.conn, *id).await? {
                orch_db::queries::threads::change_input(
                    unit.conn,
                    input.id,
                    &["delivering"],
                    &orch_db::queries::threads::InputChange {
                        state: "failed",
                        detail: Some(if inventory.threads.contains(id) {
                            "environment_lost_unconfirmed"
                        } else {
                            "provider_runtime_lost_unconfirmed"
                        }),
                        ..Default::default()
                    },
                    seq,
                )
                .await?;
                crate::ingest::settle_user_message(&mut unit, *id, input.id, "failed").await?;
            }
            sqlx::query("UPDATE turns SET seq_updated=$2 WHERE thread_id=$1 AND state='interrupted' AND error->>'code' IN ('environment_lost','provider_runtime_lost') AND completed_at=$3").bind(id).bind(seq).bind(now).execute(&mut *unit.conn).await.map_err(DbError::from)?;
            sqlx::query("UPDATE items SET status='cancelled',updated_at=$3,seq_updated=$2 WHERE thread_id=$1 AND status='in_progress' AND kind<>'user_message' AND turn_id IN(SELECT id FROM turns WHERE thread_id=$1 AND state='interrupted' AND error->>'code' IN ('environment_lost','provider_runtime_lost') AND completed_at=$3)").bind(id).bind(seq).bind(now).execute(&mut *unit.conn).await.map_err(DbError::from)?;

            sqlx::query("UPDATE threads SET state='blocked',blocked_reason=$3,blocked_detail=$4,starting_step=NULL,current_turn_id=NULL,seq_state=GREATEST(seq_state,$2) WHERE node_id=$1")
                .bind(id).bind(seq).bind(if inventory.threads.contains(id){"environment_unavailable"}else{"provider_unavailable"}).bind(if inventory.threads.contains(id){"Its environment was declared lost"}else{"Its provider runtime was declared lost"}).execute(&mut *unit.conn).await.map_err(DbError::from)?;
            unit.wake(*id);
        }
        unit.finish().await?;
        crate::notifications::notify_operation(conn, operation, "done", now).await?;
        let mut entry = orch_db::audit::Entry::new(
            orch_db::audit::Actor::User(principal.user()),
            match target {
                LossTarget::Host { .. } => orch_db::audit::name!("host.lost"),
                LossTarget::Environment { .. } => orch_db::audit::name!("environment.lost"),
                LossTarget::Machine { .. } => orch_db::audit::name!("machine.lost"),
            },
            target.kind(),
            Some(target.id()),
            "Declared permanently lost",
        );
        entry.history = true;
        entry.r#override = !stored.plan.admin_grants.is_empty();
        entry.details = json!({"environments":inventory.environments.iter().map(|env|env.id).collect::<Vec<_>>(),"jobs":inventory.jobs,"kept_bytes":stored.plan.kept_bytes});
        orch_db::audit::append(conn, &entry, &orch_db::audit::Origin::default(), now).await?;
        orch_authz::epoch::bump(conn)
            .await
            .map_err(EngineError::from)?;
        jobs::forks::forget_preflight(conn, prepared.preview.plan.preflight_id).await?;
        prepared.check_deadline(self.inner.clock.as_ref())?;
        Ok(())
    }
}

async fn enrich_audience(
    conn: &mut Conn,
    principal: &Principal,
    target: LossTarget,
    inventory: &rows::Inventory,
    audience: &mut Vec<crate::notifications::OperationRecipient>,
) -> Result<(), LossError> {
    let mut resources = inventory
        .mounts
        .iter()
        .map(|id| {
            (
                ObjectRef {
                    kind: ObjectKind::Mount,
                    id: *id,
                },
                Permission::MountManage,
            )
        })
        .collect::<Vec<_>>();
    match target {
        LossTarget::Host { id } => resources.push((
            ObjectRef {
                kind: ObjectKind::Host,
                id,
            },
            Permission::HostManage,
        )),
        LossTarget::Machine { id } => resources.push((
            ObjectRef {
                kind: ObjectKind::Machine,
                id,
            },
            Permission::MachineManage,
        )),
        LossTarget::Environment { .. } => {}
    }
    let servers:Vec<Id>=sqlx::query_scalar("SELECT DISTINCT server_id FROM mcp_server_offerings WHERE machine_id=ANY($1) AND state<>'lost'").bind(&inventory.machines).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    resources.extend(servers.into_iter().map(|id| {
        (
            ObjectRef {
                kind: ObjectKind::McpServer,
                id,
            },
            Permission::McpManage,
        )
    }));
    crate::notifications::extend_operation_audience(conn, principal, audience, &resources).await?;
    Ok(())
}
