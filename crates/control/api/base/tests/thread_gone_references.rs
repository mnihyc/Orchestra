//! A thread opens with its whole history after something it runs with is deleted: its provider, its
//! provider's model, its agent profile or its environment. Its parameters name what is gone, once, for the
//! composer's one line, and the topic's list, Home, name search and content search keep the thread.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::environment::{EnvironmentDelete, EnvironmentDeleteQuery};
use orch_api_contract::nodes::{
    NodeChildList, NodeChildListQuery, OrgOverviewGet, OrgOverviewGetQuery, OrgOverviewResponse,
};
use orch_api_contract::personal::{MeVisitCreate, PersonalTarget};
use orch_api_contract::resources::{AgentDelete, ProviderDelete, ProviderModelDelete};
use orch_api_contract::search::{
    NameKind, NameSearchList, NameSearchListQuery, SearchFound, SearchList, SearchListQuery,
};
use orch_api_contract::settings::{ThreadGone, ThreadGoneKind, ThreadParameterGet};
use orch_api_contract::threads::{ThreadOpeningGet, ThreadOpeningGetQuery};
use orch_types::Id;
use rig::{MODEL, Scene, said};
use serde_json::json;

/// Ada's thread with one finished exchange, visited so that Home lists it among her recent threads.
async fn with_history(s: &Scene) -> Id {
    let id = s.thread().await.base.id;
    s.exchange(id, 0, "First question", "First answer").await;
    let visited = s
        .rig
        .world
        .app
        .call(MeVisitCreate, &[])
        .token(&s.ada.token)
        .body(&PersonalTarget {
            kind: NameKind::Thread,
            id,
        })
        .send()
        .await;
    assert_eq!(visited.status, 204, "{:?}", visited.body);
    id
}

/// The thread opens with its whole history, and both readers of its parameters name exactly `gone`.
async fn opens_with(s: &Scene, thread: Id, gone: &[ThreadGone]) {
    let app = &s.rig.world.app;
    let opening = app
        .call(ThreadOpeningGet, &[&thread])
        .token(&s.ada.token)
        .query(&ThreadOpeningGetQuery::default())
        .send()
        .await;
    assert_eq!(opening.status, 200, "{:?}", opening.body);
    let opening = opening.ok();
    assert_eq!(
        said(&opening.sync.items),
        ["user: First question", "agent: First answer"],
        "the history is whole"
    );
    assert_eq!(opening.sync.thread.base.id, thread);
    let parameters = opening
        .parameters
        .expect("the opening keeps the thread's parameters");
    assert_eq!(parameters.gone, gone, "the opening names what is gone");
    let read = app
        .call(ThreadParameterGet, &[&thread])
        .token(&s.ada.token)
        .send()
        .await;
    assert_eq!(read.status, 200, "{:?}", read.body);
    assert_eq!(read.ok().gone, gone, "the parameters name what is gone");
}

