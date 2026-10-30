//! A permission that is no use without something on another object shows it beside it, says whether the people concerned have it, and offers to give it in the same step.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::pages::{Holder, PageChange};
use orch_access_contract::permissions::{ObjectKind, Permission as P};
use orch_access_contract::roles::Role;
use orch_authz::Caller;
use orch_authz::depends::{self, Dependency, Offer, Open, Starting};
use orch_authz::usable::Policies;
use orch_types::Id;
use serde_json::json;
use support::World;

async fn of_thread(
    w: &World,
    who: Id,
    thread: Id,
    provider: Id,
    model: &str,
    effort: Option<&str>,
) -> Dependency {
    let principal = w.principal(Caller::session(who)).await;
    let mut conn = w.conn().await;
    depends::of_thread(
        &mut conn,
        &principal,
        thread,
        provider,
        model,
        effort,
        &Policies::default(),
    )
    .await
    .unwrap()
    .remove(0)
}

#[tokio::test]
async fn the_model_a_thread_runs_on_is_shown_with_whether_everyone_who_can_talk_to_it_has_it() {
    let w = World::new().await;
    let (ada, bea) = (w.member("ada").await, w.member("bea").await);
    let provider = w.provider(ada, w.root, "Team Pro").await;
    // Nobody but its owner has the provider, to begin with.
    let dependency = of_thread(&w, ada, w.thread, provider, "big", Some("high")).await;
    assert_eq!(dependency.for_permission, P::ThreadInteract);
    assert_eq!(dependency.object.kind, ObjectKind::Provider);
    assert_eq!(dependency.permission, P::ProviderUse);
    assert_eq!(dependency.name, "Team Pro");
    assert_eq!(dependency.owner, Some(ada));
    assert_eq!(dependency.open, Open::Nobody);
    assert!(dependency.can_give, "its owner");
    assert_eq!(
        dependency.options.as_ref().unwrap().models.as_deref(),
        Some(&["big".to_owned()][..])
    );
    assert!(
        !of_thread(&w, bea, w.thread, provider, "big", None)
            .await
            .can_give
    );
    // An entry for the work in the project opens it to everybody there, within its options.
    w.put(
        "provider",
        provider,
        P::ProviderUse,
        Holder::Node { id: w.project },
        json!({ "models": ["small"] }),
    )
    .await;
    assert_eq!(
        of_thread(&w, ada, w.thread, provider, "big", None)
            .await
            .open,
        Open::Nobody,
        "not this model"
    );
    let dependency = of_thread(&w, ada, w.thread, provider, "small", None).await;
    assert_eq!(
        (dependency.open, dependency.through),
        (Open::All, Some(w.project))
    );
    // Without an entry for the work, those who can talk to the thread must each have it. The thread's
    // page names Developers of the org and one person; the provider names Viewers of the org.
    let other = w.node("thread", w.topic, "Other").await;
    for row in [P::ThreadInteract, P::ThreadManage] {
        w.row(other, row, false).await;
    }
    w.name(
        other,
        P::ThreadInteract,
        Holder::Role {
            role: Role::Developer,
            at: Some(w.root),
        },
    )
    .await;
    w.name(other, P::ThreadInteract, Holder::User { id: bea })
        .await;
    assert_eq!(
        of_thread(&w, ada, other, provider, "big", None).await.open,
        Open::Nobody
    );
    w.grant(
        "provider",
        provider,
        P::ProviderUse,
        Holder::Role {
            role: Role::Viewer,
            at: Some(w.root),
        },
    )
    .await;
    // Viewers of the org are every Developer of it, but not the person named, nor the thread's owner.
    assert_eq!(
        of_thread(&w, ada, other, provider, "big", None).await.open,
        Open::Part
    );
    w.grant(
        "provider",
        provider,
        P::ProviderUse,
        Holder::User { id: bea },
    )
    .await;
    assert_eq!(
        of_thread(&w, ada, other, provider, "big", None).await.open,
        Open::Part,
        "the maker owns the thread and talks to it"
    );
    w.grant(
        "provider",
        provider,
        P::ProviderUse,
        Holder::User { id: w.maker },
    )
    .await;
    assert_eq!(
        of_thread(&w, ada, other, provider, "big", None).await.open,
        Open::All
    );
}

