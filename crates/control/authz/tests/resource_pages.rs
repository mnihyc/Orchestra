//! The page of a resource is read and changed like a node's: its owners, its managers, its outsiders, a subscription kept to its owner, the resources of the system, and the first owner.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::decisions::Closed;
use orch_access_contract::pages::{Holder, PageState};
use orch_access_contract::permissions::Permission as P;
use orch_access_contract::roles::{PlaceRef, Role};
use orch_authz::Caller;
use orch_authz::pages::ChangeError;
use orch_authz::usable::{Policies, ResourcePolicy, TurnDecision, Usable};
use serde_json::json;
use support::World;

#[tokio::test]
async fn the_page_of_a_new_resource_is_its_owners_alone_and_follows_the_defaults_of_its_home() {
    let w = World::new().await;
    let ada = w.member("ada").await;
    let provider = w.provider(ada, w.root, "Team Pro").await;
    let page = w.read_thing(ada, "provider", provider).await;
    assert_eq!(page.name, "Team Pro");
    assert_eq!(page.owners.len(), 1, "its maker owns it");
    assert!(page.writable && page.owners_writable);
    assert_eq!(page.state, PageState::Follows);
    let permissions: Vec<P> = page.rows.iter().map(|row| row.permission).collect();
    assert_eq!(
        permissions,
        [
            P::AccessManage,
            P::AccessExternal,
            P::ProviderView,
            P::ProviderUse,
            P::ProviderManage,
            P::ProviderBypass
        ],
        "the rows of a provider, in the order of the catalogue, and nothing of a node"
    );
    assert!(
        page.rows
            .iter()
            .all(|row| row.inherit && row.entries.is_empty())
    );
    assert_eq!(
        page.admins_of,
        [PlaceRef::Node(w.root), PlaceRef::System],
        "the admins of its home, and the system's"
    );
    // A person who owns nothing of it changes nothing.
    let bea = w.member("bea").await;
    let page = w.read_thing(bea, "provider", provider).await;
    assert!(!page.writable);
}

