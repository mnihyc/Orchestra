//! An org is no wall: a resource can name anybody registered on the system, who then uses it wherever they work; who may name an outsider is a row, or a policy for the owners; and the address of a resource says where it comes from.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{Arrangement, ObjectKind, Permission as P};
use orch_access_contract::roles::Role;
use orch_authz::environments::{self, Moving};
use orch_authz::pages::ChangeError;
use orch_authz::usable::{Blocker, Policies, ResourcePolicy, TurnDecision, Usable};
use orch_authz::{Caller, Policy, external, resources};
use orch_db::queries::access::testing::{give_role, insert_member};
use orch_db::queries::hierarchy::testing::insert_node;
use orch_types::{Id, new_id};
use serde_json::json;
use support::World;
use support::things::object;

/// Acme, as in every test, and a second root org, Globex, with a project and a thread of its
/// member `gina`, a group of Globex that she is in, and a role she holds there.
struct Globex {
    root: Id,
    project: Id,
    thread: Id,
    gina: Id,
    hank: Id,
    group: Id,
}

async fn globex(w: &World) -> Globex {
    let gina = w.user("gina").await;
    let (root, project, topic, thread) = (new_id(), new_id(), new_id(), new_id());
    let mut conn = w.conn().await;
    insert_node(&mut conn, root, "org", None, "Globex", gina)
        .await
        .unwrap();
    insert_member(&mut conn, root, gina, root).await.unwrap();
    insert_node(&mut conn, project, "project", Some(root), "Web", gina)
        .await
        .unwrap();
    insert_node(&mut conn, topic, "topic", Some(project), "Site", gina)
        .await
        .unwrap();
    insert_node(&mut conn, thread, "thread", Some(topic), "Fix", gina)
        .await
        .unwrap();
    for node in [root, project, topic, thread] {
        orch_db::queries::access::testing::name_owner(&mut conn, node, ("user", gina), w.now.get())
            .await
            .unwrap();
    }
    give_role(&mut conn, root, "user", gina, "developer", gina, None)
        .await
        .unwrap();
    let hank = w.user("hank").await;
    insert_member(&mut conn, root, hank, root).await.unwrap();
    give_role(&mut conn, root, "user", hank, "developer", gina, None)
        .await
        .unwrap();
    let group =
        orch_db::queries::access::testing::insert_group(&mut conn, root, "Globex Devs", gina)
            .await
            .unwrap();
    orch_db::queries::access::testing::add_to_group(&mut conn, group, gina)
        .await
        .unwrap();
    Globex {
        root,
        project,
        thread,
        gina,
        hank,
        group,
    }
}

fn naming(holder: &Holder) -> serde_json::Value {
    json!({ "rows": [{ "permission": "provider.use", "add": [{ "holder": holder }] }] })
}

