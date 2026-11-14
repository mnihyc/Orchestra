//! Applying membership and home resource endings under their accepted authority, without deleting personal machine files.

use super::Cancelled;
use crate::commands::Outgoing;
use crate::{Engine, EngineError};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::{Decision, ObjectRef, Refusal};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_authz::ownership::{self, Disposal, FollowUp};
use orch_authz::{Act, Policy, Principal};
use orch_db::queries::{hierarchy, resources, threads};
use orch_db::{Conn, DbError};
use orch_link_contract::commands::{
    EnvDetach, McpForget, NodeRuntimeStop, ProviderStop, SerialKey,
};
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::collections::{HashMap, HashSet};

/// Why a membership ending must refuse before committing any of its changes.
#[derive(Debug, thiserror::Error)]
pub enum EndError {
    /// The member disappeared.
    #[error("the person is no member of this tree")]
    NotFound,
    /// The current caller no longer holds the ending's authority.
    #[error("the caller may not end this membership")]
    Denied(Box<Refusal>),
    /// A choice is outside the reviewed managed, sole-owned resource set.
    #[error("{0}")]
    Choice(String),
    /// Removal would strand a resource's existing users or owned units.
    #[error("the resource {0} is still in use")]
    InUse(Id),
    /// Ownership cannot be left without a surviving owner.
    #[error(transparent)]
    Ownership(#[from] ownership::EndError),
    /// A dependency refused.
    #[error(transparent)]
    Internal(#[from] EngineError),
}
impl From<DbError> for EndError {
    fn from(error: DbError) -> Self {
        Self::Internal(error.into())
    }
}
impl From<orch_authz::Error> for EndError {
    fn from(error: orch_authz::Error) -> Self {
        Self::Internal(error.into())
    }
}

/// The facts frozen by the accepted ending, for its attributed audit row.
pub struct Leaving {
    /// The time after its row locks and current-authority check.
    pub at: Time,
    /// The ownership layer's completed plan.
    pub ownership: ownership::Ended,
    /// Pending work cancelled in this tree.
    pub cancelled: Cancelled,
}

/// The resource and reviewed withdrawal choice applied together by a membership or home ending.
struct ResourceEnding {
    object: ObjectRef,
    disposal: Disposal,
    registration: bool,
}

/// What the native deletion kept about a managed resource; no response DTO is a restoration source.
#[derive(serde::Serialize, serde::Deserialize)]
struct ResourceBefore {
    kind: String,
    id: Id,
    disabled: bool,
}

/// Corroborated native restoration rows, frozen before their first effect.
pub struct PreparedHomeRestoration {
    node: Id,
    resources: Vec<resources::AccessFacts>,
}
impl PreparedHomeRestoration {
    #[must_use]
    pub fn resources(&self) -> &[resources::AccessFacts] {
        &self.resources
    }
}

/// The native ending's immutable ownership/registration operands, never a deserialized plan.
pub struct PreparedMemberEnding {
    root: Id,
    user: Id,
    domain: Id,
    ownership: ownership::PreparedEnding,
    registrations: Vec<resources::AccessFacts>,
    member_nodes: Vec<Id>,
    destinations: Vec<Id>,
}
impl PreparedMemberEnding {
    #[must_use]
    pub fn domain(&self) -> Id {
        self.domain
    }
    #[must_use]
    pub fn plan(&self) -> &ownership::Plan {
        self.ownership.plan()
    }
    #[must_use]
    pub fn registrations(&self) -> &[resources::AccessFacts] {
        &self.registrations
    }
    #[must_use]
    pub fn member_nodes(&self) -> &[Id] {
        &self.member_nodes
    }
    #[must_use]
    pub fn destinations(&self) -> &[Id] {
        &self.destinations
    }
}
/// Preserve the exact entry owner's first-effect failure alongside native membership errors.
pub type EndFailure<E> = orch_authz::pages::EffectFailure<EndError, E>;

impl Engine {
    /// The original caller uses the same native prepared ending and opaque writer.
    pub async fn end_member_resources_in(
        &self,
        conn: &mut Conn,
        actor: &Principal,
        root: Id,
        user: Id,
        choices: &HashMap<Id, Disposal>,
        origin: &orch_db::audit::Origin,
    ) -> Result<Leaving, EndError> {
        let prepared = self
            .prepare_member_ending_in(conn, actor, root, user, choices)
            .await?;
        match self
            .end_prepared_member_in(conn, actor, &prepared, choices, origin, || {
                Ok::<(), std::convert::Infallible>(())
            })
            .await
        {
            Ok(ended) => Ok(ended),
            Err(EndFailure::Native(error)) => Err(error),
            Err(EndFailure::BeforeEffect(never)) => match never {},
        }
    }

    /// Holds the existing native membership, thread, page and destination footprint before effects.
    pub async fn prepare_member_ending_in(
        &self,
        conn: &mut Conn,
        actor: &Principal,
        root: Id,
        user: Id,
        choices: &HashMap<Id, Disposal>,
    ) -> Result<PreparedMemberEnding, EndError> {
        let domain: Option<Id> = sqlx::query_scalar(
            "SELECT domain_id FROM org_members WHERE org_id=$1 AND user_id=$2 FOR UPDATE",
        )
        .bind(root)
        .bind(user)
        .fetch_optional(&mut *conn)
        .await
        .map_err(DbError::from)?;
        let domain = domain.ok_or(EndError::NotFound)?;
        let owned = resources::owned_by(conn, root, user, actor.now().get()).await?;
        let registrations = resources::registrations_of_member(conn, root, user).await?;
        let member_nodes = orch_db::queries::access::owned_nodes(conn, root, user).await?;
        let mut owned = owned;
        let mut registrations = registrations;
        let mut member_nodes = member_nodes;
        loop {
            let owned_ids: Vec<_> = owned
                .iter()
                .map(|row| row.object_id)
                .chain(registrations.iter().map(|row| row.id))
                .collect();
            // Thread rows precede node/resource rows, as they do in ordinary lifecycle settlement.
            let affected:Vec<Id>=sqlx::query_scalar("SELECT t.node_id FROM threads t JOIN nodes n ON n.id=t.node_id
            WHERE n.org_id=$1 OR t.environment_id=ANY($2) OR t.provider_id=ANY($2) OR t.agent_id=ANY($2) ORDER BY t.node_id")
            .bind(root).bind(&owned_ids).fetch_all(&mut *conn).await.map_err(DbError::from)?;
            threads::tools::lock_threads(conn, &affected).await?;
            let mut anchors = vec![root, domain];
            anchors.extend(owned.iter().filter_map(|row| row.anchor));
            anchors.extend(member_nodes.iter().map(|row| row.id));
            let node_rows = hierarchy::with_ancestors(conn, &anchors).await?;
            let ids = node_rows
                .into_iter()
                .map(|row| row.id)
                .collect::<std::collections::BTreeSet<_>>();
            for id in ids {
                hierarchy::lock(conn, id).await?;
            }
            let machines = registrations
                .iter()
                .filter_map(|row| row.machine_id)
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            sqlx::query("SELECT id FROM machines WHERE id=ANY($1) ORDER BY id FOR UPDATE")
                .bind(&machines)
                .fetch_all(&mut *conn)
                .await
                .map_err(DbError::from)?;
            let mut pages = std::collections::BTreeMap::new();
            for row in &owned {
                let kind: ObjectKind = if row.object_kind == "node" {
                    row.thing_kind.parse::<ObjectKind>()
                } else {
                    row.object_kind.parse::<ObjectKind>()
                }
                .map_err(|error| EngineError::unreadable(error.to_string()))?;
                pages.insert(
                    (row.object_id, kind),
                    ObjectRef {
                        kind,
                        id: row.object_id,
                    },
                );
            }
            for row in &registrations {
                let kind = row
                    .kind
                    .parse::<ObjectKind>()
                    .map_err(|error| EngineError::unreadable(error.to_string()))?;
                pages.insert((row.id, kind), ObjectRef { kind, id: row.id });
            }
            for object in pages.into_values() {
                let target = if object.kind.node().is_some() {
                    orch_authz::pages::Target::Live(object.id)
                } else {
                    orch_authz::pages::Target::Resource(object)
                };
                orch_authz::pages::lock_change_target(conn, target).await?;
            }
            let fresh_owned = resources::owned_by(conn, root, user, actor.now().get()).await?;
            let fresh_registrations = resources::registrations_of_member(conn, root, user).await?;
            let fresh_nodes = orch_db::queries::access::owned_nodes(conn, root, user).await?;
            let same_owned = owned
                .iter()
                .map(|row| (&row.object_kind, row.object_id))
                .eq(fresh_owned
                    .iter()
                    .map(|row| (&row.object_kind, row.object_id)));
            let same_registrations = registrations
                .iter()
                .map(|row| (&row.kind, row.id))
                .eq(fresh_registrations.iter().map(|row| (&row.kind, row.id)));
            let same_nodes = member_nodes
                .iter()
                .map(|row| row.id)
                .eq(fresh_nodes.iter().map(|row| row.id));
            owned = fresh_owned;
            registrations = fresh_registrations;
            member_nodes = fresh_nodes;
            if same_owned && same_registrations && same_nodes {
                break;
            }
        }
        let at = Time::new(self.inner.clock.now());
        let actor = Principal::load(conn, actor.caller().clone(), at).await?;
        if actor.user() != user {
            let ancestors = hierarchy::with_ancestors(conn, &[domain]).await?;
            let path = ancestors
                .iter()
                .find(|node| node.id == domain)
                .ok_or(EndError::NotFound)?
                .path
                .clone();
            let mut refused = None;
            let mut allowed = false;
            for id in path.into_iter().rev().filter(|id| {
                ancestors
                    .iter()
                    .any(|node| node.id == *id && node.kind == "org")
            }) {
                match orch_authz::nodes::check(
                    conn,
                    &actor,
                    id,
                    Permission::MembersManage,
                    Act::Use,
                    Policy::default(),
                )
                .await?
                {
                    Decision::Allowed { .. } => {
                        allowed = true;
                        break;
                    }
                    Decision::Refused(refusal) => {
                        refused.get_or_insert(refusal);
                    }
                }
            }
            if !allowed {
                return Err(refused.map_or(EndError::NotFound, |refusal| {
                    EndError::Denied(Box::new(refusal))
                }));
            }
        }
        let ownership =
            ownership::prepare_ending_rows(conn, &actor, root, user, Policy::default(), owned)
                .await?;
        let plan = ownership.plan();
        for (id, choice) in choices {
            let Some(passing) = plan.things.iter().find(|passing| {
                passing.object.id == *id
                    && passing.may_be_disposed_of
                    && matches!(passing.fate, ownership::Fate::PassedOn { .. })
            }) else {
                return Err(EndError::Choice(
                    "resource choices name only managed resources this member owns alone"
                        .to_owned(),
                ));
            };
            if *choice == Disposal::Remove {
                validate_remove(conn, passing.object).await?;
            }
        }
        let mut destinations = plan
            .things
            .iter()
            .filter_map(|passing| match passing.fate {
                ownership::Fate::PassedOn { org, .. } => Some(org),
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>();
        destinations.extend(
            member_nodes
                .iter()
                .filter(|row| row.deleted && row.alone)
                .filter_map(|row| row.lies_in),
        );
        Ok(PreparedMemberEnding {
            root,
            user,
            domain,
            ownership,
            registrations,
            member_nodes: member_nodes.into_iter().map(|row| row.id).collect(),
            destinations: destinations.into_iter().collect(),
        })
    }

    /// Applies exactly the native prepared ending; the callback reaches its first ownership write.
    pub async fn end_prepared_member_in<E: Send, F: FnOnce() -> Result<(), E> + Send>(
        &self,
        conn: &mut Conn,
        actor: &Principal,
        prepared: &PreparedMemberEnding,
        choices: &HashMap<Id, Disposal>,
        origin: &orch_db::audit::Origin,
        before_effect: F,
    ) -> Result<Leaving, EndFailure<E>> {
        let ownership = ownership::end_prepared_membership_fenced(
            conn,
            actor,
            &prepared.ownership,
            choices,
            origin,
            before_effect,
        )
        .await
        .map_err(|error| match error {
            ownership::EndFailure::Native(error) => EndFailure::Native(EndError::Ownership(error)),
            ownership::EndFailure::BeforeEffect(error) => EndFailure::BeforeEffect(error),
        })?;
        self.finish_member_ending_in(conn, actor, prepared, ownership, origin)
            .await
            .map_err(EndFailure::Native)
    }

    async fn finish_member_ending_in(
        &self,
        conn: &mut Conn,
        actor: &Principal,
        prepared: &PreparedMemberEnding,
        ownership: ownership::Ended,
        origin: &orch_db::audit::Origin,
    ) -> Result<Leaving, EndError> {
        let (root, user, at) = (prepared.root, prepared.user, actor.now());
        let cancelled = self
            .cancel_member_work_in(conn, root, user, actor.user(), at)
            .await?;
        let mut unregistered = HashSet::new();
        for follow_up in &ownership.follow_ups {
            let (object, disposal, registration) = match follow_up {
                FollowUp::Disable(object) => (*object, Disposal::Disable, false),
                FollowUp::Remove(object) => (*object, Disposal::Remove, false),
                FollowUp::Unregister(object) => (*object, Disposal::Remove, true),
            };
            self.end_resource_in(
                conn,
                ResourceEnding {
                    object,
                    disposal,
                    registration,
                },
                actor.user(),
                at,
                origin,
            )
            .await?;
            if registration {
                unregistered.insert(object.id);
            }
        }
        // Registrations can belong to a shared or another person's page: the machine remains
        // its owner's, so every registration from that personal machine is withdrawn here.
        for registration in &prepared.registrations {
            if !unregistered.contains(&registration.id) {
                let kind = registration
                    .kind
                    .parse::<ObjectKind>()
                    .map_err(|error| EngineError::unreadable(error.to_string()))?;
                self.end_resource_in(
                    conn,
                    ResourceEnding {
                        object: ObjectRef {
                            kind,
                            id: registration.id,
                        },
                        disposal: Disposal::Remove,
                        registration: true,
                    },
                    actor.user(),
                    at,
                    origin,
                )
                .await?;
            }
        }
        for passing in &ownership.passed {
            if passing.object.kind.node().is_some() {
                hierarchy::stamp(conn, passing.object.id).await?;
            }
        }
        Ok(Leaving {
            at,
            ownership,
            cancelled,
        })
    }

    /// Disables or rehomes the resources whose own home is deleted, preserving all owned data.
    pub async fn end_home_resources_in(
        &self,
        conn: &mut Conn,
        node: Id,
        to: Option<Id>,
        by: Id,
        at: Time,
        origin: &orch_db::audit::Origin,
    ) -> Result<(Vec<Id>, Json), EndError> {
        let facts = resources::resources_under_home(conn, node).await?;
        self.end_home_resource_facts_in(conn, &facts, to, by, at, origin)
            .await
    }

    /// The deletion writer consumes the same native resource rows its scope preparation held.
    pub async fn end_prepared_home_resources_in(
        &self,
        conn: &mut Conn,
        prepared: &crate::archive::deletion::Prepared,
        to: Option<Id>,
        by: Id,
        at: Time,
        origin: &orch_db::audit::Origin,
    ) -> Result<(Vec<Id>, Json), EndError> {
        self.end_home_resource_facts_in(conn, prepared.resources(), to, by, at, origin)
            .await
    }

    async fn end_home_resource_facts_in(
        &self,
        conn: &mut Conn,
        facts: &[resources::AccessFacts],
        to: Option<Id>,
        by: Id,
        at: Time,
        origin: &orch_db::audit::Origin,
    ) -> Result<(Vec<Id>, Json), EndError> {
        let mut environments = Vec::new();
        let mut kept = Vec::new();
        for fact in facts {
            if fact.kind == "environment" {
                environments.push(fact.id);
            }
            if let Some(to) = to {
                let table = table_of(&fact.kind)
                    .ok_or_else(|| EngineError::unreadable("a resource has an unknown kind"))?;
                let home = if fact.kind == "environment" {
                    "home_node_id"
                } else {
                    "home_id"
                };
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "UPDATE {table} SET {home}=$2,revision=revision+1,updated_at=$3 WHERE id=$1"
                )))
                .bind(fact.id)
                .bind(to)
                .bind(at.get())
                .execute(&mut *conn)
                .await
                .map_err(DbError::from)?;
            } else {
                kept.push(ResourceBefore {
                    kind: fact.kind.clone(),
                    id: fact.id,
                    disabled: fact.disabled,
                });
                let kind = fact
                    .kind
                    .parse::<ObjectKind>()
                    .map_err(|error| EngineError::unreadable(error.to_string()))?;
                self.end_resource_in(
                    conn,
                    ResourceEnding {
                        object: ObjectRef { kind, id: fact.id },
                        disposal: Disposal::Disable,
                        registration: false,
                    },
                    by,
                    at,
                    origin,
                )
                .await?;
            }
        }
        Ok((environments, json!(kept)))
    }

    async fn end_resource_in(
        &self,
        conn: &mut Conn,
        ending: ResourceEnding,
        by: Id,
        at: Time,
        origin: &orch_db::audit::Origin,
    ) -> Result<(), EndError> {
        let ResourceEnding {
            object,
            disposal,
            registration,
        } = ending;
        let Some(fact) = resources::resource_facts(conn, &[object.id])
            .await?
            .into_iter()
            .find(|fact| fact.kind == object.kind.as_str())
        else {
            return Ok(());
        };
        if fact.deleted {
            return Ok(());
        }
        let remove = disposal == Disposal::Remove;
        let affected = resources::ending_threads(conn, object.id).await?;
        if !affected.is_empty() {
            let mut unit = crate::unit::Unit::new(conn, at.get());
            self.settle_in(&mut unit, &affected, Some(by), "disabled", false)
                .await?;
            for id in &affected {
                let seq = unit.stamp(*id).await?;
                let reason = match fact.kind.as_str() {
                    "agent" => "agent_disabled",
                    "provider" => "provider_unavailable",
                    _ => "environment_unavailable",
                };
                threads::set_state(
                    &mut *unit.conn,
                    *id,
                    &["idle", "starting", "running", "waiting_approval", "blocked"],
                    &threads::StateChange::blocked(
                        reason,
                        Some("A resource was withdrawn when its membership or home ended."),
                    ),
                    seq,
                )
                .await?;
            }
            unit.finish().await?;
        }
        cancel_resource_jobs(conn, object.id, by, at).await?;
        match fact.kind.as_str() {
            "environment" => {
                if registration {
                    if let Some(machine) = fact.machine_id {
                        persist(
                            conn,
                            at,
                            machine,
                            SerialKey::Environment(object.id),
                            &EnvDetach {
                                environment_id: object.id,
                            },
                        )
                        .await?;
                    }
                    orch_db::queries::environments::routes::remove_registration(
                        conn,
                        object.id,
                        at.get(),
                    )
                    .await?;
                } else if remove {
                    self.remove_bindings_in(conn, object.id, by, at).await?;
                    self.schedule_removal_in(conn, object.id, at.get(), by, at)
                        .await
                        .map_err(|error| EngineError::unreadable(error.to_string()))?;
                } else {
                    orch_db::queries::environments::routes::set_disabled(
                        conn,
                        object.id,
                        true,
                        at.get(),
                    )
                    .await?;
                    self.stop_resource_units_in(conn, object.id, false, at)
                        .await?;
                }
            }
            "provider" => {
                if let Some(machine) = fact.machine_id {
                    persist(
                        conn,
                        at,
                        machine,
                        SerialKey::Provider(object.id),
                        &ProviderStop {
                            provider_id: object.id,

                            purge: None,
                        },
                    )
                    .await?;
                }
                resources::routes::set_provider_standing(
                    conn,
                    object.id,
                    "disabled",
                    Some("Its membership or home ended."),
                    at.get(),
                )
                .await?;
                if remove {
                    resources::routes::delete_provider(conn, object.id, at.get()).await?;
                    resources::routes::delete_provider_secrets_except(conn, object.id, &[]).await?;
                    if !registration {
                        crate::provider_runtime::retire_runtime(conn, object.id, at.get()).await?;
                    }
                } else {
                    self.stop_resource_units_in(conn, object.id, false, at)
                        .await?;
                }
            }
            "host" => {
                if remove {
                    sqlx::query("UPDATE machines m SET deleted_at=$2,status='revoked',updated_at=$2 FROM hosts h WHERE h.id=$1 AND m.id=h.machine_id")
                        .bind(object.id).bind(at.get()).execute(&mut *conn).await.map_err(DbError::from)?;
                } else {
                    sqlx::query("UPDATE hosts SET disabled=true,revision=revision+1,updated_at=$2 WHERE id=$1 AND NOT disabled")
                        .bind(object.id).bind(at.get()).execute(&mut *conn).await.map_err(DbError::from)?;
                    self.stop_resource_units_in(conn, object.id, true, at)
                        .await?;
                }
            }
            kind => {
                if registration
                    && kind == "mcp_server"
                    && let Some(machine) = fact.machine_id
                {
                    persist(
                        conn,
                        at,
                        machine,
                        SerialKey::Machine,
                        &McpForget {
                            server_id: object.id,
                        },
                    )
                    .await?;
                }
                let table = table_of(kind)
                    .ok_or_else(|| EngineError::unreadable("a resource has an unknown kind"))?;
                let change = if remove {
                    "deleted_at=$2"
                } else if kind == "agent" {
                    "disabled=true"
                } else {
                    "enabled=false"
                };
                sqlx::query(sqlx::AssertSqlSafe(format!("UPDATE {table} SET {change},revision=revision+1,updated_at=$2 WHERE id=$1 AND deleted_at IS NULL")))
                    .bind(object.id).bind(at.get()).execute(&mut *conn).await.map_err(DbError::from)?;
                if remove && kind == "mcp_server" {
                    resources::mcp::delete(conn, object.id, at.get()).await?;
                }
                if remove && kind == "skill" {
                    resources::skills::discard_draft(conn, object.id).await?;
                }
            }
        }
        if registration {
            sqlx::query("UPDATE permission_entries SET removed_at=$2,removed_by=$3,revision=revision+1
                WHERE object_kind=$4 AND object_id=$1 AND removed_at IS NULL AND permission='access.own'")
                .bind(object.id).bind(at.get()).bind(by).bind(&fact.kind).execute(&mut *conn).await.map_err(DbError::from)?;
        }
        let action = action_of(&fact.kind, remove);
        let mut entry = orch_db::audit::Entry::new(
            orch_db::audit::Actor::User(by),
            action.as_str(),
            &fact.kind,
            Some(object.id),
            if registration {
                "Unregistered a resource when its membership ended"
            } else if remove {
                "Removed a resource when its membership ended"
            } else {
                "Disabled a resource when its membership or home ended"
            },
        );
        entry.history = action.is_history_line();
        entry.org_id = fact.org_id;
        entry.environment_id = (fact.kind == "environment").then_some(object.id);
        entry.details = json!({"resource":object.id,"name":fact.name,"registration":registration});
        orch_db::audit::append(conn, &entry, origin, at.get()).await?;
        Ok(())
    }

    async fn stop_resource_units_in(
        &self,
        conn: &mut Conn,
        resource: Id,
        host: bool,
        at: Time,
    ) -> Result<(), EngineError> {
        let runtimes:Vec<(Id,Id)>=sqlx::query_as("SELECT r.id,h.machine_id FROM runtimes r JOIN hosts h ON h.id=r.host_id
            WHERE r.successor_of IS NULL AND r.state NOT IN ('stopped','retired','removed')
                AND (($2 AND r.host_id=$1) OR (NOT $2 AND (r.environment_id=$1 OR r.provider_id=$1))) ORDER BY r.id")
            .bind(resource).bind(host).fetch_all(&mut *conn).await.map_err(DbError::from)?;
        for (runtime, machine) in runtimes {
            let pending: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM commands WHERE kind='node.runtime.stop'
                AND body->>'runtime_id'=$1::uuid::text AND state IN ('queued','sent','accepted'))",
            )
            .bind(runtime)
            .fetch_one(&mut *conn)
            .await
            .map_err(DbError::from)?;
            if pending {
                continue;
            }
            let body: NodeRuntimeStop =
                serde_json::from_value(json!({"runtime_id":runtime,"reason":"stop"}))?;
            persist(conn, at, machine, SerialKey::Runtime(runtime), &body).await?;
            sqlx::query("UPDATE runtimes SET state='stopping',state_detail='Its resource was disabled',updated_at=$2 WHERE id=$1")
                .bind(runtime).bind(at.get()).execute(&mut *conn).await.map_err(DbError::from)?;
        }
        Ok(())
    }

    /// Keeps a committed disabled-unit intention going through an offline node or a failed stop.
    pub(crate) async fn reconcile_disabled_units(&self) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let due:Vec<(Id,bool)>=sqlx::query_as("SELECT CASE WHEN h.disabled THEN h.id ELSE COALESCE(r.environment_id,r.provider_id) END,h.disabled
            FROM runtimes r JOIN hosts h ON h.id=r.host_id JOIN machines m ON m.id=h.machine_id
            LEFT JOIN environments e ON e.id=r.environment_id LEFT JOIN providers p ON p.id=r.provider_id
            WHERE r.successor_of IS NULL AND r.state NOT IN ('stopped','retired','removed') AND m.online
                AND (h.disabled OR e.status='disabled' OR p.status='disabled')
                AND NOT EXISTS(SELECT 1 FROM commands c WHERE c.kind='node.runtime.stop' AND c.body->>'runtime_id'=r.id::text
                    AND (c.state IN ('queued','sent','accepted') OR c.updated_at>$1))
            ORDER BY r.updated_at,r.id LIMIT $2")
            .bind(self.inner.clock.now()-chrono::Duration::seconds(30)).bind(self.inner.config.sweep_batch)
            .fetch_all(&mut *tx).await.map_err(DbError::from)?;
        let mut seen = HashSet::new();
        for (resource, host) in due {
            if !seen.insert((resource, host)) {
                continue;
            }
            self.stop_resource_units_in(&mut tx, resource, host, Time::new(self.inner.clock.now()))
                .await?;
        }
        tx.commit().await.map_err(DbError::from)?;
        Ok(())
    }

    pub(crate) async fn prepare_disabled_unit_stop(
        &self,
        command: &orch_db::queries::link::CommandRow,
    ) -> Result<bool, EngineError> {
        let Some(runtime) = command
            .body
            .get("runtime_id")
            .and_then(Json::as_str)
            .and_then(|id| id.parse::<Id>().ok())
        else {
            return Ok(false);
        };
        let (row, online) = {
            let mut conn = self.inner.db.acquire().await?;
            let row = orch_db::queries::environments::hosting::runtime(&mut conn, runtime).await?;
            let online = match &row {
                Some(row) => resources::hosting::host(&mut conn, row.host_id)
                    .await?
                    .is_some_and(|host| host.online),
                None => false,
            };
            (row, online)
        };
        let Some(row) = row.filter(|_| online) else {
            return Ok(false);
        };
        // Stopping an old kept unit must not return a successor's credential.
        if row.state == "retired" {
            return Ok(true);
        }
        let result = if let Some(environment) = row.environment_id {
            self.return_credentials_of(environment)
                .await
                .map_err(|error| error.to_string())
        } else if let Some(provider) = row.provider_id {
            match self
                .return_credential(provider, crate::credentials::handover::ReturnWhy::Engine)
                .await
            {
                Ok(()) | Err(crate::credentials::handover::ReturnError::NotHeld) => Ok(()),
                Err(error) => Err(error.to_string()),
            }
        } else {
            Ok(())
        };
        if let Err(reason) = result {
            let mut conn = self.inner.db.acquire().await?;
            sqlx::query("UPDATE runtimes SET state_detail=$2 WHERE id=$1 AND state NOT IN ('stopped','removed','retired')")
                .bind(runtime).bind(format!("Disabled; its credential must be returned before stopping: {reason}"))
                .execute(&mut *conn).await.map_err(DbError::from)?;
            return Ok(false);
        }
        Ok(true)
    }

    pub(crate) async fn finish_disabled_unit_stop(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::CommandRow,
        outcome: &crate::commands::Outcome,
    ) -> Result<(), EngineError> {
        if command.kind != "node.runtime.stop" {
            return Ok(());
        }
        let Some(runtime) = command
            .body
            .get("runtime_id")
            .and_then(Json::as_str)
            .and_then(|id| id.parse::<Id>().ok())
        else {
            return Ok(());
        };
        // A host that has no such unit has nothing left to stop: the unit is stopped.
        let stopped = match outcome {
            crate::commands::Outcome::Succeeded(_) => true,
            crate::commands::Outcome::Failed(error) => {
                error.code == orch_link_contract::commands::ErrorCode::NotFound.as_str()
            }
            _ => false,
        };
        if stopped {
            sqlx::query("UPDATE runtimes r SET state='stopped',state_detail=NULL,updated_at=$2
                WHERE r.id=$1 AND r.state NOT IN ('retired','removed') AND (r.state='stopping' OR EXISTS(SELECT 1 FROM hosts h
                    LEFT JOIN environments e ON e.id=r.environment_id LEFT JOIN providers p ON p.id=r.provider_id
                    WHERE h.id=r.host_id AND (h.disabled OR e.status='disabled' OR p.status='disabled')))")
                .bind(runtime).bind(self.inner.clock.now()).execute(&mut *conn).await.map_err(DbError::from)?;
            // The host's word that it has no such unit is written down: the next start makes a unit.
            if matches!(
                outcome,
                crate::commands::Outcome::Failed(error)
                    if error.code == orch_link_contract::commands::ErrorCode::NotFound.as_str()
            ) {
                orch_db::queries::environments::hosting::set_unit_missing(
                    &mut *conn, runtime, true,
                )
                .await?;
            }
        } else {
            let why = match outcome {
                crate::commands::Outcome::Failed(error) => {
                    format!(
                        "Disabled; the host has not confirmed its stop: {}",
                        error.detail
                    )
                }
                crate::commands::Outcome::Expired => {
                    "Disabled; the host did not take the command to stop it in time".to_owned()
                }
                _ => "Disabled; the host has not confirmed its stop".to_owned(),
            };
            tracing::warn!(%runtime, %why, "the host did not confirm the stop of a unit of a resource that was disabled");
            sqlx::query(
                "UPDATE runtimes SET state_detail=$3,updated_at=$2
                WHERE id=$1 AND state NOT IN ('stopped','retired','removed')",
            )
            .bind(runtime)
            .bind(self.inner.clock.now())
            .bind(why)
            .execute(&mut *conn)
            .await
            .map_err(DbError::from)?;
        }
        Ok(())
    }

    /// Selects the native saved resources against their actual current kind/home and prospective live nodes.
    pub async fn prepare_home_restoration_in(
        &self,
        conn: &mut Conn,
        node: Id,
        deletion: Option<Id>,
        restored: &[Id],
    ) -> Result<PreparedHomeRestoration, EngineError> {
        let Some(deletion) = deletion else {
            return Ok(PreparedHomeRestoration {
                node,
                resources: Vec::new(),
            });
        };
        let saved: Option<Json> = sqlx::query_scalar("SELECT details->'resources_before' FROM audit_log WHERE action='node.deleted' AND object_id=$1 AND details->>'deletion_id'=$2::uuid::text ORDER BY at DESC,id DESC LIMIT 1")
            .bind(node).bind(deletion).fetch_optional(&mut *conn).await.map_err(DbError::from)?.flatten();
        let before = saved
            .as_ref()
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(|one| serde_json::from_value::<ResourceBefore>(one.clone()).ok())
            .filter(|one| !one.disabled && table_of(&one.kind).is_some())
            .collect::<Vec<_>>();
        let ids = before.iter().map(|one| one.id).collect::<Vec<_>>();
        let mut facts = resources::resource_facts(conn, &ids).await?;
        facts.retain(|fact| {
            before
                .iter()
                .any(|one| one.id == fact.id && one.kind == fact.kind)
        });
        facts.sort_by_key(|fact| (fact.id, fact.kind.clone()));
        for fact in &facts {
            resources::lock_resource(conn, &fact.kind, fact.id).await?;
        }
        // Reload only actual saved identities after their row locks, preserving native prospective liveness.
        let mut facts = resources::resource_facts(conn, &ids).await?;
        facts.retain(|fact| {
            before
                .iter()
                .any(|one| one.id == fact.id && one.kind == fact.kind)
        });
        let homes = facts
            .iter()
            .filter_map(|fact| fact.home_id)
            .collect::<Vec<_>>();
        let homes = hierarchy::with_ancestors(conn, &homes).await?;
        facts.retain(|fact| {
            fact.home_id.is_some_and(|home| {
                homes.iter().any(|row| {
                    row.id == home
                        && row.path.contains(&node)
                        && (row.deleted_at.is_none() || restored.contains(&home))
                })
            })
        });
        Ok(PreparedHomeRestoration {
            node,
            resources: facts,
        })
    }

    /// Enables only the exact native saved members this restore prepared, keeping units stopped.
    pub async fn restore_prepared_home_resources_in(
        &self,
        conn: &mut Conn,
        prepared: &PreparedHomeRestoration,
        at: Time,
    ) -> Result<(), EngineError> {
        for fact in &prepared.resources {
            let table = table_of(&fact.kind).ok_or_else(|| {
                EngineError::unreadable("a saved restoration resource has no table")
            })?;
            let home = if fact.kind == "environment" {
                "home_node_id"
            } else {
                "home_id"
            };
            let change = match fact.kind.as_str() {
                "provider" => {
                    "status=CASE WHEN status='disabled' THEN 'needs_setup' ELSE status END"
                }
                "environment" => {
                    "status=CASE WHEN status='disabled' THEN 'stopped' ELSE status END"
                }
                "host" | "agent" => "disabled=false",
                _ => "enabled=true",
            };
            sqlx::query(sqlx::AssertSqlSafe(format!("UPDATE {table} x SET {change},revision=revision+1,updated_at=$3 WHERE x.id=$1 AND EXISTS(SELECT 1 FROM nodes n WHERE n.id=x.{home} AND $2=ANY(n.path) AND n.deleted_at IS NULL)")))
                .bind(fact.id).bind(prepared.node).bind(at.get()).execute(&mut *conn).await.map_err(DbError::from)?;
        }
        Ok(())
    }

    /// The original native caller consumes the same actual saved-resource preparation.
    pub async fn restore_home_resources_in(
        &self,
        conn: &mut Conn,
        node: Id,
        deletion: Option<Id>,
        at: Time,
    ) -> Result<(), EngineError> {
        let prepared = self
            .prepare_home_restoration_in(conn, node, deletion, &[])
            .await?;
        self.restore_prepared_home_resources_in(conn, &prepared, at)
            .await
    }
}

async fn persist<B: orch_link_contract::commands::CommandBody>(
    conn: &mut Conn,
    at: Time,
    machine: Id,
    key: SerialKey,
    body: &B,
) -> Result<(), EngineError> {
    crate::archive::persist(
        conn,
        at.get(),
        Outgoing::new(orch_types::new_id(), machine, key, body)?,
    )
    .await
}

async fn cancel_resource_jobs(
    conn: &mut Conn,
    resource: Id,
    by: Id,
    at: Time,
) -> Result<(), EngineError> {
    let jobs: Vec<Id> = sqlx::query_scalar(
        "SELECT id FROM jobs WHERE state IN ('queued','running')
        AND (environment_id=$1 OR provider_id=$1 OR plan->>'host'=$1::uuid::text) ORDER BY id",
    )
    .bind(resource)
    .fetch_all(&mut *conn)
    .await
    .map_err(DbError::from)?;
    for job in jobs {
        orch_db::queries::jobs::request_cancel(conn, job, by, at.get()).await?;
    }
    Ok(())
}

async fn validate_remove(conn: &mut Conn, object: ObjectRef) -> Result<(), EndError> {
    let used = match object.kind.as_str() {
        "provider" => {
            resources::routes::provider_use(conn, object.id)
                .await?
                .threads
                > 0
                || resources::provider_runtime::state(conn, object.id)
                    .await?
                    .is_some_and(|state| state.held_by_environment_id.is_some())
        }
        "agent" => {
            let used = resources::routes::agent_use(conn, object.id).await?;
            used.threads > 0 || used.providers > 0
        }
        "host" => !orch_db::queries::environments::hosting::runtimes_on(conn, object.id, None)
            .await?
            .is_empty(),
        "template" => resources::routes::templates(conn, &[object.id])
            .await?
            .first()
            .is_some_and(|template| template.units > 0),
        "mount" => !resources::mounts::candidates(conn, object.id)
            .await?
            .is_empty(),
        _ => false,
    };
    if used {
        Err(EndError::InUse(object.id))
    } else {
        Ok(())
    }
}

fn table_of(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "provider" => "providers",
        "agent" => "agents",
        "environment" => "environments",
        "host" => "hosts",
        "template" => "env_templates",
        "mcp_server" => "mcp_servers",
        "mount" => "mounts",
        "skill" => "skills",
        _ => return None,
    })
}

fn action_of(kind: &str, remove: bool) -> AuditAction {
    match (kind, remove) {
        ("provider", true) => AuditAction::ProviderDeleted,
        ("provider", false) => AuditAction::ProviderDisabled,
        ("environment", true) => AuditAction::EnvironmentDeleted,
        ("environment", false) => AuditAction::EnvironmentDisabled,
        ("agent", true) => AuditAction::AgentDeleted,
        ("agent", false) => AuditAction::AgentDisabled,
        ("template", true) => AuditAction::TemplateDeleted,
        ("template", false) => AuditAction::TemplateDisabled,
        ("mcp_server", true) => AuditAction::McpDeleted,
        ("mcp_server", false) => orch_db::audit::action!(AuditAction::McpDisabled),
        ("mount", true) => AuditAction::MountDeleted,
        ("mount", false) => AuditAction::MountUpdated,
        ("host", true) => AuditAction::HostDeleted,
        ("skill", true) => AuditAction::SkillDeleted,
        ("skill", false) => AuditAction::SkillDisabled,
        _ => AuditAction::HostUpdated,
    }
}
