//! How a key resolves at a place over a real database: which scope wins, how the merge modes combine, what a lock does, where each part comes from, and what it costs in statements.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_db::queries::hierarchy::testing::put_thread_in_environment;
use orch_db::queries::settings::testing::{
    bind_thread_to_provider, insert_provider, insert_provider_model, insert_system_mail_server,
};
use orch_db::queries::settings::{Stored, put};
use orch_db::seeds::{self, BuiltInAdapter, Release};
use orch_settings::validation::keys_at;
use orch_settings::{Error, Place, Target, get, resolve_all};
use orch_settings_contract::provenance::{LockScope, SourceScope};
use orch_settings_contract::registry::{Scope, keys};
use orch_types::new_id;
use serde_json::json;
use support::{Statements, World};

#[tokio::test]
async fn exact_host_lists_are_canonical_intersections_and_an_unset_list_allows_no_other_host() {
    let world = World::new().await;
    assert_eq!(
        world
            .value(Place::Node(world.topic), keys::NETWORK_ALLOWED_HOSTS)
            .await,
        None
    );
    world
        .set(
            Target::Node(world.root),
            keys::NETWORK_ALLOWED_HOSTS,
            json!(["PYPI.ORG.", "crates.io", "pypi.org"]),
        )
        .await;
    world
        .set(
            Target::Node(world.topic),
            keys::NETWORK_ALLOWED_HOSTS,
            json!(["pypi.org", "example.org"]),
        )
        .await;
    let view = world
        .view(Place::Node(world.topic), keys::NETWORK_ALLOWED_HOSTS)
        .await;
    assert_eq!(view.value(), Some(&json!(["pypi.org"])));
    assert_eq!(view.parts().len(), 2);
    world
        .set(
            Target::Node(world.project),
            keys::NETWORK_ALLOWED_HOSTS,
            json!([]),
        )
        .await;
    assert_eq!(
        world
            .value(Place::Node(world.topic), keys::NETWORK_ALLOWED_HOSTS)
            .await,
        Some(json!([]))
    );
}

#[tokio::test]
async fn the_most_specific_scope_wins_for_a_replace_key() {
    let w = World::new().await;
    w.set(Target::Node(w.root), keys::AGENT_MODEL, json!("model-a"))
        .await;
    w.set(Target::Node(w.project), keys::AGENT_MODEL, json!("model-b"))
        .await;

    let at_topic = w.view(Place::Node(w.topic), keys::AGENT_MODEL).await;
    assert_eq!(
        at_topic.value(),
        Some(&json!("model-b")),
        "a topic that sets nothing takes its project's"
    );
    assert_eq!(at_topic.resolved.source.scope, SourceScope::Project);
    assert_eq!(at_topic.resolved.source.node, Some(w.project));
    assert!(
        at_topic.own.is_none(),
        "the row at the place asked about is shown only when it is there"
    );

    w.set(Target::Node(w.thread), keys::AGENT_MODEL, json!("model-c"))
        .await;
    let at_thread = w.view(Place::Node(w.thread), keys::AGENT_MODEL).await;
    assert_eq!(at_thread.value(), Some(&json!("model-c")));
    assert_eq!(at_thread.resolved.source.scope, SourceScope::Thread);
    assert_eq!(at_thread.own.map(|own| own.value), Some(json!("model-c")));

    let at_root = w.view(Place::Node(w.root), keys::AGENT_MODEL).await;
    assert_eq!(
        at_root.value(),
        Some(&json!("model-a")),
        "what is set below does not reach up"
    );
}

#[tokio::test]
async fn of_two_orgs_on_a_path_the_inner_one_is_nearer() {
    let w = World::new().await;
    w.set(Target::Node(w.root), keys::CHAT_ENABLED, json!(false))
        .await;
    w.set(Target::Node(w.department), keys::CHAT_ENABLED, json!(true))
        .await;
    let sibling = w.node("project", w.department, "Sibling").await;
    let view = w.view(Place::Node(sibling), keys::CHAT_ENABLED).await;
    assert_eq!(view.value(), Some(&json!(true)));
    assert_eq!(view.resolved.source.node, Some(w.department));
    let elsewhere = w.node("project", w.root, "Elsewhere").await;
    assert_eq!(
        w.value(Place::Node(elsewhere), keys::CHAT_ENABLED).await,
        Some(json!(false)),
        "a project of the root org itself is not inside the department"
    );
}

