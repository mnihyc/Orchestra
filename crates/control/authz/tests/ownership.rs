//! Who makes what and where, whose it is, how it passes from one person to another, and what becomes of a person's things when their membership ends.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use std::collections::HashMap;

use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{Permission as P, ResourceKind};
use orch_access_contract::roles::Role;
use orch_authz::ownership::{
    self, Basis, Disposal, EndError, Fate, FollowUp, ForkPage, Making, NewAccess, Restoring, Why,
};
use orch_authz::pages::ChangeError;
use orch_authz::usable::{ResourcePolicy, Usable};
use orch_authz::{Caller, Policy};
use orch_types::Id;
use support::World;
use support::things::object;

const FIVE: [ResourceKind; 5] = [
    ResourceKind::Provider,
    ResourceKind::Host,
    ResourceKind::Template,
    ResourceKind::McpServer,
    ResourceKind::Mount,
];

async fn making(
    w: &World,
    who: Id,
    kind: ResourceKind,
    home: Option<Id>,
    listed: &[ResourceKind],
) -> Making {
    making_as(w, Caller::session(who), kind, home, listed).await
}

async fn making_as(
    w: &World,
    caller: Caller,
    kind: ResourceKind,
    home: Option<Id>,
    listed: &[ResourceKind],
) -> Making {
    let principal = w.principal(caller).await;
    let mut conn = w.conn().await;
    ownership::may_make(&mut conn, &principal, kind, home, listed, Policy::default())
        .await
        .unwrap()
}

#[tokio::test]
async fn a_member_makes_each_kind_in_their_domain_or_inside_it_and_an_admin_makes_any_kind_where_the_rules_are_theirs()
 {
    let w = World::new().await;
    let d = w.deep().await;
    let (root_member, deep_member, admin) = (
        w.member("root").await,
        w.user("deep").await,
        w.member("admin").await,
    );
    {
        let mut conn = w.conn().await;
        orch_db::queries::access::testing::insert_member(&mut conn, w.root, deep_member, d.d2)
            .await
            .unwrap();
    }
    w.give(w.root, admin, Role::Admin).await;
    for kind in FIVE {
        // In the root org, for a member whose domain it is, and in an org three levels down.
        assert!(
            matches!(
                making(&w, root_member, kind, Some(w.root), &FIVE).await,
                Making::Allowed(Basis::Member)
            ),
            "{kind}"
        );
        assert!(
            matches!(
                making(&w, root_member, kind, Some(d.d3), &FIVE).await,
                Making::Allowed(Basis::Member)
            ),
            "{kind} three down"
        );
        // A member whose domain is ml makes in ml and below, and not in its sibling or above it.
        assert!(matches!(
            making(&w, deep_member, kind, Some(d.d2), &FIVE).await,
            Making::Allowed(Basis::Member)
        ));
        assert!(matches!(
            making(&w, deep_member, kind, Some(d.d3), &FIVE).await,
            Making::Allowed(Basis::Member)
        ));
        assert!(matches!(
            making(&w, deep_member, kind, Some(d.sales), &FIVE).await,
            Making::Refused(Why::OutsideYourDomain { domain }) if domain == d.d2
        ));
        // Refused where the policy leaves the kind out, and told what is lacking.
        let Making::Refused(Why::KeptByAdmins(refusal)) =
            making(&w, root_member, kind, Some(w.root), &[]).await
        else {
            panic!("{kind} is kept by the admins");
        };
        assert_eq!(refusal.required, P::SettingsManage);
        assert!(
            !refusal.missing[0].ask.is_empty(),
            "somebody makes the org's rules"
        );
        // Whoever makes the rules makes it all the same.
        assert!(matches!(
            making(&w, admin, kind, Some(w.root), &[]).await,
            Making::Allowed(Basis::Rules(_))
        ));
    }
    // An agent profile is refused to a member, and made by whoever makes the rules.
    assert!(matches!(
        making(&w, root_member, ResourceKind::Agent, Some(w.root), &FIVE).await,
        Making::Refused(Why::KeptByAdmins(_))
    ));
    assert!(matches!(
        making(&w, admin, ResourceKind::Agent, Some(d.d2), &FIVE).await,
        Making::Allowed(Basis::Rules(_))
    ));
    // Nobody who is no member of the tree makes what would not be theirs.
    let stranger = w.user("stranger").await;
    assert!(matches!(
        making(&w, stranger, ResourceKind::Mount, Some(w.root), &FIVE).await,
        Making::Refused(Why::NotAMember)
    ));
    // A provider always has a home, and an environment is made for a node.
    assert!(matches!(
        making(&w, root_member, ResourceKind::Provider, None, &FIVE).await,
        Making::Refused(Why::NotMadeThisWay)
    ));
    assert!(matches!(
        making(
            &w,
            root_member,
            ResourceKind::Environment,
            Some(w.root),
            &FIVE
        )
        .await,
        Making::Refused(Why::NotMadeThisWay)
    ));
    // The home is an org.
    assert!(matches!(
        making(&w, root_member, ResourceKind::Mount, Some(w.project), &FIVE).await,
        Making::Refused(Why::HomeIsNoOrg)
    ));
}

