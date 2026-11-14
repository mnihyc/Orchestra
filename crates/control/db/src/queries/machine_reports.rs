//! Machine sources of environment, harness, provider and host reports before any effects.

use crate::{Conn, DbError};
use orch_link_contract::streams::Stream;
use orch_types::{Id, Json};

/// Whether a machine owns a stream without considering a particular report's identifiers.
pub async fn owns(conn: &mut Conn, machine: Id, stream: &Stream) -> Result<bool, DbError> {
    if !active(conn, machine).await? {
        return Ok(false);
    }
    owns_stream(conn, machine, stream).await
}

async fn active(conn: &mut Conn, machine: Id) -> Result<bool, DbError> {
    // Admit one report writer per machine before it takes any other rows. Shared locks let
    // two streams enter together, then deadlock when each upgrades to write the machine's facts.
    // A revocation still waits for the batch, or completes first and refuses it. Key-share locks
    // for commands and sessions remain compatible, so a report never holds their references back.
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT status = 'active' AND deleted_at IS NULL FROM machines WHERE id = $1 FOR NO KEY UPDATE",
    )
    .bind(machine)
    .fetch_optional(&mut *conn)
    .await?
    .unwrap_or(false))
}

async fn owns_stream(conn: &mut Conn, machine: Id, stream: &Stream) -> Result<bool, DbError> {
    match *stream {
        Stream::Machine => Ok(true),
        Stream::Session {
            thread_id,
            segment_id,
        } => super::machine_streams::owns_segment(conn, thread_id, segment_id, machine).await,
        Stream::Env { environment_id } => Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM environments WHERE id = $1 AND machine_id = $2)",
        )
        .bind(environment_id)
        .bind(machine)
        .fetch_one(conn)
        .await?),
        Stream::Provider { provider_id } => Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM providers WHERE id = $1
             AND key_machine_id = $2)",
        )
        .bind(provider_id)
        .bind(machine)
        .fetch_one(conn)
        .await?),
        Stream::Harness {
            provider_id,
            agent_id,
        } => Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM providers p JOIN agents a ON a.id = $2
             WHERE p.id = $1 AND (a.machine_id IS NULL OR a.machine_id = $3)
             AND (p.machine_id = $3 OR EXISTS(
               SELECT 1 FROM commands c WHERE c.machine_id = $3 AND c.kind = 'agent.start'
               AND c.body->>'provider_id' = $1::text AND c.body->>'agent_id' = $2::text
               AND c.state IN ('sent','accepted','succeeded')) OR EXISTS(
               SELECT 1 FROM harnesses h WHERE h.provider_id = $1 AND h.agent_id = $2
               AND h.machine_id = $3)))",
        )
        .bind(provider_id)
        .bind(agent_id)
        .bind(machine)
        .fetch_one(conn)
        .await?),
        Stream::Node => Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM hosts WHERE machine_id = $1)",
        )
        .bind(machine)
        .fetch_one(conn)
        .await?),
    }
}

