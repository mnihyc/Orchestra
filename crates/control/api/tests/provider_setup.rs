//! What the page that adds a provider is rendered from, over HTTP: the sign-in methods of the profiles the caller may use, the protocols with what an endpoint of each is asked for, the presets, the machines that may keep a key, and what the policies of the home say.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{
    OrgAgentCreate, OrgAgentCreateBody, ProviderSetupGet, ProviderSetupGetQuery,
};
use orch_db::queries::resources::access_fixtures::insert_persons_machine;
use orch_settings::Target;
use orch_types::Id;
use places::{Who, missing_permissions};
use resources::{company, name_person};
use serde_json::json;
use support::World;

fn descriptor() -> serde_json::Value {
    json!({
        "kind": "fake", "adapter_version": 1,
        "harness": { "name": "Fake", "version": "1.0.0", "supported_range": ">=1.0.0", "status": "supported" },
        "arrangements": [{ "type": "separated" }],
        "native_provider": {
            "harness_kind": "fake", "title": "Fake plan",
            "login_methods": [{
                "id": "token", "title": "A token", "summary": "Paste a token you have.", "kind": "import",
                "fields": [], "yields": "token", "lifetime_secs": null, "requires": null, "recommended": true
            }],
            "options_schema": {}, "arrangements": ["separated"]
        },
        "forwarded_protocols": ["openai-responses"], "direct_protocols": []
    })
}

async fn agent_with_descriptor(world: &World, by: &Who, org: Id) -> Id {
    let made = world
        .app
        .call(OrgAgentCreate, &[&org])
        .token(&by.token)
        .body(&OrgAgentCreateBody {
            name: "Fake".to_owned(),
            harness_kind: "fake".to_owned(),
            profile: json!({}),
        })
        .send()
        .await
        .ok();
    sqlx::query("UPDATE agents SET descriptor = $2 WHERE id = $1")
        .bind(made.base.id)
        .bind(descriptor())
        .execute(&world.db.pool)
        .await
        .unwrap();
    made.base.id
}

#[tokio::test]
async fn a_usable_profile_without_a_report_is_explained_without_guessing_subscription_support() {
    let world = World::new().await;
    let c = company(&world).await;
    let id = agent_with_descriptor(&world, &c.boss, c.acme).await;
    sqlx::query("UPDATE agents SET descriptor='{}',descriptor_overrides='{}' WHERE id=$1")
        .bind(id)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let page = setup(&world, &c.boss, c.acme).await.ok();
    assert!(!page.subscriptions.iter().any(|entry| entry.agent.id == id));
    assert!(
        page.subscriptions_unreported
            .as_ref()
            .is_some_and(|agents| agents.iter().any(|agent| agent.id == id))
    );
    assert!(page.subscriptions_unavailable.as_ref().is_some_and(|rows|rows.iter().any(|row|
        row.agent.as_ref().is_some_and(|agent|agent.id==id)
        && row.reason==orch_api_contract::resources::ProviderSetupSubscriptionUnavailableReason::NotReported)));
    let hidden = setup(&world, &c.viewer, c.acme).await.ok();
    assert!(
        hidden
            .subscriptions_unreported
            .as_ref()
            .is_none_or(|agents| agents.iter().all(|agent| agent.id != id)),
        "unknown capability does not disclose an unusable private profile"
    );
    assert!(
        hidden
            .subscriptions_unavailable
            .as_ref()
            .is_none_or(|rows| rows
                .iter()
                .all(|row| row.agent.as_ref().is_none_or(|agent| agent.id != id)))
    );
}

