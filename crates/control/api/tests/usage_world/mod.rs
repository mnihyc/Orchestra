//! A small world for the tests of usage over HTTP: a team with a project, a topic, a thread and a provider, the people who stand in different places to it, and rows of the ledger written as their writer writes them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

use crate::places::{Who, admin, make_project, make_topic};
use crate::resources::company;
use crate::support::World;
use chrono::{DateTime, Duration, Utc};
use orch_api_contract::resources::{CreateProvider, ProviderCreate, Upstream};
use orch_api_core::routes::resources::sealing::install_master_key;
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::secrets::MasterKey;
use orch_db::usage::ledger::{self, Attribution, NewEvent, Tokens};
use orch_types::{Id, Json, new_id};
use serde_json::json;

/// The people and places of a test of usage. The boss owns the team's org, its admin holds `usage.view`
/// and `usage.manage` as shipped, its viewer holds `usage.summary` and nothing else of usage, and the
/// stranger sees nothing of the team.
pub struct Team {
    pub boss: Who,
    pub admin: Who,
    pub viewer: Who,
    pub stranger: Who,
    pub sysadmin: Who,
    pub acme: Id,
    pub project: Id,
    pub topic: Id,
    pub thread: Id,
    pub provider: Id,
}

/// Makes the team, its project `Billing`, the topic `Invoices` and the thread `Fix totals` in it, and an API
/// provider of the boss's.
pub async fn team(world: &World) -> Team {
    install_master_key(MasterKey::new([7; 32]));
    let c = company(world).await;
    let sysadmin = admin(world, "root").await;
    let project = make_project(world, &c.boss, c.acme, "Billing")
        .await
        .base
        .id;
    let topic = make_topic(world, &c.boss, project, "Invoices")
        .await
        .base
        .id;
    let thread = new_id();
    {
        let mut conn = world.conn().await;
        insert_node(
            &mut conn,
            thread,
            "thread",
            Some(topic),
            "Fix totals",
            c.boss.id(),
        )
        .await
        .unwrap();
    }
    let provider = world
        .app
        .call(ProviderCreate, &[])
        .token(&c.boss.token)
        .body(&CreateProvider::Api {
            runtime_machine_id: None,
            name: "Team key".to_owned(),
            home_id: c.acme,
            access: None,
            arrangements: None,
            runtime_config: None,
            upstreams: vec![Upstream {
                protocol: "openai-responses".to_owned(),
                base_url: "https://api.example.com/v1".to_owned(),
                options: json!({}),
            }],
            api_key: Some("sk-test".to_owned()),
            secrets: None,
            key_machine_id: None,
            sources: None,
        })
        .send()
        .await
        .ok()
        .provider
        .base
        .id;
    Team {
        boss: c.boss,
        admin: c.admin,
        viewer: c.viewer,
        stranger: c.stranger,
        sysadmin,
        acme: c.acme,
        project,
        topic,
        thread,
        provider,
    }
}

/// A use of a model to write into the ledger.
#[derive(Debug, Clone)]
pub struct Use<'a> {
    /// The person it is put down to.
    pub by: Option<Id>,
    /// When.
    pub at: DateTime<Utc>,
    /// The model.
    pub model: &'a str,
    /// The tokens, input and output together.
    pub tokens: i64,
    /// The credits it came to, where it had a rate.
    pub credits: Option<&'a str>,
    /// Where the rate came from.
    pub source: Option<&'a str>,
    /// Whether it counts.
    pub counted: bool,
}

impl<'a> Use<'a> {
    /// A counted use of `tokens` by `by` an hour ago, of a model that has no rate.
    pub fn of(world: &World, by: Id, tokens: i64) -> Self {
        Self {
            by: Some(by),
            at: world.now() - Duration::hours(1),
            model: "gpt-6-sol",
            tokens,
            credits: None,
            source: None,
            counted: true,
        }
    }

    pub fn at(mut self, at: DateTime<Utc>) -> Self {
        self.at = at;
        self
    }

    pub fn model(mut self, model: &'a str) -> Self {
        self.model = model;
        self
    }

    /// Counted at a rate that `source` names, coming to `credits`.
    pub fn rated(mut self, credits: &'a str, source: &'a str) -> Self {
        self.credits = Some(credits);
        self.source = Some(source);
        self
    }

    /// Kept as a trace, not counted.
    pub fn trace(mut self) -> Self {
        self.counted = false;
        self
    }
}

/// The rate a use is counted at, as a row of the ledger keeps it.
pub fn rate() -> Json {
    json!({ "input_per_mtok": "2", "cached_input_per_mtok": "0.2", "output_per_mtok": "10" })
}

/// Writes a use into the ledger as the writer does, which keeps the rollups too. The turn it belongs to is
/// returned.
pub async fn spend(world: &World, team: &Team, using: &Use<'_>) -> Id {
    spend_in(world, team, team.thread, using).await
}

/// As [`spend`], in another thread of the team.
pub async fn spend_in(world: &World, team: &Team, thread: Id, using: &Use<'_>) -> Id {
    let turn = new_id();
    let place = Attribution {
        org_id: team.acme,
        project_id: team.project,
        topic_id: team.topic,
        thread_id: thread,
        segment_id: new_id(),
        turn_id: turn,
        provider_id: team.provider,
        entry_id: None,
        user_id: using.by,
        agent_id: None,
        harness_kind: Some("codex".to_owned()),
        environment_id: None,
        environment_kind: Some("managed".to_owned()),
        arrangement: None,
        turn_model: using.model.to_owned(),
        effort: None,
        params: Json::Null,
    };
    let output = using.tokens / 4;
    let rate = rate();
    let mut conn = world.conn().await;
    ledger::record(
        &mut conn,
        &NewEvent {
            id: new_id(),
            at: using.at,
            origin: ledger::origin::FORWARDER,
            scope: ledger::scope::REQUEST,
            counted: using.counted,
            attribution: &place,
            model: using.model,
            tier: None,
            tokens: Tokens {
                input: using.tokens - output,
                cached_input: 0,
                cache_write: 0,
                output,
                reasoning: 0,
                total: using.tokens,
            },
            context_tokens: None,
            request_ref: None,
            rate: using.credits.map(|_| &rate),
            rate_source: using.source,
            credits: using.credits,
        },
    )
    .await
    .unwrap();
    turn
}

/// Another thread of the topic.
pub async fn another_thread(world: &World, team: &Team, name: &str) -> Id {
    let thread = new_id();
    let mut conn = world.conn().await;
    insert_node(
        &mut conn,
        thread,
        "thread",
        Some(team.topic),
        name,
        team.boss.id(),
    )
    .await
    .unwrap();
    thread
}

/// The span the tests read: the first of October to the day of the clock.
pub const FROM: &str = "2026-10-01";
/// The day of the clock of a test.
pub const TODAY: &str = "2026-10-08";
