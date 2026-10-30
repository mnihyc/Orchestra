//! What the thread engine asks about resources is answered by the access engine: a person named in a provider's Use row, one narrowed to a model, a stranger, a subscription kept to its owner, a suspended person, an agent profile, an environment's manager, and who may let a turn past a limit.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_access_contract::decisions::MissingAccess;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_core::thread_access::AuthzAccess;
use orch_db::queries::access::{NewEntry, PageKey, insert_entry, testing as access_rows};
use orch_db::queries::resources::access_fixtures;
use orch_engine::access::{Answer, Limit, ProviderUse, ResourceAccess};
use orch_settings::Target;
use orch_types::{Id, Json, new_id};
use places::{Who, join, make_org, make_project, make_topic, person};
use serde_json::json;
use support::World;

/// An org of `ada`, with a topic in it and a thread in the topic that runs in a managed environment;
/// a provider of hers with two models, an agent profile, and the environment; `bea` works in the org as a
/// developer; `sam` is a stranger to it.
struct Setting {
    world: World,
    ada: Who,
    bea: Who,
    sam: Who,
    org: Id,
    topic: Id,
    thread: Id,
    provider: Id,
    agent: Id,
    environment: Id,
    access: AuthzAccess,
}

impl Setting {
    async fn new() -> Self {
        let world = World::new().await;
        let ada = person(&world, "ada").await;
        let bea = person(&world, "bea").await;
        let sam = person(&world, "sam").await;
        let org = make_org(&world, &ada, "Acme", "acme").await.base.id;
        let project = make_project(&world, &ada, org, "Web").await.base.id;
        let topic = make_topic(&world, &ada, project, "Login").await.base.id;
        join(&world, org, org, &bea, "developer", &ada).await;
        let mut conn = world.conn().await;
        let thread = new_id();
        orch_db::queries::hierarchy::testing::insert_node(
            &mut conn,
            thread,
            "thread",
            Some(topic),
            "Fix",
            ada.id(),
        )
        .await
        .unwrap();
        let provider = access_fixtures::insert_resource(
            &mut conn,
            "provider",
            Some(ada.id()),
            Some(org),
            "Team Pro",
        )
        .await
        .unwrap();
        let agent = access_fixtures::insert_resource(
            &mut conn,
            "agent",
            Some(ada.id()),
            Some(org),
            "Coder",
        )
        .await
        .unwrap();
        let environment = access_fixtures::insert_environment(
            &mut conn,
            ada.id(),
            Some(topic),
            "Main",
            "managed",
        )
        .await
        .unwrap();
        access_fixtures::put_thread_into(&mut conn, thread, environment)
            .await
            .unwrap();
        for model in ["big", "small"] {
            access_fixtures::insert_model(
                &mut conn,
                provider,
                model,
                &["low", "high"],
                true,
                ada.id(),
            )
            .await
            .unwrap();
        }
        drop(conn);
        let setting = Self {
            access: AuthzAccess::new(world.app.app.clock.clone()),
            world,
            ada,
            bea,
            sam,
            org,
            topic,
            thread,
            provider,
            agent,
            environment,
        };
        for (kind, id) in [
            ("provider", provider),
            ("agent", agent),
            ("environment", environment),
        ] {
            setting
                .row(kind, id, "access.own", &setting.ada, json!({}))
                .await;
        }
        setting
    }

    /// Names a person in a row of a resource's page.
    async fn row(&self, kind: &str, id: Id, permission: &str, who: &Who, options: Json) {
        let mut conn = self.world.conn().await;
        insert_entry(
            &mut conn,
            &NewEntry {
                id: new_id(),
                page: &PageKey::new(kind, id, "live"),
                profile: "any",
                permission,
                holder_kind: "user",
                holder_id: Some(who.id()),
                holder_role: None,
                options: &options,
                expires_at: None,
                note: "",
                source: "page",
                created_by: None,
                at: self.world.now(),
            },
        )
        .await
        .unwrap();
    }