/// Whether every report belongs to its source, including objects named inside a node stream.
pub async fn accepts(
    conn: &mut Conn,
    machine: Id,
    stream: &Stream,
    reports: &[(&str, &Json)],
) -> Result<bool, DbError> {
    // A move successor can report before it owns the environment, but revocation/deletion
    // is final for this batch and must not fall through to that narrow move exception.
    if !active(conn, machine).await? {
        return Ok(false);
    }
    let owns = owns_stream(conn, machine, stream).await?;
    let mut accepted = owns;
    // The turns of this session found to be its own, each asked of the database once: a batch's events
    // are mostly one turn's, and each of them asked again in its transaction (C-turns).
    let mut turns_found: Vec<Id> = Vec::new();
    for (kind, body) in reports.iter().copied() {
        if !owns {
            if let Stream::Env { environment_id } = *stream
                && requested_move_report(conn, machine, environment_id, kind, body).await?
            {
                accepted = true;
                continue;
            }
            return Ok(false);
        }
        match *stream {
            Stream::Session {
                thread_id,
                segment_id,
            } if body.get("turn_id").is_some_and(|value| !value.is_null()) => {
                let Some(turn) = id(body, "turn_id") else {
                    return Ok(false);
                };
                if turns_found.contains(&turn) {
                    continue;
                }
                if !sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM turns WHERE id = $1 AND thread_id = $2 AND segment_id = $3)",
                ).bind(turn).bind(thread_id).bind(segment_id).fetch_one(&mut *conn).await? {
                    return Ok(false);
                }
                turns_found.push(turn);
            }
            Stream::Provider { provider_id } if matches!(kind, "lease.usage" | "lease.denied") => {
                let Some(lease) = id(body, "lease_id") else {
                    return Ok(false);
                };
                if !sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM leases WHERE id = $1 AND provider_id = $2)",
                )
                .bind(lease)
                .bind(provider_id)
                .fetch_one(&mut *conn)
                .await?
                {
                    return Ok(false);
                }
            }
            Stream::Env { environment_id } if kind == "browser.state" => {
                let Some(browser) = id(body, "browser_id") else {
                    return Ok(false);
                };
                if !sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM browser_sessions WHERE id = $1 AND environment_id = $2)",
                ).bind(browser).bind(environment_id).fetch_one(&mut *conn).await? {
                    return Ok(false);
                }
            }
            Stream::Env { environment_id } if kind == "pty.exited" => {
                let Some(terminal) = id(body, "terminal_id") else {
                    return Ok(false);
                };
                if !sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM terminal_sessions WHERE id = $1 AND environment_id = $2)",
                ).bind(terminal).bind(environment_id).fetch_one(&mut *conn).await? {
                    return Ok(false);
                }
            }
            Stream::Node if matches!(kind, "node.runtime" | "node.network" | "node.hook") => {
                let Some(runtime) = id(body, "runtime_id") else {
                    continue;
                };
                if !owns_runtime(conn, machine, runtime).await? {
                    return Ok(false);
                }
            }
            Stream::Node if kind == "job.progress" => {
                let Some(job) = id(body, "job_id") else {
                    continue;
                };
                if !owns_host_job(conn, machine, job).await? {
                    return Ok(false);
                }
            }
            Stream::Env { environment_id } if kind == "job.progress" => {
                let Some(job) = id(body, "job_id") else {
                    continue;
                };
                if !sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM jobs j WHERE j.id=$1 AND (j.environment_id=$2 OR
                      (j.kind='measurement' AND j.plan->'subject'->>'environment_id'=$2::text AND EXISTS(
                        SELECT 1 FROM commands c WHERE c.id::text=j.plan->'active_command'->>'id'
                        AND c.machine_id=$3 AND c.state IN ('sent','accepted','succeeded')
                        AND c.body->>'environment_id'=$2::text AND c.body->'work'->>'job_id'=$1::text))))",
                )
                .bind(job)
                .bind(environment_id)
                .bind(machine)
                .fetch_one(&mut *conn)
                .await?
                {
                    return Ok(false);
                }
            }
            _ => {}
        }
    }
    Ok(accepted)
}

fn id(body: &Json, key: &str) -> Option<Id> {
    body.get(key)?.as_str()?.parse().ok()
}

/// Whether `machine` is the unit that enrolled for the environment's current runtime while the
/// environment's own row does not name it yet. What such a unit reports is held until the row names
/// it: it is not another machine's report, and nothing is refused or counted for it.
pub async fn awaits_attachment(
    conn: &mut Conn,
    machine: Id,
    environment: Id,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtimes r JOIN environments e ON e.id = r.environment_id
         WHERE r.environment_id = $1 AND r.machine_id = $2 AND r.successor_of IS NULL
         AND r.state IN ('creating','running') AND e.machine_id IS DISTINCT FROM $2)",
    )
    .bind(environment)
    .bind(machine)
    .fetch_one(conn)
    .await?)
}

/// Whether the machine is the registered node of this runtime's host.
pub async fn owns_runtime(conn: &mut Conn, machine: Id, runtime: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtimes r JOIN hosts h ON h.id = r.host_id
         WHERE r.id = $1 AND h.machine_id = $2)",
    )
    .bind(runtime)
    .bind(machine)
    .fetch_one(conn)
    .await?)
}

