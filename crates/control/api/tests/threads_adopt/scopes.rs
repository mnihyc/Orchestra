//! Real native setup, original-operation continuation and selected preview sources.

use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::Json;
use std::time::Duration;

#[path = "scopes/creator_source.rs"]
mod creator_source;

const OP: &str = "ThreadAdoptPreview";
const WAIT: Duration = Duration::from_secs(30);
fn rule(op: &str, kind: &str, id: Id, resources: &[&str], actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":resources,"allow":[{"operation":op,"actions":actions}]})
}
fn rules(s: &Scene, environment: Option<Id>, op: &str) -> Vec<Json> {
    let mut result = vec![
        rule(
            op,
            "topic",
            s.tree.topic.base.id,
            &["thread", "topic"],
            &["write", "read"],
        ),
        rule(
            op,
            "provider",
            s.on.provider,
            &["provider"],
            &["read", "write"],
        ),
        rule(op, "agent", s.on.agent, &["agent"], &["write"]),
    ];
    if let Some(environment) = environment {
        result.push(rule(
            op,
            "environment",
            environment,
            &["environment"],
            &["read", "write"],
        ));
    }
    result
}
async fn key(s: &Scene, rules: Vec<Json>) -> (Id, String) {
    key_for(s, s.ada.id(), rules).await
}
async fn key_for(s: &Scene, actor: Id, rules: Vec<Json>) -> (Id, String) {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *s.rig.world.conn().await,
        &NewApiToken {
            id,
            user_id: actor,
            name: "Native adoption source",
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
    (id, secret)
}
fn body(s: &Scene, environment: Option<Id>) -> ThreadAdoptBody {
    ThreadAdoptBody {
        agent_access: None,
        provider_id: s.on.provider,
        agent_id: s.on.agent,
        native_session_ref: "native-a".into(),
        environment_id: environment,
        name: None,
    }
}
async fn send(s: &Scene, token: &str, environment: Option<Id>) -> RawReply {
    let request = s
        .rig
        .world
        .app
        .call(ThreadAdoptPreview, &[&s.tree.topic.base.id])
        .token(token)
        .body(&body(s, environment));
    request.send_bytes().await
}
async fn count(s: &Scene) -> (i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM nodes WHERE kind='thread'),(SELECT count(*) FROM harnesses)",
    )
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap()
}
fn success(reply: RawReply) -> Json {
    assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.raw));
    serde_json::from_slice(&reply.raw).unwrap()
}

async fn native_source(
    s: &Scene,
    secret: &str,
) -> (
    orch_authz::Caller,
    orch_engine::adoption::operation::HttpCredential,
    Option<orch_types::Time>,
) {
    use orch_api_contract::routes::Endpoint;
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
        panic!("the real native source has no authenticated actor");
    };
    let caller = authorization::caller_of(&authed).unwrap();
    let adoption = orch_engine::adoption::Adoption {
        agent_access: Some(orch_api_contract::nodes::AgentAccessChoice {
            where_: orch_api_contract::nodes::AgentAccessWhere::Nothing,
            parts: Default::default(),
        }),
        topic: s.tree.topic.base.id,
        provider: s.on.provider,
        agent: s.on.agent,
        native_session_ref: "native-a".into(),
        environment: None,
        name: None,
    };
    let mut tx = s.rig.db.begin().await.unwrap();
    let facts = orch_engine::adoption::operation::initial_facts(&mut tx, &adoption)
        .await
        .unwrap();
    let plan = orch_engine::operation_scope::sources::bind_known(
        ThreadAdopt::ROUTE,
        &orch_api_contract::adoption_sources::ADOPT_PREPARATION,
        &facts,
    )
    .unwrap();
    let admitted = plan.preparation_caller(&caller).unwrap();
    tx.commit().await.unwrap();
    let hash = token_hash(secret);
    let (credential, until) = match &authed.via {
        authentication::Via::Token {
            token_id,
            expires_at,
            ..
        } => (
            orch_engine::adoption::operation::HttpCredential::Token {
                id: *token_id,
                hash,
            },
            *expires_at,
        ),
        authentication::Via::Session { session_id } => {
            let until:chrono::DateTime<chrono::Utc>=sqlx::query_scalar("SELECT LEAST(a.expires_at,s.expires_at) FROM access_tokens a JOIN sessions s ON s.id=a.session_id WHERE s.id=$1 AND a.token_hash=$2").bind(session_id).bind(&hash).fetch_one(s.rig.db.pool()).await.unwrap();
            (
                orch_engine::adoption::operation::HttpCredential::Session {
                    id: *session_id,
                    hash,
                },
                Some(orch_types::Time::new(until)),
            )
        }
        authentication::Via::OwnerRead { .. } => {
            panic!("an adoption caller signs in with a token or a session, never an owner read")
        }
    };
    (admitted, credential, until)
}

