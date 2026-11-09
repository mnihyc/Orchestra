//! Two unbound input controls and one already-bound HTTP-footprint debt, over real native sources.
//! All requests/keys are actual stored fixture credentials; raw replies compile on Before/After.
use super::*;

#[path = "control_first_effect.rs"]
mod first_effect;
use http::StatusCode;
use orch_access_contract::{key_scope::KeyScope, permissions::Permission};
use orch_api_contract::threads::{ThreadInputRun, ThreadPlanDecide, ThreadPlanDecideBody};
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_authz::{Act, Caller, Policy, Principal};
use orch_db::queries::identity;
use orch_link_contract::{
    events::{EmittedItemKind, Event, ItemCompleted, ItemStarted},
    foreign::ItemPayload,
};
use orch_types::{Json, Time};
use std::time::Duration;

const PLAN_WRITE: &str = "ThreadPlanDecide.plan_interact";
const PLAN_MANAGE: &str = "ThreadPlanDecide.plan_manage";
const BYPASS: &str = "ThreadInputBypass";
const RUN: &str = "ThreadInputRun";
const WORDS: &str = "PRIVATE EXISTING INPUT WORDS: retain original author";

struct Key {
    id: Id,
    secret: String,
}
fn rule(kind: &str, id: Id, resource: &str, operation: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[resource],
        "allow":[{"operation":operation,"actions":actions}]})
}
async fn key(s: &Scene, actor: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *s.rig.world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: actor,
            name: "Native thread control fixture",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: s.rig.world.now(),
            expires_at: Some(s.rig.world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    Key { id, secret }
}
fn checked(reply: RawReply, status: StatusCode, stage: &str) -> Json {
    let body = if reply.raw.is_empty() {
        Json::Null
    } else {
        serde_json::from_slice(&reply.raw).unwrap()
    };
    assert_eq!(reply.status, status, "{stage}: {body}");
    body
}
async fn has(s: &Scene, actor: Id, thread: Id, permission: Permission) -> bool {
    let mut conn = s.rig.world.conn().await;
    let p = Principal::load(
        &mut conn,
        Caller::session(actor),
        Time::new(s.rig.world.now()),
    )
    .await
    .unwrap();
    orch_authz::nodes::check(
        &mut conn,
        &p,
        thread,
        permission,
        Act::Use,
        Policy::default(),
    )
    .await
    .unwrap()
    .is_allowed()
}
async fn snapshot(s: &Scene, thread: Id) -> Json {
    sqlx::query_scalar("SELECT jsonb_build_object('seq',t.next_seq,'inputs',
        (SELECT jsonb_agg(jsonb_build_object('id',i.id,'author',i.author_id,'run_as',i.run_as,
         'run_authority',i.run_as_authority,'state',i.state,'detail',i.state_detail,'bypass',i.limit_bypass)
         ORDER BY i.id) FROM thread_inputs i WHERE i.thread_id=t.node_id),
        'approvals',(SELECT jsonb_agg(jsonb_build_object('id',a.id,'state',a.state,'by',a.decided_by,'decision',a.decision)
         ORDER BY a.id) FROM approvals a WHERE a.thread_id=t.node_id)) FROM threads t WHERE t.node_id=$1")
        .bind(thread).fetch_one(s.rig.db.pool()).await.unwrap()
}
async fn hold_queue(s: &Scene, thread: Id) {
    sqlx::query("UPDATE threads SET queue_held=true WHERE node_id=$1")
        .bind(thread)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
}
struct Plan {
    thread: Id,
    item: Id,
    approval: Id,
}
async fn plan(s: &Scene, name: &str) -> Plan {
    sqlx::query("UPDATE agents SET descriptor_overrides=jsonb_set(COALESCE(descriptor_overrides,'{}'::jsonb),'{capabilities}',COALESCE(descriptor_overrides->'capabilities','{}'::jsonb)||'{\"plan_mode\":true}'::jsonb) WHERE id=$1")
        .bind(s.on.agent).execute(s.rig.db.pool()).await.unwrap();
    let thread = s.thread_by(&s.ada, name).await.base.id;
    // Cai stays an ordinary Viewer with only explicit Interact/model-use, so
    // this proves the native interactor alternative independently of Manage.
    s.rig
        .lets_use("node", thread, "thread.interact", &s.cai, json!({}))
        .await;
    // A scene makes more than one real Plan. Provider/use is shared by those
    // targets, so retain the existing IDENTICAL fixture authorizer exactly once.
    let already_given: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM permission_entries WHERE object_kind='provider'
         AND object_id=$1 AND page='live' AND profile='any' AND permission='provider.use'
         AND holder_kind='user' AND holder_id=$2 AND holder_role IS NULL
         AND removed_at IS NULL AND expires_at IS NULL AND options='{}'::jsonb)",
    )
    .bind(s.on.provider)
    .bind(s.cai.id())
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    if !already_given {
        s.rig
            .lets_use("provider", s.on.provider, "provider.use", &s.cai, json!({}))
            .await;
    }
    s.rig
        .engine
        .submit_input(
            &Caller::session(s.ada.id()),
            thread,
            orch_engine::inputs::InputRequest {
                content: text("Propose a concrete plan"),
                delivery: Delivery::Queue,
                client_id: format!("plan:{name}"),
                bypass: None,
                plan: true,
            },
        )
        .await
        .unwrap();
    let start = s.rig.turn_start(thread, 0).await;
    assert!(start.plan);
    s.rig.say(thread, rig::turn_began(start.turn_id)).await;
    let payload=ItemPayload::Plan(serde_json::from_value(json!({"explanation":"PRIVATE ORIGINAL PLAN", "steps":[{"text":"Inspect source","status":"pending"}]})).unwrap());
    s.rig
        .say(
            thread,
            Event::ItemStarted(ItemStarted {
                turn_id: start.turn_id,
                native_ref: "real-plan".into(),
                stream: "main".into(),
                parent_native_ref: None,
                kind: EmittedItemKind::Plan,
                payload: payload.clone(),
            }),
        )
        .await;
    s.rig
        .say(
            thread,
            Event::ItemCompleted(ItemCompleted {
                native_ref: "real-plan".into(),
                status: orch_agent_contract::events::ItemStatus::Completed,
                payload,
                output_blob: None,
            }),
        )
        .await;
    s.rig
        .say(
            thread,
            rig::turn_ended(start.turn_id, TurnOutcome::Completed),
        )
        .await;
    s.rig.wait_turn(start.turn_id, "completed").await;
    rig::eventually("actual completed Plan source has its linked Approval",||async {
        sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM approvals a JOIN items i ON i.id=a.object_id WHERE a.thread_id=$1 AND a.kind='plan' AND i.kind='plan')")
            .bind(thread).fetch_one(s.rig.db.pool()).await.unwrap()
    }).await;
    let (approval,item)=sqlx::query_as("SELECT a.id,a.object_id FROM approvals a JOIN items i ON i.id=a.object_id WHERE a.thread_id=$1 AND a.kind='plan' AND i.kind='plan'")
        .bind(thread).fetch_one(s.rig.db.pool()).await.unwrap();
    hold_queue(s, thread).await;
    // A terminal row precedes its retained follow-up and workspace invalidations.
    // Finish that authored work before a refusal's exact no-effect baseline.
    rig::eventually(
        "the plan's terminal and workspace work have committed",
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT end_work IS NULL AND NOT files_end_due AND NOT files_work_pending
             FROM turns WHERE id=$1",
            )
            .bind(start.turn_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
        },
    )
    .await;
    Plan {
        thread,
        item,
        approval,
    }
}
async fn decide(s: &Scene, p: &Plan, token: &str, proceed: bool) -> RawReply {
    s.rig
        .world
        .app
        .call(ThreadPlanDecide, &[&p.thread, &p.item])
        .token(token)
        .body(&ThreadPlanDecideBody {
            proceed,
            text: (!proceed).then(|| "Include rollback checks".into()),
        })
        .send_bytes()
        .await
}
async fn ordinary_plan(s: &Scene, managers: bool) -> Plan {
    let p = plan(s, "Ordinary plan premise").await;
    let who = if managers { &s.ada } else { &s.cai };
    assert!(has(s, who.id(), p.thread, Permission::ThreadInteract).await);
    if !managers {
        assert!(!has(s, who.id(), p.thread, Permission::ThreadManage).await);
    }
    let body = checked(
        decide(s, &p, &who.token, true).await,
        StatusCode::OK,
        "ordinary HTTP plan positive before modern scope",
    );
    assert_eq!(body["input"]["thread_id"], json!(p.thread));
    p
}
async fn plan_scope(managers: bool, full: bool) {
    let s = Scene::new().await;
    if managers {
        s.rig
            .world
            .set("thread.plan_answered_by", json!("managers"))
            .await;
    }
    ordinary_plan(&s, managers).await;
    let p = plan(&s, "Scoped plan target").await;
    let who = if managers { &s.ada } else { &s.cai };
    let op = if managers { PLAN_MANAGE } else { PLAN_WRITE };
    let action = if managers { "manage" } else { "write" };
    let mut actions = vec![action];
    if full {
        actions.push("read");
    }
    let k = key(
        &s,
        who.id(),
        vec![rule("thread", p.thread, "thread", op, &actions)],
    )
    .await;
    let body = checked(
        decide(&s, &p, &k.secret, !managers).await,
        StatusCode::OK,
        "modern actual plan decision",
    );
    let (input,source,approval_state):(Id,Json,String)=sqlx::query_as("SELECT i.id,i.submission_authority,a.state FROM thread_inputs i JOIN approvals a ON a.id=$2 WHERE i.thread_id=$1 AND i.client_id=$3")
        .bind(p.thread).bind(p.approval).bind(format!("plan:{}",p.item)).fetch_one(s.rig.db.pool()).await.unwrap();
    assert_eq!(source["origins"][0]["operation"], op);
    assert_eq!(
        approval_state,
        if managers { "answered" } else { "approved" }
    );
    if full {
        assert_eq!(body["input"]["id"], json!(input));
        assert_eq!(body["input"]["plan"], json!(managers));
    } else {
        assert_eq!(body["projection"], "receipt");
        assert_eq!(body["input_id"], json!(input));
        assert!(body.get("input").is_none());
        assert!(body.get("decision").is_none());
    }
}
#[tokio::test]
async fn plan_interactor_own_write_and_read_retain_normal_full_reply() {
    plan_scope(false, true).await;
}
#[tokio::test]
async fn plan_manager_own_manage_and_read_keep_planning_full_reply() {
    plan_scope(true, true).await;
}
#[tokio::test]
async fn plan_interactor_write_only_returns_actual_acceptance_receipt() {
    plan_scope(false, false).await;
}
#[tokio::test]
async fn plan_manager_manage_only_returns_actual_acceptance_receipt() {
    plan_scope(true, false).await;
}
async fn plan_refusal(managers: bool, wrong_target: bool) {
    let s = Scene::new().await;
    if managers {
        s.rig
            .world
            .set("thread.plan_answered_by", json!("managers"))
            .await;
    }
    ordinary_plan(&s, managers).await;
    let p = plan(&s, "Refused plan target").await;
    let target = if wrong_target {
        s.thread_by(&s.ada, "Different actual Thread").await.base.id
    } else {
        p.thread
    };
    let op = if managers { PLAN_MANAGE } else { PLAN_WRITE };
    let action = if managers { "write" } else { "manage" };
    let actions = if wrong_target {
        vec!["write", "manage", "read"]
    } else {
        vec![action, "read"]
    };
    let k = key(
        &s,
        s.ada.id(),
        vec![rule("thread", target, "thread", op, &actions)],
    )
    .await;
    let before = snapshot(&s, p.thread).await;
    checked(
        decide(&s, &p, &k.secret, true).await,
        StatusCode::FORBIDDEN,
        "wrong own action/actual target plan denial",
    );
    assert_eq!(snapshot(&s, p.thread).await, before);
}
#[tokio::test]
async fn plan_manage_action_does_not_borrow_write() {
    plan_refusal(false, false).await;
}
#[tokio::test]
async fn plan_write_action_does_not_borrow_manage() {
    plan_refusal(true, false).await;
}
#[tokio::test]
async fn plan_scope_names_the_actual_thread_not_another_thread() {
    plan_refusal(false, true).await;
}

