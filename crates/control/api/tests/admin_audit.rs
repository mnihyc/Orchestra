//! The audit trail in Management: the whole system's trail with its filters, and a span of it written out as a file that holds what the list holds.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod admin_support;
mod places;
mod support;

use admin_support::{Cast, assert_no_system_admin, cast, lines_of, refused_to};
use chrono::{DateTime, Duration, Utc};
use orch_api_contract::admin::{
    AUDIT_EXPORT_MAX_DAYS, AdminAuditExport, AdminAuditExportQuery, AdminAuditList,
    AdminAuditListQuery, AdminOrgUpdate, AdminSettingSet, AdminSettingSetBody, AdminUserCreate,
    AdminUserCreateBody, AuditExportFormat, OrgLimitsChange,
};
use orch_api_contract::common::{ErrorCode, Page};
use orch_api_contract::nodes::{AuditActor, AuditEntry};
use orch_api_core::testing::RawReply;
use orch_types::{Id, Time};
use places::Who;
use serde_json::json;
use support::World;

/// Something each of three kinds of act did, ten minutes apart.
async fn some_acts(world: &World, c: &Cast) {
    world.advance(Duration::minutes(10));
    world
        .app
        .call(AdminSettingSet, &[&"hosts.full_percent"])
        .token(&c.root.token)
        .body(&AdminSettingSetBody {
            value: json!(75),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    world.advance(Duration::minutes(10));
    world
        .app
        .call(AdminOrgUpdate, &[&c.acme])
        .token(&c.root.token)
        .body(&OrgLimitsChange {
            closed: Some(true),
            flat: None,
        })
        .send()
        .await
        .ok();
    world.advance(Duration::minutes(10));
    world
        .app
        .call(AdminUserCreate, &[])
        .token(&c.root.token)
        .body(&AdminUserCreateBody {
            username: "dan".into(),
            display_name: "Dan".into(),
            email: None,
            is_system_admin: None,
            personal_org: None,
        })
        .send()
        .await
        .ok();
}

async fn trail(world: &World, by: &Who, query: &AdminAuditListQuery) -> Page<AuditEntry> {
    world
        .app
        .call(AdminAuditList, &[])
        .token(&by.token)
        .query(query)
        .send()
        .await
        .ok()
}

/// Every entry of the trail, a page of `size` at a time.
async fn whole_trail(world: &World, by: &Who, size: u32) -> Vec<AuditEntry> {
    let mut all = Vec::new();
    let mut cursor = None;
    loop {
        let page = trail(
            world,
            by,
            &AdminAuditListQuery {
                limit: Some(size),
                cursor: cursor.clone(),
                ..AdminAuditListQuery::default()
            },
        )
        .await;
        all.extend(page.items);
        cursor = page.next_cursor;
        if cursor.is_none() {
            return all;
        }
    }
}

async fn export(
    world: &World,
    by: &Who,
    format: AuditExportFormat,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> RawReply {
    world
        .app
        .call(AdminAuditExport, &[])
        .token(&by.token)
        .query(&AdminAuditExportQuery {
            format,
            from: Time::new(from),
            to: Time::new(to),
            actor: None,
            include_agents: None,
            action: None,
            kind: None,
            node_id: None,
        })
        .send_bytes()
        .await
}

fn jsonl(reply: &RawReply) -> Vec<AuditEntry> {
    std::str::from_utf8(&reply.raw)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test]
async fn the_audit_trail_of_the_system_is_listed_newest_first_and_holds_what_is_about_no_node() {
    let world = World::new().await;
    let c = cast(&world).await;
    some_acts(&world, &c).await;

    let all = whole_trail(&world, &c.root, 200).await;

    assert!(
        all.windows(2)
            .all(|pair| pair[0].at.get() >= pair[1].at.get()),
        "newest first"
    );
    assert_eq!(
        all[0].action, "auth.password_reset",
        "the last thing done, the link for the person made last, is the first shown"
    );
    let setting = all
        .iter()
        .find(|entry| entry.action == "system.settings_updated")
        .unwrap();
    assert!(
        setting.node.is_none(),
        "the system's own acts are about no node"
    );
    assert!(matches!(&setting.actor, AuditActor::User { user } if user.username == "root"));
    let limits = all
        .iter()
        .find(|entry| entry.action == "org.limits_changed")
        .unwrap();
    assert_eq!(limits.node.as_ref().unwrap().id, c.acme);
    assert_eq!(limits.node.as_ref().unwrap().name.as_deref(), Some("Acme"));
}

#[tokio::test]
async fn the_trail_is_narrowed_by_who_did_it_what_it_was_which_node_and_when() {
    let world = World::new().await;
    let c = cast(&world).await;
    let started = world.now();
    some_acts(&world, &c).await;
    let (here, root) = (&world, &c.root);
    let ask = |query: AdminAuditListQuery| async move { trail(here, root, &query).await };

    let by_action = ask(AdminAuditListQuery {
        action: Some("org.limits_changed".into()),
        ..AdminAuditListQuery::default()
    })
    .await;
    let by_actor = ask(AdminAuditListQuery {
        actor: Some(c.root.id()),
        ..AdminAuditListQuery::default()
    })
    .await;
    let by_node = ask(AdminAuditListQuery {
        node_id: Some(c.acme),
        ..AdminAuditListQuery::default()
    })
    .await;
    let by_moment = ask(AdminAuditListQuery {
        from: Some(Time::new(started + Duration::minutes(15))),
        to: Some(Time::new(started + Duration::minutes(25))),
        ..AdminAuditListQuery::default()
    })
    .await;

    assert_eq!(by_action.items.len(), 1);
    assert!(!by_actor.items.is_empty());
    assert!(by_actor.items.iter().all(|entry| matches!(
        &entry.actor,
        AuditActor::User { user } if user.id == c.root.id()
    )));
    assert!(
        by_node
            .items
            .iter()
            .any(|entry| entry.action == "org.limits_changed")
    );
    assert!(
        by_node
            .items
            .iter()
            .all(|entry| entry.node.is_some() && entry.action != "system.settings_updated"),
        "what concerns the org and what lies in it, and nothing about the system"
    );
    assert_eq!(
        by_moment.items.len(),
        1,
        "one act fell in those ten minutes"
    );
    assert_eq!(by_moment.items[0].action, "org.limits_changed");
}

#[tokio::test]
async fn the_trail_goes_on_from_the_page_before_until_every_row_was_shown_once() {
    let world = World::new().await;
    let c = cast(&world).await;
    some_acts(&world, &c).await;
    let in_the_table: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();

    let by_threes = whole_trail(&world, &c.root, 3).await;
    let at_once = whole_trail(&world, &c.root, 200).await;

    assert_eq!(i64::try_from(by_threes.len()).unwrap(), in_the_table);
    assert_eq!(
        by_threes, at_once,
        "the pages are the whole, in the same order"
    );
    let mut ids: Vec<Id> = by_threes.iter().map(|entry| entry.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), by_threes.len(), "nobody twice");
}

#[tokio::test]
async fn the_export_holds_what_the_list_holds_as_json_lines_and_as_comma_separated_values() {
    let world = World::new().await;
    let c = cast(&world).await;
    some_acts(&world, &c).await;
    let started = world.now() - Duration::hours(2);
    let until = world.now();
    // The copy is taken a moment later, so that the line it leaves is not part of what it holds.
    world.advance(Duration::minutes(1));
    let listed = whole_trail(&world, &c.root, 200).await;
    let listed: Vec<AuditEntry> = listed
        .into_iter()
        .filter(|entry| entry.at.get() <= until)
        .collect();

    let lines = export(&world, &c.root, AuditExportFormat::Jsonl, started, until).await;
    let values = export(&world, &c.root, AuditExportFormat::Csv, started, until).await;

    assert_eq!(lines.status, 200);
    assert_eq!(
        lines.headers["content-type"], "application/x-ndjson",
        "{:?}",
        lines.headers
    );
    assert!(
        lines.headers["content-disposition"]
            .to_str()
            .unwrap()
            .contains(".jsonl")
    );
    let written = jsonl(&lines);
    let mut oldest_first = listed.clone();
    oldest_first.reverse();
    assert_eq!(
        written, oldest_first,
        "the file is the list, the oldest first"
    );
    let csv = std::str::from_utf8(&values.raw).unwrap();
    let rows: Vec<&str> = csv.split("\r\n").filter(|row| !row.is_empty()).collect();
    assert!(rows[0].starts_with("id,at,actor_kind,actor_id,actor,action,"));
    assert_eq!(
        rows.len(),
        1 + listed.len(),
        "a header and a row for each entry"
    );
    assert!(rows[1].starts_with(&oldest_first[0].id.to_string()));
    assert!(rows[1].contains(&oldest_first[0].action));
}

#[tokio::test]
async fn a_cell_of_the_file_that_a_spreadsheet_would_run_is_written_as_text() {
    let world = World::new().await;
    let c = cast(&world).await;
    sqlx::query("UPDATE users SET display_name = '=cmd|x' WHERE id = $1")
        .bind(c.root.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    some_acts(&world, &c).await;

    let values = export(
        &world,
        &c.root,
        AuditExportFormat::Csv,
        world.now() - Duration::days(1),
        world.now(),
    )
    .await;

    let csv = std::str::from_utf8(&values.raw).unwrap();
    assert!(csv.contains("'=cmd|x made dan"), "{csv}");
    assert!(
        csv.split("\r\n")
            .all(|row| !row.contains(",=cmd") && !row.starts_with('=')),
        "no cell begins with ="
    );
}

#[tokio::test]
async fn taking_a_copy_leaves_its_line_before_the_first_row_is_read() {
    let world = World::new().await;
    let c = cast(&world).await;
    let (from, to) = (
        world.now() - Duration::hours(1),
        world.now() + Duration::hours(1),
    );

    let copy = export(&world, &c.root, AuditExportFormat::Jsonl, from, to).await;

    let written = jsonl(&copy);
    let own = written
        .iter()
        .find(|entry| entry.action == "audit.exported")
        .expect("the copy holds the line of its own making");
    assert!(matches!(&own.actor, AuditActor::User { user } if user.id == c.root.id()));
    assert_eq!(own.details["format"], json!("jsonl"));
    let lines = lines_of(&world, "audit.exported").await;
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].actor_id, Some(c.root.id()));
}

#[tokio::test]
async fn a_span_backwards_or_longer_than_a_year_is_refused_and_leaves_no_line() {
    let world = World::new().await;
    let c = cast(&world).await;
    let now = world.now();
    let year = Duration::days(i64::from(AUDIT_EXPORT_MAX_DAYS));

    let backwards = export(
        &world,
        &c.root,
        AuditExportFormat::Csv,
        now,
        now - Duration::hours(1),
    )
    .await;
    let too_long = export(
        &world,
        &c.root,
        AuditExportFormat::Csv,
        now - year - Duration::seconds(1),
        now,
    )
    .await;
    let a_year = export(&world, &c.root, AuditExportFormat::Csv, now - year, now).await;

    for refused in [&backwards, &too_long] {
        let problem = refused.problem();
        assert_eq!(problem.code, ErrorCode::RequestInvalid);
        assert_eq!(problem.extra["errors"][0]["path"], json!("to"));
    }
    assert_eq!(a_year.status, 200, "a year is the most");
    assert_eq!(
        lines_of(&world, "audit.exported").await.len(),
        1,
        "only the copy that was taken"
    );
}

#[tokio::test]
async fn a_file_of_many_rows_is_written_whole_across_its_chunks() {
    let world = World::new().await;
    let c = cast(&world).await;
    let at = world.now() - Duration::days(3);
    sqlx::query(
        "INSERT INTO audit_log (id, at, actor_kind, action, object_kind, summary)
         SELECT gen_random_uuid(), $1::timestamptz + make_interval(secs => g), 'system',
                'system.settings_updated', 'system', 'filler ' || g
           FROM generate_series(1, 1203) g",
    )
    .bind(at)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let held: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE at <= $1")
        .bind(world.now())
        .fetch_one(&world.db.pool)
        .await
        .unwrap();

    let copy = export(
        &world,
        &c.root,
        AuditExportFormat::Jsonl,
        world.now() - Duration::days(10),
        world.now(),
    )
    .await;

    let written = jsonl(&copy);
    assert!(held > 1203);
    // The copy leaves a line of its own at the moment it is taken, which the span includes.
    assert_eq!(i64::try_from(written.len()).unwrap(), held + 1);
    let mut ids: Vec<Id> = written.iter().map(|entry| entry.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), written.len(), "no row twice across the chunks");
    assert!(
        written
            .windows(2)
            .all(|pair| pair[0].at.get() <= pair[1].at.get()),
        "oldest first"
    );
}

#[tokio::test]
async fn somebody_who_is_no_system_admin_is_refused_the_trail_and_its_export_and_so_is_an_orgs_admin()
 {
    let world = World::new().await;
    let c: Cast = cast(&world).await;
    let span = format!(
        "format=csv&from={}&to={}",
        Time::new(world.now() - Duration::days(1)),
        Time::new(world.now())
    );

    for (who, name) in [(&c.boss, "an org's admin"), (&c.ada, "an ordinary person")] {
        let list = refused_to(&world, AdminAuditList, &[], who, None, "").await;
        let copy = refused_to(&world, AdminAuditExport, &[], who, None, &span).await;
        assert_no_system_admin(&list, name);
        assert_no_system_admin(&copy, name);
    }
    assert!(
        lines_of(&world, "audit.exported").await.is_empty(),
        "a refused copy leaves no line of a copy"
    );
}
