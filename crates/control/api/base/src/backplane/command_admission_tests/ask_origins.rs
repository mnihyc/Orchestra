//! A queued personal model request keeps its original browser access lifetime at native delivery.

use super::*;
use tracing::instrument::WithSubscriber;

use crate::support::failure_trace::FailureTrace;

use crate::middleware::authentication;

use crate::registry::Registry;

use crate::testing::TestApp;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::pages::StoredKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::ask::AskMessageBody;

use orch_api_contract::ask::AskMessageCreate;

use orch_api_contract::ask::AskModel;

use orch_api_contract::threads::ContentPart;

use orch_authz::ownership;

use orch_authz::ownership::NewAccess;

use orch_db::queries::access;

use orch_db::queries::ask;

use orch_db::queries::identity;

use orch_db::queries::threads;
const MODEL: &str = "review-model";

const QUESTION: &str = "Private question for the isolated model peer";

fn outcome_status(outcome: &CommandOutcome) -> String {
    match outcome {
        CommandOutcome::Succeeded(_) => "succeeded".into(),
        CommandOutcome::Failed(error) => format!("failed code={}", error.code),
        CommandOutcome::Cancelled => "cancelled".into(),
        CommandOutcome::Expired => "expired".into(),
    }
}

async fn member_bearer(s: &Scene) -> Proof<String> {
    let mut conn = s.transport.test.pool.acquire().await?;
    let root: Id = sqlx::query_scalar("SELECT ask_root_org_id FROM threads WHERE node_id=$1")
        .bind(s.thread)
        .fetch_one(&mut *conn)
        .await?;
    let person = resources::testing::insert_user(&mut conn, "question-member").await?;
    access::testing::insert_member(&mut conn, root, person, root).await?;
    let bearer = authentication::new_token(authentication::ACCESS_TOKEN_PREFIX);
    identity::testing::insert_session(
        &mut conn,
        new_id(),
        person,
        &authentication::token_hash(&bearer),
        s.transport.clock.now() + chrono::Duration::hours(1),
    )
    .await?;
    Ok(bearer)
}

async fn business_state(s: &Scene) -> Proof<orch_types::Json> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('title',n.name,'unlisted',n.unlisted,'retention_days',t.ask_retention_days,'seq',t.next_seq,'state',t.ask_state,'inputs',(SELECT count(*) FROM thread_inputs WHERE thread_id=t.node_id),'turns',(SELECT count(*) FROM turns WHERE thread_id=t.node_id),'events',(SELECT count(*) FROM ask_events WHERE thread_id=t.node_id),'calls',(SELECT count(*) FROM ask_calls WHERE thread_id=t.node_id)) FROM threads t JOIN nodes n ON n.id=t.node_id WHERE t.node_id=$1")
        .bind(s.thread).fetch_one(&s.transport.test.pool).await?)
}

async fn private_root(s: &Scene) -> Proof<Id> {
    let mut tx = s.app.app.db.begin().await?;
    let (person, root): (Id, Id) = sqlx::query_as(
        "SELECT n.owner_id,t.ask_root_org_id FROM threads t JOIN nodes n ON n.id=t.node_id WHERE t.node_id=$1",
    )
    .bind(s.thread)
    .fetch_one(&mut *tx)
    .await?;
    let principal = orch_authz::Principal::load(
        &mut tx,
        orch_authz::Caller::session(person),
        s.transport.clock.time(),
    )
    .await?;
    let change = serde_json::from_value(json!({"whole":"private"}))?;
    orch_authz::pages::change(
        &mut tx,
        &principal,
        orch_authz::pages::Target::Live(root),
        &change,
        orch_authz::Policy::default(),
        &orch_db::audit::Origin::default(),
    )
    .await?;
    tx.commit().await?;
    Ok(root)
}

async fn unaffiliated_bearer(s: &Scene) -> Proof<String> {
    let mut conn = s.transport.test.pool.acquire().await?;
    let person = resources::testing::insert_user(&mut conn, "question-outside-reader").await?;
    let bearer = authentication::new_token(authentication::ACCESS_TOKEN_PREFIX);
    identity::testing::insert_session(
        &mut conn,
        new_id(),
        person,
        &authentication::token_hash(&bearer),
        s.transport.clock.now() + chrono::Duration::hours(1),
    )
    .await?;
    Ok(bearer)
}

async fn conversation_counts(s: &Scene) -> Proof<orch_types::Json> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('nodes',(SELECT count(*) FROM nodes),'inputs',(SELECT count(*) FROM thread_inputs),'turns',(SELECT count(*) FROM turns),'events',(SELECT count(*) FROM ask_events),'calls',(SELECT count(*) FROM ask_calls))")
        .fetch_one(&s.transport.test.pool).await?)
}

async fn message_key(s: &Scene, history_read: bool) -> Proof<String> {
    use orch_access_contract::key_scope::KeyScope;
    let mut conn = s.transport.test.pool.acquire().await?;
    let person: Id = sqlx::query_scalar("SELECT owner_id FROM nodes WHERE id=$1")
        .bind(s.thread)
        .fetch_one(&mut *conn)
        .await?;
    let actions = if history_read {
        vec!["write", "read"]
    } else {
        vec!["write"]
    };
    let scope: KeyScope = serde_json::from_value(json!({"rules":[{
        "target":{"kind":"thread","id":s.thread,"reach":"self"},
        "allow":[{"operation":"AskMessageCreate","actions":actions}]
    }]}))?;
    assert!(scope.validate().is_empty());
    let bearer = authentication::new_token(authentication::API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut conn,
        &identity::NewApiToken {
            id: new_id(),
            user_id: person,
            name: "Question history permission",
            token_hash: &authentication::token_hash(&bearer),
            scopes: &[],
            node_id: None,
            at: s.transport.clock.now(),
            expires_at: Some(s.transport.clock.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope)?),
    )
    .await?;
    Ok(bearer)
}

fn hidden(reply: &crate::testing::RawReply) {
    assert_eq!(reply.status, http::StatusCode::NOT_FOUND);
    assert_eq!(
        reply.problem().code,
        orch_api_contract::common::ErrorCode::NotFound
    );
}