    /// May `who` run a turn of `model` on the thread's provider?
    async fn turn(&self, who: &Who, model: &str, node: Id) -> Answer {
        let mut conn = self.world.conn().await;
        self.access
            .provider_use(
                &mut conn,
                &ProviderUse {
                    person: who.id(),
                    node,
                    provider: self.provider,
                    model: Some(model),
                    effort: Some("high"),
                },
            )
            .await
            .unwrap()
    }
}

fn refused(answer: Answer) -> Vec<MissingAccess> {
    match answer {
        Answer::Refused(missing) => missing,
        Answer::Allowed { .. } => panic!("a refusal was expected"),
    }
}

fn lacks(missing: &[MissingAccess]) -> Vec<(ObjectKind, Vec<Permission>)> {
    missing
        .iter()
        .map(|each| (each.object.kind, each.permissions.clone()))
        .collect()
}

#[tokio::test]
async fn the_owner_and_a_person_named_in_the_use_row_run_a_turn_and_a_stranger_is_refused_with_everything_missing()
 {
    let s = Setting::new().await;
    s.row("provider", s.provider, "provider.use", &s.bea, json!({}))
        .await;
    assert!(s.turn(&s.ada, "big", s.thread).await.is_allowed());
    assert!(s.turn(&s.bea, "big", s.thread).await.is_allowed());

    let missing = refused(s.turn(&s.sam, "big", s.thread).await);
    assert_eq!(
        lacks(&missing),
        [
            (ObjectKind::Thread, vec![Permission::ThreadInteract]),
            (ObjectKind::Provider, vec![Permission::ProviderUse]),
        ],
        "both things, not only the first"
    );
    assert!(
        missing.iter().all(|each| each.object.name.is_none()),
        "a stranger is told no name"
    );
}

#[tokio::test]
async fn a_person_named_for_one_model_runs_that_one_and_is_told_which_she_lacks_for_the_other() {
    let s = Setting::new().await;
    s.row(
        "provider",
        s.provider,
        "provider.use",
        &s.bea,
        json!({ "models": ["small"] }),
    )
    .await;
    assert!(s.turn(&s.bea, "small", s.thread).await.is_allowed());
    let missing = refused(s.turn(&s.bea, "big", s.thread).await);
    assert_eq!(
        lacks(&missing),
        [(ObjectKind::Provider, vec![Permission::ProviderUse])]
    );
    let wanted = missing[0]
        .options
        .as_ref()
        .expect("the least that would do");
    assert_eq!(wanted.models.as_deref(), Some(&["big".to_owned()][..]));
    assert!(
        missing[0].ask.contains(&s.ada.id()),
        "the owner can give it"
    );
}

#[tokio::test]
async fn a_thread_being_made_in_a_topic_is_asked_about_as_the_binding_of_a_new_thread() {
    let s = Setting::new().await;
    s.row("provider", s.provider, "provider.use", &s.bea, json!({}))
        .await;
    assert!(
        s.turn(&s.bea, "big", s.topic).await.is_allowed(),
        "a developer starts work in a topic"
    );
    let missing = refused(s.turn(&s.sam, "big", s.topic).await);
    let kinds: Vec<_> = missing.iter().map(|each| each.object.kind).collect();
    assert!(kinds.contains(&ObjectKind::Provider), "{kinds:?}");
    assert!(kinds.contains(&ObjectKind::Topic), "{kinds:?}");
    // Without the use of the provider, a developer who may start work is still refused it.
    let s = Setting::new().await;
    let missing = refused(s.turn(&s.bea, "big", s.topic).await);
    assert_eq!(
        lacks(&missing),
        [(ObjectKind::Provider, vec![Permission::ProviderUse])]
    );
}

