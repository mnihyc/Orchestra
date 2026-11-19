//! Settings and secrets of a node over HTTP: the registry, what holds at a node and why, writing and clearing, locks, and secrets by name.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::settings::{
    NodeSecretDelete, NodeSecretList, NodeSecretSet, NodeSecretSetBody, NodeSettingDelete,
    NodeSettingList, NodeSettingSet, NodeSettingSetBody, ResolvedSetting, SettingClass,
    SettingDisplayZero, SettingRegistryGet, SettingScope,
};
use orch_db::secrets::MasterKey;
use orch_types::Id;
use places::{Who, join, make_org, make_project, make_topic, person};
use serde_json::{Value, json};
use support::{Statements, World};

struct Setup {
    boss: Who,
    dan: Who,
    acme: Id,
    web: Id,
}

async fn setup(world: &World) -> Setup {
    orch_api_core::routes::settings::install_master_key(MasterKey::new([9; 32]));
    let boss = person(world, "boss").await;
    let dan = person(world, "dan").await;
    let acme = make_org(world, &boss, "Acme", "acme").await;
    join(world, acme.base.id, acme.base.id, &dan, "viewer", &boss).await;
    let web = make_project(world, &boss, acme.base.id, "Web").await;
    Setup {
        boss,
        dan,
        acme: acme.base.id,
        web: web.base.id,
    }
}

async fn settings(world: &World, who: &Who, node: Id) -> Vec<ResolvedSetting> {
    world
        .app
        .call(NodeSettingList, &[&node])
        .token(&who.token)
        .send()
        .await
        .ok()
}

fn one<'a>(list: &'a [ResolvedSetting], key: &str) -> &'a ResolvedSetting {
    list.iter().find(|setting| setting.key == key).unwrap()
}

async fn put(
    world: &World,
    who: &Who,
    node: Id,
    key: &str,
    body: &NodeSettingSetBody,
) -> orch_api_core::testing::Reply<ResolvedSetting> {
    world
        .app
        .call(NodeSettingSet, &[&node, &key])
        .token(&who.token)
        .body(body)
        .send()
        .await
}

fn value(value: Value) -> NodeSettingSetBody {
    NodeSettingSetBody {
        value,
        locked: None,
        expected_revision: None,
    }
}

#[tokio::test]
async fn the_registry_lists_every_key_with_its_class_and_where_it_is_written() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let defs = world
        .app
        .call(SettingRegistryGet, &[])
        .token(&ada.token)
        .send()
        .await
        .ok();
    let chat = defs.iter().find(|def| def.key == "chat.enabled").unwrap();
    assert_eq!(chat.class, SettingClass::Policy);
    assert!(chat.scopes.contains(&"org".to_owned()));
    assert_eq!(chat.default, Some(json!(true)));
    assert!(chat.schema.is_object());
    assert!(defs.len() > 50, "the whole registry: {}", defs.len());
    for key in [
        "env.idle_stop_minutes",
        "retention.delete_archived_days",
        "agents.harness_channel_hours",
    ] {
        let def = defs.iter().find(|def| def.key == key).unwrap();
        assert_eq!(
            def.display.as_ref().unwrap().zero,
            Some(SettingDisplayZero::Off),
            "{key}"
        );
    }
    let immediate = defs
        .iter()
        .find(|def| def.key == "env.removed_keep_days")
        .unwrap();
    assert_eq!(immediate.display.as_ref().unwrap().zero, None);
    let history = defs
        .iter()
        .find(|def| def.key == "retention.agent_history_idle_days")
        .unwrap();
    assert_eq!(history.default, Some(json!(7)));
    assert_eq!(
        history.scopes,
        ["system", "org", "project", "topic", "thread"]
    );
    assert_eq!(history.schema["minimum"], 0);
    assert_eq!(
        history.display.as_ref().unwrap().zero,
        Some(orch_api_contract::settings::SettingDisplayZero::Off)
    );
}