#[tokio::test]
async fn a_list_key_joins_the_lists_of_every_scope_each_item_once() {
    let w = World::new().await;
    w.set(
        Target::Node(w.root),
        keys::WORKSPACE_IGNORE,
        json!(["target/", "*.log"]),
    )
    .await;
    w.set(
        Target::Node(w.project),
        keys::WORKSPACE_IGNORE,
        json!(["node_modules/", "*.log"]),
    )
    .await;
    w.set(
        Target::Node(w.topic),
        keys::WORKSPACE_IGNORE,
        json!(["dist/"]),
    )
    .await;
    assert_eq!(
        w.value(Place::Node(w.topic), keys::WORKSPACE_IGNORE).await,
        Some(json!(["target/", "*.log", "node_modules/", "dist/"])),
        "in order from the top, each once"
    );
    assert_eq!(
        w.value(Place::Node(w.project), keys::WORKSPACE_IGNORE)
            .await,
        Some(json!(["target/", "*.log", "node_modules/"]))
    );
}

#[tokio::test]
async fn an_intersect_key_keeps_what_every_scope_that_sets_it_names_and_unset_restricts_nothing() {
    let w = World::new().await;
    let all = json!(["ask", "on_request", "auto", "bypass"]);
    assert_eq!(
        w.value(Place::Node(w.topic), keys::APPROVAL_ALLOWED_MODES)
            .await,
        Some(all)
    );
    w.set(
        Target::Node(w.root),
        keys::APPROVAL_ALLOWED_MODES,
        json!(["ask", "on_request"]),
    )
    .await;
    w.set(
        Target::Node(w.project),
        keys::APPROVAL_ALLOWED_MODES,
        json!(["on_request", "auto"]),
    )
    .await;
    let view = w
        .view(Place::Node(w.topic), keys::APPROVAL_ALLOWED_MODES)
        .await;
    assert_eq!(view.value(), Some(&json!(["on_request"])));
    assert_eq!(
        view.parts().len(),
        2,
        "each list is kept for whoever asks them one by one"
    );
    w.set(Target::System, keys::APPROVAL_ALLOWED_MODES, json!(["ask"]))
        .await;
    assert_eq!(
        w.value(Place::Node(w.topic), keys::APPROVAL_ALLOWED_MODES)
            .await,
        Some(json!([])),
        "nothing is named by all three"
    );
}

#[tokio::test]
async fn a_map_key_merges_member_by_member_and_a_lower_scope_overrides_the_same_member() {
    let w = World::new().await;
    w.set(
        Target::Node(w.root),
        keys::ENV_VARIABLES,
        json!({ "CI": "true", "A": "1" }),
    )
    .await;
    w.set(
        Target::Node(w.topic),
        keys::ENV_VARIABLES,
        json!({ "CI": "false", "B": "2" }),
    )
    .await;
    assert_eq!(
        w.value(Place::Node(w.topic), keys::ENV_VARIABLES).await,
        Some(json!({ "CI": "false", "A": "1", "B": "2" }))
    );
    assert_eq!(
        w.value(Place::Node(w.project), keys::ENV_VARIABLES).await,
        Some(json!({ "CI": "true", "A": "1" }))
    );
}

#[tokio::test]
async fn standing_instructions_are_joined_from_the_top_down_with_one_blank_line() {
    let w = World::new().await;
    w.set(
        Target::Node(w.root),
        keys::AGENT_INSTRUCTIONS,
        json!("Write in English.\n"),
    )
    .await;
    w.set(Target::Node(w.project), keys::AGENT_INSTRUCTIONS, json!(""))
        .await;
    w.set(
        Target::Node(w.thread),
        keys::AGENT_INSTRUCTIONS,
        json!("\nKeep changes small."),
    )
    .await;
    let view = w
        .view(Place::Node(w.thread), keys::AGENT_INSTRUCTIONS)
        .await;
    assert_eq!(
        view.value(),
        Some(&json!("Write in English.\n\nKeep changes small."))
    );
    assert_eq!(
        view.parts().len(),
        2,
        "a scope that says nothing adds nothing"
    );
}