#[tokio::test]
async fn asked_about_the_provider_alone_the_answer_is_about_the_provider_alone() {
    let s = Setting::new().await;
    let mut conn = s.world.conn().await;
    let ask = ProviderUse {
        person: s.bea.id(),
        node: s.thread,
        provider: s.provider,
        model: None,
        effort: None,
    };
    let missing = refused(s.access.provider_use(&mut conn, &ask).await.unwrap());
    assert_eq!(
        lacks(&missing),
        [(ObjectKind::Provider, vec![Permission::ProviderUse])]
    );
    drop(conn);
    s.row("provider", s.provider, "provider.use", &s.bea, json!({}))
        .await;
    let mut conn = s.world.conn().await;
    assert!(
        s.access
            .provider_use(&mut conn, &ask)
            .await
            .unwrap()
            .is_allowed()
    );
}

#[tokio::test]
async fn a_subscription_kept_to_its_owner_is_refused_to_others_with_nobody_to_ask_until_it_is_shared()
 {
    let s = Setting::new().await;
    let mut conn = s.world.conn().await;
    let subscription =
        access_fixtures::insert_subscription(&mut conn, s.ada.id(), s.org, "Ada's login", "fake")
            .await
            .unwrap();
    access_fixtures::insert_model(&mut conn, subscription, "big", &["high"], true, s.ada.id())
        .await
        .unwrap();
    drop(conn);
    s.row("provider", subscription, "access.own", &s.ada, json!({}))
        .await;
    s.row("provider", subscription, "provider.use", &s.bea, json!({}))
        .await;
    let ask = |who: Id| ProviderUse {
        person: who,
        node: s.thread,
        provider: subscription,
        model: Some("big"),
        effort: Some("high"),
    };
    // Unset, subscriptions of every kind are shared; the org leaves this kind out.
    let mut conn = s.world.conn().await;
    assert!(
        s.access
            .provider_use(&mut conn, &ask(s.bea.id()))
            .await
            .unwrap()
            .is_allowed()
    );
    drop(conn);
    s.world
        .set_at(
            Target::Node(s.org),
            "providers.subscription_sharing",
            json!(["claude-code"]),
        )
        .await;
    let mut conn = s.world.conn().await;
    assert!(
        s.access
            .provider_use(&mut conn, &ask(s.ada.id()))
            .await
            .unwrap()
            .is_allowed()
    );
    let kept = refused(
        s.access
            .provider_use(&mut conn, &ask(s.bea.id()))
            .await
            .unwrap(),
    );
    let provider = kept
        .iter()
        .find(|each| each.object.kind == ObjectKind::Provider)
        .expect("the provider is named");
    assert_eq!(
        provider.closed,
        Some(orch_access_contract::decisions::Closed::SubscriptionNotShared)
    );
    assert!(
        provider.ask.is_empty(),
        "nobody can give what is kept to its owner"
    );
    drop(conn);

    // Its org shares subscriptions of this harness: the entry that was named counts.
    s.world
        .set_at(
            Target::Node(s.org),
            "providers.subscription_sharing",
            json!(["fake"]),
        )
        .await;
    let mut conn = s.world.conn().await;
    assert!(
        s.access
            .provider_use(&mut conn, &ask(s.bea.id()))
            .await
            .unwrap()
            .is_allowed()
    );
}

#[tokio::test]
async fn a_suspended_person_is_refused_and_there_is_nothing_they_could_ask_for() {
    let s = Setting::new().await;
    s.row("provider", s.provider, "provider.use", &s.bea, json!({}))
        .await;
    assert!(s.turn(&s.bea, "big", s.thread).await.is_allowed());
    let mut conn = s.world.conn().await;
    access_rows::suspend_in(&mut conn, s.org, s.bea.id(), s.ada.id())
        .await
        .unwrap();
    drop(conn);
    let missing = refused(s.turn(&s.bea, "big", s.thread).await);
    assert!(
        missing.is_empty(),
        "a suspension is no row to ask for: {missing:?}"
    );
}

