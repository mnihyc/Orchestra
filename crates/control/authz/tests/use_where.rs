//! An entry in the Use row of a provider may say where the work done through it may run: a turn, a start and a provider asked about alone are allowed in the environments and the hosts it names, refused with the place named elsewhere, and work that runs in no environment is held to no place.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::decisions::Refusal;
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::Permission as P;
use orch_access_contract::roles::Role;
use orch_access_contract::whereabouts::{WhereKind, WhereName, Whereabouts};
use orch_authz::Caller;
use orch_authz::environments::{NewEnvironment, Place, Wants};
use orch_authz::usable::{
    self, Author, BindingAsk, BindingDecision, EnvironmentChoice, Policies, ProviderChoice,
    TurnAsk, TurnDecision, Usable,
};
use orch_db::queries::access::{PageKey, testing::add_entry};
use orch_db::queries::resources::access_fixtures;
use orch_types::Id;
use serde_json::{Value, json};
use support::World;

/// Acme with a topic, a provider of its owner `ada` with a model, two environments of the topic that
/// the work of the project may use, a host with its machine, and `bea`, who drives the thread and uses the
/// provider through entries of her own.
struct Setting {
    w: World,
    ada: Id,
    bea: Id,
    provider: Id,
    first: Id,
    second: Id,
    host: Id,
    machine: Id,
    thread: Id,
}

async fn setting() -> Setting {
    let w = World::new().await;
    let (ada, bea) = (w.member("ada").await, w.member("bea").await);
    for person in [ada, bea] {
        w.give(w.project, person, Role::Developer).await;
    }
    let provider = w.provider(ada, w.root, "Team Pro").await;
    w.model(provider, "big", &["low", "high"]).await;
    let host = w.thing("host", ada, w.root, "Rack").await;
    let machine: Id = {
        let mut conn = w.conn().await;
        sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id = $1")
            .bind(host)
            .fetch_one(&mut *conn)
            .await
            .unwrap()
    };
    let mut environments = Vec::new();
    for name in ["first", "second"] {
        let environment = connected(&w, ada, name).await;
        w.grant(
            "environment",
            environment,
            P::EnvironmentUse,
            Holder::Node { id: w.project },
        )
        .await;
        environments.push(environment);
    }
    let thread = w.thread_in(w.topic, environments[0], "Work").await;
    w.give(w.project, w.maker, Role::Interactor).await;
    Setting {
        w,
        ada,
        bea,
        provider,
        first: environments[0],
        second: environments[1],
        host,
        machine,
        thread,
    }
}

/// A directory of a machine of `owner`, in the topic, that its owner holds the page of.
async fn connected(w: &World, owner: Id, name: &str) -> Id {
    let mut conn = w.conn().await;
    let id =
        access_fixtures::insert_environment(&mut conn, owner, Some(w.topic), name, "connected")
            .await
            .unwrap();
    add_entry(
        &mut conn,
        &PageKey::new("environment", id, "live"),
        "any",
        "access.own",
        ("user", Some(owner), None),
        None,
        w.now.get(),
    )
    .await
    .unwrap();
    id
}

fn name(kind: WhereKind, id: Id) -> Value {
    json!({ "kind": kind.as_str(), "id": id })
}

impl Setting {
    /// Bea uses the provider through one more entry of hers, with these options.
    async fn entry(&self, options: Value) {
        self.w
            .put(
                "provider",
                self.provider,
                P::ProviderUse,
                Holder::User { id: self.bea },
                options,
            )
            .await;
    }

    /// Puts the environment on the machine of the host.
    async fn on_the_host(&self, environment: Id) {
        let mut conn = self.w.conn().await;
        sqlx::query("UPDATE environments SET machine_id = $2 WHERE id = $1")
            .bind(environment)
            .bind(self.machine)
            .execute(&mut *conn)
            .await
            .unwrap();
    }

    async fn place(&self, environment: Id) -> Whereabouts {
        let mut conn = self.w.conn().await;
        usable::place_of(&mut conn, environment).await.unwrap()
    }

    /// A turn of Bea on the thread, run in `at` (none for work that runs in no environment).
    async fn turn(&self, who: Id, at: Option<&Whereabouts>) -> TurnDecision {
        let principal = self.w.principal(Caller::session(who)).await;
        let mut conn = self.w.conn().await;
        usable::check_turn_at(
            &mut conn,
            Author::Person(&principal),
            &TurnAsk {
                thread: self.thread,
                provider: self.provider,
                profile: None,
                model: "big",
                effort: None,
                arrangement: None,
                models_allowed: None,
            },
            &Policies::default(),
            at,
        )
        .await
        .unwrap()
    }

