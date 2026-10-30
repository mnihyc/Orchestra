//! An input needs its author to hold `thread.interact` and to be able to run the thread's model themself; one that nobody sent runs on the use its thread's first owner has, and waits while that fails without holding anybody else's up.
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
use orch_authz::usable::{self, Author, Blocker, Policies, TurnAsk, TurnDecision};
use orch_authz::{Caller, Error};
use orch_types::Id;
use serde_json::json;
use support::World;

struct Setting {
    w: World,
    ada: Id,
    provider: Id,
    thread: Id,
}

/// A thread in a topic whose Interactors drive it as shipped, owned by the maker, on a provider of
/// `ada` with two models.
async fn setting() -> Setting {
    let w = World::new().await;
    let ada = w.member("ada").await;
    let provider = w.provider(ada, w.root, "Team Pro").await;
    w.model(provider, "big", &["low", "high"]).await;
    w.model(provider, "small", &["low"]).await;
    let thread = w.node("thread", w.topic, "Fix").await;
    Setting {
        w,
        ada,
        provider,
        thread,
    }
}

async fn turn(
    s: &Setting,
    author: Author<'_>,
    model: &str,
    effort: Option<&str>,
) -> Result<TurnDecision, Error> {
    let mut conn = s.w.conn().await;
    usable::check_turn(
        &mut conn,
        author,
        &TurnAsk {
            thread: s.thread,
            provider: s.provider,
            profile: None,
            model,
            effort,
            arrangement: None,
            models_allowed: None,
        },
        &Policies::default(),
    )
    .await
}

async fn by(s: &Setting, who: Id, model: &str) -> TurnDecision {
    let principal = s.w.principal(Caller::session(who)).await;
    turn(s, Author::Person(&principal), model, None)
        .await
        .unwrap()
}

fn refusal(decision: TurnDecision) -> Refusal {
    match decision {
        TurnDecision::Refused(refusal) => *refusal,
        other => panic!("a refusal was expected, not {other:?}"),
    }
}

fn kinds(refusal: &Refusal) -> Vec<ObjectKind> {
    refusal
        .missing
        .iter()
        .map(|entry| entry.object.kind)
        .collect()
}

#[tokio::test]
async fn an_author_needs_both_the_permission_on_the_thread_and_the_model_and_is_told_which_is_missing()
 {
    let s = setting().await;
    let (with_both, permission_only, model_only) = (
        s.w.member("both").await,
        s.w.member("perm").await,
        s.w.member("model").await,
    );
    for person in [with_both, permission_only] {
        s.w.give(s.w.project, person, Role::Interactor).await;
    }
    s.w.put(
        "provider",
        s.provider,
        P::ProviderUse,
        Holder::User { id: with_both },
        json!({ "models": ["big"] }),
    )
    .await;
    s.w.put(
        "provider",
        s.provider,
        P::ProviderUse,
        Holder::User { id: model_only },
        json!({ "models": ["big"] }),
    )
    .await;
    assert!(by(&s, with_both, "big").await.is_allowed());
    // The permission and not the model.
    let refused = refusal(by(&s, permission_only, "big").await);
    assert_eq!(kinds(&refused), [ObjectKind::Provider]);
    // The model and not the permission.
    let refused = refusal(by(&s, model_only, "big").await);
    assert_eq!(kinds(&refused), [ObjectKind::Thread]);
    assert_eq!(refused.missing[0].permissions, [P::ThreadInteract]);
    // The thread's model changed so that a waiting author no longer has it.
    let refused = refusal(by(&s, with_both, "small").await);
    assert_eq!(kinds(&refused), [ObjectKind::Provider]);
    // thread.interact lost while the input waited.
    s.w.row(s.thread, P::ThreadInteract, false).await;
    s.w.row(s.thread, P::ThreadManage, false).await;
    let refused = refusal(by(&s, with_both, "big").await);
    assert_eq!(kinds(&refused), [ObjectKind::Thread]);
}

#[tokio::test]
async fn the_provider_reached_through_a_node_entry_limited_to_the_model_in_use_serves_whoever_drives_the_thread()
 {
    let s = setting().await;
    let driver = s.w.member("driver").await;
    s.w.give(s.w.project, driver, Role::Interactor).await;
    s.w.put(
        "provider",
        s.provider,
        P::ProviderUse,
        Holder::Node { id: s.w.topic },
        json!({ "models": ["big"] }),
    )
    .await;
    // "Include the model": the entry names the topic and the model, and the driver has nothing else.
    assert!(by(&s, driver, "big").await.is_allowed());
    let refused = refusal(by(&s, driver, "small").await);
    assert_eq!(
        kinds(&refused),
        [ObjectKind::Provider],
        "the entry does not carry the other model"
    );
    // The entry is one the turn is put down to.
    let principal = s.w.principal(Caller::session(driver)).await;
    let TurnDecision::Allowed(grant) = turn(&s, Author::Person(&principal), "big", None)
        .await
        .unwrap()
    else {
        panic!("allowed");
    };
    assert!(grant.entry.is_some());
}

