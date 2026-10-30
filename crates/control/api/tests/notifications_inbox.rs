//! The Inbox of a person over HTTP: the list newest first, the badge's counters, what is marked read, what they chose about being notified, and the browsers that take push.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod support;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chat_world::Room;
use chrono::{DateTime, Duration, Utc};
use orch_api_contract::chats::ChatLevel;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::notifications::{
    MeNotificationPrefsGet, MeNotificationPrefsSet, MeNotificationPrefsSetBody,
    MePushSubscriptionCreate, MePushSubscriptionCreateBody, MePushSubscriptionDelete,
    MePushSubscriptionDeleteBody, MePushSubscriptionList, MePushSubscriptionRemove, Notification,
    NotificationGrouping, NotificationKind, NotificationList, NotificationListQuery,
    NotificationRead, NotificationReadBody, NotificationSummaryGet, PushSubscriptionKeys,
    QuietHours,
};
use orch_db::queries::notifications::people::{NewNotification, announce};
use orch_types::{Id, Time};
use places::{Who, make_org, person};
use serde_json::json;
use support::World;

/// A row in the Inbox of `to`, as the event it reports wrote it.
async fn tell(
    world: &World,
    to: &Who,
    title: &str,
    node: Option<Id>,
    chat: Option<Id>,
    at: DateTime<Utc>,
) -> Id {
    let mut conn = world.conn().await;
    announce(
        &mut conn,
        &NewNotification {
            user_id: to.id(),
            kind: "milestone",
            node_id: node,
            thread_id: None,
            approval_id: None,
            chat_id: chat,
            title,
            body: "",
            data: &json!({}),
            dedupe_key: &format!("milestone:{title}"),
            at,
        },
    )
    .await
    .unwrap()
    .expect("a row was written")
    .id
}

async fn list(
    world: &World,
    by: &Who,
    query: &NotificationListQuery,
) -> orch_api_contract::common::Page<Notification> {
    world
        .app
        .call(NotificationList, &[])
        .token(&by.token)
        .query(query)
        .send()
        .await
        .ok()
}

fn titles(page: &orch_api_contract::common::Page<Notification>) -> Vec<&str> {
    page.items.iter().map(|each| each.title.as_str()).collect()
}

#[tokio::test]
async fn the_inbox_lists_newest_first_and_pages_by_cursor_and_shows_nobody_elses() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let t0 = world.now();
    for (index, title) in ["one", "two", "three", "four", "five"].iter().enumerate() {
        tell(
            &world,
            &ada,
            title,
            None,
            None,
            t0 + Duration::seconds(index as i64),
        )
        .await;
    }
    tell(&world, &bea, "hers", None, None, t0).await;

    let first = list(
        &world,
        &ada,
        &NotificationListQuery {
            limit: Some(2),
            ..NotificationListQuery::default()
        },
    )
    .await;
    assert_eq!(titles(&first), ["five", "four"]);
    let second = list(
        &world,
        &ada,
        &NotificationListQuery {
            limit: Some(2),
            cursor: first.next_cursor.clone(),
            ..NotificationListQuery::default()
        },
    )
    .await;
    assert_eq!(titles(&second), ["three", "two"]);
    let third = list(
        &world,
        &ada,
        &NotificationListQuery {
            limit: Some(2),
            cursor: second.next_cursor.clone(),
            ..NotificationListQuery::default()
        },
    )
    .await;
    assert_eq!(titles(&third), ["one"]);
    assert_eq!(third.next_cursor, None);
    assert_eq!(
        titles(&list(&world, &bea, &NotificationListQuery::default()).await),
        ["hers"]
    );

    let bad = world
        .app
        .call(NotificationList, &[])
        .token(&ada.token)
        .raw_query("cursor=nope")
        .send()
        .await
        .problem();
    assert_eq!(bad.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_row_shows_the_place_it_happened_at_and_where_it_opens() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let org = make_org(&world, &ada, "Acme", "acme").await;
    let project = places::make_project(&world, &ada, org.base.id, "Web").await;
    tell(
        &world,
        &ada,
        "milestone",
        Some(project.base.id),
        None,
        world.now(),
    )
    .await;

    let shown = list(&world, &ada, &NotificationListQuery::default()).await;
    let row = &shown.items[0];
    assert_eq!(row.kind, NotificationKind::Milestone);
    let details = row
        .details
        .as_ref()
        .expect("new events carry structured action data");
    assert_eq!(details.v, 1);
    assert_eq!(details.action, NotificationKind::Milestone);
    assert_eq!(details.parameters, json!({}));
    assert_eq!(
        row.node.as_ref().and_then(|node| node.name.as_deref()),
        Some("Web")
    );
    assert_eq!(row.url, format!("/project/{}", project.base.id));
    assert_eq!(row.read_at, None);
}

