//! Actual native creator outcomes and the retained marker's distinct retry-history source.

use super::*;
use http::StatusCode;
use orch_api_contract::nodes::{AgentAccessChoice, AgentAccessWhere};
use orch_api_contract::routes::Endpoint;
use orch_api_contract::threads::ThreadAdoptRetryHistory;

const ADOPT: &str = "ThreadAdopt";
const RETRY: &str = "ThreadAdoptRetryHistory";

#[path = "creator_source/header_creation.rs"]
mod header_creation;

fn request(s: &Scene, environment: Id, name: &str) -> ThreadAdoptBody {
    trace_native_source();
    let mut value = body(s, Some(environment));
    value.name = Some(name.into());
    value.agent_access = Some(AgentAccessChoice {
        where_: AgentAccessWhere::Nothing,
        parts: Default::default(),
    });
    value
}
fn act(s: &Scene, environment: Id) -> Vec<Json> {
    vec![
        rule(
            ADOPT,
            "topic",
            s.tree.topic.base.id,
            &["thread"],
            &["write"],
        ),
        rule(
            ADOPT,
            "provider",
            s.on.provider,
            &["provider"],
            &["write", "read"],
        ),
        rule(ADOPT, "agent", s.on.agent, &["agent"], &["write"]),
        rule(
            ADOPT,
            "environment",
            environment,
            &["environment"],
            &["write"],
        ),
    ]
}
fn all(s: &Scene, environment: Id) -> Vec<Json> {
    let mut result = rules(s, Some(environment), ADOPT);
    result[2] = rule(ADOPT, "agent", s.on.agent, &["agent"], &["write", "read"]);
    result.push(json!({"target":{"kind":"org","id":s.tree.org.base.id,"reach":"subtree"},
        "resources":["org","project","topic","thread","document"],"allow":[{"operation":ADOPT,"actions":["read"]}]}));
    result.push(rule(
        ADOPT,
        "machine",
        s.rig.machine_id,
        &["machine"],
        &["read"],
    ));
    result
}
async fn post(s: &Scene, token: &str, value: &ThreadAdoptBody) -> RawReply {
    let call = s.rig.world.app.call(ThreadAdopt, &[&s.tree.topic.base.id]);
    call.token(token).body(value).send_bytes().await
}
fn accepted(reply: &RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

#[tokio::test]
async fn ordinary_adoption_keeps_its_native_field_fault_while_scoped_refusals_keep_details_private()
{
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    let environment = registered(&s, "/work").await;
    let ordinary = adopt(&s, &s.ada, "native-b", Some(environment), None)
        .await
        .problem();
    assert_eq!(ordinary.code, ErrorCode::RequestInvalid);
    assert_eq!(ordinary.status, 400);
    assert_eq!(ordinary.extra["errors"][0]["path"], "environment_id");
    assert!(
        ordinary.extra["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("/elsewhere/api")
    );

    let (_, secret) = key(&s, act(&s, environment)).await;
    let mut value = request(&s, environment, "Refused native folder");
    value.native_session_ref = "native-b".into();
    let scoped = post(&s, &secret, &value).await;
    assert_eq!(scoped.status, StatusCode::BAD_REQUEST);
    let sparse: Json = serde_json::from_slice(&scoped.raw).unwrap();
    assert_eq!(sparse["code"], json!(ErrorCode::RequestInvalid));
    assert!(sparse.get("errors").is_none());
    let text = String::from_utf8_lossy(&scoped.raw);
    assert!(!text.contains("/elsewhere/api"));
    assert!(!text.contains("/work"));
    let created: i64 =
        sqlx::query_scalar("SELECT count(*) FROM segments WHERE native_session_ref='native-b'")
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(created, 0, "neither native refusal made a thread");
    assert_eq!(
        s.rig.commands.issued_of("turn.start") + s.rig.commands.issued_of("turn.start.fenced"),
        0
    );
}

#[tokio::test]
async fn browser_and_scoped_adoption_complete_the_same_native_creation_stage() {
    for scoped in [false, true] {
        let s = Scene::new().await;
        the_harness_lists_its_sessions(&s).await;
        the_harness_recorded(&s, Some(MODEL), &two_turns()).await;
        let environment = registered(&s, "/work/web").await;
        let secret = if scoped {
            key(&s, all(&s, environment)).await.1
        } else {
            s.ada.token.clone()
        };
        let value = accepted(
            &post(
                &s,
                &secret,
                &request(&s, environment, "One native adoption stage"),
            )
            .await,
        );
        let thread: orch_api_contract::threads::Thread = serde_json::from_value(value).unwrap();
        assert_eq!(thread.base.name, "One native adoption stage");
        assert_eq!(thread.base.owner.id, s.ada.id());
        assert_eq!(thread.environment.as_ref().unwrap().id, environment);
        assert_eq!(
            segment_ref(&s, thread.base.id).await.as_deref(),
            Some("native-a")
        );
        assert_eq!(
            items_of(&s, thread.base.id)
                .await
                .iter()
                .filter(|item| item.before_orch)
                .count(),
            4
        );
        assert_eq!(s.rig.commands.issued_of("agent.discover_sessions"), 1);
        assert_eq!(s.rig.commands.issued_of("agent.read_session"), 1);
        assert_eq!(
            s.rig.commands.issued_of("turn.start") + s.rig.commands.issued_of("turn.start.fenced"),
            0,
            "completing the private creator does not start a turn"
        );
        let created: i64 =
            sqlx::query_scalar("SELECT count(*) FROM segments WHERE native_session_ref='native-a'")
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap();
        assert_eq!(created, 1, "the private prefix creates exactly one holder");
    }
}

#[tokio::test]
async fn adopting_has_its_real_native_creation_and_history_receipt_without_borrowing_get_read() {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    the_harness_recorded(&s, Some(MODEL), &two_turns()).await;
    let environment = registered(&s, "/work/web").await;
    let mut policy = act(&s, environment);
    policy.push(json!({"target":{"kind":"org","id":s.tree.org.base.id,"reach":"subtree"},
        "resources":["thread","org","project","topic"],"allow":[{"operation":"ThreadGet.read","actions":["read"]}]}));
    let (_, secret) = key(&s, policy).await;
    let reply = post(
        &s,
        &secret,
        &request(&s, environment, "Receipt adopted native session"),
    )
    .await;
    let value = accepted(&reply);
    let thread: Id = serde_json::from_value(value["thread_id"].clone()).unwrap();
    let segment: Id = serde_json::from_value(value["segment_id"].clone()).unwrap();
    assert_eq!(
        value,
        json!({"projection":"receipt","thread_id":thread,"topic_id":s.tree.topic.base.id,"segment_id":segment,"history_items":4})
    );
    assert_eq!(segment_ref(&s, thread).await.as_deref(), Some("native-a"));
    let items = items_of(&s, thread).await;
    assert_eq!(items.iter().filter(|item| item.before_orch).count(), 4);
    assert_eq!(adopted_line(&items).history_missing, None);
    assert_eq!(s.rig.commands.issued_of("agent.read_session"), 1);
    assert_eq!(
        s.rig.commands.issued_of("turn.start") + s.rig.commands.issued_of("turn.start.fenced"),
        0
    );
    assert!(!String::from_utf8_lossy(&reply.raw).contains("Receipt adopted native session"));
}

#[tokio::test]
async fn adopting_returns_the_existing_native_thread_only_with_its_own_actual_header_sources() {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    the_harness_recorded(&s, Some(MODEL), &two_turns()).await;
    let environment = registered(&s, "/work/web").await;
    let (_, secret) = key(&s, all(&s, environment)).await;
    let value = accepted(
        &post(
            &s,
            &secret,
            &request(&s, environment, "Scoped full adopted thread"),
        )
        .await,
    );
    let thread: orch_api_contract::threads::Thread = serde_json::from_value(value).unwrap();
    assert_eq!(thread.base.name, "Scoped full adopted thread");
    assert_eq!(thread.environment.as_ref().unwrap().id, environment);
    assert_eq!(thread.model.as_deref(), Some(MODEL));
    assert_eq!(
        segment_ref(&s, thread.base.id).await.as_deref(),
        Some("native-a")
    );
    assert_eq!(
        items_of(&s, thread.base.id)
            .await
            .iter()
            .filter(|item| item.before_orch)
            .count(),
        4
    );
}

#[tokio::test]
async fn adopted_creation_read_loss_at_the_real_postcommit_wait_keeps_the_exact_accepted_receipt() {
    for close in [false, true] {
        let s = Scene::new().await;
        the_harness_lists_its_sessions(&s).await;
        the_harness_recorded(&s, Some(MODEL), &two_turns()).await;
        let environment = registered(&s, "/work/web").await;
        let (id, secret) = key(&s, all(&s, environment)).await;
        let mut blocker = s.rig.db.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = s.rig.world.app.clone();
        let topic = s.tree.topic.base.id;
        let wanted = request(&s, environment, "Postcommit adopted source marker");
        let pending = tokio::spawn(async move {
            let call = app.call(ThreadAdopt, &[&topic]);
            call.token(&secret).body(&wanted).send_bytes().await
        });
        tokio::time::timeout(WAIT,async {
            loop {
                assert!(!pending.is_finished(),"the native creator must reach the actual activity INSERT wait");
                let waits:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(s.rig.db.pool()).await.unwrap();
                if waits {break;}
                tokio::task::yield_now().await;
            }
        }).await.expect("the real native postcommit publication wait");
        let (thread,segment):(Id,Id)=sqlx::query_as("SELECT n.id,s.id FROM nodes n JOIN segments s ON s.thread_id=n.id WHERE n.name='Postcommit adopted source marker'")
            .fetch_one(s.rig.db.pool()).await.unwrap();
        assert_eq!(
            items_of(&s, thread)
                .await
                .iter()
                .filter(|item| item.before_orch)
                .count(),
            4
        );
        if close {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(id)
                .bind(json!({"rules":act(&s,environment)}))
                .execute(s.rig.db.pool())
                .await
                .unwrap();
        }
        let xmin: String = sqlx::query_scalar("SELECT xmin::text FROM nodes WHERE id=$1")
            .bind(thread)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, pending).await.unwrap().unwrap();
        let value = accepted(&reply);
        if close {
            assert_eq!(
                value,
                json!({"projection":"receipt","thread_id":thread,"topic_id":s.tree.topic.base.id,"segment_id":segment,"history_items":4})
            );
        } else {
            assert_eq!(value["id"], json!(thread));
            assert_eq!(value["name"], "Postcommit adopted source marker");
        }
        let current: String = sqlx::query_scalar("SELECT xmin::text FROM nodes WHERE id=$1")
            .bind(thread)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
        assert_eq!(
            current, xmin,
            "readback never repeats the private creator or history writer"
        );
        assert_eq!(s.rig.commands.issued_of("agent.read_session"), 1);
    }
}

async fn retry_native_source(
    s: &Scene,
    thread: Id,
    secret: &str,
) -> (
    orch_authz::Caller,
    orch_engine::adoption::operation::HttpCredential,
    Option<orch_types::Time>,
    orch_engine::adoption::RetainedHistory,
) {
    use orch_api_core::middleware::{authentication, authorization};
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        format!("Bearer {secret}").parse().unwrap(),
    );
    let authentication::Caller::User(authed) =
        authentication::authenticate(&s.rig.world.app.app, &headers, s.rig.world.now())
            .await
            .unwrap()
    else {
        panic!("the retained source needs its actual authenticated actor");
    };
    let raw = authorization::caller_of(&authed).unwrap();
    let mut tx = s.rig.db.begin().await.unwrap();
    let source = orch_engine::adoption::retained_history_source(&mut tx, thread)
        .await
        .unwrap()
        .unwrap();
    let facts = orch_engine::adoption::operation::retry_history_facts(&mut tx, &source)
        .await
        .unwrap();
    let plan = orch_engine::operation_scope::sources::bind_known(
        ThreadAdoptRetryHistory::ROUTE,
        &orch_api_contract::adoption_sources::RETRY_PREPARATION,
        &facts,
    )
    .unwrap();
    let caller = plan.preparation_caller(&raw).unwrap();
    tx.commit().await.unwrap();
    let authentication::Via::Token {
        token_id,
        expires_at,
        ..
    } = &authed.via
    else {
        panic!("the retained source test uses its actual API key");
    };
    (
        caller,
        orch_engine::adoption::operation::HttpCredential::Token {
            id: *token_id,
            hash: token_hash(secret),
        },
        *expires_at,
        source,
    )
}
fn retry_rules(s: &Scene, thread: Id) -> Vec<Json> {
    vec![
        rule(RETRY, "thread", thread, &["thread"], &["manage"]),
        rule(RETRY, "provider", s.on.provider, &["provider"], &["read"]),
    ]
}