/// Whether the job's selected runtime and host belong to this node.
pub async fn owns_host_job(conn: &mut Conn, machine: Id, job: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM jobs j JOIN runtimes r ON r.id::text = j.plan->>'runtime'
         JOIN hosts h ON h.id = r.host_id WHERE j.id = $1 AND h.machine_id = $2
         AND (j.plan->>'host' IS NULL OR j.plan->>'host' = h.id::text)) OR EXISTS(
         SELECT 1 FROM jobs j JOIN commands c ON c.id::text=j.plan->'active_command'->>'id'
         WHERE j.id=$1 AND c.machine_id=$2 AND c.state IN ('sent','accepted','succeeded')
         AND (c.body->'work'->>'job_id'=$1::text OR c.body->>'job_id'=$1::text))",
    )
    .bind(job)
    .bind(machine)
    .fetch_one(conn)
    .await?)
}

/// The active move whose enrolled successor this machine runs, before it takes over.
pub async fn move_candidate(
    conn: &mut Conn,
    machine: Id,
    environment: Id,
) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT j.id FROM jobs j JOIN environments e ON e.id = j.environment_id
         JOIN runtimes r ON r.id::text = j.plan->>'runtime'
         JOIN runtimes old ON old.id = r.successor_of AND old.environment_id = e.id
         WHERE j.environment_id = $1 AND j.kind = 'environment_move' AND j.state = 'running'
         AND old.successor_of IS NULL AND old.state NOT IN ('retired','removed')
         AND r.environment_id = e.id AND r.machine_id = $2
         AND r.state IN ('creating','running') LIMIT 1",
    )
    .bind(environment)
    .bind(machine)
    .fetch_optional(conn)
    .await?)
}

