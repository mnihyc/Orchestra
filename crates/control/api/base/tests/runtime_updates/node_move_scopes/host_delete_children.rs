//! Accepted HTTP cascades retain each real Provider child through the ordinary worker channel.
use super::*;
use orch_api_contract::loss::{
    HostDeleteCreate, HostDeleteCreateBody, HostDeletePlan, HostDeletePreflight,
    HostDeletePreflightBody, HostEnvironmentCascade, HostProviderCascade,
};

const HOST_CREATE: &str = "HostDeleteCreate";
struct Cascade {
    world: World,
    team: Team,
    target: Id,
    template: Id,
}

async fn cascade(make: bool) -> Cascade {
    report_api_failures();
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    // These cases inspect accepted queue/source boundaries, before any physical worker runs.
    world.app.app.engine.stop();
    let mut conn = world.conn().await;
    sqlx::query("UPDATE runtimes SET state='removed' WHERE environment_id=$1")
        .bind(team.environment)
        .execute(&mut *conn)
        .await
        .unwrap();
    let template: Id = if make {
        // This test world does not run release seeding. The lost-runtime path uses
        // the real default template identity, which must exist before it is chosen.
        sqlx::query("INSERT INTO env_templates (id,name,kind,driver,spec) VALUES ($1,'Provider recovery fixture','provider','docker','{}') ON CONFLICT(id) DO NOTHING")
            .bind(orch_db::seeds::PROVIDER_RUNTIME_TEMPLATE_ID)
            .execute(&mut *conn).await.unwrap();
        orch_db::seeds::PROVIDER_RUNTIME_TEMPLATE_ID
    } else {
        sqlx::query_scalar("SELECT template_id FROM runtimes WHERE provider_id=$1")
            .bind(team.provider)
            .fetch_one(&mut *conn)
            .await
            .unwrap()
    };
    let written =
        sqlx::query("UPDATE env_templates SET driver='docker',spec=$2,enabled=true WHERE id=$1")
            .bind(template)
            .bind(json!({"image":"busybox:latest","resources":{},"colocated_harnesses":[]}))
            .execute(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        written.rows_affected(),
        1,
        "the selected provider template exists"
    );
    if !make {
        // Recreate reads the definition that made this existing unit, not today's
        // template. Supply that factual history just as the environment fixture does.
        let runtime: Id =
            sqlx::query_scalar("SELECT id FROM runtimes WHERE provider_id=$1 AND state='running'")
                .bind(team.provider)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        let definition: serde_json::Value =
            sqlx::query_scalar("SELECT spec FROM env_templates WHERE id=$1")
                .bind(template)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        sqlx::query("INSERT INTO jobs(id,kind,provider_id,state,plan) VALUES($1,'provider_provision',$2,'succeeded',$3)")
            .bind(new_id()).bind(team.provider)
            .bind(json!({"runtime":runtime,"created_spec":{"spec":definition,"resources":{},"tools":[]},"tools":[]}))
            .execute(&mut *conn).await.unwrap();
    }
    sqlx::query("UPDATE providers SET runtime_config=runtime_config||jsonb_build_object('runtime.host_id',$2::uuid::text,'runtime.template_id',$3::uuid::text) WHERE id=$1")
        .bind(team.provider).bind(team.host).bind(template).execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE hosts SET drivers=(SELECT drivers FROM hosts WHERE id=$2) WHERE id=$1")
        .bind(team.host)
        .bind(target)
        .execute(&mut *conn)
        .await
        .unwrap();
    if make {
        sqlx::query("UPDATE runtimes SET state='retired',lost_at=$2 WHERE provider_id=$1")
            .bind(team.provider)
            .bind(world.now())
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("UPDATE providers SET runtime_unavailable='The source runtime was lost',status='offline',machine_id=NULL WHERE id=$1")
            .bind(team.provider).execute(&mut *conn).await.unwrap();
    }
    drop(conn);
    resources::name_person(
        &world,
        "template",
        template,
        "template.use",
        team.c.admin.id(),
    )
    .await;
    Cascade {
        world,
        team,
        target,
        template,
    }
}

