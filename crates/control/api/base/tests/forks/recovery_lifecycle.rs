//! Recovery switches retain the same native source on their actual fork and provisioning jobs.
use super::*;
use orch_api_contract::loss::{
    EnvironmentRecoverCreate, EnvironmentRecoverPreflight, EnvironmentRecoverPreflightBody,
    RecoveryAction, RecoveryChoice,
};
use orch_types::new_id;
use thread_history_scope::{Proof, key, rule};
fn preview(source: Id, environment: ForkEnvironment) -> EnvironmentRecoverPreflightBody {
    EnvironmentRecoverPreflightBody {
        environment_ids: vec![source],
        host_id: None,
        choices: vec![RecoveryChoice {
            environment_id: source,
            action: Some(RecoveryAction::SwitchThreads),
            host_id: None,
        }],
        environment: Some(environment),
    }
}
async fn plan(w: &Forks, body: &EnvironmentRecoverPreflightBody) -> Proof<Id> {
    let call =
        w.s.rig
            .world
            .app
            .call(EnvironmentRecoverPreflight, &[])
            .token(&w.s.ada.token)
            .body(body);
    let reply = call.send_bytes().await;
    assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.raw));
    let value: Json = serde_json::from_slice(&reply.raw)?;
    assert_eq!(value["executable"], 1, "{value}");
    Ok(serde_json::from_value(value["preflight_id"].clone())?)
}
async fn confirm(w: &Forks, token: &str, preflight: Id) -> Proof<(Id, Id)> {
    let call =
        w.s.rig
            .world
            .app
            .call(EnvironmentRecoverCreate, &[])
            .token(token)
            .body(&ForkCreateBody {
                preflight_id: preflight,
                options: None,
                notify: None,
            });
    let response = call.send_bytes().await;
    assert_eq!(
        response.status,
        202,
        "{}",
        String::from_utf8_lossy(&response.raw)
    );
    let value: Json = serde_json::from_slice(&response.raw)?;
    let batch: Id = serde_json::from_value(value["batch_id"].clone())?;
    let job: Id = serde_json::from_value(value["job_ids"][0].clone())?;
    assert_eq!(
        value,
        json!({"projection":"receipt","batch_id":batch,"job_ids":[job]}),
        "no hidden conversation, paths or thread names enter the receipt"
    );
    Ok((batch, job))
}
#[tokio::test(flavor = "multi_thread")]
async fn recovery_switch_uses_native_management_on_both_environments_without_a_thread_read() -> Proof
{
    let w = Forks::new().await;
    let thread =
        w.s.thread_by(&w.s.ada, "Hidden carried recovery thread")
            .await
            .base
            .id;
    let target = another_environment(
        &w.s,
        "Actual recovery destination",
        Some(w.s.rig.machine_id),
    )
    .await;
    let preflight = plan(
        &w,
        &preview(
            w.s.on.environment,
            ForkEnvironment::Existing {
                environment_id: target,
                files: Some(orch_api_contract::forks::FileSelection::Nothing),
            },
        ),
    )
    .await?;
    w.s.rig.world.app.app.engine.stop();
    let actor = key(
        &w.s.rig.world,
        w.s.ada.id(),
        vec![
            rule(
                "EnvironmentRecoverCreate",
                "environment",
                w.s.on.environment,
                "self",
                "manage",
            ),
            rule(
                "EnvironmentRecoverCreate",
                "environment",
                target,
                "self",
                "manage",
            ),
        ],
    )
    .await?;
    let (batch, job) = confirm(&w, &actor.secret, preflight).await?;
    let row = orch_db::queries::jobs::job(&mut *w.s.rig.world.conn().await, job)
        .await?
        .ok_or("switch missing")?;
    assert_eq!(row.node_id, Some(thread));
    assert_eq!(row.environment_id, Some(target));
    assert_eq!(row.batch_id, Some(batch));
    assert_eq!(row.plan["environment_lifecycle_version"], 1);
    let clock = w.s.rig.world.app.app.clock.clone();
    let access = orch_api_core::thread_access::AuthzAccess::new(clock.clone());
    let mut tx = w.s.rig.db.begin().await?;
    let current = orch_engine::jobs::lifecycle_commands::environment::current(
        &mut tx,
        &row,
        clock.as_ref(),
        &access,
        None,
    )
    .await?;
    assert_eq!(
        current.principal.caller().admitted_operation(),
        Some("EnvironmentRecoverCreate")
    );
    current.check(clock.as_ref())?;
    tx.rollback().await?;
    // The opaque fork id is never an Environment or another source of native authority.
    let wrong = key(
        &w.s.rig.world,
        w.s.ada.id(),
        vec![rule(
            "EnvironmentRecoverCreate",
            "environment",
            new_id(),
            "self",
            "manage",
        )],
    )
    .await?;
    let call =
        w.s.rig
            .world
            .app
            .call(EnvironmentRecoverCreate, &[])
            .token(&wrong.secret)
            .body(&ForkCreateBody {
                preflight_id: preflight,
                options: None,
                notify: None,
            });
    assert!(!call.send_bytes().await.status.is_success());
    Ok(())
}
#[tokio::test(flavor = "multi_thread")]
async fn recovery_maker_retains_the_same_lifecycle_source_on_its_real_new_provision_child() -> Proof
{
    let w = Forks::new().await;
    let thread =
        w.s.thread_by(&w.s.ada, "Carried into a native recovery maker")
            .await
            .base
            .id;
    let (host, template) = creator_support::managed(&w.s).await;
    let environment = ForkEnvironment::New {
        new: orch_api_contract::forks::ForkNewEnvironment {
            home_node_id: Some(w.s.tree.topic.base.id),
            host_id: Some(Some(host)),
            template_id: Some(template),
            files: Some(orch_api_contract::forks::FileSelection::Nothing),
            ..Default::default()
        },
    };
    let preflight = plan(&w, &preview(w.s.on.environment, environment)).await?;
    let mut policy = vec![
        rule(
            "EnvironmentRecoverCreate",
            "environment",
            w.s.on.environment,
            "self",
            "manage",
        ),
        rule("EnvironmentRecoverCreate", "host", host, "self", "write"),
        rule(
            "EnvironmentRecoverCreate",
            "template",
            template,
            "self",
            "write",
        ),
    ];
    policy.push(creator_support::rule(
        "EnvironmentRecoverCreate",
        "topic",
        w.s.tree.topic.base.id,
        "self",
        &["environment"],
        &["write"],
    ));
    let actor = key(&w.s.rig.world, w.s.ada.id(), policy).await?;
    let (_, parent) = confirm(&w, &actor.secret, preflight).await?;
    let (environment,child)=tokio::time::timeout(rig::PATIENCE,async{loop{
        let found:Option<(Id,Id)>=sqlx::query_as("SELECT e.id,c.id FROM jobs p JOIN environments e ON e.id=(p.plan->>'new_environment')::uuid JOIN jobs c ON c.environment_id=e.id AND c.kind='environment_provision' WHERE p.id=$1 AND c.plan ? 'environment_lifecycle'")
            .bind(parent).fetch_optional(w.s.rig.db.pool()).await.unwrap();
        if let Some(found)=found{break found;}
        tokio::task::yield_now().await;
    }}).await?;
    let row = orch_db::queries::jobs::job(&mut *w.s.rig.world.conn().await, child)
        .await?
        .ok_or("native provision missing")?;
    assert_eq!(row.environment_id, Some(environment));
    assert_eq!(row.created_by, Some(w.s.ada.id()));
    assert_eq!(row.plan["mode"], "make");
    assert_eq!(row.plan["environment_lifecycle_version"], 1);
    assert!(
        row.plan.get("creator_provision").is_none(),
        "the parent never becomes another fork operation"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM threads WHERE environment_id=$1")
        .bind(environment)
        .fetch_one(w.s.rig.db.pool())
        .await?;
    assert_eq!(count, 0, "the real make child precedes the target binding");
    let clock = w.s.rig.world.app.app.clock.clone();
    let access = orch_api_core::thread_access::AuthzAccess::new(clock.clone());
    let mut tx = w.s.rig.db.begin().await?;
    let current =
        orch_engine::jobs::provision::creator_checkpoint(&mut tx, &row, clock.as_ref(), &access)
            .await?
            .ok_or("lifecycle make source missing")?;
    assert_eq!(
        current.principal().caller().admitted_operation(),
        Some("EnvironmentRecoverCreate")
    );
    current.check(clock.as_ref())?;
    tx.rollback().await?;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(actor.id)
        .bind(w.s.rig.world.now())
        .execute(w.s.rig.db.pool())
        .await?;
    let mut tx = w.s.rig.db.begin().await?;
    assert!(matches!(
        orch_engine::jobs::provision::creator_checkpoint(&mut tx, &row, clock.as_ref(), &access)
            .await,
        Err(orch_engine::EngineError::BeforeEffect(_))
    ));
    tx.rollback().await?;
    let still = orch_db::queries::threads::thread(&mut *w.s.rig.world.conn().await, thread)
        .await?
        .ok_or("source thread missing")?;
    assert_eq!(still.environment_id, Some(w.s.on.environment));
    w.s.rig.world.app.app.engine.stop();
    Ok(())
}
