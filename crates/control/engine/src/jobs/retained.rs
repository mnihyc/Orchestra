//! Restore and purge jobs reserve the same frozen files and deletion revision.

use super::runner::{Done, Failure, Job, MakeError, Making, Phase, Phased, Stop, Undone, make};
use crate::{Engine, commands::Outgoing, error::EngineError, ids};
use async_trait::async_trait;
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::{
    environments::{hosting, retained},
    jobs,
    resources::hosting as hosts,
};
use orch_db::{Conn, DbError};
use orch_link_contract::commands::{EnvDetach, NodeRuntimeRemove, NodeRuntimeStop, SerialKey};
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;

/// Restore spends most of its progress creating and checking the replacement.
const RESTORE_PHASES: [Phase; 6] = [
    Phase {
        name: "kept",
        weight: 10,
    },
    Phase {
        name: "create",
        weight: 60,
    },
    Phase {
        name: "after_up",
        weight: 5,
    },
    Phase {
        name: "verify",
        weight: 20,
    },
    Phase {
        name: "stop",
        weight: 5,
    },
    Phase {
        name: "commit",
        weight: 5,
    },
];
/// Purge spends most of its progress asking the host to remove the frozen files.
const PURGE_PHASES: [Phase; 3] = [
    Phase {
        name: "authority",
        weight: 10,
    },
    Phase {
        name: "volumes",
        weight: 85,
    },
    Phase {
        name: "commit",
        weight: 5,
    },
];

fn failed(code: &str, why: impl Into<String>) -> Stop {
    Stop::Failed(Failure::new(code, why))
}