    /// Bea starts a thread in the topic, in the environment the ask names.
    async fn start(&self, environment: Option<EnvironmentChoice>) -> BindingDecision {
        let principal = self.w.principal(Caller::session(self.bea)).await;
        let mut conn = self.w.conn().await;
        usable::check_binding_at(
            &mut conn,
            &principal,
            &BindingAsk {
                node: self.w.topic,
                permission: P::TopicStartThreads,
                provider: Some(ProviderChoice {
                    id: self.provider,
                    model: "big",
                    effort: None,
                    arrangement: None,
                }),
                profile: None,
                environment,
                models_allowed: None,
            },
            &Policies::default(),
            None,
        )
        .await
        .unwrap()
    }
}

fn refusal(decision: TurnDecision) -> Refusal {
    match decision {
        TurnDecision::Refused(refusal) => *refusal,
        other => panic!("a refusal was expected, not {other:?}"),
    }
}

fn named_places(refusal: &Refusal) -> Vec<WhereName> {
    refusal
        .missing
        .iter()
        .filter_map(|lack| lack.options.as_ref())
        .filter_map(|options| options.r#where.clone())
        .flatten()
        .collect()
}

#[tokio::test]
async fn an_entry_that_leaves_where_out_lets_the_turn_run_in_any_environment() {
    let s = setting().await;
    s.entry(json!({ "models": ["big"] })).await;
    for environment in [s.first, s.second] {
        let at = s.place(environment).await;
        assert!(s.turn(s.bea, Some(&at)).await.is_allowed());
    }
}

#[tokio::test]
async fn an_environment_that_is_named_is_allowed_and_another_is_refused_with_the_place_named() {
    let s = setting().await;
    s.entry(json!({ "where": [name(WhereKind::Environment, s.first)] }))
        .await;
    let first = s.place(s.first).await;
    let second = s.place(s.second).await;
    assert!(s.turn(s.bea, Some(&first)).await.is_allowed());
    let refused = refusal(s.turn(s.bea, Some(&second)).await);
    assert_eq!(
        named_places(&refused),
        [WhereName {
            kind: WhereKind::Environment,
            id: s.second
        }],
        "the refusal names the environment the work was to run in"
    );
    let lack = &refused.missing[0];
    assert_eq!(lack.permissions, [P::ProviderUse]);
    assert_eq!(lack.object.id, s.provider);
    assert_eq!(
        lack.object.owner,
        Some(s.ada),
        "the provider's owner is told, as for any missing use"
    );
}

#[tokio::test]
async fn a_host_that_is_named_covers_the_environments_on_it_and_those_made_on_it_afterwards() {
    let s = setting().await;
    s.entry(json!({ "where": [name(WhereKind::Host, s.host)] }))
        .await;
    let off = s.place(s.first).await;
    assert!(!s.turn(s.bea, Some(&off)).await.is_allowed());
    s.on_the_host(s.first).await;
    let on = s.place(s.first).await;
    assert_eq!(on.host, Some(s.host));
    assert!(s.turn(s.bea, Some(&on)).await.is_allowed());
    // An environment made later on the same host is covered without the entry being touched.
    let later = connected(&s.w, s.ada, "later").await;
    s.on_the_host(later).await;
    s.w.grant(
        "environment",
        later,
        P::EnvironmentUse,
        Holder::Node { id: s.w.project },
    )
    .await;
    assert!(
        s.turn(s.bea, Some(&s.place(later).await))
            .await
            .is_allowed()
    );
    // A thread started in an environment still to be made on the host is covered by the host.
    s.w.grant("host", s.host, P::HostUse, Holder::User { id: s.bea })
        .await;
    let template = s.w.thing("template", s.bea, s.w.root, "recipe").await;
    let made = |host| {
        Some(EnvironmentChoice::New(NewEnvironment {
            home: s.w.topic,
            place: Place::Host {
                host,
                template,
                wants: Wants::default(),
            },
            mounts: Vec::new(),
        }))
    };
    let decision = s.start(made(s.host)).await;
    assert!(
        matches!(decision, BindingDecision::Allowed(_)),
        "the host is named: {decision:?}"
    );
    // The same entry, naming an environment instead, does not cover one that is not made yet.
    let elsewhere = s.w.thing("host", s.ada, s.w.root, "Other").await;
    s.w.grant("host", elsewhere, P::HostUse, Holder::User { id: s.bea })
        .await;
    match s.start(made(elsewhere)).await {
        BindingDecision::Refused(refusal) => assert_eq!(named_places(&refusal).len(), 1),
        other => panic!("a refusal was expected, not {other:?}"),
    }
}

#[tokio::test]
async fn a_machine_that_is_named_covers_the_environments_on_it() {
    let s = setting().await;
    s.entry(json!({ "where": [name(WhereKind::Machine, s.machine)] }))
        .await;
    let off = s.place(s.second).await;
    assert!(!s.turn(s.bea, Some(&off)).await.is_allowed());
    s.on_the_host(s.second).await;
    assert!(
        s.turn(s.bea, Some(&s.place(s.second).await))
            .await
            .is_allowed()
    );
}

#[tokio::test]
async fn several_entries_allow_what_either_allows_and_an_entry_without_where_allows_everywhere() {
    let s = setting().await;
    s.entry(json!({ "where": [name(WhereKind::Environment, s.first)] }))
        .await;
    let second = s.place(s.second).await;
    assert!(!s.turn(s.bea, Some(&second)).await.is_allowed());
    s.entry(json!({ "where": [name(WhereKind::Environment, s.second)] }))
        .await;
    assert!(s.turn(s.bea, Some(&second)).await.is_allowed());
    assert!(
        s.turn(s.bea, Some(&s.place(s.first).await))
            .await
            .is_allowed()
    );
    // One entry that names no place lets the work run anywhere, whatever the others name.
    s.entry(json!({ "models": ["big"] })).await;
    s.on_the_host(s.second).await;
    assert!(
        s.turn(s.bea, Some(&s.place(s.second).await))
            .await
            .is_allowed()
    );
}

#[tokio::test]
async fn an_owner_and_work_that_runs_in_no_environment_are_held_to_no_place() {
    let s = setting().await;
    s.entry(json!({ "where": [name(WhereKind::Environment, s.first)] }))
        .await;
    let second = s.place(s.second).await;
    assert!(
        s.turn(s.bea, None).await.is_allowed(),
        "a turn that runs in no environment is not narrowed by where"
    );
    assert!(!s.turn(s.bea, Some(&second)).await.is_allowed());
    // The provider's owner uses it through no entry, and so through none that narrows.
    assert!(s.turn(s.ada, Some(&second)).await.is_allowed());
}

#[tokio::test]
async fn a_thread_that_moved_to_another_environment_is_refused_at_its_next_turn() {
    let s = setting().await;
    s.entry(json!({ "where": [name(WhereKind::Environment, s.first)] }))
        .await;
    let first = s.place(s.first).await;
    assert!(s.turn(s.bea, Some(&first)).await.is_allowed());
    // The thread moves to the other environment: the next turn is asked there.
    s.w.put_in(s.thread, s.second).await;
    let second = s.place(s.second).await;
    assert!(!s.turn(s.bea, Some(&second)).await.is_allowed());
}

#[tokio::test]
async fn starting_a_thread_is_allowed_in_a_named_environment_and_refused_in_another() {
    let s = setting().await;
    s.entry(json!({ "where": [name(WhereKind::Environment, s.first)] }))
        .await;
    let first = s.start(Some(EnvironmentChoice::Existing(s.first))).await;
    assert!(
        matches!(first, BindingDecision::Allowed(_)),
        "bea starts where the entry names: {first:?}"
    );
    match s.start(Some(EnvironmentChoice::Existing(s.second))).await {
        BindingDecision::Refused(refusal) => assert_eq!(
            named_places(&refusal),
            [WhereName {
                kind: WhereKind::Environment,
                id: s.second
            }]
        ),
        other => panic!("a refusal was expected, not {other:?}"),
    }
}

#[tokio::test]
async fn a_provider_asked_about_alone_is_refused_for_the_place_with_who_can_give_it() {
    let s = setting().await;
    s.entry(json!({ "where": [name(WhereKind::Environment, s.first)] }))
        .await;
    let principal = s.w.principal(Caller::session(s.bea)).await;
    let mut conn = s.w.conn().await;
    let Usable::Yes(granted) = usable::usable(
        &mut conn,
        &principal,
        s.provider,
        Some(s.thread),
        &Policies::default(),
    )
    .await
    .unwrap() else {
        panic!("bea uses the provider through her entry");
    };
    let outside = s.place(s.second).await;
    let refused = usable::refuse_outside_place(
        &mut conn,
        &principal,
        s.provider,
        s.thread,
        &granted,
        &outside,
        &Policies::default(),
    )
    .await
    .unwrap()
    .expect("the entry leaves the second environment out");
    assert_eq!(refused.required, P::ProviderUse);
    assert_eq!(
        named_places(&refused),
        [WhereName {
            kind: WhereKind::Environment,
            id: s.second
        }]
    );
    let inside = s.place(s.first).await;
    assert!(
        usable::refuse_outside_place(
            &mut conn,
            &principal,
            s.provider,
            s.thread,
            &granted,
            &inside,
            &Policies::default(),
        )
        .await
        .unwrap()
        .is_none()
    );
}