#[tokio::test]
async fn who_may_name_somebody_of_another_root_org_is_the_row_or_the_owners_where_the_policy_lets_them()
 {
    let w = World::new().await;
    let g = globex(&w).await;
    let (ada, dev, manager) = (
        w.member("ada").await,
        w.member("dev").await,
        w.member("manager").await,
    );
    w.give(w.root, dev, Role::Developer).await;
    // Giving at Acme does not reveal Globex; the giver must see a named holder's source.
    for giver in [ada, dev, manager] {
        w.grant("node", g.root, P::OrgView, Holder::User { id: giver })
            .await;
        w.grant(
            "node",
            g.project,
            P::ProjectView,
            Holder::User { id: giver },
        )
        .await;
    }
    // The four ways in which a page can name somebody from outside: a person, a group, a role held
    // there, and the work in a place there.
    let outsiders = [
        Holder::User { id: g.gina },
        Holder::Group { id: g.group },
        Holder::Role {
            role: Role::Developer,
            at: Some(g.root),
        },
        Holder::Node { id: g.project },
    ];
    let off = ResourcePolicy::default();
    let on = ResourcePolicy {
        owners_name_outsiders: true,
        ..ResourcePolicy::default()
    };
    for holder in &outsiders {
        // Each time on a page of its own, owned by Ada, which Dev and the manager may change.
        for (policy, label) in [(&off, "off"), (&on, "on")] {
            let provider = w.provider(ada, w.root, "Team Pro").await;
            for person in [dev, manager] {
                w.grant(
                    "provider",
                    provider,
                    P::AccessManage,
                    Holder::User { id: person },
                )
                .await;
                // Nobody names more than they hold: both use the provider they open to others.
                w.grant(
                    "provider",
                    provider,
                    P::ProviderUse,
                    Holder::User { id: person },
                )
                .await;
            }
            let by = |who: Id| {
                let (w, provider, holder) = (&w, provider, *holder);
                async move {
                    w.change_thing_as(
                        Caller::session(who),
                        "provider",
                        provider,
                        naming(&holder),
                        policy,
                    )
                    .await
                }
            };
            // A Developer of its home holds the row `access.external`, whatever the policy says.
            assert!(
                by(dev).await.is_ok(),
                "{holder:?}, policy {label}: the Developer names it"
            );
        }
    }
    for holder in &outsiders {
        // One of its owners who is no Developer: with the policy on, held; off, refused and told who can.
        let held = w.provider(ada, w.root, "Held").await;
        let refused = w
            .change_thing_as(Caller::session(ada), "provider", held, naming(holder), &on)
            .await;
        assert!(
            refused.is_ok(),
            "{holder:?}: the owner, where the policy lets owners"
        );
        let shut = w.provider(ada, w.root, "Shut").await;
        w.grant("provider", shut, P::AccessManage, Holder::User { id: dev })
            .await;
        w.grant("provider", shut, P::ProviderUse, Holder::User { id: dev })
            .await;
        let refused = w
            .change_thing_as(Caller::session(ada), "provider", shut, naming(holder), &off)
            .await;
        let Err(ChangeError::Outside { ask, .. }) = refused else {
            panic!("{holder:?}: the owner is refused with the policy off, not {refused:?}");
        };
        assert!(
            ask.contains(&dev),
            "and told who can: a Developer who may change the page"
        );
        // A member who may change the page and is neither is refused, with the policy on or off.
        let neither = w.provider(ada, w.root, "Neither").await;
        w.grant(
            "provider",
            neither,
            P::AccessManage,
            Holder::User { id: manager },
        )
        .await;
        w.grant(
            "provider",
            neither,
            P::ProviderUse,
            Holder::User { id: manager },
        )
        .await;
        for policy in [&off, &on] {
            let refused = w
                .change_thing_as(
                    Caller::session(manager),
                    "provider",
                    neither,
                    naming(holder),
                    policy,
                )
                .await;
            assert!(
                matches!(refused, Err(ChangeError::Outside { .. })),
                "{holder:?}"
            );
        }
    }
    // The option is shown disabled, with who can.
    let shown = w.provider(ada, w.root, "Shown").await;
    w.grant("provider", shown, P::AccessManage, Holder::User { id: dev })
        .await;
    let principal = w.principal(Caller::session(ada)).await;
    let mut conn = w.conn().await;
    let naming_right = external::may_name(
        &mut conn,
        &principal,
        object("provider", shown),
        off.node_policy(),
    )
    .await
    .unwrap();
    assert!(!naming_right.allowed());
    assert!(naming_right.ask.contains(&dev));
    let naming_right = external::may_name(
        &mut conn,
        &principal,
        object("provider", shown),
        on.node_policy(),
    )
    .await
    .unwrap();
    assert!(naming_right.allowed());
}