#[tokio::test]
async fn retained_history_reads_the_actual_marker_without_borrowing_adopt_or_agent_use() {
    trace_native_source();
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    registered(&s, "/work/web").await;
    a_history_read_fails(&s);
    let thread = adopt(&s, &s.ada, "native-a", None, None)
        .await
        .ok()
        .into_full()
        .unwrap()
        .base
        .id;
    let historian = s.rig.person("retained-historian").await;
    s.rig
        .lets_use("node", thread, "thread.manage", &historian, json!({}))
        .await;
    let (_, secret) = key_for(&s, historian.id(), retry_rules(&s, thread)).await;
    let (caller, credential, until, source) = retry_native_source(&s, thread, &secret).await;
    let marker = source.marker();
    let segment = source.segment();
    let mut conn = s.rig.world.conn().await;
    let principal = orch_authz::Principal::load(
        &mut conn,
        caller.clone(),
        orch_types::Time::new(s.rig.world.now()),
    )
    .await
    .unwrap();
    for (kind, id, permission) in [
        (
            orch_access_contract::permissions::ObjectKind::Agent,
            s.on.agent,
            orch_access_contract::permissions::Permission::AgentUse,
        ),
        (
            orch_access_contract::permissions::ObjectKind::Provider,
            s.on.provider,
            orch_access_contract::permissions::Permission::ProviderUse,
        ),
    ] {
        assert!(
            !orch_authz::resources::check(
                &mut conn,
                &principal,
                orch_access_contract::decisions::ObjectRef { kind, id },
                permission,
                orch_authz::Policy::default()
            )
            .await
            .unwrap()
            .is_allowed(),
            "the retained actor has no native binding Use"
        );
    }
    drop(conn);
    the_harness_recorded(&s, Some(MODEL), &two_turns()).await;
    let fetched = s
        .rig
        .world
        .app
        .app
        .engine
        .fetch_retained_adopted_history(&caller, credential, source, None, until)
        .await
        .unwrap();
    assert_eq!(
        (
            fetched.source().thread(),
            fetched.source().marker(),
            fetched.source().segment()
        ),
        (thread, marker, segment)
    );
    match fetched.record().unwrap() {
        orch_engine::adoption::HistoryRecord::Read {
            entries: Ok(entries),
            ..
        } => assert_eq!(entries.len(), 4),
        _ => panic!("the real retained reader must decode its native record"),
    }
    let items = items_of(&s, thread).await;
    assert!(
        items.iter().all(|item| !item.before_orch),
        "the producer does not import or change the marker"
    );
    assert_eq!(
        adopted_line(&items).history_missing,
        Some(AdoptHistoryMissing::Unreadable)
    );
    let mut tx = s.rig.db.begin().await.unwrap();
    let check = fetched
        .check_in(&mut tx, &caller, s.rig.world.app.app.clock.as_ref())
        .await
        .unwrap();
    assert_eq!(check.caller().admitted_operation(), Some(RETRY));
    check.check(s.rig.world.app.app.clock.as_ref()).unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        s.rig.commands.issued_of("turn.start") + s.rig.commands.issued_of("turn.start.fenced"),
        0
    );
}