#[tokio::test]
async fn public_programs_without_a_usable_profile_stay_named_without_exposing_private_profiles_or_guessing_sign_in()
 {
    use orch_api_contract::resources::ProviderSetupSubscriptionUnavailableReason as Reason;
    let world = World::new().await;
    let c = company(&world).await;
    let mut public = Vec::new();
    for (kind, name) in [
        ("example-codex", "Example Codex"),
        ("example-claude", "Example Claude"),
    ] {
        let id = agent_with_descriptor(&world, &c.boss, c.acme).await;
        sqlx::query("UPDATE agents SET name=$2,harness_kind=$3,owner_id=NULL,org_id=NULL,home_id=NULL,machine_id=NULL,descriptor='{}',descriptor_overrides='{}' WHERE id=$1")
            .bind(id).bind(name).bind(kind).execute(&world.db.pool).await.unwrap();
        public.push((id, kind, name));
    }
    let private = agent_with_descriptor(&world, &c.boss, c.acme).await;
    sqlx::query("UPDATE agents SET name='A private program',harness_kind='private-example',descriptor='{}' WHERE id=$1")
        .bind(private).execute(&world.db.pool).await.unwrap();
    let page = setup(&world, &c.viewer, c.acme).await.ok();
    let unavailable = page.subscriptions_unavailable.as_ref().unwrap();
    for (_, kind, name) in &public {
        let choice = unavailable
            .iter()
            .find(|row| row.harness.kind == *kind)
            .unwrap();
        assert_eq!(choice.harness.name, *name);
        assert_eq!(choice.reason, Reason::ProfileNeeded);
        assert!(
            choice.agent.is_none(),
            "a catalogue name reveals no private profile"
        );
    }
    assert!(
        unavailable
            .iter()
            .all(|row| row.harness.kind != "private-example")
    );
    assert!(
        page.subscriptions.is_empty(),
        "an installed name does not declare native sign-in support"
    );
    let (id, _, _) = public[0];
    name_person(&world, "agent", id, "agent.use", c.viewer.id()).await;
    let readable = setup(&world, &c.viewer, c.acme).await.ok();
    let rows = readable.subscriptions_unavailable.as_ref().unwrap();
    assert_eq!(
        rows.iter()
            .filter(|row| row.harness.kind == public[0].1)
            .count(),
        1
    );
    assert!(rows.iter().any(|row| row.reason == Reason::NotReported
        && row.agent.as_ref().is_some_and(|agent| agent.id == id)));
    sqlx::query("UPDATE agents SET descriptor=$2 WHERE id=$1")
        .bind(id)
        .bind({
            let mut report = descriptor();
            report["kind"] = json!(public[0].1);
            report["native_provider"] = serde_json::Value::Null;
            report
        })
        .execute(&world.db.pool)
        .await
        .unwrap();
    let unsupported = setup(&world, &c.viewer, c.acme).await.ok();
    assert!(
        unsupported
            .subscriptions_unavailable
            .as_ref()
            .unwrap()
            .iter()
            .any(|row| row.reason == Reason::Unsupported
                && row.agent.as_ref().is_some_and(|agent| agent.id == id))
    );
}

async fn setup(
    world: &World,
    by: &Who,
    org: Id,
) -> orch_api_core::testing::Reply<orch_api_contract::resources::ProviderSetup> {
    world
        .app
        .call(ProviderSetupGet, &[])
        .token(&by.token)
        .query(&ProviderSetupGetQuery {
            org_id: org,
            provider_id: None,
        })
        .send()
        .await
}