#[tokio::test]
async fn a_resource_of_the_system_is_made_by_a_system_admin_alone() {
    let w = World::new().await;
    let (sysadmin, member) = (w.user("root").await, w.member("member").await);
    {
        let mut conn = w.conn().await;
        orch_db::queries::access::testing::make_system_admin(&mut conn, sysadmin)
            .await
            .unwrap();
    }
    for kind in [
        ResourceKind::Host,
        ResourceKind::Template,
        ResourceKind::McpServer,
        ResourceKind::Mount,
        ResourceKind::Agent,
    ] {
        assert!(
            matches!(
                making_as(
                    &w,
                    Caller::session(sysadmin).with_admin_mode(false),
                    kind,
                    None,
                    &FIVE
                )
                .await,
                Making::Refused(Why::SystemAdminsOnly)
            ),
            "administrative creation is not available with As admin Off: {kind}"
        );
        assert!(
            matches!(
                making_as(
                    &w,
                    Caller::session(sysadmin).with_admin_mode(true),
                    kind,
                    None,
                    &FIVE
                )
                .await,
                Making::Allowed(Basis::SystemAdmin)
            ),
            "{kind}"
        );
        assert!(
            matches!(
                making(&w, member, kind, None, &FIVE).await,
                Making::Refused(Why::SystemAdminsOnly)
            ),
            "{kind}"
        );
    }
    // With an API token a system admin holds none of it.
    let principal = w
        .principal(Caller::token(
            sysadmin,
            orch_authz::TokenGrant::everything(),
        ))
        .await;
    let mut conn = w.conn().await;
    let made = ownership::may_make(
        &mut conn,
        &principal,
        ResourceKind::Host,
        None,
        &FIVE,
        Policy::default(),
    )
    .await
    .unwrap();
    assert!(matches!(made, Making::Refused(Why::SystemAdminsOnly)));
}

#[tokio::test]
async fn a_new_resource_is_its_makers_alone_and_open_to_its_home_only_when_its_maker_said_so() {
    let w = World::new().await;
    let (ada, bea) = (w.member("ada").await, w.member("bea").await);
    w.give(w.root, bea, Role::Viewer).await;
    // The org says nothing about providers: a new one is its owner's alone, however it is made.
    for (choice, bea_uses) in [
        (NewAccess::Defaults, false),
        (NewAccess::OnlyMe, false),
        (NewAccess::Home, true),
    ] {
        let id = {
            let mut conn = w.conn().await;
            orch_db::queries::resources::access_fixtures::insert_resource(
                &mut conn,
                "template",
                Some(ada),
                Some(w.root),
                "recipe",
            )
            .await
            .unwrap()
        };
        let mut conn = w.conn().await;
        ownership::start_resource(&mut conn, object("template", id), ada, choice, w.now)
            .await
            .unwrap();
        drop(conn);
        assert!(
            w.can_on(ada, "template", id, P::TemplateManage).await,
            "its maker owns it"
        );
        assert_eq!(
            w.can_on(bea, "template", id, P::TemplateUse).await,
            bea_uses,
            "{choice:?}"
        );
    }
    // "Only me" is a row cut loose, so that a later default of the home does not open it.
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
    let only_me = {
        let mut conn = w.conn().await;
        let id = orch_db::queries::resources::access_fixtures::insert_resource(
            &mut conn,
            "template",
            Some(ada),
            Some(w.root),
            "private",
        )
        .await
        .unwrap();
        ownership::start_resource(
            &mut conn,
            object("template", id),
            ada,
            NewAccess::OnlyMe,
            w.now,
        )
        .await
        .unwrap();
        id
    };
    let follows = {
        let mut conn = w.conn().await;
        let id = orch_db::queries::resources::access_fixtures::insert_resource(
            &mut conn,
            "template",
            Some(ada),
            Some(w.root),
            "open",
        )
        .await
        .unwrap();
        ownership::start_resource(
            &mut conn,
            object("template", id),
            ada,
            NewAccess::Defaults,
            w.now,
        )
        .await
        .unwrap();
        id
    };
    assert!(!w.can_on(bea, "template", only_me, P::TemplateUse).await);
    assert!(w.can_on(bea, "template", follows, P::TemplateUse).await);
}