async fn requested_move_report(
    conn: &mut Conn,
    machine: Id,
    environment: Id,
    kind: &str,
    body: &Json,
) -> Result<bool, DbError> {
    let Some(job) = move_candidate(conn, machine, environment).await? else {
        return Ok(false);
    };
    let commands: &[&str] = match kind {
        "env.facts" | "env.status" => &["env.attach"],
        "job.progress" if id(body, "job_id") == Some(job) => &["fs.snapshot", "fs.materialize"],
        _ => return Ok(false),
    };
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM commands WHERE machine_id = $1 AND kind = ANY($2)
         AND body->>'environment_id' = $3::text AND state IN ('sent','accepted','succeeded')
         AND (kind = 'env.attach' OR body->>'job_id' = $4::text))",
    )
    .bind(machine)
    .bind(commands)
    .bind(environment)
    .bind(job)
    .fetch_one(conn)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queries::resources;
    use orch_testkit::{clock, database::TestDb};
    use orch_types::new_id;
    use serde_json::json;

    #[tokio::test]
    async fn concurrent_streams_finish_their_reports_without_upgrading_shared_machine_locks() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let machine = resources::testing::insert_machine(&mut conn, "host", "connected")
            .await
            .unwrap();
        let host = new_id();
        sqlx::query("INSERT INTO hosts(id,machine_id,name) VALUES($1,$2,'Host')")
            .bind(host)
            .bind(machine)
            .execute(&mut *conn)
            .await
            .unwrap();
        let mut first = test.pool.begin().await.unwrap();
        let mut second = test.pool.begin().await.unwrap();
        let first_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *first)
            .await
            .unwrap();
        let second_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *second)
            .await
            .unwrap();
        let facts = json!({"hostname":"Host"});
        let reports = [("machine.facts", &facts)];
        assert!(
            accepts(&mut first, machine, &Stream::Machine, &reports)
                .await
                .unwrap()
        );
        // Commands and sessions only need a key-share lock for their machine reference.
        let referenced: Id =
            sqlx::query_scalar("SELECT id FROM machines WHERE id=$1 FOR KEY SHARE NOWAIT")
                .bind(machine)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(referenced, machine);
        let mut admission = Box::pin(accepts(&mut second, machine, &Stream::Node, &[]));
        // The existing database concurrency cases allow ten seconds for scheduling, and poll at
        // ten milliseconds. The exact blocker proves serialization; elapsed time proves nothing.
        let observed = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let blocked: bool =
                    sqlx::query_scalar("SELECT pg_blocking_pids($1) @> ARRAY[$2]::integer[]")
                        .bind(second_pid)
                        .bind(first_pid)
                        .fetch_one(&mut *conn)
                        .await
                        .unwrap();
                if blocked {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        });
        tokio::select! {
            result = &mut admission => panic!("a second writer was admitted before the first report committed: {result:?}"),
            result = observed => result.expect("the second report must wait on the first machine writer"),
        }
        resources::set_machine_facts(
            &mut first,
            machine,
            &facts,
            (None, None),
            0.25,
            clock::start(),
        )
        .await
        .unwrap();
        first.commit().await.unwrap();
        assert!(admission.await.unwrap());
        resources::mounts::record_sizes(&mut second, host, &json!([]))
            .await
            .unwrap();
        second.commit().await.unwrap();
        assert_eq!(
            resources::machine(&mut conn, machine)
                .await
                .unwrap()
                .unwrap()
                .facts["hostname"],
            "Host"
        );
    }

    #[tokio::test]
    async fn a_machine_report_keeps_the_mount_sizes_reported_by_its_host() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let machine = resources::testing::insert_machine(&mut conn, "host", "connected")
            .await
            .unwrap();
        let host = new_id();
        sqlx::query("INSERT INTO hosts(id,machine_id,name) VALUES($1,$2,'Host')")
            .bind(host)
            .bind(machine)
            .execute(&mut *conn)
            .await
            .unwrap();
        let sizes = json!([{"mount_id":new_id(),"kind":"path","source":"/srv/work","bytes":123}]);
        resources::mounts::record_sizes(&mut conn, host, &sizes)
            .await
            .unwrap();
        resources::set_machine_facts(
            &mut conn,
            machine,
            &json!({"hostname":"Host"}),
            (None, None),
            0.25,
            clock::start(),
        )
        .await
        .unwrap();
        let snapshot = resources::mounts::on_machine(&mut conn, machine)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.sizes, sizes);
        assert_eq!(
            resources::machine(&mut conn, machine)
                .await
                .unwrap()
                .unwrap()
                .facts,
            json!({"hostname":"Host"})
        );
    }

    #[tokio::test]
    async fn an_active_source_can_report_while_offline_but_revocation_and_deletion_end_its_admission()
     {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let machine = resources::testing::insert_machine(&mut conn, "source", "connected")
            .await
            .unwrap();
        // These are source checks; the receiving link separately decodes the event's complete body.
        let facts = json!({});
        let reports = [("machine.facts", &facts)];
        assert!(owns(&mut conn, machine, &Stream::Machine).await.unwrap());
        assert!(
            accepts(&mut conn, machine, &Stream::Machine, &reports)
                .await
                .unwrap()
        );
        assert!(
            !resources::machine(&mut conn, machine)
                .await
                .unwrap()
                .unwrap()
                .online,
            "a queued report's authority does not depend on a transient online mark"
        );
        assert!(
            resources::revoke_machine(&mut conn, machine, clock::start())
                .await
                .unwrap()
        );
        assert!(!owns(&mut conn, machine, &Stream::Machine).await.unwrap());
        assert!(
            !accepts(&mut conn, machine, &Stream::Machine, &reports)
                .await
                .unwrap()
        );
        assert!(
            !accepts(
                &mut conn,
                machine,
                &Stream::Env {
                    environment_id: new_id()
                },
                &[("env.facts", &facts)]
            )
            .await
            .unwrap(),
            "an inactive source is refused before any move-successor exception is considered"
        );

        let removed = resources::testing::insert_machine(&mut conn, "removed-source", "connected")
            .await
            .unwrap();
        sqlx::query("UPDATE machines SET deleted_at=$2 WHERE id=$1")
            .bind(removed)
            .bind(clock::start())
            .execute(&mut *conn)
            .await
            .unwrap();
        assert!(!owns(&mut conn, removed, &Stream::Machine).await.unwrap());
        assert!(
            !accepts(&mut conn, removed, &Stream::Machine, &reports)
                .await
                .unwrap()
        );
        assert!(
            !accepts(&mut conn, new_id(), &Stream::Machine, &reports)
                .await
                .unwrap()
        );
    }
}