#[tokio::test]
async fn the_owner_names_a_person_with_options_and_the_change_is_audited_against_the_provider() {
    let w = World::new().await;
    let (ada, bea) = (w.member("ada").await, w.member("bea").await);
    let provider = w.provider(ada, w.root, "Team Pro").await;
    let before = w.epoch().await;
    let outcome = w
        .change_thing(
            ada,
            "provider",
            provider,
            json!({ "rows": [{ "permission": "provider.use",
                                "add": [{ "holder": { "kind": "user", "id": bea },
                                           "options": { "models": ["small"] } }] }] }),
        )
        .await
        .unwrap();
    assert!(outcome.changed && !outcome.r#override);
    assert_eq!(w.epoch().await, before + 1, "the epoch moves once");
    let page = w.read_thing(ada, "provider", provider).await;
    let row = page
        .rows
        .iter()
        .find(|row| row.permission == P::ProviderUse)
        .unwrap();
    assert_eq!(row.entries.len(), 1);
    assert_eq!(row.entries[0].options, json!({ "models": ["small"] }));
    assert_eq!(page.revision, 2, "a page nobody changed is the first");
    let rows = w.audit().await;
    let added = rows
        .iter()
        .find(|row| row.action == "permission.added")
        .unwrap();
    assert!(added.history && !added.r#override);
    assert_eq!(added.node_id, None, "a resource lies in no node");
    assert_eq!(added.details["object"]["kind"], "provider");
    assert_eq!(added.details["object"]["id"], provider.to_string());
    assert!(added.summary.contains("Team Pro"), "{}", added.summary);
    // Options the permission does not take are refused, naming the member.
    let refused = w
        .change_thing(
            ada,
            "provider",
            provider,
            json!({ "rows": [{ "permission": "provider.use",
                                "add": [{ "holder": { "kind": "user", "id": ada },
                                           "options": { "colour": "red" } }] }] }),
        )
        .await;
    assert!(
        matches!(refused, Err(ChangeError::Invalid(faults)) if faults[0].path.ends_with("options.colour"))
    );
    // A holder the row does not take is refused: the work in a place stands in a Use row only.
    let refused = w
        .change_thing(
            ada,
            "provider",
            provider,
            json!({ "rows": [{ "permission": "provider.view",
                                "add": [{ "holder": { "kind": "node", "id": w.project } }] }] }),
        )
        .await;
    assert!(matches!(refused, Err(ChangeError::Invalid(_))));
}

#[tokio::test]
async fn somebody_named_to_change_the_page_cannot_pass_that_on_but_an_owner_and_an_admin_can() {
    let w = World::new().await;
    let (ada, mia, bea, zed) = (
        w.member("ada").await,
        w.member("mia").await,
        w.member("bea").await,
        w.member("zed").await,
    );
    let admin = w.member("admin").await;
    w.give(w.root, admin, Role::Admin).await;
    let host = w.thing("host", ada, w.root, "gpu").await;
    // The owner names a manager of the page, who also uses the host: nobody names more than they hold.
    w.change_thing(
        ada,
        "host",
        host,
        json!({ "rows": [{ "permission": "access.manage",
                           "add": [{ "holder": { "kind": "user", "id": mia } }] },
                         { "permission": "host.use",
                           "add": [{ "holder": { "kind": "user", "id": mia } }] }] }),
    )
    .await
    .unwrap();
    // She changes the page like anybody who may…
    w.change_thing(
        mia,
        "host",
        host,
        json!({ "rows": [{ "permission": "host.use",
                           "add": [{ "holder": { "kind": "user", "id": bea } }] }] }),
    )
    .await
    .unwrap();
    // …and cannot name another manager, nor change who owns it.
    let named = w
        .change_thing(
            mia,
            "host",
            host,
            json!({ "rows": [{ "permission": "access.manage",
                               "add": [{ "holder": { "kind": "user", "id": zed } }] }] }),
        )
        .await;
    assert!(matches!(named, Err(ChangeError::Denied(_))));
    let owners = w
        .change_thing(
            mia,
            "host",
            host,
            json!({ "owners": { "add": [{ "kind": "user", "id": zed }] } }),
        )
        .await;
    assert!(matches!(owners, Err(ChangeError::Denied(_))));
    // An admin of the home does both, and is no owner.
    w.change_thing_as_admin(
        admin,
        "host",
        host,
        json!({ "rows": [{ "permission": "access.manage",
                           "add": [{ "holder": { "kind": "user", "id": zed } }] }],
                "owners": { "add": [{ "kind": "user", "id": zed }] } }),
    )
    .await
    .unwrap();
    let page = w.read_thing(zed, "host", host).await;
    assert_eq!(page.owners.len(), 2, "ada and zed");
    // Somebody who is none of them changes nothing, and is told who can.
    let stranger = w.member("stranger").await;
    let refused = w
        .change_thing(stranger, "host", host, json!({ "whole": "reset" }))
        .await;
    let Err(ChangeError::Denied(refusal)) = refused else {
        panic!("{refused:?}");
    };
    assert_eq!(refusal.required, P::AccessManage);
    assert!(refusal.missing[0].ask.contains(&ada));
}

#[tokio::test]
async fn somebody_from_outside_the_root_org_is_named_only_by_whom_the_policy_or_the_row_lets() {
    let w = World::new().await;
    let (ada, developer) = (w.member("ada").await, w.member("dev").await);
    let olga = w.user("olga").await;
    w.give(w.root, developer, Role::Developer).await;
    let provider = w.provider(ada, w.root, "Team Pro").await;
    // The page names the developer as one who may change it, and who uses the provider they open to others.
    w.change_thing(
        ada,
        "provider",
        provider,
        json!({ "rows": [{ "permission": "access.manage",
                           "add": [{ "holder": { "kind": "user", "id": developer } }] },
                         { "permission": "provider.use",
                           "add": [{ "holder": { "kind": "user", "id": developer } }] }] }),
    )
    .await
    .unwrap();
    let naming = json!({ "rows": [{ "permission": "provider.use",
                                    "add": [{ "holder": { "kind": "user", "id": olga } }] }] });
    // The owner may where the policy lets owners, and not otherwise, and is told who can.
    let refused = w
        .change_thing(ada, "provider", provider, naming.clone())
        .await;
    let Err(ChangeError::Outside { ask, .. }) = refused else {
        panic!("{refused:?}");
    };
    assert!(
        !ask.is_empty(),
        "somebody holds the right to name an outsider"
    );
    let lets = ResourcePolicy {
        owners_name_outsiders: true,
        ..ResourcePolicy::default()
    };
    // A Developer of the home holds the row `access.external` as shipped, whatever the policy says.
    w.change_thing_as(
        Caller::session(developer),
        "provider",
        provider,
        naming.clone(),
        &ResourcePolicy::default(),
    )
    .await
    .unwrap();
    let other = w.user("otto").await;
    let by_owner = json!({ "rows": [{ "permission": "provider.use",
                                       "add": [{ "holder": { "kind": "user", "id": other } }] }] });
    w.change_thing_as(Caller::session(ada), "provider", provider, by_owner, &lets)
        .await
        .unwrap();
    // After that it is access like anybody's: wherever they work, they use it.
    assert!(matches!(
        w.usable(Caller::session(olga), provider, None).await,
        Usable::Yes(_)
    ));
    // The row that takes is named by an admin of the place alone.
    let naming_the_row = json!({ "rows": [{ "permission": "access.external",
                                             "add": [{ "holder": { "kind": "user", "id": developer } }] }] });
    let refused = w
        .change_thing(ada, "provider", provider, naming_the_row.clone())
        .await;
    assert!(
        matches!(refused, Err(ChangeError::Denied(_))),
        "an owner does not hold the row, and does not give it"
    );
    let admin = w.member("admin").await;
    w.give(w.root, admin, Role::Admin).await;
    w.change_thing_as_admin(admin, "provider", provider, naming_the_row)
        .await
        .unwrap();
}

#[tokio::test]
async fn an_entry_in_the_use_row_of_a_subscription_kept_to_its_owner_is_refused() {
    let w = World::new().await;
    let (ada, bea) = (w.member("ada").await, w.member("bea").await);
    let subscription = {
        let mut conn = w.conn().await;
        orch_db::queries::resources::access_fixtures::insert_subscription(
            &mut conn, ada, w.root, "Plan", "codex",
        )
        .await
        .unwrap()
    };
    {
        use orch_db::queries::access::{PageKey, testing::add_entry};
        let mut conn = w.conn().await;
        add_entry(
            &mut conn,
            &PageKey::new("provider", subscription, "live"),
            "any",
            "access.own",
            ("user", Some(ada), None),
            None,
            w.now.get(),
        )
        .await
        .unwrap();
    }
    let kept = ResourcePolicy {
        shared_subscriptions: Some(vec!["claude".to_owned()]),
        ..ResourcePolicy::default()
    };
    let naming = json!({ "rows": [{ "permission": "provider.use",
                                    "add": [{ "holder": { "kind": "user", "id": bea } }] }] });
    let refused = w
        .change_thing_as(
            Caller::session(ada),
            "provider",
            subscription,
            naming.clone(),
            &kept,
        )
        .await;
    assert!(matches!(refused, Err(ChangeError::SubscriptionNotShared)));
    // Looking is not using, and a key is not touched by the policy.
    w.change_thing_as(
        Caller::session(ada),
        "provider",
        subscription,
        json!({ "rows": [{ "permission": "provider.view",
                           "add": [{ "holder": { "kind": "user", "id": bea } }] }] }),
        &kept,
    )
    .await
    .unwrap();
    let key = w.provider(ada, w.root, "Key").await;
    w.change_thing_as(Caller::session(ada), "provider", key, naming.clone(), &kept)
        .await
        .unwrap();
    // Where the kind is shared, the same entry is made.
    let shared = ResourcePolicy {
        shared_subscriptions: Some(vec!["codex".to_owned()]),
        ..ResourcePolicy::default()
    };
    w.change_thing_as(
        Caller::session(ada),
        "provider",
        subscription,
        naming,
        &shared,
    )
    .await
    .unwrap();
    // An entry that exists counts for nothing while the policy holds, and counts again when the
    // kind is back; the owner uses it either way.
    let policies = |policy: &ResourcePolicy| Policies::everywhere(policy.clone());
    let thread = w.thread;
    w.give(w.project, bea, Role::Interactor).await;
    w.give(w.project, ada, Role::Interactor).await;
    assert!(matches!(
        w.usable_under(Caller::session(bea), subscription, None, &policies(&shared))
            .await,
        Usable::Yes(_)
    ));
    assert!(matches!(
        w.usable_under(Caller::session(bea), subscription, None, &policies(&kept))
            .await,
        Usable::KeptToItsOwner
    ));
    assert!(matches!(
        w.usable_under(Caller::session(ada), subscription, None, &policies(&kept))
            .await,
        Usable::Yes(_)
    ));
    // Asked of a turn, the refusal says that nobody can give it, and no admin forces it.
    w.model(subscription, "big", &[]).await;
    let admin = w.member("admin").await;
    w.give(w.root, admin, Role::Admin).await;
    w.give(w.project, admin, Role::Interactor).await;
    let decision = w
        .turn_as(
            Caller::session(admin),
            thread,
            subscription,
            "big",
            None,
            &policies(&kept),
        )
        .await;
    let TurnDecision::Refused(refusal) = decision else {
        panic!("{decision:?}");
    };
    let entry = refusal
        .missing
        .iter()
        .find(|m| m.object.id == subscription)
        .unwrap();
    assert_eq!(entry.closed, Some(Closed::SubscriptionNotShared));
    assert!(entry.ask.is_empty() && !entry.r#override && !refusal.r#override);
}

#[tokio::test]
async fn a_resource_of_the_system_follows_nothing_names_everyone_and_is_the_system_admins() {
    let w = World::new().await;
    let sysadmin = w.user("root").await;
    {
        let mut conn = w.conn().await;
        orch_db::queries::access::testing::make_system_admin(&mut conn, sysadmin)
            .await
            .unwrap();
    }
    let (ada, olga) = (w.member("ada").await, w.user("olga").await);
    let template = w.system_thing("template", "base").await;
    // Native administration requires that this session selects it; tokens never acquire it.
    assert!(
        !w.can_on(sysadmin, "template", template, P::TemplateManage)
            .await
    );
    assert!(
        w.decide_on(
            Caller::session(sysadmin).with_admin_mode(true),
            "template",
            template,
            P::TemplateManage
        )
        .await
        .is_allowed()
    );
    let token = Caller::token(sysadmin, orch_authz::TokenGrant::everything());
    assert!(
        !w.decide_on(token, "template", template, P::TemplateManage)
            .await
            .is_allowed()
    );
    assert!(
        !w.can_on(ada, "template", template, P::TemplateView).await,
        "nobody else, for it is the system's"
    );
    // Its page is what was put on it: it follows nothing, and cannot be cut loose from nothing.
    let page = w
        .read_thing_as(
            Caller::session(sysadmin).with_admin_mode(true),
            "template",
            template,
        )
        .await;
    assert!(
        page.rows
            .iter()
            .all(|row| !row.inherit && row.inherited.is_none())
    );
    assert_eq!(page.state, PageState::Follows);
    let refused = w
        .change_thing_as_admin(
            sysadmin,
            "template",
            template,
            json!({ "whole": "private" }),
        )
        .await;
    assert!(matches!(refused, Err(ChangeError::Invalid(_))));
    // It may name everyone.
    w.change_thing_as_admin(
        sysadmin,
        "template",
        template,
        json!({ "rows": [{ "permission": "template.use", "add": [{ "holder": { "kind": "everyone" } }] }] }),
    )
    .await
    .unwrap();
    for person in [ada, olga] {
        assert!(
            matches!(
                w.usable(Caller::session(person), template, None).await,
                Usable::Yes(_)
            ),
            "everyone uses it"
        );
    }
    // A member cannot change it, and a resource that is not the system's cannot name everyone.
    let refused = w
        .change_thing(ada, "template", template, json!({ "rows": [{ "permission": "template.view",
                                                                    "add": [{ "holder": { "kind": "user", "id": ada } }] }] }))
        .await;
    assert!(matches!(refused, Err(ChangeError::Denied(_))));
    let own = w.thing("template", ada, w.root, "mine").await;
    let refused = w
        .change_thing(ada, "template", own, json!({ "rows": [{ "permission": "template.use",
                                                               "add": [{ "holder": { "kind": "everyone" } }] }] }))
        .await;
    assert!(matches!(refused, Err(ChangeError::Invalid(_))));
}

#[tokio::test]
async fn the_first_owner_is_changed_by_the_ordinary_change_and_audited_as_a_transfer_of_the_resource()
 {
    let w = World::new().await;
    let (ada, bob) = (w.member("ada").await, w.member("bob").await);
    let mount = w.thing("mount", ada, w.root, "data").await;
    w.change_thing(ada, "mount", mount, json!({ "owners": { "first": bob } }))
        .await
        .unwrap();
    let page = w.read_thing(ada, "mount", mount).await;
    assert_eq!(page.first_owner, Some(bob));
    assert_eq!(
        page.owners.len(),
        2,
        "the person who was first stays an owner"
    );
    let transferred = w
        .audit()
        .await
        .into_iter()
        .find(|row| row.action == "resource.transferred")
        .unwrap();
    assert_eq!(transferred.object_kind, "mount");
    assert_eq!(transferred.object_id, Some(mount));
    assert_eq!(transferred.details["to"], bob.to_string());
    // The last person is not taken out.
    let page = w.read_thing(bob, "mount", mount).await;
    let both: Vec<_> = page.owners.iter().map(|entry| entry.id).collect();
    let refused = w
        .change_thing(bob, "mount", mount, json!({ "owners": { "remove": both } }))
        .await;
    assert!(matches!(refused, Err(ChangeError::LastOwner)));
    // Somebody who is no member of the root org is not made an owner.
    let olga = w.user("olga").await;
    let refused = w
        .change_thing(
            bob,
            "mount",
            mount,
            json!({ "owners": { "add": [{ "kind": "user", "id": olga }] } }),
        )
        .await;
    assert!(matches!(refused, Err(ChangeError::Outside { .. })));
}

#[tokio::test]
async fn a_source_bound_provider_keeps_its_registering_machines_owner_first() {
    let w = World::new().await;
    let (ada, bob) = (a_machine_owner(&w, "ada").await, w.member("bob").await);
    let provider = w.provider(ada, w.root, "Plan").await;
    {
        let mut conn = w.conn().await;
        orch_db::queries::resources::access_fixtures::register_from_a_machine(
            &mut conn, "provider", provider, ada,
        )
        .await
        .unwrap();
    }
    let refused = w
        .change_thing(
            ada,
            "provider",
            provider,
            json!({ "owners": { "first": bob } }),
        )
        .await;
    assert!(matches!(refused, Err(ChangeError::FirstOwnerStays { owner }) if owner == ada));
    // Further owners are named like on anything.
    w.change_thing(
        ada,
        "provider",
        provider,
        json!({ "owners": { "add": [{ "kind": "user", "id": bob }] } }),
    )
    .await
    .unwrap();
    let page = w.read_thing(ada, "provider", provider).await;
    assert_eq!(page.first_owner, Some(ada));
    assert_eq!(page.owners.len(), 2);
}

#[tokio::test]
async fn the_defaults_of_the_home_feed_the_pages_of_its_resources_until_a_row_is_cut_loose() {
    let w = World::new().await;
    let (ada, bea) = (w.member("ada").await, w.member("bea").await);
    let template = w.thing("template", ada, w.root, "recipe").await;
    assert!(
        !w.can_on(bea, "template", template, P::TemplateUse).await,
        "a new one is its owners' alone"
    );
    // The org says that its members use the templates made in it.
    w.name_default(
        Some(w.root),
        P::TemplateUse,
        "any",
        Holder::Role {
            role: Role::Viewer,
            at: None,
        },
    )
    .await;
    w.give(w.root, bea, Role::Viewer).await;
    assert!(
        w.can_on(bea, "template", template, P::TemplateUse).await,
        "it follows"
    );
    assert!(
        w.can_on(bea, "template", template, P::TemplateView).await,
        "use holds looking"
    );
    assert!(!w.can_on(bea, "template", template, P::TemplateManage).await);
    // Cut loose, the row is held by its owners and by whom it names.
    w.change_thing(
        ada,
        "template",
        template,
        json!({ "rows": [{ "permission": "template.use", "inherit": false }] }),
    )
    .await
    .unwrap();
    assert!(!w.can_on(bea, "template", template, P::TemplateUse).await);
    // A lock at the home decides, whatever the page stored.
    w.default_row(w.root, P::TemplateUse, "any", true, true)
        .await;
    assert!(
        w.can_on(bea, "template", template, P::TemplateUse).await,
        "the lock holds the row to the default"
    );
    let refused = w
        .change_thing(ada, "template", template, json!({ "rows": [{ "permission": "template.use",
                                                                    "add": [{ "holder": { "kind": "user", "id": ada } }] }] }))
        .await;
    assert!(
        matches!(refused, Err(ChangeError::Locked { by: PlaceRef::Node(place), .. }) if place == w.root)
    );
}

#[tokio::test]
async fn a_group_in_the_owners_row_makes_its_members_owners_of_the_resource() {
    let w = World::new().await;
    let (ada, cyd) = (w.member("ada").await, w.member("cyd").await);
    w.give(w.root, ada, Role::Viewer).await;
    let group = w.group(w.root, "crew", &[cyd]).await;
    let provider = w.provider(ada, w.root, "Team Pro").await;
    assert!(!w.can_on(cyd, "provider", provider, P::ProviderManage).await);
    w.change_thing(
        ada,
        "provider",
        provider,
        json!({ "owners": { "add": [{ "kind": "group", "id": group }] } }),
    )
    .await
    .unwrap();
    assert!(
        w.can_on(cyd, "provider", provider, P::ProviderManage).await,
        "a member of the group owns it"
    );
    assert!(matches!(
        w.usable(Caller::session(cyd), provider, None).await,
        Usable::Yes(_)
    ));
}

// Machine registrations need a real personal home as enrollment supplies, not a bare identity row.
async fn a_machine_owner(w: &World, name: &str) -> orch_types::Id {
    let mut conn = w.conn().await;
    let owner =
        orch_db::queries::resources::testing::insert_person_with_home(&mut conn, name, w.now.get())
            .await
            .unwrap();
    orch_db::queries::access::testing::insert_member(&mut conn, w.root, owner, w.root)
        .await
        .unwrap();
    owner
}

#[tokio::test]
async fn a_directory_environment_has_independent_owners_while_its_machine_owner_keeps_management() {
    let w = World::new().await;
    let (ada, bob) = (a_machine_owner(&w, "ada").await, w.member("bob").await);
    let environment = w.thing("environment", ada, w.topic, "work").await;
    let mut conn = w.conn().await;
    orch_db::queries::resources::access_fixtures::register_from_a_machine(
        &mut conn,
        "environment",
        environment,
        ada,
    )
    .await
    .unwrap();
    drop(conn);
    w.change_thing(
        ada,
        "environment",
        environment,
        json!({"owners":{"first":bob}}),
    )
    .await
    .unwrap();
    let page = w.read_thing(bob, "environment", environment).await;
    let former = page
        .owners
        .iter()
        .find(|entry| entry.holder == Holder::User { id: ada })
        .unwrap()
        .id;
    w.change_thing(
        bob,
        "environment",
        environment,
        json!({"owners":{"remove":[former]}}),
    )
    .await
    .unwrap();
    let page = w.read_thing(bob, "environment", environment).await;
    assert_eq!(page.first_owner, Some(bob));
    assert_eq!(page.owners.len(), 1);
    assert_eq!(page.owners[0].holder, Holder::User { id: bob });
    assert!(
        w.can_on(ada, "environment", environment, P::EnvironmentManage)
            .await,
        "the physical machine's owner retains management independently of the page's owners"
    );
    assert!(
        w.can_on(bob, "environment", environment, P::EnvironmentManage)
            .await
    );
}