#[tokio::test]
async fn the_offer_is_a_change_of_the_providers_page_for_its_manager_and_a_request_for_anybody_else()
 {
    let w = World::new().await;
    let (ada, bea) = (w.member("ada").await, w.member("bea").await);
    // Naming the work of this thread on a resource requires seeing the named thread.
    w.give(w.root, ada, Role::Viewer).await;
    w.give(w.project, bea, Role::Interactor).await;
    let provider = w.provider(ada, w.root, "Team Pro").await;
    let policies = Policies::default();
    // Its owner gives it: the change adds the work of the thread to the Use row with the model in use.
    let dependency = of_thread(&w, ada, w.thread, provider, "big", Some("high")).await;
    let principal = w.principal(Caller::session(ada)).await;
    let mut conn = w.conn().await;
    let Offer::Give { object, change } =
        depends::offer(&mut conn, &principal, &dependency, w.thread, &policies)
            .await
            .unwrap()
    else {
        panic!("the owner gives it");
    };
    drop(conn);
    assert_eq!(object.id, provider);
    let change: PageChange = *change;
    let change = serde_json::to_value(&change).unwrap();
    assert_eq!(change["rows"][0]["permission"], "provider.use");
    assert_eq!(
        change["rows"][0]["add"][0]["holder"],
        json!({ "kind": "node", "id": w.thread })
    );
    assert_eq!(
        change["rows"][0]["add"][0]["options"],
        json!({ "models": ["big"], "efforts": ["high"] })
    );
    w.change_thing(ada, "provider", provider, change)
        .await
        .unwrap();
    let after = of_thread(&w, ada, w.thread, provider, "big", Some("high")).await;
    assert_eq!((after.open, after.through), (Open::All, Some(w.thread)));
    // Somebody who does not manage it asks its owner for the same entry, in the shape of an item of
    // an access request.
    let other_provider = w.provider(ada, w.root, "Other").await;
    let dependency = of_thread(&w, bea, w.thread, other_provider, "big", None).await;
    assert!(!dependency.can_give);
    let principal = w.principal(Caller::session(bea)).await;
    let mut conn = w.conn().await;
    let Offer::Ask { item, holder } =
        depends::offer(&mut conn, &principal, &dependency, w.project, &policies)
            .await
            .unwrap()
    else {
        panic!("anybody else asks");
    };
    assert_eq!(holder, Holder::Node { id: w.project });
    assert_eq!(item.permissions, [P::ProviderUse]);
    assert_eq!(
        item.options.as_ref().unwrap().models.as_deref(),
        Some(&["big".to_owned()][..])
    );
    assert!(item.ask.contains(&ada), "its owner can give it");
}