#[tokio::test]
async fn a_node_lists_what_holds_there_and_whether_the_caller_may_write_it() {
    let world = World::new().await;
    let s = setup(&world).await;
    let boss = settings(&world, &s.boss, s.web).await;
    let chat = one(&boss, "chat.enabled");
    assert_eq!(chat.value, Some(json!(true)));
    assert_eq!(chat.source.scope, SettingScope::Default);
    assert!(chat.own.is_none() && chat.locked_by.is_none());
    assert!(chat.writable, "an owner makes the rules of the place");
    let dan = settings(&world, &s.dan, s.web).await;
    assert!(
        !one(&dan, "chat.enabled").writable,
        "a viewer writes nothing"
    );
    // A stranger reads nothing.
    let eve = person(&world, "eve").await;
    let refused = world
        .app
        .call(NodeSettingList, &[&s.web])
        .token(&eve.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn writing_a_key_at_an_org_is_seen_below_with_where_it_comes_from_and_leaves_its_row() {
    let world = World::new().await;
    let s = setup(&world).await;
    let written = put(
        &world,
        &s.boss,
        s.acme,
        "chat.enabled",
        &value(json!(false)),
    )
    .await
    .ok();
    assert_eq!(written.value, Some(json!(false)));
    assert_eq!(written.source.scope, SettingScope::Org);
    assert_eq!(written.source.node.as_ref().unwrap().id, s.acme);
    let own = written.own.as_ref().unwrap();
    assert_eq!(
        (own.value.clone(), own.locked, own.revision),
        (json!(false), false, 1)
    );
    let below = settings(&world, &s.dan, s.web).await;
    let chat = one(&below, "chat.enabled");
    assert_eq!(chat.value, Some(json!(false)));
    assert_eq!(chat.source.scope, SettingScope::Org);
    assert_eq!(
        chat.source.node.as_ref().unwrap().name.as_deref(),
        Some("Acme")
    );
    assert!(chat.own.is_none());
    assert!(
        world
            .audit()
            .await
            .iter()
            .any(|action| action == "settings.updated")
    );
}

#[tokio::test]
async fn a_viewer_writes_no_key_and_is_told_what_is_missing() {
    let world = World::new().await;
    let s = setup(&world).await;
    let refused = put(&world, &s.dan, s.web, "chat.enabled", &value(json!(false)))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "settings.manage");
    assert_eq!(refused.extra["missing"][0]["ask"][0]["username"], "boss");
    let refused = world
        .app
        .call(NodeSettingDelete, &[&s.web, &"chat.enabled"])
        .token(&s.dan.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.extra["required"], "settings.manage");
}

#[tokio::test]
async fn a_topic_developer_changes_parameters_but_not_policies_or_locks_and_the_list_agrees() {
    let world = World::new().await;
    let s = setup(&world).await;
    let alice = person(&world, "alice").await;
    join(&world, s.acme, s.acme, &alice, "developer", &s.boss).await;
    let topic = make_topic(&world, &s.boss, s.web, "Login").await.base.id;
    let listed = settings(&world, &alice, topic).await;
    assert!(
        one(&listed, "agent.model").writable,
        "02 section 2.3: topic.manage writes parameters"
    );
    assert!(
        !one(&listed, "chat.enabled").writable,
        "policies still need settings.manage"
    );
    let saved = put(
        &world,
        &alice,
        topic,
        "agent.model",
        &value(json!("qa-r3b")),
    )
    .await
    .ok();
    assert!(saved.writable);
    assert_eq!(saved.value, Some(json!("qa-r3b")));
    assert!(!saved.own.as_ref().unwrap().locked);
    let policy = put(&world, &alice, topic, "chat.enabled", &value(json!(false)))
        .await
        .problem();
    assert_eq!(policy.code, ErrorCode::AuthzDenied);
    assert_eq!(policy.extra["required"], "settings.manage");
    let mut lock = value(json!("qa-r3b"));
    lock.locked = Some(true);
    lock.expected_revision = Some(saved.own.as_ref().unwrap().revision);
    let refused = put(&world, &alice, topic, "agent.model", &lock)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert!(
        !one(&settings(&world, &alice, topic).await, "agent.model")
            .own
            .as_ref()
            .unwrap()
            .locked
    );
    // Undo may restore an unchanged false lock flag; it does not acquire policy authority.
    let mut undo = value(json!("before"));
    undo.locked = Some(false);
    undo.expected_revision = Some(saved.own.as_ref().unwrap().revision);
    let restored = put(&world, &alice, topic, "agent.model", &undo).await.ok();
    assert!(restored.writable);
    assert_eq!(restored.value, Some(json!("before")));
    assert!(!restored.own.as_ref().unwrap().locked);
    let viewer = settings(&world, &s.dan, topic).await;
    assert!(!one(&viewer, "agent.model").writable);
    let refused = put(
        &world,
        &s.dan,
        topic,
        "agent.model",
        &value(json!("another")),
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "topic.manage");
    world
        .app
        .call(NodeSettingDelete, &[&topic, &"agent.model"])
        .token(&alice.token)
        .send()
        .await
        .ok();
    assert!(
        one(&settings(&world, &alice, topic).await, "agent.model")
            .own
            .is_none()
    );
    lock.expected_revision = None;
    put(&world, &s.boss, s.web, "agent.model", &lock).await.ok();
    assert!(!one(&settings(&world, &alice, topic).await, "agent.model").writable);
    let refused = put(
        &world,
        &alice,
        topic,
        "agent.model",
        &value(json!("another")),
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::AccessLocked);
}

#[tokio::test]
async fn a_lock_decides_the_key_below_it_and_only_whoever_makes_the_rules_sets_or_lifts_it() {
    let world = World::new().await;
    let s = setup(&world).await;
    let mut locked = value(json!(false));
    locked.locked = Some(true);
    let at_org = put(&world, &s.boss, s.acme, "chat.enabled", &locked)
        .await
        .ok();
    assert!(at_org.own.as_ref().unwrap().locked);
    // Below, the key is decided from above, and cannot be written.
    let below = settings(&world, &s.boss, s.web).await;
    let chat = one(&below, "chat.enabled");
    assert_eq!(
        chat.locked_by.as_ref().unwrap().node.as_ref().unwrap().id,
        s.acme
    );
    assert!(!chat.writable);
    let refused = put(&world, &s.boss, s.web, "chat.enabled", &value(json!(true)))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AccessLocked);
    assert_eq!(refused.extra["by"]["id"], json!(s.acme));
    // A lock above refuses to take a value back below it as well.
    let refused = world
        .app
        .call(NodeSettingDelete, &[&s.web, &"chat.enabled"])
        .token(&s.boss.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AccessLocked);
    // Lifting the lock is done where it is set.
    let mut open = value(json!(false));
    open.locked = Some(false);
    let lifted = put(&world, &s.boss, s.acme, "chat.enabled", &open)
        .await
        .ok();
    assert!(!lifted.own.as_ref().unwrap().locked);
    let freed = put(&world, &s.boss, s.web, "chat.enabled", &value(json!(true)))
        .await
        .ok();
    assert_eq!(freed.value, Some(json!(true)));
}