#[tokio::test]
async fn a_lock_above_decides_whatever_is_set_below_and_what_was_set_below_is_as_it_was_when_it_is_lifted()
 {
    let w = World::new().await;
    w.set(
        Target::Node(w.project),
        keys::APPROVAL_MODE,
        json!("on_request"),
    )
    .await;
    w.set(Target::Node(w.thread), keys::APPROVAL_MODE, json!("auto"))
        .await;
    let project_row = w
        .view(Place::Node(w.project), keys::APPROVAL_MODE)
        .await
        .own
        .unwrap();

    w.lock(Target::Node(w.root), keys::APPROVAL_MODE, json!("ask"))
        .await;
    let locked = w.view(Place::Node(w.thread), keys::APPROVAL_MODE).await;
    assert_eq!(locked.value(), Some(&json!("ask")));
    let lock = locked.resolved.locked_by.clone().unwrap();
    assert_eq!((lock.scope, lock.node), (LockScope::Org, Some(w.root)));
    assert_eq!(
        locked.parts().len(),
        1,
        "what is stored below a lock counts for nothing"
    );
    assert_eq!(
        locked.own.as_ref().map(|own| own.value.clone()),
        Some(json!("auto")),
        "it is still stored, and still shown as the thread's row"
    );
    assert!(locked.lock_above().is_some());

    w.try_set(
        Target::Node(w.root),
        keys::APPROVAL_MODE,
        json!("ask"),
        Some(false),
        None,
        true,
    )
    .await
    .unwrap();
    let lifted = w.view(Place::Node(w.thread), keys::APPROVAL_MODE).await;
    assert_eq!(
        lifted.value(),
        Some(&json!("auto")),
        "the thread's choice is back"
    );
    assert_eq!(lifted.resolved.locked_by, None);
    let after = w
        .view(Place::Node(w.project), keys::APPROVAL_MODE)
        .await
        .own
        .unwrap();
    assert_eq!(
        after, project_row,
        "the project's row was not touched by the lock or by lifting it"
    );
}

#[tokio::test]
async fn of_two_locks_the_one_further_up_decides_and_a_lock_does_not_bind_what_is_above_it() {
    let w = World::new().await;
    w.set(Target::Node(w.root), keys::TERMINAL_ENABLED, json!(true))
        .await;
    w.lock(
        Target::Node(w.project),
        keys::TERMINAL_ENABLED,
        json!(false),
    )
    .await;
    assert_eq!(
        w.value(Place::Node(w.topic), keys::TERMINAL_ENABLED).await,
        Some(json!(false))
    );
    assert_eq!(
        w.value(Place::Node(w.root), keys::TERMINAL_ENABLED).await,
        Some(json!(true))
    );
    w.lock(Target::System, keys::TERMINAL_ENABLED, json!(true))
        .await;
    let view = w.view(Place::Node(w.topic), keys::TERMINAL_ENABLED).await;
    assert_eq!(
        view.value(),
        Some(&json!(true)),
        "the system's lock is further up"
    );
    assert_eq!(
        view.resolved.locked_by.map(|lock| lock.scope),
        Some(LockScope::System)
    );
}

#[tokio::test]
async fn the_provenance_names_the_scope_each_part_came_from() {
    let w = World::new().await;
    w.set(Target::Node(w.root), keys::WORKSPACE_IGNORE, json!(["a"]))
        .await;
    w.set(
        Target::Node(w.project),
        keys::WORKSPACE_IGNORE,
        json!(["b"]),
    )
    .await;
    w.set(Target::Node(w.topic), keys::WORKSPACE_IGNORE, json!(["c"]))
        .await;
    let view = w.view(Place::Node(w.topic), keys::WORKSPACE_IGNORE).await;
    let parts: Vec<_> = view
        .parts()
        .iter()
        .map(|part| (part.source.scope, part.source.node, part.value.clone()))
        .collect();
    assert_eq!(
        parts,
        vec![
            (SourceScope::Org, Some(w.root), json!(["a"])),
            (SourceScope::Project, Some(w.project), json!(["b"])),
            (SourceScope::Topic, Some(w.topic), json!(["c"])),
        ]
    );
    assert_eq!(
        view.resolved.source.scope,
        SourceScope::Topic,
        "the nearest scope that added to it"
    );
    let mut named = view.nodes();
    named.sort_unstable();
    let mut expected = vec![w.root, w.project, w.topic];
    expected.sort_unstable();
    assert_eq!(
        named, expected,
        "the nodes to look up once for a whole list"
    );
}