#[tokio::test]
async fn the_unread_filter_leaves_out_what_was_read_and_reading_is_by_ids_a_node_a_chat_or_a_moment()
 {
    let room = Room::new().await;
    let world = &room.world;
    let ada = &room.ada;
    let project = room.project;
    let chat = room.direct(&room.ada, &room.bea).await.base.id;
    let t0 = world.now();
    let by_id = tell(world, ada, "by id", None, None, t0).await;
    tell(
        world,
        ada,
        "at the project",
        Some(project),
        None,
        t0 + Duration::seconds(1),
    )
    .await;
    tell(
        world,
        ada,
        "in the chat",
        None,
        Some(chat),
        t0 + Duration::seconds(2),
    )
    .await;
    tell(world, ada, "old", None, None, t0 - Duration::hours(1)).await;
    tell(world, ada, "new", None, None, t0 + Duration::hours(1)).await;

    let read = |body: NotificationReadBody| {
        let token = ada.token.clone();
        async move {
            world
                .app
                .call(NotificationRead, &[])
                .token(&token)
                .body(&body)
                .send()
                .await
        }
    };
    // Opening the project marks what happened there.
    let reply = read(NotificationReadBody {
        node_id: Some(project),
        ..NotificationReadBody::default()
    })
    .await
    .ok();
    assert_eq!(reply.unread, 4);
    let reply = read(NotificationReadBody {
        ids: Some(vec![by_id]),
        ..NotificationReadBody::default()
    })
    .await
    .ok();
    assert_eq!(reply.unread, 3);
    let reply = read(NotificationReadBody {
        chat_id: Some(chat),
        ..NotificationReadBody::default()
    })
    .await
    .ok();
    assert_eq!(reply.unread, 2);
    let reply = read(NotificationReadBody {
        before: Some(Time::new(t0)),
        ..NotificationReadBody::default()
    })
    .await
    .ok();
    assert_eq!(reply.unread, 1, "what was written up to that moment");

    let unread = list(
        world,
        ada,
        &NotificationListQuery {
            unread: Some(true),
            ..NotificationListQuery::default()
        },
    )
    .await;
    assert_eq!(titles(&unread), ["new"]);
    let all = list(world, ada, &NotificationListQuery::default()).await;
    assert_eq!(all.items.len(), 5);
    assert!(all.items.iter().filter(|row| row.read_at.is_some()).count() == 4);
}

