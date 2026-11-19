//! A provider is used by whoever its page lets: its owner, a person, a group and the work in a place, within the models and efforts each entry allows, and a refusal says what is missing and who can give it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::decisions::Refusal;
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{ObjectKind, Permission as P};
use orch_access_contract::roles::Role;
use orch_authz::Caller;
use orch_authz::usable::{Blocker, TurnDecision, Usable};
use orch_types::Id;
use serde_json::json;
use support::{Statements, World};

/// Acme with a thread under a project, and a provider of its owner `ada` whose Use row names `bea`
/// for one model, a group that `gus` is in, and the work in the project; `pam` is a member who
/// works in the project; `sam` is a stranger. Every one of them but the stranger may drive the
/// thread as shipped.
struct Setting {
    w: World,
    ada: Id,
    bea: Id,
    gus: Id,
    pam: Id,
    sam: Id,
    provider: Id,
}

async fn setting() -> Setting {
    let w = World::new().await;
    let (ada, bea, gus, pam) = (
        w.member("ada").await,
        w.member("bea").await,
        w.member("gus").await,
        w.member("pam").await,
    );
    let sam = w.user("sam").await;
    for person in [ada, bea, gus, pam] {
        w.give(w.project, person, Role::Interactor).await;
    }
    let provider = w.provider(ada, w.root, "Team Pro").await;
    w.model(provider, "big", &["low", "high"]).await;
    w.model(provider, "small", &["low"]).await;
    let group = w.group(w.root, "Ops", &[gus]).await;
    w.put(
        "provider",
        provider,
        P::ProviderUse,
        Holder::User { id: bea },
        json!({ "models": ["small"] }),
    )
    .await;
    w.grant(
        "provider",
        provider,
        P::ProviderUse,
        Holder::Group { id: group },
    )
    .await;
    w.grant(
        "provider",
        provider,
        P::ProviderUse,
        Holder::Node { id: w.project },
    )
    .await;
    Setting {
        w,
        ada,
        bea,
        gus,
        pam,
        sam,
        provider,
    }
}

fn allowed(decision: &TurnDecision) -> bool {
    decision.is_allowed()
}

fn refusal(decision: TurnDecision) -> Refusal {
    match decision {
        TurnDecision::Refused(refusal) => *refusal,
        other => panic!("a refusal was expected, not {other:?}"),
    }
}

/// A thread in another project, which the work named on the provider does not cover, where the same
/// people drive as shipped.
async fn elsewhere(s: &Setting) -> Id {
    let other_project = s.w.node("project", s.w.root, "Other").await;
    let other_topic = s.w.node("topic", other_project, "Elsewhere").await;
    for person in [s.ada, s.bea, s.gus, s.pam] {
        s.w.give(other_project, person, Role::Interactor).await;
    }
    s.w.node("thread", other_topic, "Away").await
}

#[tokio::test]
async fn each_who_the_page_lets_use_a_provider_and_a_stranger_is_refused_for_two_models() {
    let s = setting().await;
    let away = elsewhere(&s).await;
    let ask = |who: Id, thread: Id, model: &'static str| {
        let w = &s.w;
        let provider = s.provider;
        async move { w.turn(who, thread, provider, model, None).await }
    };
    // On a thread of the project that the page names as work, everyone who drives it runs both
    // models: the entry for the work has no limit, and entries combine permissively.
    for who in [s.ada, s.bea, s.gus, s.pam] {
        for model in ["big", "small"] {
            assert!(
                allowed(&ask(who, s.w.thread, model).await),
                "{who} runs {model} in the project"
            );
        }
    }
    // Anywhere else the owner and a group run both models,
    for who in [s.ada, s.gus] {
        assert!(
            allowed(&ask(who, away, "big").await),
            "{who} runs the big one"
        );
        assert!(
            allowed(&ask(who, away, "small").await),
            "{who} runs the small one"
        );
    }
    // the person named for one model runs that one and is refused the other,
    assert!(allowed(&ask(s.bea, away, "small").await));
    let refused = refusal(ask(s.bea, away, "big").await);
    assert_eq!(
        refused.required,
        P::ThreadInteract,
        "the act is driving the thread"
    );
    assert_eq!(
        refused.missing.len(),
        1,
        "of the two things it needs she lacks one"
    );
    let missing = &refused.missing[0];
    assert_eq!(missing.object.kind, ObjectKind::Provider);
    assert_eq!(missing.object.id, s.provider);
    assert_eq!(
        missing.object.name.as_deref(),
        Some("Team Pro"),
        "she can see the thread"
    );
    assert_eq!(missing.permissions, [P::ProviderUse]);
    let wanted = missing.options.as_ref().unwrap();
    assert_eq!(wanted.models.as_deref(), Some(&["big".to_owned()][..]));
    assert!(missing.ask.contains(&s.ada), "the owner can give it");
    assert!(missing.closed.is_none() && !missing.r#override);
    // and somebody who works in the project only is let in by the work named there, and nowhere else.
    assert!(!allowed(&ask(s.pam, away, "small").await));
    // A stranger lacks the thread and the provider, and is told neither's name.
    for model in ["big", "small"] {
        let refused = refusal(ask(s.sam, away, model).await);
        let lacks: Vec<_> = refused
            .missing
            .iter()
            .map(|m| (m.object.kind, m.permissions.clone()))
            .collect();
        assert_eq!(
            lacks,
            [
                (ObjectKind::Thread, vec![P::ThreadInteract]),
                (ObjectKind::Provider, vec![P::ProviderUse]),
            ],
            "everything, not only the first thing"
        );
        assert!(
            refused
                .missing
                .iter()
                .all(|m| m.object.name.is_none() && m.object.owner.is_none())
        );
        assert!(refused.requestable);
        // In the project the provider is open to the work there, so the thread is all that is lacking.
        let refused = refusal(ask(s.sam, s.w.thread, model).await);
        let lacks: Vec<_> = refused.missing.iter().map(|m| m.object.kind).collect();
        assert_eq!(lacks, [ObjectKind::Thread]);
    }
}