#[tokio::test]
async fn whoever_makes_a_resource_holds_all_of_it_and_uses_it_without_being_named_in_its_use_row() {
    let w = World::new().await;
    let (ada, bea) = (w.member("ada").await, w.member("bea").await);
    for (kind, view, using, manage) in [
        (
            "provider",
            P::ProviderView,
            P::ProviderUse,
            P::ProviderManage,
        ),
        ("agent", P::AgentView, P::AgentUse, P::AgentManage),
        ("host", P::HostView, P::HostUse, P::HostManage),
        (
            "template",
            P::TemplateView,
            P::TemplateUse,
            P::TemplateManage,
        ),
        ("mcp_server", P::McpView, P::McpUse, P::McpManage),
        ("mount", P::MountView, P::MountUse, P::MountManage),
    ] {
        let id = {
            let mut conn = w.conn().await;
            let id = orch_db::queries::resources::access_fixtures::insert_resource(
                &mut conn,
                kind,
                Some(ada),
                Some(w.root),
                "recipe",
            )
            .await
            .unwrap();
            ownership::start_resource(&mut conn, object(kind, id), ada, NewAccess::Defaults, w.now)
                .await
                .unwrap();
            id
        };
        for level in [view, using, manage] {
            assert!(
                w.can_on(ada, kind, id, level).await,
                "{kind}: its maker holds {level}"
            );
        }
        assert!(
            !w.can_on(bea, kind, id, using).await,
            "{kind}: as shipped, a new one is its owner's alone"
        );
    }
}

#[tokio::test]
async fn the_provider_a_member_made_runs_turns_for_its_maker_and_for_nobody_else() {
    let w = World::new().await;
    let (ada, bea) = (w.member("ada").await, w.member("bea").await);
    let provider = {
        let mut conn = w.conn().await;
        let id = orch_db::queries::resources::access_fixtures::insert_resource(
            &mut conn,
            "provider",
            Some(ada),
            Some(w.root),
            "mine",
        )
        .await
        .unwrap();
        ownership::start_resource(
            &mut conn,
            object("provider", id),
            ada,
            NewAccess::Defaults,
            w.now,
        )
        .await
        .unwrap();
        id
    };
    assert!(matches!(
        w.usable(Caller::session(ada), provider, None).await,
        Usable::Yes(_)
    ));
    assert!(!matches!(
        w.usable(Caller::session(bea), provider, None).await,
        Usable::Yes(_)
    ));
}

#[tokio::test]
async fn a_host_of_the_system_is_used_by_every_user_and_managed_by_system_admins_alone() {
    let w = World::new().await;
    let (sysadmin, member, outsider) = (
        w.user("root").await,
        w.member("member").await,
        w.user("outsider").await,
    );
    {
        let mut conn = w.conn().await;
        orch_db::queries::access::testing::make_system_admin(&mut conn, sysadmin)
            .await
            .unwrap();
    }
    // Enrolled with no page, a host of the system is a system admin's and nobody else's.
    let bare = w.system_thing("host", "bare").await;
    assert!(
        !w.decide_on(
            Caller::session(sysadmin).with_admin_mode(false),
            "host",
            bare,
            P::HostUse
        )
        .await
        .is_allowed()
    );
    assert!(
        w.decide_on(
            Caller::session(sysadmin).with_admin_mode(true),
            "host",
            bare,
            P::HostUse
        )
        .await
        .is_allowed()
    );
    assert!(!w.can_on(member, "host", bare, P::HostUse).await);
    // With the page it is made with, everyone uses it, and only system admins manage it.
    let bundled = w.system_thing("host", "bundled").await;
    {
        let mut conn = w.conn().await;
        ownership::start_resource(
            &mut conn,
            object("host", bundled),
            sysadmin,
            NewAccess::Everyone,
            w.now,
        )
        .await
        .unwrap();
    }
    for person in [member, outsider] {
        assert!(w.can_on(person, "host", bundled, P::HostView).await);
        assert!(w.can_on(person, "host", bundled, P::HostUse).await);
        assert!(!w.can_on(person, "host", bundled, P::HostManage).await);
    }
    assert!(
        !w.decide_on(
            Caller::session(sysadmin).with_admin_mode(false),
            "host",
            bundled,
            P::HostManage
        )
        .await
        .is_allowed()
    );
    assert!(
        w.decide_on(
            Caller::session(sysadmin).with_admin_mode(true),
            "host",
            bundled,
            P::HostManage
        )
        .await
        .is_allowed()
    );
    assert!(matches!(
        w.usable(Caller::session(member), bundled, None).await,
        Usable::Yes(_)
    ));
    // Its first owner is the person who made it, alone in the row.
    let owners: Vec<Option<Id>> = sqlx::query_scalar(
        "SELECT holder_id FROM permission_entries
          WHERE object_kind = 'host' AND object_id = $1 AND page = 'live' AND permission = 'access.own'",
    )
    .bind(bundled)
    .fetch_all(&w.db.pool)
    .await
    .unwrap();
    assert_eq!(owners, [Some(sysadmin)]);
}

