//! Native boundary governors and source-card decisions keep their existing producers.
use super::*;
use orch_access_contract::{decisions::ObjectRef, permissions::ResourceKind};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::{approvals as rows, resources::access_fixtures};
use orch_link_contract::{
    events::{EmittedItemKind, ItemCompleted, ItemStarted, TurnOutcome},
    foreign::ItemPayload,
};

async fn actual_host(s: &Scene) -> Id {
    actual_host_with_access(s, NewAccess::Defaults).await
}

async fn actual_host_with_access(s: &Scene, access: NewAccess) -> Id {
    let mut conn = s.rig.world.conn().await;
    let host = access_fixtures::insert_resource(
        &mut conn,
        "host",
        Some(s.ada.id()),
        Some(s.tree.org.base.id),
        "Approval host",
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Host.into(),
            id: host,
        },
        s.ada.id(),
        access,
        Time::new(s.rig.world.now()),
    )
    .await
    .unwrap();
    // The existing scene has selected this physical machine before its first runtime.
    // Give that exact enrolled machine its actual Host record, not a display-only random ID.
    sqlx::query("UPDATE hosts SET machine_id=$2 WHERE id=$1")
        .bind(host)
        .bind(s.rig.machine_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    let selected: Id = sqlx::query_scalar(
        "SELECT h.id FROM hosts h JOIN environments e ON e.machine_id=h.machine_id WHERE e.id=$1",
    )
    .bind(s.on.environment)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(selected, host);
    let (template, generation, driver, spec): (Id, i32, String, Json) = sqlx::query_as(
        "SELECT e.template_id,e.generation,t.driver,t.spec FROM environments e JOIN env_templates t ON t.id=e.template_id WHERE e.id=$1",
    )
    .bind(s.on.environment)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let runtime = new_id();
    let unit_name = format!("approval-{runtime}");
    let spec_hash = orch_types::Hash::of(&serde_json::to_vec(&spec).unwrap());
    // A connected rig machine alone does not declare a unit driver. The native Host
    // decision instead uses this actual owned placement. Keep creating: no fabricated
    // Ready state, connector enrollment, driver declaration or native ACK is needed.
    orch_db::queries::environments::hosting::insert_runtime(
        &mut conn,
        &orch_db::queries::environments::hosting::NewRuntime {
            id: runtime,
            host_id: host,
            unit_name: &unit_name,
            environment_id: s.on.environment,
            driver: &driver,
            template_id: template,
            spec_hash: spec_hash.as_bytes(),
            generation,
            volumes: &json!({}),
        },
        s.rig.world.now(),
    )
    .await
    .unwrap();
    let owned = orch_db::queries::environments::capabilities::host_of(&mut conn, s.on.environment)
        .await
        .unwrap()
        .expect("the ordinary Host resolver sees the real owned runtime");
    assert_eq!(owned.0, host);
    host
}
async fn capability(s: &Scene, thread: Id, host: Id, scope: &str, reference: &str) -> Id {
    let mut conn = s.rig.world.conn().await;
    let env =
        orch_db::queries::environments::thread_engine::environment(&mut conn, s.on.environment)
            .await
            .unwrap()
            .unwrap();
    let environment =
        orch_engine::views::environment_ref_json(&mut conn, &env, Time::new(s.rig.world.now()))
            .await
            .unwrap();
    let address = orch_authz::resources::addresses(&mut conn, &[host])
        .await
        .unwrap()
        .into_iter()
        .find(|(id, _)| *id == host)
        .expect("the actual host has its canonical address")
        .1;
    let request = json!({"capability":"port_place","label":"Open a port to a place","risk":"medium","reason":reference,
        "environment":environment,"host":{"id":host,"name":"Approval host","address":address},"consequences":["The chosen place may reach the port"]});
    serde_json::from_value::<orch_api_contract::approvals::CapabilityRequest>(request.clone())
        .unwrap();
    let id = new_id();
    let row = rows::capability::insert(
        &mut conn,
        &rows::capability::NewCapabilityApproval {
            id,
            thread_id: thread,
            turn_id: None,
            environment_id: s.on.environment,
            title: "Review a boundary",
            request: &request,
            approver_scope: scope,
            expires_at: s.rig.world.now() + chrono::Duration::hours(1),
            at: s.rig.world.now(),
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(row.environment_id, Some(s.on.environment));
    id
}
fn scope_rule(kind: &str, id: Option<Id>, resource: &str, op: &str, actions: &[&str]) -> Json {
    let mut target = json!({"kind":kind,"reach":"self"});
    if let Some(id) = id {
        target["id"] = json!(id);
    }
    json!({"target":target,"resources":[resource],"allow":[{"operation":op,"actions":actions}]})
}
async fn capability_scope(scope: &str) {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let host = actual_host(&s).await;
    let admin;
    let actor = if scope == "system" {
        admin = places::admin(&s.rig.world, "approvaladmin").await;
        &admin
    } else {
        &s.ada
    };
    let browser_id = capability(&s, thread, host, scope, "ordinary scope witness").await;
    let row = rows::approval(&mut *s.rig.world.conn().await, browser_id)
        .await
        .unwrap()
        .unwrap();
    let native = Caller::session(actor.id()).with_admin_mode(scope == "system");
    assert!(
        s.rig
            .engine
            .may_decide_capability_request(&native, &row)
            .await
            .unwrap(),
        "ordinary scope must hold before key admission"
    );
    let deny = json!({"approve":false,"note":"Keep the current boundary"});
    let browser = checked(
        decide(&s, &actor.token, browser_id, &deny).await,
        StatusCode::OK,
    );
    assert_eq!(browser["state"], "denied");
    let id = capability(&s, thread, host, scope, "explicit key scope witness").await;
    let (kind, target) = match scope {
        "environment" => ("environment", Some(s.on.environment)),
        "host" => ("host", Some(host)),
        "org" => ("org", Some(s.tree.org.base.id)),
        "system" => ("system", None),
        _ => unreachable!(),
    };
    let op = "ApprovalDecide.capability";
    let rules = vec![
        scope_rule(kind, target, kind, op, &["manage"]),
        scope_rule(kind, target, "approval", op, &["read"]),
        names(&s, op),
        rule(
            "environment",
            s.on.environment,
            "environment",
            op,
            &["read"],
        ),
        rule("host", host, "host", op, &["read"]),
    ];
    let key = key(&s, actor.id(), rules).await;
    let shown = checked(decide(&s, &key.secret, id, &deny).await, StatusCode::OK);
    assert_eq!(shown["state"], "denied");
    assert_eq!(shown["decided_by"]["id"], json!(actor.id()));
    assert_eq!(
        shown["request"]["environment"]["id"],
        json!(s.on.environment)
    );
    assert_eq!(shown["request"]["host"]["id"], json!(host));
    assert_eq!(state(&s, id).await, "denied");
}
#[tokio::test]
async fn capability_environment_uses_current_ports_manage_without_thread_authority_substitution() {
    capability_scope("environment").await;
}
#[tokio::test]
async fn capability_host_uses_the_actual_selected_host() {
    capability_scope("host").await;
}
#[tokio::test]
async fn capability_org_uses_the_actual_environment_home_org() {
    capability_scope("org").await;
}
#[tokio::test]
async fn capability_system_uses_explicit_key_operation_and_current_admin_standing() {
    capability_scope("system").await;
}

async fn source_plan(s: &Scene, thread: Id) -> (Id, Id) {
    sqlx::query("UPDATE agents SET descriptor_overrides=jsonb_set(COALESCE(descriptor_overrides,'{}'::jsonb),'{capabilities}',COALESCE(descriptor_overrides->'capabilities','{}'::jsonb)||'{\"plan_mode\":true}'::jsonb) WHERE id=$1")
        .bind(s.on.agent).execute(s.rig.db.pool()).await.unwrap();
    s.rig
        .engine
        .submit_input(
            &Caller::session(s.ada.id()),
            thread,
            orch_engine::inputs::InputRequest {
                content: vec![orch_api_contract::threads::ContentPart::Text {
                    text: "Propose the agreed change".into(),
                }],
                delivery: Delivery::Queue,
                client_id: "approval-source-plan".into(),
                bypass: None,
                plan: true,
            },
        )
        .await
        .unwrap();
    let start = s.rig.turn_start(thread, 0).await;
    assert!(start.plan);
    s.rig.say(thread, rig::turn_began(start.turn_id)).await;
    let payload=serde_json::from_value(json!({"explanation":"Inspect, then make the agreed change","steps":[{"text":"Inspect the code","status":"pending"}]})).unwrap();
    s.rig
        .say(
            thread,
            Event::ItemStarted(ItemStarted {
                turn_id: start.turn_id,
                native_ref: "approval-plan-item".into(),
                stream: "main".into(),
                parent_native_ref: None,
                kind: EmittedItemKind::Plan,
                payload: ItemPayload::Plan(payload),
            }),
        )
        .await;
    let payload=serde_json::from_value(json!({"explanation":"Inspect, then make the agreed change","steps":[{"text":"Inspect the code","status":"pending"}]})).unwrap();
    s.rig
        .say(
            thread,
            Event::ItemCompleted(ItemCompleted {
                native_ref: "approval-plan-item".into(),
                status: orch_agent_contract::events::ItemStatus::Completed,
                payload: ItemPayload::Plan(payload),
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
    rig::eventually("the completed source plan has its actual approval",||async {
        sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM approvals a JOIN items i ON i.id=a.object_id WHERE a.thread_id=$1 AND a.kind='plan' AND i.kind='plan')")
            .bind(thread).fetch_one(s.rig.db.pool()).await.unwrap()
    }).await;
    sqlx::query_as("SELECT a.id,a.object_id FROM approvals a JOIN items i ON i.id=a.object_id WHERE a.thread_id=$1 AND a.kind='plan' AND i.kind='plan'")
        .bind(thread).fetch_one(s.rig.db.pool()).await.unwrap()
}
async fn decide_source_plan(managers: bool) {
    let s = Scene::new().await;
    if managers {
        s.rig
            .world
            .set("thread.plan_answered_by", json!("managers"))
            .await;
    }
    let thread = s.thread().await.base.id;
    let (id, item) = source_plan(&s, thread).await;
    let op = if managers {
        "ApprovalDecide.plan_manage"
    } else {
        "ApprovalDecide.plan_interact"
    };
    let action = if managers { "manage" } else { "write" };
    let card = checked(get(&s, &s.ada.token, id).await, StatusCode::OK);
    assert_eq!(card["kind"], "plan");
    assert_eq!(card["can_decide"], true);
    let key = key(&s, s.ada.id(), decide_rules(&s, thread, op, action, true)).await;
    let shown = checked(
        decide(&s, &key.secret, id, &json!({"option_id":"proceed"})).await,
        StatusCode::OK,
    );
    assert_eq!(shown["state"], "approved");
    let origin: Json = sqlx::query_scalar(
        "SELECT submission_authority FROM thread_inputs WHERE thread_id=$1 AND client_id=$2",
    )
    .bind(thread)
    .bind(format!("plan:{item}"))
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(
        origin["origins"][0]["operation"], op,
        "the exact approval operation must survive native input submission"
    );
    let next = s.rig.turn_start(thread, 1).await;
    assert!(
        !next.plan,
        "Proceed starts the accepted implementation input"
    );
    assert_eq!(state(&s, id).await, "approved");
}
#[tokio::test]
async fn source_plan_interact_keeps_exact_approval_origin_through_native_dispatch() {
    decide_source_plan(false).await;
}
#[tokio::test]
async fn source_plan_manage_keeps_exact_approval_origin_through_native_dispatch() {
    decide_source_plan(true).await;
}
#[tokio::test]
async fn held_input_runs_on_the_deciders_access_with_its_original_author_and_exact_operation() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let input = s
        .say(
            &s.dan,
            thread,
            "Please run these exact words",
            Delivery::Queue,
            "approval-held-input",
        )
        .await
        .ok()
        .input;
    assert_eq!(input.state_detail.as_deref(), Some("model_use_required"));
    assert_eq!(input.author.as_ref().unwrap().id, s.dan.id());
    let id = input
        .approval_id
        .expect("native model-use wait has a source card");
    let card = checked(get(&s, &s.ada.token, id).await, StatusCode::OK);
    assert_eq!(card["kind"], "input");
    assert_eq!(card["can_decide"], true);
    let op = "ApprovalDecide.input";
    let key = key(&s, s.ada.id(), decide_rules(&s, thread, op, "write", true)).await;
    let shown = checked(
        decide(&s, &key.secret, id, &json!({"run":true})).await,
        StatusCode::OK,
    );
    assert_eq!(shown["state"], "approved");
    let (author, run_as, origin): (Option<Id>, Option<Id>, Json) =
        sqlx::query_as("SELECT author_id,run_as,run_as_authority FROM thread_inputs WHERE id=$1")
            .bind(input.id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(author, Some(s.dan.id()));
    assert_eq!(run_as, Some(s.ada.id()));
    assert_eq!(origin["origins"][0]["operation"], op);
    let start = s.rig.turn_start(thread, 0).await;
    let responsible: Option<Id> =
        sqlx::query_scalar("SELECT audit_for_user_id FROM turns WHERE id=$1")
            .bind(start.turn_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(responsible, Some(s.ada.id()));
}

#[tokio::test]
async fn capability_host_scope_keeps_the_specific_native_system_admin_decider_alternative() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let host = actual_host_with_access(&s, NewAccess::OnlyMe).await;
    let admin = places::admin(&s.rig.world, "boundaryadmin").await;
    let plain = Principal::load(
        &mut *s.rig.world.conn().await,
        Caller::session(admin.id()),
        Time::new(s.rig.world.now()),
    )
    .await
    .unwrap();
    assert!(plain.has_system_admin_standing());
    assert!(!plain.is_system_admin());
    assert!(
        !orch_authz::resources::check(
            &mut *s.rig.world.conn().await,
            &plain,
            ObjectRef {
                kind: ResourceKind::Host.into(),
                id: host
            },
            Permission::HostManage,
            Policy::default()
        )
        .await
        .unwrap()
        .is_allowed(),
        "there is no ordinary Host ACL to disguise the native system-admin decision alternative"
    );
    let browser_id = capability(
        &s,
        thread,
        host,
        "host",
        "native system admin compatibility",
    )
    .await;
    let browser = checked(
        decide(&s, &admin.token, browser_id, &json!({"approve":false})).await,
        StatusCode::OK,
    );
    assert_eq!(browser["state"], "denied");
    let id = capability(
        &s,
        thread,
        host,
        "host",
        "explicit system admin business operation",
    )
    .await;
    let op = "ApprovalDecide.capability";
    let rules = vec![
        rule("host", host, "host", op, &["manage"]),
        rule("host", host, "approval", op, &["read"]),
        rule("host", host, "host", op, &["read"]),
        rule(
            "environment",
            s.on.environment,
            "environment",
            op,
            &["read"],
        ),
        names(&s, op),
    ];
    let key = key(&s, admin.id(), rules).await;
    let shown = checked(
        decide(&s, &key.secret, id, &json!({"approve":false})).await,
        StatusCode::OK,
    );
    assert_eq!(shown["state"], "denied");
    assert_eq!(shown["decided_by"]["id"], json!(admin.id()));
    // The exact admitted business decision does not turn the token into a private Host reader.
    let denied = s
        .rig
        .world
        .app
        .call(orch_api_contract::resources::HostGet, &[&host])
        .token(&key.secret)
        .send_bytes()
        .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
}
#[tokio::test]
async fn capability_existing_card_source_read_is_same_operation_and_does_not_borrow_another_read() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let host = actual_host(&s).await;
    let op = "ApprovalDecide.capability";
    let rules = vec![
        rule(
            "environment",
            s.on.environment,
            "environment",
            op,
            &["manage"],
        ),
        rule("environment", s.on.environment, "approval", op, &["read"]),
        rule(
            "environment",
            s.on.environment,
            "environment",
            op,
            &["read"],
        ),
        names(&s, op),
        rule("host", host, "host", GET, &["read"]),
    ];
    let limited = key(&s, s.ada.id(), rules.clone()).await;
    let first = capability(
        &s,
        thread,
        host,
        "environment",
        "same-operation source negative",
    )
    .await;
    let ordinary = checked(get(&s, &s.ada.token, first).await, StatusCode::OK);
    assert_eq!(ordinary["request"]["host"]["name"], "Approval host");
    let receipt = checked(
        decide(&s, &limited.secret, first, &json!({"approve":false})).await,
        StatusCode::OK,
    );
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["approval_id"], json!(first));
    assert_eq!(receipt["state"], "denied");
    assert!(receipt.get("request").is_none());
    let mut full_rules = rules;
    full_rules.push(rule("host", host, "host", op, &["read"]));
    let full = key(&s, s.ada.id(), full_rules).await;
    let second = capability(
        &s,
        thread,
        host,
        "environment",
        "same-operation source positive",
    )
    .await;
    let shown = checked(
        decide(&s, &full.secret, second, &json!({"approve":false})).await,
        StatusCode::OK,
    );
    assert_eq!(shown["request"]["host"]["name"], "Approval host");
    assert!(shown.get("projection").is_none());
}
#[tokio::test]
async fn approval_get_preserves_existing_shared_card_labels_without_inventing_host_view() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let host = actual_host_with_access(&s, NewAccess::OnlyMe).await;
    let mut conn = s.rig.world.conn().await;
    let page = orch_db::queries::access::PageKey::new("host", host, "live");
    orch_db::queries::access::write_row(
        &mut conn,
        &orch_db::queries::access::RowWrite {
            page: &page,
            profile: "any",
            permission: "host.view",
            inherit: false,
            locked: false,
        },
        Some(s.ada.id()),
        s.rig.world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let principal = Principal::load(
        &mut *s.rig.world.conn().await,
        Caller::session(s.cai.id()),
        Time::new(s.rig.world.now()),
    )
    .await
    .unwrap();
    assert!(has(&s, &s.cai, thread, Permission::ThreadView).await);
    assert!(
        !orch_authz::resources::check(
            &mut *s.rig.world.conn().await,
            &principal,
            ObjectRef {
                kind: ResourceKind::Host.into(),
                id: host
            },
            Permission::HostView,
            Policy::default()
        )
        .await
        .unwrap()
        .is_allowed()
    );
    let id = capability(
        &s,
        thread,
        host,
        "environment",
        "shared approval request context",
    )
    .await;
    let browser = checked(get(&s, &s.cai.token, id).await, StatusCode::OK);
    assert_eq!(browser["request"]["host"]["name"], "Approval host");
    assert_eq!(browser["can_decide"], false);
    let rules = vec![
        rule("environment", s.on.environment, "approval", GET, &["read"]),
        rule(
            "environment",
            s.on.environment,
            "environment",
            GET,
            &["read"],
        ),
        rule("host", host, "host", GET, &["read"]),
        names(&s, GET),
    ];
    let key = key(&s, s.cai.id(), rules).await;
    assert_eq!(
        checked(get(&s, &key.secret, id).await, StatusCode::OK),
        browser,
        "source bounds restrict this operation's disclosure; they do not add a fresh ordinary HostView requirement"
    );
}

async fn pending_approved_input(s: &Scene, thread: Id) -> (Id, Id, Key) {
    let input = s
        .say(
            &s.dan,
            thread,
            "Retain my own contribution",
            Delivery::Queue,
            "continuing-approval-input",
        )
        .await
        .ok()
        .input;
    assert_eq!(input.state_detail.as_deref(), Some("model_use_required"));
    let approval = input.approval_id.unwrap();
    // Same ordinary availability hold used by existing input_operation_authority controls.
    sqlx::query("UPDATE environments SET status='failed',status_detail='approval origin control' WHERE id=$1").bind(s.on.environment).execute(s.rig.db.pool()).await.unwrap();
    let key = key(
        s,
        s.ada.id(),
        decide_rules(s, thread, "ApprovalDecide.input", "write", false),
    )
    .await;
    checked(
        decide(s, &key.secret, approval, &json!({"run":true})).await,
        StatusCode::OK,
    );
    let origin: Json = sqlx::query_scalar("SELECT run_as_authority FROM thread_inputs WHERE id=$1")
        .bind(input.id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(origin["origins"][0]["operation"], "ApprovalDecide.input");
    assert_eq!(state(s, approval).await, "approved");
    // Approval can leave an earlier asynchronous source/file preparation in flight.
    // Establish the real environment hold before either caller changes authority.
    crate::rig::eventually(
        "the approved input is held by its unavailable environment",
        || async {
            s.rig.engine.preflight(thread).await.unwrap();
            assert!(s.rig.turn_starts(thread).is_empty());
            let mut conn = s.rig.db.acquire().await.unwrap();
            orch_db::queries::threads::thread(&mut conn, thread)
                .await
                .unwrap()
                .is_some_and(|row| {
                    row.state == "blocked"
                        && row.blocked_reason.as_deref() == Some("environment_unavailable")
                })
        },
    )
    .await;
    assert!(s.rig.turn_starts(thread).is_empty());
    (approval, input.id, key)
}
async fn ready(s: &Scene) {
    sqlx::query("UPDATE environments SET status='running',status_detail=NULL WHERE id=$1")
        .bind(s.on.environment)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
}
#[tokio::test]
async fn unaccepted_approved_input_rechecks_its_exact_current_key_scope_and_can_recover() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let (approval, input, key) = pending_approved_input(&s, thread).await;
    let policy = |action: &str| json!({"rules":[rule("thread",thread,"thread","ApprovalDecide.input",&[action])]});
    let narrowed: KeyScope = serde_json::from_value(policy("read")).unwrap();
    assert!(narrowed.validate().is_empty());
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(key.id)
        .bind(serde_json::to_value(narrowed).unwrap())
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    ready(&s).await;
    s.rig.engine.preflight(thread).await.unwrap();
    assert!(
        s.rig.turn_starts(thread).is_empty(),
        "a completed approval is not permission to dispatch new input after its key loses the original action"
    );
    let pending: String = sqlx::query_scalar("SELECT state FROM thread_inputs WHERE id=$1")
        .bind(input)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(pending, "pending");
    assert_eq!(
        state(&s, approval).await,
        "approved",
        "the already accepted decision is not undone"
    );
    let postnegative: (String, Option<String>, Option<Id>, Option<Id>, Option<Json>) =
        sqlx::query_as("SELECT state,state_detail,author_id,run_as,run_as_authority FROM thread_inputs WHERE id=$1")
            .bind(input)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    eprintln!(
        "key-loss input={input}: state={}, detail={:?}, author={:?}, run_as={:?}, has_runner_authority={}",
        postnegative.0,
        postnegative.1,
        postnegative.2,
        postnegative.3,
        postnegative.4.is_some()
    );
    assert_eq!(postnegative.0, "pending");
    assert_eq!(postnegative.1.as_deref(), Some("model_use_required"));
    assert_eq!(postnegative.2, Some(s.dan.id()));
    assert_eq!(postnegative.3, None);
    assert!(
        postnegative.4.is_none(),
        "lost runner authority must be cleared"
    );
    let restored: KeyScope = serde_json::from_value(policy("write")).unwrap();
    assert!(restored.validate().is_empty());
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(key.id)
        .bind(serde_json::to_value(restored).unwrap())
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    s.rig.engine.preflight(thread).await.unwrap();
    assert!(
        s.rig.turn_starts(thread).is_empty(),
        "restoring a policy does not recreate the withdrawn runner delegation"
    );
    let still_held: (String, Option<String>, Option<Id>, Option<Json>) = sqlx::query_as(
        "SELECT state,state_detail,run_as,run_as_authority FROM thread_inputs WHERE id=$1",
    )
    .bind(input)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(still_held.0, "pending");
    assert_eq!(still_held.1.as_deref(), Some("model_use_required"));
    assert_eq!(still_held.2, None);
    assert!(still_held.3.is_none());
    // ApprovalDecide.input cannot donate authority to the separate Run operation.
    checked(
        s.rig
            .world
            .app
            .call(
                orch_api_contract::threads::ThreadInputRun,
                &[&thread, &input],
            )
            .token(&key.secret)
            .send_bytes()
            .await,
        StatusCode::FORBIDDEN,
    );
    assert!(s.rig.turn_starts(thread).is_empty());
    // A fresh ordinary Run it action supplies the current actor's own model access.
    checked(
        s.rig
            .world
            .app
            .call(
                orch_api_contract::threads::ThreadInputRun,
                &[&thread, &input],
            )
            .token(&s.ada.token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    let start = s.rig.turn_start(thread, 0).await;
    let stored: (Option<Id>, Option<Id>) =
        sqlx::query_as("SELECT author_id,run_as FROM thread_inputs WHERE id=$1")
            .bind(input)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(stored, (Some(s.dan.id()), Some(s.ada.id())));
    let responsibility: Option<Id> =
        sqlx::query_scalar("SELECT audit_for_user_id FROM turns WHERE id=$1")
            .bind(start.turn_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(responsibility, Some(s.ada.id()));
}
#[tokio::test]
async fn approved_input_still_requires_its_original_authors_current_contribution() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let (approval, input, _key) = pending_approved_input(&s, thread).await;
    let mut conn = s.rig.world.conn().await;
    let roles =
        orch_db::queries::access::roles_in(&mut conn, s.tree.org.base.id, s.rig.world.now())
            .await
            .unwrap();
    let role = roles
        .iter()
        .find(|row| row.subject_kind == "user" && row.subject_id == s.dan.id())
        .unwrap();
    assert_eq!(role.role, "developer");
    let role_id = role.id;
    orch_db::queries::access::set_role(&mut conn, role_id, "viewer")
        .await
        .unwrap();
    orch_authz::epoch::bump(&mut conn).await.unwrap();
    drop(conn);
    assert!(!has(&s, &s.dan, thread, Permission::ThreadInteract).await);
    ready(&s).await;
    s.rig.engine.preflight(thread).await.unwrap();
    assert!(
        s.rig.turn_starts(thread).is_empty(),
        "approver model use does not replace the author's current ability to contribute"
    );
    assert_eq!(state(&s, approval).await, "approved");
    let postnegative: (String, Option<String>, Option<Id>) =
        sqlx::query_as("SELECT state,state_detail,author_id FROM thread_inputs WHERE id=$1")
            .bind(input)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    eprintln!(
        "author-loss input={input}: state={}, detail={:?}, author={:?}",
        postnegative.0, postnegative.1, postnegative.2
    );
    assert_eq!(postnegative.0, "cancelled");
    assert_eq!(postnegative.1.as_deref(), Some("author_lost_access"));
    assert_eq!(postnegative.2, Some(s.dan.id()));
    let mut conn = s.rig.world.conn().await;
    orch_db::queries::access::set_role(&mut conn, role_id, "developer")
        .await
        .unwrap();
    orch_authz::epoch::bump(&mut conn).await.unwrap();
    drop(conn);
    assert!(has(&s, &s.dan, thread, Permission::ThreadInteract).await);
    s.rig.engine.preflight(thread).await.unwrap();
    assert!(
        s.rig.turn_starts(thread).is_empty(),
        "restoring driving rights does not resurrect cancelled accepted words"
    );
    let cancelled: (String, Option<String>, Option<Id>) =
        sqlx::query_as("SELECT state,state_detail,author_id FROM thread_inputs WHERE id=$1")
            .bind(input)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(cancelled, postnegative);
    // The restored author may submit a new contribution under the ordinary rules.
    let replacement = s
        .say(
            &s.dan,
            thread,
            "A new contribution after my access returned",
            Delivery::Queue,
            "restored-author-replacement",
        )
        .await
        .ok()
        .input;
    assert_ne!(replacement.id, input);
    assert_eq!(replacement.author.as_ref().unwrap().id, s.dan.id());
    assert_eq!(
        replacement.state_detail.as_deref(),
        Some("model_use_required")
    );
    let replacement_approval = replacement.approval_id.unwrap();
    assert_ne!(replacement_approval, approval);
    checked(
        decide(&s, &s.ada.token, replacement_approval, &json!({"run":true})).await,
        StatusCode::OK,
    );
    assert_eq!(state(&s, replacement_approval).await, "approved");
    s.rig.turn_start(thread, 0).await;
    let old: (String, Option<String>, Option<Id>) =
        sqlx::query_as("SELECT state,state_detail,author_id FROM thread_inputs WHERE id=$1")
            .bind(input)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(
        old, postnegative,
        "the old input stays cancelled with its author"
    );
    assert_eq!(state(&s, approval).await, "approved");
    let fresh_author: Option<Id> =
        sqlx::query_scalar("SELECT author_id FROM thread_inputs WHERE id=$1")
            .bind(replacement.id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(fresh_author, Some(s.dan.id()));
}
