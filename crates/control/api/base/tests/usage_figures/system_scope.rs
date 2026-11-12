//! System totals use the same counted sources, filters, records and CSV across distinct root orgs.
use super::*;

#[tokio::test]
async fn system_usage_adds_distinct_roots_once_and_keeps_person_node_record_and_csv_filters() {
    let world = World::new().await;
    let first = team(&world).await;
    let other_org = orch_types::new_id();
    let project = orch_types::new_id();
    let topic = orch_types::new_id();
    let thread = orch_types::new_id();
    let mut conn = world.conn().await;
    for (id, kind, parent, name) in [
        (other_org, "org", None, "Other company"),
        (project, "project", Some(other_org), "Other project"),
        (topic, "topic", Some(project), "Other topic"),
        (thread, "thread", Some(topic), "Other work"),
    ] {
        orch_db::queries::hierarchy::testing::insert_node(
            &mut conn,
            id,
            kind,
            parent,
            name,
            first.stranger.id(),
        )
        .await
        .unwrap();
    }
    drop(conn);
    let outside = Team {
        boss: first.stranger.clone(),
        admin: first.stranger.clone(),
        viewer: first.stranger.clone(),
        stranger: first.boss.clone(),
        sysadmin: first.sysadmin.clone(),
        acme: other_org,
        project,
        topic,
        thread,
        provider: first.provider,
    };
    spend(
        &world,
        &first,
        &Use::of(&world, first.boss.id(), 100).rated("1", "catalogue"),
    )
    .await;
    spend(
        &world,
        &outside,
        &Use::of(&world, outside.boss.id(), 200).rated("2", "catalogue"),
    )
    .await;
    spend(&world, &first, &Use::of(&world, first.sysadmin.id(), 40)).await;
    spend(
        &world,
        &outside,
        &Use::of(&world, outside.boss.id(), 999).trace(),
    )
    .await;
    for who in [&first.boss, &first.admin, &first.viewer] {
        let refused = world
            .app
            .call(UsageGet, &[])
            .token(&who.token)
            .query(&query("system"))
            .send()
            .await;
        assert_eq!(refused.status, http::StatusCode::NOT_FOUND);
    }
    places::admin_mode(&world, &first.sysadmin, false).await;
    assert_eq!(
        world
            .app
            .call(UsageGet, &[])
            .token(&first.sysadmin.token)
            .query(&query("system"))
            .send()
            .await
            .status,
        http::StatusCode::NOT_FOUND
    );
    places::admin_mode(&world, &first.sysadmin, true).await;
    let mut asked = query("system");
    asked.bucket = Some(UsageBucket::Day);
    let global = figures(&world, &first.sysadmin, &asked).await;
    assert_eq!(global.scope, "system");
    assert_eq!(global.depth, UsageDepth::Reports);
    assert_eq!(global.totals.tokens.total_tokens, 340);
    assert_eq!(global.totals.credits, "3");
    assert_eq!(global.me.as_ref().unwrap().tokens.total_tokens, 40);
    assert_eq!(
        global
            .series
            .iter()
            .map(|point| point.figures.tokens.total_tokens)
            .sum::<i64>(),
        340
    );
    let orgs = global
        .breakdowns
        .iter()
        .find(|by| by.by == UsageDimension::Org)
        .unwrap();
    assert_eq!(orgs.rows.len(), 2);
    assert_eq!(
        orgs.rows
            .iter()
            .map(|row| row.figures.tokens.total_tokens)
            .sum::<i64>(),
        340
    );
    assert_eq!(
        figures(&world, &first.boss, &query(&format!("node:{}", first.acme)))
            .await
            .totals
            .tokens
            .total_tokens,
        140
    );
    assert_eq!(
        figures(&world, &first.boss, &query("me"))
            .await
            .totals
            .tokens
            .total_tokens,
        100
    );
    let mut by_org = asked.clone();
    by_org.org_id = Some(other_org);
    assert_eq!(
        figures(&world, &first.sysadmin, &by_org)
            .await
            .totals
            .tokens
            .total_tokens,
        200
    );
    let mut by_person = asked.clone();
    by_person.user_id = Some(first.boss.id());
    assert_eq!(
        figures(&world, &first.sysadmin, &by_person)
            .await
            .totals
            .tokens
            .total_tokens,
        100
    );
    let mut records = events_query("system");
    records.counted = Some(true);
    let rows = events(&world, &first.sysadmin, &records).await;
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows.iter().map(|row| row.tokens.total_tokens).sum::<i64>(),
        340
    );
    let csv = world
        .app
        .call(UsageGet, &[])
        .token(&first.sysadmin.token)
        .query(&asked)
        .header("accept", "text/csv")
        .send_bytes()
        .await;
    assert!(csv.status.is_success());
    let csv = String::from_utf8(csv.raw.to_vec()).unwrap();
    assert!(csv.lines().next().unwrap().starts_with("at,turns,"));
    assert!(csv.contains(",340,"));
    places::admin_mode(&world, &first.sysadmin, false).await;
    assert_eq!(
        world
            .app
            .call(UsageEventList, &[])
            .token(&first.sysadmin.token)
            .query(&records)
            .send()
            .await
            .status,
        http::StatusCode::NOT_FOUND
    );
}