#[tokio::test]
async fn the_native_observed_creator_keeps_its_real_record_and_commits_birth_and_history_together()
{
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    the_harness_recorded(&s, Some(MODEL), &two_turns()).await;
    let environment = registered(&s, "/work/web").await;
    let (caller, credential, until) = native_source(&s, &s.ada.token).await;
    let request = orch_engine::adoption::Adoption {
        agent_access: Some(orch_api_contract::nodes::AgentAccessChoice {
            where_: orch_api_contract::nodes::AgentAccessWhere::Nothing,
            parts: Default::default(),
        }),
        topic: s.tree.topic.base.id,
        provider: s.on.provider,
        agent: s.on.agent,
        native_session_ref: "native-a".into(),
        environment: Some(environment),
        name: None,
    };
    let engine = &s.rig.world.app.app.engine;
    let fetched = Arc::new(
        engine
            .fetch_adoption(&caller, credential, request, None, until)
            .await
            .unwrap(),
    );
    let mut tx = s.rig.db.begin().await.unwrap();
    let principal =
        orch_authz::Principal::load(&mut tx, caller, orch_types::Time::new(s.rig.world.now()))
            .await
            .unwrap();
    let prepared = engine
        .prepare_adoption_birth_in(&mut tx, &principal, fetched)
        .await
        .unwrap();
    assert_eq!(prepared.new_thread().environment, environment);
    assert_eq!(prepared.birth().prepare_source, Some(false));
    let created = engine
        .create_thread_taking_in_transaction(
            &mut tx,
            prepared.principal(),
            prepared.new_thread(),
            prepared.birth(),
            new_id(),
            prepared.native_session_ref(),
        )
        .await
        .unwrap();
    let clock = s.rig.world.app.app.clock.clone();
    let actor = prepared.principal().caller().clone();
    let write = engine
        .write_adoption_in(
            &mut tx,
            prepared.principal(),
            &prepared,
            &created,
            move || {
                if actor.key_deadline_allows(clock.as_ref())
                    && until.is_none_or(|end| clock.time().get() < end.get())
                {
                    Ok(())
                } else {
                    Err(orch_engine::EngineError::BeforeEffect(Box::new(
                        orch_engine::adoption::operation::PreparationEnded,
                    )))
                }
            },
        )
        .await
        .unwrap();
    assert_eq!(write.outcome().history_items, 4);
    let outside: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE id=$1")
        .bind(created.thread)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(
        outside, 0,
        "birth and imported history remain on the same private creator transaction"
    );
    let adopted = engine.commit_adoption(tx, write).await.unwrap();
    assert_eq!(adopted.created.thread, created.thread);
    assert_eq!(
        segment_ref(&s, created.thread).await.as_deref(),
        Some("native-a")
    );
    let items = items_of(&s, created.thread).await;
    assert_eq!(items.iter().filter(|item| item.before_orch).count(), 4);
    assert_eq!(adopted_line(&items).turns, 2);
    assert_eq!(s.rig.commands.issued_of("agent.read_session"), 1);
    assert_eq!(
        s.rig.commands.issued_of("turn.start") + s.rig.commands.issued_of("turn.start.fenced"),
        0
    );
}