#[tokio::test]
async fn a_second_owner_is_named_by_an_owner_or_an_admin_and_holds_all_but_naming_outsiders() {
    let w = World::new().await;
    let (ada, bob, cy) = (
        w.member("ada").await,
        w.member("bob").await,
        w.member("cyd").await,
    );
    let admin = w.member("admin").await;
    w.give(w.root, admin, Role::Admin).await;
    let provider = w.provider(ada, w.root, "Team Pro").await;
    let policy = ResourcePolicy::default();
    let origin = orch_db::audit::Origin::default();
    // By an owner, with the person who was first staying an owner.
    let principal = w.principal(Caller::session(ada)).await;
    {
        let mut tx = w.db.pool.begin().await.unwrap();
        ownership::transfer(
            &mut tx,
            &principal,
            object("provider", provider),
            bob,
            true,
            &policy,
            &origin,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let page = w.read_thing(bob, "provider", provider).await;
    assert_eq!(page.first_owner, Some(bob));
    assert_eq!(page.owners.len(), 2);
    // Bob holds everything on the page but naming outsiders, and his address follows.
    for permission in [P::ProviderManage, P::ProviderUse, P::AccessManage] {
        assert!(w.can_on(bob, "provider", provider, permission).await);
    }
    assert!(!w.can_on(bob, "provider", provider, P::AccessExternal).await);
    // Off does not supply administrative authority over somebody else's private page.
    let ordinary = w
        .principal(Caller::session(admin).with_admin_mode(false))
        .await;
    {
        let mut tx = w.db.pool.begin().await.unwrap();
        let refused = ownership::transfer(
            &mut tx,
            &ordinary,
            object("provider", provider),
            cy,
            false,
            &policy,
            &origin,
        )
        .await;
        assert!(matches!(refused, Err(ChangeError::Denied(_))));
        tx.rollback().await.unwrap();
    }
    assert_eq!(
        w.read_thing(bob, "provider", provider).await.first_owner,
        Some(bob)
    );
    // By an admin of the home in the explicit mode, with the previous first owner removed.
    let principal = w
        .principal(Caller::session(admin).with_admin_mode(true))
        .await;
    {
        let mut tx = w.db.pool.begin().await.unwrap();
        ownership::transfer(
            &mut tx,
            &principal,
            object("provider", provider),
            cy,
            false,
            &policy,
            &origin,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let page = w.read_thing(cy, "provider", provider).await;
    assert_eq!(page.first_owner, Some(cy));
    let holders: Vec<_> = page.owners.iter().map(|entry| entry.holder).collect();
    assert!(
        holders.contains(&Holder::User { id: cy }) && holders.contains(&Holder::User { id: ada })
    );
    assert!(
        !holders.contains(&Holder::User { id: bob }),
        "bob was first and was taken out"
    );
    // Somebody who is no owner and no admin cannot, and an outsider is not made an owner.
    let stranger = w.member("stranger").await;
    let principal = w.principal(Caller::session(stranger)).await;
    let mut tx = w.db.pool.begin().await.unwrap();
    let refused = ownership::transfer(
        &mut tx,
        &principal,
        object("provider", provider),
        stranger,
        true,
        &policy,
        &origin,
    )
    .await;
    assert!(matches!(refused, Err(ChangeError::Denied(_))));
    drop(tx);
    let olga = w.user("olga").await;
    let principal = w
        .principal(Caller::session(admin).with_admin_mode(true))
        .await;
    let mut tx = w.db.pool.begin().await.unwrap();
    let refused = ownership::transfer(
        &mut tx,
        &principal,
        object("provider", provider),
        olga,
        true,
        &policy,
        &origin,
    )
    .await;
    assert!(
        matches!(refused, Err(ChangeError::Outside { .. })),
        "an owner is a member of the root org"
    );
}

/// A tree in which `lee` leaves: the department Research is first owned by `dana`, and the root
/// org by `boss`; `lee` owns things there of every kind.
struct Leaving {
    w: World,
    lee: Id,
    dana: Id,
    cy: Id,
    research: Id,
    thread: Id,
    shared_topic: Id,
    provider: Id,
    template: Id,
    environment: Id,
    quiet: Id,
}

async fn leaving() -> Leaving {
    let w = World::new().await;
    let (lee, dana, cy) = (
        w.member("lee").await,
        w.member("dana").await,
        w.member("cyd").await,
    );
    let research = w.node_owned_by("org", w.root, "Research", dana).await;
    let project = w.node("project", research, "Web").await;
    let topic = w.node("topic", project, "Login").await;
    let thread = w.node_owned_by("thread", topic, "Fix", lee).await;
    let shared_topic = w.node_owned_by("topic", project, "Shared", lee).await;
    {
        let mut conn = w.conn().await;
        orch_db::queries::access::testing::name_owner(
            &mut conn,
            shared_topic,
            ("user", cy),
            w.now.get(),
        )
        .await
        .unwrap();
    }
    let quiet = w.node_owned_by("thread", topic, "Quiet", lee).await;
    w.row(quiet, P::ThreadView, false).await;
    let provider = w.provider(lee, research, "Plan").await;
    let template = w.thing("template", lee, research, "recipe").await;
    let environment = w.thing("environment", lee, topic, "laptop").await;
    {
        let mut conn = w.conn().await;
        // This is the leaver's actual machine, registered from its independently owned home.
        // Bare authz users have no personal home; the old convenience helper left owner_id NULL.
        let source_home = orch_types::new_id();
        orch_db::queries::hierarchy::testing::insert_node(
            &mut conn,
            source_home,
            "org",
            None,
            "Machine home",
            lee,
        )
        .await
        .unwrap();
        orch_db::queries::access::testing::insert_member(&mut conn, source_home, lee, source_home)
            .await
            .unwrap();
        orch_authz::pages::start(
            &mut conn,
            orch_access_contract::pages::StoredKind::Node,
            source_home,
            lee,
            w.now,
        )
        .await
        .unwrap();
        let machine = orch_db::queries::resources::access_fixtures::insert_resource(
            &mut conn,
            "machine",
            Some(lee),
            Some(source_home),
            "laptop",
        )
        .await
        .unwrap();
        ownership::start_resource(
            &mut conn,
            object("machine", machine),
            lee,
            NewAccess::Defaults,
            w.now,
        )
        .await
        .unwrap();
        orch_db::queries::resources::access_fixtures::register_on(
            &mut conn,
            "environment",
            environment,
            machine,
        )
        .await
        .unwrap();
        let facts = orch_db::queries::resources::resource_facts(&mut conn, &[environment])
            .await
            .unwrap();
        assert_eq!(
            facts[0].machine_owner_id,
            Some(lee),
            "the unregister premise is physical ownership, not environment attribution"
        );
    }
    Leaving {
        w,
        lee,
        dana,
        cy,
        research,
        thread,
        shared_topic,
        provider,
        template,
        environment,
        quiet,
    }
}

#[tokio::test]
async fn what_they_own_alone_goes_to_the_first_owner_of_the_org_it_lies_in_and_what_they_share_to_the_others()
 {
    let l = leaving().await;
    let principal = l.w.principal(Caller::session(l.w.boss)).await;
    let mut conn = l.w.conn().await;
    let plan = ownership::plan_leaving(&mut conn, &principal, l.w.root, l.lee, Policy::default())
        .await
        .unwrap();
    let fate = |id: Id| {
        plan.things
            .iter()
            .find(|thing| thing.object.id == id)
            .map(|thing| thing.fate.clone())
    };
    // A thread of acme.research.web goes to the first owner of research.
    assert_eq!(
        fate(l.thread),
        Some(Fate::PassedOn {
            org: l.research,
            owner: l.dana
        })
    );
    assert_eq!(fate(l.shared_topic), Some(Fate::LeftToOthers { others: 1 }));
    assert_eq!(
        fate(l.provider),
        Some(Fate::PassedOn {
            org: l.research,
            owner: l.dana
        })
    );
    assert_eq!(
        fate(l.template),
        Some(Fate::PassedOn {
            org: l.research,
            owner: l.dana
        })
    );
    assert_eq!(
        fate(l.environment),
        Some(Fate::Unregistered),
        "it was on their own machine"
    );
    // The dialog counts apart what was restricted, and marks a managed resource that can be disposed of.
    let restricted: Vec<Id> = plan
        .things
        .iter()
        .filter(|thing| thing.restricted)
        .map(|thing| thing.object.id)
        .collect();
    assert_eq!(restricted, [l.quiet]);
    assert!(
        plan.things
            .iter()
            .find(|thing| thing.object.id == l.provider)
            .unwrap()
            .may_be_disposed_of
    );
    assert!(
        !plan
            .things
            .iter()
            .find(|thing| thing.object.id == l.thread)
            .unwrap()
            .may_be_disposed_of
    );
    assert!(plan.stuck().is_empty());
}

#[tokio::test]
async fn ending_a_membership_hands_things_over_in_one_transaction_with_a_line_for_each() {
    let l = leaving().await;
    // A person's name on somebody else's page in the tree goes too.
    let theirs = l.w.provider(l.dana, l.research, "Dana's").await;
    l.w.grant(
        "provider",
        theirs,
        P::ProviderUse,
        Holder::User { id: l.lee },
    )
    .await;
    let before = l.w.epoch().await;
    let admin_principal = l.w.principal(Caller::session(l.w.boss)).await;
    let mut choices = HashMap::new();
    choices.insert(l.provider, Disposal::Disable);
    choices.insert(l.template, Disposal::Remove);
    let ended = {
        let mut tx = l.w.db.pool.begin().await.unwrap();
        let ended = ownership::end_membership(
            &mut tx,
            &admin_principal,
            l.w.root,
            l.lee,
            &choices,
            Policy::default(),
            &orch_db::audit::Origin::default(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        ended
    };
    assert_eq!(l.w.epoch().await, before + 1, "the epoch moves once");
    assert_eq!(
        ended
            .follow_ups
            .iter()
            .cloned()
            .collect::<std::collections::HashSet<_>>(),
        [
            FollowUp::Unregister(object("environment", l.environment)),
            FollowUp::Disable(object("provider", l.provider)),
            FollowUp::Remove(object("template", l.template)),
        ]
        .into_iter()
        .collect(),
        "what needs a part that is not here comes back as data"
    );
    assert_eq!(
        ended.entries_taken, 1,
        "the entry that named lee on Dana's provider"
    );
    // The thread is Dana's now, alone, and its first owner; the shared topic is Cy's.
    let page =
        l.w.read(l.w.boss, orch_authz::pages::Target::Live(l.thread))
            .await;
    assert_eq!(page.first_owner, Some(l.dana));
    assert_eq!(page.owners.len(), 1);
    assert!(l.w.can(l.dana, l.thread, P::ThreadManage).await);
    assert!(
        !l.w.can(l.lee, l.thread, P::ThreadManage).await,
        "lee owns nothing there now"
    );
    let page =
        l.w.read(l.w.boss, orch_authz::pages::Target::Live(l.shared_topic))
            .await;
    assert_eq!(page.first_owner, Some(l.cy), "the one who is left is first");
    // The provider keeps working for whom its page names, under its new first owner.
    let page = l.w.read_thing(l.dana, "provider", l.provider).await;
    assert_eq!(page.first_owner, Some(l.dana));
    assert!(matches!(
        l.w.usable(Caller::session(l.dana), l.provider, None).await,
        Usable::Yes(_)
    ));
    // A line of the history of each, naming the thing.
    let audit = l.w.audit().await;
    assert!(
        audit
            .iter()
            .any(|row| row.action == "node.transferred" && row.object_id == Some(l.thread))
    );
    assert!(
        audit
            .iter()
            .any(|row| row.action == "resource.transferred" && row.object_id == Some(l.provider))
    );
    assert!(
        audit
            .iter()
            .filter(|row| row.action == "permission.removed" && row.history)
            .count()
            >= 4
    );
    let kinds: Vec<&str> = audit
        .iter()
        .filter(|row| row.action == "resource.transferred")
        .map(|row| row.object_kind.as_str())
        .collect();
    assert!(kinds.contains(&"provider") && kinds.contains(&"template"));
}

#[tokio::test]
async fn the_last_owner_of_a_root_org_cannot_leave_it_and_a_first_owner_who_leaves_passes_things_up()
 {
    let w = World::new().await;
    let nina = w.member("nina").await;
    let principal = w.principal(Caller::session(w.boss)).await;
    let mut tx = w.db.pool.begin().await.unwrap();
    // The boss is the sole owner of the root org Acme, the topic, the project.
    let plan = ownership::plan_leaving(&mut tx, &principal, w.root, w.boss, Policy::default())
        .await
        .unwrap();
    let root_thing = plan
        .things
        .iter()
        .find(|thing| thing.object.id == w.root)
        .unwrap();
    assert_eq!(
        root_thing.fate,
        Fate::Stuck,
        "the last owner of a root org names another first"
    );
    let refused = ownership::end_membership(
        &mut tx,
        &principal,
        w.root,
        w.boss,
        &HashMap::new(),
        Policy::default(),
        &orch_db::audit::Origin::default(),
    )
    .await;
    assert!(
        matches!(refused, Err(EndError::Stuck(stuck)) if stuck.iter().any(|thing| thing.id == w.root))
    );
    drop(tx);
    // Where the org's first owner is the one who leaves, the thing goes up to the org above.
    let dept = w.node_owned_by("org", w.root, "Dept", nina).await;
    let project = w.node_owned_by("project", dept, "Proj", nina).await;
    let principal = w.principal(Caller::session(w.boss)).await;
    let mut conn = w.conn().await;
    let plan = ownership::plan_leaving(&mut conn, &principal, w.root, nina, Policy::default())
        .await
        .unwrap();
    let fate = plan
        .things
        .iter()
        .find(|thing| thing.object.id == project)
        .unwrap()
        .fate
        .clone();
    assert_eq!(
        fate,
        Fate::PassedOn {
            org: w.root,
            owner: w.boss
        },
        "not to nina herself, who owns Dept"
    );
}

#[tokio::test]
async fn who_may_end_a_membership_and_who_may_restore_what_was_deleted() {
    let w = World::new().await;
    let d = w.deep().await;
    let (member, admin_above, admin_beside) = (
        w.user("mel").await,
        w.member("above").await,
        w.member("beside").await,
    );
    {
        let mut conn = w.conn().await;
        orch_db::queries::access::testing::insert_member(&mut conn, w.root, member, d.d2)
            .await
            .unwrap();
    }
    w.give(d.d1, admin_above, Role::Admin).await;
    w.give(d.sales, admin_beside, Role::Admin).await;
    let policy = Policy::default();
    let mut conn = w.conn().await;
    for (who, allowed) in [
        (admin_above, true),
        (admin_beside, false),
        (member, true),
        (w.maker, false),
    ] {
        let principal = w.principal(Caller::session(who)).await;
        let decision = ownership::may_end_membership(&mut conn, &principal, w.root, member, policy)
            .await
            .unwrap();
        assert_eq!(decision.is_allowed(), allowed, "{who}");
    }
    // A deleted topic is restored by whoever manages its project; a deleted root org by an owner or a system admin.
    let topic = w.node("topic", d.project, "Gone").await;
    let boss = w.principal(Caller::session(w.boss)).await;
    let nobody = w.principal(Caller::session(w.maker)).await;
    assert!(matches!(
        ownership::may_restore(&mut conn, &boss, topic, policy)
            .await
            .unwrap(),
        Restoring::NotDeleted
    ));
    orch_db::queries::hierarchy::testing::soft_delete(&mut conn, topic, w.boss, w.now.get())
        .await
        .unwrap();
    assert!(matches!(
        ownership::may_restore(&mut conn, &boss, topic, policy)
            .await
            .unwrap(),
        Restoring::Allowed
    ));
    assert!(matches!(
        ownership::may_restore(&mut conn, &nobody, topic, policy)
            .await
            .unwrap(),
        Restoring::Refused(_)
    ));
    let dead_root = {
        let id = orch_types::new_id();
        orch_db::queries::hierarchy::testing::insert_node(
            &mut conn,
            id,
            "org",
            None,
            "Dead Corp",
            w.maker,
        )
        .await
        .unwrap();
        orch_db::queries::access::testing::name_owner(
            &mut conn,
            id,
            ("user", w.maker),
            w.now.get(),
        )
        .await
        .unwrap();
        orch_db::queries::hierarchy::testing::soft_delete(&mut conn, id, w.maker, w.now.get())
            .await
            .unwrap();
        id
    };
    assert!(
        matches!(
            ownership::may_restore(&mut conn, &nobody, dead_root, policy)
                .await
                .unwrap(),
            Restoring::Allowed
        ),
        "its owner"
    );
    assert!(matches!(
        ownership::may_restore(&mut conn, &boss, dead_root, policy)
            .await
            .unwrap(),
        Restoring::OwnersAndSystemAdminsOnly
    ));
}

#[tokio::test]
async fn a_fork_is_its_makers_and_starts_with_the_page_of_its_source_or_with_the_defaults_of_where_it_is_made()
 {
    let w = World::new().await;
    let fork_maker = w.member("forker").await;
    let reader = w.member("reader").await;
    // The source has a row changed and a person named in it; its owner is the thread's maker.
    w.name(w.thread, P::ThreadView, Holder::User { id: reader })
        .await;
    w.row(w.thread, P::ThreadFork, false).await;
    let as_original = w
        .node_owned_by("thread", w.topic, "Fork 1", fork_maker)
        .await;
    let as_the_place = w
        .node_owned_by("thread", w.topic, "Fork 2", fork_maker)
        .await;
    // Fresh nodes: the helper above wrote their Owners row, so remove it and start them as a fork does.
    for fork in [as_original, as_the_place] {
        let mut conn = w.conn().await;
        sqlx::query("DELETE FROM permission_entries WHERE object_id = $1")
            .bind(fork)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let mut conn = w.conn().await;
    ownership::start_fork(
        &mut conn,
        w.thread,
        as_original,
        fork_maker,
        ForkPage::AsTheOriginal,
        w.now,
    )
    .await
    .unwrap();
    ownership::start_fork(
        &mut conn,
        w.thread,
        as_the_place,
        fork_maker,
        ForkPage::AsThePlaceDecides,
        w.now,
    )
    .await
    .unwrap();
    drop(conn);
    let original = w
        .read(fork_maker, orch_authz::pages::Target::Live(as_original))
        .await;
    let place = w
        .read(fork_maker, orch_authz::pages::Target::Live(as_the_place))
        .await;
    // The maker owns it, the source's owner does not.
    for page in [&original, &place] {
        assert_eq!(page.owners.len(), 1);
        assert_eq!(page.owners[0].holder, Holder::User { id: fork_maker });
    }
    let row = |page: &orch_authz::pages::PageView, permission: P| {
        page.rows
            .iter()
            .find(|row| row.permission == permission)
            .unwrap()
            .clone()
    };
    assert!(
        !row(&original, P::ThreadFork).inherit,
        "a row changed there is copied"
    );
    assert_eq!(
        row(&original, P::ThreadView).entries.len(),
        1,
        "and so is a person named in it"
    );
    assert!(
        row(&place, P::ThreadFork).inherit && row(&place, P::ThreadView).entries.is_empty(),
        "nothing is copied"
    );
    assert!(w.can(reader, as_original, P::ThreadView).await);
    assert!(
        !w.can(w.maker, as_original, P::ThreadManage).await,
        "the source's owner owns none of it"
    );
}

#[tokio::test]
async fn a_person_suspended_in_an_org_uses_nothing_there_and_their_things_go_on_serving_others() {
    use orch_db::queries::access::testing::{lift_suspension, suspend_in};
    let w = World::new().await;
    let d = w.deep().await;
    let (ada, bea) = (w.member("ada").await, w.member("bea").await);
    let in_sales = w.provider(ada, d.sales, "Sales Pro").await;
    let in_research = w.provider(ada, d.d1, "Research Pro").await;
    for provider in [in_sales, in_research] {
        w.grant(
            "provider",
            provider,
            P::ProviderUse,
            Holder::User { id: bea },
        )
        .await;
    }
    let uses = |who: Id, provider: Id| {
        let w = &w;
        async move {
            matches!(
                w.usable(Caller::session(who), provider, None).await,
                Usable::Yes(_)
            )
        }
    };
    assert!(uses(ada, in_sales).await && uses(ada, in_research).await);
    // Suspended in Sales alone: nothing there, everything held elsewhere.
    {
        let mut conn = w.conn().await;
        suspend_in(&mut conn, d.sales, ada, w.boss).await.unwrap();
    }
    assert!(!uses(ada, in_sales).await && uses(ada, in_research).await);
    assert!(
        uses(bea, in_sales).await,
        "what they own still serves others"
    );
    assert!(
        !w.decide_on(
            Caller::session(w.boss).with_admin_mode(false),
            "provider",
            in_sales,
            P::AccessManage
        )
        .await
        .is_allowed(),
        "Off retains ordinary page authority only"
    );
    assert!(
        w.decide_on(
            Caller::session(w.boss).with_admin_mode(true),
            "provider",
            in_sales,
            P::AccessManage
        )
        .await
        .is_allowed(),
        "and is administered by the admins of the org when acting as admin"
    );
    // Lifted: all of it is back.
    {
        let mut conn = w.conn().await;
        lift_suspension(&mut conn, d.sales, ada).await.unwrap();
    }
    assert!(uses(ada, in_sales).await);
    // Suspended in the root org: nothing in either department.
    {
        let mut conn = w.conn().await;
        suspend_in(&mut conn, w.root, ada, w.boss).await.unwrap();
    }
    assert!(!uses(ada, in_sales).await && !uses(ada, in_research).await);
    assert!(uses(bea, in_research).await);
    {
        let mut conn = w.conn().await;
        lift_suspension(&mut conn, w.root, ada).await.unwrap();
    }
    // Suspended altogether.
    {
        let mut conn = w.conn().await;
        sqlx::query("UPDATE users SET status = 'suspended' WHERE id = $1")
            .bind(ada)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    assert!(!uses(ada, in_sales).await && !uses(ada, in_research).await);
    assert!(uses(bea, in_sales).await);
}

#[tokio::test]
async fn the_address_follows_the_first_owner_when_another_is_made_first() {
    let w = World::new().await;
    let (ada, bea) = (w.member("ada").await, w.member("bea").await);
    let provider = w.provider(ada, w.root, "main").await;
    assert!(address_of(&w, provider).await.ends_with(".ada.main"));
    let principal = w.principal(Caller::session(ada)).await;
    let mut tx = w.db.pool.begin().await.unwrap();
    ownership::transfer(
        &mut tx,
        &principal,
        object("provider", provider),
        bea,
        true,
        &ResourcePolicy::default(),
        &orch_db::audit::Origin::default(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert!(address_of(&w, provider).await.ends_with(".bea.main"));
}

async fn address_of(w: &World, resource: Id) -> String {
    let mut conn = w.conn().await;
    orch_authz::resources::addresses(&mut conn, &[resource])
        .await
        .unwrap()
        .remove(0)
        .1
}
