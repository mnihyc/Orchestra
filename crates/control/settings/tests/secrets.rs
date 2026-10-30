//! Secrets over a real database: sealed to their row, listed by name down the hierarchy, given to the environments below a node, and never in a line of history.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_db::audit::{Actor, Origin};
use orch_db::secrets::MasterKey;
use orch_settings::secrets::{
    Delete, Owner, Put, SECRET_MAX_BYTES, SecretError, SecretInfo, delete, list, open,
    open_resolved, put,
};
use orch_settings::{Place, Target};
use orch_settings_contract::provenance::LockScope;
use orch_settings_contract::registry::keys;
use orch_types::new_id;
use serde_json::json;
use support::{World, master};

async fn put_secret(
    w: &World,
    owner: Owner,
    name: &str,
    value: &str,
) -> Result<SecretInfo, SecretError> {
    put_secret_with(w, &master(), owner, name, value).await
}

async fn put_secret_with(
    w: &World,
    key: &MasterKey,
    owner: Owner,
    name: &str,
    value: &str,
) -> Result<SecretInfo, SecretError> {
    let origin = Origin::default();
    let request = Put {
        actor: Actor::User(w.ada),
        owner,
        name,
        value,
        origin: &origin,
    };
    let mut conn = w.conn().await;
    put(&mut conn, key, &request, World::now()).await
}

async fn delete_secret(w: &World, owner: Owner, name: &str) -> Result<SecretInfo, SecretError> {
    let origin = Origin::default();
    let request = Delete {
        actor: Actor::User(w.ada),
        owner,
        name,
        origin: &origin,
    };
    let mut conn = w.conn().await;
    delete(&mut conn, &request, World::now()).await
}

async fn names(w: &World, owner: Owner) -> Vec<(String, Option<orch_types::Id>)> {
    let mut conn = w.conn().await;
    list(&mut conn, owner)
        .await
        .unwrap()
        .into_iter()
        .map(|info| (info.name, info.node))
        .collect()
}

#[tokio::test]
async fn a_secret_is_sealed_and_opens_only_for_its_own_row() {
    let w = World::new().await;
    put_secret(&w, Owner::Node(w.root), "API_KEY", "sk-live-1")
        .await
        .unwrap();
    let sealed: Vec<u8> = sqlx::query_scalar("SELECT sealed FROM secrets")
        .fetch_one(&w.db.pool)
        .await
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&sealed).contains("sk-live"),
        "what is stored is not the value"
    );

    let mut conn = w.conn().await;
    let opened = open(&mut conn, &master(), Owner::Node(w.root), "API_KEY")
        .await
        .unwrap();
    assert_eq!(opened.as_deref(), Some("sk-live-1"));
    assert_eq!(
        open(&mut conn, &master(), Owner::Node(w.root), "NOPE")
            .await
            .unwrap(),
        None
    );
    let wrong_key = open(
        &mut conn,
        &MasterKey::new([1; 32]),
        Owner::Node(w.root),
        "API_KEY",
    )
    .await;
    assert!(
        matches!(wrong_key, Err(SecretError::Seal(_))),
        "another key does not open it"
    );

    // The same sealed bytes put in other rows: another owner, and another name.
    sqlx::query("INSERT INTO secrets (id, owner_kind, owner_id, name, sealed) SELECT gen_random_uuid(), 'setting', $1, 'API_KEY', sealed FROM secrets")
        .bind(w.project)
        .execute(&w.db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO secrets (id, owner_kind, owner_id, name, sealed) SELECT gen_random_uuid(), 'setting', owner_id, 'OTHER_KEY', sealed FROM secrets WHERE name = 'API_KEY' AND owner_id = $1")
        .bind(w.root)
        .execute(&w.db.pool)
        .await
        .unwrap();
    let elsewhere = open(&mut conn, &master(), Owner::Node(w.project), "API_KEY").await;
    assert!(
        matches!(elsewhere, Err(SecretError::Seal(_))),
        "a copy in another owner's row does not open"
    );
    let renamed = open(&mut conn, &master(), Owner::Node(w.root), "OTHER_KEY").await;
    assert!(
        matches!(renamed, Err(SecretError::Seal(_))),
        "a copy under another name does not open"
    );
    let still = open(&mut conn, &master(), Owner::Node(w.root), "API_KEY")
        .await
        .unwrap();
    assert_eq!(
        still.as_deref(),
        Some("sk-live-1"),
        "its own row still does"
    );
}