#[tokio::test]
async fn a_side_thread_has_the_settings_of_its_parent() {
    let w = World::new().await;
    w.set(Target::Node(w.thread), keys::AGENT_MODEL, json!("model-c"))
        .await;
    let side = w.node("thread", w.thread, "Fix (side)").await;
    let view = w.view(Place::Node(side), keys::AGENT_MODEL).await;
    assert_eq!(view.value(), Some(&json!("model-c")));
    assert_eq!(
        view.resolved.source.node,
        Some(w.thread),
        "it is the parent's row that holds"
    );
}

#[tokio::test]
async fn a_default_the_environment_supplies_sits_under_every_row_and_only_for_a_thread_in_one() {
    let w = World::new().await;
    assert_eq!(
        w.value(Place::Node(w.thread), keys::APPROVAL_MODE).await,
        None,
        "a thread in no environment has no mode of its own to start from"
    );
    let spec =
        json!({ "agent_defaults": { "approval.mode": "auto" }, "sandbox": { "default": "open" } });
    w.managed_environment(w.thread, w.topic, &spec, &json!({}))
        .await;
    let supplied = w.view(Place::Node(w.thread), keys::APPROVAL_MODE).await;
    assert_eq!(supplied.value(), Some(&json!("auto")));
    assert_eq!(supplied.resolved.source.scope, SourceScope::Environment);
    assert_eq!(supplied.resolved.source.node, None);
    assert_eq!(
        w.value(Place::Node(w.thread), keys::SANDBOX_MODE).await,
        Some(json!("open"))
    );

    w.set(Target::Node(w.project), keys::APPROVAL_MODE, json!("ask"))
        .await;
    let overridden = w.view(Place::Node(w.thread), keys::APPROVAL_MODE).await;
    assert_eq!(
        overridden.value(),
        Some(&json!("ask")),
        "any row overrides it"
    );
    assert_eq!(overridden.resolved.source.scope, SourceScope::Project);
    assert_eq!(
        w.value(Place::Node(w.topic), keys::SANDBOX_MODE).await,
        None,
        "a topic runs nothing, so no environment supplies it a sandbox"
    );
}

#[tokio::test]
async fn a_thread_on_a_persons_machine_starts_from_the_machines_defaults() {
    let w = World::new().await;
    let mut conn = w.conn().await;
    put_thread_in_environment(&mut conn, w.thread, "connected", w.ada, w.topic)
        .await
        .unwrap();
    drop(conn);
    assert_eq!(
        w.value(Place::Node(w.thread), keys::APPROVAL_MODE).await,
        Some(json!("on_request"))
    );
    assert_eq!(
        w.value(Place::Node(w.thread), keys::SANDBOX_MODE).await,
        Some(json!("workspace"))
    );
}

#[tokio::test]
async fn an_environments_own_values_come_last_and_a_lock_above_takes_the_choice_from_it() {
    let w = World::new().await;
    let spec = json!({ "exec": { "offers": ["plain"], "orch_tools": true } });
    let environment = w
        .managed_environment(w.thread, w.topic, &spec, &json!({ "env.update": "manual" }))
        .await;
    w.set(Target::Node(w.project), keys::ENV_UPDATE, json!("on_start"))
        .await;

    let own = w
        .view(Place::Environment(environment), keys::ENV_UPDATE)
        .await;
    assert_eq!(own.value(), Some(&json!("manual")));
    assert_eq!(own.resolved.source.scope, SourceScope::EnvironmentOwn);
    assert_eq!(
        own.own.as_ref().map(|row| row.value.clone()),
        Some(json!("manual"))
    );
    assert!(own.lock_above().is_none());

    let offers = w
        .view(Place::Environment(environment), keys::EXEC_OFFERS)
        .await;
    assert_eq!(
        offers.value(),
        Some(&json!(["plain"])),
        "its template says what its exec node offers"
    );
    assert_eq!(offers.resolved.source.scope, SourceScope::Environment);

    w.lock(Target::Node(w.root), keys::ENV_UPDATE, json!("on_start"))
        .await;
    let locked = w
        .view(Place::Environment(environment), keys::ENV_UPDATE)
        .await;
    assert_eq!(
        locked.value(),
        Some(&json!("on_start")),
        "a lock above takes the choice from the environment"
    );
    assert_eq!(
        locked
            .resolved
            .locked_by
            .as_ref()
            .and_then(|lock| lock.node),
        Some(w.root)
    );
    assert!(
        !locked.clone().into_setting(true).writable,
        "and its own value cannot be written under it"
    );
    assert_eq!(
        locked.own.as_ref().map(|row| row.value.clone()),
        Some(json!("manual")),
        "its own value stays as it was"
    );
}