#[tokio::test]
async fn starting_work_in_a_place_shows_what_it_runs_on_and_where_and_whether_it_is_open_to_the_work_there()
 {
    let w = World::new().await;
    let ada = w.member("ada").await;
    let (provider, profile, host, template) = (
        w.provider(ada, w.root, "Pro").await,
        w.thing("agent", ada, w.root, "Coder").await,
        w.thing("host", ada, w.root, "gpu").await,
        w.thing("template", ada, w.root, "recipe").await,
    );
    let environment = w.thing("environment", ada, w.topic, "main").await;
    let starting = Starting {
        provider: Some(provider),
        profile: Some(profile),
        host: Some(host),
        template: Some(template),
        environment: Some(environment),
    };
    let principal = w.principal(Caller::session(ada)).await;
    let mut conn = w.conn().await;
    let found = depends::of_place(
        &mut conn,
        &principal,
        w.topic,
        &starting,
        &Policies::default(),
    )
    .await
    .unwrap();
    let kinds: Vec<_> = found
        .iter()
        .map(|dependency| (dependency.object.kind, dependency.permission))
        .collect();
    assert_eq!(
        kinds,
        [
            (ObjectKind::Provider, P::ProviderUse),
            (ObjectKind::Agent, P::AgentUse),
            (ObjectKind::Host, P::HostUse),
            (ObjectKind::Template, P::TemplateUse),
            (ObjectKind::Environment, P::EnvironmentUse),
        ]
    );
    assert!(
        found
            .iter()
            .all(|dependency| dependency.for_permission == P::TopicStartThreads)
    );
    // The Developers of the topic start work there as shipped; none of the four is open to them yet,
    // and the environment is usable by the Developers as shipped, so it is.
    let by_kind = |kind: ObjectKind| {
        found
            .iter()
            .find(|dependency| dependency.object.kind == kind)
            .unwrap()
    };
    assert_eq!(by_kind(ObjectKind::Provider).open, Open::Nobody);
    assert_eq!(by_kind(ObjectKind::Environment).open, Open::All);
    drop(conn);
    // Open to the work here: an entry for the topic.
    w.grant(
        "provider",
        provider,
        P::ProviderUse,
        Holder::Node { id: w.topic },
    )
    .await;
    let mut conn = w.conn().await;
    let found = depends::of_place(
        &mut conn,
        &principal,
        w.topic,
        &starting,
        &Policies::default(),
    )
    .await
    .unwrap();
    let provider_dependency = found
        .iter()
        .find(|dependency| dependency.object.kind == ObjectKind::Provider)
        .unwrap();
    assert_eq!(
        (provider_dependency.open, provider_dependency.through),
        (Open::All, Some(w.topic))
    );
}

#[tokio::test]
async fn starting_threads_given_alone_shows_and_can_open_environment_use_to_the_work() {
    let w = World::new().await;
    let (owner, starter) = (w.member("owner").await, w.member("starter").await);
    w.give(w.root, owner, Role::Viewer).await;
    w.give(w.root, starter, Role::Viewer).await;
    w.name(w.topic, P::TopicStartThreads, Holder::User { id: starter })
        .await;
    let environment = w.thing("environment", owner, w.topic, "Shared").await;
    let starting = Starting {
        provider: None,
        profile: None,
        host: None,
        template: None,
        environment: Some(environment),
    };
    let policies = Policies::default();
    let principal = w.principal(Caller::session(owner)).await;
    let mut conn = w.conn().await;
    let dependency = depends::of_place(&mut conn, &principal, w.topic, &starting, &policies)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(dependency.for_permission, P::TopicStartThreads);
    assert_eq!(dependency.permission, P::EnvironmentUse);
    assert_ne!(dependency.open, Open::All);
    let Offer::Give { change, .. } =
        depends::offer(&mut conn, &principal, &dependency, w.topic, &policies)
            .await
            .unwrap()
    else {
        panic!("the owner can open the environment");
    };
    drop(conn);
    let change = serde_json::to_value(&change).unwrap();
    assert_eq!(change["rows"][0]["permission"], "environment.use");
    assert_eq!(
        change["rows"][0]["add"][0]["holder"],
        json!({ "kind": "node", "id": w.topic })
    );
    w.change_thing(owner, "environment", environment, change)
        .await
        .unwrap();
    let mut conn = w.conn().await;
    let after = depends::of_place(&mut conn, &principal, w.topic, &starting, &policies)
        .await
        .unwrap()
        .remove(0);
    assert_eq!((after.open, after.through), (Open::All, Some(w.topic)));
    drop(conn);
    w.name(
        w.project,
        P::ProjectCreateTopics,
        Holder::User { id: starter },
    )
    .await;
    let mut conn = w.conn().await;
    let project = depends::of_place(&mut conn, &principal, w.project, &starting, &policies)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(project.for_permission, P::ProjectCreateTopics);
    assert_eq!(project.permission, P::EnvironmentUse);
}