#[tokio::test]
async fn reading_needs_at_least_one_member_and_touches_nobody_elses_rows() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let hers = tell(&world, &bea, "hers", None, None, world.now()).await;
    tell(&world, &ada, "mine", None, None, world.now()).await;

    let nothing = world
        .app
        .call(NotificationRead, &[])
        .token(&ada.token)
        .body(&NotificationReadBody::default())
        .send()
        .await
        .problem();
    assert_eq!(nothing.code, ErrorCode::RequestInvalid);

    // Naming her row marks nothing of hers.
    let reply = world
        .app
        .call(NotificationRead, &[])
        .token(&ada.token)
        .body(&NotificationReadBody {
            ids: Some(vec![hers]),
            ..NotificationReadBody::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(reply.unread, 1);
    let still = list(
        &world,
        &bea,
        &NotificationListQuery {
            unread: Some(true),
            ..NotificationListQuery::default()
        },
    )
    .await;
    assert_eq!(titles(&still), ["hers"]);
}

#[tokio::test]
async fn a_row_that_is_read_takes_the_next_number_of_the_inbox_so_that_every_device_learns() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let first = tell(&world, &ada, "one", None, None, world.now()).await;
    tell(&world, &ada, "two", None, None, world.now()).await;
    let before = world
        .app
        .call(NotificationSummaryGet, &[])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(
        (before.unread, before.approvals_pending, before.seq),
        (2, 0, 2)
    );

    world
        .app
        .call(NotificationRead, &[])
        .token(&ada.token)
        .body(&NotificationReadBody {
            ids: Some(vec![first]),
            ..NotificationReadBody::default()
        })
        .send()
        .await
        .ok();
    let after = world
        .app
        .call(NotificationSummaryGet, &[])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!((after.unread, after.seq), (1, 3));
    let row = list(&world, &ada, &NotificationListQuery::default())
        .await
        .items
        .into_iter()
        .find(|row| row.id == first)
        .unwrap();
    assert_eq!(row.inbox_seq, 3, "the feed carries the row again");
    assert!(row.read_at.is_some());
}

#[tokio::test]
async fn what_a_person_chose_about_being_notified_starts_as_shipped_and_changes_a_member_at_a_time()
{
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let get = |token: String| {
        let world = &world;
        async move {
            world
                .app
                .call(MeNotificationPrefsGet, &[])
                .token(&token)
                .send()
                .await
                .ok()
        }
    };
    let shipped = get(ada.token.clone()).await;
    assert!(shipped.approvals_always);
    assert_eq!(shipped.chat_node_level, ChatLevel::Mentions);
    assert_eq!(shipped.chat_direct_level, ChatLevel::All);
    assert_eq!(shipped.quiet_hours, None);

    let set = |body: MeNotificationPrefsSetBody| {
        let world = &world;
        let token = ada.token.clone();
        async move {
            world
                .app
                .call(MeNotificationPrefsSet, &[])
                .token(&token)
                .body(&body)
                .send()
                .await
        }
    };
    let changed = set(MeNotificationPrefsSetBody {
        expected_revision: shipped.revision,
        approvals_always: Some(false),
        quiet_hours: Some(Some(QuietHours {
            start_minute: 22 * 60,
            end_minute: 7 * 60,
        })),
        ..MeNotificationPrefsSetBody::default()
    })
    .await
    .ok();
    assert!(!changed.approvals_always);
    assert_eq!(
        changed.chat_node_level,
        ChatLevel::Mentions,
        "what was left out stays"
    );
    assert_eq!(
        changed.quiet_hours,
        Some(QuietHours {
            start_minute: 1320,
            end_minute: 420
        })
    );
    let levels = set(MeNotificationPrefsSetBody {
        expected_revision: changed.revision,
        chat_node_level: Some(ChatLevel::Off),
        chat_direct_level: Some(ChatLevel::Mentions),
        ..MeNotificationPrefsSetBody::default()
    })
    .await
    .ok();
    assert_eq!(levels.chat_node_level, ChatLevel::Off);
    assert_eq!(levels.chat_direct_level, ChatLevel::Mentions);
    assert_eq!(
        levels.quiet_hours, changed.quiet_hours,
        "quiet hours stay while they are left out"
    );
    assert_eq!(get(ada.token.clone()).await, levels);

    let off = set(MeNotificationPrefsSetBody {
        expected_revision: levels.revision,
        quiet_hours: Some(None),
        ..MeNotificationPrefsSetBody::default()
    })
    .await
    .ok();
    assert_eq!(off.quiet_hours, None, "null takes them away");
    assert!(!off.approvals_always);

    let stale = set(MeNotificationPrefsSetBody {
        expected_revision: levels.revision,
        approvals_always: Some(true),
        ..MeNotificationPrefsSetBody::default()
    })
    .await
    .problem();
    assert_eq!(
        stale.code,
        ErrorCode::ConflictStaleRevision,
        "a change made from an older view is refused"
    );
    assert!(
        !get(ada.token.clone()).await.approvals_always,
        "and changes nothing"
    );
}