/// Claims files for a restore or a purge in the caller's guarded transaction.
pub async fn queue(
    conn: &mut Conn,
    environment: Id,
    revision: i64,
    purge: bool,
    by: Option<Id>,
    identity: Json,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<jobs::JobRow, MakeError> {
    let promise = retained::removal(conn, environment)
        .await?
        .ok_or(MakeError::NoEnvironment)?;
    let retry = purge && promise.removal_phase.as_deref() == Some("purging");
    if promise.removal_revision != revision
        || (!retry && promise.removal_phase.as_deref() != Some("kept"))
    {
        return Err(MakeError::Db(DbError::Rule {
            constraint: "environment_removal_changed".into(),
        }));
    }
    let kind = if purge {
        jobs::retained::PURGE
    } else {
        jobs::retained::RESTORE
    };
    let mut origin = audit::current_origin();
    let mut by = by;
    let mut identity = identity;
    if retry
        && let Some((requester, plan)) =
            retained::started_purge(conn, environment, revision).await?
    {
        by = requester;
        identity = plan.get("identity").cloned().unwrap_or(Json::Null);
        origin = plan
            .get("origin")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();
    }
    let row=make(conn,&Making{kind,environment,plan:json!({"removal_revision":revision,"volume_refs":promise.retained_volumes,"identity":identity,"origin":origin,"restoring":!purge,"fresh":false,"mode":"make"}),
        phases:if purge {&PURGE_PHASES}else{&RESTORE_PHASES},created_by:by},now).await?;
    if !retry
        && !retained::reserve(
            conn,
            environment,
            revision,
            if purge { "purging" } else { "restoring" },
            now,
        )
        .await?
    {
        return Err(MakeError::Db(DbError::Rule {
            constraint: "environment_removal_changed".into(),
        }));
    }
    if retry {
        jobs::merge_plan(conn, row.id, &json!({"purge_started":true}), now).await?;
    }
    Ok(row)
}

/// Current intrinsic home administration, with token scopes and subtree still narrowing it.
pub async fn authority(
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    environment: Id,
) -> Result<(bool, bool), EngineError> {
    use orch_access_contract::{
        decisions::ObjectRef,
        permissions::{Permission, ResourceKind},
    };
    let object = ObjectRef {
        kind: ResourceKind::Environment.into(),
        id: environment,
    };
    let (_, _, site) = orch_authz::resources::site(conn, object).await?;
    if !principal.caller().permits(Permission::EnvironmentManage)
        || !principal.caller().reaches(site.path.as_deref())
        || principal.suspension().closing(&site.orgs).is_some()
    {
        return Ok((false, false));
    }
    let effective =
        orch_authz::resources::effective(conn, principal, object, orch_authz::Policy::default())
            .await?;
    Ok((
        effective
            .held
            .iter()
            .any(|h| h.permission == Permission::EnvironmentManage),
        effective.administers,
    ))
}

/// A durable restore or purge runner; its plan, rather than memory, owns its candidate.
pub struct Kept {
    engine: Engine,
    environment: Id,
    purge: bool,
    revision: i64,
}
impl Kept {
    /// Reads this lifecycle job's fixed identity.
    pub fn of(engine: Engine, row: &jobs::JobRow) -> Result<Self, EngineError> {
        Ok(Self {
            engine,
            environment: row
                .environment_id
                .ok_or_else(|| EngineError::unreadable("the job has no environment"))?,
            purge: row.kind == jobs::retained::PURGE,
            revision: row
                .plan
                .get("removal_revision")
                .and_then(Json::as_i64)
                .ok_or_else(|| EngineError::unreadable("the job has no removal revision"))?,
        })
    }

    async fn check_purge(&self, job: &Job) -> Result<(), Stop> {
        let mut tx = self.engine.db().begin().await.map_err(EngineError::from)?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        hosting::lock_environment(&mut tx, self.environment).await?;
        self.check_purge_in(&mut tx, job).await
    }

    async fn check_purge_in(&self, conn: &mut Conn, job: &Job) -> Result<(), Stop> {
        let promise = retained::removal(conn, self.environment)
            .await?
            .ok_or_else(|| failed("gone", "the environment is gone"))?;
        if promise.removal_revision != self.revision
            || promise.removal_phase.as_deref() != Some("purging")
        {
            return Err(failed("conflict", "the removal changed"));
        }
        let row = job.row().await?;
        if row.plan.get("purge_started").and_then(Json::as_bool) == Some(true)
            || promise.volumes_purge_at.is_some_and(|at| at <= job.now())
        {
            return Ok(());
        }
        let principal = requester(conn, &row, job.now()).await?;
        if !authority(conn, &principal, self.environment).await?.1 {
            return Err(failed(
                "denied",
                "the requester is no longer an admin of the environment's home",
            ));
        }
        Ok(())
    }

    async fn check_restore(&self, job: &Job) -> Result<(), Stop> {
        let mut tx = self.engine.db().begin().await.map_err(EngineError::from)?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        hosting::lock_environment(&mut tx, self.environment).await?;
        self.check_restore_in(&mut tx, job).await
    }

    async fn check_restore_in(&self, conn: &mut Conn, job: &Job) -> Result<(), Stop> {
        let promise = retained::removal(conn, self.environment)
            .await?
            .ok_or_else(|| failed("gone", "the environment is gone"))?;
        if promise.removal_revision != self.revision
            || promise.removal_phase.as_deref() != Some("restoring")
        {
            return Err(failed("conflict", "the kept files are no longer reserved"));
        }
        let row = job.row().await?;
        let principal = requester(conn, &row, job.now()).await?;
        let (manager, admin) = authority(conn, &principal, self.environment).await?;
        if !manager && !admin {
            return Err(failed(
                "denied",
                "the requester no longer manages this environment or its home",
            ));
        }
        let environment = hosting::environment(conn, self.environment)
            .await?
            .ok_or_else(|| failed("gone", "the environment is gone"))?;
        if orch_db::queries::environment_facts::of(conn, environment.id)
            .await?
            .is_some_and(|facts| facts.attaches())
        {
            self.connected_machine(conn, &environment, &row.plan)
                .await?;
        }
        Ok(())
    }

    async fn connected_machine(
        &self,
        conn: &mut Conn,
        environment: &hosting::EnvironmentState,
        plan: &Json,
    ) -> Result<orch_db::queries::resources::MachineRow, Stop> {
        let machine = environment
            .machine_id
            .ok_or_else(|| failed("unavailable", "the registered machine is unknown"))?;
        let row = orch_db::queries::resources::machine(conn, machine)
            .await?
            .filter(|row| {
                row.online
                    && row.is_active()
                    && row.session_marker().is_some()
                    && row.roles.iter().any(|role| role == "environment")
            })
            .ok_or_else(|| failed("unavailable", "the registered machine is unavailable"))?;
        let reported = orch_db::queries::environment_facts::of(conn, environment.id)
            .await?
            .filter(|facts| facts.attaches())
            .ok_or_else(|| {
                failed(
                    "unavailable",
                    "the machine no longer declares directory attachment",
                )
            })?;
        let policy: orch_link_contract::policy::LocalPolicy =
            serde_json::from_value(reported.policy.ok_or_else(|| {
                failed(
                    "unavailable",
                    "the machine has not reported its current local policy",
                )
            })?)
            .map_err(|_| failed("unavailable", "the machine's local policy is unreadable"))?;
        policy
            .check_root(std::path::Path::new(&environment.workspace_root))
            .map_err(|_| {
                failed(
                    "denied",
                    "the original directory is no longer offered by its machine",
                )
            })?;
        if plan.get("connected").and_then(Json::as_bool) == Some(true) {
            let session: Option<chrono::DateTime<chrono::Utc>> = serde_json::from_value(
                plan.get("connected_session").cloned().unwrap_or(Json::Null),
            )
            .map_err(EngineError::from)?;
            if plan["connected_machine"] != json!(machine)
                || plan["connected_root"] != environment.workspace_root
                || session != row.session_marker()
            {
                return Err(failed(
                    "conflict",
                    "the original registration or machine session changed",
                ));
            }
        }
        Ok(row)
    }

    async fn verify_network(&self, conn: &mut Conn, plan: &Json) -> Result<(), Stop> {
        let runtime = plan
            .get("runtime")
            .and_then(Json::as_str)
            .and_then(|value| value.parse::<Id>().ok())
            .ok_or_else(|| failed("unavailable", "the restoring runtime is unknown"))?;
        let policy: orch_driver_contract::network::NetworkPolicy = serde_json::from_value(
            plan.get("network_policy")
                .cloned()
                .ok_or_else(|| failed("unavailable", "the restoring network policy is unknown"))?,
        )
        .map_err(EngineError::from)?;
        if !crate::network::verified(conn, runtime, &policy).await? {
            return Err(failed(
                "network.restricted_unavailable",
                "the host has not verified the retained network restriction",
            ));
        }
        Ok(())
    }

    async fn old(&self, conn: &mut Conn) -> Result<hosting::RuntimeFull, Stop> {
        let ids = retained::last_runtime(conn, self.environment).await?;
        hosting::runtime(
            conn,
            ids.ok_or_else(|| failed("unavailable", "there is no retained runtime definition"))?,
        )
        .await?
        .ok_or_else(|| failed("unavailable", "the retained runtime definition is gone"))
    }

    async fn commit(&self, job: &Job) -> Result<Done, Stop> {
        job.pass_commit().await?;
        let mut tx = self.engine.db().begin().await.map_err(EngineError::from)?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        let env = hosting::lock_environment(&mut tx, self.environment)
            .await?
            .ok_or_else(|| failed("gone", "the environment is gone"))?;
        let plan = job.plan().await?;
        let changed = if self.purge {
            retained::purged(&mut tx, self.environment, self.revision, job.now()).await?
        } else if plan.get("connected").and_then(Json::as_bool) == Some(true) {
            self.check_restore_in(&mut tx, job).await?;
            let machine = self.connected_machine(&mut tx, &env, &plan).await?;
            let facts = plan.get("candidate_facts").ok_or_else(|| {
                failed("unavailable", "the registered directory was not verified")
            })?;
            hosting::record_facts(
                &mut tx,
                self.environment,
                facts,
                Some(&env.workspace_root),
                job.now(),
            )
            .await?;
            // A directory has no managed unit generation to manufacture. Its connector's
            // verified generation remains the one used by file and screen commands.
            retained::restored_registration(
                &mut tx,
                self.environment,
                self.revision,
                machine.id,
                machine.session_marker(),
                env.generation,
                job.now(),
            )
            .await?
        } else {
            self.check_restore_in(&mut tx, job).await?;
            self.verify_network(&mut tx, &plan).await?;
            let runtime = plan
                .get("runtime")
                .and_then(Json::as_str)
                .and_then(|v| v.parse::<Id>().ok())
                .ok_or_else(|| failed("unavailable", "the verified runtime is unknown"))?;
            retained::publish(&mut tx, runtime).await?;
            if let Some(facts) = plan.get("candidate_facts") {
                hosting::record_facts(
                    &mut tx,
                    self.environment,
                    facts,
                    facts.get("workspace_root").and_then(Json::as_str),
                    job.now(),
                )
                .await?;
            }
            retained::restored(&mut tx, self.environment, self.revision, runtime, job.now()).await?
        };
        if !changed {
            return Err(failed("conflict", "the kept files changed before commit"));
        }
        let action = if self.purge {
            audit::name!("environment.purged")
        } else {
            audit::name!("environment.restored")
        };
        let row = job.row().await?;
        let summary = format!(
            "{} was {}",
            env.name,
            if self.purge {
                "removed for good"
            } else {
                "restored stopped"
            }
        );
        let mut entry = Entry::new(
            row.created_by.map_or(Actor::System, Actor::User),
            action,
            "environment",
            Some(self.environment),
            &summary,
        );
        entry.history = true;
        entry.node_id = Some(env.home_node_id);
        entry.org_id = Some(env.org_id);
        entry.environment_id = Some(self.environment);
        entry.details = json!({"job":job.id(),"removal_revision":self.revision,"reason":if row.created_by.is_some(){"request"}else{"expired"}});
        let origin: Origin = row
            .plan
            .get("origin")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();
        audit::append(&mut tx, &entry, &origin, job.now()).await?;
        jobs::merge_plan(
            &mut tx,
            job.id(),
            &json!({"lifecycle_committed":true}),
            job.now(),
        )
        .await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(Done::Finished)
    }
}

#[async_trait]
impl Phased for Kept {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        let plan = job.plan().await?;
        if plan.get("lifecycle_committed").and_then(Json::as_bool) == Some(true) {
            return Ok(Done::Finished);
        }
        if !self.purge {
            self.check_restore(job).await?;
        }
        match phase {
            "authority" => {
                self.check_purge(job).await?;
                Ok(Done::Finished)
            }
            "volumes" => {
                self.check_purge(job).await?;
                // Persist the irreversible boundary before any host command. A command whose
                // response is lost may already have removed files, so a failed attempt cannot
                // release this reservation for restore.
                let mut tx = self.engine.db().begin().await.map_err(EngineError::from)?;
                orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
                hosting::lock_environment(&mut tx, self.environment).await?;
                self.check_purge_in(&mut tx, job).await?;
                jobs::merge_plan(&mut tx, job.id(), &json!({"purge_started":true}), job.now())
                    .await?;
                tx.commit().await.map_err(DbError::from)?;
                let refs: Vec<retained::Volume> =
                    serde_json::from_value(plan["volume_refs"].clone())
                        .map_err(EngineError::from)?;
                for reference in &refs {
                    let mut conn = self
                        .engine
                        .db()
                        .acquire()
                        .await
                        .map_err(EngineError::from)?;
                    if hosting::volume_in_use(
                        &mut conn,
                        reference.host_id,
                        &reference.name,
                        reference.runtime_id,
                    )
                    .await?
                    {
                        return Err(failed(
                            "in_use",
                            "a live successor still references the kept files",
                        ));
                    }
                }
                if let Some(reference) = refs.first() {
                    let mut conn = self
                        .engine
                        .db()
                        .acquire()
                        .await
                        .map_err(EngineError::from)?;
                    let host = hosts::host(&mut conn, reference.host_id)
                        .await?
                        .ok_or_else(|| failed("no_host", "the host keeping the files is gone"))?;
                    drop(conn);
                    job.command(
                        Outgoing::new(
                            ids::derived("node.runtime.remove", &[job.id().as_bytes(), b"purge"]),
                            host.machine_id,
                            SerialKey::Runtime(reference.runtime_id),
                            &NodeRuntimeRemove {
                                runtime_id: reference.runtime_id,
                                keep_volumes: false,
                                volume_refs: serde_json::from_value(plan["volume_refs"].clone())
                                    .map_err(EngineError::from)?,
                            },
                        )
                        .map_err(EngineError::from)?,
                    )
                    .await?;
                }
                let old = {
                    let mut conn = self
                        .engine
                        .db()
                        .acquire()
                        .await
                        .map_err(EngineError::from)?;
                    retained::last_runtime(&mut conn, self.environment).await?
                };
                if let Some(old) = old {
                    let runtime = {
                        let mut conn = self
                            .engine
                            .db()
                            .acquire()
                            .await
                            .map_err(EngineError::from)?;
                        hosting::runtime(&mut conn, old).await?
                    };
                    if let Some(runtime) = runtime {
                        self.engine.purge_unit_images(job, &runtime).await?;
                    }
                }
                Ok(Done::Finished)
            }
            "kept" => {
                let mut tx = self.engine.db().begin().await.map_err(EngineError::from)?;
                orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
                hosting::lock_environment(&mut tx, self.environment).await?;
                self.check_restore_in(&mut tx, job).await?;
                let environment = hosting::environment(&mut tx, self.environment)
                    .await?
                    .ok_or_else(|| failed("gone", "the environment is gone"))?;
                if orch_db::queries::environment_facts::of(&mut tx, environment.id)
                    .await?
                    .is_some_and(|facts| facts.attaches())
                {
                    let machine = self.connected_machine(&mut tx, &environment, &plan).await?;
                    jobs::merge_plan(
                        &mut tx,
                        job.id(),
                        &json!({"connected":true,
                        "connected_machine":machine.id,"connected_session":machine.session_marker(),
                        "connected_root":environment.workspace_root}),
                        job.now(),
                    )
                    .await?;
                    tx.commit().await.map_err(DbError::from)?;
                    return Ok(Done::Finished);
                }
                let promise = retained::removal(&mut tx, self.environment)
                    .await?
                    .ok_or_else(|| failed("gone", "the environment is gone"))?;
                if promise.removal_phase.as_deref() != Some("restoring")
                    || promise.removal_revision != self.revision
                {
                    return Err(failed("conflict", "the kept files are no longer reserved"));
                }
                if plan.get("runtime").is_none() {
                    let old = self.old(&mut tx).await?;
                    let saved = retained::definition(&mut tx, old.id)
                        .await?
                        .ok_or_else(|| {
                            failed(
                                "unavailable",
                                "the original unit definition is not retained",
                            )
                        })?;
                    let mut definition = saved.get("spec").cloned().ok_or_else(|| {
                        failed("unavailable", "the original unit definition is incomplete")
                    })?;
                    let current = hosts::template(&mut tx, old.template_id)
                        .await?
                        .ok_or_else(|| failed("unavailable", "the retained template is gone"))?;
                    let required = orch_db::queries::network::required(&mut tx, old.id).await?;
                    // Restoration cannot turn a retained restriction into an open unit. Current
                    // template restrictions may strengthen the original, never weaken it.
                    let only_orch = definition["network"]["mode"] == "none"
                        || current.spec["network"]["mode"] == "none";
                    let restricted = only_orch
                        || required.as_deref() == Some("restricted")
                        || saved["network_policy"]["mode"] == "restricted"
                        || definition["network_egress"] == "restricted"
                        || current.spec["network_egress"] == "restricted"
                        || crate::network::mode(&definition)
                            == orch_driver_contract::network::EgressMode::Restricted
                        || crate::network::mode(&current.spec)
                            == orch_driver_contract::network::EgressMode::Restricted;
                    if restricted {
                        let fields = definition.as_object_mut().ok_or_else(|| {
                            failed("unavailable", "the retained definition is unreadable")
                        })?;
                        let network = fields.entry("network").or_insert_with(|| json!({}));
                        if network.is_null() {
                            *network = json!({});
                        }
                        let network = network.as_object_mut().ok_or_else(|| {
                            failed(
                                "unavailable",
                                "the retained network definition is unreadable",
                            )
                        })?;
                        if only_orch {
                            network.insert("mode".to_owned(), json!("none"));
                        } else {
                            network.entry("mode").or_insert_with(|| json!("bridge"));
                        }
                        network.insert("egress".to_owned(), json!("restricted"));
                    }
                    let network_policy = orch_db::queries::trace::read_scope(
                        &[self.environment],
                        crate::network::policy(
                            &mut tx,
                            orch_settings::Place::Environment(self.environment),
                            &definition,
                        ),
                    )
                    .await?;
                    let candidate = new_id();
                    orch_db::queries::environments::changes::successor(
                        &mut tx,
                        &old,
                        candidate,
                        &crate::jobs::provision::unit_name(&candidate),
                        old.template_id,
                        &old.spec_hash,
                        job.now(),
                    )
                    .await?;
                    retained::new_machine(&mut tx, candidate).await?;
                    sqlx::query("UPDATE runtimes SET kept_image=$2 WHERE id=$1")
                        .bind(candidate)
                        .bind(old.kept_image.clone())
                        .execute(&mut *tx)
                        .await
                        .map_err(DbError::from)?;
                    if network_policy.restricted() {
                        orch_db::queries::network::require(&mut tx, candidate, Some("restricted"))
                            .await?;
                    }
                    jobs::merge_plan(
                        &mut tx,
                        job.id(),
                        &json!({"runtime":candidate,"host":old.host_id,"definition":definition,"retained_spec":saved,"network_policy":network_policy,"kept_image":old.kept_image}),
                        job.now(),
                    )
                    .await?;
                }
                tx.commit().await.map_err(DbError::from)?;
                Ok(Done::Finished)
            }
            "create" => {
                if plan.get("connected").and_then(Json::as_bool) == Some(true) {
                    let mut conn = self
                        .engine
                        .db()
                        .acquire()
                        .await
                        .map_err(EngineError::from)?;
                    let environment = hosting::environment(&mut conn, self.environment)
                        .await?
                        .ok_or_else(|| failed("gone", "the environment is gone"))?;
                    let machine = self
                        .connected_machine(&mut conn, &environment, &plan)
                        .await?;
                    let with = orch_db::queries::trace::read_scope(
                        &[self.environment],
                        crate::environments::attach_with(&mut conn, self.environment, job.now()),
                    )
                    .await?;
                    let body = crate::environments::attach_body(
                        self.environment,
                        Some(&environment.workspace_root),
                        &with,
                    );
                    drop(conn);
                    // A lost answer may still have attached the directory. Record cleanup intent
                    // before issuing it, just as the managed candidate is recorded before creation.
                    job.remember(&json!({"registration_attach_started":true}))
                        .await?;
                    let facts = job
                        .command(
                            Outgoing::new(
                                ids::derived(
                                    "env.attach",
                                    &[job.id().as_bytes(), b"restore-registration"],
                                ),
                                machine.id,
                                SerialKey::Environment(self.environment),
                                &body,
                            )
                            .map_err(EngineError::from)?,
                        )
                        .await?;
                    job.remember(&json!({"candidate_facts":facts,"registration_attached":true}))
                        .await?;
                    return Ok(Done::Finished);
                }
                let row = job.row().await?;
                let provision = super::provision::Provision::of(self.engine.clone(), &row);
                for phase in ["image", "tools", "create", "enroll", "attach"] {
                    self.check_restore(job).await?;
                    orch_db::queries::trace::read_scope(
                        &[self.environment],
                        provision.run(job, phase),
                    )
                    .await?;
                }
                Ok(Done::Finished)
            }
            "after_up" => super::after_up::run(job).await,
            "verify" => {
                super::after_up::run(job).await?;
                let mut conn = self
                    .engine
                    .db()
                    .acquire()
                    .await
                    .map_err(EngineError::from)?;
                if plan.get("connected").and_then(Json::as_bool) == Some(true) {
                    let environment = hosting::environment(&mut conn, self.environment)
                        .await?
                        .ok_or_else(|| failed("gone", "the environment is gone"))?;
                    self.connected_machine(&mut conn, &environment, &plan)
                        .await?;
                    let facts = plan.get("candidate_facts").ok_or_else(|| {
                        failed("unavailable", "the registered directory was not verified")
                    })?;
                    if facts["workspace_root"] != environment.workspace_root
                        || facts["generation"].as_i64() != Some(i64::from(environment.generation))
                    {
                        return Err(failed(
                            "conflict",
                            "the machine reported a different directory or generation",
                        ));
                    }
                    return Ok(Done::Finished);
                }
                let runtime = plan
                    .get("runtime")
                    .and_then(Json::as_str)
                    .and_then(|v| v.parse::<Id>().ok())
                    .ok_or_else(|| failed("unavailable", "there is no restoring runtime"))?;
                let machine = hosts::machine_of_runtime(&mut conn, runtime).await?;
                let state = hosting::runtime(&mut conn, runtime).await?;
                let facts = plan
                    .get("candidate_facts")
                    .ok_or_else(|| failed("unhealthy", "the candidate did not report its files"))?;
                let findings = super::health::findings(&super::health::Side {
                    online: machine.is_some_and(|(_, online)| online),
                    facts,
                    runtime_state: state.as_ref().map_or("failed", |r| r.state.as_str()),
                });
                if !findings.is_empty() {
                    return Err(failed("unhealthy", findings.join("; ")));
                }
                self.verify_network(&mut conn, &plan).await?;
                Ok(Done::Finished)
            }
            "stop" => {
                if plan.get("connected").and_then(Json::as_bool) == Some(true) {
                    // A person's machine and its processes are not managed units. Registration
                    // restoration starts no executor, shell, browser or conversation.
                    return Ok(Done::Finished);
                }
                let runtime = plan
                    .get("runtime")
                    .and_then(Json::as_str)
                    .and_then(|v| v.parse::<Id>().ok())
                    .ok_or_else(|| failed("unavailable", "there is no restoring runtime"))?;
                let mut conn = self
                    .engine
                    .db()
                    .acquire()
                    .await
                    .map_err(EngineError::from)?;
                let candidate = hosting::runtime(&mut conn, runtime)
                    .await?
                    .ok_or_else(|| failed("gone", "the restoring unit is gone"))?;
                let host = hosts::host(&mut conn, candidate.host_id)
                    .await?
                    .ok_or_else(|| failed("no_host", "the retained host is unavailable"))?;
                drop(conn);
                job.command(
                    Outgoing::new(
                        ids::derived("node.runtime.stop", &[job.id().as_bytes(), b"restore"]),
                        host.machine_id,
                        SerialKey::Runtime(runtime),
                        &NodeRuntimeStop {
                            runtime_id: runtime,
                            reason: None,
                        },
                    )
                    .map_err(EngineError::from)?,
                )
                .await?;
                let mut conn = self
                    .engine
                    .db()
                    .acquire()
                    .await
                    .map_err(EngineError::from)?;
                hosting::set_runtime_state(
                    &mut conn,
                    runtime,
                    &hosting::runtime_state::LIVE,
                    "stopped",
                    None,
                    job.now(),
                )
                .await?;
                Ok(Done::Finished)
            }
            "commit" => self.commit(job).await,
            _ => Err(failed("internal", "the kept-file job has no such phase")),
        }
    }

    async fn undo(&self, job: &Job, why: &Stop) -> Undone {
        if super::after_up::record_interrupted(job).await.is_err() {
            tracing::warn!("an interrupted restore command outcome could not be recorded");
        }
        let result = async {
            let plan = job.plan().await?;
            if plan.get("lifecycle_committed").and_then(Json::as_bool) == Some(true) {
                return Ok::<_, EngineError>(());
            }
            if !self.purge
                && (plan
                    .get("registration_attach_started")
                    .and_then(Json::as_bool)
                    == Some(true)
                    || plan.get("registration_attached").and_then(Json::as_bool) == Some(true))
            {
                let machine = plan
                    .get("connected_machine")
                    .and_then(Json::as_str)
                    .and_then(|value| value.parse::<Id>().ok())
                    .ok_or_else(|| {
                        EngineError::unreadable("the restored registration's machine is unknown")
                    })?;
                job.command_to_undo(Outgoing::new(
                    ids::derived(
                        "env.detach",
                        &[job.id().as_bytes(), b"restore-registration-rollback"],
                    ),
                    machine,
                    SerialKey::Environment(self.environment),
                    &EnvDetach {
                        environment_id: self.environment,
                    },
                )?)
                .await
                .map_err(|error| EngineError::unreadable(format!("{error:?}")))?;
            }
            if !self.purge
                && let Some(runtime) = plan
                    .get("runtime")
                    .and_then(Json::as_str)
                    .and_then(|v| v.parse::<Id>().ok())
            {
                let mut conn = self.engine.db().acquire().await?;
                if let Some(candidate) = hosting::runtime(&mut conn, runtime).await?
                    && let Some(host) = hosts::host(&mut conn, candidate.host_id).await?
                {
                    drop(conn);
                    let remove = Outgoing::new(
                        ids::derived(
                            "node.runtime.remove",
                            &[job.id().as_bytes(), b"restore-rollback"],
                        ),
                        host.machine_id,
                        SerialKey::Runtime(runtime),
                        &NodeRuntimeRemove {
                            runtime_id: runtime,
                            keep_volumes: true,
                            volume_refs: Vec::new(),
                        },
                    )?;
                    job.command_to_undo(remove)
                        .await
                        .map_err(|e| EngineError::unreadable(format!("{e:?}")))?;
                    let mut conn = self.engine.db().acquire().await?;
                    orch_db::queries::environments::changes::discard(&mut conn, runtime, job.now())
                        .await?;
                }
            }
            let mut tx = self.engine.db().begin().await?;
            orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
            if self.purge && plan.get("purge_started").and_then(Json::as_bool) == Some(true) {
                let failures = hosting::failed_removals(&mut tx, self.environment, None).await? + 1;
                let pause = super::remove::pause_after(
                    failures,
                    self.engine.inner.config.hosting.removal_first_pause,
                    self.engine.inner.config.hosting.reaper_retry_max,
                );
                retained::retry_purge(
                    &mut tx,
                    self.environment,
                    self.revision,
                    &match why {
                        Stop::Failed(f) => f.message.clone(),
                        Stop::Cancelled => {
                            "The operation was cancelled; its files remain kept".to_owned()
                        }
                        Stop::Lost => "The controller stopped; its files remain kept".to_owned(),
                    },
                    job.now() + chrono::Duration::from_std(pause).unwrap_or_default(),
                    job.now(),
                )
                .await?;
            } else {
                retained::release(
                    &mut tx,
                    self.environment,
                    self.revision,
                    if self.purge { "purging" } else { "restoring" },
                    &match why {
                        Stop::Failed(f) => f.message.clone(),
                        Stop::Cancelled => {
                            "The operation was cancelled; its files remain kept".to_owned()
                        }
                        Stop::Lost => "The controller stopped; its files remain kept".to_owned(),
                    },
                    job.now(),
                )
                .await?;
            }
            tx.commit().await.map_err(DbError::from)?;
            Ok(())
        }
        .await;
        result.map_or_else(|e| Undone::Partly(e.to_string()), |()| Undone::Everything)
    }
}

