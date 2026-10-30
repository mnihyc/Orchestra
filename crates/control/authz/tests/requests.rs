//! Access requests on a real database: who one is sent to, what cannot be asked, and that a decision gives exactly what was asked, by someone who could have given it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_access_contract::roles::Role;
use orch_authz::pages::ChangeError;
use orch_authz::requests::{
    Ask, AskError, Asked, Giving, GrantError, Sent, Whom, grant, is_held, plan,
};
use orch_authz::usable::ResourcePolicy;
use orch_authz::{Caller, Policy};
use orch_db::audit::Origin;
use orch_types::{Id, Time};
use serde_json::json;
use support::{World, user_holder};

fn rules() -> ResourcePolicy {
    ResourcePolicy {
        owners_name_outsiders: true,
        shared_subscriptions: None,
    }
}

fn topic_ask(w: &World, permissions: &[Permission]) -> Ask {
    Ask {
        label: None,
        object: ObjectRef {
            kind: ObjectKind::Topic,
            id: w.topic,
        },
        permissions: permissions.to_vec(),
        options: None,
        whom: Whom::Requester,
    }
}

async fn planned(w: &World, who: Id, ask: &Ask) -> Result<Sent, AskError> {
    let principal = w.principal(Caller::session(who)).await;
    let mut conn = w.conn().await;
    plan(&mut conn, &principal, ask, &rules()).await
}

async fn granted(
    w: &World,
    approver: Id,
    requester: Id,
    ask: &Ask,
    giving: &Giving,
) -> Result<Vec<Permission>, GrantError> {
    granted_under(w, approver, requester, ask, giving, &rules()).await
}

async fn granted_under(
    w: &World,
    approver: Id,
    requester: Id,
    ask: &Ask,
    giving: &Giving,
    policy: &ResourcePolicy,
) -> Result<Vec<Permission>, GrantError> {
    let principal = w.principal(Caller::session(approver)).await;
    let asked = Asked::of(ask, "please");
    let mut tx = w.db.pool.begin().await.unwrap();
    let outcome = grant(
        &mut tx,
        &principal,
        requester,
        ask.object,
        &asked,
        giving,
        policy,
        &Origin::default(),
    )
    .await;
    if outcome.is_ok() {
        tx.commit().await.unwrap();
    }
    outcome.map(|(_, given)| given)
}

#[tokio::test]
async fn a_request_goes_to_the_owners_and_the_admins_of_the_first_place_that_has_any_and_never_to_the_person_who_asks()
 {
    let w = World::new().await;
    let rae = w.member("rae").await;
    let dee = w.member("dee").await;
    w.give(w.root, rae, Role::Viewer).await;
    w.give(w.project, dee, Role::Admin).await;
    let sent = planned(&w, rae, &topic_ask(&w, &[Permission::TopicInteract]))
        .await
        .unwrap();
    assert!(
        sent.givers.nearest.contains(&w.boss),
        "the owner of the topic"
    );
    assert!(
        sent.givers.nearest.contains(&dee),
        "an admin of the project it lies in"
    );
    assert!(!sent.givers.nearest.contains(&rae));
    assert_eq!(sent.kind, ObjectKind::Topic);
    // The person who owns it and asks for more is not asked of themself.
    let by_owner = planned(&w, w.boss, &topic_ask(&w, &[Permission::TopicInteract])).await;
    assert!(matches!(by_owner, Err(AskError::Held)), "{by_owner:?}");
}

#[tokio::test]
async fn what_is_held_what_is_locked_and_what_is_made_up_is_not_planned() {
    let w = World::new().await;
    let rae = w.member("rae").await;
    w.give(w.root, rae, Role::Viewer).await;
    let held = planned(&w, rae, &topic_ask(&w, &[Permission::TopicView])).await;
    assert!(matches!(held, Err(AskError::Held)), "{held:?}");
    let owner_row = planned(&w, rae, &topic_ask(&w, &[Permission::AccessOwn])).await;
    assert!(
        matches!(
            owner_row,
            Err(AskError::Invalid {
                path: "permissions",
                ..
            })
        ),
        "{owner_row:?}"
    );
    let on_a_thread = planned(&w, rae, &topic_ask(&w, &[Permission::ThreadView])).await;
    assert!(matches!(
        on_a_thread,
        Err(AskError::Invalid {
            path: "permissions",
            ..
        })
    ));
    let nothing = planned(&w, rae, &topic_ask(&w, &[])).await;
    assert!(matches!(
        nothing,
        Err(AskError::Invalid {
            path: "permissions",
            ..
        })
    ));
    let mut elsewhere = topic_ask(&w, &[Permission::TopicInteract]);
    elsewhere.object.id = Id::nil();
    assert!(matches!(
        planned(&w, rae, &elsewhere).await,
        Err(AskError::NotFound)
    ));
    let mut wrong_kind = topic_ask(&w, &[Permission::TopicInteract]);
    wrong_kind.object.kind = ObjectKind::Project;
    assert!(matches!(
        planned(&w, rae, &wrong_kind).await,
        Err(AskError::NotFound)
    ));
    let mut options = topic_ask(&w, &[Permission::TopicInteract]);
    options.options = Some(orch_authz::requests::AskedOptions::default());
    assert!(matches!(
        planned(&w, rae, &options).await,
        Err(AskError::Invalid {
            path: "options",
            ..
        })
    ));
    // A lock above decides the row: nobody gives it on this page.
    w.change_defaults(
        w.boss,
        w.root,
        json!({ "rows": [{ "permission": "topic.interact", "locked": true }] }),
    )
    .await
    .unwrap();
    let locked = planned(&w, rae, &topic_ask(&w, &[Permission::TopicInteract])).await;
    assert!(matches!(locked, Err(AskError::Closed { .. })), "{locked:?}");
}

