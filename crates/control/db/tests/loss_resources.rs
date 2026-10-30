//! Operations renew one Inbox item and native resources survive the permanent loss of their offering machine.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_db::queries::{
    identity::testing::insert_user,
    notifications::{operations, people::NewNotification},
    resources::{self, mcp, mounts},
};
use orch_testkit::{clock, database::TestDb};
use orch_types::{Id, Time, new_id};
use serde_json::json;

fn notice<'a>(
    person: Id,
    key: &'a str,
    data: &'a serde_json::Value,
    at: chrono::DateTime<chrono::Utc>,
) -> NewNotification<'a> {
    NewNotification {
        user_id: person,
        kind: "environment.attention",
        node_id: None,
        thread_id: None,
        approval_id: None,
        chat_id: None,
        title: "Environment operation",
        body: "changed",
        data,
        dedupe_key: key,
        at,
    }
}

#[tokio::test]
async fn a_second_operation_on_the_same_resources_updates_one_item_and_an_older_completion_cannot_replace_it()
 {
    let db = TestDb::fresh().await;
    let mut conn = db.pool.acquire().await.unwrap();
    let person = new_id();
    insert_user(&mut conn, person, "operator").await.unwrap();
    let resource = new_id();
    let key = operations::group_key("resources", &[("environment".into(), resource)]);
    let first = new_id();
    let second = new_id();
    let at = clock::start();
    operations::save(&mut conn, first, &json!({"notify":true}), at)
        .await
        .unwrap();
    let old = json!({"state":"lost"});
    let written = operations::publish(&mut conn, first, &notice(person, &key, &old, at))
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE notifications SET read_at=$2 WHERE id=$1")
        .bind(written.id)
        .bind(at)
        .execute(&mut *conn)
        .await
        .unwrap();
    operations::save(&mut conn, second, &json!({"notify":true}), at)
        .await
        .unwrap();
    let restored = json!({"state":"recovered"});
    let updated = operations::publish(&mut conn, second, &notice(person, &key, &restored, at))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        updated.id, written.id,
        "later operations retain the Inbox item's identity"
    );
    let late = json!({"state":"old failure"});
    operations::publish(&mut conn, first, &notice(person, &key, &late, at))
        .await
        .unwrap();
    let (count, state, unread): (i64, String, bool) = sqlx::query_as(
        "SELECT count(*) OVER(),data->>'state',read_at IS NULL FROM notifications WHERE user_id=$1",
    )
    .bind(person)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!((count, state, unread), (1, "recovered".into(), true));
}

async fn machine(conn: &mut orch_db::Conn, owner: Id, name: &str) -> Id {
    resources::access_fixtures::insert_persons_machine(conn, owner, name)
        .await
        .unwrap()
}