#[tokio::test]
async fn secrets_are_listed_by_name_down_the_hierarchy_the_nearest_winning_and_opened_the_same_way()
{
    let w = World::new().await;
    put_secret(&w, Owner::Node(w.root), "A", "a-root")
        .await
        .unwrap();
    put_secret(&w, Owner::Node(w.root), "B", "b-root")
        .await
        .unwrap();
    put_secret(&w, Owner::Node(w.project), "B", "b-project")
        .await
        .unwrap();
    put_secret(&w, Owner::Node(w.project), "C", "c-project")
        .await
        .unwrap();

    assert_eq!(
        names(&w, Owner::Node(w.topic)).await,
        vec![
            ("A".to_owned(), Some(w.root)),
            ("B".to_owned(), Some(w.project)),
            ("C".to_owned(), Some(w.project)),
        ],
        "one for each name, from the nearest owner"
    );
    assert_eq!(
        names(&w, Owner::Node(w.root)).await,
        vec![
            ("A".to_owned(), Some(w.root)),
            ("B".to_owned(), Some(w.root))
        ]
    );
    let mut conn = w.conn().await;
    let opened = open_resolved(&mut conn, &master(), Owner::Node(w.topic))
        .await
        .unwrap();
    let pairs: Vec<(&str, &str)> = opened
        .iter()
        .map(|each| (each.name.as_str(), each.value.as_str()))
        .collect();
    assert_eq!(
        pairs,
        vec![("A", "a-root"), ("B", "b-project"), ("C", "c-project")]
    );
    assert!(
        !format!("{opened:?}").contains("b-project"),
        "a secret opened does not print itself"
    );
}

#[tokio::test]
async fn setting_a_secret_gives_its_name_to_the_nodes_environments_and_deleting_it_takes_the_name_back()
 {
    let w = World::new().await;
    w.set(
        Target::Node(w.project),
        keys::ENV_SECRET_NAMES,
        json!(["EXISTING"]),
    )
    .await;
    let first = put_secret(&w, Owner::Node(w.project), "TOKEN", "t-1")
        .await
        .unwrap();
    assert_eq!(first.rotated_at, None);
    assert_eq!(
        w.value(Place::Node(w.topic), keys::ENV_SECRET_NAMES).await,
        Some(json!(["EXISTING", "TOKEN"])),
        "the name joins those the node already listed"
    );
    let replaced = put_secret(&w, Owner::Node(w.project), "TOKEN", "t-2")
        .await
        .unwrap();
    assert!(
        replaced.rotated_at.is_some(),
        "a new value for a name counts as rotated"
    );
    assert_eq!(
        w.view(Place::Node(w.project), keys::ENV_SECRET_NAMES)
            .await
            .own
            .unwrap()
            .value,
        json!(["EXISTING", "TOKEN"]),
        "and lists nothing twice"
    );
    delete_secret(&w, Owner::Node(w.project), "TOKEN")
        .await
        .unwrap();
    assert_eq!(
        w.value(Place::Node(w.topic), keys::ENV_SECRET_NAMES).await,
        Some(json!(["EXISTING"]))
    );

    put_secret(&w, Owner::Node(w.topic), "ONLY", "o")
        .await
        .unwrap();
    assert_eq!(
        w.view(Place::Node(w.topic), keys::ENV_SECRET_NAMES)
            .await
            .own
            .unwrap()
            .value,
        json!(["ONLY"])
    );
    delete_secret(&w, Owner::Node(w.topic), "ONLY")
        .await
        .unwrap();
    assert!(
        w.view(Place::Node(w.topic), keys::ENV_SECRET_NAMES)
            .await
            .own
            .is_none(),
        "a list that is empty again is not kept as a row"
    );
}