#[tokio::test]
async fn the_setup_lists_the_protocols_with_their_options_and_the_presets_a_member_may_add_a_provider_from()
 {
    let world = World::new().await;
    let c = company(&world).await;
    let page = setup(&world, &c.viewer, c.acme).await.ok();
    let names: Vec<&str> = page.protocols.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "openai-responses",
            "openai-chat",
            "anthropic-messages",
            "openai-embeddings"
        ]
    );
    assert_eq!(page.protocols[3].title, "OpenAI Embeddings");
    assert!(
        page.protocols[3].agents.is_empty(),
        "no agent speaks the format that makes vectors"
    );
    let options: Vec<&str> = page.protocols[0]
        .endpoint_options
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert!(
        options.contains(&"auth") && options.contains(&"secret_headers"),
        "{options:?}"
    );
    let preset_names: Vec<&str> = page.presets.iter().map(|p| p.name.as_str()).collect();
    assert!(
        preset_names.contains(&"OpenAI")
            && preset_names.contains(&"Anthropic")
            && preset_names.contains(&"OpenRouter"),
        "{preset_names:?}"
    );
    let openai = page.presets.iter().find(|p| p.name == "OpenAI").unwrap();
    assert_eq!(openai.upstreams[0].protocol, "openai-responses");
    let openrouter = page
        .presets
        .iter()
        .find(|preset| preset.name == "OpenRouter")
        .unwrap();
    assert_eq!(openrouter.upstreams[0].protocol, "openai-chat");
    assert_eq!(
        openrouter.upstreams[0].base_url,
        "https://openrouter.ai/api/v1"
    );
    assert!(!page.http_allowed);
    assert_eq!(
        page.api_arrangements.iter().filter(|a| a.allowed).count(),
        2
    );
    world
        .set_at(
            Target::Node(c.acme),
            "providers.allow_http_upstreams",
            json!(true),
        )
        .await;
    assert!(setup(&world, &c.viewer, c.acme).await.ok().http_allowed);
}