pub(super) fn reply() -> CommandResult {
    use orch_model_contract::generation::{
        GenerationMessage, GenerationOutput, GenerationPart, GenerationRole, GenerationStop,
    };
    CommandResult::ProviderGenerate(orch_link_contract::commands::ProviderGenerateResult {
        native_result_blob: None,
        output: GenerationOutput {
            native_result: serde_json::Value::Null,
            message: GenerationMessage {
                role: GenerationRole::Assistant,
                content: vec![GenerationPart::Text {
                    text: "The isolated response completed.".into(),
                }],
                continuation: Vec::new(),
            },
            stop: GenerationStop::Complete,
        },
        usage: Default::default(),
        timing: Default::default(),
        request_ref: None,
    })
}

struct Scene {
    transport: Fixture,
    app: TestApp,
    thread: Id,
    bearer: String,
}

impl Scene {
    async fn new() -> Proof<(Self, Id)> {
        let mut transport = Fixture::new(false).await?;
        let mut registry = Registry::new();
        crate::ask::mount(&mut registry);
        let app = TestApp::new(
            transport.link.inner.db.clone(),
            transport.clock.clone(),
            registry,
        );
        transport.link = app.app.link.clone();
        transport.dispatcher = Dispatcher::new(transport.link.clone());
        let mut conn = transport.test.pool.acquire().await?;
        let (person, root): (Id, Id) =
            sqlx::query_as("SELECT owner_id,org_id FROM environments WHERE id=$1")
                .bind(transport.environment)
                .fetch_one(&mut *conn)
                .await?;
        access::testing::insert_member(&mut conn, root, person, root).await?;
        access::testing::name_owner(&mut conn, root, ("user", person), transport.clock.now())
            .await?;
        let (project, topic, thread) = (new_id(), new_id(), new_id());
        for (id, kind, parent, name) in [
            (project, "project", root, "Model source"),
            (topic, "topic", project, "Questions"),
        ] {
            hierarchy::testing::insert_node(&mut conn, id, kind, Some(parent), name, person)
                .await?;
            orch_authz::pages::start(
                &mut conn,
                StoredKind::Node,
                id,
                person,
                transport.clock.time(),
            )
            .await?;
        }
        assert!(
            threads::insert_thread(
                &mut conn,
                thread,
                topic,
                "Personal question",
                person,
                transport.clock.now(),
            )
            .await?
        );
        orch_authz::pages::start(
            &mut conn,
            StoredKind::Node,
            thread,
            person,
            transport.clock.time(),
        )
        .await?;
        // Native fixture construction avoids unrelated object-creation endpoints.
        // The actual message below still crosses its real HTTP identity and source gate.
        ask::bind(
            &mut conn,
            thread,
            root,
            None,
            None,
            30,
            transport.clock.now(),
        )
        .await?;
        // The isolated provider is anonymous; this case never sends a model key.
        let provider = new_id();
        sqlx::query("INSERT INTO providers(id,name,owner_id,org_id,home_id,kind,key_machine_id,machine_id,protocols,arrangements,status,upstreams) VALUES($1,'Isolated question provider',$2,$3,$3,'api',$4,$4,ARRAY['openai-responses'],ARRAY['key_separated'],'ready',$5)")
            .bind(provider).bind(person).bind(root).bind(transport.machine)
            .bind(json!([{"protocol":"openai-responses","base_url":"https://model.example.invalid","options":{}}]))
            .execute(&mut *conn).await?;
        // Give the isolated model the same window as the ordinary Ask fixture, so the
        // configured tool/output reserves leave room for this delivery control's question.
        sqlx::query("INSERT INTO provider_models(id,provider_id,model_name,display_name,parameters,enabled,source,created_by) VALUES($1,$2,$3,'Isolated model','{\"context_window\":200000}',true,'manual',$4)")
            .bind(new_id()).bind(provider).bind(MODEL).bind(person).execute(&mut *conn).await?;
        ownership::start_resource(
            &mut conn,
            ObjectRef {
                kind: ObjectKind::Provider,
                id: provider,
            },
            person,
            NewAccess::OnlyMe,
            transport.clock.time(),
        )
        .await?;
        let choices = json!([{"provider_id":provider,"model":MODEL}]);
        let written = orch_db::queries::settings::put(
            &mut conn,
            Some(root),
            "ask.models",
            &orch_db::queries::settings::Stored {
                value: &choices,
                locked: false,
                by: Some(person),
                at: transport.clock.now(),
            },
            None,
        )
        .await?;
        assert!(matches!(
            written,
            orch_db::queries::settings::Put::Written(_)
        ));
        let bearer = authentication::new_token(authentication::ACCESS_TOKEN_PREFIX);
        let digest = authentication::token_hash(&bearer);
        identity::testing::insert_session(
            &mut conn,
            new_id(),
            person,
            &digest,
            transport.clock.now() + chrono::Duration::hours(1),
        )
        .await?;
        sqlx::query("UPDATE access_tokens SET expires_at=$2 WHERE token_hash=$1")
            .bind(digest)
            .bind(transport.clock.now() + chrono::Duration::seconds(5))
            .execute(&mut *conn)
            .await?;
        drop(conn);
        within(transport.connect(false)).await??;
        // The next machine handoff is the question's model IO, not runtime startup.
        within(app.app.engine.start_ask_provider(thread, provider)).await??;
        Ok((
            Self {
                transport,
                app,
                thread,
                bearer,
            },
            provider,
        ))
    }
}