#[tokio::test]
async fn the_native_adoption_record_source_cannot_turn_read_loss_into_missing_history() {
    for close in [false, true] {
        let s = Scene::new().await;
        the_harness_lists_its_sessions(&s).await;
        the_harness_recorded(&s, Some(MODEL), &two_turns()).await;
        let environment = registered(&s, "/work/web").await;
        let (id, secret) = key(&s, rules(&s, Some(environment), "ThreadAdopt")).await;
        let (caller, credential, until) = native_source(&s, &secret).await;
        let request = orch_engine::adoption::Adoption {
            agent_access: Some(orch_api_contract::nodes::AgentAccessChoice {
                where_: orch_api_contract::nodes::AgentAccessWhere::Nothing,
                parts: Default::default(),
            }),
            topic: s.tree.topic.base.id,
            provider: s.on.provider,
            agent: s.on.agent,
            native_session_ref: "native-a".into(),
            environment: Some(environment),
            name: None,
        };
        s.rig.machine.handler.hold(CommandKind::AgentReadSession);
        let engine = s.rig.world.app.app.engine.clone();
        let mut task = tokio::spawn(async move {
            engine
                .fetch_adoption(&caller, credential, request, None, until)
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
                    .any(|execution| execution.kind == CommandKind::AgentReadSession)
                {
                    break true;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or(false);
        if reached && close {
            let mut narrowed = rules(&s, Some(environment), "ThreadAdopt");
            narrowed[1] = rule(
                "ThreadAdopt",
                "provider",
                s.on.provider,
                &["provider"],
                &["write"],
            );
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(id)
                .bind(json!({"rules":narrowed}))
                .execute(s.rig.db.pool())
                .await
                .unwrap();
        }
        s.rig.machine.handler.release();
        let done = tokio::time::timeout(WAIT, &mut task).await;
        if done.is_err() {
            task.abort();
            let _ = task.await;
        }
        let result = done
            .expect("the actual native record source must drain")
            .unwrap();
        assert!(
            reached,
            "the genuine record-reading native command must execute before changing source Read"
        );
        match (close, result) {
            (false, Ok(_)) => {}
            (
                true,
                Err(orch_engine::adoption::AdoptError::Internal(
                    orch_engine::EngineError::BeforeEffect(_),
                )),
            ) => {}
            _ => panic!(
                "source closure must fail the native preparation, never yield a successful unreadable-history fallback"
            ),
        }
        assert_eq!(count(&s).await.0, 0);
        assert_eq!(s.rig.commands.issued_of("agent.read_session"), 1);
    }
}

#[tokio::test]
async fn preview_requires_its_real_topic_provider_agent_and_environment_under_its_own_operation() {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    let environment = registered(&s, "/work/web").await;
    let all = rules(&s, Some(environment), OP);
    let before = count(&s).await;
    let commands = s.rig.commands.issued_total();
    for absent in 0..3 {
        let mut incomplete = all.clone();
        incomplete.remove(absent);
        let (_, secret) = key(&s, incomplete).await;
        let reply = send(&s, &secret, Some(environment)).await;
        assert_eq!(reply.status, 403, "{}", String::from_utf8_lossy(&reply.raw));
        assert_eq!(count(&s).await, before);
        assert_eq!(
            s.rig.commands.issued_total(),
            commands,
            "missing known setup facets refuse before harness setup or RPC"
        );
    }
    let (_, secret) = key(&s, rules(&s, Some(environment), "ThreadAdopt")).await;
    assert_eq!(
        send(&s, &secret, Some(environment)).await.status,
        403,
        "the creator does not lend its namespace to preview"
    );
    assert_eq!(s.rig.commands.issued_total(), commands);
    let mut wrong = all.clone();
    wrong[3] = rule(
        OP,
        "environment",
        new_id(),
        &["environment"],
        &["read", "write"],
    );
    let (_, secret) = key(&s, wrong).await;
    assert_eq!(
        send(&s, &secret, Some(environment)).await.status,
        403,
        "actual selected Environment remains required after native observation"
    );
    let (_, secret) = key(&s, all).await;
    let result = success(send(&s, &secret, Some(environment)).await);
    assert_eq!(result["environment"]["id"], json!(environment));
    assert_eq!(result["session"]["native_session_ref"], "native-a");
    assert!(result["adopted_thread_id"].is_null());
    assert!(result.get("created_thread").is_none());
    assert_eq!(
        count(&s).await.0,
        before.0,
        "preview never creates a Thread"
    );
}

#[tokio::test]
async fn preview_preserves_ordinary_prerequisites_and_uses_its_own_content_and_name_read() {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    let environment = registered(&s, "/work/web").await;
    let all = rules(&s, Some(environment), OP);
    let (_, secret) = key_for(&s, s.cai.id(), all.clone()).await;
    let before = s.rig.commands.issued_total();
    assert_eq!(send(&s, &secret, Some(environment)).await.status, 403);
    assert_eq!(
        s.rig.commands.issued_total(),
        before,
        "scope cannot supply ordinary use or Topic creation rights"
    );
    let mut borrowed = all.clone();
    borrowed[0] = rule(OP, "topic", s.tree.topic.base.id, &["thread"], &["write"]);
    borrowed[3] = rule(OP, "environment", environment, &["environment"], &["write"]);
    borrowed.push(rule(
        "TopicGet",
        "topic",
        s.tree.topic.base.id,
        &["topic"],
        &["read"],
    ));
    borrowed.push(rule(
        "EnvironmentGet",
        "environment",
        environment,
        &["environment"],
        &["read"],
    ));
    let (_, secret) = key(&s, borrowed).await;
    assert_eq!(
        send(&s, &secret, Some(environment)).await.status,
        403,
        "neighboring read operations cannot disclose this preview"
    );
    sqlx::query("UPDATE orgs SET slug='private-ancestor-preview-marker' WHERE node_id=$1")
        .bind(s.tree.org.base.id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let (_, secret) = key(&s, all.clone()).await;
    let result = success(send(&s, &secret, Some(environment)).await);
    assert!(
        !result
            .to_string()
            .contains("private-ancestor-preview-marker")
    );
    let mut named = all;
    named.push(rule(OP, "org", s.tree.org.base.id, &["org"], &["read"]));
    let (_, secret) = key(&s, named).await;
    let result = success(send(&s, &secret, Some(environment)).await);
    assert!(
        result["environment"]["address"]
            .as_str()
            .unwrap()
            .contains("private-ancestor-preview-marker"),
        "the actual optional ancestor name remains available under its own operation Read"
    );
}

#[tokio::test]
async fn automatic_adoption_places_keep_the_nearest_eligible_folder_within_the_original_scope() {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    let closest = registered(&s, "/work/web").await;
    let eligible = registered(&s, "/work").await;
    let (_, secret) = key(&s, rules(&s, Some(eligible), OP)).await;
    let result = success(send(&s, &secret, None).await);
    assert_eq!(result["environment"]["id"], json!(eligible));
    assert_eq!(result["directory"], "web");
    assert!(
        !result.to_string().contains(&closest.to_string()),
        "a nearer folder outside the key's binding is not selected or emitted"
    );
}

#[tokio::test]
async fn preview_requires_current_ordinary_use_of_the_environment_even_when_scope_covers_it() {
    use orch_engine::access::ResourceAccess;
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    let environment = s.on.environment;
    sqlx::query("UPDATE environments SET workspace_root='/work' WHERE id=$1")
        .bind(environment)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let (_, secret) = key_for(&s, s.eve.id(), rules(&s, Some(environment), OP)).await;
    success(send(&s, &secret, Some(environment)).await);
    // Close the managed defaults too: removing a direct Manage grant leaves inherited Use.
    let entry:Id=sqlx::query_scalar("SELECT id FROM permission_entries WHERE object_kind='environment' AND object_id=$1 AND permission='environment.manage' AND holder_kind='user' AND holder_id=$2 AND removed_at IS NULL")
        .bind(environment).bind(s.eve.id()).fetch_one(s.rig.db.pool()).await.unwrap();
    s.rig
        .world
        .app
        .call(
            orch_api_contract::access::PermissionPageUpdate,
            &[&"environments", &environment],
        )
        .token(&s.ada.token)
        .raw_body(&json!({"rows":[
            {"permission":"environment.manage","inherit":false,"remove":[entry]},
            {"permission":"environment.use","inherit":false},
        ]}))
        .send()
        .await
        .ok();
    let access = orch_api_core::thread_access::AuthzAccess::new(s.rig.world.app.app.clock.clone());
    assert!(
        !access
            .environment_place(&mut *s.rig.world.conn().await, s.eve.id(), environment)
            .await
            .unwrap()
            .is_allowed(),
        "the acting person must genuinely lack native Environment Use"
    );
    assert_eq!(
        send(&s, &secret, Some(environment)).await.status,
        403,
        "operation scope cannot supply the person's missing native Use"
    );
    s.rig
        .lets_use(
            "environment",
            environment,
            "environment.use",
            &s.eve,
            json!({}),
        )
        .await;
    success(send(&s, &secret, Some(environment)).await);
}

#[tokio::test]
async fn preview_without_a_registered_folder_has_no_fabricated_environment_or_created_thread() {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    let mut observed = rules(&s, None, OP);
    observed[0] = rule(OP, "topic", s.tree.topic.base.id, &["thread"], &["write"]);
    let (_, secret) = key(&s, observed).await;
    let before = count(&s).await.0;
    let result = success(send(&s, &secret, None).await);
    assert!(result["environment"].is_null());
    assert_eq!(result["register_first"], "/work/web");
    assert!(result["agent_access"].is_null());
    assert!(result["adopted_thread_id"].is_null());
    assert_eq!(count(&s).await.0, before);
    assert_eq!(s.rig.commands.issued_of("agent.discover_sessions"), 1);
    assert_eq!(
        s.rig.commands.issued_of("agent.read_session"),
        1,
        "publication never repeats native observation"
    );
}

#[tokio::test]
async fn preview_checks_the_original_key_after_the_actual_native_harness_claim_wait() {
    for expire in [false, true] {
        let s = Scene::new().await;
        the_harness_lists_its_sessions(&s).await;
        let environment = registered(&s, "/work/web").await;
        let (_, secret) = key(&s, rules(&s, Some(environment), OP)).await;
        let before = count(&s).await;
        let commands = s.rig.commands.issued_total();
        let mut blocker = s.rig.db.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let tuple = format!(
            "harness.claim:{}:{}:{}",
            s.on.provider, s.on.agent, s.rig.machine_id
        );
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(tuple)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let app = s.rig.world.app.clone();
        let topic = s.tree.topic.base.id;
        let body = body(&s, Some(environment));
        let mut task = tokio::spawn(async move {
            let request = app
                .call(ThreadAdoptPreview, &[&topic])
                .token(&secret)
                .body(&body);
            request.send_bytes().await
        });
        let reached=tokio::time::timeout(WAIT,async {loop {
            if task.is_finished(){break false;}
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%pg_advisory_xact_lock(hashtextextended($1,0))%')").bind(pid).fetch_one(s.rig.db.pool()).await.unwrap();
            if waiting{break true;}tokio::task::yield_now().await;
        }}).await.unwrap_or(false);
        let during = count(&s).await;
        let issued = s.rig.commands.issued_total();
        if reached && expire {
            s.rig.world.advance(chrono::Duration::hours(2));
        }
        blocker.rollback().await.unwrap();
        let completed = tokio::time::timeout(WAIT, &mut task).await;
        if completed.is_err() {
            task.abort();
            let _ = task.await;
        }
        let reply = completed.expect("the native claim must drain").unwrap();
        assert!(
            reached,
            "the source must reach the actual native claim mutex"
        );
        assert_eq!(during, before);
        assert_eq!(issued, commands);
        if expire {
            assert!(
                matches!(reply.status.as_u16(), 401 | 403),
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert_eq!(
                count(&s).await,
                before,
                "no harness claim or Thread remains after source expiry"
            );
            assert_eq!(s.rig.commands.issued_total(), commands);
        } else {
            success(reply);
            assert!(s.rig.commands.issued_total() > commands);
        }
    }
}

#[tokio::test]
async fn preview_rechecks_the_same_original_operation_between_discovery_and_history_read() {
    for mode in [
        "live_key",
        "narrow_key",
        "replace_key_hash",
        "live_browser",
        "expire_browser",
    ] {
        let s = Scene::new().await;
        the_harness_lists_its_sessions(&s).await;
        let environment = registered(&s, "/work/web").await;
        let (id, secret) = if mode.ends_with("browser") {
            (None, s.ada.token.clone())
        } else {
            let (id, secret) = key(&s, rules(&s, Some(environment), OP)).await;
            (Some(id), secret)
        };
        let bearer_end = if mode == "expire_browser" {
            // The clock is manual. Expire only the actual bearer after discovery,
            // while its long-lived session and the native command are still live.
            sqlx::query("UPDATE access_tokens SET expires_at=$2 WHERE token_hash=$1")
                .bind(token_hash(&secret))
                .bind(s.rig.world.now() + chrono::Duration::seconds(1))
                .execute(s.rig.db.pool())
                .await
                .unwrap();
            let (end,session_end):(chrono::DateTime<chrono::Utc>,chrono::DateTime<chrono::Utc>)=sqlx::query_as("SELECT a.expires_at,s.expires_at FROM access_tokens a JOIN sessions s ON s.id=a.session_id WHERE a.token_hash=$1 AND s.revoked_at IS NULL").bind(token_hash(&secret)).fetch_one(s.rig.db.pool()).await.unwrap();
            assert!(
                session_end > end + chrono::Duration::milliseconds(1),
                "the browser session must really outlive its bearer for this control"
            );
            Some(end)
        } else {
            None
        };
        s.rig
            .machine
            .handler
            .hold(CommandKind::AgentDiscoverSessions);
        let app = s.rig.world.app.clone();
        let topic = s.tree.topic.base.id;
        let body = body(&s, Some(environment));
        let mut task = tokio::spawn(async move {
            let request = app
                .call(ThreadAdoptPreview, &[&topic])
                .token(&secret)
                .body(&body);
            request.send_bytes().await
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
                    .any(|entry| entry.kind == CommandKind::AgentDiscoverSessions)
                {
                    break true;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or(false);
        if reached && mode == "narrow_key" {
            let mut next = rules(&s, Some(environment), OP);
            next[2] = rule(OP, "agent", s.on.agent, &["agent"], &["read"]);
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(id.unwrap())
                .bind(json!({"rules":next}))
                .execute(s.rig.db.pool())
                .await
                .unwrap();
        }
        if reached && mode == "replace_key_hash" {
            sqlx::query("UPDATE api_tokens SET token_hash=$2 WHERE id=$1")
                .bind(id.unwrap())
                .bind(token_hash(&new_token(API_TOKEN_PREFIX)))
                .execute(s.rig.db.pool())
                .await
                .unwrap();
        }
        if reached && let Some(end) = bearer_end {
            s.rig
                .world
                .advance(end - s.rig.world.now() + chrono::Duration::milliseconds(1));
        }
        s.rig.machine.handler.release();
        let completed = tokio::time::timeout(WAIT, &mut task).await;
        if completed.is_err() {
            task.abort();
            let _ = task.await;
        }
        let reply = completed
            .expect("discovery and its owning HTTP task must drain")
            .unwrap();
        assert!(
            reached,
            "actual native discovery must execute before changing the source"
        );
        if matches!(mode, "narrow_key" | "replace_key_hash" | "expire_browser") {
            assert!(
                matches!(reply.status.as_u16(), 401 | 403),
                "{}: {}",
                mode,
                String::from_utf8_lossy(&reply.raw)
            );
            assert_eq!(
                s.rig.commands.issued_of("agent.read_session"),
                0,
                "the later history read cannot borrow discovery's earlier admission"
            );
        } else {
            success(reply);
            assert_eq!(s.rig.commands.issued_of("agent.read_session"), 1);
        }
    }
}

#[tokio::test]
async fn a_later_authorized_preview_resumes_the_exact_accepted_harness_receipt_after_its_original_key_ends()
 {
    use orch_db::queries::link;
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    let environment = registered(&s, "/work/web").await;
    let (original, secret) = key(&s, rules(&s, Some(environment), OP)).await;
    s.rig.machine.handler.hold(CommandKind::AgentStart);
    let app = s.rig.world.app.clone();
    let topic = s.tree.topic.base.id;
    let request_body = body(&s, Some(environment));
    let mut task = tokio::spawn(async move {
        let request = app
            .call(ThreadAdoptPreview, &[&topic])
            .token(&secret)
            .body(&request_body);
        request.send_bytes().await
    });
    let reached = tokio::time::timeout(WAIT, async {
        loop {
            if task.is_finished() {
                break None;
            }
            if let Some(execution) = s
                .rig
                .machine
                .handler
                .executions()
                .into_iter()
                .find(|entry| entry.kind == CommandKind::AgentStart)
            {
                break Some(execution.id);
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .ok()
    .flatten();
    let retained = if let Some(run) = reached {
        let (claim,frozen):(Id,Json)=sqlx::query_as("SELECT start_command_id,after_up_plan FROM harnesses WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3")
            .bind(s.on.provider).bind(s.on.agent).bind(s.rig.machine_id).fetch_one(s.rig.db.pool()).await.unwrap();
        assert_eq!(
            claim, run,
            "the actual native execution must belong to this real setup claim"
        );
        let authority = json!(
            s.rig
                .commands
                .issued_authority(run)
                .expect("the actual native execution has its original private evidence")
        );
        let serial = orch_link_contract::commands::SerialKey::Harness {
            provider_id: s.on.provider,
            agent_id: s.on.agent,
        }
        .wire();
        let queued = link::NewCommand {
            id: run,
            machine_id: s.rig.machine_id,
            kind: "agent.start",
            body: &frozen["request"],
            serial_key: serial.as_deref(),
            deadline_at: s.rig.world.now()
                + chrono::Duration::seconds(i64::from(CommandKind::AgentStart.deadline_secs())),
        };
        let mut tx = s.rig.db.begin().await.unwrap();
        assert!(
            link::insert_command_with_authority(
                &mut tx,
                &queued,
                s.rig.world.now(),
                Some(&authority)
            )
            .await
            .unwrap(),
            "journal the same actual command that the scripted link accepted"
        );
        assert!(
            link::mark_accepted(&mut tx, run, s.rig.world.now())
                .await
                .unwrap()
        );
        tx.commit().await.unwrap();
        identity::revoke_api_token(
            &mut *s.rig.world.conn().await,
            original,
            s.ada.id(),
            s.rig.world.now(),
        )
        .await
        .unwrap();
        Some((run, frozen, authority))
    } else {
        None
    };
    s.rig.machine.handler.release();
    let completed = tokio::time::timeout(WAIT, &mut task).await;
    if completed.is_err() {
        task.abort();
        let _ = task.await;
    }
    let first = completed
        .expect("the real accepted harness command must drain")
        .unwrap();
    let (run, frozen, authority) = retained.expect("actual AgentStart acceptance must be reached");
    assert!(
        matches!(first.status.as_u16(), 401 | 403),
        "the original closed reader must not proceed to a new discovery"
    );
    let stored = link::command(&mut *s.rig.world.conn().await, run)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.state, "succeeded",
        "the accepted native result is reconciled independently of later new-work authority"
    );
    // Reproduce a restart after its durable result but before the native claim's cleanup,
    // using the genuine original frozen claim rather than a replacement command/source.
    sqlx::query("UPDATE harnesses SET status='starting',start_command_id=$4,after_up_plan=$5 WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3")
        .bind(s.on.provider).bind(s.on.agent).bind(s.rig.machine_id).bind(run).bind(frozen).execute(s.rig.db.pool()).await.unwrap();
    let (_, secret) = key(&s, rules(&s, Some(environment), OP)).await;
    let result = success(send(&s, &secret, Some(environment)).await);
    assert_eq!(result["environment"]["id"], json!(environment));
    assert_eq!(
        s.rig.machine.handler.runs_of(run),
        1,
        "an accepted setup is not executed again or leased by the new credential"
    );
    assert_eq!(
        link::command_authority(&mut *s.rig.world.conn().await, run)
            .await
            .unwrap(),
        Some(authority),
        "the new reader cannot rewrite the original command's private source"
    );
}

#[tokio::test]
async fn native_adoption_preparation_rechecks_the_actual_browser_session_proof_between_rpcs() {
    for clear_proof in [false, true] {
        let s = Scene::new().await;
        the_harness_lists_its_sessions(&s).await;
        let environment = registered(&s, "/work/web").await;
        let session: Id =
            sqlx::query_scalar("SELECT session_id FROM access_tokens WHERE token_hash=$1")
                .bind(token_hash(&s.ada.token))
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap();
        sqlx::query("UPDATE users SET email_code_at=$2 WHERE id=$1")
            .bind(s.ada.id())
            .bind(s.rig.world.now())
            .execute(s.rig.db.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE sessions SET second_step_at=$2 WHERE id=$1")
            .bind(session)
            .bind(s.rig.world.now())
            .execute(s.rig.db.pool())
            .await
            .unwrap();
        s.rig
            .world
            .set_at(
                orch_settings::Target::Node(s.tree.org.base.id),
                "auth.second_step",
                json!("required"),
            )
            .await;
        let (_, key_secret) = key(&s, rules(&s, Some(environment), OP)).await;
        s.rig
            .machine
            .handler
            .hold(CommandKind::AgentDiscoverSessions);
        let app = s.rig.world.app.clone();
        let topic = s.tree.topic.base.id;
        let token = s.ada.token.clone();
        let body = body(&s, Some(environment));
        let mut task = tokio::spawn(async move {
            let request = app
                .call(ThreadAdoptPreview, &[&topic])
                .token(&token)
                .body(&body);
            request.send_bytes().await
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
                    .any(|execution| execution.kind == CommandKind::AgentDiscoverSessions)
                {
                    break true;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or(false);
        if reached && clear_proof {
            sqlx::query("UPDATE sessions SET second_step_at=NULL WHERE id=$1")
                .bind(session)
                .execute(s.rig.db.pool())
                .await
                .unwrap();
        }
        s.rig.machine.handler.release();
        let done = tokio::time::timeout(WAIT, &mut task).await;
        if done.is_err() {
            task.abort();
            let _ = task.await;
        }
        let reply = done
            .expect("the actual native discovery must drain")
            .unwrap();
        assert!(
            reached,
            "a genuinely configured and proven browser must reach actual discovery"
        );
        if clear_proof {
            assert!(
                matches!(reply.status.as_u16(), 401 | 403),
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert_eq!(
                s.rig.commands.issued_of("agent.read_session"),
                0,
                "a valid bearer and configured account do not replace the actual Session's missing second proof"
            );
        } else {
            success(reply);
            assert_eq!(s.rig.commands.issued_of("agent.read_session"), 1);
        }
        success(send(&s, &key_secret, Some(environment)).await);
        if clear_proof {
            sqlx::query("UPDATE sessions SET second_step_at=$2 WHERE id=$1")
                .bind(session)
                .bind(s.rig.world.now())
                .execute(s.rig.db.pool())
                .await
                .unwrap();
            success(send(&s, &s.ada.token, Some(environment)).await);
        }
    }
}

#[tokio::test]
async fn native_preview_commands_keep_exact_private_preparation_evidence_and_current_sources() {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    let environment = registered(&s, "/work/web").await;
    let (id, secret) = key(&s, rules(&s, Some(environment), OP)).await;
    success(send(&s, &secret, Some(environment)).await);
    let execution = s
        .rig
        .machine
        .handler
        .executions()
        .into_iter()
        .find(|entry| entry.kind == CommandKind::AgentDiscoverSessions)
        .unwrap();
    let authority = s
        .rig
        .commands
        .issued_authority(execution.id)
        .expect("actual discovery retains private source evidence");
    let encoded = json!(authority);
    assert_eq!(encoded["kind"], "adoption_preparation");
    assert!(encoded["authority"]["request"].get("session").is_none());
    assert_eq!(
        encoded["authority"]["request"]["credential"]["kind"],
        "token"
    );
    assert_eq!(
        encoded["authority"]["request"]["credential"]["id"],
        json!(id)
    );
    let authority: orch_engine::commands::CommandAuthority =
        serde_json::from_value(encoded.clone()).unwrap();
    let body = json!(orch_link_contract::commands::AgentDiscoverSessions {
        provider_id: s.on.provider,
        agent_id: s.on.agent
    });
    let serial = orch_link_contract::commands::SerialKey::Harness {
        provider_id: s.on.provider,
        agent_id: s.on.agent,
    }
    .wire();
    let actual = orch_db::queries::link::NewCommand {
        id: execution.id,
        machine_id: s.rig.machine_id,
        kind: "agent.discover_sessions",
        body: &body,
        serial_key: serial.as_deref(),
        deadline_at: s.rig.world.now() + chrono::Duration::minutes(5),
    };
    let app = &s.rig.world.app.app;
    let mut tx = s.rig.db.begin().await.unwrap();
    let callers = authority
        .current_issue_callers(
            &mut tx,
            &actual,
            app.clock.as_ref(),
            &orch_engine::access::Unconnected,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(callers.len(), 1);
    assert_eq!(
        callers[0].0.as_ref().unwrap().admitted_operation(),
        Some(OP)
    );
    let wrong = orch_db::queries::link::NewCommand {
        machine_id: new_id(),
        ..actual
    };
    assert!(
        authority
            .current_issue_callers(
                &mut tx,
                &wrong,
                app.clock.as_ref(),
                &orch_engine::access::Unconnected
            )
            .await
            .unwrap()
            .is_none()
    );
    tx.rollback().await.unwrap();
    let mut wrong_credential = encoded;
    wrong_credential["authority"]["request"]["credential"]["hash"] = json!([]);
    let wrong_credential: orch_engine::commands::CommandAuthority =
        serde_json::from_value(wrong_credential).unwrap();
    let mut tx = s.rig.db.begin().await.unwrap();
    assert!(
        wrong_credential
            .current_issue_callers(
                &mut tx,
                &actual,
                app.clock.as_ref(),
                &orch_engine::access::Unconnected
            )
            .await
            .unwrap()
            .is_none(),
        "a real key id cannot lend another raw credential to the original native source"
    );
    tx.rollback().await.unwrap();
    identity::revoke_api_token(
        &mut *s.rig.world.conn().await,
        id,
        s.ada.id(),
        s.rig.world.now(),
    )
    .await
    .unwrap();
    let mut tx = s.rig.db.begin().await.unwrap();
    assert!(
        authority
            .current_issue_callers(
                &mut tx,
                &actual,
                app.clock.as_ref(),
                &orch_engine::access::Unconnected
            )
            .await
            .unwrap()
            .is_none()
    );
    tx.rollback().await.unwrap();
    assert_eq!(
        s.rig.commands.issued_of("agent.discover_sessions"),
        1,
        "verification never replays native work"
    );
}