#[tokio::test]
async fn the_work_in_a_place_names_no_person_so_it_lets_in_only_a_thread_there() {
    let s = setting().await;
    // Asked about the person alone, the project's entry gives pam nothing.
    assert!(matches!(
        s.w.usable(Caller::session(s.pam), s.provider, None).await,
        Usable::No
    ));
    // For the thread in the project it does, with no limit.
    let Usable::Yes(granted) =
        s.w.usable(Caller::session(s.pam), s.provider, Some(s.w.thread))
            .await
    else {
        panic!("the thread runs on it");
    };
    assert_eq!(granted.work.len(), 1);
    assert_eq!(granted.work[0].place, s.w.project);
    assert_eq!(granted.provider(), Default::default());
    // For a topic of another project it does not.
    let other_project = s.w.node("project", s.w.root, "Other").await;
    let other_topic = s.w.node("topic", other_project, "Topic").await;
    assert!(matches!(
        s.w.usable(Caller::session(s.pam), s.provider, Some(other_topic))
            .await,
        Usable::No
    ));
    // The owner needs no thread.
    assert!(matches!(
        s.w.usable(Caller::session(s.ada), s.provider, None).await,
        Usable::Yes(_)
    ));
}

#[tokio::test]
async fn an_administrators_ordinary_session_needs_a_provider_entry_while_selected_administration_has_native_use()
 {
    let s = setting().await;
    let admin = s.w.member("admin").await;
    s.w.give(s.w.root, admin, Role::Admin).await;
    let administrative = Caller::session(admin).with_admin_mode(true);
    assert!(
        s.w.decide_on(
            administrative.clone(),
            "provider",
            s.provider,
            P::AccessManage
        )
        .await
        .is_allowed(),
        "they administer it"
    );
    assert!(matches!(
        s.w.usable(Caller::session(admin), s.provider, None).await,
        Usable::No
    ));
    assert!(
        !s.w.can_on(admin, "provider", s.provider, P::ProviderUse)
            .await
    );
    assert!(
        s.w.decide_on(
            administrative.clone(),
            "provider",
            s.provider,
            P::ProviderUse
        )
        .await
        .is_allowed()
    );
    assert!(matches!(
        s.w.usable(administrative.clone(), s.provider, None).await,
        Usable::Yes(_)
    ));
    let page =
        s.w.read_thing_as(administrative, "provider", s.provider)
            .await;
    assert!(
        page.rows
            .iter()
            .flat_map(|row| &row.entries)
            .all(|entry| entry.holder != Holder::User { id: admin }),
        "native administration adds no self-naming entry"
    );
    // An explicit page entry then lets the ordinary session use it too.
    s.w.grant(
        "provider",
        s.provider,
        P::ProviderUse,
        Holder::User { id: admin },
    )
    .await;
    assert!(matches!(
        s.w.usable(Caller::session(admin), s.provider, None).await,
        Usable::Yes(_)
    ));
}

#[tokio::test]
async fn a_provider_that_is_switched_off_is_used_by_nobody_and_the_turn_is_blocked_for_everybody() {
    let s = setting().await;
    {
        let mut conn = s.w.conn().await;
        orch_db::queries::resources::access_fixtures::set_disabled(
            &mut conn, "provider", s.provider, true,
        )
        .await
        .unwrap();
    }
    assert!(matches!(
        s.w.usable(Caller::session(s.ada), s.provider, None).await,
        Usable::Unavailable(_)
    ));
    let decision = s.w.turn(s.gus, s.w.thread, s.provider, "big", None).await;
    assert_eq!(
        decision,
        TurnDecision::Blocked(vec![Blocker::ProviderDisabled])
    );
    // A stranger is refused, and learns nothing of its state.
    let refused = refusal(s.w.turn(s.sam, s.w.thread, s.provider, "big", None).await);
    assert!(refused.missing.iter().all(|m| m.closed.is_none()));
}

#[tokio::test]
async fn a_turn_takes_the_same_few_statements_whoever_asks_and_however_much_the_page_names() {
    let s = setting().await;
    let away = elsewhere(&s).await;
    let count = |who: Id, thread: Id, model: &'static str| {
        let w = &s.w;
        let provider = s.provider;
        async move {
            let principal = w.principal(Caller::session(who)).await;
            let mut conn = w.conn().await;
            let before = Statements::now();
            let decision = orch_authz::usable::check_turn(
                &mut conn,
                orch_authz::usable::Author::Person(&principal),
                &orch_authz::usable::TurnAsk {
                    thread,
                    provider,
                    profile: None,
                    model,
                    effort: None,
                    arrangement: None,
                    models_allowed: None,
                },
                &orch_authz::usable::Policies::default(),
            )
            .await
            .unwrap();
            (decision.is_allowed(), Statements::now() - before)
        }
    };
    let (allowed, allowed_cost) = count(s.ada, away, "big").await;
    assert!(allowed);
    let (refused, refused_cost) = count(s.sam, away, "big").await;
    assert!(!refused);
    eprintln!(
        "statements: a turn allowed {allowed_cost}, a turn refused with everything missing {refused_cost}"
    );
    assert!(
        allowed_cost <= 14,
        "an allowed turn takes {allowed_cost} statements"
    );
    assert!(
        refused_cost <= 40,
        "a refused turn takes {refused_cost} statements"
    );
}
