//! Putting a thread on a provider, a profile and an environment, by creating, forking or switching it, is checked against the person who does it, for each thing they choose, and the refusal names everything that is lacking.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::decisions::Refusal;
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{Arrangement, ObjectKind, Permission as P, ResourceLimits};
use orch_access_contract::roles::Role;
use orch_authz::Caller;
use orch_authz::environments::{Limit, NewEnvironment, Place, Wants};
use orch_authz::usable::{
    self, BindingAsk, BindingDecision, Blocker, EnvironmentChoice, Policies, ProviderChoice,
};
use orch_db::queries::resources::access_fixtures;
use orch_types::Id;
use serde_json::json;
use support::World;

/// Acme with a topic whose Developers start work there as shipped; a provider of its owner `ada`
/// with two models, a profile, an environment managed by its owner alone; and `dev`, a Developer
/// who holds none of them.
struct Setting {
    w: World,
    ada: Id,
    dev: Id,
    provider: Id,
    profile: Id,
    environment: Id,
}

async fn setting() -> Setting {
    let w = World::new().await;
    let (ada, dev) = (w.member("ada").await, w.member("dev").await);
    w.give(w.project, dev, Role::Developer).await;
    let provider = w.provider(ada, w.root, "Team Pro").await;
    w.model(provider, "big", &["low", "high"]).await;
    w.model(provider, "small", &["low"]).await;
    let profile = w.thing("agent", ada, w.root, "Coder").await;
    let environment = w.thing("environment", ada, w.topic, "private").await;
    w.thing_row("environment", environment, P::EnvironmentManage, false)
        .await;
    w.thing_row("environment", environment, P::EnvironmentUse, false)
        .await;
    Setting {
        w,
        ada,
        dev,
        provider,
        profile,
        environment,
    }
}

fn creating<'a>(
    s: &Setting,
    model: &'a str,
    environment: Option<EnvironmentChoice>,
) -> BindingAsk<'a> {
    BindingAsk {
        node: s.w.topic,
        permission: P::TopicStartThreads,
        provider: Some(ProviderChoice {
            id: s.provider,
            model,
            effort: Some("low"),
            arrangement: None,
        }),
        profile: Some(s.profile),
        environment,
        models_allowed: None,
    }
}

async fn bind(w: &World, who: Id, ask: &BindingAsk<'_>) -> BindingDecision {
    let principal = w.principal(Caller::session(who)).await;
    let mut conn = w.conn().await;
    usable::check_binding(&mut conn, &principal, ask, &Policies::default())
        .await
        .unwrap()
}

fn refusal(decision: BindingDecision) -> Refusal {
    match decision {
        BindingDecision::Refused(refusal) => *refusal,
        other => panic!("a refusal was expected, not {other:?}"),
    }
}

fn lacks(refusal: &Refusal) -> Vec<(ObjectKind, Vec<P>)> {
    let mut found: Vec<_> = refusal
        .missing
        .iter()
        .map(|entry| (entry.object.kind, entry.permissions.clone()))
        .collect();
    found.sort_by_key(|(kind, _)| *kind);
    found
}

#[tokio::test]
async fn creating_a_thread_takes_the_use_of_a_provider_a_profile_and_an_environment() {
    let s = setting().await;
    let existing = Some(EnvironmentChoice::Existing(s.environment));
    let refused = refusal(bind(&s.w, s.dev, &creating(&s, "big", existing.clone())).await);
    assert_eq!(refused.required, P::TopicStartThreads);
    assert_eq!(
        lacks(&refused),
        [
            (ObjectKind::Provider, vec![P::ProviderUse]),
            (ObjectKind::Agent, vec![P::AgentUse]),
            (ObjectKind::Environment, vec![P::EnvironmentUse]),
        ],
        "all three, not only the first; the topic is held"
    );
    let provider = refused
        .missing
        .iter()
        .find(|entry| entry.object.kind == ObjectKind::Provider)
        .unwrap();
    assert_eq!(
        provider.options.as_ref().unwrap().models.as_deref(),
        Some(&["big".to_owned()][..])
    );
    assert!(provider.ask.contains(&s.ada), "the owner can give it");
    // A Viewer lacks the topic as well.
    let vee = s.w.member("vee").await;
    s.w.give(s.w.project, vee, Role::Viewer).await;
    let refused = refusal(bind(&s.w, vee, &creating(&s, "big", existing.clone())).await);
    assert_eq!(refused.missing.len(), 4);
    assert_eq!(
        refused.missing[0].object.kind,
        ObjectKind::Topic,
        "the act itself first"
    );
    // Given all of it, the choice goes through, with what the entry comes to.
    s.w.put(
        "provider",
        s.provider,
        P::ProviderUse,
        Holder::User { id: s.dev },
        json!({ "models": ["big"] }),
    )
    .await;
    s.w.grant("agent", s.profile, P::AgentUse, Holder::User { id: s.dev })
        .await;
    s.w.grant(
        "environment",
        s.environment,
        P::EnvironmentUse,
        Holder::User { id: s.dev },
    )
    .await;
    assert!(
        !s.w.can_on(s.dev, "environment", s.environment, P::EnvironmentManage)
            .await
    );
    let BindingDecision::Allowed(grant) =
        bind(&s.w, s.dev, &creating(&s, "big", existing.clone())).await
    else {
        panic!("everything is held");
    };
    let provider = grant.provider.unwrap();
    assert_eq!(provider.models, ["big"]);
    assert!(provider.entry.is_some());
    // The entry is limited to one model: the other is asked for.
    let refused = refusal(bind(&s.w, s.dev, &creating(&s, "small", existing)).await);
    assert_eq!(
        lacks(&refused),
        [(ObjectKind::Provider, vec![P::ProviderUse])]
    );
}