struct Input {
    thread: Id,
    input: Id,
    approval: Option<Id>,
    homes: Vec<(String, Id)>,
    caps: Vec<Id>,
}
async fn held_input(s: &Scene, name: &str) -> Input {
    let thread = s.thread_by(&s.ada, name).await.base.id;
    let input = s
        .say(&s.dan, thread, WORDS, Delivery::Queue, name)
        .await
        .ok()
        .input;
    assert_eq!(input.state_detail.as_deref(), Some("model_use_required"));
    assert_eq!(input.author.as_ref().unwrap().id, s.dan.id());
    let approval = input
        .approval_id
        .expect("normal held input has its source approval");
    hold_queue(s, thread).await;
    Input {
        thread,
        input: input.id,
        approval: Some(approval),
        homes: Vec::new(),
        caps: Vec::new(),
    }
}
async fn bypass_input(s: &Scene, name: &str, mixed: bool) -> Input {
    let thread = s.thread_by(&s.ada, name).await.base.id;
    hold_queue(s, thread).await;
    let mut homes = vec![("provider".into(), s.on.provider)];
    let mut caps = vec![new_id()];
    sqlx::query("INSERT INTO usage_caps(id,provider_id,subject_kind,split,meter,amount,at_limit,created_by) VALUES($1,$2,'everyone','shared','running',0,'stop',$3)")
        .bind(caps[0]).bind(s.on.provider).bind(s.ada.id()).execute(s.rig.db.pool()).await.unwrap();
    if mixed {
        let cap = new_id();
        sqlx::query("INSERT INTO usage_caps(id,node_id,subject_kind,split,meter,amount,at_limit,created_by) VALUES($1,$2,'everyone','shared','running',0,'stop',$3)")
        .bind(cap).bind(s.tree.topic.base.id).bind(s.ada.id()).execute(s.rig.db.pool()).await.unwrap();
        caps.push(cap);
        homes.push(("topic".into(), s.tree.topic.base.id));
    }
    let input = s
        .say(&s.eve, thread, WORDS, Delivery::Queue, name)
        .await
        .ok()
        .input;
    assert_eq!(input.author.as_ref().unwrap().id, s.eve.id());
    let row = orch_db::queries::threads::thread(&mut *s.rig.world.conn().await, thread)
        .await
        .unwrap()
        .unwrap();
    let selected = s
        .rig
        .engine
        .input_bypass_subjects(
            &mut *s.rig.world.conn().await,
            &row,
            s.eve.id(),
            &LimitBypassRequest {
                caps: Some(caps.clone()),
                ..Default::default()
            },
            s.rig.world.now(),
        )
        .await
        .unwrap();
    assert_eq!(
        selected.len(),
        homes.len(),
        "normal selector proves every real reached cap home"
    );
    Input {
        thread,
        input: input.id,
        approval: None,
        homes,
        caps,
    }
}
async fn bypass(s: &Scene, i: &Input, token: &str, caps: Option<Vec<Id>>) -> RawReply {
    s.rig
        .world
        .app
        .call(ThreadInputBypass, &[&i.thread, &i.input])
        .token(token)
        .body(&LimitBypassRequest {
            caps,
            ..Default::default()
        })
        .send_bytes()
        .await
}
async fn run(s: &Scene, i: &Input, token: &str) -> RawReply {
    s.rig
        .world
        .app
        .call(ThreadInputRun, &[&i.thread, &i.input])
        .token(token)
        .send_bytes()
        .await
}
fn bypass_rules(i: &Input, full: bool, all: bool) -> Vec<Json> {
    let mut rules = vec![rule(
        "thread",
        i.thread,
        "thread",
        BYPASS,
        if full { &["write", "read"] } else { &["write"] },
    )];
    for (index, (kind, id)) in i.homes.iter().enumerate() {
        if all || index == 0 {
            rules.push(rule(kind, *id, kind, BYPASS, &["manage"]));
        }
    }
    rules
}
async fn bypass_scope(mixed: bool, full: bool) {
    let s = Scene::new().await;
    let ordinary = bypass_input(&s, "Ordinary cap bypass", mixed).await;
    checked(
        bypass(&s, &ordinary, &s.ada.token, Some(ordinary.caps.clone())).await,
        StatusCode::OK,
        "ordinary actual cap bypass positive",
    );
    let i = bypass_input(&s, "Scoped cap bypass", mixed).await;
    let k = key(&s, s.ada.id(), bypass_rules(&i, full, true)).await;
    let body = checked(
        bypass(&s, &i, &k.secret, Some(i.caps.clone())).await,
        StatusCode::OK,
        "modern exact cap bypass homes",
    );
    let raw: Json = sqlx::query_scalar("SELECT limit_bypass FROM thread_inputs WHERE id=$1")
        .bind(i.input)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    let mine = raw
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["by"] == json!(s.ada.id()))
        .unwrap();
    for cap in i.caps {
        assert!(mine["caps"].as_array().unwrap().contains(&json!(cap)));
    }
    if full {
        assert_eq!(body["id"], json!(i.input));
        assert_eq!(body["content"][0]["text"], WORDS);
    } else {
        assert_eq!(body["projection"], "receipt");
        assert_eq!(body["input_id"], json!(i.input));
        assert!(body.get("content").is_none());
        assert!(body.get("author").is_none());
    }
}
#[tokio::test]
async fn bypass_provider_home_manage_and_own_thread_read_keep_full_input() {
    bypass_scope(false, true).await;
}
#[tokio::test]
async fn bypass_mixed_provider_and_topic_homes_are_both_required() {
    bypass_scope(true, true).await;
}
#[tokio::test]
async fn bypass_write_and_reached_manage_without_read_return_receipt() {
    bypass_scope(false, false).await;
}
#[tokio::test]
async fn bypass_missing_one_actual_mixed_home_refuses_without_effects() {
    let s = Scene::new().await;
    let i = bypass_input(&s, "Mixed cap refusal", true).await;
    let k = key(&s, s.ada.id(), bypass_rules(&i, true, false)).await;
    let before = snapshot(&s, i.thread).await;
    checked(
        bypass(&s, &i, &k.secret, Some(i.caps.clone())).await,
        StatusCode::FORBIDDEN,
        "missing actual Topic manage",
    );
    assert_eq!(snapshot(&s, i.thread).await, before);
}
#[tokio::test]
async fn bypass_empty_selection_clears_only_own_entry_without_fabricated_manage() {
    let s = Scene::new().await;
    let i = bypass_input(&s, "Clear accepted own bypass", false).await;
    checked(
        bypass(&s, &i, &s.ada.token, Some(i.caps.clone())).await,
        StatusCode::OK,
        "ordinary existing bypass before empty clear",
    );
    let k = key(
        &s,
        s.ada.id(),
        vec![rule(
            "thread",
            i.thread,
            "thread",
            BYPASS,
            &["write", "read"],
        )],
    )
    .await;
    checked(
        bypass(&s, &i, &k.secret, None).await,
        StatusCode::OK,
        "true empty bypass homes",
    );
    let raw: Json = sqlx::query_scalar("SELECT limit_bypass FROM thread_inputs WHERE id=$1")
        .bind(i.input)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert!(
        raw.as_array()
            .unwrap()
            .iter()
            .all(|e| e["by"] != json!(s.ada.id()))
    );
}
async fn run_scope(full: bool) {
    let s = Scene::new().await;
    let ordinary = held_input(&s, "Ordinary model approver").await;
    checked(
        run(&s, &ordinary, &s.eve.token).await,
        StatusCode::OK,
        "ordinary current model-use actor positive",
    );
    let i = held_input(&s, "Scoped model approver").await;
    let k = key(
        &s,
        s.eve.id(),
        vec![rule(
            "thread",
            i.thread,
            "thread",
            RUN,
            if full { &["write", "read"] } else { &["write"] },
        )],
    )
    .await;
    let body = checked(
        run(&s, &i, &k.secret).await,
        StatusCode::OK,
        "modern same-operation run",
    );
    let (author, actor, origin): (Option<Id>, Option<Id>, Json) =
        sqlx::query_as("SELECT author_id,run_as,run_as_authority FROM thread_inputs WHERE id=$1")
            .bind(i.input)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(author, Some(s.dan.id()));
    assert_eq!(actor, Some(s.eve.id()));
    assert_eq!(origin["origins"][0]["operation"], RUN);
    if full {
        assert_eq!(body["id"], json!(i.input));
        assert_eq!(body["content"][0]["text"], WORDS);
    } else {
        assert_eq!(body["projection"], "receipt");
        assert_eq!(body["input_id"], json!(i.input));
        assert!(body.get("content").is_none());
        assert!(body.get("author").is_none());
    }
}
#[tokio::test]
async fn already_bound_run_keeps_original_author_and_actual_full_model_actor() {
    run_scope(true).await;
}
#[tokio::test]
async fn run_write_only_does_not_disclose_the_original_authors_words() {
    run_scope(false).await;
}