#[tokio::test]
async fn the_keys_of_an_environment_are_those_of_its_scope_and_its_home_decides_the_rest() {
    let w = World::new().await;
    let environment = w
        .managed_environment(w.thread, w.topic, &json!({}), &json!({}))
        .await;
    w.set(
        Target::Node(w.department),
        keys::ENV_UPDATE,
        json!("on_start"),
    )
    .await;
    let other = w.node("project", w.root, "Other").await;
    w.set(Target::Node(other), keys::ENV_UPDATE, json!("manual"))
        .await;
    let mut conn = w.conn().await;
    let views = resolve_all(&mut conn, Place::Environment(environment))
        .await
        .unwrap();
    let listed: Vec<_> = views.iter().map(|view| view.key()).collect();
    assert!(listed.contains(&keys::EXEC_OFFERS) && listed.contains(&keys::ENV_UPDATE));
    assert!(
        !listed.contains(&keys::AGENT_MODEL),
        "that is not a key of an environment"
    );
    let update = views
        .iter()
        .find(|view| view.key() == keys::ENV_UPDATE)
        .unwrap();
    assert_eq!(
        update.value(),
        Some(&json!("on_start")),
        "what the path of its home node resolves"
    );
    assert_eq!(update.resolved.source.node, Some(w.department));
}

#[tokio::test]
async fn a_providers_runtime_resolves_through_the_orgs_down_to_its_home_and_then_its_own_values() {
    let w = World::new().await;
    let provider = new_id();
    let mut conn = w.conn().await;
    insert_provider(&mut conn, provider, "Key", w.ada, w.department, &json!({}))
        .await
        .unwrap();
    drop(conn);
    w.set(Target::System, keys::RUNTIME_RENEW_DAYS, json!(5))
        .await;
    w.set(Target::Node(w.root), keys::RUNTIME_RENEW_DAYS, json!(3))
        .await;
    let inherited = w
        .view(Place::Provider(provider), keys::RUNTIME_RENEW_DAYS)
        .await;
    assert_eq!(inherited.value(), Some(&json!(3)));
    assert_eq!(inherited.resolved.source.node, Some(w.root));

    sqlx::query(
        "UPDATE providers SET runtime_config = '{\"runtime.renew_days\": 1}' WHERE id = $1",
    )
    .bind(provider)
    .execute(&w.db.pool)
    .await
    .unwrap();
    let own = w
        .view(Place::Provider(provider), keys::RUNTIME_RENEW_DAYS)
        .await;
    assert_eq!(own.value(), Some(&json!(1)));
    assert_eq!(own.resolved.source.scope, SourceScope::ProviderOwn);
    let sibling = w.node("project", w.department, "Sibling").await;
    assert_eq!(
        w.value(Place::Node(sibling), keys::RUNTIME_RENEW_DAYS)
            .await,
        Some(json!(3)),
        "a project sees the orgs' value, not the provider's own"
    );
}

