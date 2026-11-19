//! Writing and clearing over a real database: what is refused and why, what is stale, what a lock allows, what the caller has to be asked, and what the history says.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::permissions::Permission;
use orch_db::audit::{Actor, Origin};
use orch_db::queries::hierarchy::testing::soft_delete;
use orch_settings::locks::Refusal;
use orch_settings::validation::{Need, Needs, needs};
use orch_settings::write::{Write, WriteError, write};
use orch_settings::{Error, Place, Target, resolve};
use orch_settings_contract::provenance::LockScope;
use orch_settings_contract::registry::{Invalid, Scope, keys};
use orch_types::{Id, new_id};
use serde_json::json;
use support::World;

async fn rows(w: &World) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM settings")
        .fetch_one(&w.db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_value_is_written_at_a_node_and_holds_there_and_below_with_its_writer_recorded() {
    let w = World::new().await;
    let written = w
        .set(Target::Node(w.project), keys::AGENT_MODEL, json!("model-b"))
        .await;
    assert!(written.changed);
    let own = written.view.own.clone().unwrap();
    assert_eq!(
        (own.value, own.locked, own.revision),
        (json!("model-b"), false, 1)
    );
    assert_eq!(written.view.value(), Some(&json!("model-b")));
    let again = w
        .set(Target::Node(w.project), keys::AGENT_MODEL, json!("model-c"))
        .await;
    assert_eq!(
        again.before.value(),
        Some(&json!("model-b")),
        "and says what held before it, for the marker in a thread's conversation"
    );
    assert_eq!(again.before.own.as_ref().map(|own| own.revision), Some(1));
    assert_eq!(again.view.own.unwrap().revision, 2, "each write counts one");
    assert_eq!(
        w.value(Place::Node(w.thread), keys::AGENT_MODEL).await,
        Some(json!("model-c"))
    );
    let by: Option<Id> = sqlx::query_scalar("SELECT updated_by FROM settings WHERE node_id = $1")
        .bind(w.project)
        .fetch_one(&w.db.pool)
        .await
        .unwrap();
    assert_eq!(by, Some(w.ada));
}

#[tokio::test]
async fn a_value_outside_its_schema_is_refused_with_what_is_wrong_and_nothing_is_written() {
    let w = World::new().await;
    let target = Target::Node(w.project);
    let refused = w
        .try_set(
            target,
            keys::TERMINAL_MAX_PER_PERSON,
            json!(0),
            None,
            None,
            true,
        )
        .await;
    let Err(WriteError::Invalid(Invalid::Value { key, faults })) = refused else {
        panic!("a value below its minimum was accepted");
    };
    assert_eq!(key, keys::TERMINAL_MAX_PER_PERSON);
    assert_eq!(faults.len(), 1);
    let wrong_kind = w
        .try_set(
            target,
            keys::TERMINAL_MAX_PER_PERSON,
            json!("4"),
            None,
            None,
            true,
        )
        .await;
    assert!(matches!(
        wrong_kind,
        Err(WriteError::Invalid(Invalid::Value { .. }))
    ));
    let inside = json!({ "on": ["turn_completed", "sometimes"], "min_interval_secs": 120 });
    let Err(WriteError::Invalid(Invalid::Value { faults, .. })) = w
        .try_set(target, keys::AUTO_TRIGGERS, inside, None, None, true)
        .await
    else {
        panic!("a trigger that does not exist was accepted");
    };
    assert_eq!(
        faults[0].path, "on[1]",
        "the fault is told with its place in the value"
    );
    assert_eq!(rows(&w).await, 0);
    assert!(w.audit().await.is_empty(), "a refused write leaves no line");
}

#[tokio::test]
async fn a_key_is_refused_at_a_scope_it_does_not_take_and_an_unknown_key_is_refused() {
    let w = World::new().await;
    let at_system = w
        .try_set(
            Target::System,
            keys::AGENT_MODEL,
            json!("m"),
            None,
            None,
            true,
        )
        .await;
    assert!(matches!(
        at_system,
        Err(WriteError::Invalid(Invalid::WrongScope {
            scope: Scope::System,
            ..
        }))
    ));
    let at_org = w
        .try_set(
            Target::Node(w.root),
            keys::HOSTS_FULL_PERCENT,
            json!(50),
            None,
            None,
            true,
        )
        .await;
    assert!(matches!(
        at_org,
        Err(WriteError::Invalid(Invalid::WrongScope {
            scope: Scope::Org,
            ..
        }))
    ));
    let unknown = w
        .try_set(
            Target::System,
            "agent.mood",
            json!("calm"),
            None,
            None,
            true,
        )
        .await;
    assert!(matches!(
        unknown,
        Err(WriteError::Invalid(Invalid::UnknownKey(_)))
    ));
    let clearing = w
        .try_clear(Target::Node(w.root), keys::HOSTS_FULL_PERCENT, None, true)
        .await;
    assert!(matches!(
        clearing,
        Err(WriteError::Invalid(Invalid::WrongScope { .. }))
    ));
    assert_eq!(rows(&w).await, 0);
}

#[tokio::test]
async fn a_locked_ancestor_refuses_a_write_or_a_clear_below_it_and_names_where_the_lock_is() {
    let w = World::new().await;
    w.set(Target::Node(w.thread), keys::APPROVAL_MODE, json!("auto"))
        .await;
    w.lock(Target::Node(w.root), keys::APPROVAL_MODE, json!("ask"))
        .await;

    let Err(WriteError::Locked { by }) = w
        .try_set(
            Target::Node(w.project),
            keys::APPROVAL_MODE,
            json!("on_request"),
            None,
            None,
            true,
        )
        .await
    else {
        panic!("a write under a lock was accepted");
    };
    assert_eq!((by.scope, by.node), (LockScope::Org, Some(w.root)));
    let cleared = w
        .try_clear(Target::Node(w.thread), keys::APPROVAL_MODE, None, true)
        .await;
    assert!(
        matches!(cleared, Err(WriteError::Locked { .. })),
        "a row under a lock is not taken back either"
    );
    assert_eq!(
        w.view(Place::Node(w.thread), keys::APPROVAL_MODE)
            .await
            .own
            .unwrap()
            .value,
        json!("auto"),
        "the thread's row is as it was"
    );

    let own = w
        .set(
            Target::Node(w.root),
            keys::APPROVAL_MODE,
            json!("on_request"),
        )
        .await;
    assert!(
        own.changed,
        "whoever makes the rules where the lock is changes that row"
    );
    assert_eq!(
        w.value(Place::Node(w.thread), keys::APPROVAL_MODE).await,
        Some(json!("on_request"))
    );
}

#[tokio::test]
async fn a_lock_at_the_system_decides_every_org() {
    let w = World::new().await;
    w.lock(Target::System, keys::TERMINAL_ENABLED, json!(false))
        .await;
    let Err(WriteError::Locked { by }) = w
        .try_set(
            Target::Node(w.root),
            keys::TERMINAL_ENABLED,
            json!(true),
            None,
            None,
            true,
        )
        .await
    else {
        panic!("an org overrode the system's lock");
    };
    assert_eq!((by.scope, by.node), (LockScope::System, None));
    let at_system = w
        .set(Target::System, keys::TERMINAL_ENABLED, json!(true))
        .await;
    assert!(at_system.changed);
}

#[tokio::test]
async fn a_lock_changes_nothing_where_lower_scopes_can_only_narrow_or_nothing_lies_below() {
    let w = World::new().await;
    let narrowing = w
        .try_set(
            Target::Node(w.root),
            keys::APPROVAL_ALLOWED_MODES,
            json!(["ask"]),
            Some(true),
            None,
            true,
        )
        .await;
    assert!(matches!(
        narrowing,
        Err(WriteError::Lock(Refusal::NotLockable))
    ));
    let at_thread = w
        .try_set(
            Target::Node(w.thread),
            keys::AGENT_MODEL,
            json!("m"),
            Some(true),
            None,
            true,
        )
        .await;
    assert!(
        matches!(at_thread, Err(WriteError::Lock(Refusal::NotLockable))),
        "nothing lies below a thread"
    );
    w.set(
        Target::Node(w.root),
        keys::APPROVAL_ALLOWED_MODES,
        json!(["ask", "auto"]),
    )
    .await;
    let below = w
        .try_set(
            Target::Node(w.project),
            keys::APPROVAL_ALLOWED_MODES,
            json!(["ask"]),
            None,
            None,
            true,
        )
        .await;
    assert!(below.is_ok(), "nobody is shut out of narrowing it");
    assert_eq!(rows(&w).await, 2);
}

#[tokio::test]
async fn a_stale_revision_is_refused_with_the_value_as_it_stands_now() {
    let w = World::new().await;
    let target = Target::Node(w.project);
    w.set(target, keys::AGENT_MODEL, json!("a")).await;
    w.set(target, keys::AGENT_MODEL, json!("b")).await;

    let Err(WriteError::Stale { current }) = w
        .try_set(target, keys::AGENT_MODEL, json!("c"), None, Some(1), true)
        .await
    else {
        panic!("a write at an old revision was accepted");
    };
    let own = current.own.clone().unwrap();
    assert_eq!(
        (own.value, own.revision),
        (json!("b"), 2),
        "the caller is shown what is there now"
    );
    assert_eq!(current.value(), Some(&json!("b")));
    assert_eq!(
        w.view(Place::Node(w.project), keys::AGENT_MODEL)
            .await
            .own
            .unwrap()
            .revision,
        2
    );

    let ok = w
        .try_set(target, keys::AGENT_MODEL, json!("c"), None, Some(2), true)
        .await
        .unwrap();
    assert_eq!(ok.view.own.unwrap().revision, 3);

    let Err(WriteError::Stale { current }) = w
        .try_set(
            Target::Node(w.topic),
            keys::AGENT_MODEL,
            json!("x"),
            None,
            Some(1),
            true,
        )
        .await
    else {
        panic!("a write expecting a row that is not there was accepted");
    };
    assert!(current.own.is_none(), "the row the writer saw is gone");
    let cleared = w.try_clear(target, keys::AGENT_MODEL, Some(1), true).await;
    assert!(
        matches!(cleared, Err(WriteError::Stale { .. })),
        "a clear is held to the revision too"
    );
    assert!(
        w.try_clear(target, keys::AGENT_MODEL, Some(3), true)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn setting_or_lifting_a_lock_is_for_a_caller_who_showed_they_may() {
    let w = World::new().await;
    let target = Target::Node(w.root);
    let refused = w
        .try_set(
            target,
            keys::AGENT_MODEL,
            json!("m"),
            Some(true),
            None,
            false,
        )
        .await;
    assert!(matches!(
        refused,
        Err(WriteError::Lock(Refusal::NotPermitted))
    ));
    assert_eq!(
        rows(&w).await,
        0,
        "a refusal writes nothing, not even the value"
    );

    let locked = w
        .try_set(
            target,
            keys::AGENT_MODEL,
            json!("m"),
            Some(true),
            None,
            true,
        )
        .await
        .unwrap();
    assert!(locked.view.own.unwrap().locked);
    let kept = w
        .try_set(target, keys::AGENT_MODEL, json!("n"), None, None, false)
        .await
        .unwrap();
    assert!(
        kept.view.own.unwrap().locked,
        "a new value, with the lock as it was, asks nothing more"
    );
    let lifting = w
        .try_set(
            target,
            keys::AGENT_MODEL,
            json!("n"),
            Some(false),
            None,
            false,
        )
        .await;
    assert!(matches!(
        lifting,
        Err(WriteError::Lock(Refusal::NotPermitted))
    ));
    let taking_back = w.try_clear(target, keys::AGENT_MODEL, None, false).await;
    assert!(
        matches!(taking_back, Err(WriteError::Lock(Refusal::NotPermitted))),
        "taking back a locked row lifts its lock"
    );
    assert!(
        w.try_clear(target, keys::AGENT_MODEL, None, true)
            .await
            .unwrap()
            .changed
    );
}

#[tokio::test]
async fn writing_a_value_as_it_stands_changes_nothing_and_leaves_no_trace() {
    let w = World::new().await;
    let target = Target::Node(w.project);
    w.set(target, keys::AGENT_MODEL, json!("a")).await;
    let again = w.set(target, keys::AGENT_MODEL, json!("a")).await;
    assert!(!again.changed);
    assert_eq!(
        again.view.own.unwrap().revision,
        1,
        "no revision is counted"
    );
    assert_eq!(
        w.audit().await.len(),
        1,
        "and no line of history is written"
    );
    let stale = w
        .try_set(target, keys::AGENT_MODEL, json!("a"), None, Some(5), true)
        .await;
    assert!(
        matches!(stale, Err(WriteError::Stale { .. })),
        "a revision that is wrong is still refused"
    );
    let locked = w
        .try_set(
            target,
            keys::AGENT_MODEL,
            json!("a"),
            Some(true),
            None,
            true,
        )
        .await
        .unwrap();
    assert!(locked.changed, "locking is a change");
}

#[tokio::test]
async fn a_side_thread_takes_no_setting() {
    let w = World::new().await;
    let side = w.node("thread", w.thread, "Fix (side)").await;
    let written = w
        .try_set(
            Target::Node(side),
            keys::AGENT_MODEL,
            json!("m"),
            None,
            None,
            true,
        )
        .await;
    assert!(matches!(written, Err(WriteError::SideThread)));
    let cleared = w
        .try_clear(Target::Node(side), keys::AGENT_MODEL, None, true)
        .await;
    assert!(matches!(cleared, Err(WriteError::SideThread)));
}

#[tokio::test]
async fn a_deleted_node_or_one_that_never_was_takes_no_setting_and_shows_none() {
    let w = World::new().await;
    let mut conn = w.conn().await;
    soft_delete(&mut conn, w.topic, w.ada, World::now())
        .await
        .unwrap();
    drop(conn);
    let deleted = w
        .try_set(
            Target::Node(w.topic),
            keys::AGENT_MODEL,
            json!("m"),
            None,
            None,
            true,
        )
        .await;
    assert!(matches!(deleted, Err(WriteError::NotFound)));
    let nowhere = w
        .try_set(
            Target::Node(new_id()),
            keys::AGENT_MODEL,
            json!("m"),
            None,
            None,
            true,
        )
        .await;
    assert!(matches!(nowhere, Err(WriteError::NotFound)));
    let mut conn = w.conn().await;
    let read = resolve(&mut conn, Place::Node(w.thread), keys::AGENT_MODEL).await;
    assert!(
        matches!(read, Err(Error::NotFound)),
        "what lies in a deleted node is deleted with it"
    );
}

#[tokio::test]
async fn a_write_leaves_a_line_of_history_that_says_what_a_parameter_became_and_only_that_a_policy_changed()
 {
    let w = World::new().await;
    let project = Target::Node(w.project);
    w.set(project, keys::AGENT_MODEL, json!("gpt-6-sol")).await;
    w.set(project, keys::AGENT_MODEL, json!("gpt-6-luna")).await;
    w.set(project, keys::TERMINAL_ENABLED, json!(false)).await;
    w.set(project, keys::ENV_IDLE_STOP_MINUTES, json!(30)).await;
    w.set(project, keys::AGENT_INSTRUCTIONS, json!("x".repeat(200)))
        .await;
    w.try_set(
        project,
        keys::AGENT_MODEL,
        json!("gpt-6-luna"),
        Some(true),
        None,
        true,
    )
    .await
    .unwrap();
    w.try_set(
        project,
        keys::AGENT_MODEL,
        json!("gpt-6-luna"),
        Some(false),
        None,
        true,
    )
    .await
    .unwrap();
    w.try_clear(project, keys::AGENT_MODEL, None, true)
        .await
        .unwrap();
    w.set(Target::System, keys::TERMINAL_ENABLED, json!(false))
        .await;

    let rows = w.audit().await;
    let lines: Vec<(&str, bool, &str)> = rows
        .iter()
        .map(|(action, history, summary, _)| (action.as_str(), *history, summary.as_str()))
        .collect();
    assert_eq!(
        lines,
        vec![
            ("settings.updated", true, "Model set to gpt-6-sol"),
            ("settings.updated", true, "Model set to gpt-6-luna"),
            ("settings.updated", true, "Shells changed"),
            ("settings.updated", true, "Idle stop set to 30 minutes"),
            ("settings.updated", true, "Instructions changed"),
            ("settings.updated", true, "Model locked"),
            ("settings.updated", true, "Model unlocked"),
            ("settings.reset", true, "Model reset to what is inherited"),
            ("system.settings_updated", false, "Shells changed"),
        ]
    );
    let second = &rows[1].3;
    assert_eq!(
        (second["was"].as_str(), second["value"].as_str()),
        (Some("gpt-6-sol"), Some("gpt-6-luna"))
    );
    assert_eq!(
        (second["scope"].as_str(), second["revision"].as_i64()),
        (Some("project"), Some(2))
    );
    assert!(
        rows[2].3.get("value").is_none(),
        "a policy's value is not in its line"
    );
    assert!(
        rows[4].3.get("value").is_none(),
        "nor is a text too long for a line"
    );

    let (object_kind, object_id, node_id, org_id, actor_kind, actor_id): (String, Option<Id>, Option<Id>, Option<Id>, String, Option<Id>) =
        sqlx::query_as("SELECT object_kind, object_id, node_id, org_id, actor_kind, actor_id FROM audit_log ORDER BY at, id LIMIT 1")
            .fetch_one(&w.db.pool)
            .await
            .unwrap();
    assert_eq!(object_kind, "settings");
    assert_eq!(
        (object_id, node_id, org_id),
        (Some(w.project), Some(w.project), Some(w.root))
    );
    assert_eq!((actor_kind.as_str(), actor_id), ("user", Some(w.ada)));
    let (system_kind, system_node): (String, Option<Id>) = sqlx::query_as(
        "SELECT object_kind, node_id FROM audit_log WHERE action = 'system.settings_updated'",
    )
    .fetch_one(&w.db.pool)
    .await
    .unwrap();
    assert_eq!((system_kind.as_str(), system_node), ("system", None));
}

#[tokio::test]
async fn taking_a_value_back_makes_the_key_inherit_again_and_taking_back_nothing_is_no_change() {
    let w = World::new().await;
    w.set(Target::Node(w.root), keys::AGENT_MODEL, json!("a"))
        .await;
    w.set(Target::Node(w.project), keys::AGENT_MODEL, json!("b"))
        .await;
    let cleared = w
        .try_clear(Target::Node(w.project), keys::AGENT_MODEL, None, true)
        .await
        .unwrap();
    assert!(cleared.changed);
    assert!(cleared.view.own.is_none());
    assert_eq!(cleared.view.value(), Some(&json!("a")), "now inherited");
    assert_eq!(
        cleared.before.value(),
        Some(&json!("b")),
        "from what it was"
    );
    assert_eq!(cleared.view.resolved.source.node, Some(w.root));
    let nothing = w
        .try_clear(Target::Node(w.project), keys::AGENT_MODEL, None, true)
        .await
        .unwrap();
    assert!(!nothing.changed);
    assert_eq!(w.audit().await.len(), 3, "two writes and one reset");
    let again = w
        .set(Target::Node(w.project), keys::AGENT_MODEL, json!("c"))
        .await;
    assert_eq!(
        again.view.own.unwrap().revision,
        1,
        "a row made anew starts over"
    );
}

#[tokio::test]
async fn what_a_write_asks_of_the_caller_follows_the_class_of_the_key_and_the_place() {
    let w = World::new().await;
    let mut conn = w.conn().await;
    let project = needs(&mut conn, keys::AGENT_MODEL, Target::Node(w.project))
        .await
        .unwrap();
    assert_eq!(
        project,
        Needs {
            value: Need::OnNode {
                permission: Permission::ProjectManage,
                node: w.project
            },
            lock: Need::OnNode {
                permission: Permission::SettingsManage,
                node: w.project
            },
        }
    );
    let policy = needs(&mut conn, keys::TERMINAL_ENABLED, Target::Node(w.topic))
        .await
        .unwrap();
    assert_eq!(
        policy.value,
        Need::OnNode {
            permission: Permission::SettingsManage,
            node: w.topic
        }
    );
    let system = needs(&mut conn, keys::REGISTRATION_MODE, Target::System)
        .await
        .unwrap();
    assert_eq!(
        (system.value, system.lock),
        (Need::SystemAdmin, Need::SystemAdmin)
    );

    let alone = needs(&mut conn, keys::THREAD_CWD, Target::Node(w.thread))
        .await
        .unwrap();
    assert_eq!(
        alone.value,
        Need::OnNode {
            permission: Permission::ThreadManage,
            node: w.thread
        }
    );
    drop(conn);
    let environment = w
        .managed_environment(w.thread, w.topic, &json!({}), &json!({}))
        .await;
    let mut conn = w.conn().await;
    let there = needs(&mut conn, keys::THREAD_CWD, Target::Node(w.thread))
        .await
        .unwrap();
    assert_eq!(
        there.value,
        Need::OnEnvironment {
            permission: Permission::EnvironmentManage,
            environment
        },
        "where a thread runs is the environment's to decide"
    );
    let model = needs(&mut conn, keys::AGENT_MODEL, Target::Node(w.thread))
        .await
        .unwrap();
    assert_eq!(
        model.value,
        Need::OnNode {
            permission: Permission::ThreadManage,
            node: w.thread
        }
    );

    assert!(matches!(
        needs(&mut conn, keys::AGENT_MODEL, Target::System).await,
        Err(WriteError::Invalid(Invalid::WrongScope { .. }))
    ));
    assert!(matches!(
        needs(&mut conn, "agent.mood", Target::System).await,
        Err(WriteError::Invalid(Invalid::UnknownKey(_)))
    ));
    assert!(matches!(
        needs(&mut conn, keys::AGENT_MODEL, Target::Node(new_id())).await,
        Err(WriteError::NotFound)
    ));
}

#[tokio::test]
async fn a_write_in_a_transaction_that_is_dropped_leaves_neither_the_row_nor_its_line() {
    let w = World::new().await;
    let origin = Origin::default();
    let request = Write {
        actor: Actor::User(w.ada),
        target: Target::Node(w.project),
        key: keys::AGENT_MODEL,
        value: json!("m"),
        locked: None,
        expected_revision: None,
        may_lock: true,
        origin: &origin,
    };
    let mut tx = w.db.pool.begin().await.unwrap();
    write(&mut tx, &request, World::now()).await.unwrap();
    drop(tx);
    assert_eq!(rows(&w).await, 0);
    assert!(w.audit().await.is_empty());
    let mut tx = w.db.pool.begin().await.unwrap();
    write(&mut tx, &request, World::now()).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        (rows(&w).await, w.audit().await.len()),
        (1, 1),
        "they come together"
    );
}