#[tokio::test]
async fn a_colleague_of_a_sibling_department_is_no_outsider_and_a_closed_row_is_opened_where_it_was_locked()
 {
    let w = World::new().await;
    let d = w.deep().await;
    let ada = w.member("ada").await;
    let colleague = w.user("colleague").await;
    {
        let mut conn = w.conn().await;
        insert_member(&mut conn, w.root, colleague, d.sales)
            .await
            .unwrap();
    }
    let provider = w.provider(ada, d.d1, "Team Pro").await;
    // Anybody who may change the page names a colleague of a sibling department, policy off.
    w.change_thing(
        ada,
        "provider",
        provider,
        naming(&Holder::User { id: colleague }),
    )
    .await
    .unwrap();
    assert!(matches!(
        w.usable(Caller::session(colleague), provider, None).await,
        Usable::Yes(_)
    ));
    // The department empties the row `access.external` and locks it: a Developer is refused, and the
    // row is changed where the lock is.
    let dev = w.member("dev").await;
    w.give(w.root, dev, Role::Developer).await;
    w.grant(
        "provider",
        provider,
        P::AccessManage,
        Holder::User { id: dev },
    )
    .await;
    w.grant(
        "provider",
        provider,
        P::ProviderUse,
        Holder::User { id: dev },
    )
    .await;
    w.default_row(d.d1, P::AccessExternal, "any", false, true)
        .await;
    let outsider = w.user("olga").await;
    let refused = w
        .change_thing(
            dev,
            "provider",
            provider,
            naming(&Holder::User { id: outsider }),
        )
        .await;
    assert!(
        matches!(refused, Err(ChangeError::Outside { .. })),
        "the row is empty and locked: the Developer holds nothing"
    );
    // An admin above is refused to put themself on the page of the row below the lock.
    let naming_the_row = |holder: Id| json!({ "rows": [{ "permission": "access.external", "add": [{ "holder": { "kind": "user", "id": holder } }] }] });
    let admin = Caller::session(w.boss).with_admin_mode(true);
    let refused = w
        .change_thing_as(
            admin.clone(),
            "provider",
            provider,
            naming_the_row(w.boss),
            &ResourcePolicy::default(),
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(ChangeError::Locked {
                permission: P::AccessExternal,
                ..
            })
        ),
        "not below the lock"
    );
    // The admin changes the advanced row where it is locked. Native Manage already includes Use,
    // so enabling administration needs no self-naming Use entry before naming the outsider.
    w.change_as(
        admin.clone(),
        orch_authz::pages::Target::Defaults(orch_access_contract::roles::PlaceRef::Node(d.d1)),
        naming_the_row(w.boss),
        Policy::default(),
    )
    .await
    .unwrap();
    let without_admin_mode = w
        .change_thing(
            w.boss,
            "provider",
            provider,
            naming(&Holder::User { id: outsider }),
        )
        .await;
    assert!(
        matches!(without_admin_mode, Err(ChangeError::Denied(_))),
        "ordinary rights still come from the page: {without_admin_mode:?}"
    );
    let admitted = w
        .change_thing_as(
            admin,
            "provider",
            provider,
            naming(&Holder::User { id: outsider }),
            &ResourcePolicy::default(),
        )
        .await
        .unwrap();
    assert!(admitted.changed);
    assert!(!admitted.r#override, "native Manage already includes Use");
    let self_use_entries: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM permission_entries WHERE object_kind='provider' AND object_id=$1
          AND permission='provider.use' AND holder_kind='user' AND holder_id=$2 AND removed_at IS NULL",
    )
    .bind(provider)
    .bind(w.boss)
    .fetch_one(&mut *w.conn().await)
    .await
    .unwrap();
    assert_eq!(
        self_use_entries, 0,
        "native reach never adds a self-naming row"
    );
    // An owner trying to name a holder of that row is refused, whatever the policy.
    let refused = w
        .change_thing_as(
            Caller::session(ada),
            "provider",
            provider,
            naming_the_row(ada),
            &ResourcePolicy {
                owners_name_outsiders: true,
                ..ResourcePolicy::default()
            },
        )
        .await;
    assert!(matches!(
        refused,
        Err(ChangeError::Denied(_) | ChangeError::Locked { .. })
    ));
}

#[tokio::test]
async fn somebody_named_runs_their_thread_of_their_own_org_on_the_provider_and_puts_it_in_the_environment()
 {
    let w = World::new().await;
    let g = globex(&w).await;
    let ada = w.member("ada").await;
    let provider = w.provider(ada, w.root, "Team Pro").await;
    w.model(provider, "big", &["low"]).await;
    let environment = w.thing("environment", ada, w.topic, "shared").await;
    let allow = ResourcePolicy {
        owners_name_outsiders: true,
        ..ResourcePolicy::default()
    };
    // Ada names Gina in the Use row of the provider (limited to a model and an arrangement) and in
    // the Manage row of the environment, which an owner does where the policy lets owners.
    let entry = w
        .put(
            "provider",
            provider,
            P::ProviderUse,
            Holder::User { id: g.gina },
            json!({ "models": ["big"], "arrangements": ["key_separated"] }),
        )
        .await;
    w.change_thing_as(
        Caller::session(ada),
        "environment",
        environment,
        json!({ "rows": [{ "permission": "environment.manage", "add": [{ "holder": { "kind": "user", "id": g.gina } }] }] }),
        &allow,
    )
    .await
    .unwrap();
    // Her thread of her own org runs on it, and is put into the environment.
    assert!(
        w.turn(g.gina, g.thread, provider, "big", None)
            .await
            .is_allowed()
    );
    let principal = w.principal(Caller::session(g.gina)).await;
    let mut conn = w.conn().await;
    let moving = Moving {
        thread: Some(g.thread),
        from: None,
        to: Some(environment),
        all_of_the_environment: false,
    };
    let decision = environments::check_move(&mut conn, &principal, &moving, Policy::default())
        .await
        .unwrap();
    assert!(
        decision.is_allowed(),
        "an environment that names her takes her threads"
    );
    drop(conn);
    // An arrangement type allowed by one side only is refused: the entry names one, the thread is bound in another.
    let ask = |arrangement: Arrangement| {
        let w = &w;
        let (thread, provider) = (g.thread, provider);
        async move {
            let principal = w.principal(Caller::session(g.gina)).await;
            let mut conn = w.conn().await;
            orch_authz::usable::check_turn(
                &mut conn,
                orch_authz::usable::Author::Person(&principal),
                &orch_authz::usable::TurnAsk {
                    thread,
                    provider,
                    profile: None,
                    model: "big",
                    effort: None,
                    arrangement: Some(arrangement),
                    models_allowed: None,
                },
                &Policies::default(),
            )
            .await
            .unwrap()
        }
    };
    assert!(ask(Arrangement::KeySeparated).await.is_allowed());
    assert_eq!(
        ask(Arrangement::Separated).await,
        TurnDecision::Blocked(vec![Blocker::ArrangementNotAllowed {
            arrangement: Arrangement::Separated
        }]),
        "the provider's owner allows it and the entry does not"
    );
    {
        let mut conn = w.conn().await;
        orch_db::queries::resources::access_fixtures::set_arrangements(
            &mut conn,
            provider,
            &["separated"],
        )
        .await
        .unwrap();
    }
    assert!(
        !ask(Arrangement::KeySeparated).await.is_allowed(),
        "and the other way round: the owner allows one side's"
    );
    {
        let mut conn = w.conn().await;
        orch_db::queries::resources::access_fixtures::set_arrangements(
            &mut conn,
            provider,
            &["separated", "key_separated"],
        )
        .await
        .unwrap();
    }
    // The provider's home disables it, or removes the entry: the next turn is refused.
    {
        let mut conn = w.conn().await;
        orch_db::queries::resources::access_fixtures::set_disabled(
            &mut conn, "provider", provider, true,
        )
        .await
        .unwrap();
    }
    assert_eq!(
        w.turn(g.gina, g.thread, provider, "big", None).await,
        TurnDecision::Blocked(vec![Blocker::ProviderDisabled])
    );
    {
        let mut conn = w.conn().await;
        orch_db::queries::resources::access_fixtures::set_disabled(
            &mut conn, "provider", provider, false,
        )
        .await
        .unwrap();
        orch_db::queries::access::remove_entry(&mut conn, entry, ada, w.now.get())
            .await
            .unwrap();
    }
    let TurnDecision::Refused(refusal) = w.turn(g.gina, g.thread, provider, "big", None).await
    else {
        panic!("the entry is gone");
    };
    assert_eq!(refusal.missing[0].object.kind, ObjectKind::Provider);
    assert!(refusal.missing[0].ask.contains(&ada));
}