#[tokio::test]
async fn switching_a_thread_takes_managing_it_and_the_callers_own_use_of_the_new_provider_whatever_they_hold_on_its_environment()
 {
    let s = setting().await;
    let other = s.w.provider(s.ada, s.w.root, "Other").await;
    s.w.model(other, "big", &["low"]).await;
    // The thread's owner manages it; the new provider is another's.
    let switch = |provider: Id| BindingAsk {
        node: s.w.thread,
        permission: P::ThreadManage,
        provider: Some(ProviderChoice {
            id: provider,
            model: "big",
            effort: Some("low"),
            arrangement: None,
        }),
        profile: None,
        environment: None,
        models_allowed: None,
    };
    let refused = refusal(bind(&s.w, s.w.maker, &switch(other)).await);
    assert_eq!(refused.required, P::ThreadManage);
    assert_eq!(
        lacks(&refused),
        [(ObjectKind::Provider, vec![P::ProviderUse])],
        "told what is missing"
    );
    // Once they may use it, they may switch, with nothing asked of the environment.
    s.w.grant(
        "provider",
        other,
        P::ProviderUse,
        Holder::User { id: s.w.maker },
    )
    .await;
    assert!(matches!(
        bind(&s.w, s.w.maker, &switch(other)).await,
        BindingDecision::Allowed(_)
    ));
    // Somebody who may use it and does not manage the thread is refused the thread.
    let user = s.w.member("user").await;
    s.w.give(s.w.project, user, Role::Interactor).await;
    s.w.grant("provider", other, P::ProviderUse, Holder::User { id: user })
        .await;
    let refused = refusal(bind(&s.w, user, &switch(other)).await);
    assert_eq!(
        lacks(&refused),
        [(ObjectKind::Thread, vec![P::ThreadManage])]
    );
}

#[tokio::test]
async fn what_cannot_run_for_anybody_blocks_the_choice_instead_of_refusing_the_caller() {
    let s = setting().await;
    s.w.put(
        "provider",
        s.provider,
        P::ProviderUse,
        Holder::User { id: s.dev },
        json!({ "arrangements": ["key_separated"] }),
    )
    .await;
    s.w.grant("agent", s.profile, P::AgentUse, Holder::User { id: s.dev })
        .await;
    let choice = |model: &'static str,
                  effort: Option<&'static str>,
                  arrangement: Option<Arrangement>| BindingAsk {
        node: s.w.topic,
        permission: P::TopicStartThreads,
        provider: Some(ProviderChoice {
            id: s.provider,
            model,
            effort,
            arrangement,
        }),
        profile: Some(s.profile),
        environment: None,
        models_allowed: None,
    };
    assert!(matches!(
        bind(
            &s.w,
            s.dev,
            &choice("big", Some("high"), Some(Arrangement::KeySeparated))
        )
        .await,
        BindingDecision::Allowed(_)
    ));
    // A model the provider does not have, an effort the model does not list, an arrangement type the
    // entry does not name.
    assert_eq!(
        bind(&s.w, s.dev, &choice("huge", None, None)).await,
        BindingDecision::Blocked(vec![Blocker::ModelNotAllowed {
            model: "huge".into()
        }])
    );
    assert_eq!(
        bind(&s.w, s.dev, &choice("small", Some("high"), None)).await,
        BindingDecision::Blocked(vec![Blocker::EffortNotAllowed {
            effort: "high".into()
        }])
    );
    assert_eq!(
        bind(
            &s.w,
            s.dev,
            &choice("big", None, Some(Arrangement::Separated))
        )
        .await,
        BindingDecision::Blocked(vec![Blocker::ArrangementNotAllowed {
            arrangement: Arrangement::Separated
        }])
    );
    // The profile and the provider switched off.
    {
        let mut conn = s.w.conn().await;
        access_fixtures::set_disabled(&mut conn, "agent", s.profile, true)
            .await
            .unwrap();
        access_fixtures::set_disabled(&mut conn, "provider", s.provider, true)
            .await
            .unwrap();
    }
    let BindingDecision::Blocked(blockers) = bind(&s.w, s.dev, &choice("big", None, None)).await
    else {
        panic!("switched off");
    };
    assert!(
        blockers.contains(&Blocker::ProviderDisabled)
            && blockers.contains(&Blocker::ProfileDisabled)
    );
    // The setting of the place that allows models narrows it for everybody too.
    let mut ask = choice("big", None, None);
    {
        let mut conn = s.w.conn().await;
        access_fixtures::set_disabled(&mut conn, "agent", s.profile, false)
            .await
            .unwrap();
        access_fixtures::set_disabled(&mut conn, "provider", s.provider, false)
            .await
            .unwrap();
    }
    let allowed = ["small".to_owned()];
    ask.models_allowed = Some(&allowed);
    assert_eq!(
        bind(&s.w, s.dev, &ask).await,
        BindingDecision::Blocked(vec![Blocker::ModelNotAllowed {
            model: "big".into()
        }])
    );
}

