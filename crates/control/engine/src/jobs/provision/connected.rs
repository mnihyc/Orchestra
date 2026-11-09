//! Owned connected-data initialization within the existing provision job and command journal.
use super::*;
use orch_link_contract::commands::{EnvAttach, EnvDataRemove, EnvDetach, ExecStart};

async fn target(job: &Job, environment: Id) -> Result<(EnvironmentState, Id), Stop> {
    let mut tx = job.engine().db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    if row.environment_id != Some(environment)
        || row.kind != kind::ENVIRONMENT_PROVISION
        || row.plan["mode"] != "copy"
    {
        return Err(failed(
            "copy_changed",
            "the connected provision lost its original association",
        ));
    }
    creator_authority::guard_in(&mut tx, job).await?;
    let (_, machine) = crate::environment_creation::connected_target(&mut tx, &row)
        .await?
        .ok_or_else(|| {
            failed(
                "copy_changed",
                "the connected provision has no native creator",
            )
        })?;
    let state = env_rows::environment(&mut tx, environment)
        .await?
        .ok_or(Stop::Lost)?;
    tx.commit().await.map_err(DbError::from)?;
    Ok((state, machine))
}

pub(super) async fn run(job: &Job, environment: Id, phase: &str) -> Result<Done, Stop> {
    if phase == "commit" && job.plan().await?["provision_committed"] == true {
        return Ok(Done::Finished);
    }
    let (state, machine) = target(job, environment).await?;
    match phase {
        "attach" => {
            let saved = job.plan().await?.get("copy_attach").cloned();
            let body: EnvAttach = if let Some(body) = saved {
                serde_json::from_value(body).map_err(EngineError::from)?
            } else {
                let mut conn = job.engine().db().acquire().await?;
                let mut with = environments::attach_with(&mut conn, environment, job.now()).await?;
                with.data_generation = Some(i64::from(state.generation));
                let body = environments::attach_body(environment, Some(""), &with);
                drop(conn);
                job.remember(&json!({"copy_attach":body,"copy_executors":with.executors}))
                    .await?;
                body
            };
            if body.environment_id != environment
                || body.data_generation != Some(0)
                || body.data_sources.as_ref() != Some(&Default::default())
            {
                return Err(failed(
                    "copy_changed",
                    "the connected copy no longer creates fresh owned data",
                ));
            }
            let facts = job
                .command(
                    Outgoing::new(
                        ids::derived("env.attach", &[job.id().as_bytes()]),
                        machine,
                        SerialKey::Environment(environment),
                        &body,
                    )
                    .map_err(body_stop)?,
                )
                .await?;
            let root = facts
                .get("workspace_root")
                .and_then(Json::as_str)
                .filter(|root| !root.is_empty())
                .ok_or_else(|| {
                    failed(
                        "copy_unavailable",
                        "the machine did not report its fresh workspace",
                    )
                })?;
            let mut tx = job.engine().db().begin().await?;
            let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
            crate::environment_creation::connected_target(&mut tx, &row)
                .await?
                .ok_or_else(|| failed("copy_changed", "the attached copy target changed"))?;
            env_rows::record_facts(&mut tx, environment, &facts, Some(root), job.now()).await?;
            jobs::merge_plan(&mut tx, job.id(), &json!({"copy_attached":true}), job.now()).await?;
            tx.commit().await.map_err(DbError::from)?;
            // As in ordinary provision, the executor can become available privately;
            // the Environment itself remains provisioning until the copied files and checks end.
            let executors: Vec<String> =
                serde_json::from_value(job.plan().await?["copy_executors"].clone())
                    .map_err(EngineError::from)?;
            for executor in executors {
                let _ = job
                    .command(
                        Outgoing::new(
                            ids::derived("exec.start", &[job.id().as_bytes(), executor.as_bytes()]),
                            machine,
                            SerialKey::Environment(environment),
                            &ExecStart {
                                environment_id: environment,
                                executor,
                            },
                        )
                        .map_err(body_stop)?,
                    )
                    .await?;
            }
            Ok(Done::Finished)
        }
        "files" => crate::initial_files::deliver(job, environment, machine)
            .await?
            .ok_or_else(|| {
                failed(
                    "copy_changed",
                    "the connected copy lost its initial workspace selection",
                )
            }),
        "after_up" => crate::jobs::after_up::run(job).await,
        "verify" => {
            let plan = job.plan().await?;
            if plan["copy_attached"] != true || plan["initial_files_done"] != true {
                return Err(failed(
                    "copy_unavailable",
                    "the initial workspace has not finished materializing",
                ));
            }
            job.remember(&json!({"copy_verified":true})).await?;
            Ok(Done::Finished)
        }
        "commit" => {
            let mut tx = job.engine().db().begin().await?;
            if crate::environments::ending::moving_review(&job.row().await?)?.is_some() {
                orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
            } else {
                orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
            }
            creator_authority::guard_in(&mut tx, job).await?;
            let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
            let checked = creator_authority::current(
                &mut tx,
                &row,
                job.engine().inner.clock.as_ref(),
                job.engine().inner.access.as_ref(),
            )
            .await?
            .ok_or_else(|| {
                failed(
                    "copy_changed",
                    "the connected creator lost its native checkpoint",
                )
            })?;
            if row.plan["copy_verified"] != true {
                return Err(failed(
                    "copy_unavailable",
                    "the copied workspace is not verified",
                ));
            }
            let (state, actual) = crate::environment_creation::connected_target(&mut tx, &row)
                .await?
                .ok_or_else(|| failed("copy_changed", "the copy lost its owned target"))?;
            if actual != machine || state.generation != 0 {
                return Err(failed(
                    "copy_changed",
                    "the connected copy generation changed",
                ));
            }
            let session:Option<chrono::DateTime<Utc>>=sqlx::query_scalar("SELECT COALESCE(session_started_at,last_seen_at) FROM machines WHERE id=$1 AND status='active' AND online AND deleted_at IS NULL FOR SHARE")
                .bind(machine).fetch_optional(&mut *tx).await.map_err(DbError::from)?
                .ok_or_else(||failed("copy_unavailable","the target machine is offline"))?;
            sqlx::query("UPDATE environments SET status='running',status_detail=NULL,attached_session=$2,updated_at=$3 WHERE id=$1 AND status='provisioning'")
                .bind(environment).bind(session).bind(job.now()).execute(&mut *tx).await.map_err(DbError::from)?;
            jobs::stamp_environment(&mut tx, environment, job.now()).await?;
            crate::initial_files::release(&mut tx, job.id(), Time::new(job.now())).await?;
            crate::environments::ending::finish(
                job.engine(),
                &mut tx,
                &row,
                checked.principal(),
                job.engine().inner.clock.as_ref(),
            )
            .await?;
            jobs::merge_plan(
                &mut tx,
                job.id(),
                &json!({"provision_committed":true}),
                job.now(),
            )
            .await?;
            checked.check(job.engine().inner.clock.as_ref())?;
            tx.commit().await.map_err(DbError::from)?;
            // The copied workspace is now committed. Finishing that truthful job
            // outcome and waking independent consumers cannot repeat creator work.
            job.engine().reevaluate_environment(environment).await?;
            Ok(Done::Finished)
        }
        _ => Err(failed(
            INTERNAL,
            "the connected copy has no such provision phase",
        )),
    }
}

