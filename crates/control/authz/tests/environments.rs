//! Where a thread runs is its environment's managers' to say; what an environment is made from is the use of a host and a template within the entries that give it; the four doors of an environment let somebody in who does not manage it, and whoever reaches in is told by what; and a shell is its opener's alone.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::decisions::Decision;
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{ObjectKind, Permission as P, ResourceLimits};
use orch_access_contract::roles::Role;
use orch_authz::environments::{
    self, Creating, Limit, Moving, NewEnvironment, Place, Reaching, Shell, Wants,
};
use orch_authz::usable::Policies;
use orch_authz::{Caller, Policy};
use orch_types::Id;
use serde_json::json;
use support::World;

async fn moving(w: &World, who: Id, moving: Moving) -> Decision {
    let principal = w.principal(Caller::session(who)).await;
    let mut conn = w.conn().await;
    environments::check_move(&mut conn, &principal, &moving, Policy::default())
        .await
        .unwrap()
}

/// An environment made for the work of a topic, managed by the Developers there as shipped, and a
/// second one; a Developer, an Interactor, and the maker of the thread who owns it.
struct Setting {
    w: World,
    eve: Id,
    dev: Id,
    ivy: Id,
    env: Id,
    other: Id,
}

async fn setting() -> Setting {
    let w = World::new().await;
    let (eve, dev, ivy) = (
        w.member("eve").await,
        w.member("dev").await,
        w.member("ivy").await,
    );
    w.give(w.root, eve, Role::Viewer).await;
    w.give(w.project, dev, Role::Developer).await;
    w.give(w.project, ivy, Role::Interactor).await;
    let env = w.thing("environment", eve, w.topic, "main").await;
    let other = w.thing("environment", eve, w.topic, "second").await;
    w.put_in(w.thread, env).await;
    Setting {
        w,
        eve,
        dev,
        ivy,
        env,
        other,
    }
}

