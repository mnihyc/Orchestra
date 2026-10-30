//! An org home's compact live tree names the wait and its actual reader, without exposing request contents or raw setup diagnostics.

mod places;
mod rig;
mod support;

use orch_api_contract::nodes::{
    OrgTreeGet, OrgTreeGetQuery, TreeBlockedCause, TreeThread, TreeWaiting, TreeWaitingKind,
};
use orch_db::queries::hierarchy;
use orch_types::{Id, new_id};
use rig::Scene;
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn compact(
    scene: &Scene,
    token: &str,
    thread: Id,
) -> Result<TreeThread, Box<dyn std::error::Error>> {
    let reply = scene
        .rig
        .world
        .app
        .call(OrgTreeGet, &[&scene.tree.org.base.id])
        .token(token)
        .query(&OrgTreeGetQuery::default())
        .send()
        .await
        .ok();
    reply
        .readable()
        .into_iter()
        .find(|row| row.id == thread)
        .and_then(|row| row.thread.clone())
        .ok_or_else(|| "the readable thread is in the compact tree".into())
}

#[tokio::test]
async fn the_home_says_your_approval_only_to_the_person_who_can_decide_and_names_questions_separately()
-> TestResult {
    let scene = Scene::new().await;
    let thread = scene.thread().await.base.id;
    let now = scene.rig.world.now();
    let request = new_id();
    let mut conn = scene.rig.world.conn().await;
    sqlx::query("INSERT INTO approvals(id,kind,node_id,thread_id,title,request,required_permission,created_at,updated_at)
        VALUES($1,'harness',$2,$2,'Deploy staging',$3,'thread.manage',$4,$4)")
        .bind(request).bind(thread).bind(json!({"request_kind":"command","detail":{"command":"deploy","cwd":"/work","reason":null},"options":[]})).bind(now)
        .execute(&mut *conn).await?;
    hierarchy::stamp(&mut conn, thread).await?;
    drop(conn);
    let owner = compact(&scene, &scene.ada.token, thread).await?;
    assert_eq!(
        owner.waiting,
        Some(TreeWaiting {
            kind: TreeWaitingKind::Approval,
            for_caller: true
        })
    );
    let viewer = compact(&scene, &scene.cai.token, thread).await?;
    assert_eq!(
        viewer.waiting,
        Some(TreeWaiting {
            kind: TreeWaitingKind::Approval,
            for_caller: false
        })
    );
    let mut conn = scene.rig.world.conn().await;
    sqlx::query("UPDATE approvals SET request=$2 WHERE id=$1").bind(request)
        .bind(json!({"request_kind":"user_input","detail":{"questions":[{"id":"origin","header":"Origin","question":"Which origin?","options":null,"multiple":false,"allow_other":true}]},"options":[]}))
        .execute(&mut *conn).await?;
    hierarchy::stamp(&mut conn, thread).await?;
    drop(conn);
    assert_eq!(
        compact(&scene, &scene.ada.token, thread).await?.waiting,
        Some(TreeWaiting {
            kind: TreeWaitingKind::Answer,
            for_caller: true
        })
    );
    let mut conn = scene.rig.world.conn().await;
    sqlx::query("UPDATE approvals SET state='approved',decided_at=$2,updated_at=$2 WHERE id=$1")
        .bind(request)
        .bind(now)
        .execute(&mut *conn)
        .await?;
    hierarchy::stamp(&mut conn, thread).await?;
    drop(conn);
    assert!(
        compact(&scene, &scene.ada.token, thread)
            .await?
            .waiting
            .is_none(),
        "an answered request stops asking this reader"
    );
    Ok(())
}

#[tokio::test]
async fn a_thread_manager_is_not_called_the_approver_of_an_environments_capability() -> TestResult {
    let scene = Scene::new().await;
    let thread = scene.thread().await.base.id;
    let manager = scene.rig.person("threadmanager").await;
    let porter = scene.rig.person("portsmanager").await;
    for person in [&manager, &porter] {
        places::join(
            &scene.rig.world,
            scene.tree.org.base.id,
            scene.tree.org.base.id,
            person,
            "viewer",
            &scene.ada,
        )
        .await;
    }
    scene
        .rig
        .lets_use("node", thread, "thread.manage", &manager, json!({}))
        .await;
    scene
        .rig
        .lets_use(
            "environment",
            scene.on.environment,
            "ports.manage",
            &porter,
            json!({}),
        )
        .await;
    let mut conn = scene.rig.world.conn().await;
    sqlx::query("INSERT INTO approvals(id,kind,node_id,thread_id,environment_id,requester_thread_id,title,request,required_permission,approver_scope,created_at,updated_at)
        VALUES($1,'capability',$2,$2,$3,$2,'Open a port',$4,'ports.manage','environment',$5,$5)")
        // The request as the engine keeps it: the environment named in full, as the contract reads it.
        .bind(new_id()).bind(thread).bind(scene.on.environment).bind(json!({"capability":"port_place","label":"Open a port to a place","risk":"medium","reason":"Show the result",
            "environment":{"id":scene.on.environment,"name":"Workspace",
                "owner":{"id":scene.ada.id(),"kind":"human","username":scene.ada.person.username,"display_name":"Ada","avatar":{"text":"A","color":1}},
                "address":"acme.workspace","ports_exposed":false,"placement":"managed","status":"running"},
            "host":{"id":new_id(),"name":"Host","address":"system.Host"},"consequences":[]}))
        .bind(scene.rig.world.now()).execute(&mut *conn).await?;
    hierarchy::stamp(&mut conn, thread).await?;
    drop(conn);
    assert_eq!(
        compact(&scene, &manager.token, thread).await?.waiting,
        Some(TreeWaiting {
            kind: TreeWaitingKind::Approval,
            for_caller: false
        })
    );
    assert_eq!(
        compact(&scene, &porter.token, thread).await?.waiting,
        Some(TreeWaiting {
            kind: TreeWaitingKind::Approval,
            for_caller: true
        })
    );
    Ok(())
}

#[tokio::test]
async fn the_compact_reason_classifies_a_sandbox_refusal_without_leaking_its_settings_keys_and_ignores_expired_requests()
-> TestResult {
    let scene = Scene::new().await;
    let thread = scene.thread().await.base.id;
    let mut conn = scene.rig.world.conn().await;
    sqlx::query("UPDATE threads SET state='blocked',blocked_reason='binding_invalid',blocked_detail=$2 WHERE node_id=$1")
        .bind(thread).bind(orch_engine::sessions::SANDBOX_UNSUPPORTED).execute(&mut *conn).await?;
    sqlx::query("INSERT INTO approvals(id,kind,node_id,thread_id,title,request,required_permission,expires_at,created_at,updated_at)
        VALUES($1,'harness',$2,$2,'Already expired',$3,'thread.manage',$4,$4,$4)")
        .bind(new_id()).bind(thread).bind(json!({"request_kind":"command","detail":{"command":"deploy","cwd":"/work","reason":null},"options":[]}))
        .bind(scene.rig.world.now()).execute(&mut *conn).await?;
    hierarchy::stamp(&mut conn, thread).await?;
    drop(conn);
    let shown = compact(&scene, &scene.cai.token, thread).await?;
    assert_eq!(shown.blocked_cause, Some(TreeBlockedCause::Sandbox));
    assert!(shown.waiting.is_none());
    let encoded = serde_json::to_string(&shown)?;
    assert!(!encoded.contains("sandbox.mode") && !encoded.contains("agent.max_sandbox"));
    assert!(!encoded.contains("blocked_detail"));
    Ok(())
}