#[tokio::test]
async fn what_the_system_was_seeded_with_stands_where_the_registrys_default_would() {
    let w = World::new().await;
    let adapters = [BuiltInAdapter {
        kind: "codex",
        title: "Codex",
        colocated: true,
    }];
    let release = Release {
        version: "0.1.0",
        adapters: &adapters,
    };
    let mut conn = w.conn().await;
    seeds::seed(&mut conn, &release, World::now())
        .await
        .unwrap();
    drop(conn);

    let template = w.view(Place::Node(w.topic), keys::ENV_TEMPLATE_ID).await;
    assert_eq!(template.value(), Some(&json!(seeds::DEFAULT_TEMPLATE_ID)));
    assert_eq!(template.resolved.source.scope, SourceScope::Default);
    let chosen = new_id();
    w.set(
        Target::Node(w.project),
        keys::ENV_TEMPLATE_ID,
        json!(chosen),
    )
    .await;
    assert_eq!(
        w.value(Place::Node(w.topic), keys::ENV_TEMPLATE_ID).await,
        Some(json!(chosen))
    );
    assert_eq!(
        w.value(Place::System, keys::RUNTIME_TEMPLATE_ID).await,
        Some(json!(seeds::PROVIDER_RUNTIME_TEMPLATE_ID))
    );

    let policy = w
        .value(Place::Node(w.project), keys::CAPABILITY_POLICY)
        .await
        .unwrap();
    let mut conn = w.db.pool.acquire().await.unwrap();
    let definitions: i64 = sqlx::query_scalar("SELECT count(*) FROM capability_defs")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    assert_eq!(
        policy.as_object().map(serde_json::Map::len),
        usize::try_from(definitions).ok(),
        "one for each definition"
    );
    for (key, decision) in [
        ("port_place", "ask"),
        ("port_host", "deny"),
        ("port_link", "deny"),
    ] {
        assert_eq!(
            policy[key],
            json!({"decision":decision,"approver_scope":"environment"})
        );
    }
    assert_eq!(
        policy["privileged"],
        json!({ "decision": "deny", "approver_scope": "system" })
    );
    w.set(
        Target::Node(w.root),
        keys::CAPABILITY_POLICY,
        json!({ "gpu": { "decision": "deny", "approver_scope": "org" } }),
    )
    .await;
    let policy = w
        .view(Place::Node(w.project), keys::CAPABILITY_POLICY)
        .await;
    assert_eq!(
        policy.value().unwrap()["gpu"]["decision"],
        "deny",
        "an org overrides one member"
    );
    assert_eq!(
        policy.value().unwrap()["ptrace"]["decision"],
        "allow",
        "and leaves the rest as seeded"
    );

    w.set(
        Target::Node(w.root),
        keys::PROVIDERS_PRESETS,
        json!({ "OpenAI": null }),
    )
    .await;
    let presets = w
        .value(Place::Node(w.department), keys::PROVIDERS_PRESETS)
        .await
        .unwrap();
    assert_eq!(
        presets["OpenAI"],
        serde_json::Value::Null,
        "hidden by the org that says so"
    );
    assert!(
        presets["Anthropic"]["upstreams"].is_array() && presets["DeepSeek"]["upstreams"].is_array()
    );
    assert!(
        w.value(Place::System, keys::PROVIDERS_PRESETS)
            .await
            .unwrap()["OpenAI"]
            .is_object()
    );
}

#[tokio::test]
async fn an_address_has_to_be_confirmed_where_the_system_has_a_mail_server_to_send_the_mail() {
    let w = World::new().await;
    assert_eq!(
        w.value(Place::System, keys::REGISTRATION_EMAIL_VERIFICATION)
            .await,
        Some(json!("off"))
    );
    let mut conn = w.conn().await;
    insert_system_mail_server(&mut conn, w.ada, true)
        .await
        .unwrap();
    drop(conn);
    let view = w
        .view(Place::System, keys::REGISTRATION_EMAIL_VERIFICATION)
        .await;
    assert_eq!(view.value(), Some(&json!("required")));
    w.set(
        Target::System,
        keys::REGISTRATION_EMAIL_VERIFICATION,
        json!("off"),
    )
    .await;
    assert_eq!(
        w.value(Place::System, keys::REGISTRATION_EMAIL_VERIFICATION)
            .await,
        Some(json!("off")),
        "the system's row beats what the installation supplies"
    );
}