#[tokio::test]
async fn a_decision_gives_exactly_what_was_asked_and_marks_it_as_a_request() {
    let w = World::new().await;
    let rae = w.member("rae").await;
    w.give(w.root, rae, Role::Viewer).await;
    let ask = topic_ask(&w, &[Permission::TopicInteract]);
    assert!(!w.can(rae, w.topic, Permission::TopicInteract).await);
    let given = granted(&w, w.boss, rae, &ask, &Giving::default())
        .await
        .unwrap();
    assert_eq!(given, [Permission::TopicInteract]);
    assert!(w.can(rae, w.topic, Permission::TopicInteract).await);
    assert!(
        !w.can(rae, w.topic, Permission::TopicManage).await,
        "nothing beyond it"
    );
    let source: String = sqlx::query_scalar(
        "SELECT source FROM permission_entries
          WHERE object_id = $1 AND holder_id = $2 AND removed_at IS NULL AND permission = 'topic.interact'",
    )
    .bind(w.topic)
    .bind(rae)
    .fetch_one(&w.db.pool)
    .await
    .unwrap();
    assert_eq!(source, "request");
    let mut conn = w.conn().await;
    let principal = w.principal(Caller::session(rae)).await;
    assert!(
        is_held(
            &mut conn,
            &principal,
            ask.object,
            &Asked::of(&ask, ""),
            Policy::default()
        )
        .await
        .unwrap()
    );
}

#[tokio::test]
async fn a_decision_cannot_reach_beyond_the_ask_and_is_made_by_somebody_who_could_have_given_it() {
    let w = World::new().await;
    let rae = w.member("rae").await;
    let ben = w.member("ben").await;
    w.give(w.root, rae, Role::Viewer).await;
    w.give(w.root, ben, Role::Viewer).await;
    let ask = topic_ask(&w, &[Permission::TopicInteract]);
    let beyond = granted(
        &w,
        w.boss,
        rae,
        &ask,
        &Giving {
            permissions: Some(vec![Permission::TopicManage]),
            ..Giving::default()
        },
    )
    .await;
    assert!(
        matches!(
            beyond,
            Err(GrantError::Invalid {
                path: "permissions",
                ..
            })
        ),
        "{beyond:?}"
    );
    let empty = granted(
        &w,
        w.boss,
        rae,
        &ask,
        &Giving {
            permissions: Some(Vec::new()),
            ..Giving::default()
        },
    )
    .await;
    assert!(matches!(
        empty,
        Err(GrantError::Invalid {
            path: "permissions",
            ..
        })
    ));
    let past = granted(
        &w,
        w.boss,
        rae,
        &ask,
        &Giving {
            expires_at: Some(Time::new(w.now.get() - chrono::Duration::hours(1))),
            ..Giving::default()
        },
    )
    .await;
    assert!(matches!(
        past,
        Err(GrantError::Invalid {
            path: "expires_at",
            ..
        })
    ));
    // Ben cannot change the page, so he cannot give; nobody gives to themself.
    let by_ben = granted(&w, ben, rae, &ask, &Giving::default()).await;
    assert!(
        matches!(by_ben, Err(GrantError::Change(error)) if matches!(*error, ChangeError::Denied(_)))
    );
    let own = granted(&w, rae, rae, &ask, &Giving::default()).await;
    assert!(matches!(own, Err(GrantError::OwnRequest)));
    assert!(
        !w.can(rae, w.topic, Permission::TopicInteract).await,
        "nothing was given"
    );
    // An end that is set is the entry's end, after which it counts for nothing.
    let end = Time::new(w.now.get() + chrono::Duration::days(2));
    granted(
        &w,
        w.boss,
        rae,
        &ask,
        &Giving {
            expires_at: Some(end),
            ..Giving::default()
        },
    )
    .await
    .unwrap();
    assert!(w.can(rae, w.topic, Permission::TopicInteract).await);
    assert!(
        !w.can_at(
            rae,
            w.topic,
            Permission::TopicInteract,
            w.later(chrono::Duration::days(3))
        )
        .await
    );
    let _ = user_holder;
}

#[tokio::test]
async fn somebody_from_outside_is_named_by_a_decision_only_as_the_approver_may_name_an_outsider() {
    let w = World::new().await;
    // The maker owns a thread, which is no place: owning it makes nobody an admin of anything.
    let olga = w.user("olga").await;
    let ask = Ask {
        label: None,
        object: ObjectRef {
            kind: ObjectKind::Thread,
            id: w.thread,
        },
        permissions: vec![Permission::ThreadView],
        options: None,
        whom: Whom::Requester,
    };
    let strict = ResourcePolicy {
        owners_name_outsiders: false,
        shared_subscriptions: None,
    };
    let refused = granted_under(&w, w.maker, olga, &ask, &Giving::default(), &strict).await;
    assert!(
        matches!(&refused, Err(GrantError::Change(error)) if matches!(**error, ChangeError::Outside { .. })),
        "{refused:?}"
    );
    assert!(!w.can(olga, w.thread, Permission::ThreadView).await);
    // Where the policy lets owners name outsiders, the owner does.
    granted_under(&w, w.maker, olga, &ask, &Giving::default(), &rules())
        .await
        .unwrap();
    assert!(w.can(olga, w.thread, Permission::ThreadView).await);
}