#[tokio::test]
async fn a_pause_is_one_moment_ahead_resume_clears_it_alone_and_a_past_moment_is_refused() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let first = world
        .app
        .call(MeNotificationPrefsGet, &[])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(first.pause_until, None);
    let set = |body: MeNotificationPrefsSetBody| {
        let world = &world;
        let token = ada.token.clone();
        async move {
            world
                .app
                .call(MeNotificationPrefsSet, &[])
                .token(&token)
                .body(&body)
                .send()
                .await
        }
    };
    let past = set(MeNotificationPrefsSetBody {
        expected_revision: first.revision,
        pause_until: Some(Some(orch_types::Time::new(
            world.now() - chrono::Duration::minutes(1),
        ))),
        ..MeNotificationPrefsSetBody::default()
    })
    .await
    .problem();
    assert_eq!(past.code, ErrorCode::RequestInvalid);
    let until = orch_types::Time::new(world.now() + chrono::Duration::hours(3));
    let paused = set(MeNotificationPrefsSetBody {
        expected_revision: first.revision,
        quiet_hours: Some(Some(QuietHours {
            start_minute: 22 * 60,
            end_minute: 7 * 60,
        })),
        pause_until: Some(Some(until)),
        ..MeNotificationPrefsSetBody::default()
    })
    .await
    .ok();
    assert_eq!(paused.pause_until, Some(until));
    assert!(
        paused.revision > first.revision,
        "every change moves the revision"
    );
    let resumed = set(MeNotificationPrefsSetBody {
        expected_revision: paused.revision,
        pause_until: Some(None),
        ..MeNotificationPrefsSetBody::default()
    })
    .await
    .ok();
    assert_eq!(resumed.pause_until, None);
    assert!(
        resumed.quiet_hours.is_some(),
        "Resume leaves quiet hours as they are"
    );
}

#[tokio::test]
async fn quiet_hours_are_minutes_of_a_day() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    for (start, end, path) in [
        (1440, 10, "quiet_hours.start_minute"),
        (10, 5000, "quiet_hours.end_minute"),
    ] {
        let refused = world
            .app
            .call(MeNotificationPrefsSet, &[])
            .token(&ada.token)
            .body(&MeNotificationPrefsSetBody {
                quiet_hours: Some(Some(QuietHours {
                    start_minute: start,
                    end_minute: end,
                })),
                ..MeNotificationPrefsSetBody::default()
            })
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid);
        assert!(
            serde_json::to_string(&refused).unwrap().contains(path),
            "{refused:?}"
        );
    }
    let unknown = world
        .app
        .call(MeNotificationPrefsSet, &[])
        .token(&ada.token)
        .raw_body(&json!({ "silence": true }))
        .send()
        .await
        .problem();
    assert_eq!(unknown.code, ErrorCode::RequestInvalid);
}

fn keys() -> PushSubscriptionKeys {
    let mut point = [7_u8; 65];
    point[0] = 4;
    PushSubscriptionKeys {
        p256dh: URL_SAFE_NO_PAD.encode(point),
        auth: URL_SAFE_NO_PAD.encode([9_u8; 16]),
    }
}