#[tokio::test]
async fn plan_scope_does_not_grant_an_ordinary_stranger_decision_authority() {
    let s = Scene::new().await;
    ordinary_plan(&s, false).await;
    let p = plan(&s, "Stranger cannot become decider").await;
    assert!(!has(&s, s.bea.id(), p.thread, Permission::ThreadInteract).await);
    checked(
        decide(&s, &p, &s.bea.token, true).await,
        StatusCode::FORBIDDEN,
        "ordinary stranger baseline",
    );
    let before = snapshot(&s, p.thread).await;
    let k = key(
        &s,
        s.bea.id(),
        vec![rule(
            "thread",
            p.thread,
            "thread",
            PLAN_WRITE,
            &["write", "read"],
        )],
    )
    .await;
    checked(
        decide(&s, &p, &k.secret, true).await,
        StatusCode::FORBIDDEN,
        "scope cannot grant ordinary plan rights",
    );
    assert_eq!(snapshot(&s, p.thread).await, before);
}
#[tokio::test]
async fn bypass_scope_does_not_grant_an_ordinary_provider_manager_right() {
    let s = Scene::new().await;
    let i = bypass_input(&s, "Ordinary cap right retained", false).await;
    checked(
        bypass(&s, &i, &s.eve.token, Some(i.caps.clone())).await,
        StatusCode::FORBIDDEN,
        "ordinary developer lacks actual cap-pass right",
    );
    let before = snapshot(&s, i.thread).await;
    let k = key(&s, s.eve.id(), bypass_rules(&i, true, true)).await;
    checked(
        bypass(&s, &i, &k.secret, Some(i.caps.clone())).await,
        StatusCode::FORBIDDEN,
        "key scope is not ordinary cap authority",
    );
    assert_eq!(snapshot(&s, i.thread).await, before);
}
#[tokio::test]
async fn an_accepted_bypass_keeps_its_native_grant_lifetime_after_the_giving_key_is_revoked() {
    let s = Scene::new().await;
    let i = bypass_input(&s, "Accepted independent cap grant", false).await;
    let k = key(&s, s.ada.id(), bypass_rules(&i, true, true)).await;
    checked(
        bypass(&s, &i, &k.secret, Some(i.caps.clone())).await,
        StatusCode::OK,
        "accepted separate bypass key",
    );
    let before: Json = sqlx::query_scalar("SELECT limit_bypass FROM thread_inputs WHERE id=$1")
        .bind(i.input)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(before[0]["by"], json!(s.ada.id()));
    assert!(s.rig.seen.of_kind(CommandKind::TurnStart).is_empty());
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(k.id)
        .bind(s.rig.world.now())
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let revision: i64 = sqlx::query_scalar("SELECT queue_revision FROM threads WHERE node_id=$1")
        .bind(i.thread)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    checked(
        s.rig
            .world
            .app
            .call(ThreadQueueResume, &[&i.thread])
            .token(&s.eve.token)
            .body(&ThreadQueueResumeBody { revision })
            .send_bytes()
            .await,
        StatusCode::OK,
        "original model actor resumes ordinary queue",
    );
    let start = s.rig.turn_start(i.thread, 0).await;
    let (author, run_as, bypass): (Option<Id>, Option<Id>, Json) =
        sqlx::query_as("SELECT author_id,run_as,limit_bypass FROM thread_inputs WHERE id=$1")
            .bind(i.input)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(author, Some(s.eve.id()));
    assert!(run_as.is_none());
    assert_eq!(bypass, before);
    assert_eq!(start.thread_id, i.thread);
}