#[tokio::test]
async fn a_threads_effort_is_the_default_effort_its_model_lists_unless_something_sets_it() {
    let w = World::new().await;
    let provider = new_id();
    let mut conn = w.conn().await;
    insert_provider(&mut conn, provider, "Key", w.ada, w.root, &json!({}))
        .await
        .unwrap();
    let parameters = json!({ "efforts": ["low", "high"], "default_effort": "low" });
    insert_provider_model(&mut conn, provider, "gpt-x", &parameters, w.ada)
        .await
        .unwrap();
    bind_thread_to_provider(&mut conn, w.thread, provider)
        .await
        .unwrap();
    drop(conn);

    assert_eq!(
        w.value(Place::Node(w.thread), keys::AGENT_EFFORT).await,
        None,
        "no model, no effort"
    );
    w.set(Target::Node(w.thread), keys::AGENT_MODEL, json!("gpt-x"))
        .await;
    let effort = w.view(Place::Node(w.thread), keys::AGENT_EFFORT).await;
    assert_eq!(effort.value(), Some(&json!("low")));
    assert_eq!(effort.resolved.source.scope, SourceScope::Default);
    w.set(Target::Node(w.thread), keys::AGENT_EFFORT, json!("high"))
        .await;
    assert_eq!(
        w.value(Place::Node(w.thread), keys::AGENT_EFFORT).await,
        Some(json!("high"))
    );
    w.set(Target::Node(w.thread), keys::AGENT_MODEL, json!("unlisted"))
        .await;
    w.try_clear(Target::Node(w.thread), keys::AGENT_EFFORT, None, true)
        .await
        .unwrap();
    assert_eq!(
        w.value(Place::Node(w.thread), keys::AGENT_EFFORT).await,
        None,
        "a model the provider does not list has none"
    );
}

#[tokio::test]
async fn a_default_that_names_another_key_follows_that_keys_value() {
    let w = World::new().await;
    assert_eq!(
        w.value(Place::Node(w.project), keys::DOCS_FILE_MAX_BYTES)
            .await,
        Some(json!(8_589_934_592_i64)),
        "the largest file Orch stores"
    );
    w.set(
        Target::System,
        keys::STORAGE_MAX_BLOB_BYTES,
        json!(1_000_000),
    )
    .await;
    let view = w
        .view(Place::Node(w.project), keys::DOCS_FILE_MAX_BYTES)
        .await;
    assert_eq!(view.value(), Some(&json!(1_000_000)));
    assert_eq!(
        view.resolved.source.scope,
        SourceScope::Default,
        "it is still a default of the key"
    );
    w.set(Target::Node(w.project), keys::DOCS_FILE_MAX_BYTES, json!(5))
        .await;
    assert_eq!(
        w.value(Place::Node(w.topic), keys::DOCS_FILE_MAX_BYTES)
            .await,
        Some(json!(5))
    );
}

#[tokio::test]
async fn a_typed_read_gives_the_value_as_the_caller_knows_it_and_nothing_for_an_unset_key() {
    let w = World::new().await;
    let mut conn = w.conn().await;
    let shells: Option<i64> = get(
        &mut conn,
        Place::Node(w.topic),
        keys::TERMINAL_MAX_PER_PERSON,
    )
    .await
    .unwrap();
    assert_eq!(
        shells,
        Some(4),
        "the registry's default, with nobody having set it"
    );
    let mode: Option<String> = get(&mut conn, Place::System, keys::REGISTRATION_MODE)
        .await
        .unwrap();
    assert_eq!(mode.as_deref(), Some("invite"));
    let model: Option<String> = get(&mut conn, Place::Node(w.thread), keys::AGENT_MODEL)
        .await
        .unwrap();
    assert_eq!(model, None, "there is no model until somebody chooses one");
    drop(conn);

    w.set(Target::System, keys::REGISTRATION_MODE, json!("open"))
        .await;
    let mut conn = w.conn().await;
    let mode: Option<String> = get(&mut conn, Place::System, keys::REGISTRATION_MODE)
        .await
        .unwrap();
    assert_eq!(mode.as_deref(), Some("open"));
    let wrong = get::<bool>(&mut conn, Place::System, keys::REGISTRATION_MODE).await;
    assert!(
        matches!(wrong, Err(Error::NotOfThatType { .. })),
        "{wrong:?}"
    );
    let unknown = get::<bool>(&mut conn, Place::System, "registration.nope").await;
    assert!(matches!(unknown, Err(Error::UnknownKey(_))));
    let gone = get::<bool>(&mut conn, Place::Node(new_id()), keys::CHAT_ENABLED).await;
    assert!(matches!(gone, Err(Error::NotFound)));
}

