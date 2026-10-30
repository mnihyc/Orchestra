//! Lists of resources: what a caller holds anything on, what they may use where they stand and within which options, and what such a list costs in statements.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{Permission as P, ResourceKind};
use orch_access_contract::roles::Role;
use orch_authz::nodes::Page;
use orch_authz::usable::{self, Policies};
use orch_authz::{Caller, Policy, resources};
use orch_db::queries::resources::access_fixtures;
use orch_types::Id;
use serde_json::json;
use support::{Statements, World};

fn ids<T>(items: &[T], id: impl Fn(&T) -> Id) -> Vec<Id> {
    let mut found: Vec<Id> = items.iter().map(id).collect();
    found.sort();
    found
}

fn sorted(mut list: Vec<Id>) -> Vec<Id> {
    list.sort();
    list
}

#[tokio::test]
async fn a_list_shows_what_the_caller_holds_anything_on_and_a_deleted_one_to_nobody() {
    let w = World::new().await;
    let (ada, bea, cat) = (
        w.member("ada").await,
        w.member("bea").await,
        w.member("cat").await,
    );
    w.give(w.root, bea, Role::Viewer).await;
    let stranger = w.user("sam").await;
    let owned = w.provider(ada, w.root, "Mine").await;
    let named = w.provider(ada, w.root, "Named").await;
    w.grant("provider", named, P::ProviderView, Holder::User { id: bea })
        .await;
    let followed = w.provider(ada, w.root, "Followed").await;
    w.name_default(
        Some(w.root),
        P::ProviderView,
        "any",
        Holder::Role {
            role: Role::Viewer,
            at: None,
        },
    )
    .await;
    let private = w.provider(ada, w.root, "Private").await;
    w.thing_row("provider", private, P::ProviderView, false)
        .await;
    let disabled = w.provider(ada, w.root, "Off").await;
    let deleted = w.provider(ada, w.root, "Gone").await;
    {
        let mut conn = w.conn().await;
        access_fixtures::set_disabled(&mut conn, "provider", disabled, true)
            .await
            .unwrap();
        access_fixtures::set_deleted(&mut conn, "provider", deleted, w.now.get())
            .await
            .unwrap();
    }
    // Another tree's provider that names the stranger.
    let other_root = orch_types::new_id();
    {
        let mut conn = w.conn().await;
        orch_db::queries::hierarchy::testing::insert_node(
            &mut conn,
            other_root,
            "org",
            None,
            "Elsewhere",
            ada,
        )
        .await
        .unwrap();
    }
    let abroad = w.provider(ada, other_root, "Abroad").await;
    w.grant(
        "provider",
        abroad,
        P::ProviderView,
        Holder::User { id: stranger },
    )
    .await;
    let seen = |who: Id| {
        let w = &w;
        async move {
            sorted(
                w.visible(who, "provider")
                    .await
                    .into_iter()
                    .map(|seen| seen.resource.id)
                    .collect(),
            )
        }
    };
    // Its owner sees theirs, the disabled one included, and not the deleted one.
    let mine = seen(ada).await;
    assert!(mine.contains(&owned) && mine.contains(&disabled) && mine.contains(&private));
    assert!(!mine.contains(&deleted));
    // A Viewer sees the one that names them and those that follow a default that names Viewers, the
    // disabled one among them; not a private one, nor a deleted one, nor another tree's.
    let theirs = seen(bea).await;
    assert_eq!(theirs, sorted(vec![named, followed, owned, disabled]));
    // The admins of the home see all of it: whose it is and what its page says.
    let boss = seen(w.boss).await;
    assert!(boss.contains(&private) && boss.contains(&owned) && !boss.contains(&abroad));
    // A member with no role sees nothing, and a stranger only what names them.
    assert!(seen(cat).await.is_empty());
    assert_eq!(seen(stranger).await, [abroad]);
    // What they hold on each is told, with how.
    let listed = w.visible(bea, "provider").await;
    let on_named = listed
        .iter()
        .find(|seen| seen.resource.id == named)
        .unwrap();
    assert_eq!(
        on_named
            .held
            .iter()
            .map(|held| held.permission)
            .collect::<Vec<_>>(),
        [P::ProviderView]
    );
    assert!(!on_named.owner && !on_named.administers);
    let on_owned = w.visible(ada, "provider").await;
    assert!(
        on_owned
            .iter()
            .find(|seen| seen.resource.id == owned)
            .unwrap()
            .owner
    );
}

