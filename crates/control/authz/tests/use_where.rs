//! An entry in the Use row of a provider is one grant: a turn, a start and a provider asked about alone are allowed when a single entry of the person allows the model, the effort, the arrangement type and the place together; the places it names are environments, hosts and machines, and work that runs in no environment is held to no place.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::decisions::Refusal;
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{Arrangement, Permission as P};
use orch_access_contract::roles::Role;
use orch_access_contract::whereabouts::{WhereKind, WhereName, Whereabouts};
use orch_authz::Caller;
use orch_authz::environments::{NewEnvironment, Place, Wants};
use orch_authz::usable::{
    self, Alone, Author, BindingAsk, BindingDecision, Blocker, EnvironmentChoice, Policies,
    ProviderChoice, TurnAsk, TurnDecision, Usable,
};
use orch_db::queries::access::{PageKey, testing::add_entry};
use orch_db::queries::resources::access_fixtures;
use orch_types::Id;
use serde_json::{Value, json};
use support::World;

/// Acme with a topic, a provider of its owner `ada` with two models, two environments of the topic
/// that `ada` and `bea` use, a host with its machine, and `bea`, who drives the thread and uses the
/// provider through entries of hers: her own, and the one everyone has.
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
    // What a real participant holds: they take part in the threads of the project, and start work in
    // the topic.
    for person in [ada, bea] {
        w.give(w.project, person, Role::Interactor).await;
        w.give(w.topic, person, Role::Developer).await;
    }
    let provider = w.provider(ada, w.root, "Team Pro").await;
    w.model(provider, "big", &["low", "high"]).await;
    w.model(provider, "small", &["low", "high"]).await;
    {
        let mut conn = w.conn().await;
        sqlx::query("UPDATE providers SET arrangements = ARRAY['separated','key_separated','credential_shared','handed_over'] WHERE id = $1")
            .bind(provider)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
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
        environments.push(connected(&w, ada, bea, name).await);
    }
    // The thread lives in a managed environment, where the people of the project take part in it as
    // shipped; the two directories of a machine are the places the turns are asked about, since a
    // thread in a person's own machine is theirs alone by default.
    let home = w.thing("environment", ada, w.topic, "home").await;
    w.thing_row("environment", home, P::EnvironmentUse, false)
        .await;
    for person in [ada, bea] {
        w.grant(
            "environment",
            home,
            P::EnvironmentUse,
            Holder::User { id: person },
        )
        .await;
    }
    let thread = w.thread_in(w.topic, home, "Work").await;
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

/// A directory of a machine of `owner`, in the topic, that its owner holds the page of and `user`
/// may use.
async fn connected(w: &World, owner: Id, user: Id, name: &str) -> Id {
    let id = {
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
    };
    w.thing_row("environment", id, P::EnvironmentUse, false)
        .await;
    w.grant(
        "environment",
        id,
        P::EnvironmentUse,
        Holder::User { id: user },
    )
    .await;
    id
}

fn name(kind: WhereKind, id: Id) -> Value {
    json!({ "kind": kind.as_str(), "id": id })
}

fn assert_allowed(decision: TurnDecision, what: &str) {
    assert!(decision.is_allowed(), "{what}: {decision:?}");
}

fn assert_refused(decision: TurnDecision, what: &str) {
    assert!(
        matches!(decision, TurnDecision::Refused(_)),
        "{what}: {decision:?}"
    );
}

impl Setting {
    /// Bea uses the provider through an entry of her own, with these options.
    async fn entry(&self, options: Value) {
        self.entry_of(Holder::User { id: self.bea }, options).await;
    }