#[tokio::test]
async fn the_device_list_hides_endpoints_and_only_its_person_can_remove_a_device() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    for (who, endpoint) in [
        (&ada, "https://push.example/ada-private-endpoint"),
        (&bea, "https://push.example/bea-private-endpoint"),
    ] {
        world
            .app
            .call(MePushSubscriptionCreate, &[])
            .token(&who.token)
            .header("user-agent", "Firefox/130")
            .body(&MePushSubscriptionCreateBody {
                endpoint: endpoint.to_owned(),
                keys: keys(),
            })
            .send()
            .await
            .ok();
    }
    let own = world
        .app
        .call(MePushSubscriptionList, &[])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(own.len(), 1);
    let expected: String =
        sqlx::query_scalar("SELECT encode(sha256(convert_to($1, 'UTF8')), 'hex')")
            .bind("https://push.example/ada-private-endpoint")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(own[0].endpoint_hash, expected);
    let safe = serde_json::to_string(&own).unwrap();
    assert!(!safe.contains("private-endpoint"));
    assert!(!safe.contains("p256dh"));
    assert!(!safe.contains(&keys().auth));
    let foreign = world
        .app
        .call(MePushSubscriptionRemove, &[&own[0].id])
        .token(&bea.token)
        .send()
        .await
        .problem();
    assert_eq!(foreign.code, ErrorCode::NotFound);
    world
        .app
        .call(MePushSubscriptionRemove, &[&own[0].id])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert!(
        world
            .app
            .call(MePushSubscriptionList, &[])
            .token(&ada.token)
            .send()
            .await
            .ok()
            .is_empty()
    );
    assert_eq!(
        world
            .app
            .call(MePushSubscriptionList, &[])
            .token(&bea.token)
            .send()
            .await
            .ok()
            .len(),
        1
    );
}