/// The topic's list, Home's recent threads, name search and content search all keep the thread.
async fn still_listed(s: &Scene, thread: Id) {
    let app = &s.rig.world.app;
    let topic = app
        .call(NodeChildList, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .query(&NodeChildListQuery::default())
        .send()
        .await;
    assert_eq!(topic.status, 200, "{:?}", topic.body);
    let rows = serde_json::to_value(topic.ok()).unwrap();
    assert!(
        rows["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == json!(thread)),
        "the topic lists the thread: {rows}"
    );
    let home = app
        .call(OrgOverviewGet, &[&s.tree.org.base.id])
        .token(&s.ada.token)
        .query(&OrgOverviewGetQuery::default())
        .send()
        .await;
    assert_eq!(home.status, 200, "{:?}", home.body);
    let OrgOverviewResponse::Overview { data } = home.ok() else {
        panic!("Home reads its overview");
    };
    assert!(
        data.recent.items.iter().any(|hit| hit.id == thread),
        "Home keeps the thread among the recent ones"
    );
    let named = app
        .call(NameSearchList, &[])
        .token(&s.ada.token)
        .query(&NameSearchListQuery {
            q: Some("Fix login".to_owned()),
            kinds: Some("thread".to_owned()),
            ..Default::default()
        })
        .send()
        .await;
    assert_eq!(named.status, 200, "{:?}", named.body);
    assert!(
        named.ok().items.iter().any(|hit| hit.id == thread),
        "name search finds the thread"
    );
    let found = app
        .call(SearchList, &[])
        .token(&s.ada.token)
        .query(&SearchListQuery {
            q: "First answer".to_owned(),
            immediate: Some(true),
            ..Default::default()
        })
        .send()
        .await;
    assert_eq!(found.status, 200, "{:?}", found.body);
    assert!(
        found.ok().items.iter().any(|hit| matches!(
            &hit.found,
            SearchFound::Item { thread: found, .. } if found.id == thread
        )),
        "content search finds what was said in the thread"
    );
}

#[tokio::test]
async fn a_thread_whose_provider_was_deleted_opens_with_its_history_and_names_the_provider() {
    let s = Scene::new().await;
    let thread = with_history(&s).await;
    let deleted = s
        .rig
        .world
        .app
        .call(ProviderDelete, &[&s.on.provider])
        .token(&s.ada.token)
        .send()
        .await;
    assert_eq!(deleted.status, 204, "{:?}", deleted.body);
    opens_with(
        &s,
        thread,
        &[ThreadGone {
            kind: ThreadGoneKind::Provider,
            id: s.on.provider,
            name: "Fake subscription".to_owned(),
        }],
    )
    .await;
    still_listed(&s, thread).await;
}

#[tokio::test]
async fn a_thread_whose_model_was_removed_from_its_provider_opens_and_names_the_model() {
    let s = Scene::new().await;
    let thread = with_history(&s).await;
    let model: Id =
        sqlx::query_scalar("SELECT id FROM provider_models WHERE provider_id=$1 AND model_name=$2")
            .bind(s.on.provider)
            .bind(MODEL)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    let removed = s
        .rig
        .world
        .app
        .call(ProviderModelDelete, &[&s.on.provider, &model])
        .token(&s.ada.token)
        .send()
        .await;
    assert_eq!(removed.status, 204, "{:?}", removed.body);
    opens_with(
        &s,
        thread,
        &[ThreadGone {
            kind: ThreadGoneKind::Model,
            id: s.on.provider,
            name: MODEL.to_owned(),
        }],
    )
    .await;
    still_listed(&s, thread).await;
}

#[tokio::test]
async fn a_thread_whose_agent_profile_was_deleted_opens_and_names_the_profile() {
    let s = Scene::new().await;
    let thread = with_history(&s).await;
    let deleted = s
        .rig
        .world
        .app
        .call(AgentDelete, &[&s.on.agent])
        .token(&s.ada.token)
        .send()
        .await;
    assert_eq!(deleted.status, 204, "{:?}", deleted.body);
    opens_with(
        &s,
        thread,
        &[ThreadGone {
            kind: ThreadGoneKind::Agent,
            id: s.on.agent,
            name: "Fake agent".to_owned(),
        }],
    )
    .await;
    still_listed(&s, thread).await;
}

#[tokio::test]
async fn a_thread_taken_out_of_its_removed_environment_opens_and_names_the_environment() {
    let s = Scene::new().await;
    let thread = with_history(&s).await;
    // A removal due later takes the thread out of the environment at once and queues no job yet.
    let at = orch_types::Time::new(s.rig.world.now() + chrono::Duration::hours(1));
    let removed = s
        .rig
        .world
        .app
        .call(EnvironmentDelete, &[&s.on.environment])
        .token(&s.ada.token)
        .query(&EnvironmentDeleteQuery {
            confirm: "Workspace".to_owned(),
            at: Some(at),
            expected_keep_days: None,
            expected_revision: None,
        })
        .send()
        .await;
    assert_eq!(removed.status, 202, "{:?}", removed.body);
    assert_eq!(
        s.rig.state(thread).await,
        ("blocked".to_owned(), Some("unbound".to_owned()))
    );
    opens_with(
        &s,
        thread,
        &[ThreadGone {
            kind: ThreadGoneKind::Environment,
            id: s.on.environment,
            name: "Workspace".to_owned(),
        }],
    )
    .await;
    still_listed(&s, thread).await;
}

#[tokio::test]
async fn a_thread_with_nothing_gone_names_nothing() {
    let s = Scene::new().await;
    let thread = with_history(&s).await;
    opens_with(&s, thread, &[]).await;
    still_listed(&s, thread).await;
}

#[tokio::test]
async fn the_machine_of_its_agent_profile_is_named_once_deleted_and_not_while_switched_off() {
    let s = Scene::new().await;
    let thread = with_history(&s).await;
    let db = s.rig.db.pool();
    // The thread's agent profile is the machine's own.
    sqlx::query("UPDATE agents SET machine_id = $2 WHERE id = $1")
        .bind(s.on.agent)
        .bind(s.rig.machine_id)
        .execute(db)
        .await
        .unwrap();
    // Switched off, the machine is still there to be switched on again: nothing is gone.
    sqlx::query("UPDATE machines SET status = 'revoked', online = false WHERE id = $1")
        .bind(s.rig.machine_id)
        .execute(db)
        .await
        .unwrap();
    opens_with(&s, thread, &[]).await;
    sqlx::query("UPDATE machines SET deleted_at = clock_timestamp() WHERE id = $1")
        .bind(s.rig.machine_id)
        .execute(db)
        .await
        .unwrap();
    let read = s
        .rig
        .world
        .app
        .call(ThreadParameterGet, &[&thread])
        .token(&s.ada.token)
        .send()
        .await;
    assert_eq!(read.status, 200, "{:?}", read.body);
    let gone = read.ok().gone;
    assert!(
        gone.len() == 1
            && matches!(gone[0].kind, ThreadGoneKind::Machine)
            && gone[0].id == s.rig.machine_id,
        "the deleted machine is named: {gone:?}"
    );
}