    /// A person is let in by several entries only through different holders: one active entry a
    /// holder has in a row.
    async fn entry_of(&self, holder: Holder, options: Value) {
        self.w
            .put("provider", self.provider, P::ProviderUse, holder, options)
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

    /// A turn of `who` on the thread with `model`, run in `at` (none for work that runs in no
    /// environment).
    async fn turn_with(
        &self,
        who: Id,
        model: &str,
        effort: Option<&str>,
        at: Option<&Whereabouts>,
    ) -> TurnDecision {
        let principal = self.w.principal(Caller::session(who)).await;
        let mut conn = self.w.conn().await;
        usable::check_turn_at(
            &mut conn,
            Author::Person(&principal),
            &TurnAsk {
                thread: self.thread,
                provider: self.provider,
                profile: None,
                model,
                effort,
                arrangement: None,
                models_allowed: None,
            },
            &Policies::default(),
            at,
        )
        .await
        .unwrap()
    }

    async fn turn(&self, who: Id, at: Option<&Whereabouts>) -> TurnDecision {
        self.turn_with(who, "big", None, at).await
    }

    /// Bea starts a thread in the topic, in the environment the ask names, with a model and a type.
    async fn start_with(
        &self,
        environment: Option<EnvironmentChoice>,
        model: &str,
        arrangement: Option<Arrangement>,
    ) -> BindingDecision {
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
                    model,
                    effort: None,
                    arrangement,
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

    async fn start(&self, environment: Option<EnvironmentChoice>) -> BindingDecision {
        self.start_with(environment, "big", None).await
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

fn offered(decision: &TurnDecision) -> Vec<Arrangement> {
    match decision {
        TurnDecision::Allowed(grant) => grant.arrangements.clone(),
        other => panic!("an allowance was expected, not {other:?}"),
    }
}

#[tokio::test]
async fn an_entry_that_leaves_where_out_lets_the_turn_run_in_any_environment() {
    let s = setting().await;
    s.entry(json!({ "models": ["big"] })).await;
    for environment in [s.first, s.second] {
        let at = s.place(environment).await;
        assert_allowed(s.turn(s.bea, Some(&at)).await, "anywhere");
    }
}

#[tokio::test]
async fn an_environment_that_is_named_is_allowed_and_another_is_refused_with_the_place_named() {
    let s = setting().await;
    s.entry(json!({ "where": [name(WhereKind::Environment, s.first)] }))
        .await;
    let first = s.place(s.first).await;
    let second = s.place(s.second).await;
    assert_allowed(s.turn(s.bea, Some(&first)).await, "the named one");
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
    assert_refused(s.turn(s.bea, Some(&off)).await, "off the host");
    s.on_the_host(s.first).await;
    let on = s.place(s.first).await;
    assert_eq!(on.host, Some(s.host));
    assert_allowed(s.turn(s.bea, Some(&on)).await, "on the host");
    // An environment made later on the same host is covered without the entry being touched.
    let later = connected(&s.w, s.ada, s.bea, "later").await;
    s.on_the_host(later).await;
    assert_allowed(
        s.turn(s.bea, Some(&s.place(later).await)).await,
        "made later on the host",
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
    // The same entry does not cover an environment to be made on another host.
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
    assert_refused(s.turn(s.bea, Some(&off)).await, "off the machine");
    s.on_the_host(s.second).await;
    assert_allowed(
        s.turn(s.bea, Some(&s.place(s.second).await)).await,
        "on the machine",
    );
}

#[tokio::test]
async fn entries_that_each_name_a_place_allow_either_place_and_one_that_names_none_allows_everywhere()
 {
    let s = setting().await;
    s.entry(json!({ "where": [name(WhereKind::Environment, s.first)] }))
        .await;
    let second = s.place(s.second).await;
    assert_refused(s.turn(s.bea, Some(&second)).await, "only the first named");
    s.entry_of(
        Holder::Everyone,
        json!({ "where": [name(WhereKind::Environment, s.second)] }),
    )
    .await;
    assert_allowed(
        s.turn(s.bea, Some(&second)).await,
        "the second is named too",
    );
    assert_allowed(
        s.turn(s.bea, Some(&s.place(s.first).await)).await,
        "the first still is",
    );
    // One entry that names no place lets the work run anywhere.
    s.entry_of(
        Holder::Node { id: s.w.project },
        json!({ "models": ["big"] }),
    )
    .await;
    s.on_the_host(s.second).await;
    assert_allowed(
        s.turn(s.bea, Some(&s.place(s.second).await)).await,
        "an entry that names no place",
    );
}

#[tokio::test]
async fn an_owner_and_work_that_runs_in_no_environment_are_held_to_no_place() {
    let s = setting().await;
    s.entry(json!({ "where": [name(WhereKind::Environment, s.first)] }))
        .await;
    let second = s.place(s.second).await;
    assert_allowed(
        s.turn(s.bea, None).await,
        "a turn that runs in no environment is not narrowed by where",
    );
    assert_refused(s.turn(s.bea, Some(&second)).await, "in the other one");
    // The provider's owner uses it through no entry, and so through none that narrows.
    assert_allowed(s.turn(s.ada, Some(&second)).await, "the owner");
}

#[tokio::test]
async fn a_thread_that_moved_to_another_environment_is_refused_at_its_next_turn() {
    let s = setting().await;
    s.entry(json!({ "where": [name(WhereKind::Environment, s.first)] }))
        .await;
    let first = s.place(s.first).await;
    assert_allowed(s.turn(s.bea, Some(&first)).await, "where it started");
    // The thread moves to the other environment: the next turn is asked where it runs now.
    let second = s.place(s.second).await;
    assert_refused(s.turn(s.bea, Some(&second)).await, "after the move");
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
    let alone = usable::judge_alone(
        &mut conn,
        &principal,
        s.provider,
        s.thread,
        &granted,
        Some(&outside),
        &Policies::default(),
    )
    .await
    .unwrap();
    let Alone::Refused(refused) = alone else {
        panic!("the entry leaves the second environment out: {alone:?}");
    };
    assert_eq!(refused.required, P::ProviderUse);
    assert_eq!(
        named_places(&refused),
        [WhereName {
            kind: WhereKind::Environment,
            id: s.second
        }]
    );
    let inside = s.place(s.first).await;
    let alone = usable::judge_alone(
        &mut conn,
        &principal,
        s.provider,
        s.thread,
        &granted,
        Some(&inside),
        &Policies::default(),
    )
    .await
    .unwrap();
    assert!(matches!(alone, Alone::Allowed { .. }), "{alone:?}");
}

#[tokio::test]
async fn the_place_of_one_entry_does_not_join_the_models_of_another() {
    let s = setting().await;
    // Her own entry allows `small` anywhere, the one everyone has allows `big` in the first
    // environment alone.
    s.entry(json!({ "models": ["small"] })).await;
    s.entry_of(
        Holder::Everyone,
        json!({ "models": ["big"], "where": [name(WhereKind::Environment, s.first)] }),
    )
    .await;
    let (first, second) = (s.place(s.first).await, s.place(s.second).await);
    assert_allowed(
        s.turn_with(s.bea, "small", None, Some(&second)).await,
        "small anywhere",
    );
    assert_allowed(
        s.turn_with(s.bea, "big", None, Some(&first)).await,
        "big in the first",
    );
    let refused = refusal(s.turn_with(s.bea, "big", None, Some(&second)).await);
    assert_eq!(
        named_places(&refused),
        [WhereName {
            kind: WhereKind::Environment,
            id: s.second
        }],
        "the place is what stands in the way of big"
    );
}

#[tokio::test]
async fn the_copy_of_one_entry_is_not_made_elsewhere_because_another_entry_names_no_place() {
    let s = setting().await;
    // The entry she has names nothing: the types in which the credential stays, anywhere. The one
    // everyone has names the copy, in the first environment alone.
    s.entry(json!({})).await;
    s.entry_of(
        Holder::Everyone,
        json!({
            "arrangements": ["credential_shared"],
            "where": [name(WhereKind::Environment, s.first)]
        }),
    )
    .await;
    let (first, second) = (s.place(s.first).await, s.place(s.second).await);
    // Offered for a turn: the copy only in the first environment.
    assert_eq!(
        offered(&s.turn(s.bea, Some(&first)).await),
        [
            Arrangement::Separated,
            Arrangement::KeySeparated,
            Arrangement::CredentialShared
        ]
    );
    assert_eq!(
        offered(&s.turn(s.bea, Some(&second)).await),
        [Arrangement::Separated, Arrangement::KeySeparated]
    );
    // And chosen for a start.
    let copy = |environment| {
        s.start_with(
            Some(EnvironmentChoice::Existing(environment)),
            "big",
            Some(Arrangement::CredentialShared),
        )
    };
    let in_first = copy(s.first).await;
    assert!(
        matches!(in_first, BindingDecision::Allowed(_)),
        "the copy where it is named: {in_first:?}"
    );
    assert_eq!(
        copy(s.second).await,
        BindingDecision::Blocked(vec![Blocker::ArrangementNotAllowed {
            arrangement: Arrangement::CredentialShared
        }]),
        "no copy elsewhere"
    );
}

#[tokio::test]
async fn the_model_the_effort_and_the_type_have_to_come_from_one_entry() {
    let s = setting().await;
    s.entry(json!({ "models": ["small"], "efforts": ["low"] }))
        .await;
    s.entry_of(
        Holder::Everyone,
        json!({ "models": ["big"], "efforts": ["high"] }),
    )
    .await;
    assert_allowed(
        s.turn_with(s.bea, "small", Some("low"), None).await,
        "the first entry's",
    );
    assert_allowed(
        s.turn_with(s.bea, "big", Some("high"), None).await,
        "the second entry's",
    );
    assert_refused(
        s.turn_with(s.bea, "big", Some("low"), None).await,
        "the model of the second with the effort of the first",
    );
    assert_refused(
        s.turn_with(s.bea, "small", Some("high"), None).await,
        "the model of the first with the effort of the second",
    );
}