/// Reloads the original request identity and its recorded administration mode for durable work.
pub async fn requester(
    conn: &mut Conn,
    row: &jobs::JobRow,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<orch_authz::Principal, Stop> {
    let person = row
        .created_by
        .ok_or_else(|| failed("denied", "the operation needs its original requester"))?;
    let identity = &row.plan["identity"];
    let caller = if let Some(token) = identity
        .get("token_id")
        .and_then(Json::as_str)
        .and_then(|v| v.parse::<Id>().ok())
    {
        let grant = retained::active_token(conn, token, person, now).await?;
        let (scopes, node) = grant.ok_or_else(|| failed("denied", "the token ended"))?;
        orch_authz::Caller::token(
            person,
            orch_authz::TokenGrant::from_stored(&scopes, node)
                .map_err(|e| failed("denied", e.to_string()))?
                .for_token(token),
        )
    } else {
        let session = identity
            .get("session_id")
            .and_then(Json::as_str)
            .and_then(|v| v.parse::<Id>().ok())
            .ok_or_else(|| failed("denied", "the browser sign-in is unknown"))?;
        let active = retained::active_session(conn, session, person, now).await?;
        if !active {
            return Err(failed("denied", "the browser sign-in ended"));
        }
        // The job acts for the original request, outside it: an admin who asked in the session's
        // As admin mode is checked in that mode, which the request's recorded origin says.
        let as_admin = row
            .plan
            .get("origin")
            .cloned()
            .and_then(|origin| serde_json::from_value::<Origin>(origin).ok())
            .is_some_and(|origin| origin.as_admin == Some(person));
        orch_authz::Caller::session(person).with_admin_mode(as_admin)
    };
    Ok(orch_authz::Principal::load(conn, caller, Time::new(now))
        .await
        .map_err(EngineError::from)?)
}