#[tokio::test]
async fn a_list_pages_by_identifier_and_says_where_the_next_page_begins() {
    let w = World::new().await;
    let ada = w.member("ada").await;
    let mut all = Vec::new();
    for index in 0..5 {
        all.push(w.provider(ada, w.root, &format!("P{index}")).await);
    }
    let principal = w.principal(Caller::session(ada)).await;
    let mut conn = w.conn().await;
    let first = resources::visible(
        &mut conn,
        &principal,
        ResourceKind::Provider,
        Page {
            after: None,
            limit: 2,
        },
        Policy::default(),
    )
    .await
    .unwrap();
    assert_eq!(first.items.len(), 2);
    assert_eq!(first.next, Some(first.items[1].resource.id));
    let second = resources::visible(
        &mut conn,
        &principal,
        ResourceKind::Provider,
        Page {
            after: first.next,
            limit: 2,
        },
        Policy::default(),
    )
    .await
    .unwrap();
    let third = resources::visible(
        &mut conn,
        &principal,
        ResourceKind::Provider,
        Page {
            after: second.next,
            limit: 2,
        },
        Policy::default(),
    )
    .await
    .unwrap();
    assert_eq!(third.items.len(), 1);
    assert_eq!(third.next, None);
    let seen: Vec<Id> = first
        .items
        .iter()
        .chain(&second.items)
        .chain(&third.items)
        .map(|seen| seen.resource.id)
        .collect();
    assert_eq!(
        seen,
        sorted(all),
        "in the order of their identifiers, each once"
    );
}