#[tokio::test]
async fn the_setup_is_for_whoever_may_add_a_provider_in_that_org() {
    let world = World::new().await;
    let c = company(&world).await;
    let refused = setup(&world, &c.stranger, c.acme).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let nobody = world
        .app
        .call(ProviderSetupGet, &[])
        .query(&ProviderSetupGetQuery {
            org_id: c.acme,
            provider_id: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
    world
        .set_at(
            Target::Node(c.acme),
            "resources.created_by_members",
            json!(["host"]),
        )
        .await;
    let kept = setup(&world, &c.viewer, c.acme).await.problem();
    assert_eq!(missing_permissions(&kept), ["settings.manage"]);
    assert!(setup(&world, &c.admin, c.acme).await.body.is_ok());
    let project = places::make_project(&world, &c.boss, c.acme, "Web").await;
    let not_an_org = setup(&world, &c.boss, project.base.id).await.problem();
    assert_eq!(not_an_org.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn chat_completions_names_only_available_agents_that_declare_the_format_and_the_reader_can_use()
 {
    let world = World::new().await;
    let c = company(&world).await;
    let chat = agent_with_descriptor(&world, &c.boss, c.acme).await;
    let hidden = agent_with_descriptor(&world, &c.boss, c.acme).await;
    let disabled = agent_with_descriptor(&world, &c.boss, c.acme).await;
    let responses = agent_with_descriptor(&world, &c.boss, c.acme).await;
    let mut chat_descriptor = descriptor();
    chat_descriptor["forwarded_protocols"] = json!(["openai-chat"]);
    chat_descriptor["native_provider"] = json!(null);
    for id in [chat, hidden, disabled] {
        sqlx::query("UPDATE agents SET descriptor=$2 WHERE id=$1")
            .bind(id)
            .bind(&chat_descriptor)
            .execute(&world.db.pool)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE agents SET disabled=true WHERE id=$1")
        .bind(disabled)
        .execute(&world.db.pool)
        .await
        .unwrap();
    for id in [chat, disabled, responses] {
        name_person(&world, "agent", id, "agent.use", c.viewer.id()).await;
    }
    let page = setup(&world, &c.viewer, c.acme).await.ok();
    let offered = page
        .protocols
        .iter()
        .find(|protocol| protocol.name == "openai-chat")
        .unwrap();
    assert_eq!(
        offered
            .agents
            .iter()
            .map(|agent| agent.id)
            .collect::<Vec<_>>(),
        [chat]
    );
    assert_eq!(offered.title, "OpenAI Chat Completions");
    let offered = page
        .protocols
        .iter()
        .find(|protocol| protocol.name == "openai-responses")
        .unwrap();
    assert_eq!(
        offered
            .agents
            .iter()
            .map(|agent| agent.id)
            .collect::<Vec<_>>(),
        [responses]
    );
    assert!(
        page.subscriptions
            .iter()
            .all(|subscription| subscription.agent.id != chat),
        "a custom forwarded agent needs no subscription"
    );
}

#[tokio::test]
async fn a_profile_that_declares_a_sign_in_is_offered_with_its_methods_and_a_withheld_harness_is_listed_apart()
 {
    let world = World::new().await;
    let c = company(&world).await;
    let agent = agent_with_descriptor(&world, &c.boss, c.acme).await;
    // The viewer may use the profile, the stranger may not.
    assert!(
        setup(&world, &c.viewer, c.acme)
            .await
            .ok()
            .subscriptions
            .is_empty(),
        "not named yet"
    );
    name_person(&world, "agent", agent, "agent.use", c.viewer.id()).await;
    let page = setup(&world, &c.viewer, c.acme).await.ok();
    assert_eq!(page.subscriptions.len(), 1);
    let subscription = &page.subscriptions[0];
    assert_eq!(subscription.harness.kind, "fake");
    assert_eq!(subscription.harness.name, "Fake plan");
    assert_eq!(subscription.agent.id, agent);
    assert_eq!(subscription.methods[0].id, "token");
    assert!(subscription.methods[0].recommended);
    assert_eq!(
        subscription
            .arrangements
            .iter()
            .filter(|a| a.allowed)
            .count(),
        1
    );
    let responses = page
        .protocols
        .iter()
        .find(|p| p.name == "openai-responses")
        .unwrap();
    assert_eq!(
        responses.agents.iter().map(|a| a.id).collect::<Vec<_>>(),
        [agent]
    );
    assert!(page.subscriptions_withheld.is_empty());
    // A policy that does not accept that harness there moves it out of the list, with the place
    // that says so.
    world
        .set_at(
            Target::Node(c.acme),
            "providers.subscription_harnesses",
            json!(["codex"]),
        )
        .await;
    let withheld = setup(&world, &c.viewer, c.acme).await.ok();
    assert!(withheld.subscriptions.is_empty());
    assert_eq!(withheld.subscriptions_withheld.len(), 1);
    assert_eq!(withheld.subscriptions_withheld[0].harness.kind, "fake");
    assert_eq!(
        withheld.subscriptions_withheld[0]
            .scope
            .as_ref()
            .unwrap()
            .id,
        c.acme
    );
}

/// A vendor's reader as a program registers it, standing in for OpenRouter's: what the page offers of it is the
/// host it is the default for and what it declares, whichever reader the program carries.
fn stand_in_vendor() -> std::sync::Arc<orch_provider_contract::builtins::BuiltinSources> {
    use orch_provider_contract::builtins::{BuiltinSource, BuiltinSources, VendorClient};
    use orch_provider_contract::{
        FactMember, FactsReport, FactsSource, SourceAsk, SourceDescriptor, SourceError,
        SourceErrorCode,
    };
    use std::sync::Arc;
    struct Silent;
    #[async_trait::async_trait]
    impl FactsSource for Silent {
        fn describe(&self) -> SourceDescriptor {
            declared()
        }
        async fn read_facts(&self, _ask: SourceAsk) -> Result<FactsReport, SourceError> {
            Err(SourceError::new(
                SourceErrorCode::NotTold,
                "a stand-in tells nothing",
            ))
        }
    }
    fn declared() -> SourceDescriptor {
        let mut descriptor =
            SourceDescriptor::new("openrouter", "OpenRouter", "Reads the key's spending.");
        descriptor.facts = Some(orch_provider_contract::source::SourceDescriptorFacts {
            reports: vec![FactMember::Spend],
            ..Default::default()
        });
        descriptor
    }
    fn at_its_host(base: &url::Url) -> bool {
        base.host_str() == Some("openrouter.ai")
    }
    fn made(
        _client: Arc<dyn VendorClient>,
        _clock: Arc<dyn orch_types::Clock>,
    ) -> Arc<dyn FactsSource> {
        Arc::new(Silent)
    }
    let entry = BuiltinSource {
        name: "openrouter",
        hosts: &["openrouter.ai"],
        descriptor: declared,
        matches: at_its_host,
        make: made,
    };
    Arc::new(BuiltinSources::new([entry]).unwrap())
}

#[tokio::test]
async fn the_machines_that_may_keep_a_key_are_the_callers_own() {
    let world = World::with_builtin_sources(stand_in_vendor()).await;
    let c = company(&world).await;
    let mut conn = world.conn().await;
    let mine = insert_persons_machine(&mut conn, c.viewer.id(), "laptop")
        .await
        .unwrap();
    let theirs = insert_persons_machine(&mut conn, c.admin.id(), "theirs")
        .await
        .unwrap();
    let mut descriptor =
        orch_provider_contract::SourceDescriptor::new("plan", "Plan details", "Reports the plan.");
    descriptor.facts = Some(Default::default());
    // Both play the provider role, which keeping a key takes: only one is the caller's.
    for (machine, name) in [(mine, "my-plan"), (theirs, "their-plan")] {
        sqlx::query("UPDATE machines SET facts=$2, roles=ARRAY['provider'] WHERE id=$1")
            .bind(machine)
            .bind(json!({"plugins":[{"kind":"provider","name":name,"descriptor":descriptor}]}))
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    drop(conn);
    let page = setup(&world, &c.viewer, c.acme).await.ok();
    assert_eq!(
        page.key_machines.iter().map(|m| m.id).collect::<Vec<_>>(),
        [mine]
    );
    assert!(page.sources.iter().any(|source| {
        source.name == "my-plan"
            && source
                .machine
                .as_ref()
                .is_some_and(|machine| machine.id == mine)
            && source.descriptor.is_some()
    }));
    assert!(
        !page
            .sources
            .iter()
            .any(|source| source.name == "their-plan")
    );
    let openrouter = page
        .sources
        .iter()
        .find(|source| source.name == "openrouter")
        .unwrap();
    assert_eq!(openrouter.default_for, ["openrouter.ai"]);
    assert!(
        openrouter
            .descriptor
            .as_ref()
            .unwrap()
            .facts
            .as_ref()
            .unwrap()
            .reports
            .contains(&orch_api_contract::resources::FactMember::Spend)
    );
}

#[tokio::test]
async fn setup_offers_programs_only_from_provider_templates_the_caller_may_use_with_their_commands_until_described()
 {
    let world = World::new().await;
    let c = company(&world).await;
    let visible = orch_types::new_id();
    let hidden = orch_types::new_id();
    for (id, name, program) in [
        (visible, "Offered plan", "offered-plan"),
        (hidden, "Private plan", "private-plan"),
    ] {
        sqlx::query("INSERT INTO env_templates(id,owner_id,org_id,home_id,name,kind,driver,spec) VALUES($1,$2,$3,$3,$4,'provider','docker',$5)")
            .bind(id).bind(c.boss.id()).bind(c.acme).bind(name).bind(json!({"image":"example/plan","plugins":[{"kind":"provider","name":program,"command":["/opt/plan","--stdio"]}]})).execute(&world.db.pool).await.unwrap();
    }
    name_person(&world, "template", visible, "template.use", c.viewer.id()).await;
    let page = setup(&world, &c.viewer, c.acme).await.ok();
    let offer = page
        .sources
        .iter()
        .find(|source| {
            source
                .template
                .as_ref()
                .is_some_and(|template| template.id == visible)
        })
        .unwrap();
    assert_eq!(offer.name, "offered-plan");
    assert_eq!(
        offer.command.as_deref(),
        Some(&["/opt/plan".to_owned(), "--stdio".to_owned()][..])
    );
    assert!(offer.descriptor.is_none());
    assert!(!page.sources.iter().any(|source| {
        source
            .template
            .as_ref()
            .is_some_and(|template| template.id == hidden)
    }));
}