#[cfg(any(test, feature = "test-support"))]
async fn a_queued_ask_request_keeps_the_original_access_token_deadline_after_a_native_row_wait()
-> Proof {
    for expires in [false, true] {
        let mut trace = FailureTrace::new("queued Ask raw-deadline phases", WAIT);
        trace.mark("create native scene and start provider");
        trace.mark(format!("expires={expires}"));
        let (s, provider) = Scene::new().await?;
        trace.mark("native scene ready");
        let (queued, release) = gate(s.transport.machine, Phase::MachineAdoption);
        let body = AskMessageBody {
            client_id: new_id().to_string(),
            content: vec![ContentPart::Text {
                text: QUESTION.into(),
            }],
            context: None,
            model: Some(Some(AskModel {
                provider_id: provider,
                model: MODEL.into(),
                params: None,
            })),
        };
        let app = s.app.clone();
        let thread = s.thread;
        let bearer = s.bearer.clone();
        let diagnostic = trace.recorder();
        let events = diagnostic.clone();
        let post = tokio::spawn(
            async move {
                let request = app
                    .call(AskMessageCreate, &[&thread])
                    .token(&bearer)
                    .body(&body);
                let reply = request.send_bytes().await;
                diagnostic.mark(format!("AskMessageCreate HTTP status={}", reply.status));
                reply
            }
            .with_subscriber(events),
        );
        trace.mark("wait for native machine adoption");
        within(queued).await??;
        trace.mark("native machine adoption reached");
        let command: Id = sqlx::query_scalar("SELECT c.id FROM ask_calls c JOIN commands q ON q.id=c.id WHERE c.thread_id=$1 AND q.kind='provider.generate'")
            .bind(s.thread).fetch_one(&s.transport.test.pool).await?;
        let mut conn = s.app.app.db.acquire().await?;
        let retained = orch_db::queries::link::command_authority(&mut conn, command)
            .await?
            .ok_or("the actual model command lost its private authority")?;
        assert_eq!(retained["kind"], "http_origin");
        assert_eq!(retained["authority"]["native"]["kind"], "source");
        assert_eq!(retained["authority"]["native"]["evidence"]["source"], "ask");
        let source: orch_engine::ask::command::CommandSource = serde_json::from_value(
            retained["authority"]["native"]["evidence"]["evidence"].clone(),
        )?;
        assert_eq!(source.origin().operation(), "AskMessageCreate");
        let body: orch_types::Json = sqlx::query_scalar("SELECT body FROM commands WHERE id=$1")
            .bind(command)
            .fetch_one(&mut *conn)
            .await?;
        let request: orch_link_contract::commands::ProviderGenerate = serde_json::from_value(body)?;
        let outgoing = orch_engine::commands::Outgoing::new(
            command,
            s.transport.machine,
            orch_link_contract::commands::SerialKey::None,
            &request,
        )?;
        assert!(source.command(&outgoing).is_ok());
        let mut other_call = outgoing.clone();
        other_call.id = new_id();
        let mut other_machine = outgoing.clone();
        other_machine.machine = new_id();
        let mut other_serial = outgoing.clone();
        other_serial.key = orch_link_contract::commands::SerialKey::Machine;
        let mut other_body = outgoing;
        other_body.body["model_name"] = json!("unselected-model");
        for changed in [other_call, other_machine, other_serial, other_body] {
            assert!(
                source.command(&changed).is_err(),
                "a native source cannot move to another command"
            );
        }
        drop(conn);
        let mut blocker = s.app.app.db.begin().await?;
        let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        let locked: Id = sqlx::query_scalar("SELECT id FROM commands WHERE id=$1 FOR UPDATE")
            .bind(command)
            .fetch_one(&mut *blocker)
            .await?;
        assert_eq!(locked, command);
        let accepted = within(post).await??;
        assert_eq!(
            accepted.status,
            http::StatusCode::ACCEPTED,
            "{}",
            String::from_utf8_lossy(&accepted.raw)
        );
        release
            .send(())
            .map_err(|_| "the native queue handoff ended")?;
        // The visible native command proves its preliminary source transaction committed.
        // Delivery now fences that same thread before waiting on this command, so taking
        // another thread-share lock here would wait behind our own deliberate command hold.
        within(async {
            loop {
                let waiting: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%commands%')",
                )
                .bind(blocker_pid)
                .fetch_one(&s.transport.test.pool)
                .await?;
                if waiting {
                    return Ok::<(), sqlx::Error>(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await??;
        if expires {
            s.transport.clock.advance(chrono::Duration::seconds(10));
        }
        blocker.rollback().await?;
        // Only the isolated peer answers; no external model is called.
        trace.mark(format!(
            "wait for native command outcome; command={command}"
        ));
        let outcome = within(s.transport.dispatcher.wait(command)).await??;
        trace.mark(format!(
            "native outcome kind={:?}",
            outcome_status(&outcome)
        ));
        let published = s
            .transport
            .native
            .wire
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|request| request.id == command)
            .cloned()
            .collect::<Vec<_>>();
        s.app.app.engine.stop();
        s.transport.close().await?;
        if expires {
            assert!(
                published.is_empty(),
                "expired original browser access must prevent private model IO even while its session lives"
            );
            assert!(
                matches!(outcome, CommandOutcome::Expired)
                    || matches!(
                        outcome,
                        CommandOutcome::Failed(ref error) if error.code == ErrorCode::Conflict.as_str()
                    )
            );
        } else {
            assert!(matches!(outcome, CommandOutcome::Succeeded(_)));
            assert_eq!(published.len(), 1);
            assert!(published[0].body.to_string().contains(QUESTION));
            assert!(published[0].body.get("authority").is_none());
            assert!(published[0].body.get("login_authority").is_none());
        }
        trace.finish();
    }
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_queued_ask_request_keeps_the_original_access_token_deadline_after_a_native_row_wait()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(
        self::a_queued_ask_request_keeps_the_original_access_token_deadline_after_a_native_row_wait(
        ),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn a_queued_ask_command_rechecks_its_controller_model_endpoint_and_retained_request_before_delivery()
-> Proof {
    #[derive(Debug)]
    enum Change {
        Controller,
        Model,
        Endpoint,
        Request,
    }
    for change in [
        Change::Controller,
        Change::Model,
        Change::Endpoint,
        Change::Request,
    ] {
        let mut trace = FailureTrace::new("queued Ask native-source phases", WAIT);
        trace.mark(format!(
            "create native scene and start provider; change={change:?}"
        ));
        let (s, provider) = Scene::new().await?;
        trace.mark("native scene ready");
        let (queued, release) = gate(s.transport.machine, Phase::MachineAdoption);
        let body = AskMessageBody {
            client_id: new_id().to_string(),
            content: vec![ContentPart::Text {
                text: QUESTION.into(),
            }],
            context: None,
            model: Some(Some(AskModel {
                provider_id: provider,
                model: MODEL.into(),
                params: None,
            })),
        };
        let app = s.app.clone();
        let thread = s.thread;
        let bearer = s.bearer.clone();
        let diagnostic = trace.recorder();
        let events = diagnostic.clone();
        let post = tokio::spawn(
            async move {
                let request = app
                    .call(AskMessageCreate, &[&thread])
                    .token(&bearer)
                    .body(&body);
                let reply = request.send_bytes().await;
                diagnostic.mark(format!("AskMessageCreate HTTP status={}", reply.status));
                reply
            }
            .with_subscriber(events),
        );
        trace.mark("wait for native machine adoption");
        within(queued).await??;
        trace.mark("native machine adoption reached");
        assert_eq!(within(post).await??.status, http::StatusCode::ACCEPTED);
        let mut conn = s.app.app.db.acquire().await?;
        let command: Id = sqlx::query_scalar("SELECT c.id FROM ask_calls c JOIN commands q ON q.id=c.id WHERE c.thread_id=$1 AND q.kind='provider.generate'")
            .bind(s.thread).fetch_one(&mut *conn).await?;
        match change {
            Change::Controller => {
                sqlx::query("UPDATE threads SET ask_runner_id=$2 WHERE node_id=$1")
                    .bind(s.thread)
                    .bind(new_id())
                    .execute(&mut *conn)
                    .await?;
            }
            Change::Model => {
                sqlx::query("UPDATE provider_models SET enabled=false WHERE provider_id=$1 AND model_name=$2")
                    .bind(provider).bind(MODEL).execute(&mut *conn).await?;
            }
            Change::Endpoint => {
                sqlx::query("UPDATE providers SET upstreams=$2 WHERE id=$1")
                    .bind(provider).bind(json!([{"protocol":"openai-responses","base_url":"https://changed-model.example.invalid","options":{}}]))
                    .execute(&mut *conn).await?;
            }
            Change::Request => {
                sqlx::query(
                    "UPDATE ask_calls SET request=jsonb_set(request,'{model_name}',$2) WHERE id=$1",
                )
                .bind(command)
                .bind(json!("unselected-model"))
                .execute(&mut *conn)
                .await?;
            }
        }
        drop(conn);
        release
            .send(())
            .map_err(|_| "the queued native delivery ended")?;
        trace.mark(format!(
            "wait for native command outcome; command={command}"
        ));
        let outcome = within(s.transport.dispatcher.wait(command)).await??;
        trace.mark(format!(
            "native outcome kind={:?}",
            outcome_status(&outcome)
        ));
        let published = s
            .transport
            .native
            .wire
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|request| request.id == command);
        s.app.app.engine.stop();
        s.transport.close().await?;
        assert!(
            !published,
            "a changed native source must not publish the model command"
        );
        assert!(
            matches!(outcome, CommandOutcome::Failed(ref error) if error.code == ErrorCode::Conflict.as_str())
        );
        trace.finish();
    }
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_queued_ask_command_rechecks_its_controller_model_endpoint_and_retained_request_before_delivery()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(self::a_queued_ask_command_rechecks_its_controller_model_endpoint_and_retained_request_before_delivery())
}

#[cfg(any(test, feature = "test-support"))]
async fn ordinary_members_cannot_change_another_persons_ask_preferences_or_delete_it() -> Proof {
    use orch_api_contract::ask::{AskDelete, AskGet, AskUpdate, AskUpdateBody};
    let (s, _) = Scene::new().await?;
    let member = member_bearer(&s).await?;
    let before = business_state(&s).await?;
    let body = AskUpdateBody {
        shown: Some(true),
        retention_days: Some(7),
    };
    let denied = s
        .app
        .call(AskUpdate, &[&s.thread])
        .token(&member)
        .body(&body)
        .send_bytes()
        .await;
    hidden(&denied);
    assert_eq!(business_state(&s).await?, before);
    let denied = s
        .app
        .call(AskDelete, &[&s.thread])
        .token(&member)
        .send_bytes()
        .await;
    hidden(&denied);
    assert_eq!(business_state(&s).await?, before);
    let allowed = s
        .app
        .call(AskUpdate, &[&s.thread])
        .token(&s.bearer)
        .body(&body)
        .send_bytes()
        .await;
    assert_eq!(allowed.status, http::StatusCode::OK);
    let after = business_state(&s).await?;
    assert_eq!(after["unlisted"], false);
    assert_eq!(after["retention_days"], 7);
    assert_eq!(after["inputs"], before["inputs"]);
    assert_eq!(after["turns"], before["turns"]);
    assert_eq!(after["calls"], before["calls"]);
    let deleted = s
        .app
        .call(AskDelete, &[&s.thread])
        .token(&s.bearer)
        .send_bytes()
        .await;
    assert_eq!(deleted.status, http::StatusCode::NO_CONTENT);
    let absent = s
        .app
        .call(AskGet, &[&s.thread])
        .token(&s.bearer)
        .send_bytes()
        .await;
    hidden(&absent);
    s.app.app.engine.stop();
    s.transport.close().await?;
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_ordinary_members_cannot_change_another_persons_ask_preferences_or_delete_it()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(
        self::ordinary_members_cannot_change_another_persons_ask_preferences_or_delete_it(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn ordinary_members_cannot_read_another_persons_ask_and_event_frames_keep_current_access()
-> Proof {
    use futures::StreamExt as _;
    use orch_api_contract::ask::{
        AskEvents, AskEventsQuery, AskExport, AskGet, AskUpdate, AskUpdateBody,
    };
    let (s, _) = Scene::new().await?;
    let member = member_bearer(&s).await?;
    let before = business_state(&s).await?;
    for denied in [
        s.app
            .call(AskGet, &[&s.thread])
            .token(&member)
            .send_bytes()
            .await,
        s.app
            .call(AskExport, &[&s.thread])
            .token(&member)
            .send_bytes()
            .await,
        s.app
            .call(AskEvents, &[&s.thread])
            .token(&member)
            .send_bytes()
            .await,
    ] {
        hidden(&denied);
        assert!(!String::from_utf8_lossy(&denied.raw).contains("Personal question"));
    }
    assert_eq!(business_state(&s).await?, before);
    let allowed = s
        .app
        .call(AskGet, &[&s.thread])
        .token(&s.bearer)
        .send_bytes()
        .await;
    assert_eq!(allowed.status, http::StatusCode::OK);
    let exported = s
        .app
        .call(AskExport, &[&s.thread])
        .token(&s.bearer)
        .send_bytes()
        .await;
    assert_eq!(exported.status, http::StatusCode::OK);
    let data: orch_types::Json = serde_json::from_slice(&exported.raw)?;
    assert_eq!(data["conversation"]["id"], s.thread.to_string());
    for shown in [true, false] {
        let changed = s
            .app
            .call(AskUpdate, &[&s.thread])
            .token(&s.bearer)
            .body(&AskUpdateBody {
                shown: Some(shown),
                retention_days: None,
            })
            .send_bytes()
            .await;
        assert_eq!(changed.status, http::StatusCode::OK);
    }
    let events = s
        .app
        .call(AskEvents, &[&s.thread])
        .token(&s.bearer)
        .query(&AskEventsQuery { after: Some(0) })
        .send_response()
        .await;
    assert_eq!(events.status(), http::StatusCode::OK);
    let mut frames = events.into_body().into_data_stream();
    let first = within(frames.next())
        .await?
        .ok_or("the authorized event frame is absent")??;
    assert!(String::from_utf8_lossy(&first).contains(&s.thread.to_string()));
    s.transport.clock.advance(chrono::Duration::seconds(10));
    assert!(within(frames.next()).await?.is_none());
    drop(frames);
    s.app.app.engine.stop();
    s.transport.close().await?;
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_ordinary_members_cannot_read_another_persons_ask_and_event_frames_keep_current_access()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(
        self::ordinary_members_cannot_read_another_persons_ask_and_event_frames_keep_current_access(
        ),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn an_ordinary_member_cannot_stop_another_persons_ask_and_the_owner_can_stop_it_idle() -> Proof
{
    use orch_api_contract::ask::AskStop;
    let (s, _) = Scene::new().await?;
    let member = member_bearer(&s).await?;
    let before = business_state(&s).await?;
    let denied = s
        .app
        .call(AskStop, &[&s.thread])
        .token(&member)
        .send_bytes()
        .await;
    hidden(&denied);
    assert_eq!(business_state(&s).await?, before);
    let stopped = s
        .app
        .call(AskStop, &[&s.thread])
        .token(&s.bearer)
        .send_bytes()
        .await;
    assert_eq!(stopped.status, http::StatusCode::OK);
    let receipt: orch_types::Json = serde_json::from_slice(&stopped.raw)?;
    assert_eq!(receipt["conversation_id"], s.thread.to_string());
    assert!(receipt["turn_id"].is_null());
    let after = business_state(&s).await?;
    assert_eq!(after["state"], "ready");
    assert_eq!(after["inputs"], before["inputs"]);
    assert_eq!(after["turns"], before["turns"]);
    assert_eq!(after["calls"], before["calls"]);
    s.app.app.engine.stop();
    s.transport.close().await?;
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_an_ordinary_member_cannot_stop_another_persons_ask_and_the_owner_can_stop_it_idle()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(
        self::an_ordinary_member_cannot_stop_another_persons_ask_and_the_owner_can_stop_it_idle(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn explicit_root_inspection_reads_an_ask_without_permission_to_stop_it() -> Proof {
    use orch_api_contract::ask::{AskGet, AskStop};
    let (s, _) = Scene::new().await?;
    let member = member_bearer(&s).await?;
    let mut conn = s.transport.test.pool.acquire().await?;
    let root: Id = sqlx::query_scalar("SELECT ask_root_org_id FROM threads WHERE node_id=$1")
        .bind(s.thread)
        .fetch_one(&mut *conn)
        .await?;
    let (session, person): (Id, Id) = sqlx::query_as(
        "SELECT s.id,s.user_id FROM sessions s JOIN access_tokens a ON a.session_id=s.id WHERE a.token_hash=$1",
    )
    .bind(authentication::token_hash(&member))
    .fetch_one(&mut *conn)
    .await?;
    access::testing::name_owner(&mut conn, root, ("user", person), s.transport.clock.now()).await?;
    sqlx::query("UPDATE sessions SET as_admin=true WHERE id=$1")
        .bind(session)
        .execute(&mut *conn)
        .await?;
    drop(conn);
    let read = s
        .app
        .call(AskGet, &[&s.thread])
        .token(&member)
        .send_bytes()
        .await;
    assert_eq!(read.status, http::StatusCode::OK);
    let view: orch_types::Json = serde_json::from_slice(&read.raw)?;
    assert_eq!(view["conversation"]["inspecting"], true);
    let before = business_state(&s).await?;
    let denied = s
        .app
        .call(AskStop, &[&s.thread])
        .token(&member)
        .send_bytes()
        .await;
    assert_eq!(denied.status, http::StatusCode::FORBIDDEN);
    assert_eq!(
        denied.problem().code,
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
    assert_eq!(business_state(&s).await?, before);
    s.app.app.engine.stop();
    s.transport.close().await?;
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_explicit_root_inspection_reads_an_ask_without_permission_to_stop_it()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(
        self::explicit_root_inspection_reads_an_ask_without_permission_to_stop_it(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn ask_creation_history_and_model_choices_require_current_access_to_the_selected_root()
-> Proof {
    use orch_api_contract::ask::{
        AskCreate, AskCreateBody, AskList, AskListQuery, AskModelsGet, AskModelsQuery,
    };
    let (s, _) = Scene::new().await?;
    let root = private_root(&s).await?;
    let outside = unaffiliated_bearer(&s).await?;
    let before = conversation_counts(&s).await?;
    let body = AskCreateBody {
        root_org_id: Some(root),
        context: None,
        model: None,
        retention_days: None,
    };
    for (operation, denied) in [
        (
            "AskCreate",
            s.app
                .call(AskCreate, &[])
                .token(&outside)
                .body(&body)
                .send_bytes()
                .await,
        ),
        (
            "AskList",
            s.app
                .call(AskList, &[])
                .token(&outside)
                .query(&AskListQuery {
                    root_org_id: Some(root),
                    ..Default::default()
                })
                .send_bytes()
                .await,
        ),
        (
            "AskModelsGet",
            s.app
                .call(AskModelsGet, &[])
                .token(&outside)
                .query(&AskModelsQuery { root_org_id: root })
                .send_bytes()
                .await,
        ),
    ] {
        assert_eq!(
            denied.status,
            http::StatusCode::FORBIDDEN,
            "{operation}: {}",
            String::from_utf8_lossy(&denied.raw)
        );
        assert_eq!(
            denied.problem().code,
            orch_api_contract::common::ErrorCode::AuthzDenied
        );
    }
    assert_eq!(conversation_counts(&s).await?, before);
    let models = s
        .app
        .call(AskModelsGet, &[])
        .token(&s.bearer)
        .query(&AskModelsQuery { root_org_id: root })
        .send_bytes()
        .await;
    assert_eq!(models.status, http::StatusCode::OK);
    assert!(String::from_utf8_lossy(&models.raw).contains(MODEL));
    let created = s
        .app
        .call(AskCreate, &[])
        .token(&s.bearer)
        .body(&body)
        .send_bytes()
        .await;
    assert_eq!(created.status, http::StatusCode::CREATED);
    let receipt: orch_types::Json = serde_json::from_slice(&created.raw)?;
    let listed = s
        .app
        .call(AskList, &[])
        .token(&s.bearer)
        .query(&AskListQuery {
            root_org_id: Some(root),
            ..Default::default()
        })
        .send_bytes()
        .await;
    assert_eq!(listed.status, http::StatusCode::OK);
    let history: orch_types::Json = serde_json::from_slice(&listed.raw)?;
    assert!(history["items"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item["id"] == receipt["conversation_id"])
    }));
    let after = conversation_counts(&s).await?;
    assert_eq!(after["inputs"], before["inputs"]);
    assert_eq!(after["turns"], before["turns"]);
    assert_eq!(after["calls"], before["calls"]);
    s.app.app.engine.stop();
    s.transport.close().await?;
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_ask_creation_history_and_model_choices_require_current_access_to_the_selected_root()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(
        self::ask_creation_history_and_model_choices_require_current_access_to_the_selected_root(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn submitting_an_ask_needs_its_own_history_read_beside_message_write() -> Proof {
    let mut trace = FailureTrace::new("Ask history facet phases", WAIT);
    trace.mark("create native scene and start provider");
    let (s, provider) = Scene::new().await?;
    let write = message_key(&s, false).await?;
    let complete = message_key(&s, true).await?;
    let before = business_state(&s).await?;
    let mut body = AskMessageBody {
        client_id: new_id().to_string(),
        content: vec![ContentPart::Text {
            text: QUESTION.into(),
        }],
        context: None,
        model: Some(Some(AskModel {
            provider_id: provider,
            model: MODEL.into(),
            params: None,
        })),
    };
    let denied = s
        .app
        .call(AskMessageCreate, &[&s.thread])
        .token(&write)
        .body(&body)
        .send_bytes()
        .with_subscriber(trace.recorder())
        .await;
    trace.mark(format!("write-only HTTP status={}", denied.status));
    assert_eq!(denied.status, http::StatusCode::FORBIDDEN);
    assert_eq!(
        denied.problem().code,
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
    assert_eq!(business_state(&s).await?, before);
    body.client_id = new_id().to_string();
    let accepted = s
        .app
        .call(AskMessageCreate, &[&s.thread])
        .token(&complete)
        .body(&body)
        .send_bytes()
        .with_subscriber(trace.recorder())
        .await;
    trace.mark(format!(
        "Read+Write HTTP status={}; wait for retained native command",
        accepted.status
    ));
    assert_eq!(accepted.status, http::StatusCode::ACCEPTED);
    let command = within(async {
        loop {
            let found: Option<Id> = sqlx::query_scalar(
                "SELECT q.id FROM ask_calls c JOIN commands q ON q.id=c.id WHERE c.thread_id=$1 AND q.kind='provider.generate'",
            )
            .bind(s.thread)
            .fetch_optional(&s.transport.test.pool)
            .await?;
            if let Some(id) = found {
                return Ok::<Id, sqlx::Error>(id);
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    trace.mark(format!("native command={command}; wait for completion"));
    let outcome = within(s.transport.dispatcher.wait(command)).await??;
    trace.mark(format!("native outcome={}", outcome_status(&outcome)));
    assert!(matches!(outcome, CommandOutcome::Succeeded(_)));
    let after = business_state(&s).await?;
    assert_eq!(after["inputs"], 1);
    assert_eq!(after["turns"], 1);
    assert_eq!(after["calls"], 1);
    s.app.app.engine.stop();
    s.transport.close().await?;
    trace.finish();
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_submitting_an_ask_needs_its_own_history_read_beside_message_write()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(
        self::submitting_an_ask_needs_its_own_history_read_beside_message_write(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn ordinary_members_cannot_add_or_open_another_persons_ask_attachment() -> Proof {
    use orch_api_contract::ask::{
        AskAttachmentBody, AskAttachmentCreate, AskAttachmentGet, AskAttachmentReply,
    };
    let (s, _) = Scene::new().await?;
    let member = member_bearer(&s).await?;
    let bytes = bytes::Bytes::from_static(b"A private file retained by its current owner.");
    let meta = s.app.app.blobs.put_bytes(bytes.clone()).await?;
    let mut conn = s.transport.test.pool.acquire().await?;
    let owner: Id = sqlx::query_scalar("SELECT owner_id FROM nodes WHERE id=$1")
        .bind(s.thread)
        .fetch_one(&mut *conn)
        .await?;
    s.app
        .app
        .blobs
        .register(&mut conn, meta, s.transport.clock.now())
        .await?;
    orch_db::queries::uploads::remember(&mut conn, owner, &meta.hash, s.transport.clock.now())
        .await?;
    drop(conn);
    let body = AskAttachmentBody {
        blob: meta.hash,
        name: "permission-notes.txt".into(),
    };
    let before = business_state(&s).await?;
    let refused = s
        .app
        .call(AskAttachmentCreate, &[&s.thread])
        .token(&member)
        .body(&body)
        .send_bytes()
        .await;
    hidden(&refused);
    assert_eq!(business_state(&s).await?, before);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM ask_files WHERE thread_id=$1")
        .bind(s.thread)
        .fetch_one(&s.transport.test.pool)
        .await?;
    assert_eq!(count, 0);
    let attached = s
        .app
        .call(AskAttachmentCreate, &[&s.thread])
        .token(&s.bearer)
        .body(&body)
        .send_bytes()
        .await;
    assert_eq!(attached.status, http::StatusCode::CREATED);
    let attached: AskAttachmentReply = serde_json::from_slice(&attached.raw)?;
    assert_eq!(attached.blob, meta.hash);
    assert_eq!(attached.name, body.name);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ask_files WHERE thread_id=$1 AND blob=$2")
            .bind(s.thread)
            .bind(meta.hash.as_bytes().as_slice())
            .fetch_one(&s.transport.test.pool)
            .await?;
    assert_eq!(count, 1);
    let before = business_state(&s).await?;
    let refused = s
        .app
        .call(AskAttachmentGet, &[&s.thread, &meta.hash])
        .token(&member)
        .send_bytes()
        .await;
    hidden(&refused);
    assert_eq!(business_state(&s).await?, before);
    let opened = s
        .app
        .call(AskAttachmentGet, &[&s.thread, &meta.hash])
        .token(&s.bearer)
        .send_bytes()
        .await;
    assert_eq!(opened.status, http::StatusCode::OK);
    assert_eq!(opened.raw, bytes);
    assert_eq!(business_state(&s).await?, before);
    s.app.app.engine.stop();
    s.transport.close().await?;
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_ordinary_members_cannot_add_or_open_another_persons_ask_attachment()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(
        self::ordinary_members_cannot_add_or_open_another_persons_ask_attachment(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn keeping_a_temporary_ask_requires_its_current_owner() -> Proof {
    use orch_api_contract::ask::{AskKeep, AskKeepBody, AskKeepReply};
    let (s, _) = Scene::new().await?;
    let member = member_bearer(&s).await?;
    let mut conn = s.transport.test.pool.acquire().await?;
    let source = ask::get(&mut conn, s.thread, false)
        .await?
        .ok_or("the actual source is absent")?;
    let topic: Id = sqlx::query_scalar("SELECT parent_id FROM nodes WHERE id=$1")
        .bind(s.thread)
        .fetch_one(&mut *conn)
        .await?;
    drop(conn);
    let body = AskKeepBody {
        topic_id: Some(topic),
        ..Default::default()
    };
    let before = business_state(&s).await?;
    let counts = conversation_counts(&s).await?;
    let refused = s
        .app
        .call(AskKeep, &[&s.thread])
        .token(&member)
        .body(&body)
        .send_bytes()
        .await;
    hidden(&refused);
    assert_eq!(business_state(&s).await?, before);
    assert_eq!(conversation_counts(&s).await?, counts);
    let kept = s
        .app
        .call(AskKeep, &[&s.thread])
        .token(&s.bearer)
        .body(&body)
        .send_bytes()
        .await;
    assert_eq!(
        kept.status,
        http::StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&kept.raw)
    );
    let kept: AskKeepReply = serde_json::from_slice(&kept.raw)?;
    assert_eq!(kept.conversation_id, s.thread);
    assert_eq!(kept.topic_id, topic);
    assert_ne!(kept.thread_id, s.thread);
    let mut conn = s.transport.test.pool.acquire().await?;
    let permanent = ask::get(&mut conn, kept.thread_id, false)
        .await?
        .ok_or("the kept thread is absent")?;
    assert!(!permanent.temporary);
    assert_eq!(permanent.user_id, source.user_id);
    assert_eq!(permanent.root_org_id, source.root_org_id);
    let old = ask::get(&mut conn, s.thread, false)
        .await?
        .ok_or("the temporary source is absent")?;
    assert_eq!(old.state, "kept");
    drop(conn);
    let after = conversation_counts(&s).await?;
    assert_eq!(
        after["nodes"].as_i64(),
        counts["nodes"].as_i64().map(|n| n + 1)
    );
    for unchanged in ["inputs", "turns", "calls"] {
        assert_eq!(after[unchanged], counts[unchanged]);
    }
    s.app.app.engine.stop();
    s.transport.close().await?;
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_keeping_a_temporary_ask_requires_its_current_owner()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(self::keeping_a_temporary_ask_requires_its_current_owner())
}

async fn listen_for_ask_changes(s: &Scene) -> Proof {
    s.app.app.realtime.start(s.app.app.engine.clone());
    within(s.app.app.realtime.listening()).await?;
    s.app.app.realtime.sweep().await?;
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
async fn idle_ask_streams_read_once_then_wake_on_committed_events_and_resume_their_own_cursor()
-> Proof {
    use futures::StreamExt as _;
    use orch_api_contract::ask::{AskEvents, AskEventsQuery};
    let (s, _) = Scene::new().await?;
    listen_for_ask_changes(&s).await?;
    let reads = crate::ask::events::reads::Probe::new(s.thread);
    let mut readers = Vec::new();
    for _ in 0..3 {
        let response = s
            .app
            .call(AskEvents, &[&s.thread])
            .token(&s.bearer)
            .query(&AskEventsQuery { after: Some(0) })
            .send_response()
            .await;
        assert_eq!(response.status(), http::StatusCode::OK);
        readers.push(tokio::spawn(async move {
            response.into_body().into_data_stream().next().await
        }));
    }
    within(reads.wait_for(3)).await?;
    // This observation covers two old half-second polls, not a product timeout or work limit.
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    assert_eq!(reads.reads(), 3, "idle readers must not reload event pages");
    let mut conn = s.app.app.db.acquire().await?;
    let first = ask::emit(
        &mut conn,
        s.thread,
        &json!({"kind":"notice","code":"ready","message":"A committed event"}),
    )
    .await?;
    drop(conn);
    let first_id = first.to_string();
    for reader in readers {
        let frame = within(reader).await??.ok_or("the event stream ended")??;
        let frame = String::from_utf8_lossy(&frame);
        assert!(frame.lines().any(|line| {
            line.strip_prefix("event:")
                .is_some_and(|value| value.trim() == "ask")
        }));
        assert!(frame.contains("A committed event"));
        assert!(frame.lines().any(|line| {
            line.strip_prefix("id:")
                .is_some_and(|value| value.trim() == first_id)
        }));
    }
    let last = first.to_string();
    let request = s
        .app
        .call(AskEvents, &[&s.thread])
        .token(&s.bearer)
        .header("last-event-id", &last);
    let response = request.send_response().await;
    assert_eq!(response.status(), http::StatusCode::OK);
    let reader = tokio::spawn(async move { response.into_body().into_data_stream().next().await });
    let mut conn = s.app.app.db.acquire().await?;
    let second = ask::emit(
        &mut conn,
        s.thread,
        &json!({"kind":"notice","code":"ready","message":"Only the later event"}),
    )
    .await?;
    drop(conn);
    let frame = within(reader).await??.ok_or("the resumed stream ended")??;
    let frame = String::from_utf8_lossy(&frame);
    let second_id = second.to_string();
    assert!(frame.lines().any(|line| {
        line.strip_prefix("id:")
            .is_some_and(|value| value.trim() == second_id)
    }));
    assert!(frame.contains("Only the later event"));
    assert!(!frame.contains("A committed event"));
    assert!(
        !s.app
            .app
            .realtime
            .inner
            .registry()
            .sources
            .contains_key(&crate::realtime::Source::Thread(s.thread)),
        "the last internal reader must release its shared repair entry"
    );
    s.app.app.realtime.stop().await;
    s.app.app.engine.stop();
    s.transport.close().await?;
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_idle_ask_streams_read_once_then_wake_on_committed_events_and_resume_their_own_cursor()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(
        self::idle_ask_streams_read_once_then_wake_on_committed_events_and_resume_their_own_cursor(
        ),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn the_shared_thread_sweep_repairs_an_ask_event_without_a_notification() -> Proof {
    use futures::StreamExt as _;
    use orch_api_contract::ask::{AskEvents, AskEventsQuery};
    let (s, _) = Scene::new().await?;
    listen_for_ask_changes(&s).await?;
    let reads = crate::ask::events::reads::Probe::new(s.thread);
    let response = s
        .app
        .call(AskEvents, &[&s.thread])
        .token(&s.bearer)
        .query(&AskEventsQuery { after: Some(0) })
        .send_response()
        .await;
    assert_eq!(response.status(), http::StatusCode::OK);
    let reader = tokio::spawn(async move { response.into_body().into_data_stream().next().await });
    within(reads.wait_for(1)).await?;
    let mut tx = s.app.app.db.begin().await?;
    let seq = orch_db::sequences::next(&mut tx, s.thread)
        .await?
        .ok_or("the actual thread is absent")?;
    // The same sequenced row, deliberately without emit's commit notification.
    sqlx::query("INSERT INTO ask_events(thread_id,seq,body) VALUES($1,$2,$3)")
        .bind(s.thread)
        .bind(seq)
        .bind(json!({"kind":"notice","code":"ready","message":"Recovered by shared repair"}))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    s.app.app.realtime.sweep().await?;
    let frame = within(reader)
        .await??
        .ok_or("the repaired stream ended")??;
    assert!(String::from_utf8_lossy(&frame).contains("Recovered by shared repair"));
    s.app.app.realtime.stop().await;
    s.app.app.engine.stop();
    s.transport.close().await?;
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_the_shared_thread_sweep_repairs_an_ask_event_without_a_notification()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(
        self::the_shared_thread_sweep_repairs_an_ask_event_without_a_notification(),
    )
}

async fn idle_ask_stream_ends(expire_original: bool) -> Proof {
    use futures::StreamExt as _;
    use orch_api_contract::ask::{AskEvents, AskEventsQuery};
    let (s, _) = Scene::new().await?;
    listen_for_ask_changes(&s).await?;
    let reads = crate::ask::events::reads::Probe::new(s.thread);
    let response = s
        .app
        .call(AskEvents, &[&s.thread])
        .token(&s.bearer)
        .query(&AskEventsQuery { after: Some(0) })
        .send_response()
        .await;
    assert_eq!(response.status(), http::StatusCode::OK);
    let reader = tokio::spawn(async move { response.into_body().into_data_stream().next().await });
    within(reads.wait_for(1)).await?;
    let mut conn = s.app.app.db.acquire().await?;
    if expire_original {
        s.transport.clock.advance(chrono::Duration::seconds(10));
        sqlx::query("UPDATE access_tokens SET expires_at=$2 WHERE token_hash=$1")
            .bind(authentication::token_hash(&s.bearer))
            .bind(s.transport.clock.now() + chrono::Duration::hours(1))
            .execute(&mut *conn)
            .await?;
    } else {
        let (session, person): (Id, Id) = sqlx::query_as(
            "SELECT s.id,s.user_id FROM sessions s JOIN access_tokens a ON a.session_id=s.id WHERE a.token_hash=$1",
        )
        .bind(authentication::token_hash(&s.bearer))
        .fetch_one(&mut *conn)
        .await?;
        assert!(
            identity::revoke_session(&mut conn, session, person, s.transport.clock.now()).await?
        );
    }
    drop(conn);
    assert!(
        within(reader).await??.is_none(),
        "an idle stream must close at its original lifetime or actual identity revocation"
    );
    s.app.app.realtime.stop().await;
    s.app.app.engine.stop();
    s.transport.close().await?;
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
async fn an_idle_ask_stream_ends_at_its_original_access_deadline_without_new_rows() -> Proof {
    idle_ask_stream_ends(true).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_an_idle_ask_stream_ends_at_its_original_access_deadline_without_new_rows()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(
        self::an_idle_ask_stream_ends_at_its_original_access_deadline_without_new_rows(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn an_idle_ask_stream_wakes_on_its_own_session_revocation_without_new_rows() -> Proof {
    idle_ask_stream_ends(false).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_an_idle_ask_stream_wakes_on_its_own_session_revocation_without_new_rows()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(
        self::an_idle_ask_stream_wakes_on_its_own_session_revocation_without_new_rows(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn an_idle_ask_stream_rechecks_its_root_after_a_committed_permission_epoch_change() -> Proof {
    use futures::StreamExt as _;
    use orch_api_contract::ask::{AskEvents, AskEventsQuery};
    let (s, _) = Scene::new().await?;
    listen_for_ask_changes(&s).await?;
    let reads = crate::ask::events::reads::Probe::new(s.thread);
    let response = s
        .app
        .call(AskEvents, &[&s.thread])
        .token(&s.bearer)
        .query(&AskEventsQuery { after: Some(0) })
        .send_response()
        .await;
    assert_eq!(response.status(), http::StatusCode::OK);
    let reader = tokio::spawn(async move { response.into_body().into_data_stream().next().await });
    within(reads.wait_for(1)).await?;
    let mut tx = s.app.app.db.begin().await?;
    let (root, person): (Id, Id) = sqlx::query_as(
        "SELECT t.ask_root_org_id,n.owner_id FROM threads t JOIN nodes n ON n.id=t.node_id WHERE t.node_id=$1",
    )
    .bind(s.thread)
    .fetch_one(&mut *tx)
    .await?;
    access::suspend_member(
        &mut tx,
        root,
        person,
        "The governing root is paused for this reader",
        person,
        s.transport.clock.now(),
    )
    .await?;
    access::bump_epoch(&mut tx).await?;
    tx.commit().await?;
    assert!(
        within(reader).await??.is_none(),
        "the Ask operation must be rechecked without needing a conversation event"
    );
    s.app.app.realtime.stop().await;
    s.app.app.engine.stop();
    s.transport.close().await?;
    Ok(())
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_an_idle_ask_stream_rechecks_its_root_after_a_committed_permission_epoch_change()
-> ::futures::future::LocalBoxFuture<'static, crate::backplane::command_admission_tests::Proof> {
    ::std::boxed::Box::pin(
        self::an_idle_ask_stream_rechecks_its_root_after_a_committed_permission_epoch_change(),
    )
}