#[tokio::test]
async fn retained_history_closes_on_its_actual_provider_or_marker_source_instead_of_reporting_missing_content()
 {
    trace_native_source();
    for mode in ["live", "provider_read_lost", "marker_changed"] {
        let s = Scene::new().await;
        the_harness_lists_its_sessions(&s).await;
        registered(&s, "/work/web").await;
        a_history_read_fails(&s);
        let thread = adopt(&s, &s.ada, "native-a", None, None)
            .await
            .ok()
            .into_full()
            .unwrap()
            .base
            .id;
        the_harness_recorded(&s, Some(MODEL), &two_turns()).await;
        let (id, secret) = key(&s, retry_rules(&s, thread)).await;
        let (caller, credential, until, source) = retry_native_source(&s, thread, &secret).await;
        let marker = source.marker();
        let before = s
            .rig
            .machine
            .handler
            .executions()
            .iter()
            .filter(|execution| execution.kind == CommandKind::AgentReadSession)
            .count();
        s.rig.machine.handler.hold(CommandKind::AgentReadSession);
        let engine = s.rig.world.app.app.engine.clone();
        let mut task = tokio::spawn(async move {
            engine
                .fetch_retained_adopted_history(&caller, credential, source, None, until)
                .await
        });
        let reached = tokio::time::timeout(WAIT, async {
            loop {
                if task.is_finished() {
                    break false;
                }
                if s.rig
                    .machine
                    .handler
                    .executions()
                    .iter()
                    .filter(|execution| execution.kind == CommandKind::AgentReadSession)
                    .count()
                    > before
                {
                    break true;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or(false);
        if reached {
            match mode {
                "provider_read_lost" => {
                    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                        .bind(id)
                        .bind(json!({"rules":[retry_rules(&s,thread).remove(0)]}))
                        .execute(s.rig.db.pool())
                        .await
                        .unwrap();
                }
                "marker_changed" => {
                    sqlx::query("UPDATE items SET payload=jsonb_set(payload,'{data,native_session_ref}','\"native-b\"') WHERE id=$1").bind(marker).execute(s.rig.db.pool()).await.unwrap();
                }
                _ => {}
            }
        }
        s.rig.machine.handler.release();
        let result = tokio::time::timeout(WAIT, &mut task).await;
        if result.is_err() {
            task.abort();
            let _ = task.await;
        }
        let result = result
            .expect("the real retained reader must drain")
            .unwrap();
        assert!(
            reached,
            "the actual retry record command must run before changing the source"
        );
        match (mode, result) {
            ("live", Ok(fetched)) => assert!(matches!(
                fetched.record().unwrap(),
                orch_engine::adoption::HistoryRecord::Read { entries: Ok(_), .. }
            )),
            (
                _,
                Err(orch_engine::adoption::AdoptError::Internal(
                    orch_engine::EngineError::BeforeEffect(_),
                )),
            ) => {}
            _ => panic!("a known source refusal must not become successful unreadable history"),
        }
        assert!(
            items_of(&s, thread)
                .await
                .iter()
                .all(|item| !item.before_orch)
        );
        assert_eq!(
            s.rig.commands.issued_of("turn.start") + s.rig.commands.issued_of("turn.start.fenced"),
            0
        );
    }
}