#[tokio::test]
async fn a_lock_above_that_holds_the_list_refuses_the_secret_and_stores_nothing() {
    let w = World::new().await;
    w.lock(
        Target::Node(w.root),
        keys::ENV_SECRET_NAMES,
        json!(["API_KEY"]),
    )
    .await;
    let Err(SecretError::Locked { by }) =
        put_secret(&w, Owner::Node(w.project), "TOKEN", "t").await
    else {
        panic!("a secret was stored under a lock on the list of names");
    };
    assert_eq!((by.scope, by.node), (LockScope::Org, Some(w.root)));
    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM secrets")
        .fetch_one(&w.db.pool)
        .await
        .unwrap();
    assert_eq!(stored, 0);
    put_secret(&w, Owner::Node(w.root), "API_KEY", "k")
        .await
        .unwrap();
    assert_eq!(
        w.value(Place::Node(w.topic), keys::ENV_SECRET_NAMES).await,
        Some(json!(["API_KEY"])),
        "where the lock is, the list is the org's to extend"
    );
}

#[tokio::test]
async fn a_thread_keeps_no_secret_and_a_side_thread_has_none_of_its_own() {
    let w = World::new().await;
    let on_thread = put_secret(&w, Owner::Node(w.thread), "TOKEN", "t").await;
    assert!(
        matches!(on_thread, Err(SecretError::NotHere)),
        "{on_thread:?}"
    );
    let side = w.node("thread", w.thread, "Fix (side)").await;
    let on_side = put_secret(&w, Owner::Node(side), "TOKEN", "t").await;
    assert!(
        matches!(on_side, Err(SecretError::SideThread)),
        "{on_side:?}"
    );
    let nowhere = put_secret(&w, Owner::Node(new_id()), "TOKEN", "t").await;
    assert!(matches!(nowhere, Err(SecretError::NotFound)));
    assert!(matches!(
        put_secret(&w, Owner::Environment(new_id()), "TOKEN", "t").await,
        Err(SecretError::NotFound)
    ));
}

#[tokio::test]
async fn an_environments_own_secret_is_its_alone_and_comes_before_the_one_of_its_home() {
    let w = World::new().await;
    let environment = w
        .managed_environment(w.thread, w.topic, &json!({}), &json!({}))
        .await;
    put_secret(&w, Owner::Node(w.project), "TOKEN", "from-project")
        .await
        .unwrap();
    put_secret(&w, Owner::Node(w.project), "SHARED", "shared")
        .await
        .unwrap();
    put_secret(
        &w,
        Owner::Environment(environment),
        "TOKEN",
        "from-environment",
    )
    .await
    .unwrap();
    put_secret(&w, Owner::Environment(environment), "ONLY_HERE", "only")
        .await
        .unwrap();

    assert_eq!(
        names(&w, Owner::Environment(environment)).await,
        vec![
            ("ONLY_HERE".to_owned(), None),
            ("SHARED".to_owned(), Some(w.project)),
            ("TOKEN".to_owned(), None),
        ],
        "its own has no node, and of two with one name its own is the one"
    );
    let mut conn = w.conn().await;
    let opened = open_resolved(&mut conn, &master(), Owner::Environment(environment))
        .await
        .unwrap();
    let token = opened.iter().find(|each| each.name == "TOKEN").unwrap();
    assert_eq!(token.value, "from-environment");
    assert_eq!(
        names(&w, Owner::Node(w.topic)).await,
        vec![
            ("SHARED".to_owned(), Some(w.project)),
            ("TOKEN".to_owned(), Some(w.project))
        ],
        "what is the environment's alone is not seen from the node"
    );
    let listed: Option<serde_json::Value> = w
        .view(Place::Node(w.project), keys::ENV_SECRET_NAMES)
        .await
        .value()
        .cloned();
    assert_eq!(
        listed,
        Some(json!(["TOKEN", "SHARED"])),
        "and put no name in the node's list"
    );

    delete_secret(&w, Owner::Environment(environment), "TOKEN")
        .await
        .unwrap();
    let opened = open_resolved(&mut conn, &master(), Owner::Environment(environment))
        .await
        .unwrap();
    let token = opened.iter().find(|each| each.name == "TOKEN").unwrap();
    assert_eq!(
        token.value, "from-project",
        "the home's comes back when its own is gone"
    );
}