#[tokio::test]
async fn imported_servers_and_mounts_keep_their_identity_without_a_machine_and_a_matching_new_machine_offers_the_server_again()
 {
    let db = TestDb::fresh().await;
    let mut conn = db.pool.acquire().await.unwrap();
    let at = clock::start();
    let owner = resources::testing::insert_person_with_home(&mut conn, "operator", at)
        .await
        .unwrap();
    let home: Id = sqlx::query_scalar("SELECT personal_org_id FROM users WHERE id=$1")
        .bind(owner)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let old = machine(&mut conn, owner, "old machine").await;
    let server = new_id();
    let spec = json!({"transport":"stdio","imported":{"source":"codex","scope":"user","directory":null,"name":"local-db","digest":"original","definition_fingerprint":"same-structural-definition","at":Time::new(at),"by":owner}});
    mcp::insert(
        &mut conn,
        &mcp::McpRow {
            imported: true,
            id: server,
            name: "local-db".into(),
            description: "kept description".into(),
            org_id: Some(home),
            home_id: Some(home),
            owner_id: Some(owner),
            machine_id: Some(old),
            environment_id: None,
            spec: spec.clone(),
            enabled: true,
            revision: 1,
        },
        at,
    )
    .await
    .unwrap();
    let mount = new_id();
    sqlx::query("INSERT INTO mounts(id,org_id,home_id,owner_id,name,target) VALUES($1,$2,$2,$3,'data','/data')").bind(mount).bind(home).bind(owner).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO mount_locations(mount_id,machine_id,kind,source,created_by,last_bytes,size_at) VALUES($1,$2,'volume','owned-data',$3,4096,$4)").bind(mount).bind(old).bind(owner).bind(at).execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE machines SET status='revoked',online=false,lost_at=$2 WHERE id=$1")
        .bind(old)
        .bind(at)
        .execute(&mut *conn)
        .await
        .unwrap();
    let retained = mcp::servers(&mut conn, &[server])
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert!(retained.imported && retained.enabled);
    assert_eq!(retained.machine_id, None);
    assert_eq!(retained.spec, spec);
    assert_eq!(retained.description, "kept description");
    assert!(
        mcp::offerings(&mut conn, &[server])
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        mounts::locations(&mut conn, &[mount])
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        mounts::lost_locations(&mut conn, mount).await.unwrap()[0].total_bytes,
        Some(4096)
    );
    let definition: bool = sqlx::query_scalar("SELECT enabled FROM mounts WHERE id=$1")
        .bind(mount)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert!(
        definition,
        "loss forgets locations without disabling their definition"
    );
    for name in ["replacement one", "replacement two"] {
        let replacement = machine(&mut conn, owner, name).await;
        let environment = new_id();
        let found = json!({"name":"local-db","source":"codex","scope":"user","directory":null,"transport":"stdio","digest":"new copy","definition_fingerprint":"same-structural-definition"});
        sqlx::query("INSERT INTO environments(id,name,owner_id,org_id,placement,home_node_id,machine_id,status,facts,updated_at) VALUES($1,$2,$3,$4,'connected',$4,$5,'running',$6,$7)").bind(environment).bind(name).bind(owner).bind(home).bind(replacement).bind(json!({"mcp_found":[found.clone()]})).bind(at).execute(&mut *conn).await.unwrap();
        let offers = mcp::offerings(&mut conn, &[server]).await.unwrap();
        assert!(
            offers
                .iter()
                .any(|offering| offering.machine_id == replacement && offering.state == "pending")
        );
        let queued:i64=sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='mcp_offering' AND plan->>'server'=$1 AND plan->>'machine'=$2").bind(server.to_string()).bind(replacement.to_string()).fetch_one(&mut *conn).await.unwrap();
        assert_eq!(queued, 1);
        sqlx::query("UPDATE environments SET facts=facts WHERE id=$1")
            .bind(environment)
            .execute(&mut *conn)
            .await
            .unwrap();
        let queued: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM jobs WHERE kind='mcp_offering' AND plan->>'machine'=$1",
        )
        .bind(replacement.to_string())
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(queued, 1, "repeated reports do not queue repeated imports");
        let job: Id = sqlx::query_scalar(
            "SELECT job_id FROM mcp_server_offerings WHERE server_id=$1 AND machine_id=$2",
        )
        .bind(server)
        .bind(replacement)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        mcp::offer_failed(
            &mut conn,
            server,
            replacement,
            new_id(),
            "an older job failed",
            at,
        )
        .await
        .unwrap();
        assert!(
            mcp::offerings(&mut conn, &[server])
                .await
                .unwrap()
                .iter()
                .any(|row| row.machine_id == replacement && row.state == "pending"),
            "an old failure cannot replace the current pending copy"
        );
        mcp::offer(
            &mut conn,
            server,
            replacement,
            Some(environment),
            &found,
            at,
        )
        .await
        .unwrap();
        mcp::offer_failed(
            &mut conn,
            server,
            replacement,
            job,
            "the old job finished late",
            at,
        )
        .await
        .unwrap();
        assert!(
            mcp::offerings(&mut conn, &[server])
                .await
                .unwrap()
                .iter()
                .any(|row| row.machine_id == replacement && row.state == "ready"),
            "a late failure cannot withdraw a manager's ready copy"
        );
    }
    let offers = mcp::offerings(&mut conn, &[server]).await.unwrap();
    assert_eq!(
        offers
            .iter()
            .filter(|offering| offering.state == "ready")
            .count(),
        2
    );
    let retained = mcp::servers(&mut conn, &[server])
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(retained.id, server);
    assert_eq!(retained.owner_id, Some(owner));
    assert_eq!(
        retained.spec, spec,
        "offerings never replace the resource's settings or imported history"
    );
}