#[tokio::test]
async fn an_input_nobody_sent_runs_on_the_use_its_first_owner_has_and_waits_while_that_fails() {
    let s = setting().await;
    // The thread is owned by a person who is a member of the org.
    let owner = s.w.member("owner").await;
    {
        let mut conn = s.w.conn().await;
        orch_db::queries::hierarchy::set_first_owner(&mut conn, s.thread, owner, s.w.now.get())
            .await
            .unwrap();
        orch_db::queries::access::testing::name_owner(
            &mut conn,
            s.thread,
            ("user", owner),
            s.w.now.get(),
        )
        .await
        .unwrap();
    }
    let teammate = s.w.member("mate").await;
    for person in [owner, teammate] {
        s.w.give(s.w.project, person, Role::Interactor).await;
    }
    s.w.grant(
        "provider",
        s.provider,
        P::ProviderUse,
        Holder::User { id: teammate },
    )
    .await;
    let first_owner = |model: &'static str| {
        let s = &s;
        async move {
            turn(s, Author::FirstOwner { now: s.w.now }, model, None)
                .await
                .unwrap()
        }
    };
    // The owner may not use the provider: the input without an author waits, and a teammate's goes ahead.
    let waits = refusal(first_owner("big").await);
    assert_eq!(
        kinds(&waits),
        [ObjectKind::Provider],
        "nothing is asked of the thread itself"
    );
    assert!(by(&s, teammate, "big").await.is_allowed());
    // Once the owner may, it goes ahead.
    s.w.grant(
        "provider",
        s.provider,
        P::ProviderUse,
        Holder::User { id: owner },
    )
    .await;
    assert!(first_owner("big").await.is_allowed());
    // The owner is suspended in the org: they use nothing there, and the input waits for them.
    {
        let mut conn = s.w.conn().await;
        orch_db::queries::access::testing::suspend_in(&mut conn, s.w.root, owner, s.w.boss)
            .await
            .unwrap();
    }
    assert!(matches!(first_owner("big").await, TurnDecision::Refused(_)));
    assert!(
        by(&s, teammate, "big").await.is_allowed(),
        "and nobody else's is held up"
    );
}

#[tokio::test]
async fn a_suspended_author_is_told_so_and_an_archived_thread_is_told_to_those_who_would_otherwise_drive_it()
 {
    let s = setting().await;
    let author = s.w.member("author").await;
    s.w.give(s.w.project, author, Role::Interactor).await;
    s.w.grant(
        "provider",
        s.provider,
        P::ProviderUse,
        Holder::User { id: author },
    )
    .await;
    assert!(by(&s, author, "big").await.is_allowed());
    // Archived: the input is refused as a mutation under an archived node.
    {
        let mut conn = s.w.conn().await;
        orch_db::queries::hierarchy::archive(&mut conn, s.thread, s.w.boss, s.w.now.get())
            .await
            .unwrap();
    }
    let principal = s.w.principal(Caller::session(author)).await;
    let found = turn(&s, Author::Person(&principal), "big", None).await;
    assert!(matches!(found, Err(Error::Archived { node }) if node == s.thread));
    // Suspended in the org: nothing is held there.
    {
        let mut conn = s.w.conn().await;
        orch_db::queries::access::testing::suspend_in(&mut conn, s.w.root, author, s.w.boss)
            .await
            .unwrap();
    }
    let principal = s.w.principal(Caller::session(author)).await;
    let found = turn(&s, Author::Person(&principal), "big", None).await;
    assert!(matches!(found, Err(Error::Suspended { org: Some(org) }) if org == s.w.root));
}

#[tokio::test]
async fn a_model_the_provider_no_longer_lists_blocks_the_thread_for_everybody_and_not_one_person() {
    let s = setting().await;
    let author = s.w.member("author").await;
    s.w.give(s.w.project, author, Role::Interactor).await;
    s.w.grant(
        "provider",
        s.provider,
        P::ProviderUse,
        Holder::User { id: author },
    )
    .await;
    {
        let mut conn = s.w.conn().await;
        sqlx::query("UPDATE provider_models SET enabled = false WHERE provider_id = $1 AND model_name = 'big'")
            .bind(s.provider)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    assert_eq!(
        by(&s, author, "big").await,
        TurnDecision::Blocked(vec![Blocker::ModelNotAllowed {
            model: "big".into()
        }])
    );
    // The owner of the provider holds nothing on the thread, so they are refused the thread before
    // anything is said of the state of the model.
    let refused = refusal(by(&s, s.ada, "big").await);
    assert_eq!(kinds(&refused), [ObjectKind::Thread]);
}
