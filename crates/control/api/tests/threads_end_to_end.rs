//! A thread goes the whole way through HTTP: the first admin sets the system up, a person makes a thread in a topic and sends an input, a scripted machine answers the commands and streams a reply, and the thread reads back; a stranger is refused with what is missing, and a viewer reads and cannot send.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::auth::{SetupCreate, SetupCreateBody};
use orch_api_contract::binding::EnvironmentChoice;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, ThreadCreate};
use orch_api_contract::threads::{
    ContentPart, Delivery, InputState, ThreadInputCreate, ThreadInputCreateBody, ThreadState,
    ThreadSyncGet, ThreadSyncGetQuery, TurnState,
};
use places::{Who, join, missing_permissions};
use rig::{MODEL, Rig, said};
use serde_json::json;
use support::{PASSWORD, Person};

fn text(words: &str) -> Vec<ContentPart> {
    vec![ContentPart::Text {
        text: words.to_owned(),
    }]
}

#[tokio::test]
async fn a_thread_goes_from_its_making_to_the_agents_answer_through_the_whole_system() {
    let rig = Rig::new().await;
    let world = &rig.world;

    // The first admin sets the system up and makes the place the work is in.
    let opened = world
        .app
        .call(SetupCreate, &[])
        .body(&SetupCreateBody {
            username: "root".to_owned(),
            display_name: "Root".to_owned(),
            password: PASSWORD.to_owned(),
            email: None,
            timezone: None,
        })
        .send()
        .await
        .ok();
    let root = Who {
        person: Person {
            id: opened.user.user.id,
            username: "root".to_owned(),
            email: String::new(),
            personal_org: orch_types::Id::nil(),
        },
        token: opened.access_token,
    };
    assert!(opened.user.is_system_admin);
    let tree = rig.tree(&root).await;
    let on = rig.resources(&root, &tree).await;

    // Ada works in the org and may use what the thread runs on; Bea is a stranger to it all; Cai may read.
    let org = tree.org.base.id;
    let ada = rig.person("ada").await;
    let bea = rig.person("bea").await;
    let cai = rig.person("cai").await;
    join(world, org, org, &ada, "developer", &root).await;
    join(world, org, org, &cai, "viewer", &root).await;
    rig.lets_use("provider", on.provider, "provider.use", &ada, json!({}))
        .await;
    rig.lets_use("agent", on.agent, "agent.use", &ada, json!({}))
        .await;
    rig.lets_use(
        "environment",
        on.environment,
        "environment.manage",
        &ada,
        json!({}),
    )
    .await;

    // Ada makes a thread in the topic, bound to the agent, the provider and the environment.
    let thread = world
        .app
        .call(ThreadCreate, &[&tree.topic.base.id])
        .token(&ada.token)
        .body(&CreateThread {
            name: Some("Fix login".to_owned()),
            binding: Some(CreateThreadBinding {
                agent_id: on.agent,
                provider_id: on.provider,
                arrangement: None,
            }),
            model: Some(MODEL.to_owned()),
            environment: Some(EnvironmentChoice::Use(on.environment)),
            directory: Some("web".to_owned()),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok();
    let id = thread.base.id;
    assert_eq!(thread.base.name, "Fix login");
    assert_eq!(thread.topic_id, tree.topic.base.id);
    assert_eq!(thread.state, ThreadState::Idle);
    assert_eq!(thread.model.as_deref(), Some(MODEL));
    assert_eq!(thread.directory, "web");
    assert_eq!(thread.base.owner.username, "ada");
    let binding = thread.binding.as_ref().expect("the thread is bound");
    assert_eq!(binding.agent.id, on.agent);
    assert_eq!(binding.provider.id, on.provider);
    assert_eq!(
        thread
            .environment
            .as_ref()
            .map(|environment| environment.id),
        Some(on.environment)
    );
    assert!(thread.input_access.allowed, "{:?}", thread.input_access);

    // She says something. The machine is asked, in order, to start the agent, open a session and
    // start a turn; the scripted harness answers the three and streams a reply.
    let sent = world
        .app
        .call(ThreadInputCreate, &[&id])
        .token(&ada.token)
        .body(&ThreadInputCreateBody {
            plan: false,
            content: text("Why does the login redirect loop?"),
            delivery: Delivery::Queue,
            client_id: "first".to_owned(),
            bypass: None,
            native_action: None,
        })
        .send()
        .await;
    assert_eq!(sent.status, 201);
    let sent = sent.ok();
    assert!(matches!(
        sent.input.state,
        InputState::Pending | InputState::Delivering
    ));
    let start = rig.turn_start(id, 0).await;
    let kinds: Vec<_> = rig
        .seen
        .all()
        .iter()
        .map(|command| command.kind().to_string())
        .collect();
    assert_eq!(
        kinds.iter().map(String::as_str).collect::<Vec<_>>(),
        ["agent.start", "session.open", "turn.start"],
        "the three commands, in this order"
    );
    rig.answer(id, &start, "m0", "A stale cookie sends it round.")
        .await;
    rig.say(
        id,
        rig::turn_ended(
            start.turn_id,
            orch_link_contract::events::TurnOutcome::Completed,
        ),
    )
    .await;
    rig.wait_turn(start.turn_id, "completed").await;
    rig.wait_state(id, "idle").await;

    // The sync read over HTTP shows what the user said and what the agent answered.
    let synced = world
        .app
        .call(ThreadSyncGet, &[&id])
        .token(&ada.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(
        said(&synced.items),
        [
            "user: Why does the login redirect loop?",
            "agent: A stale cookie sends it round."
        ]
    );
    assert_eq!(synced.thread.state, ThreadState::Idle);
    assert_eq!(synced.turns.len(), 1);
    assert_eq!(synced.turns[0].state, TurnState::Completed);
    assert_eq!(synced.turns[0].model, MODEL);
    assert_eq!(
        synced.inputs.len(),
        0,
        "an input that was delivered is no longer open"
    );
    assert!(synced.complete && synced.seq >= 1);

    // Bea has nothing here: she can neither read the thread nor send to it, and is told what is missing.
    // Where the place refuses what is written without use of the thread's model (rather than holding
    // it, as shipped), that is missing too.
    sqlx::query("INSERT INTO settings (node_id,key,value) VALUES ($1,'thread.inputs_without_use','\"refused\"'::jsonb)")
        .bind(tree.topic.base.id)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let refused = world
        .app
        .call(ThreadSyncGet, &[&id])
        .token(&bea.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&refused), ["thread.view"]);
    let refused = world
        .app
        .call(ThreadInputCreate, &[&id])
        .token(&bea.token)
        .body(&ThreadInputCreateBody {
            plan: false,
            content: text("Let me in"),
            delivery: Delivery::Queue,
            client_id: "bea-1".to_owned(),
            bypass: None,
            native_action: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let lacking = missing_permissions(&refused);
    assert!(
        lacking.contains(&"thread.interact".to_owned()),
        "{lacking:?}"
    );
    assert!(lacking.contains(&"provider.use".to_owned()), "{lacking:?}");

    // Cai may read the thread and cannot send to it.
    let read = world
        .app
        .call(ThreadSyncGet, &[&id])
        .token(&cai.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(said(&read.items).len(), 2);
    assert!(!read.thread.input_access.allowed);
    let refused = world
        .app
        .call(ThreadInputCreate, &[&id])
        .token(&cai.token)
        .body(&ThreadInputCreateBody {
            plan: false,
            content: text("Me too"),
            delivery: Delivery::Queue,
            client_id: "cai-1".to_owned(),
            bypass: None,
            native_action: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert!(missing_permissions(&refused).contains(&"thread.interact".to_owned()));
    assert_eq!(
        rig.turn_starts(id).len(),
        1,
        "nobody who was refused started a turn"
    );
    rig.engine.stop();
}