#[tokio::test]
async fn a_different_same_name_definition_is_only_found_until_the_manager_selects_it() {
    let db = TestDb::fresh().await;
    let mut conn = db.pool.acquire().await.unwrap();
    let at = clock::start();
    let owner = resources::testing::insert_person_with_home(&mut conn, "manager", at)
        .await
        .unwrap();
    let home: Id = sqlx::query_scalar("SELECT personal_org_id FROM users WHERE id=$1")
        .bind(owner)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let old = machine(&mut conn, owner, "old").await;
    let new = machine(&mut conn, owner, "host-3").await;
    let server = new_id();
    let spec = json!({"transport":"stdio","imported":{"source":"codex","scope":"user","name":"local-db","digest":"original","definition_fingerprint":"original-definition","at":Time::new(at),"by":owner}});
    mcp::insert(
        &mut conn,
        &mcp::McpRow {
            imported: true,
            id: server,
            name: "local-db".into(),
            description: "original settings".into(),
            org_id: Some(home),
            home_id: Some(home),
            owner_id: Some(owner),
            machine_id: Some(old),
            environment_id: None,
            spec: spec.clone(),
            enabled: true,
            revision: 1,
        },
        at,
    )
    .await
    .unwrap();
    let env = new_id();
    let found = json!({"source":"codex","scope":"user","name":"local-db","directory":null,"transport":"stdio","digest":"different","definition_fingerprint":"different-definition"});
    sqlx::query("INSERT INTO environments(id,name,owner_id,org_id,placement,home_node_id,machine_id,status,facts,updated_at) VALUES($1,'Different',$2,$3,'connected',$3,$4,'running',$5,$6)").bind(env).bind(owner).bind(home).bind(new).bind(json!({"mcp_found":[found.clone()]})).bind(at).execute(&mut *conn).await.unwrap();
    let offers = mcp::offerings(&mut conn, &[server]).await.unwrap();
    let offer = offers.iter().find(|row| row.machine_id == new).unwrap();
    assert_eq!(offer.state, "found");
    assert_eq!(offer.reason.as_deref(), Some("different_definition"));
    assert_eq!(offer.offered_at, None);
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind='mcp_offering' AND plan->>'machine'=$1",
    )
    .bind(new.to_string())
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        queued, 0,
        "finding a different definition performs no import"
    );
    let mut retained = mcp::servers(&mut conn, &[server])
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(retained.spec, spec);
    retained.spec = json!({"transport":"stdio","imported":{"source":"codex","scope":"user","name":"local-db","digest":"different","definition_fingerprint":"different-definition","at":Time::new(at),"by":owner}});
    mcp::select_definition(&mut conn, &retained, new, env, at)
        .await
        .unwrap();
    let offers = mcp::offerings(&mut conn, &[server]).await.unwrap();
    assert_eq!(
        offers
            .iter()
            .find(|row| row.machine_id == new)
            .unwrap()
            .state,
        "ready"
    );
    assert_eq!(
        offers
            .iter()
            .find(|row| row.machine_id == old)
            .unwrap()
            .state,
        "found"
    );
    let selected = mcp::servers(&mut conn, &[server])
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(selected.id, server);
    assert_eq!(selected.owner_id, Some(owner));
    assert_eq!(selected.description, "original settings");
    assert_eq!(
        selected.spec["imported"]["definition_fingerprint"],
        "different-definition"
    );
}

#[tokio::test]
async fn an_operations_actual_receivers_honor_their_own_kind_choices_and_names_stay_private() {
    let db = TestDb::fresh().await;
    let mut conn = db.pool.acquire().await.unwrap();
    let reader = new_id();
    let person = new_id();
    let muted = new_id();
    let inactive = new_id();
    for (id, name) in [
        (reader, "reader"),
        (person, "private"),
        (muted, "muted"),
        (inactive, "inactive"),
    ] {
        insert_user(&mut conn, id, name).await.unwrap();
    }
    sqlx::query("INSERT INTO user_profiles(user_id,audience) VALUES($1,'private')")
        .bind(person)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("INSERT INTO notification_prefs(user_id,disabled_kinds) VALUES($1,'[\"environment.attention\"]')").bind(muted).execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE users SET status='suspended' WHERE id=$1")
        .bind(inactive)
        .execute(&mut *conn)
        .await
        .unwrap();
    let ids = [reader, person, muted, inactive];
    let actual = operations::receivers(&mut conn, &ids, "environment.attention")
        .await
        .unwrap();
    assert_eq!(actual.len(), 2);
    assert!(actual.contains(&reader) && actual.contains(&person));
    assert_eq!(
        operations::visible_names(&mut conn, reader, &actual)
            .await
            .unwrap(),
        vec![reader]
    );
    let shown = operations::receivers(&mut conn, &ids, "job.finished")
        .await
        .unwrap();
    assert_eq!(shown.len(), 3);
    assert!(shown.contains(&muted));
}