#[tokio::test]
async fn the_history_of_a_secret_names_it_and_never_carries_its_value() {
    let w = World::new().await;
    put_secret(&w, Owner::Node(w.project), "API_KEY", "very-secret-value")
        .await
        .unwrap();
    put_secret(&w, Owner::Node(w.project), "API_KEY", "very-secret-value-2")
        .await
        .unwrap();
    delete_secret(&w, Owner::Node(w.project), "API_KEY")
        .await
        .unwrap();
    let rows = w.audit().await;
    let lines: Vec<(&str, bool, &str)> = rows
        .iter()
        .map(|(action, history, summary, _)| (action.as_str(), *history, summary.as_str()))
        .collect();
    assert_eq!(
        lines,
        vec![
            ("settings.secret_set", true, "Secret API_KEY set"),
            ("settings.secret_set", true, "Secret API_KEY replaced"),
            ("settings.secret_deleted", true, "Secret API_KEY deleted"),
        ],
        "the list of names changes with them and has no line of its own"
    );
    assert_eq!(rows[0].3["name"], "API_KEY");
    assert_eq!(rows[0].3["listed"], true);
    assert_eq!(rows[1].3["listed"], false);
    assert_eq!(rows[2].3["unlisted"], true);
    let everything: String = sqlx::query_scalar("SELECT string_agg(summary || details::text || coalesce(user_agent, ''), ' ') FROM audit_log")
        .fetch_one(&w.db.pool)
        .await
        .unwrap();
    assert!(
        !everything.contains("very-secret"),
        "no audit row holds a value"
    );
    let (node_id, org_id): (Option<orch_types::Id>, Option<orch_types::Id>) =
        sqlx::query_as("SELECT node_id, org_id FROM audit_log LIMIT 1")
            .fetch_one(&w.db.pool)
            .await
            .unwrap();
    assert_eq!((node_id, org_id), (Some(w.project), Some(w.root)));
}

#[tokio::test]
async fn a_name_that_is_not_a_secret_name_an_empty_value_and_one_too_large_are_refused() {
    let w = World::new().await;
    for bad in ["api_key", "2FA", "A-B", "", &"A".repeat(65)] {
        let refused = put_secret(&w, Owner::Node(w.root), bad, "v").await;
        assert!(
            matches!(refused, Err(SecretError::BadName)),
            "{bad:?}: {refused:?}"
        );
    }
    assert!(matches!(
        put_secret(&w, Owner::Node(w.root), "OK_NAME", "").await,
        Err(SecretError::Empty)
    ));
    let huge = "x".repeat(SECRET_MAX_BYTES + 1);
    assert!(matches!(
        put_secret(&w, Owner::Node(w.root), "OK_NAME", &huge).await,
        Err(SecretError::TooLarge { .. })
    ));
    let fits = "x".repeat(SECRET_MAX_BYTES);
    put_secret(&w, Owner::Node(w.root), "OK_NAME", &fits)
        .await
        .unwrap();
    assert!(
        w.audit().await.len() == 1,
        "only the one that was stored left a line"
    );
}

#[tokio::test]
async fn deleting_what_is_not_there_says_so_and_changes_nothing() {
    let w = World::new().await;
    let missing = delete_secret(&w, Owner::Node(w.project), "TOKEN").await;
    assert!(matches!(missing, Err(SecretError::NoSuchSecret)));
    assert!(w.audit().await.is_empty());
}

#[tokio::test]
async fn two_secrets_put_at_once_at_one_node_are_both_given_to_its_environments() {
    let w = World::new().await;
    let origin = Origin::default();
    let first = Put {
        actor: Actor::User(w.ada),
        owner: Owner::Node(w.project),
        name: "ONE",
        value: "1",
        origin: &origin,
    };
    let mut held = w.db.pool.begin().await.unwrap();
    put(&mut held, &master(), &first, World::now())
        .await
        .unwrap();

    let (pool, ada, project) = (w.db.pool.clone(), w.ada, w.project);
    let other = tokio::spawn(async move {
        let origin = Origin::default();
        let second = Put {
            actor: Actor::User(ada),
            owner: Owner::Node(project),
            name: "TWO",
            value: "2",
            origin: &origin,
        };
        let mut tx = pool.begin().await.unwrap();
        put(&mut tx, &master(), &second, World::now())
            .await
            .unwrap();
        tx.commit().await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !other.is_finished(),
        "the second waits for the first, which holds the node"
    );
    held.commit().await.unwrap();
    other.await.unwrap();
    assert_eq!(
        w.value(Place::Node(w.topic), keys::ENV_SECRET_NAMES).await,
        Some(json!(["ONE", "TWO"])),
        "neither name was lost to the other"
    );
}