#[tokio::test]
async fn a_provider_that_is_switched_off_is_no_refusal_of_the_person() {
    let s = Setting::new().await;
    s.row("provider", s.provider, "provider.use", &s.bea, json!({}))
        .await;
    let mut conn = s.world.conn().await;
    access_fixtures::set_disabled(&mut conn, "provider", s.provider, true)
        .await
        .unwrap();
    drop(conn);
    assert!(
        s.turn(&s.bea, "big", s.thread).await.is_allowed(),
        "what stops a turn for everybody is the engine's own check to make, not a lack of the person's"
    );
}

#[tokio::test]
async fn an_agent_profile_is_used_by_whoever_its_page_names() {
    let s = Setting::new().await;
    let ask = |who: Id| {
        let (access, world) = (&s.access, &s.world);
        let (thread, agent) = (s.thread, s.agent);
        async move {
            let mut conn = world.conn().await;
            access
                .agent_use(&mut conn, who, thread, agent)
                .await
                .unwrap()
        }
    };
    assert!(ask(s.ada.id()).await.is_allowed());
    let missing = refused(ask(s.bea.id()).await);
    assert_eq!(
        lacks(&missing),
        [(ObjectKind::Agent, vec![Permission::AgentUse])]
    );
    s.row("agent", s.agent, "agent.use", &s.bea, json!({}))
        .await;
    assert!(ask(s.bea.id()).await.is_allowed());
}

#[tokio::test]
async fn an_environments_manager_may_put_a_thread_there_and_nobody_else_may() {
    let s = Setting::new().await;
    // A directory on Ada's own machine: only its owner manages it until somebody is named.
    let mut conn = s.world.conn().await;
    let environment = access_fixtures::insert_environment(
        &mut conn,
        s.ada.id(),
        Some(s.topic),
        "Laptop",
        "connected",
    )
    .await
    .unwrap();
    drop(conn);
    s.row("environment", environment, "access.own", &s.ada, json!({}))
        .await;
    let ask = |who: Id| {
        let access = &s.access;
        let world = &s.world;
        async move {
            let mut conn = world.conn().await;
            access
                .environment_manage(&mut conn, who, environment)
                .await
                .unwrap()
        }
    };
    assert!(ask(s.ada.id()).await.is_allowed());
    let missing = refused(ask(s.bea.id()).await);
    assert_eq!(
        lacks(&missing),
        [(ObjectKind::Environment, vec![Permission::EnvironmentManage])]
    );
    s.row(
        "environment",
        environment,
        "environment.manage",
        &s.bea,
        json!({}),
    )
    .await;
    assert!(ask(s.bea.id()).await.is_allowed());
    assert!(!ask(s.sam.id()).await.is_allowed());
    // An environment made from a template is managed by the Developers of its place as shipped.
    let mut conn = s.world.conn().await;
    assert!(
        s.access
            .environment_manage(&mut conn, s.bea.id(), s.environment)
            .await
            .unwrap()
            .is_allowed()
    );
}

#[tokio::test]
async fn only_whoever_holds_the_bypass_row_may_let_a_turn_past_the_providers_limit_and_a_cap_nobody()
 {
    let s = Setting::new().await;
    let may = |who: Id, limit: Limit| {
        let (access, world) = (&s.access, &s.world);
        async move {
            let mut conn = world.conn().await;
            // As each acts in an ordinary session, without As admin.
            access
                .may_pass(&mut conn, who, false, &limit)
                .await
                .unwrap()
        }
    };
    let limit = Limit::ProviderLimit {
        provider: s.provider,
    };
    assert!(may(s.ada.id(), limit).await, "its owner");
    assert!(!may(s.bea.id(), limit).await);
    s.row("provider", s.provider, "provider.bypass", &s.bea, json!({}))
        .await;
    assert!(may(s.bea.id(), limit).await);
    assert!(!may(s.sam.id(), limit).await);
    assert!(
        !may(s.ada.id(), Limit::Cap { cap: new_id() }).await,
        "a cap that is not there is passed by nobody"
    );
}