#[tokio::test]
async fn grouped_reach_counts_keep_nested_members_owners_and_each_objects_suspension_cut() {
    let s = setting().await;
    let inner = s.w.group(s.w.root, "Inside", &[s.dev, s.ivy]).await;
    let outer = s.w.group(s.w.root, "Outside", &[]).await;
    {
        let mut conn = s.w.conn().await;
        sqlx::query("INSERT INTO group_members(group_id,member_group_id) VALUES($1,$2)")
            .bind(outer)
            .bind(inner)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let mut ids = vec![s.env];
    for index in 0..10 {
        ids.push(
            s.w.thing("environment", s.eve, s.w.topic, &format!("Work {index}"))
                .await,
        );
    }
    for id in &ids {
        s.w.grant(
            "environment",
            *id,
            P::FilesView,
            Holder::Group { id: outer },
        )
        .await;
        s.w.grant(
            "environment",
            *id,
            P::TerminalView,
            Holder::User { id: s.dev },
        )
        .await;
    }
    async fn same(w: &World, ids: &[Id]) {
        let principal = w.principal(Caller::session(w.boss)).await;
        let mut conn = w.conn().await;
        let grouped = environments::reach_counts(&mut conn, &principal, ids, Policy::default())
            .await
            .unwrap();
        for id in ids {
            let detailed = environments::reach(&mut conn, &principal, *id, Policy::default())
                .await
                .unwrap();
            assert_eq!(
                grouped[id],
                i64::try_from(detailed.people.len()).unwrap() + detailed.hidden,
                "owners and repeated doors count each person once"
            );
        }
    }
    same(&s.w, &ids).await;
    {
        let mut conn = s.w.conn().await;
        orch_db::queries::access::testing::suspend_in(&mut conn, s.w.root, s.dev, s.w.boss)
            .await
            .unwrap();
    }
    same(&s.w, &ids).await;
}

#[tokio::test]
async fn whoever_manages_an_environment_decides_where_threads_run_in_it_and_nobody_else_does() {
    let s = setting().await;
    let into = |thread: Option<Id>, from: Option<Id>, to: Option<Id>| Moving {
        thread,
        from,
        to,
        all_of_the_environment: false,
    };
    // Moving or forking work into it takes Manage; starting a new thread checks Use separately.
    for (who, allowed) in [
        (s.eve, true),
        (s.dev, true),
        (s.ivy, false),
        (s.w.maker, false),
    ] {
        let decision = moving(&s.w, who, into(None, None, Some(s.env))).await;
        assert_eq!(decision.is_allowed(), allowed, "{who}");
    }
    // The Interactor is told what is missing, and who can give it.
    let Decision::Refused(refusal) = moving(&s.w, s.ivy, into(None, None, Some(s.env))).await
    else {
        panic!("refused");
    };
    assert_eq!(refusal.required, P::EnvironmentManage);
    assert_eq!(refusal.missing[0].object.kind, ObjectKind::Environment);
    assert!(refusal.missing[0].ask.contains(&s.eve));
    // Taking a thread to another environment needs both, and seeing the thread.
    let away = into(Some(s.w.thread), Some(s.env), Some(s.other));
    assert!(moving(&s.w, s.eve, away.clone()).await.is_allowed());
    assert!(
        moving(&s.w, s.dev, away.clone()).await.is_allowed(),
        "the Developers manage both"
    );
    // The thread's maker owns the thread, manages it, and reaches none of that.
    assert!(s.w.can(s.w.maker, s.w.thread, P::ThreadManage).await);
    assert!(
        !moving(&s.w, s.w.maker, away.clone()).await.is_allowed(),
        "managing the thread gives none of it"
    );
    // Somebody who manages the environment and holds nothing on the thread but View: allowed.
    let watcher = s.w.member("watcher").await;
    s.w.grant(
        "environment",
        s.env,
        P::EnvironmentManage,
        Holder::User { id: watcher },
    )
    .await;
    s.w.grant(
        "environment",
        s.other,
        P::EnvironmentManage,
        Holder::User { id: watcher },
    )
    .await;
    s.w.name(s.w.thread, P::ThreadView, Holder::User { id: watcher })
        .await;
    assert!(moving(&s.w, watcher, away.clone()).await.is_allowed());
    // One who cannot see the thread cannot move it, unless the request names all the threads of the
    // environment together.
    let blind = s.w.member("blind").await;
    s.w.grant(
        "environment",
        s.env,
        P::EnvironmentManage,
        Holder::User { id: blind },
    )
    .await;
    s.w.grant(
        "environment",
        s.other,
        P::EnvironmentManage,
        Holder::User { id: blind },
    )
    .await;
    s.w.row(s.w.thread, P::ThreadView, false).await;
    assert!(!moving(&s.w, blind, away.clone()).await.is_allowed());
    let all = Moving {
        all_of_the_environment: true,
        ..away
    };
    assert!(moving(&s.w, blind, all).await.is_allowed());
    // A thread that is in none needs `thread.manage`, which its owner holds.
    let loose = s.w.node("thread", s.w.topic, "Loose").await;
    let to_env = into(Some(loose), None, Some(s.env));
    assert!(
        !moving(&s.w, s.w.maker, to_env.clone()).await.is_allowed(),
        "and the environment too"
    );
    assert!(
        !moving(&s.w, s.eve, to_env).await.is_allowed(),
        "the owner of the environment does not manage the thread"
    );
    // A change of directory in the same environment is the environment's alone.
    let same = into(Some(s.w.thread), Some(s.env), Some(s.env));
    assert!(moving(&s.w, s.dev, same).await.is_allowed());
}

/// A host and a template of other people, and a Developer who may create environments in the topic.
struct Making {
    w: World,
    cal: Id,
    hal: Id,
    host: Id,
    template: Id,
}

async fn making() -> Making {
    let w = World::new().await;
    let (cal, hal, tim) = (
        w.member("cal").await,
        w.member("hal").await,
        w.member("tim").await,
    );
    w.give(w.project, cal, Role::Developer).await;
    let host = w.thing("host", hal, w.root, "gpu").await;
    let template = w.thing("template", tim, w.root, "recipe").await;
    Making {
        w,
        cal,
        hal,
        host,
        template,
    }
}

async fn creating(m: &Making, who: Id, place: Place, mounts: Vec<Id>) -> Creating {
    let principal = m.w.principal(Caller::session(who)).await;
    let mut conn = m.w.conn().await;
    environments::check_creation(
        &mut conn,
        &principal,
        &NewEnvironment {
            home: m.w.topic,
            place,
            mounts,
        },
        &Policies::default(),
    )
    .await
    .unwrap()
}

fn on_host(m: &Making, wants: Wants) -> Place {
    Place::Host {
        host: m.host,
        template: m.template,
        wants,
    }
}

#[tokio::test]
async fn creating_an_environment_takes_the_use_of_a_host_and_a_template_and_the_refusal_names_both()
{
    let m = making().await;
    let Creating::Refused(refusal) =
        creating(&m, m.cal, on_host(&m, Wants::default()), vec![]).await
    else {
        panic!("a Developer of the topic uses neither");
    };
    let lacks: Vec<_> = refusal
        .missing
        .iter()
        .map(|entry| (entry.object.kind, entry.permissions.clone()))
        .collect();
    assert_eq!(
        lacks,
        [
            (ObjectKind::Host, vec![P::HostUse]),
            (ObjectKind::Template, vec![P::TemplateUse])
        ],
        "node.interact on the topic is held, and the others are not"
    );
    assert!(
        refusal.missing[0].ask.contains(&m.hal),
        "the host's owner can give it"
    );
    // With the host alone, the template is what is missing.
    m.w.grant("host", m.host, P::HostUse, Holder::User { id: m.cal })
        .await;
    let Creating::Refused(refusal) =
        creating(&m, m.cal, on_host(&m, Wants::default()), vec![]).await
    else {
        panic!("refused");
    };
    assert_eq!(refusal.missing.len(), 1);
    assert_eq!(refusal.missing[0].object.kind, ObjectKind::Template);
    m.w.grant(
        "template",
        m.template,
        P::TemplateUse,
        Holder::User { id: m.cal },
    )
    .await;
    assert!(matches!(
        creating(&m, m.cal, on_host(&m, Wants::default()), vec![]).await,
        Creating::Allowed(_)
    ));
    // Somebody who may not interact with the node is refused it, with the rest.
    let ivy = m.w.member("ivy").await;
    m.w.give(m.w.project, ivy, Role::Interactor).await;
    m.w.grant("host", m.host, P::HostUse, Holder::User { id: ivy })
        .await;
    m.w.grant(
        "template",
        m.template,
        P::TemplateUse,
        Holder::User { id: ivy },
    )
    .await;
    let Creating::Refused(refusal) = creating(&m, ivy, on_host(&m, Wants::default()), vec![]).await
    else {
        panic!("an Interactor does not start work in a topic as shipped");
    };
    assert_eq!(refusal.missing.len(), 1);
    assert_eq!(refusal.missing[0].object.kind, ObjectKind::Topic);
    assert_eq!(refusal.missing[0].permissions, [P::TopicInteract]);
}

#[tokio::test]
async fn the_narrower_of_the_hosts_and_the_templates_limits_counts() {
    let m = making().await;
    m.w.put(
        "host",
        m.host,
        P::HostUse,
        Holder::User { id: m.cal },
        json!({ "resources_max": { "memory_mb": 4096, "cpu_millis": 1000 }, "storages": ["fast"], "sources": ["image"] }),
    )
    .await;
    m.w.put(
        "template",
        m.template,
        P::TemplateUse,
        Holder::User { id: m.cal },
        json!({ "resources_max": { "memory_mb": 2048 }, "capabilities": ["net"] }),
    )
    .await;
    let wants = |memory: i64, cpu: i64| Wants {
        resources: ResourceLimits {
            memory_mb: Some(memory),
            cpu_millis: Some(cpu),
            ..ResourceLimits::default()
        },
        ..Wants::default()
    };
    // The template allows less memory and the host less processor: each limit is the smaller.
    let Creating::Exceeds(limits) =
        creating(&m, m.cal, on_host(&m, wants(3000, 2000)), vec![]).await
    else {
        panic!("beyond both");
    };
    assert!(limits.contains(&Limit::ResourcesAbove {
        resource: "memory_mb",
        allowed: 2048,
        wanted: 3000
    }));
    assert!(limits.contains(&Limit::ResourcesAbove {
        resource: "cpu_millis",
        allowed: 1000,
        wanted: 2000
    }));
    let Creating::Allowed(created) =
        creating(&m, m.cal, on_host(&m, wants(2048, 1000)), vec![]).await
    else {
        panic!("within both");
    };
    assert_eq!(created.ceiling.memory_mb, Some(2048));
    assert_eq!(created.ceiling.cpu_millis, Some(1000));
    assert!(
        created.host.as_ref().unwrap().0.is_some()
            && created.template.as_ref().unwrap().0.is_some(),
        "the entries are recorded"
    );
    // The capabilities, the source and the storage each have an entry's say.
    let asks = Wants {
        capabilities: vec!["net".into(), "docker".into()],
        source: Some("build".into()),
        storage: Some("bulk".into()),
        ..Wants::default()
    };
    let Creating::Exceeds(limits) = creating(&m, m.cal, on_host(&m, asks), vec![]).await else {
        panic!("beyond the entries");
    };
    assert_eq!(limits.len(), 3, "{limits:?}");
    assert!(limits.contains(&Limit::CapabilityNotAllowed {
        capability: "docker".into()
    }));
    assert!(limits.contains(&Limit::SourceNotAllowed {
        source: "build".into()
    }));
    assert!(limits.contains(&Limit::StorageNotAllowed {
        storage: "bulk".into()
    }));
}

#[tokio::test]
async fn a_hosts_entry_caps_how_many_environments_exist_through_it_and_which_templates_it_takes() {
    let m = making().await;
    let entry =
        m.w.put(
            "host",
            m.host,
            P::HostUse,
            Holder::User { id: m.cal },
            json!({ "max_environments": 1 }),
        )
        .await;
    m.w.grant(
        "template",
        m.template,
        P::TemplateUse,
        Holder::User { id: m.cal },
    )
    .await;
    assert!(matches!(
        creating(&m, m.cal, on_host(&m, Wants::default()), vec![]).await,
        Creating::Allowed(_)
    ));
    // One environment now exists that was made through the entry.
    let made = m.w.thing("environment", m.cal, m.w.topic, "first").await;
    {
        let mut conn = m.w.conn().await;
        sqlx::query("UPDATE environments SET host_entry_id = $2 WHERE id = $1")
            .bind(made)
            .bind(entry)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let Creating::Exceeds(limits) =
        creating(&m, m.cal, on_host(&m, Wants::default()), vec![]).await
    else {
        panic!("the cap is reached");
    };
    assert_eq!(
        limits,
        [Limit::TooManyEnvironments {
            allowed: 1,
            made: 1
        }]
    );
    // A wider entry of the person's group or role lets more be made: of two numbers the larger counts.
    m.w.put(
        "host",
        m.host,
        P::HostUse,
        Holder::Role {
            role: Role::Developer,
            at: None,
        },
        json!({ "max_environments": 5 }),
    )
    .await;
    m.w.give(m.w.root, m.cal, Role::Developer).await;
    assert!(matches!(
        creating(&m, m.cal, on_host(&m, Wants::default()), vec![]).await,
        Creating::Allowed(_)
    ));
    // A host whose manager listed the templates takes those.
    let seen = m.w.thing("template", m.hal, m.w.root, "seen").await;
    m.w.grant("template", seen, P::TemplateUse, Holder::User { id: m.cal })
        .await;
    let other_host = m.w.thing("host", m.hal, m.w.root, "listed").await;
    m.w.put(
        "host",
        other_host,
        P::HostUse,
        Holder::User { id: m.cal },
        json!({ "templates": [seen] }),
    )
    .await;
    let place = Place::Host {
        host: other_host,
        template: m.template,
        wants: Wants::default(),
    };
    let Creating::Exceeds(limits) = creating(&m, m.cal, place, vec![]).await else {
        panic!("not a recipe the manager has seen");
    };
    assert_eq!(
        limits,
        [Limit::TemplateNotAllowedOnTheHost {
            template: m.template
        }]
    );
    let place = Place::Host {
        host: other_host,
        template: seen,
        wants: Wants::default(),
    };
    assert!(matches!(
        creating(&m, m.cal, place, vec![]).await,
        Creating::Allowed(_)
    ));
}

#[tokio::test]
async fn a_template_of_the_creators_own_counts_and_the_hosts_entry_says_how_large_it_may_be() {
    let m = making().await;
    let own = m.w.thing("template", m.cal, m.w.root, "mine").await;
    m.w.put(
        "host",
        m.host,
        P::HostUse,
        Holder::User { id: m.cal },
        json!({ "resources_max": { "memory_mb": 1024 } }),
    )
    .await;
    let place = |memory: i64| Place::Host {
        host: m.host,
        template: own,
        wants: Wants {
            resources: ResourceLimits {
                memory_mb: Some(memory),
                ..ResourceLimits::default()
            },
            ..Wants::default()
        },
    };
    assert!(matches!(
        creating(&m, m.cal, place(1024), vec![]).await,
        Creating::Allowed(_)
    ));
    assert!(matches!(
        creating(&m, m.cal, place(2048), vec![]).await,
        Creating::Exceeds(_)
    ));
}

#[tokio::test]
async fn a_mount_needs_its_use_and_a_directory_of_a_machine_needs_owning_it() {
    let m = making().await;
    m.w.grant("host", m.host, P::HostUse, Holder::User { id: m.cal })
        .await;
    m.w.grant(
        "template",
        m.template,
        P::TemplateUse,
        Holder::User { id: m.cal },
    )
    .await;
    let mount = m.w.thing("mount", m.hal, m.w.root, "data").await;
    let Creating::Refused(refusal) =
        creating(&m, m.cal, on_host(&m, Wants::default()), vec![mount]).await
    else {
        panic!("no use of the mount");
    };
    assert_eq!(refusal.missing[0].object.kind, ObjectKind::Mount);
    m.w.grant("mount", mount, P::MountUse, Holder::User { id: m.cal })
        .await;
    assert!(matches!(
        creating(&m, m.cal, on_host(&m, Wants::default()), vec![mount]).await,
        Creating::Allowed(_)
    ));
    // A machine of one's own.
    let machine = m.w.thing("machine", m.cal, m.w.root, "laptop").await;
    assert!(matches!(
        creating(&m, m.cal, Place::Machine { machine }, vec![]).await,
        Creating::Allowed(_)
    ));
    assert!(matches!(
        creating(&m, m.hal, Place::Machine { machine }, vec![]).await,
        Creating::NotYourMachine
    ));
}

#[tokio::test]
async fn a_host_of_the_system_open_to_everyone_serves_a_member_who_names_nothing_else() {
    let m = making().await;
    let system_host = m.w.system_thing("host", "shared").await;
    let system_template = m.w.system_thing("template", "base").await;
    for (kind, id, permission) in [
        ("host", system_host, P::HostUse),
        ("template", system_template, P::TemplateUse),
    ] {
        m.w.grant(kind, id, permission, Holder::Everyone).await;
    }
    let place = Place::Host {
        host: system_host,
        template: system_template,
        wants: Wants::default(),
    };
    assert!(matches!(
        creating(&m, m.cal, place, vec![]).await,
        Creating::Allowed(_)
    ));
}

#[tokio::test]
async fn everybody_who_reaches_into_an_environment_is_told_by_what_its_doors_and_the_agents_there()
{
    let s = setting().await;
    // A second thread in another topic of the same environment, whose agent somebody who sees nothing
    // else may talk to, and doors of the environment's own page for two more people.
    let other_project = s.w.node("project", s.w.root, "Other").await;
    let other_topic = s.w.node("topic", other_project, "Elsewhere").await;
    let second = s.w.thread_in(other_topic, s.env, "Second").await;
    let talker = s.w.member("talker").await;
    let reader = s.w.member("reader").await;
    let runner = s.w.member("runner").await;
    s.w.name(second, P::ThreadInteract, Holder::User { id: talker })
        .await;
    s.w.grant(
        "environment",
        s.env,
        P::FilesView,
        Holder::User { id: reader },
    )
    .await;
    s.w.grant(
        "environment",
        s.env,
        P::TerminalManage,
        Holder::User { id: runner },
    )
    .await;
    // A thread alone does not let its people use the environment.
    let principal = s.w.principal(Caller::session(s.eve)).await;
    let mut conn = s.w.conn().await;
    let without_use = environments::reach(&mut conn, &principal, s.env, Policy::default())
        .await
        .unwrap();
    for user in [talker, s.w.maker] {
        assert!(without_use.people.iter().all(|person| person.user != user));
    }
    drop(conn);
    for user in [talker, s.w.maker] {
        s.w.grant(
            "environment",
            s.env,
            P::EnvironmentUse,
            Holder::User { id: user },
        )
        .await;
    }
    let principal = s.w.principal(Caller::session(s.eve)).await;
    let mut conn = s.w.conn().await;
    let reach = environments::reach(&mut conn, &principal, s.env, Policy::default())
        .await
        .unwrap();
    let find = |user: Id| reach.people.iter().find(|person| person.user == user);
    assert!(find(s.eve).unwrap().through.contains(&Reaching::Owner));
    // Named in one door of the page: that door, at the highest level they hold, and nothing else.
    assert_eq!(
        find(reader).unwrap().through,
        [Reaching::Page(P::FilesView)]
    );
    assert_eq!(
        find(runner).unwrap().through,
        [Reaching::Page(P::TerminalManage)],
        "the Manage of a door says its View"
    );
    // Somebody who may talk to an agent there reaches it by that, in a thread of another topic as well.
    assert_eq!(find(talker).unwrap().through, [Reaching::Thread(second)]);
    // The maker owns both threads, and reaches into the environment through their agents and no
    // other way: a thread's page holds none of the doors.
    let mut through_agents = vec![Reaching::Thread(s.w.thread), Reaching::Thread(second)];
    through_agents.sort();
    assert_eq!(find(s.w.maker).unwrap().through, through_agents);
    // The Developers who manage the environment are on its page by managing it, and not once for
    // each of the four doors they hold with that.
    let developer = &find(s.dev).unwrap().through;
    assert!(developer.contains(&Reaching::Page(P::EnvironmentManage)));
    assert!(
        developer
            .iter()
            .all(|way| !matches!(way, Reaching::Page(door) if *door != P::EnvironmentManage)),
        "{developer:?}"
    );
    // A reader who sees none of the threads is told how many reach it only through threads they
    // cannot view, and not who.
    let stranger = s.w.member("stranger").await;
    s.w.grant(
        "environment",
        s.env,
        P::EnvironmentView,
        Holder::User { id: stranger },
    )
    .await;
    s.w.row(second, P::ThreadView, false).await;
    let principal = s.w.principal(Caller::session(stranger)).await;
    let reach = environments::reach(&mut conn, &principal, s.env, Policy::default())
        .await
        .unwrap();
    assert!(
        reach
            .people
            .iter()
            .all(|person| person.user != talker && person.user != s.w.maker),
        "who reach it only through threads the reader cannot view are not named"
    );
    assert!(
        reach.people.iter().all(|person| person
            .through
            .iter()
            .all(|way| !matches!(way, Reaching::Thread(_)))),
        "nothing is named that goes through a thread they cannot view"
    );
    assert!(
        reach.hidden >= 2,
        "at least the talker and the maker are counted"
    );
    assert!(
        reach.people.iter().any(|person| person.user == reader),
        "what the doors of the environment's own page say is no secret of any thread"
    );
}

#[tokio::test]
async fn a_threads_page_says_how_many_people_read_the_files_of_its_environment_whatever_it_says_itself()
 {
    let s = setting().await;
    let other_project = s.w.node("project", s.w.root, "Other").await;
    let other_topic = s.w.node("topic", other_project, "Elsewhere").await;
    s.w.thread_in(other_topic, s.env, "Second").await;
    let reader = s.w.member("reader").await;
    s.w.grant(
        "environment",
        s.env,
        P::FilesView,
        Holder::User { id: reader },
    )
    .await;
    // The thread's own page is made private: its conversation is its people's, its files are not.
    s.w.row(s.w.thread, P::ThreadView, false).await;
    let principal = s.w.principal(Caller::session(s.eve)).await;
    let mut conn = s.w.conn().await;
    let reaches = environments::reaches(&mut conn, &principal, s.w.thread, Policy::default())
        .await
        .unwrap()
        .unwrap();
    assert_eq!((reaches.environment, reaches.threads), (s.env, 1));
    // It counts who holds `files.view` there, by whatever way: the one decision.
    let mut holding = 0_i64;
    for user in [s.w.boss, s.w.maker, s.eve, s.dev, s.ivy, reader] {
        if s.w.can_on(user, "environment", s.env, P::FilesView).await {
            holding += 1;
        }
    }
    assert!(
        holding >= 4,
        "the Viewers, the Developers and the one named"
    );
    assert_eq!(reaches.people, holding);
    assert!(
        environments::reaches(
            &mut conn,
            &principal,
            s.w.node("thread", s.w.topic, "None").await,
            Policy::default()
        )
        .await
        .unwrap()
        .is_none(),
        "a thread that is in no environment reaches nothing"
    );
}

#[tokio::test]
async fn a_shell_is_its_openers_alone_closed_by_the_environments_managers_and_when_what_opened_it_stops()
 {
    let s = setting().await;
    let opener = s.w.member("opener").await;
    let entry =
        s.w.grant(
            "environment",
            s.env,
            P::TerminalManage,
            Holder::User { id: opener },
        )
        .await;
    let shell = Shell {
        environment: s.env,
        thread: Some(s.w.thread),
        opened_by: opener,
    };
    let policy = Policy::default();
    // Nobody attaches to somebody else's, whatever they hold.
    assert!(environments::may_attach(&shell, opener));
    assert!(!environments::may_attach(&shell, s.eve));
    let mut conn = s.w.conn().await;
    let as_user = |user: Id| {
        let w = &s.w;
        async move { w.principal(Caller::session(user)).await }
    };
    // A shell is opened by `terminal.manage` on the environment: named in it, or by managing it.
    // Whatever is held on the thread it is opened beside gives none of it.
    for (user, allowed) in [
        (opener, true),
        (s.eve, true),
        (s.dev, true),
        (s.ivy, false),
        (s.w.maker, false),
    ] {
        let p = as_user(user).await;
        assert_eq!(
            environments::may_open_shell(&mut conn, &p, s.env, policy)
                .await
                .unwrap()
                .is_allowed(),
            allowed,
            "{user}"
        );
    }
    assert!(
        s.w.can(s.w.maker, s.w.thread, P::ThreadManage).await,
        "the maker owns the thread and holds everything on it"
    );
    // The opener and the managers close it; a Viewer does not.
    for (user, closes) in [(opener, true), (s.eve, true), (s.dev, true), (s.ivy, false)] {
        let p = as_user(user).await;
        assert_eq!(
            environments::may_close_shell(&mut conn, &p, &shell, policy)
                .await
                .unwrap(),
            closes,
            "{user}"
        );
    }
    // What it printed is read by its opener, its managers and whoever audits the thread; a read
    // by another is audited.
    let auditor = s.w.member("auditor").await;
    s.w.name(s.w.thread, P::AuditView, Holder::User { id: auditor })
        .await;
    for (user, allowed, audited) in [
        (opener, true, false),
        (s.eve, true, true),
        (auditor, true, true),
        (s.ivy, false, false),
    ] {
        let p = as_user(user).await;
        let reading = environments::may_read_recording(&mut conn, &p, &shell, policy)
            .await
            .unwrap();
        assert_eq!(
            (reading.allowed, reading.audited),
            (allowed, audited),
            "{user}"
        );
    }
    // It stays open while its opener holds what opened it, and is closed when they do not: the row
    // of the environment's page, wherever the shell was opened beside.
    let p = as_user(opener).await;
    assert!(
        environments::stays_open(&mut conn, &p, &shell, policy)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE permission_entries SET removed_at = now() WHERE id = $1")
        .bind(entry)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(
        !environments::stays_open(&mut conn, &p, &shell, policy)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn a_shared_machine_use_entry_admits_environment_creation_without_making_the_caller_its_owner()
 {
    let m = making().await;
    let machine =
        m.w.thing("machine", m.hal, m.w.root, "shared-machine")
            .await;
    let before = creating(&m, m.cal, Place::Machine { machine }, vec![]).await;
    assert!(
        !matches!(before, Creating::Allowed(_)),
        "no grant admits no registration"
    );
    m.w.grant(
        "machine",
        machine,
        P::MachineUse,
        Holder::User { id: m.cal },
    )
    .await;
    assert!(matches!(
        creating(&m, m.cal, Place::Machine { machine }, vec![]).await,
        Creating::Allowed(_)
    ));
    let page = m.w.read_thing(m.hal, "machine", machine).await;
    assert_eq!(page.first_owner, Some(m.hal));
    assert!(
        !page
            .owners
            .iter()
            .any(|entry| entry.holder == Holder::User { id: m.cal })
    );
}

#[tokio::test]
async fn a_machine_owner_manages_a_creators_connected_environment_without_becoming_its_owner() {
    let m = making().await;
    let machine = m.w.thing("machine", m.hal, m.w.root, "machine").await;
    let environment =
        m.w.thing("environment", m.cal, m.w.root, "registered")
            .await;
    {
        let mut conn = m.w.conn().await;
        orch_db::queries::resources::access_fixtures::register_on(
            &mut conn,
            "environment",
            environment,
            machine,
        )
        .await
        .unwrap();
    }
    assert!(
        m.w.can_on(m.cal, "environment", environment, P::EnvironmentManage)
            .await
    );
    assert!(
        m.w.can_on(m.hal, "environment", environment, P::EnvironmentManage)
            .await
    );
    assert!(
        !m.w.can_on(m.w.maker, "environment", environment, P::EnvironmentManage)
            .await
    );
    let page = m.w.read_thing(m.cal, "environment", environment).await;
    assert_eq!(page.first_owner, Some(m.cal));
    assert_eq!(page.owners.len(), 1);
    assert_eq!(page.owners[0].holder, Holder::User { id: m.cal });
}

#[tokio::test]
async fn a_registered_environments_creator_can_transfer_ownership_without_transferring_the_machine()
{
    let m = making().await;
    let successor = m.w.member("successor").await;
    let machine = m.w.thing("machine", m.hal, m.w.root, "machine").await;
    let environment =
        m.w.thing("environment", m.cal, m.w.root, "registered")
            .await;
    {
        let mut conn = m.w.conn().await;
        orch_db::queries::resources::access_fixtures::register_on(
            &mut conn,
            "environment",
            environment,
            machine,
        )
        .await
        .unwrap();
    }
    let principal = m.w.principal(Caller::session(m.cal)).await;
    {
        let mut tx = m.w.db.pool.begin().await.unwrap();
        orch_authz::ownership::transfer(
            &mut tx,
            &principal,
            support::things::object("environment", environment),
            successor,
            false,
            &orch_authz::usable::ResourcePolicy::default(),
            &orch_db::audit::Origin::default(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let page = m.w.read_thing(successor, "environment", environment).await;
    assert_eq!(page.first_owner, Some(successor));
    assert_eq!(page.owners.len(), 1);
    assert_eq!(page.owners[0].holder, Holder::User { id: successor });
    assert!(
        m.w.can_on(successor, "environment", environment, P::EnvironmentManage)
            .await
    );
    assert!(
        m.w.can_on(m.hal, "environment", environment, P::EnvironmentManage)
            .await
    );
    let machine_page = m.w.read_thing(m.hal, "machine", machine).await;
    assert_eq!(machine_page.first_owner, Some(m.hal));
}
