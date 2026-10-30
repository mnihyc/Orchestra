//! What a host and an environment report: the state of its units, what a host has and uses, how far a job is, and what an environment is.
//!
//! A host's node tells the system about its units, what it has and can make, and what it uses
//! (`node.runtime`, `node.facts`, `node.load`), and how far the work it was given has come
//! (`job.progress`); an environment's connector tells what it is and where its workspace is
//! (`env.facts`) and whether it is attached (`env.status`). The system acts on its own rows and the
//! node only tells: a unit that the row says runs and the node finds gone or ended makes the runtime
//! `failed`, and with it the environment, which the next start of a turn finds and says.
//!
//! The events of a stream are applied in the transaction that moves its cursor, in order, and
//! applying one twice changes nothing more than the first time did.

use crate::engine::Engine;
use crate::error::EngineError;
use crate::jobs::runner::report_progress;
use crate::unit::Unit;
use orch_db::queries::environments::hosting::{self as environments, runtime_state, status};
use orch_db::queries::resources::hosting as hosts;
use orch_link::streams::DeliveredEvent;
use orch_link_contract::events::{Event, NodeFacts, NodeHook, NodeLoad, NodeNetwork, NodeRuntime};
use orch_types::{Id, Json};
use serde_json::json;

impl Engine {
    /// Applies what a host's node reported.
    pub(crate) async fn apply_node_events(
        &self,
        unit: &mut Unit<'_>,
        machine: Id,
        events: &[DeliveredEvent],
    ) -> Result<(), EngineError> {
        let Some(host) = hosts::host_of_machine(&mut *unit.conn, machine).await? else {
            tracing::warn!(%machine, "a machine that is no host's node reported as one");
            return Ok(());
        };
        let at = unit.now();
        for event in events {
            let decoded = match Event::decode(&event.kind, event.body.clone()) {
                Ok(decoded) => decoded,
                Err(error) => {
                    tracing::warn!(%error, kind = %event.kind, "an event of a host does not read");
                    continue;
                }
            };
            match decoded {
                Event::NodeRuntime(report) => self.on_node_runtime(unit, host, &report).await?,
                Event::NodeNetwork(report) => self.on_node_network(unit, host, &report).await?,
                Event::NodeFacts(facts) => {
                    self.on_node_facts(unit, host, &facts).await?;
                    self.reconcile_host_networks(&mut *unit.conn, host, Some(event.seq))
                        .await?;
                }
                Event::NodeLoad(load) => self.on_node_load(unit, host, &load).await?,
                Event::NodeKeptImages(report) => {
                    self.on_kept_images(unit, host, &report, event.seq).await?
                }
                Event::NodeHook(run) => self.on_node_hook(unit, host, &run).await?,
                Event::PortState(report) => {
                    crate::ports::node_port_state(&mut *unit.conn, host, &report).await?;
                }
                Event::JobProgress(progress) => {
                    report_progress(
                        &mut *unit.conn,
                        machine,
                        progress.job_id,
                        progress.command_id,
                        &serde_json::to_value(&progress.progress)?,
                        progress.log.as_deref(),
                        at,
                    )
                    .await?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// A unit's state, as its node finds it, against what the row says. A unit that was running and is
    /// gone, ended or failed makes the runtime failed, and the environment with it; one that was being
    /// made and failed makes the runtime failed, which the job making it sees. What a row says is not
    /// changed by a unit that is merely somewhere else in its life.
    async fn on_node_runtime(
        &self,
        unit: &mut Unit<'_>,
        host: Id,
        report: &NodeRuntime,
    ) -> Result<(), EngineError> {
        let Some(runtime) = environments::runtime(&mut *unit.conn, report.runtime_id).await? else {
            tracing::warn!(runtime = %report.runtime_id, state = %report.state, "a host reports a unit that no runtime row names");
            return Ok(());
        };
        if runtime.host_id != host {
            return Ok(());
        }
        let missing_changed = sqlx::query(
            "UPDATE runtimes SET loss_missing=$2 WHERE id=$1 AND loss_missing IS DISTINCT FROM $2",
        )
        .bind(runtime.id)
        .bind(report.state == "absent")
        .execute(&mut *unit.conn)
        .await
        .map_err(orch_db::DbError::from)?
        .rows_affected();
        if missing_changed > 0
            && let Some(environment) = runtime.environment_id
        {
            let now = unit.now();
            orch_db::queries::jobs::stamp_environment(unit.conn, environment, now).await?;
        }
        let gone = matches!(report.state.as_str(), "absent" | "stopped" | "failed");
        let detail = report
            .detail
            .clone()
            .unwrap_or_else(|| match report.state.as_str() {
                "absent" => "the unit is gone".to_owned(),
                "stopped" => "the unit stopped by itself".to_owned(),
                _ => "the unit failed".to_owned(),
            });
        let now = unit.now();
        let fails = match runtime.state.as_str() {
            runtime_state::RUNNING => gone,
            runtime_state::CREATING => report.state == "failed",
            _ => false,
        };
        if !fails {
            // Replaying live mounts can fail while the same unit is running. Keep that reason
            // visible without manufacturing a runtime failure or hiding an unrelated detail.
            let mount_reason = report.detail.as_deref().filter(|detail| {
                detail.starts_with("mount changes pending:")
                    || detail.starts_with("mount changes need recreate:")
            });
            let mount_applied = report.detail.as_deref() == Some("mount changes applied");
            let had_mount_reason = runtime.state_detail.as_deref().is_some_and(|detail| {
                detail.starts_with("mount changes pending:")
                    || detail.starts_with("mount changes need recreate:")
            });
            if runtime.kind == "environment"
                && runtime.state == runtime_state::RUNNING
                && report.state == "running"
                && (mount_reason.is_some() || had_mount_reason || mount_applied)
                && runtime.state_detail != report.detail
            {
                environments::set_runtime_state(
                    &mut *unit.conn,
                    runtime.id,
                    &[runtime_state::RUNNING],
                    runtime_state::RUNNING,
                    report.detail.as_deref(),
                    now,
                )
                .await?;
                if let Some(environment) = runtime.environment_id {
                    orch_db::queries::jobs::stamp_environment(&mut *unit.conn, environment, now)
                        .await?;
                }
            }
            return Ok(());
        }
        let moved = environments::set_runtime_state(
            &mut *unit.conn,
            runtime.id,
            &[runtime_state::RUNNING, runtime_state::CREATING],
            runtime_state::FAILED,
            Some(&detail),
            now,
        )
        .await?;
        // A provider's unit that is gone leaves the provider without a machine (the part that keeps
        // providers): the next turn that needs it makes the unit again.
        if moved
            && runtime.kind == "provider"
            && let Some(provider) = runtime.provider_id
        {
            self.provider_unit_failed(&mut *unit.conn, provider, runtime.machine_id, now)
                .await?;
        }
        if moved
            && runtime.kind == "environment"
            && let Some(environment) = runtime.environment_id
        {
            environments::set_status(
                &mut *unit.conn,
                environment,
                &[status::RUNNING, status::OFFLINE],
                status::FAILED,
                Some(&detail),
                now,
            )
            .await?;
        }
        Ok(())
    }

    async fn on_node_network(
        &self,
        unit: &mut Unit<'_>,
        host: Id,
        report: &NodeNetwork,
    ) -> Result<(), EngineError> {
        let Some(runtime) = environments::runtime(&mut *unit.conn, report.runtime_id).await? else {
            return Ok(());
        };
        if runtime.host_id != host {
            return Ok(());
        }
        let previous = orch_db::queries::network::of_runtimes(&mut *unit.conn, &[runtime.id])
            .await?
            .into_iter()
            .next()
            .and_then(|row| row.network_state);
        let state = serde_json::to_value(&report.network)?;
        let at = unit.now();
        orch_db::queries::network::store(&mut *unit.conn, runtime.id, &state, at).await?;
        let policy_changed = previous.as_ref().is_none_or(|previous| {
            previous["revision"] != state["revision"]
                || previous["always_revision"] != state["always_revision"]
                || previous["state"] != state["state"]
                || previous["required_mode"] != state["required_mode"]
        });
        if policy_changed {
            let ready =
                report.network.state == orch_api_contract::environment::NetworkReadiness::Ready;
            let (kind, object) = match (runtime.environment_id, runtime.provider_id) {
                (Some(id), _) => ("environment", id),
                (_, Some(id)) => ("provider", id),
                _ => return Ok(()),
            };
            let mut entry = orch_db::audit::Entry::new(
                orch_db::audit::Actor::System,
                if ready {
                    orch_db::audit::name!("network.policy_applied")
                } else {
                    orch_db::audit::name!("network.failed_closed")
                },
                kind,
                Some(object),
                if ready {
                    "The host verified the network policy"
                } else {
                    "The requested network is unavailable; outward traffic stays closed"
                },
            );
            entry.history = true;
            entry.environment_id = runtime.environment_id;
            entry.details = json!({ "runtime": runtime.id, "mode": report.network.mode,
                "required_mode": report.network.required_mode, "revision": report.network.revision,
                "always_revision": report.network.always_revision });
            let now = unit.now();
            orch_db::audit::append(
                &mut *unit.conn,
                &entry,
                &orch_db::audit::Origin::default(),
                now,
            )
            .await?;
        }
        for thread in orch_db::queries::jobs::changes::threads(
            &mut *unit.conn,
            runtime.environment_id,
            runtime.provider_id,
            false,
        )
        .await?
        {
            unit.wake(thread);
        }
        Ok(())
    }

    /// One run of the host operator's lifecycle hook for a unit, as a line of the history of what the unit serves:
    /// the moment, why, how it ended and how long it took. Only the node of the runtime's own host is heard, and
    /// nothing a hook printed ever reaches the system.
    async fn on_node_hook(
        &self,
        unit: &mut Unit<'_>,
        host: Id,
        run: &NodeHook,
    ) -> Result<(), EngineError> {
        let Some(runtime) = environments::runtime(&mut *unit.conn, run.runtime_id).await? else {
            return Ok(());
        };
        if runtime.host_id != host {
            return Ok(());
        }
        let (kind, object) = match (runtime.environment_id, runtime.provider_id) {
            (Some(id), _) => ("environment", id),
            (_, Some(id)) => ("provider", id),
            _ => return Ok(()),
        };
        let summary = hook_summary(run);
        let mut entry = orch_db::audit::Entry::new(
            orch_db::audit::Actor::System,
            orch_db::audit::name!("unit.hook_ran"),
            kind,
            Some(object),
            &summary,
        );
        entry.history = true;
        entry.environment_id = runtime.environment_id;
        entry.details = json!({ "runtime": runtime.id, "event": run.event, "reason": run.reason,
            "replay": run.replay, "at": run.at, "duration_ms": run.duration_ms, "exit": run.exit,
            "timed_out": run.timed_out, "failed": run.failed });
        let now = unit.now();
        orch_db::audit::append(
            &mut *unit.conn,
            &entry,
            &orch_db::audit::Origin::default(),
            now,
        )
        .await?;
        Ok(())
    }

    async fn on_node_facts(
        &self,
        unit: &mut Unit<'_>,
        host: Id,
        facts: &NodeFacts,
    ) -> Result<(), EngineError> {
        let capacity = serde_json::to_value(facts.capacity).map_err(EngineError::from)?;
        let drivers = serde_json::to_value(&facts.drivers).map_err(EngineError::from)?;
        let schemas =
            serde_json::to_value(&facts.driver_configuration_schemas).map_err(EngineError::from)?;
        let storages = serde_json::to_value(&facts.storages).map_err(EngineError::from)?;
        let at = unit.now();
        hosts::record_node_facts(
            &mut *unit.conn,
            host,
            &capacity,
            &drivers,
            &schemas,
            &storages,
            at,
        )
        .await?;
        let sizes = serde_json::to_value(&facts.mount_sizes).map_err(EngineError::from)?;
        orch_db::queries::resources::mounts::record_sizes(&mut *unit.conn, host, &sizes).await?;
        Ok(())
    }

    async fn on_node_load(
        &self,
        unit: &mut Unit<'_>,
        host: Id,
        load: &NodeLoad,
    ) -> Result<(), EngineError> {
        let load: Json = json!({
            "cpu_percent": load.cpu_percent,
            "memory_used_mb": load.memory_used_mb,
            "disk_used_gb": load.disk_used_gb,
            "disk_free_gb": load.disk_free_gb,
        });
        let at = unit.now();
        hosts::record_node_load(&mut *unit.conn, host, &load, at).await?;
        Ok(())
    }

    /// Applies what an environment's connector reported.
    pub(crate) async fn apply_env_events(
        &self,
        unit: &mut Unit<'_>,
        machine: Id,
        environment: Id,
        events: &[DeliveredEvent],
    ) -> Result<(), EngineError> {
        let at = unit.now();
        // The destination's facts stay on the active move until its successor takes over.
        let state = environments::environment(&mut *unit.conn, environment).await?;
        if state
            .as_ref()
            .is_none_or(|state| state.machine_id != Some(machine))
        {
            if let Some(job) = orch_db::queries::machine_reports::move_candidate(
                &mut *unit.conn,
                machine,
                environment,
            )
            .await?
            {
                for event in events {
                    let change = match Event::decode(&event.kind, event.body.clone()) {
                        Ok(Event::EnvFacts(facts)) => json!({"candidate_facts": facts}),
                        Ok(Event::EnvStatus(status)) => json!({"candidate_status": status}),
                        _ => continue,
                    };
                    orch_db::queries::jobs::merge_plan(&mut *unit.conn, job, &change, at).await?;
                }
            }
            return Ok(());
        }
        // A directory on a person's machine keeps the root it was registered with and follows its
        // machine's session, which the schedule watches: what its connector says of where it is or
        // whether it holds it does not move either.
        let unit_lifecycle = orch_db::queries::environment_facts::of(&mut *unit.conn, environment)
            .await?
            .is_some_and(|facts| facts.makes_units());
        for event in events {
            let decoded = match Event::decode(&event.kind, event.body.clone()) {
                Ok(decoded) => decoded,
                Err(error) => {
                    tracing::warn!(%error, kind = %event.kind, "an event of an environment does not read");
                    continue;
                }
            };
            match decoded {
                Event::EnvFacts(facts) => {
                    let root = facts.workspace_root.clone();
                    let facts = serde_json::to_value(&facts).map_err(EngineError::from)?;
                    environments::record_facts(
                        &mut *unit.conn,
                        environment,
                        &facts,
                        unit_lifecycle.then_some(root.as_str()),
                        at,
                    )
                    .await?;
                }
                Event::EnvStatus(reported) if unit_lifecycle => {
                    // A connector that says it holds the environment again, or lets go of it, moves a
                    // running environment between `running` and `offline`; the jobs' statuses are theirs.
                    let (from, to) = match reported.status.as_str() {
                        "running" => (status::OFFLINE, status::RUNNING),
                        "offline" => (status::RUNNING, status::OFFLINE),
                        _ => continue,
                    };
                    environments::set_status(
                        &mut *unit.conn,
                        environment,
                        &[from],
                        to,
                        reported.detail.as_deref(),
                        at,
                    )
                    .await?;
                }
                // How far the machine is with a snapshot or a delivery of the workspace, and the list
                // of the tree that it uploaded on its own account.
                Event::JobProgress(progress) => {
                    self.apply_workspace_job_progress(
                        &mut *unit.conn,
                        machine,
                        progress.job_id,
                        progress.command_id,
                        &serde_json::to_value(&progress.progress)?,
                    )
                    .await?;
                }
                Event::EnvManifest(manifest) => {
                    self.keep_manifest(
                        &mut *unit.conn,
                        machine,
                        environment,
                        &manifest,
                        event.at.get(),
                    )
                    .await?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// The line a hook run writes: the moment, why, and how it ended, in words anybody who may view the thing reads.
fn hook_summary(run: &NodeHook) -> String {
    let again = if run.replay {
        ", run again as its host started"
    } else {
        ""
    };
    let ended = if run.timed_out {
        "ran out of its time".to_owned()
    } else {
        match run.exit {
            Some(0) => "passed".to_owned(),
            Some(code) => format!("failed with exit {code}"),
            None => "could not be run".to_owned(),
        }
    };
    format!(
        "The host's {} hook for a {}{again} {ended} in {} ms",
        run.event.as_str(),
        run.reason.as_str(),
        run.duration_ms
    )
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod scale_tests {
    use orch_testkit::database::TestDb;
    use orch_types::{Id, Json};
    use serde_json::json;

    fn indexed_single_row(plan: &Json) -> String {
        fn find(node: &Json) -> Option<&Json> {
            assert_ne!(
                node["Node Type"], "Seq Scan",
                "a million-host lookup scanned its table"
            );
            if node["Relation Name"] == "hosts" && node["Index Name"].is_string() {
                return Some(node);
            }
            node.get("Plans")
                .and_then(Json::as_array)
                .and_then(|children| children.iter().find_map(find))
        }
        let scan = find(&plan[0]["Plan"]).expect("the host lookup uses an index");
        assert_eq!(scan["Actual Rows"], 1);
        assert_eq!(
            scan.get("Rows Removed by Filter")
                .and_then(Json::as_u64)
                .unwrap_or(0),
            0
        );
        scan["Index Name"].as_str().unwrap().to_owned()
    }

    #[tokio::test]
    async fn million_host_schema_lookup_and_report_use_single_row_indexes() {
        let database = TestDb::fresh().await;
        let mut conn = database.pool.acquire().await.unwrap();
        // Bulk SQL creates the required size without retaining the rows or issuing a query per host.
        let machines = sqlx::query("INSERT INTO machines (id,name,placement,platform_os,platform_arch,connector_version,protocol_version,cert_serial,cert_fingerprint,cert_not_after)
            SELECT md5('scale-machine-'||n)::uuid, 'Scale '||n, 'connected','linux','x86_64','fixture',1,'scale-'||n,decode(md5('scale-cert-'||n)||md5('scale-cert-suffix-'||n),'hex'),now()+interval '1 day'
            FROM generate_series(1,1000000) AS n")
            .execute(&mut *conn).await.unwrap();
        assert_eq!(machines.rows_affected(), 1_000_000);
        let hosts = sqlx::query("INSERT INTO hosts (id,machine_id,name)
            SELECT md5('scale-host-'||n)::uuid,md5('scale-machine-'||n)::uuid,'Scale '||n FROM generate_series(1,1000000) AS n")
            .execute(&mut *conn).await.unwrap();
        assert_eq!(hosts.rows_affected(), 1_000_000);
        sqlx::query("ANALYZE hosts")
            .execute(&mut *conn)
            .await
            .unwrap();
        let machine: Id = sqlx::query_scalar("SELECT md5('scale-machine-1000000')::uuid")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        let host = orch_db::queries::resources::hosting::host_of_machine(&mut conn, machine)
            .await
            .unwrap()
            .unwrap();
        let lookup: Json = sqlx::query_scalar(
            "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) SELECT id FROM hosts WHERE machine_id=$1",
        )
        .bind(machine)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        let report: Json = sqlx::query_scalar("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) UPDATE hosts SET driver_configuration_schemas=$2 WHERE id=$1")
            .bind(host).bind(json!({"cluster":{"type":"object"}})).fetch_one(&mut *conn).await.unwrap();
        let lookup_index = indexed_single_row(&lookup);
        let report_index = indexed_single_row(&report);
        orch_db::queries::resources::hosting::record_node_facts(
            &mut conn,
            host,
            &json!({}),
            &json!({}),
            &json!({"cluster":{"type":"object"}}),
            &json!([]),
            chrono::Utc::now(),
        )
        .await
        .unwrap();
        let row = orch_db::queries::resources::hosting::host(&mut conn, host)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            row.driver_configuration_schemas,
            json!({"cluster":{"type":"object"}})
        );
        println!(
            "scale: 1000000 hosts; lookup index {lookup_index}; report index {report_index}; one scanned row each"
        );
    }
}