async fn review(fixture: &Cascade, providers: HostProviderCascade) -> HostDeleteCreateBody {
    let plan: HostDeletePlan = fixture
        .world
        .app
        .call(HostDeletePreflight, &[&fixture.team.host])
        .token(&fixture.team.c.admin.token)
        .body(&HostDeletePreflightBody {
            environments: HostEnvironmentCascade::Remove,
            providers,
            host_id: Some(fixture.target),
            erase: Some(false),
            ..HostDeletePreflightBody::default()
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("the native manager reads the actual cascade review");
    assert!(plan.executable, "{:?}", plan.rows);
    HostDeleteCreateBody {
        preflight_id: plan.preflight_id,
        confirm: plan.name,
        notify: false,
        volumes: Vec::new(),
        data_confirmations: Vec::new(),
    }
}

fn provider_rules(fixture: &Cascade) -> Vec<serde_json::Value> {
    vec![
        rule(HOST_CREATE, "host", fixture.team.host, &["manage"]),
        rule(HOST_CREATE, "provider", fixture.team.provider, &["manage"]),
        rule(HOST_CREATE, "host", fixture.target, &["write"]),
        rule(HOST_CREATE, "template", fixture.template, &["write"]),
    ]
}

async fn create(fixture: &Cascade, secret: &str, body: &HostDeleteCreateBody) -> RawReply {
    fixture
        .world
        .app
        .call(HostDeleteCreate, &[&fixture.team.host])
        .token(secret)
        .body(body)
        .send_bytes()
        .await
}

async fn provider_child(make: bool) {
    let fixture = cascade(make).await;
    let body = review(&fixture, HostProviderCascade::Recreate).await;
    let mut without_provider = provider_rules(&fixture);
    without_provider.retain(|rule| rule["target"]["kind"] != "provider");
    let missing = key(
        &fixture.world,
        fixture.team.c.admin.id(),
        without_provider.clone(),
    )
    .await;
    let before:Vec<Id>=sqlx::query_scalar("SELECT id FROM jobs WHERE (provider_id=$1 AND kind IN ('provider_recreate','provider_provision')) OR (kind='host_delete' AND plan#>>'{cascade,host}'=$2) ORDER BY id")
        .bind(fixture.team.provider).bind(fixture.team.host.to_string())
        .fetch_all(&fixture.world.db.pool).await.unwrap();
    let refused = create(&fixture, &missing.secret, &body).await;
    assert_eq!(
        refused.status.as_u16(),
        403,
        "{}",
        String::from_utf8_lossy(&refused.raw)
    );
    let queued: Vec<Id> = sqlx::query_scalar("SELECT id FROM jobs WHERE (provider_id=$1 AND kind IN ('provider_recreate','provider_provision')) OR (kind='host_delete' AND plan#>>'{cascade,host}'=$2) ORDER BY id")
        .bind(fixture.team.provider).bind(fixture.team.host.to_string())
        .fetch_all(&fixture.world.db.pool).await.unwrap();
    assert_eq!(
        queued, before,
        "a missing Provider scope accepts no parent or child"
    );
    let actor = key(
        &fixture.world,
        fixture.team.c.admin.id(),
        provider_rules(&fixture),
    )
    .await;
    let reply = create(&fixture, &actor.secret, &body).await;
    assert_eq!(
        reply.status.as_u16(),
        202,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let accepted: serde_json::Value = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(accepted["projection"], "receipt");
    assert!(accepted.get("progress").is_none() && accepted.get("created_by").is_none());
    let parent: Id = serde_json::from_value(accepted["job_id"].clone()).unwrap();
    let parent = orch_db::queries::jobs::job(&mut *fixture.world.conn().await, parent)
        .await
        .unwrap()
        .unwrap();
    let children: Vec<Id> =
        serde_json::from_value(parent.plan["cascade"]["children"].clone()).unwrap();
    assert_eq!(children.len(), 1);
    let child = orch_db::queries::jobs::job(&mut *fixture.world.conn().await, children[0])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(child.provider_id, Some(fixture.team.provider));
    assert_eq!(child.environment_id, None);
    assert_eq!(
        child.kind,
        if make {
            "provider_provision"
        } else {
            "provider_recreate"
        }
    );
    assert!(orch_engine::jobs::lifecycle_commands::environment::present(
        &child
    ));
    let access =
        orch_api_core::thread_access::AuthzAccess::new(fixture.world.app.app.clock.clone());
    let current = orch_engine::jobs::lifecycle_commands::environment::principal(
        &mut *fixture.world.conn().await,
        &child,
        fixture.world.app.app.clock.as_ref(),
        &access,
        None,
    )
    .await
    .expect("the actual Provider child keeps the accepted Host operation");
    assert_eq!(current.principal.user(), fixture.team.c.admin.id());
    assert_eq!(
        current
            .principal
            .caller()
            .grant()
            .and_then(|grant| grant.token()),
        Some(actor.id),
        "Provider work cannot substitute a session for the accepted key"
    );
    assert_eq!(
        current.principal.caller().admitted_operation(),
        Some(HOST_CREATE)
    );
    let mut replaced = child.clone();
    replaced.provider_id = Some(new_id());
    assert!(
        orch_engine::jobs::lifecycle_commands::environment::principal(
            &mut *fixture.world.conn().await,
            &replaced,
            fixture.world.app.app.clock.as_ref(),
            &access,
            None,
        )
        .await
        .is_err(),
        "another Provider cannot borrow this child source"
    );
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(actor.id)
        .bind(json!({"rules":without_provider}))
        .execute(&fixture.world.db.pool)
        .await
        .unwrap();
    assert!(
        orch_engine::jobs::host_delete::authority::principal(
            &mut *fixture.world.conn().await,
            &parent,
            fixture.world.app.app.clock.as_ref(),
        )
        .await
        .is_ok(),
        "the intrinsic Host act remains current"
    );
    assert!(
        orch_engine::jobs::lifecycle_commands::environment::principal(
            &mut *fixture.world.conn().await,
            &child,
            fixture.world.app.app.clock.as_ref(),
            &access,
            None,
        )
        .await
        .is_err(),
        "an accepted Provider child still needs its own current management scope"
    );
    assert_eq!(
        orch_db::queries::jobs::job(&mut *fixture.world.conn().await, child.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        "queued"
    );
}

#[tokio::test]
async fn host_delete_provider_replace_retains_the_actual_provider_and_current_scope() {
    provider_child(false).await;
}

#[tokio::test]
async fn host_delete_provider_make_retains_the_actual_provider_and_current_scope() {
    provider_child(true).await;
}

#[tokio::test]
async fn host_delete_provider_leave_remains_an_intrinsic_host_act() {
    let fixture = cascade(false).await;
    let body = review(&fixture, HostProviderCascade::Leave).await;
    let actor = key(
        &fixture.world,
        fixture.team.c.admin.id(),
        vec![rule(HOST_CREATE, "host", fixture.team.host, &["manage"])],
    )
    .await;
    let reply = create(&fixture, &actor.secret, &body).await;
    assert_eq!(
        reply.status.as_u16(),
        202,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let accepted: serde_json::Value = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(accepted["projection"], "receipt");
    let parent: Id = serde_json::from_value(accepted["job_id"].clone()).unwrap();
    let parent = orch_db::queries::jobs::job(&mut *fixture.world.conn().await, parent)
        .await
        .unwrap()
        .unwrap();
    let children: Vec<Id> =
        serde_json::from_value(parent.plan["cascade"]["children"].clone()).unwrap();
    assert_eq!(children.len(), 1);
    let child = orch_db::queries::jobs::job(&mut *fixture.world.conn().await, children[0])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(child.provider_id, Some(fixture.team.provider));
    assert!(
        !orch_engine::jobs::lifecycle_commands::environment::present(&child),
        "owned retirement is not a new Provider management act"
    );
    assert!(
        orch_engine::jobs::host_delete::authority::principal(
            &mut *fixture.world.conn().await,
            &parent,
            fixture.world.app.app.clock.as_ref(),
        )
        .await
        .is_ok()
    );
}

#[tokio::test]
async fn host_delete_environment_move_keeps_its_host_source_and_observed_measurements() {
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    let answers = answer_move_plans(&world, &team, target).await;
    // Complete the real native file-state observation through its ordinary planner/worker.
    // A second operation may reuse that data, never its original requester's authority.
    let _ = ready_review(&world, &team, target).await;
    let plan = world
        .app
        .call(HostDeletePreflight, &[&team.host])
        .token(&team.c.admin.token)
        .body(&HostDeletePreflightBody {
            environments: HostEnvironmentCascade::Recover,
            providers: HostProviderCascade::Leave,
            host_id: Some(target),
            erase: Some(false),
            ..HostDeletePreflightBody::default()
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert!(plan.executable, "{:?}", plan.rows);
    world.app.app.engine.stop();
    let rules = vec![
        rule(HOST_CREATE, "host", team.host, &["manage"]),
        rule(HOST_CREATE, "environment", team.environment, &["manage"]),
        rule(HOST_CREATE, "host", target, &["write"]),
        rule(HOST_CREATE, "template", team.template, &["write"]),
    ];
    let actor = key(&world, team.c.admin.id(), rules.clone()).await;
    let observed = measurement_sources(&world).await;
    let reply = world
        .app
        .call(HostDeleteCreate, &[&team.host])
        .token(&actor.secret)
        .body(&HostDeleteCreateBody {
            preflight_id: plan.preflight_id,
            confirm: plan.name,
            notify: false,
            volumes: Vec::new(),
            data_confirmations: Vec::new(),
        })
        .send_bytes()
        .await;
    assert_eq!(
        reply.status.as_u16(),
        202,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert_eq!(
        measurement_sources(&world).await,
        observed,
        "Host confirmation retains complete observation data without retiming or reissuing it"
    );
    let accepted: serde_json::Value = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(accepted["projection"], "receipt");
    let parent: Id = serde_json::from_value(accepted["job_id"].clone()).unwrap();
    let parent = orch_db::queries::jobs::job(&mut *world.conn().await, parent)
        .await
        .unwrap()
        .unwrap();
    let children: Vec<Id> =
        serde_json::from_value(parent.plan["cascade"]["children"].clone()).unwrap();
    let mut moved = Vec::new();
    for child in children {
        let row = orch_db::queries::jobs::job(&mut *world.conn().await, child)
            .await
            .unwrap()
            .unwrap();
        if row.environment_id == Some(team.environment) {
            moved.push(row);
        } else {
            assert_eq!(row.provider_id, Some(team.provider));
            assert!(!orch_engine::jobs::lifecycle_commands::environment::present(&row));
        }
    }
    assert_eq!(moved.len(), 1);
    let child = &moved[0];
    assert!(orch_engine::jobs::lifecycle_commands::environment::present(
        child
    ));
    let access = orch_api_core::thread_access::AuthzAccess::new(world.app.app.clock.clone());
    let checkpoint = orch_engine::jobs::lifecycle_commands::environment::principal(
        &mut *world.conn().await,
        child,
        world.app.app.clock.as_ref(),
        &access,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        checkpoint.principal.caller().admitted_operation(),
        Some(HOST_CREATE)
    );
    assert_eq!(checkpoint.principal.user(), team.c.admin.id());
    assert_eq!(
        checkpoint
            .principal
            .caller()
            .grant()
            .and_then(|grant| grant.token()),
        Some(actor.id),
        "Environment work keeps its actual Host key rather than a global move source"
    );
    let mut narrowed = rules;
    narrowed.retain(|rule| rule["target"]["kind"] != "environment");
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(actor.id)
        .bind(json!({"rules":narrowed}))
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert!(
        orch_engine::jobs::lifecycle_commands::environment::principal(
            &mut *world.conn().await,
            child,
            world.app.app.clock.as_ref(),
            &access,
            None,
        )
        .await
        .is_err(),
        "Host management cannot replace this child's own Environment management"
    );
    answers.abort();
    let _ = answers.await;
}