#[tokio::test]
async fn a_group_read_stops_at_its_cut_and_undo_leaves_later_individual_reads_alone() {
    let world = World::new().await;
    orch_api_core::routes::install_master_key(&orch_db::secrets::MasterKey::new([7; 32]));
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let org = make_org(&world, &ada, "Acme", "acme").await;
    let project = places::make_project(&world, &ada, org.base.id, "Web").await;
    let first = tell(
        &world,
        &ada,
        "first",
        Some(project.base.id),
        None,
        world.now(),
    )
    .await;
    let second = tell(
        &world,
        &ada,
        "second",
        Some(project.base.id),
        None,
        world.now(),
    )
    .await;
    let grouped = list(
        &world,
        &ada,
        &NotificationListQuery {
            group_by: Some(NotificationGrouping::Place),
            ..NotificationListQuery::default()
        },
    )
    .await;
    let group = grouped.items[0].group.as_ref().unwrap();
    assert_eq!(group.place.as_ref().unwrap().name.as_deref(), Some("Web"));
    assert_eq!(group.unread, 2);
    let newest = tell(
        &world,
        &ada,
        "newest",
        Some(project.base.id),
        None,
        world.now(),
    )
    .await;
    let read = world
        .app
        .call(NotificationRead, &[])
        .token(&ada.token)
        .body(&NotificationReadBody {
            group_key: Some(group.key.clone()),
            through_seq: Some(group.through_seq),
            ..NotificationReadBody::default()
        })
        .send()
        .await
        .ok();
    assert_eq!((read.changed, read.unread), (2, 1));
    let ticket = read.undo_token.unwrap();
    let foreign = world
        .app
        .call(NotificationRead, &[])
        .token(&bea.token)
        .body(&NotificationReadBody {
            undo_token: Some(ticket.clone()),
            ..NotificationReadBody::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(foreign.code, ErrorCode::RequestInvalid);
    world
        .app
        .call(NotificationRead, &[])
        .token(&ada.token)
        .body(&NotificationReadBody {
            ids: Some(vec![first]),
            ..NotificationReadBody::default()
        })
        .send()
        .await
        .ok();
    let undo = world
        .app
        .call(NotificationRead, &[])
        .token(&ada.token)
        .body(&NotificationReadBody {
            undo_token: Some(ticket.clone()),
            ..NotificationReadBody::default()
        })
        .send()
        .await
        .ok();
    assert_eq!((undo.changed, undo.unread), (1, 2));
    let unread = list(
        &world,
        &ada,
        &NotificationListQuery {
            unread: Some(true),
            ..NotificationListQuery::default()
        },
    )
    .await;
    assert!(unread.items.iter().any(|row| row.id == second));
    assert!(unread.items.iter().any(|row| row.id == newest));
    assert!(!unread.items.iter().any(|row| row.id == first));
    let repeated = world
        .app
        .call(NotificationRead, &[])
        .token(&ada.token)
        .body(&NotificationReadBody {
            undo_token: Some(ticket),
            ..NotificationReadBody::default()
        })
        .send()
        .await
        .ok();
    assert_eq!((repeated.changed, repeated.unread), (0, 2));
}

#[tokio::test]
async fn grouped_pages_keep_new_notifications_out_until_the_next_list() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let t0 = world.now();
    tell(&world, &ada, "older", None, None, t0).await;
    tell(&world, &ada, "newer", None, None, t0 + Duration::seconds(1)).await;
    let first = list(
        &world,
        &ada,
        &NotificationListQuery {
            group_by: Some(NotificationGrouping::Place),
            limit: Some(1),
            ..NotificationListQuery::default()
        },
    )
    .await;
    assert_eq!(titles(&first), ["newer"]);
    tell(
        &world,
        &ada,
        "arrived while paging",
        None,
        None,
        t0 + Duration::seconds(2),
    )
    .await;
    let next = list(
        &world,
        &ada,
        &NotificationListQuery {
            group_by: Some(NotificationGrouping::Place),
            limit: Some(1),
            cursor: first.next_cursor,
            ..NotificationListQuery::default()
        },
    )
    .await;
    assert_eq!(titles(&next), ["older"]);
    assert_eq!(next.items[0].group.as_ref().unwrap().unread, 2);
    let fresh = list(
        &world,
        &ada,
        &NotificationListQuery {
            group_by: Some(NotificationGrouping::Place),
            kind: Some("milestone".to_owned()),
            since: Some(Time::new(t0 + Duration::seconds(1))),
            ..NotificationListQuery::default()
        },
    )
    .await;
    assert_eq!(titles(&fresh), ["arrived while paging", "newer"]);
}

#[tokio::test]
async fn a_grouped_walk_resyncs_after_a_read_and_an_expired_undo_changes_nothing() {
    let world = World::new().await;
    orch_api_core::routes::install_master_key(&orch_db::secrets::MasterKey::new([7; 32]));
    let ada = person(&world, "ada").await;
    tell(&world, &ada, "one", None, None, world.now()).await;
    tell(&world, &ada, "two", None, None, world.now()).await;
    let first = list(
        &world,
        &ada,
        &NotificationListQuery {
            group_by: Some(NotificationGrouping::Place),
            limit: Some(1),
            ..NotificationListQuery::default()
        },
    )
    .await;
    let group = first.items[0].group.as_ref().unwrap();
    let read = world
        .app
        .call(NotificationRead, &[])
        .token(&ada.token)
        .body(&NotificationReadBody {
            group_key: Some(group.key.clone()),
            through_seq: Some(group.through_seq),
            ..NotificationReadBody::default()
        })
        .send()
        .await
        .ok();
    let changed = world
        .app
        .call(NotificationList, &[])
        .token(&ada.token)
        .query(&NotificationListQuery {
            group_by: Some(NotificationGrouping::Place),
            limit: Some(1),
            cursor: first.next_cursor,
            ..NotificationListQuery::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(changed.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(changed.extra["resync"], true);
    world.advance(Duration::hours(25));
    let expired = world
        .app
        .call(NotificationRead, &[])
        .token(&ada.token)
        .body(&NotificationReadBody {
            undo_token: read.undo_token,
            ..NotificationReadBody::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(expired.code, ErrorCode::Gone);
    assert!(
        list(
            &world,
            &ada,
            &NotificationListQuery {
                unread: Some(true),
                ..NotificationListQuery::default()
            }
        )
        .await
        .items
        .is_empty()
    );
}

#[tokio::test]
async fn a_browser_that_takes_push_is_added_renewed_and_taken_out_by_its_person_alone() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let endpoint = "https://push.example/send/abc";
    let subscribe = |who: &Who, endpoint: &str, keys: PushSubscriptionKeys| {
        let world = &world;
        let token = who.token.clone();
        let endpoint = endpoint.to_owned();
        async move {
            world
                .app
                .call(MePushSubscriptionCreate, &[])
                .token(&token)
                .header("user-agent", "Firefox/130")
                .body(&MePushSubscriptionCreateBody { endpoint, keys })
                .send()
                .await
        }
    };
    let added = subscribe(&ada, endpoint, keys()).await;
    assert_eq!(added.status, 204, "{:?}", added.body);
    let rows: Vec<(Id, String)> =
        sqlx::query_as("SELECT user_id, user_agent FROM push_subscriptions")
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(rows, [(ada.id(), "Firefox/130".to_owned())]);

    // The same browser again is one row, not two.
    subscribe(&ada, endpoint, keys()).await.ok();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM push_subscriptions")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    // Another person cannot take it out; she is answered as if she had.
    let gone = world
        .app
        .call(MePushSubscriptionDelete, &[])
        .token(&bea.token)
        .body(&MePushSubscriptionDeleteBody {
            endpoint: endpoint.to_owned(),
        })
        .send()
        .await;
    assert_eq!(gone.status, 204);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM push_subscriptions")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let own = world
        .app
        .call(MePushSubscriptionDelete, &[])
        .token(&ada.token)
        .body(&MePushSubscriptionDeleteBody {
            endpoint: endpoint.to_owned(),
        })
        .send()
        .await;
    assert_eq!(own.status, 204);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM push_subscriptions")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn a_browser_is_refused_for_an_address_that_is_no_push_service_or_keys_that_are_none() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let mut short = keys();
    short.p256dh = URL_SAFE_NO_PAD.encode([4_u8; 10]);
    let mut weak = keys();
    weak.auth = URL_SAFE_NO_PAD.encode([9_u8; 4]);
    for (endpoint, keys, path) in [
        ("http://push.example/send/abc", keys(), "endpoint"),
        ("not an address", keys(), "endpoint"),
        ("", keys(), "endpoint"),
        ("https://push.example/send/abc", short, "keys.p256dh"),
        ("https://push.example/send/abc", weak, "keys.auth"),
    ] {
        let refused = world
            .app
            .call(MePushSubscriptionCreate, &[])
            .token(&ada.token)
            .body(&MePushSubscriptionCreateBody {
                endpoint: endpoint.to_owned(),
                keys,
            })
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid, "{endpoint}");
        assert!(
            serde_json::to_string(&refused).unwrap().contains(path),
            "{refused:?}"
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM push_subscriptions")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn nobody_who_is_not_signed_in_reads_or_changes_an_inbox() {
    let world = World::new().await;
    let signed_out = [
        world.app.call(NotificationList, &[]).send().await.problem(),
        world
            .app
            .call(NotificationSummaryGet, &[])
            .send()
            .await
            .problem(),
        world
            .app
            .call(NotificationRead, &[])
            .body(&NotificationReadBody {
                before: Some(Time::new(world.now())),
                ..NotificationReadBody::default()
            })
            .send()
            .await
            .problem(),
        world
            .app
            .call(MeNotificationPrefsGet, &[])
            .send()
            .await
            .problem(),
        world
            .app
            .call(MeNotificationPrefsSet, &[])
            .body(&MeNotificationPrefsSetBody::default())
            .send()
            .await
            .problem(),
        world
            .app
            .call(MePushSubscriptionCreate, &[])
            .body(&MePushSubscriptionCreateBody {
                endpoint: "https://push.example/x".to_owned(),
                keys: keys(),
            })
            .send()
            .await
            .problem(),
        world
            .app
            .call(MePushSubscriptionDelete, &[])
            .body(&MePushSubscriptionDeleteBody {
                endpoint: "https://push.example/x".to_owned(),
            })
            .send()
            .await
            .problem(),
    ];
    for problem in signed_out {
        assert_eq!(problem.code, ErrorCode::AuthRequired);
    }
}