#[tokio::test]
async fn a_node_shows_the_keys_set_there_or_below_each_with_its_own_row() {
    let w = World::new().await;
    w.set(Target::Node(w.project), keys::AGENT_MODEL, json!("model-b"))
        .await;
    let mut conn = w.conn().await;
    let views = resolve_all(&mut conn, Place::Node(w.project))
        .await
        .unwrap();
    let listed: Vec<&str> = views.iter().map(|view| view.key()).collect();
    let expected: Vec<&str> = keys_at(Scope::Project).map(|def| def.key).collect();
    assert_eq!(listed, expected, "in the order of the registry");
    let model = views
        .iter()
        .find(|view| view.key() == keys::AGENT_MODEL)
        .unwrap();
    assert_eq!(model.own.as_ref().map(|own| own.revision), Some(1));
    let cwd = views
        .iter()
        .find(|view| view.key() == keys::THREAD_CWD)
        .unwrap();
    assert!(
        !cwd.clone().into_setting(true).writable,
        "it is a key of threads, shown here for what they start from"
    );
    let system = resolve_all(&mut conn, Place::System).await.unwrap();
    assert!(system.iter().all(|view| view.def().set_at(Scope::System)));
}

/// A raw row, as a registry that changed since it was written could leave one.
async fn raw_row(w: &World, node: Option<orch_types::Id>, key: &str) {
    let mut conn = w.conn().await;
    let value = json!("x");
    let stored = Stored {
        value: &value,
        locked: false,
        by: None,
        at: World::now(),
    };
    put(&mut conn, node, key, &stored, None).await.unwrap();
}

#[tokio::test]
async fn resolving_every_key_of_a_node_takes_the_same_statements_whatever_the_number_of_keys_set() {
    let w = World::new().await;
    let mut conn = w.conn().await;
    let start = Statements::now();
    let few = resolve_all(&mut conn, Place::Node(w.topic)).await.unwrap();
    let few_cost = Statements::now() - start;
    drop(conn);

    let chain = [
        (w.root, Scope::Org),
        (w.department, Scope::Org),
        (w.project, Scope::Project),
        (w.topic, Scope::Topic),
    ];
    for def in keys_at(Scope::Topic) {
        for (node, scope) in chain {
            if def.set_at(scope) {
                raw_row(&w, Some(node), def.key).await;
            }
        }
        if def.set_at(Scope::System) {
            raw_row(&w, None, def.key).await;
        }
    }
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM settings")
        .fetch_one(&w.db.pool)
        .await
        .unwrap();
    assert!(
        rows > 150,
        "{rows} rows are set, so the second reading has plenty to read"
    );

    let mut conn = w.conn().await;
    let start = Statements::now();
    let many = resolve_all(&mut conn, Place::Node(w.topic)).await.unwrap();
    let many_cost = Statements::now() - start;
    assert_eq!(few.len(), many.len());
    assert_eq!(
        few_cost, many_cost,
        "{few_cost} statements for none of the keys set, {many_cost} for all of them"
    );
    assert!(
        many_cost <= 4,
        "the node's path, its rows, and the capability definitions: {many_cost}"
    );
}

#[tokio::test]
async fn a_key_nobody_set_costs_no_statement_beyond_the_rows_of_the_chain() {
    let w = World::new().await;
    let mut conn = w.conn().await;
    let start = Statements::now();
    let view = orch_settings::resolve(
        &mut conn,
        Place::Node(w.topic),
        keys::TERMINAL_MAX_PER_PERSON,
    )
    .await
    .unwrap();
    assert_eq!(view.value(), Some(&json!(4)));
    assert_eq!(
        Statements::now() - start,
        2,
        "the nodes on the way, and their rows"
    );
    let start = Statements::now();
    orch_settings::resolve(&mut conn, Place::System, keys::REGISTRATION_MODE)
        .await
        .unwrap();
    assert_eq!(Statements::now() - start, 1, "the system's rows");
}