#[tokio::test]
async fn a_role_held_in_another_org_is_held_for_as_long_as_the_person_holds_it_in_their_own() {
    let w = World::new().await;
    let g = globex(&w).await;
    let ada = w.member("ada").await;
    w.grant("node", g.root, P::OrgView, Holder::User { id: ada })
        .await;
    let provider = w.provider(ada, w.root, "Team Pro").await;
    let developers = Holder::Role {
        role: Role::Developer,
        at: Some(g.root),
    };
    w.change_thing_as(
        Caller::session(ada),
        "provider",
        provider,
        naming(&developers),
        &ResourcePolicy {
            owners_name_outsiders: true,
            ..ResourcePolicy::default()
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(
            w.usable(Caller::session(g.hank), provider, None).await,
            Usable::Yes(_)
        ),
        "he is a Developer of Globex"
    );
    // He loses the role in his own org: he loses the provider here.
    {
        let mut conn = w.conn().await;
        sqlx::query(
            "UPDATE node_roles SET removed_at = now() WHERE node_id = $1 AND subject_id = $2",
        )
        .bind(g.root)
        .bind(g.hank)
        .execute(&mut *conn)
        .await
        .unwrap();
    }
    assert!(matches!(
        w.usable(Caller::session(g.hank), provider, None).await,
        Usable::No
    ));
}

#[tokio::test]
async fn the_address_of_a_resource_says_where_it_comes_from() {
    let w = World::new().await;
    let d = w.deep().await;
    let ada = w.member("ada").await;
    let on_root = w.provider(ada, w.root, "main").await;
    let deep = w.thing("host", ada, d.d3, "gpu").await;
    let environment = w.thing("environment", ada, d.topic, "work").await;
    let system = w.system_thing("template", "base").await;
    let mut conn = w.conn().await;
    let slug = |org: Id| {
        let w = &w;
        async move {
            let mut conn = w.conn().await;
            let slug: String = sqlx::query_scalar("SELECT slug FROM orgs WHERE node_id = $1")
                .bind(org)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
            slug
        }
    };
    let found = resources::addresses(&mut conn, &[deep, on_root, system, environment, new_id()])
        .await
        .unwrap();
    let (r, d1, d2, d3) = (
        slug(w.root).await,
        slug(d.d1).await,
        slug(d.d2).await,
        slug(d.d3).await,
    );
    let user = "ada";
    assert_eq!(
        found,
        [
            (deep, format!("{r}.{d1}.{d2}.{d3}.{user}.gpu")),
            (on_root, format!("{r}.{user}.main")),
            (system, "system.base".to_owned()),
            (environment, format!("{r}.{d1}.{d2}.{d3}.{user}.work")),
        ],
        "in the order asked, from the root org down to the home, a thing that is not there left out"
    );
}