pub(super) async fn undo(job: &Job, environment: Id, _why: &Stop) -> Undone {
    let cleanup=async {
        let plan=job.plan().await?;
        if plan["provision_committed"]==true {return Ok::<(),Stop>(());}
        let machine:Id=serde_json::from_value(plan.pointer("/creator_provision/machine").cloned()
            .ok_or_else(||failed("copy_changed","the copy lost its actual recipient"))?).map_err(EngineError::from)?;
        let generation:i64=plan.get("generation").and_then(Json::as_i64).filter(|g|*g==0)
            .ok_or_else(||failed("copy_changed","the copy lost its owned generation"))?;
        if plan.pointer("/creator_provision/environment")!=Some(&json!(environment))
            || plan.pointer("/creator_provision/origin/operation").and_then(Json::as_str)
                .is_none_or(|operation|!orch_api_contract::environment_sources::duplicating::is_connected(operation)) {
            return Err(failed("copy_changed","the cleanup belongs to another creation"));
        }
        // Native owned storage records fence both commands to this Environment/generation;
        // the original source workspace and any borrowed path are never cleanup operands.
        let detach=job.command_to_undo(Outgoing::new(ids::derived("env.attach.rollback",&[job.id().as_bytes()]),machine,
            SerialKey::Environment(environment),&EnvDetach{environment_id:environment}).map_err(body_stop)?).await;
        if let Err(stop)=detach
            && !matches!(&stop,Stop::Failed(error) if error.code=="not_found") {return Err(stop);}
        let removed=job.command_to_undo(Outgoing::new(ids::derived("env.data.remove",&[job.id().as_bytes(),b"copy.rollback"]),machine,
            SerialKey::Environment(environment),&EnvDataRemove{environment_id:environment,data_generation:generation}).map_err(body_stop)?).await;
        if let Err(stop)=removed
            && !matches!(&stop,Stop::Failed(error) if error.code=="not_found") {return Err(stop);}
        let mut tx=job.engine().db().begin().await?;
        job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
        crate::initial_files::release(&mut tx,job.id(),Time::new(job.now())).await?;
        sqlx::query("UPDATE environments SET status='failed',status_detail='The initial workspace copy did not finish',updated_at=$2 WHERE id=$1 AND status='provisioning'")
            .bind(environment).bind(job.now()).execute(&mut *tx).await.map_err(DbError::from)?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(())
    }.await;
    match cleanup {
        Ok(()) => Undone::Everything,
        Err(Stop::Failed(error)) => Undone::Deferred(error.message),
        Err(Stop::Cancelled | Stop::Lost) => {
            Undone::Deferred("The owned connected-data cleanup must resume".into())
        }
    }
}