#[tokio::test]
async fn a_value_that_does_not_fit_and_a_key_that_is_not_there_are_refused() {
    let world = World::new().await;
    let s = setup(&world).await;
    let refused = put(
        &world,
        &s.boss,
        s.acme,
        "chat.enabled",
        &value(json!("yes")),
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert!(
        refused.extra["errors"][0]["path"]
            .as_str()
            .unwrap()
            .starts_with("value")
    );
    let refused = put(&world, &s.boss, s.acme, "no.such.key", &value(json!(1)))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NotFound);
    // A key that is set at the system alone is not written at a node.
    let refused = put(
        &world,
        &s.boss,
        s.acme,
        "registration.mode",
        &value(json!("open")),
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_setting_is_written_against_the_revision_the_writer_saw_and_taken_back() {
    let world = World::new().await;
    let s = setup(&world).await;
    let first = put(
        &world,
        &s.boss,
        s.acme,
        "chat.enabled",
        &value(json!(false)),
    )
    .await
    .ok();
    let mut stale = value(json!(true));
    stale.expected_revision = Some(first.own.as_ref().unwrap().revision + 5);
    let refused = put(&world, &s.boss, s.acme, "chat.enabled", &stale)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(refused.extra["current"]["value"], json!(false));
    let mut right = value(json!(true));
    right.expected_revision = Some(first.own.as_ref().unwrap().revision);
    let second = put(&world, &s.boss, s.acme, "chat.enabled", &right)
        .await
        .ok();
    assert_eq!(second.own.as_ref().unwrap().revision, 2);
    // The same value again changes nothing and writes no row.
    let rows_before = world
        .audit()
        .await
        .iter()
        .filter(|action| *action == "settings.updated")
        .count();
    put(&world, &s.boss, s.acme, "chat.enabled", &value(json!(true)))
        .await
        .ok();
    let rows_after = world
        .audit()
        .await
        .iter()
        .filter(|action| *action == "settings.updated")
        .count();
    assert_eq!(rows_before, rows_after);
    // Taken back: inherited again, from the default.
    let gone = world
        .app
        .call(NodeSettingDelete, &[&s.acme, &"chat.enabled"])
        .token(&s.boss.token)
        .send()
        .await
        .ok();
    assert!(gone.own.is_none());
    assert_eq!(gone.source.scope, SettingScope::Default);
    assert!(
        world
            .audit()
            .await
            .iter()
            .any(|action| action == "settings.reset")
    );
}

#[tokio::test]
async fn a_first_setting_write_and_a_reset_use_the_row_seen_and_omitting_a_revision_still_writes() {
    use orch_api_contract::settings::NodeSettingDeleteQuery;
    let world = World::new().await;
    let s = setup(&world).await;
    let mut first = value(json!(false));
    first.expected_revision = Some(0);
    let first = put(&world, &s.boss, s.acme, "chat.enabled", &first)
        .await
        .ok();
    assert_eq!(first.own.as_ref().unwrap().revision, 1);
    let mut absent = value(json!(true));
    absent.expected_revision = Some(0);
    let refused = put(&world, &s.boss, s.acme, "chat.enabled", &absent)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(refused.extra["current"]["value"], json!(false));
    assert_eq!(refused.extra["changed_by"]["id"], json!(s.boss.id()));
    let next = put(&world, &s.boss, s.acme, "chat.enabled", &value(json!(true)))
        .await
        .ok();
    assert_eq!(
        next.value,
        Some(json!(true)),
        "API callers may still write without a revision"
    );
    assert_eq!(next.own.as_ref().unwrap().revision, 2);
    let stale = world
        .app
        .call(NodeSettingDelete, &[&s.acme, &"chat.enabled"])
        .token(&s.boss.token)
        .query(&NodeSettingDeleteQuery {
            expected_revision: Some(1),
        })
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(stale.extra["current"]["value"], json!(true));
    let gone = world
        .app
        .call(NodeSettingDelete, &[&s.acme, &"chat.enabled"])
        .token(&s.boss.token)
        .query(&NodeSettingDeleteQuery {
            expected_revision: Some(2),
        })
        .send()
        .await
        .ok();
    assert!(gone.own.is_none());
}

#[tokio::test]
async fn the_list_of_settings_costs_the_same_statements_whatever_is_written_in_it() {
    let world = World::new().await;
    let s = setup(&world).await;
    async fn measure(world: &World, who: &Who, node: Id) -> (usize, usize) {
        let call = world.app.call(NodeSettingList, &[&node]).token(&who.token);
        let before = Statements::now();
        let listed = call.send().await.ok();
        (Statements::now() - before, listed.len())
    }
    // Naming the places that rows come from costs a few statements once; more
    // rows cost nothing more.
    put(
        &world,
        &s.boss,
        s.acme,
        "chat.enabled",
        &value(json!(false)),
    )
    .await
    .ok();
    put(&world, &s.boss, s.web, "auto.title", &value(json!("off")))
        .await
        .ok();
    let (two_rows, count) = measure(&world, &s.boss, s.web).await;
    put(&world, &s.boss, s.acme, "auto.tags", &value(json!("off")))
        .await
        .ok();
    put(
        &world,
        &s.boss,
        s.web,
        "auto.description",
        &value(json!("off")),
    )
    .await
    .ok();
    put(
        &world,
        &s.boss,
        s.web,
        "auto.tags",
        &value(json!("summarizer")),
    )
    .await
    .ok();
    let (five_rows, same_count) = measure(&world, &s.boss, s.web).await;
    assert_eq!(count, same_count);
    assert_eq!(
        two_rows, five_rows,
        "{two_rows} statements with two rows, {five_rows} with five"
    );
}

#[tokio::test]
async fn a_secret_is_written_once_listed_by_name_down_the_hierarchy_and_its_name_is_listed_as_a_setting()
 {
    let world = World::new().await;
    let s = setup(&world).await;
    let set = |who: &Who, node: Id, name: &str, secret: &str| {
        world
            .app
            .call(NodeSecretSet, &[&node, &name])
            .token(&who.token)
            .body(&NodeSecretSetBody {
                value: secret.to_owned(),
            })
            .send()
    };
    let info = set(&s.boss, s.acme, "API_TOKEN", "s3cret").await.ok();
    assert_eq!(info.name, "API_TOKEN");
    assert_eq!(info.node.as_ref().unwrap().id, s.acme);
    assert!(info.rotated_at.is_some());
    set(&s.boss, s.web, "DB_URL", "postgres://x").await.ok();
    // The names that apply at the project are its own and the org's.
    let listed = world
        .app
        .call(NodeSecretList, &[&s.web])
        .token(&s.dan.token)
        .send()
        .await
        .ok();
    let names: Vec<&str> = listed.iter().map(|each| each.name.as_str()).collect();
    assert_eq!(names, vec!["API_TOKEN", "DB_URL"]);
    // No value is ever read back: the body of the answer holds only names.
    let raw = world
        .app
        .call(NodeSecretList, &[&s.web])
        .token(&s.dan.token)
        .send()
        .await
        .raw;
    assert!(!String::from_utf8_lossy(&raw).contains("s3cret"));
    // The name is also listed in the setting `env.secret_names` of the node.
    let at_web = settings(&world, &s.boss, s.web).await;
    assert_eq!(
        one(&at_web, "env.secret_names").own.as_ref().unwrap().value,
        json!(["DB_URL"])
    );
    // A viewer writes none, and a bad name or an empty value is refused.
    let refused = set(&s.dan, s.web, "NOPE", "x").await.problem();
    assert_eq!(refused.extra["required"], "settings.manage");
    assert_eq!(
        set(&s.boss, s.web, "lower", "x").await.problem().code,
        ErrorCode::RequestInvalid
    );
    assert_eq!(
        set(&s.boss, s.web, "EMPTY", "").await.problem().code,
        ErrorCode::RequestInvalid
    );
    // Deleted: gone from the list, and the audit rows name it and never say its value.
    let gone = world
        .app
        .call(NodeSecretDelete, &[&s.web, &"DB_URL"])
        .token(&s.boss.token)
        .send()
        .await;
    assert_eq!(gone.status, 204);
    let listed = world
        .app
        .call(NodeSecretList, &[&s.web])
        .token(&s.dan.token)
        .send()
        .await
        .ok();
    assert_eq!(listed.len(), 1);
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT summary || details::text FROM audit_log WHERE action LIKE 'settings.secret_%'",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert!(!rows.is_empty());
    assert!(
        rows.iter()
            .all(|row| !row.contains("s3cret") && !row.contains("postgres://x"))
    );
    let missing = world
        .app
        .call(NodeSecretDelete, &[&s.web, &"DB_URL"])
        .token(&s.boss.token)
        .send()
        .await
        .problem();
    assert_eq!(missing.code, ErrorCode::NotFound);
}