#[tokio::test]
async fn what_a_person_may_use_where_they_stand_is_listed_with_the_options_that_apply_to_them() {
    let w = World::new().await;
    let (ada, bea) = (w.member("ada").await, w.member("bea").await);
    w.give(w.project, bea, Role::Interactor).await;
    let by_name = w.provider(ada, w.root, "Named").await;
    w.put(
        "provider",
        by_name,
        P::ProviderUse,
        Holder::User { id: bea },
        json!({ "models": ["small"] }),
    )
    .await;
    let by_work = w.provider(ada, w.root, "ByWork").await;
    w.grant(
        "provider",
        by_work,
        P::ProviderUse,
        Holder::Node { id: w.project },
    )
    .await;
    let looking = w.provider(ada, w.root, "Looking").await;
    w.grant(
        "provider",
        looking,
        P::ProviderView,
        Holder::User { id: bea },
    )
    .await;
    let off = w.provider(ada, w.root, "Off").await;
    w.grant("provider", off, P::ProviderUse, Holder::User { id: bea })
        .await;
    {
        let mut conn = w.conn().await;
        access_fixtures::set_disabled(&mut conn, "provider", off, true)
            .await
            .unwrap();
    }
    // Where they stand: a topic of the project the work is named for.
    let here = w.usable_list(bea, "provider", Some(w.topic)).await;
    assert_eq!(
        ids(&here, |item| item.resource.id),
        sorted(vec![by_name, by_work])
    );
    let named = here
        .iter()
        .find(|item| item.resource.id == by_name)
        .unwrap();
    assert_eq!(
        named.granted.provider().models.as_deref(),
        Some(&["small".to_owned()][..])
    );
    let work = here
        .iter()
        .find(|item| item.resource.id == by_work)
        .unwrap();
    assert_eq!(
        work.granted.provider(),
        Default::default(),
        "the work in the project has no limit"
    );
    assert_eq!(work.granted.work[0].place, w.project);
    // Nowhere in particular, the work in a place names no person.
    let anywhere = w.usable_list(bea, "provider", None).await;
    assert_eq!(ids(&anywhere, |item| item.resource.id), [by_name]);
    // In a topic of another project it does not reach.
    let other_project = w.node("project", w.root, "Other").await;
    let other_topic = w.node("topic", other_project, "Elsewhere").await;
    let there = w.usable_list(bea, "provider", Some(other_topic)).await;
    assert_eq!(ids(&there, |item| item.resource.id), [by_name]);
    // The owner uses theirs, with no limit, and the disabled one is not among them.
    let mine = w.usable_list(ada, "provider", None).await;
    assert!(
        mine.iter()
            .all(|item| item.granted.provider() == Default::default())
    );
    assert_eq!(mine.len(), 3, "not the disabled one");
    // An environment is not used but managed: those the caller manages are what may be put work in.
    let environment = w.thing("environment", ada, w.topic, "main").await;
    let hold = w.member("hold").await;
    w.give(w.project, hold, Role::Developer).await;
    let managed = w.usable_list(hold, "environment", Some(w.topic)).await;
    assert_eq!(
        ids(&managed, |item| item.resource.id),
        [environment],
        "the Developers manage it as shipped"
    );
    assert!(
        w.usable_list(bea, "environment", Some(w.topic))
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn a_host_of_the_system_open_to_everyone_is_listed_for_every_member() {
    let w = World::new().await;
    let (ada, stranger) = (w.member("ada").await, w.user("sam").await);
    let shared = w.system_thing("host", "shared").await;
    w.grant("host", shared, P::HostUse, Holder::Everyone).await;
    let mine = w.thing("host", ada, w.root, "gpu").await;
    for person in [ada, stranger] {
        let found = w.usable_list(person, "host", None).await;
        assert!(
            found.iter().any(|item| item.resource.id == shared),
            "{person}"
        );
    }
    assert_eq!(
        ids(&w.usable_list(ada, "host", None).await, |item| item
            .resource
            .id),
        sorted(vec![shared, mine])
    );
    assert_eq!(
        ids(&w.usable_list(stranger, "host", None).await, |item| item
            .resource
            .id),
        [shared]
    );
}

/// The statements a list of what a person may use takes.
async fn statements_of_usable(w: &World, who: Id, node: Id) -> (usize, usize) {
    let principal = w.principal(Caller::session(who)).await;
    let mut conn = w.conn().await;
    let before = Statements::now();
    let listing = usable::usable_list(
        &mut conn,
        &principal,
        ResourceKind::Provider,
        Some(node),
        &Policies::default(),
        Page {
            after: None,
            limit: 1000,
        },
    )
    .await
    .unwrap();
    (Statements::now() - before, listing.items.len())
}

async fn statements_of_visible(w: &World, who: Id) -> (usize, usize) {
    let principal = w.principal(Caller::session(who)).await;
    let mut conn = w.conn().await;
    let before = Statements::now();
    let listing = resources::visible(
        &mut conn,
        &principal,
        ResourceKind::Provider,
        Page {
            after: None,
            limit: 1000,
        },
        Policy::default(),
    )
    .await
    .unwrap();
    (Statements::now() - before, listing.items.len())
}

#[tokio::test]
async fn the_statements_of_a_list_of_what_may_be_used_do_not_grow_with_the_number_of_resources() {
    let w = World::new().await;
    let (ada, bea) = (w.member("ada").await, w.member("bea").await);
    w.give(w.root, bea, Role::Interactor).await;
    let group = w.group(w.root, "Ops", &[bea]).await;
    // One of each way in, so that growing the list only repeats what is there.
    let make = |index: usize| {
        let w = &w;
        async move {
            let provider = w.provider(ada, w.root, &format!("P{index}")).await;
            match index % 5 {
                0 => {
                    w.put(
                        "provider",
                        provider,
                        P::ProviderUse,
                        Holder::User { id: bea },
                        json!({ "models": ["small"] }),
                    )
                    .await;
                }
                1 => {
                    w.grant(
                        "provider",
                        provider,
                        P::ProviderUse,
                        Holder::Group { id: group },
                    )
                    .await;
                }
                2 => {
                    w.grant(
                        "provider",
                        provider,
                        P::ProviderUse,
                        Holder::Node { id: w.project },
                    )
                    .await;
                }
                3 => {
                    w.grant(
                        "provider",
                        provider,
                        P::ProviderUse,
                        Holder::Role {
                            role: Role::Interactor,
                            at: None,
                        },
                    )
                    .await;
                }
                _ => {
                    w.grant(
                        "provider",
                        provider,
                        P::ProviderView,
                        Holder::User { id: bea },
                    )
                    .await;
                }
            }
            provider
        }
    };
    for index in 0..3 {
        make(index).await;
    }
    let (three, three_found) = statements_of_usable(&w, bea, w.topic).await;
    assert_eq!(three_found, 3, "a person, a group and the work in a place");
    for index in 3..5 {
        make(index).await;
    }
    let (few, few_found) = statements_of_usable(&w, bea, w.topic).await;
    let (few_seen, few_visible) = statements_of_visible(&w, bea).await;
    assert_eq!(few_found, 4, "four ways in, and one that only looks");
    for index in 5..60 {
        make(index).await;
    }
    let (many, many_found) = statements_of_usable(&w, bea, w.topic).await;
    let (many_seen, many_visible) = statements_of_visible(&w, bea).await;
    assert_eq!(many_found, 48, "four of every five");
    assert_eq!(
        (few_visible, many_visible),
        (4, 48),
        "all but the ones that name only the work in a place"
    );
    assert_eq!(few, many, "the same statements for 5 resources and for 60");
    assert_eq!(three, many, "and for 3");
    assert_eq!(
        few_seen, many_seen,
        "and for the list of what they hold anything on"
    );
    eprintln!("statements: a list of what may be used {many}, a list of what is seen {many_seen}");
    assert!(
        many <= 12,
        "a list of what may be used takes {many} statements"
    );
    assert!(
        many_seen <= 12,
        "a list of what is seen takes {many_seen} statements"
    );
}
