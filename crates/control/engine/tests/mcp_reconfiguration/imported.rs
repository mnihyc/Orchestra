//! Discovery describes another machine's configuration; it never copies or selects its credentials.
use super::*;
use orch_db::queries::{jobs, resources};
use orch_types::{Hash, Json, new_id};

fn source(digest: &str) -> Json {
    json!({"source":"fake","scope":"user","directory":null,"name":"team-tool","transport":"stdio",
        "digest":digest,"definition_fingerprint":Hash::of(b"the same declared launch shape")})
}

async fn imported(world: &World, machine: Id, digest: &str) -> Id {
    let server = preset(world, world.ada).await;
    let mut origin = source(digest);
    origin["by"] = json!(world.ada);
    origin["at"] = json!(Time::new(world.clock.now()));
    sqlx::query("UPDATE mcp_servers SET imported=true,machine_id=$2,spec=$3 WHERE id=$1")
        .bind(server)
        .bind(machine)
        .bind(json!({"transport":"stdio","imported":origin}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    rows::offer(
        &mut world.pool.acquire().await.unwrap(),
        server,
        machine,
        None,
        &source(digest),
        world.clock.now(),
    )
    .await
    .unwrap();
    world
        .set(world.root, "mcp.enabled", json!({server.to_string():true}))
        .await;
    server
}

async fn discover(world: &World, digest: &str) {
    sqlx::query(
        "UPDATE environments SET facts=jsonb_set(facts,'{mcp_found}',$2),updated_at=$3 WHERE id=$1",
    )
    .bind(world.environment)
    .bind(json!([source(digest)]))
    .bind(world.clock.now())
    .execute(world.pool.pool())
    .await
    .unwrap();
}

async fn no_pending_job(conn: &mut orch_db::Conn, world: &World, server: Id) {
    let empty: bool = sqlx::query_scalar(
        "SELECT job_id IS NULL FROM mcp_server_offerings WHERE server_id=$1 AND machine_id=$2",
    )
    .bind(server)
    .bind(world.machine_id)
    .fetch_one(conn)
    .await
    .unwrap();
    assert!(
        empty,
        "the actual discovery candidate owns no pending import job"
    );
}

#[tokio::test]
async fn another_machines_discovery_or_retained_copy_never_selects_its_definition_without_an_import()
 {
    let world = World::new().await;
    let original = resources::testing::insert_machine(
        &mut world.pool.acquire().await.unwrap(),
        "original-tool-machine",
        "connected",
    )
    .await
    .unwrap();
    let server = imported(&world, original, "original-private-values").await;
    discover(&world, "other-private-values").await;
    let mut conn = world.pool.acquire().await.unwrap();
    let found = rows::offerings(&mut conn, &[server]).await.unwrap();
    let here = found
        .iter()
        .find(|offering| offering.machine_id == world.machine_id)
        .unwrap();
    assert_eq!(here.state, "found");
    no_pending_job(&mut conn, &world, server).await;
    assert!(here.offered_at.is_none());
    let jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind='mcp_offering' AND plan->>'server'=$1",
    )
    .bind(server.to_string())
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        jobs, 0,
        "discovery is not a request to copy a protected definition"
    );
    // Reconstruct a copy retained by the earlier automatic-offering behavior.
    // It does not become a manager's selection of another machine's tokens.
    rows::offer(
        &mut conn,
        server,
        world.machine_id,
        Some(world.environment),
        &source("other-private-values"),
        world.clock.now(),
    )
    .await
    .unwrap();
    let resolved =
        orch_engine::mcp::resolve(&mut conn, world.environment, Time::new(world.clock.now()))
            .await
            .unwrap();
    assert_eq!(resolved[0].row.machine_id, Some(original));
    assert_eq!(
        resolved[0].reason_code,
        Some(orch_api_contract::environment::McpReasonCode::OtherMachine)
    );
    assert_eq!(
        resolved[0].reason.as_deref(),
        Some("its imported definition belongs to another machine")
    );
    drop(conn);
    let thread = world.thread("No unseen tool from another machine").await;
    let before = world.commands.issued_of("mcp.start");
    assert!(
        world
            .engine()
            .mcp_for_session(world.environment, thread)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(world.commands.issued_of("mcp.start"), before);
    assert_eq!(world.commands.issued_of("mcp.import"), 0);
    sqlx::query("UPDATE machines SET lost_at=$2,status='revoked',online=false WHERE id=$1")
        .bind(original)
        .bind(world.clock.now())
        .execute(world.pool.pool())
        .await
        .unwrap();
    let row = rows::servers(&mut world.pool.acquire().await.unwrap(), &[server])
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(
        row.machine_id,
        Some(original),
        "loss does not select the other machine's copy"
    );
    use orch_engine::environment_mcp::existing;
    let selection = existing::select::<existing::Reimport>(
        &mut world.pool.acquire().await.unwrap(),
        &Caller::session(world.ada),
        Time::new(world.clock.now()),
        &existing::Intent::Reimport { server },
    )
    .await;
    assert!(
        matches!(selection, Err(existing::Error::NoOffering)),
        "Import again cannot silently choose a different machine"
    );
}

#[tokio::test]
async fn rediscovery_marks_a_changed_origin_without_overwriting_its_copy_and_clears_a_reverted_change()
 {
    let world = World::new().await;
    let server = imported(&world, world.machine_id, "saved-private-values").await;
    discover(&world, "saved-private-values").await;
    let original = rows::servers(&mut world.pool.acquire().await.unwrap(), &[server])
        .await
        .unwrap()
        .pop()
        .unwrap();
    discover(&world, "changed-private-values").await;
    let offerings = rows::offerings(&mut world.pool.acquire().await.unwrap(), &[server])
        .await
        .unwrap();
    let here = offerings
        .iter()
        .find(|offering| offering.machine_id == world.machine_id)
        .unwrap();
    assert_eq!(
        here.state, "ready",
        "the already imported copy remains the one that runs"
    );
    assert_eq!(
        here.proposal.as_ref().unwrap()["source"]["digest"],
        "changed-private-values"
    );
    assert_eq!(here.source["digest"], "saved-private-values");
    let unchanged = rows::servers(&mut world.pool.acquire().await.unwrap(), &[server])
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(unchanged.spec, original.spec);
    assert_eq!(world.count("SELECT count(*) FROM jobs WHERE kind='mcp_offering' AND plan->>'server'=$1::uuid::text", server).await, 0);
    discover(&world, "saved-private-values").await;
    let offerings = rows::offerings(&mut world.pool.acquire().await.unwrap(), &[server])
        .await
        .unwrap();
    assert!(
        offerings
            .iter()
            .find(|offering| offering.machine_id == world.machine_id)
            .unwrap()
            .proposal
            .is_none()
    );
}

#[tokio::test]
async fn a_retained_automatic_discovery_job_settles_as_a_candidate_without_issuing_a_native_import()
{
    let world = World::new().await;
    let server = imported(&world, world.machine_id, "saved-private-values").await;
    let job = new_id();
    let found = source("unreviewed-private-values");
    let mut tx = world.pool.begin().await.unwrap();
    let plan = json!({"server":server,"machine":world.machine_id,"environment":world.environment,"source":found});
    let progress = json!({"phase":"import","phases":[{"name":"import","weight":100,"state":"pending"}],"percent":0,"files_done":0,"files_total":null,"bytes_done":0,"bytes_total":null,"rate_bps":null,"eta_seconds":null,"message":null});
    // The legacy checkpoint has real durable ownership; no new public route can create it.
    jobs::insert(
        &mut tx,
        &jobs::NewJob {
            id: job,
            kind: "mcp_offering",
            node_id: None,
            environment_id: Some(world.environment),
            provider_id: None,
            plan: &plan,
            progress: &progress,
            created_by: None,
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE mcp_server_offerings SET state='pending',source=$3,job_id=$4 WHERE server_id=$1 AND machine_id=$2")
        .bind(server).bind(world.machine_id).bind(&found).bind(job).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    common::eventually(
        "the retained discovery job becomes an explicit-import candidate",
        || async {
            jobs::job(&mut world.pool.acquire().await.unwrap(), job)
                .await
                .unwrap()
                .is_some_and(|row| row.state == "succeeded")
        },
    )
    .await;
    let found = rows::offerings(&mut world.pool.acquire().await.unwrap(), &[server])
        .await
        .unwrap();
    let here = found
        .iter()
        .find(|offering| offering.machine_id == world.machine_id)
        .unwrap();
    assert_eq!(here.state, "found");
    assert_eq!(here.reason.as_deref(), Some("import_required"));
    no_pending_job(&mut world.pool.acquire().await.unwrap(), &world, server).await;
    assert_eq!(world.commands.issued_of("mcp.import"), 0);
}