#[derive(Clone, Copy, Debug)]
enum Work {
    Plan,
    Run,
    Bypass,
}
#[derive(Clone, Copy, Debug)]
enum Wait {
    Thread,
    Approval,
}

/// The exact waiting HTTP transaction is identified by the real blocker chain AND
/// its credential-table row lock. Background preflight has no access-token identity,
/// so it cannot stand in for the request before authentication/body checks complete.
async fn blocked_http(
    s: &Scene,
    locker: i32,
    session: bool,
) -> Result<i32, tokio::time::error::Elapsed> {
    tokio::time::timeout(Duration::from_secs(10),async {
        loop {
            let pid:Option<i32>=sqlx::query_scalar("SELECT a.pid FROM pg_stat_activity a
                WHERE a.datname=current_database() AND a.pid<>pg_backend_pid()
                AND EXISTS(SELECT 1 FROM pg_locks l WHERE l.pid=a.pid AND l.locktype='relation'
                  AND l.mode='RowShareLock' AND l.relation=CASE WHEN $2 THEN 'access_tokens'::regclass ELSE 'api_tokens'::regclass END)
                AND EXISTS(WITH RECURSIVE chain(pid) AS (
                   SELECT unnest(pg_blocking_pids(a.pid)) UNION
                   SELECT unnest(pg_blocking_pids(c.pid)) FROM chain c)
                   SELECT 1 FROM chain WHERE pid=$1)
                ORDER BY a.pid LIMIT 1")
                .bind(locker).bind(session).fetch_optional(s.rig.db.pool()).await.unwrap();
            if let Some(pid)=pid{return pid;}
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await
}
async fn legacy(s: &Scene, actor: Id) -> Key {
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token(
        &mut *s.rig.world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: actor,
            name: "Actual legacy thread-control deadline",
            token_hash: &token_hash(&secret),
            scopes: &["*".into()],
            node_id: None,
            at: s.rig.world.now(),
            expires_at: Some(s.rig.world.now() + chrono::Duration::seconds(1)),
        },
    )
    .await
    .unwrap();
    Key { id, secret }
}
async fn deadline(work: Work, wait: Wait) {
    let s = Scene::new().await;
    // Explicit successful ordinary samepath first; all later requests still target
    // a fresh pending source rather than replaying a decision already consumed.
    let (thread, operand, approval, caps, actor) = match work {
        Work::Plan => {
            ordinary_plan(&s, false).await;
            let p = plan(&s, "Plan deadline target").await;
            (p.thread, p.item, Some(p.approval), Vec::new(), s.eve.id())
        }
        Work::Run => {
            let first = held_input(&s, "Run deadline positive").await;
            checked(
                run(&s, &first, &s.eve.token).await,
                StatusCode::OK,
                "ordinary run before deadline premise",
            );
            let i = held_input(&s, "Run deadline target").await;
            (i.thread, i.input, i.approval, Vec::new(), s.eve.id())
        }
        Work::Bypass => {
            let first = bypass_input(&s, "Bypass deadline positive", false).await;
            checked(
                bypass(&s, &first, &s.ada.token, Some(first.caps.clone())).await,
                StatusCode::OK,
                "ordinary bypass before deadline premise",
            );
            let i = bypass_input(&s, "Bypass deadline target", false).await;
            (i.thread, i.input, None, i.caps, s.ada.id())
        }
    };
    let session = !matches!(work, Work::Bypass);
    let token = if session {
        let token = s.eve.token.clone();
        assert_eq!(sqlx::query("UPDATE sessions SET expires_at=$2 WHERE id=(SELECT session_id FROM access_tokens WHERE token_hash=$1)")
            .bind(token_hash(&token)).bind(s.rig.world.now()+chrono::Duration::seconds(1)).execute(s.rig.db.pool()).await.unwrap().rows_affected(),1);
        token
    } else {
        legacy(&s, actor).await.secret
    };
    let before = snapshot(&s, thread).await;
    let mut held = s.rig.db.pool().begin().await.unwrap();
    let locker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    match wait {
        Wait::Thread => {
            sqlx::query("SELECT node_id FROM threads WHERE node_id=$1 FOR UPDATE")
                .bind(thread)
                .execute(&mut *held)
                .await
                .unwrap();
        }
        Wait::Approval => {
            sqlx::query("SELECT id FROM approvals WHERE id=$1 FOR UPDATE")
                .bind(approval.expect("normal source has actual approval"))
                .execute(&mut *held)
                .await
                .unwrap();
        }
    }
    let app = s.rig.world.app.clone();
    let mut pending = tokio::spawn(async move {
        match work {
            Work::Plan => {
                let request = app
                    .call(ThreadPlanDecide, &[&thread, &operand])
                    .token(&token)
                    .body(&ThreadPlanDecideBody {
                        proceed: true,
                        text: None,
                    });
                request.send_bytes().await
            }
            Work::Run => {
                let request = app.call(ThreadInputRun, &[&thread, &operand]).token(&token);
                request.send_bytes().await
            }
            Work::Bypass => {
                let request = app
                    .call(ThreadInputBypass, &[&thread, &operand])
                    .token(&token)
                    .body(&LimitBypassRequest {
                        caps: Some(caps),
                        ..Default::default()
                    });
                request.send_bytes().await
            }
        }
    });
    tokio::select! {
        finished=&mut pending=>{held.rollback().await.unwrap();let reply=finished.unwrap();panic!("{work:?}/{wait:?} finished before the exact credential-holding native blocker premise: status={}",reply.status);}
        witness=blocked_http(&s,locker,session)=>{match witness{Ok(pid)=>eprintln!("thread-control phase=blocked work={work:?} wait={wait:?} request_pid={pid} locker_pid={locker}"),Err(_)=>{pending.abort();let _=pending.await;held.rollback().await.unwrap();panic!("no exact credential-holding native blocker chain for {work:?}/{wait:?}");}}}
    }
    s.rig.world.advance(chrono::Duration::seconds(2));
    held.commit().await.unwrap();
    let reply = match tokio::time::timeout(rig::PATIENCE, &mut pending).await {
        Ok(reply) => reply.unwrap(),
        Err(_) => {
            pending.abort();
            let _ = pending.await;
            panic!(
                "the released exact native writer did not answer within its existing fixture watchdog"
            );
        }
    };
    let after = snapshot(&s, thread).await;
    eprintln!(
        "thread-control phase=returned work={work:?} wait={wait:?} status={} effect_unchanged={}",
        reply.status,
        after == before
    );
    checked(
        reply,
        StatusCode::UNAUTHORIZED,
        "actual HTTP credential expires after native footprint wait",
    );
    assert_eq!(
        after, before,
        "expired HTTP identity cannot record a new plan/run/bypass effect"
    );
}
#[tokio::test]
async fn plan_http_session_expiry_after_actual_thread_wait_has_no_effect() {
    deadline(Work::Plan, Wait::Thread).await;
}
#[tokio::test]
async fn plan_http_session_expiry_after_actual_linked_approval_wait_has_no_effect() {
    deadline(Work::Plan, Wait::Approval).await;
}
#[tokio::test]
async fn run_http_session_expiry_after_actual_thread_wait_has_no_effect() {
    deadline(Work::Run, Wait::Thread).await;
}
#[tokio::test]
async fn run_http_session_expiry_after_actual_linked_approval_wait_has_no_effect() {
    deadline(Work::Run, Wait::Approval).await;
}
#[tokio::test]
async fn bypass_legacy_key_expiry_after_actual_thread_wait_has_no_effect() {
    deadline(Work::Bypass, Wait::Thread).await;
}

#[tokio::test]
async fn a_separate_same_account_bypass_key_does_not_demand_manage_from_the_original_create_key() {
    let s = Scene::new().await;
    let mut i = bypass_input(&s, "Same account independent accepted bypass", false).await;
    // Retire the ordinary fixture input through its actual native writer. The real
    // replacement below is authored by a separate key that has no bypass operation.
    checked(
        s.rig
            .world
            .app
            .call(ThreadInputCancel, &[&i.thread, &i.input])
            .token(&s.eve.token)
            .send_bytes()
            .await,
        StatusCode::OK,
        "ordinary fixture input cancelled before keyed author",
    );
    hold_queue(&s, i.thread).await;
    let original = key(
        &s,
        s.ada.id(),
        vec![rule(
            "thread",
            i.thread,
            "thread",
            "ThreadInputCreate",
            &["write", "read"],
        )],
    )
    .await;
    let accepted = checked(
        s.rig
            .world
            .app
            .call(orch_api_contract::threads::ThreadInputCreate, &[&i.thread])
            .token(&original.secret)
            .body(&input_request("independent-create-key", WORDS))
            .send_bytes()
            .await,
        StatusCode::CREATED,
        "real original create key has no bypass Manage",
    );
    i.input = serde_json::from_value(accepted["input"]["id"].clone()).unwrap();
    let giver = key(&s, s.ada.id(), bypass_rules(&i, true, true)).await;
    assert_ne!(original.id, giver.id);
    checked(
        bypass(&s, &i, &giver.secret, Some(i.caps.clone())).await,
        StatusCode::OK,
        "same account different bypass key accepted grant",
    );
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(giver.id)
        .bind(s.rig.world.now())
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let origin: Json =
        sqlx::query_scalar("SELECT submission_authority FROM thread_inputs WHERE id=$1")
            .bind(i.input)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(origin["origins"][0]["operation"], "ThreadInputCreate");
    let revision: i64 = sqlx::query_scalar("SELECT queue_revision FROM threads WHERE node_id=$1")
        .bind(i.thread)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    checked(
        s.rig
            .world
            .app
            .call(ThreadQueueResume, &[&i.thread])
            .token(&s.ada.token)
            .body(&ThreadQueueResumeBody { revision })
            .send_bytes()
            .await,
        StatusCode::OK,
        "ordinary queue resume retains accepted separate bypass",
    );
    let start = s.rig.turn_start(i.thread, 0).await;
    let actual: Id = sqlx::query_scalar("SELECT input_id FROM turns WHERE id=$1")
        .bind(start.turn_id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(
        actual, i.input,
        "the original create key still authorizes the real native input, not the bypass endpoint"
    );
}