#[tokio::test]
async fn a_new_environment_adds_what_creating_one_takes_to_the_same_refusal_and_is_held_to_the_entries()
 {
    let s = setting().await;
    let tim = s.w.member("tim").await;
    let host = s.w.thing("host", s.ada, s.w.root, "gpu").await;
    let template = s.w.thing("template", tim, s.w.root, "recipe").await;
    let new = |memory: i64| {
        Some(EnvironmentChoice::New(NewEnvironment {
            home: s.w.topic,
            place: Place::Host {
                host,
                template,
                wants: Wants {
                    resources: ResourceLimits {
                        memory_mb: Some(memory),
                        ..ResourceLimits::default()
                    },
                    ..Wants::default()
                },
            },
            mounts: vec![],
        }))
    };
    // Nothing is held: the provider, the profile, the host and the template are all asked for at once.
    let refused = refusal(bind(&s.w, s.dev, &creating(&s, "big", new(512))).await);
    let kinds: Vec<ObjectKind> = {
        let mut kinds: Vec<ObjectKind> = refused
            .missing
            .iter()
            .map(|entry| entry.object.kind)
            .collect();
        kinds.sort();
        kinds
    };
    assert_eq!(
        kinds,
        [
            ObjectKind::Provider,
            ObjectKind::Agent,
            ObjectKind::Host,
            ObjectKind::Template
        ]
    );
    s.w.grant(
        "provider",
        s.provider,
        P::ProviderUse,
        Holder::User { id: s.dev },
    )
    .await;
    s.w.grant("agent", s.profile, P::AgentUse, Holder::User { id: s.dev })
        .await;
    s.w.put(
        "host",
        host,
        P::HostUse,
        Holder::User { id: s.dev },
        json!({ "resources_max": { "memory_mb": 1024 } }),
    )
    .await;
    s.w.grant(
        "template",
        template,
        P::TemplateUse,
        Holder::User { id: s.dev },
    )
    .await;
    // Within the entry, the new environment is the caller's and what it was let through by is told.
    let BindingDecision::Allowed(grant) = bind(&s.w, s.dev, &creating(&s, "big", new(512))).await
    else {
        panic!("within the entries");
    };
    assert_eq!(grant.created.unwrap().ceiling.memory_mb, Some(1024));
    // Beyond the entry, the caller holds what is needed and the entry is what stops it.
    assert_eq!(
        bind(&s.w, s.dev, &creating(&s, "big", new(4096))).await,
        BindingDecision::Exceeds(vec![Limit::ResourcesAbove {
            resource: "memory_mb",
            allowed: 1024,
            wanted: 4096
        }])
    );
    // A directory of a machine that is not theirs.
    let machine = {
        let mut conn = s.w.conn().await;
        access_fixtures::insert_persons_machine(&mut conn, s.ada, "laptop")
            .await
            .unwrap()
    };
    let directory = Some(EnvironmentChoice::New(NewEnvironment {
        home: s.w.topic,
        place: Place::Machine { machine },
        mounts: vec![],
    }));
    assert_eq!(
        bind(&s.w, s.dev, &creating(&s, "big", directory)).await,
        BindingDecision::NotYourMachine
    );
}

#[tokio::test]
async fn a_thread_cannot_be_created_under_an_archived_node_by_somebody_who_holds_what_it_takes() {
    let s = setting().await;
    s.w.grant("agent", s.profile, P::AgentUse, Holder::User { id: s.dev })
        .await;
    s.w.grant(
        "provider",
        s.provider,
        P::ProviderUse,
        Holder::User { id: s.dev },
    )
    .await;
    {
        let mut conn = s.w.conn().await;
        orch_db::queries::hierarchy::archive(&mut conn, s.w.topic, s.w.boss, s.w.now.get())
            .await
            .unwrap();
    }
    let principal = s.w.principal(Caller::session(s.dev)).await;
    let mut conn = s.w.conn().await;
    let found = usable::check_binding(
        &mut conn,
        &principal,
        &creating(&s, "big", None),
        &Policies::default(),
    )
    .await;
    assert!(matches!(found, Err(orch_authz::Error::Archived { node }) if node == s.w.topic));
    // Somebody who would be refused anyway is not told of the archive.
    let vee = s.w.member("vee").await;
    s.w.give(s.w.project, vee, Role::Viewer).await;
    let principal = s.w.principal(Caller::session(vee)).await;
    let found = usable::check_binding(
        &mut conn,
        &principal,
        &creating(&s, "big", None),
        &Policies::default(),
    )
    .await
    .unwrap();
    assert!(matches!(found, BindingDecision::Refused(_)));
}
