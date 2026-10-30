//! The tree of an org, the topics of a project, what was shared by name, a place's Activity and its audit trail, over HTTP.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod reader_diagnostics;
mod support;

use orch_api_contract::common::{ErrorCode, PageQuery};
use orch_api_contract::nodes::{
    ActivityKind, ArchivedFilter, NewNodeAccess, Node, NodeActivityList, NodeActivityListQuery,
    NodeAuditList, NodeAuditListQuery, NodeUpdate, NodeUpdateBody, OrgTreeGet, OrgTreeGetQuery,
    ProjectCreate, ProjectCreateBody, SharedList, SharedListItem, TREE_SYNC_MAX_CHANGES, TopicList,
    TopicListQuery, TopicSort, TopicStatus, TopicStatusFilter, TopicUpdate, TopicUpdateBody,
};
use orch_db::queries::access::testing as access_rows;
use orch_db::queries::hierarchy;
use orch_types::{Id, new_id};
use places::{Who, join, make_org, make_project, make_topic, person};
use support::{Statements, World};

/// An org of `boss` with a viewer `dan` and a project with two topics.
struct Tree {
    boss: Who,
    dan: Who,
    acme: Id,
    web: Id,
    topics: Vec<Id>,
}

async fn tree(world: &World) -> Tree {
    let boss = person(world, "boss").await;
    let dan = person(world, "dan").await;
    let acme = make_org(world, &boss, "Acme", "acme").await;
    join(world, acme.base.id, acme.base.id, &dan, "viewer", &boss).await;
    let web = make_project(world, &boss, acme.base.id, "Web").await;
    let mut topics = Vec::new();
    for name in ["Login", "Billing"] {
        topics.push(make_topic(world, &boss, web.base.id, name).await.base.id);
    }
    Tree {
        boss,
        dan,
        acme: acme.base.id,
        web: web.base.id,
        topics,
    }
}

#[tokio::test]
async fn a_tree_holds_every_node_the_caller_may_view_with_its_marks() {
    let world = World::new().await;
    let t = tree(&world).await;
    let reply = world
        .app
        .call(OrgTreeGet, &[&t.acme])
        .token(&t.boss.token)
        .query(&OrgTreeGetQuery::default())
        .send()
        .await
        .ok();
    assert!(reply.complete);
    assert_eq!(reply.since, None);
    assert!(reply.seq >= 1);
    // The org, General, Web and its two topics.
    assert_eq!(reply.readable().len(), 5);
    let login = reply
        .readable()
        .into_iter()
        .find(|n| n.id == t.topics[0])
        .unwrap();
    assert_eq!(login.name, "Login");
    assert_eq!(login.parent_id, Some(t.web));
    assert_eq!(login.topic.as_ref().unwrap().status, TopicStatus::Open);
    assert!(login.topic.as_ref().unwrap().tag_ids.is_empty());
    assert!(!login.chat.id.is_nil());
    assert!(!login.deleted && !login.archived && !login.restricted);
    assert!(login.tree_seq > 0);
}

#[tokio::test]
async fn a_full_tree_pages_past_the_delta_bound_and_keeps_its_initial_sequence() {
    let world = World::new().await;
    let t = tree(&world).await;
    let count = TREE_SYNC_MAX_CHANGES + 2;
    {
        let mut conn = world.conn().await;
        sqlx::query("INSERT INTO nodes (id,kind,parent_id,org_id,path,name,owner_id,created_by,created_at,updated_at)
            SELECT id,'project',$1,$1,ARRAY[$1,id]::uuid[],'Project ' || number,$2,$2,$4,$4
            FROM (SELECT ('ffffffff-ffff-7fff-8fff-' || lpad(to_hex(number),12,'0'))::uuid id,number
                  FROM generate_series(1,$3::bigint) number) made")
            .bind(t.acme).bind(t.boss.id()).bind(count).bind(world.now())
            .execute(&mut *conn).await.unwrap();
        // A function called a whole tree cannot silently apply its delta limit.
        assert_eq!(
            hierarchy::tree(&mut conn, t.acme, None, 1)
                .await
                .unwrap()
                .len(),
            usize::try_from(count + 5).unwrap()
        );
    }
    let first = world
        .app
        .call(OrgTreeGet, &[&t.acme])
        .token(&t.dan.token)
        .query(&OrgTreeGetQuery::default())
        .send()
        .await
        .ok();
    assert!(!first.complete);
    assert_eq!(
        first.nodes.len(),
        usize::try_from(TREE_SYNC_MAX_CHANGES).unwrap()
    );
    let seq = first.seq;
    // A change between pages must not advance the full-load watermark past
    // a row in an already delivered page. The live feed repairs from `seq`.
    let changed_seq = {
        let mut conn = world.conn().await;
        hierarchy::stamp(&mut conn, t.web).await.unwrap();
        hierarchy::tree_seq(&mut conn, t.acme).await.unwrap()
    };
    assert!(changed_seq > seq);
    let mut ids: std::collections::HashSet<_> =
        first.readable().iter().map(|node| node.id).collect();
    let mut cursor = first.next_cursor;
    while let Some(next) = cursor {
        let page = world
            .app
            .call(OrgTreeGet, &[&t.acme])
            .token(&t.dan.token)
            .query(&OrgTreeGetQuery {
                cursor: Some(next),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert_eq!(page.seq, seq);
        assert_eq!(page.complete, page.next_cursor.is_none());
        for node in page.readable() {
            assert!(ids.insert(node.id), "pages never repeat an id");
        }
        cursor = page.next_cursor;
    }
    assert_eq!(ids.len(), usize::try_from(count + 5).unwrap());
    let newest: Id = format!("ffffffff-ffff-7fff-8fff-{:012x}", count)
        .parse()
        .unwrap();
    assert!(
        ids.contains(&newest),
        "the old oldest-first cap lost this last node"
    );
    let delta = world
        .app
        .call(OrgTreeGet, &[&t.acme])
        .token(&t.dan.token)
        .query(&OrgTreeGetQuery {
            since: Some(seq),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(delta.readable().iter().any(|node| node.id == t.web));
}

#[tokio::test]
async fn an_empty_visible_tree_page_still_continues_to_a_readable_shared_node() {
    let world = World::new().await;
    let t = tree(&world).await;
    let eve = person(&world, "eve").await;
    {
        let mut conn = world.conn().await;
        access_rows::add_entry(
            &mut conn,
            &orch_db::queries::access::PageKey::new("node", t.topics[1], "live"),
            "any",
            "topic.view",
            ("user", Some(eve.id()), None),
            None,
            world.now(),
        )
        .await
        .unwrap();
    }
    let mut cursor = None;
    let mut saw_empty = false;
    let mut visible = Vec::new();
    loop {
        let page = world
            .app
            .call(OrgTreeGet, &[&t.acme])
            .token(&eve.token)
            .query(&OrgTreeGetQuery {
                cursor,
                limit: Some(1),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        saw_empty |= page.nodes.is_empty() && page.next_cursor.is_some();
        visible.extend(page.readable().into_iter().map(|node| node.id));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert!(saw_empty);
    assert_eq!(visible, vec![t.topics[1]]);
}

#[tokio::test]
async fn a_tree_leaves_out_what_the_caller_may_not_view_and_marks_what_is_private() {
    let world = World::new().await;
    let t = tree(&world).await;
    let secret = world
        .app
        .call(ProjectCreate, &[&t.acme])
        .token(&t.boss.token)
        .body(&ProjectCreateBody {
            name: "Secret".to_owned(),
            description: None,
            access: Some(NewNodeAccess::Private),
        })
        .send()
        .await
        .ok();
    let call = |who: &Who| {
        world
            .app
            .call(OrgTreeGet, &[&t.acme])
            .token(&who.token)
            .query(&OrgTreeGetQuery::default())
            .send()
    };
    let boss_sees = call(&t.boss).await.ok();
    let mine = boss_sees
        .readable()
        .into_iter()
        .find(|n| n.id == secret.base.id)
        .unwrap();
    assert!(mine.restricted);
    let dan_sees = call(&t.dan).await.ok();
    assert!(
        dan_sees
            .readable()
            .into_iter()
            .all(|n| n.id != secret.base.id)
    );
    assert!(dan_sees.hidden().is_empty(), "dan administers nothing here");
    assert_eq!(dan_sees.nodes.len(), 5);
    let eve = person(&world, "eve").await;
    let nothing = call(&eve).await.ok();
    assert!(nothing.nodes.is_empty(), "a stranger sees no node of it");
}

#[tokio::test]
async fn a_tree_since_a_sequence_holds_what_changed_and_marks_what_is_gone_from_view() {
    let world = World::new().await;
    let t = tree(&world).await;
    let first = world
        .app
        .call(OrgTreeGet, &[&t.acme])
        .token(&t.dan.token)
        .query(&OrgTreeGetQuery::default())
        .send()
        .await
        .ok();
    // One topic is renamed; the project is cut loose from the viewers.
    let login = match places::read(&world, &t.boss, t.topics[0]).await {
        Node::Topic(topic) => topic,
        _ => panic!("a topic stays one"),
    };
    world
        .app
        .call(NodeUpdate, &[&t.topics[0]])
        .token(&t.boss.token)
        .body(&NodeUpdateBody {
            readme_document_id: None,
            color: None,
            custom_color: None,
            name: Some("Sign in".to_owned()),
            description: None,
            expected_revision: login.base.revision,
        })
        .send()
        .await
        .ok();
    let since = world
        .app
        .call(OrgTreeGet, &[&t.acme])
        .token(&t.dan.token)
        .query(&OrgTreeGetQuery {
            since: Some(first.seq),
            ..OrgTreeGetQuery::default()
        })
        .send()
        .await
        .ok();
    assert!(since.complete);
    assert_eq!(since.since, Some(first.seq));
    assert_eq!(since.nodes.len(), 1);
    assert_eq!(since.readable()[0].name, "Sign in");
    assert!(since.seq > first.seq);

    // Cutting the project's View row loose: dan no longer sees it, and the
    // view learns that, without a name.
    let mut conn = world.conn().await;
    sqlx::query(
        "INSERT INTO permission_rows (object_kind, object_id, page, profile, permission, inherit)
         VALUES ('node', $1, 'live', 'any', 'project.view', false)",
    )
    .bind(t.web)
    .execute(&mut *conn)
    .await
    .unwrap();
    hierarchy::stamp(&mut conn, t.web).await.unwrap();
    drop(conn);
    let after = world
        .app
        .call(OrgTreeGet, &[&t.acme])
        .token(&t.dan.token)
        .query(&OrgTreeGetQuery {
            since: Some(since.seq),
            ..OrgTreeGetQuery::default()
        })
        .send()
        .await
        .ok();
    let gone = after
        .readable()
        .into_iter()
        .find(|n| n.id == t.web)
        .unwrap();
    assert!(gone.deleted);
    assert_eq!(
        gone.name, "",
        "a node that is not theirs to see is not named"
    );
}

#[tokio::test]
async fn a_tree_costs_the_same_statements_for_a_few_nodes_and_for_many() {
    let world = World::new().await;
    let t = tree(&world).await;
    async fn measure(world: &World, who: &Who, org: Id) -> (usize, usize) {
        let call = world
            .app
            .call(OrgTreeGet, &[&org])
            .token(&who.token)
            .query(&OrgTreeGetQuery::default());
        let before = Statements::now();
        let reply = call.send().await.ok();
        (Statements::now() - before, reply.nodes.len())
    }
    let (few, few_count) = measure(&world, &t.dan, t.acme).await;
    for index in 0..10 {
        let project = make_project(&world, &t.boss, t.acme, &format!("P{index}")).await;
        make_topic(&world, &t.boss, project.base.id, "T").await;
    }
    let (many, many_count) = measure(&world, &t.dan, t.acme).await;
    assert_eq!((few_count, many_count), (5, 25));
    assert_eq!(few, many, "{few} statements for 5 nodes, {many} for 25");
}

#[tokio::test]
async fn a_tree_is_asked_of_a_root_org_and_of_nothing_else() {
    let world = World::new().await;
    let t = tree(&world).await;
    for id in [t.web, new_id()] {
        let refused = world
            .app
            .call(OrgTreeGet, &[&id])
            .token(&t.boss.token)
            .query(&OrgTreeGetQuery::default())
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::NotFound);
    }
}

async fn topics(world: &World, who: &Who, project: Id, query: &TopicListQuery) -> Vec<String> {
    world
        .app
        .call(TopicList, &[&project])
        .token(&who.token)
        .query(query)
        .send()
        .await
        .ok()
        .items
        .into_iter()
        .filter_map(orch_api_contract::common::Listed::into_visible)
        .map(|topic| topic.base.name)
        .collect()
}

#[tokio::test]
async fn the_open_topics_of_a_project_are_listed_and_done_ones_when_asked() {
    let world = World::new().await;
    let t = tree(&world).await;
    world
        .app
        .call(TopicUpdate, &[&t.topics[1]])
        .token(&t.boss.token)
        .body(&TopicUpdateBody {
            status: Some(TopicStatus::Done),
            tag_ids: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    let open = topics(&world, &t.boss, t.web, &TopicListQuery::default()).await;
    assert_eq!(open, vec!["Login".to_owned()]);
    let done = TopicListQuery {
        status: Some(TopicStatusFilter::Done),
        ..TopicListQuery::default()
    };
    assert_eq!(
        topics(&world, &t.boss, t.web, &done).await,
        vec!["Billing".to_owned()]
    );
    let all = TopicListQuery {
        status: Some(TopicStatusFilter::All),
        sort: Some(TopicSort::Name),
        ..TopicListQuery::default()
    };
    assert_eq!(
        topics(&world, &t.dan, t.web, &all).await,
        vec!["Billing".to_owned(), "Login".to_owned()],
        "by name"
    );
    let created = TopicListQuery {
        status: Some(TopicStatusFilter::All),
        sort: Some(TopicSort::Created),
        ..TopicListQuery::default()
    };
    assert_eq!(
        topics(&world, &t.dan, t.web, &created).await,
        vec!["Billing".to_owned(), "Login".to_owned()],
        "the latest made first"
    );
    let found = TopicListQuery {
        status: Some(TopicStatusFilter::All),
        q: Some("LOG".to_owned()),
        ..TopicListQuery::default()
    };
    assert_eq!(
        topics(&world, &t.dan, t.web, &found).await,
        vec!["Login".to_owned()]
    );
}

#[tokio::test]
async fn topics_are_paged_in_every_order_without_losing_or_repeating_one() {
    let world = World::new().await;
    let t = tree(&world).await;
    for name in ["Alpha", "Delta", "Charlie", "Bravo", "Echo"] {
        make_topic(&world, &t.boss, t.web, name).await;
    }
    for sort in [TopicSort::Activity, TopicSort::Created, TopicSort::Name] {
        let mut seen: Vec<String> = Vec::new();
        let mut cursor = None;
        loop {
            let page = world
                .app
                .call(TopicList, &[&t.web])
                .token(&t.dan.token)
                .query(&TopicListQuery {
                    status: Some(TopicStatusFilter::All),
                    sort: Some(sort),
                    limit: Some(3),
                    cursor: cursor.clone(),
                    ..TopicListQuery::default()
                })
                .send()
                .await
                .ok();
            seen.extend(
                page.items
                    .iter()
                    .filter_map(orch_api_contract::common::Listed::as_visible)
                    .map(|topic| topic.base.name.clone()),
            );
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(seen.len(), 7, "{sort:?}: {seen:?}");
        assert_eq!(sorted.len(), 7, "{sort:?} repeats one: {seen:?}");
        if sort == TopicSort::Name {
            assert_eq!(seen[0], "Alpha");
            assert_eq!(seen[6], "Login");
        }
    }
}

#[tokio::test]
async fn topics_that_carry_every_tag_asked_for_are_listed_and_tags_come_from_the_project_or_above()
{
    let world = World::new().await;
    let t = tree(&world).await;
    let (urgent, mobile, elsewhere) = (new_id(), new_id(), new_id());
    {
        let mut conn = world.conn().await;
        let at = world.now();
        hierarchy::create_tag(&mut conn, urgent, t.acme, "urgent", 1, at)
            .await
            .unwrap();
        hierarchy::create_tag(&mut conn, mobile, t.web, "mobile", 2, at)
            .await
            .unwrap();
        let other = make_project(&world, &t.boss, t.acme, "Other").await;
        hierarchy::create_tag(&mut conn, elsewhere, other.base.id, "elsewhere", 3, at)
            .await
            .unwrap();
    }
    let set = |topic: Id, tags: Vec<Id>| {
        world
            .app
            .call(TopicUpdate, &[&topic])
            .token(&t.boss.token)
            .body(&TopicUpdateBody {
                status: None,
                tag_ids: Some(tags),
                expected_revision: None,
            })
            .send()
    };
    let both = set(t.topics[0], vec![urgent, mobile]).await.ok();
    assert_eq!(both.base.tags.len(), 2);
    assert_eq!(
        both.base.tags_source,
        orch_api_contract::nodes::FieldSource::User
    );
    assert_eq!(both.base.tags[0].node.name.as_deref(), Some("Acme"));
    set(t.topics[1], vec![urgent]).await.ok();
    // The binder cannot read a repeated parameter (`tag_id=a&tag_id=b`), so the
    // filter is looked at where it is made.
    let carrying = |tags: Vec<Id>| {
        let world = &world;
        let project = t.web;
        async move {
            let mut conn = world.conn().await;
            let mut found = hierarchy::topics_of(
                &mut conn,
                &hierarchy::TopicQuery {
                    project,
                    status: None,
                    tags: &tags,
                    attention: None,
                    with_folders: false,
                    text: None,
                    archived: hierarchy::ArchivedMode::Exclude,
                    order: hierarchy::TopicOrder::Name,
                    after: None,
                    limit: 10,
                },
            )
            .await
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
            found.sort();
            found
        }
    };
    let mut both_topics = vec![t.topics[0], t.topics[1]];
    both_topics.sort();
    assert_eq!(carrying(vec![urgent, mobile]).await, vec![t.topics[0]]);
    assert_eq!(carrying(vec![urgent]).await, both_topics);
    assert_eq!(carrying(vec![]).await, both_topics);
    // A tag of another project is none of this topic's.
    let refused = set(t.topics[0], vec![elsewhere]).await.problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert_eq!(refused.extra["errors"][0]["path"], "tag_ids[0]");
    // And no topic carries more than the setting allows (5 as shipped).
    let mut many = Vec::new();
    let mut conn = world.conn().await;
    for index in 0..6 {
        let id = new_id();
        hierarchy::create_tag(&mut conn, id, t.web, &format!("t{index}"), 0, world.now())
            .await
            .unwrap();
        many.push(id);
    }
    drop(conn);
    let refused = set(t.topics[0], many).await.problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert_eq!(refused.extra["errors"][0]["path"], "tag_ids");
}

#[tokio::test]
async fn a_topic_marked_done_and_reopened_leaves_a_line_each_time_and_nothing_when_it_was_so() {
    let world = World::new().await;
    let t = tree(&world).await;
    let change = |status| {
        world
            .app
            .call(TopicUpdate, &[&t.topics[0]])
            .token(&t.boss.token)
            .body(&TopicUpdateBody {
                status: Some(status),
                tag_ids: None,
                expected_revision: None,
            })
            .send()
    };
    let done = change(TopicStatus::Done).await.ok();
    assert_eq!(done.status, TopicStatus::Done);
    assert!(done.done_at.is_some());
    change(TopicStatus::Done).await.ok();
    let open = change(TopicStatus::Open).await.ok();
    assert_eq!(open.status, TopicStatus::Open);
    assert!(open.done_at.is_none());
    let lines = world
        .audit()
        .await
        .into_iter()
        .filter(|action| action == "topic.status_changed")
        .count();
    assert_eq!(lines, 2);
    // A viewer may not change it.
    let refused = world
        .app
        .call(TopicUpdate, &[&t.topics[0]])
        .token(&t.dan.token)
        .body(&TopicUpdateBody {
            status: Some(TopicStatus::Done),
            tag_ids: None,
            expected_revision: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.extra["required"], "topic.interact");
}

#[tokio::test]
async fn archived_topics_are_listed_only_when_asked() {
    let world = World::new().await;
    let t = tree(&world).await;
    world
        .app
        .call(orch_api_contract::nodes::NodeArchive, &[&t.topics[0]])
        .token(&t.boss.token)
        .body(&places::archive_body(&world, &t.boss.token, t.topics[0], true).await)
        .send()
        .await
        .ok();
    async fn listed(world: &World, who: &Who, project: Id, filter: ArchivedFilter) -> Vec<String> {
        let query = TopicListQuery {
            archived: Some(filter),
            ..TopicListQuery::default()
        };
        topics(world, who, project, &query).await
    }
    assert_eq!(
        listed(&world, &t.boss, t.web, ArchivedFilter::Exclude).await,
        vec!["Billing".to_owned()]
    );
    assert_eq!(
        listed(&world, &t.boss, t.web, ArchivedFilter::Only).await,
        vec!["Login".to_owned()]
    );
    assert_eq!(
        listed(&world, &t.boss, t.web, ArchivedFilter::Include)
            .await
            .len(),
        2
    );
}

#[tokio::test]
async fn a_list_of_topics_costs_the_same_statements_for_two_and_for_many() {
    let world = World::new().await;
    let t = tree(&world).await;
    async fn measure(world: &World, who: &Who, project: Id) -> (usize, usize) {
        let call = world
            .app
            .call(TopicList, &[&project])
            .token(&who.token)
            .query(&TopicListQuery {
                limit: Some(100),
                ..TopicListQuery::default()
            });
        let before = Statements::now();
        let (reply, evidence) = reader_diagnostics::trace("topic_list_cost", call.send()).await;
        let count = Statements::now() - before;
        eprintln!(
            "topic_list_cost {}",
            serde_json::json!({"thread_statements":count,"scoped":evidence})
        );
        (count, reply.ok().items.len())
    }
    async fn cold(world: &World, who: &Who, org: Id) {
        sqlx::query("DELETE FROM org_user_activity WHERE org_id=$1 AND user_id=$2")
            .bind(org)
            .bind(who.id())
            .execute(&world.db.pool)
            .await
            .unwrap();
    }
    // Include the first Last seen publication, then its coalesced repeat, at both sizes.
    // These fixture resets are outside the measured requests; no initial cost is discarded.
    cold(&world, &t.dan, t.acme).await;
    let (few, few_count) = measure(&world, &t.dan, t.web).await;
    let (few_warm, few_warm_count) = measure(&world, &t.dan, t.web).await;
    for index in 0..15 {
        make_topic(&world, &t.boss, t.web, &format!("T{index}")).await;
    }
    cold(&world, &t.dan, t.acme).await;
    let (many, many_count) = measure(&world, &t.dan, t.web).await;
    let (many_warm, many_warm_count) = measure(&world, &t.dan, t.web).await;
    assert_eq!((few_count, many_count), (2, 17));
    assert_eq!((few_warm_count, many_warm_count), (2, 17));
    assert_eq!(few, many, "{few} statements for 2 topics, {many} for 17");
    assert_eq!(
        few_warm, many_warm,
        "coalesced requests cost the same at both sizes"
    );
    assert_eq!(
        few,
        few_warm + 1,
        "the cold request publishes Last seen once"
    );
    assert_eq!(
        many,
        many_warm + 1,
        "more topics add no extra Last seen publication"
    );
}

#[tokio::test]
async fn what_somebody_was_named_on_is_listed_apart_when_its_parent_is_not_theirs_to_view() {
    let world = World::new().await;
    let t = tree(&world).await;
    let eve = person(&world, "eve").await;
    {
        let mut conn = world.conn().await;
        // Eve is named in person on the Login topic's View row, in a tree she
        // is no member of, and Dan (a member) is named there too.
        for who in [&eve, &t.dan] {
            access_rows::add_entry(
                &mut conn,
                &orch_db::queries::access::PageKey::new("node", t.topics[0], "live"),
                "any",
                "topic.view",
                ("user", Some(who.id()), None),
                None,
                world.now(),
            )
            .await
            .unwrap();
        }
    }
    let shared = world
        .app
        .call(SharedList, &[])
        .token(&eve.token)
        .send()
        .await
        .ok();
    assert_eq!(shared.items.len(), 1);
    assert!(!shared.partial);
    let SharedListItem::Node(node) = &shared.items[0] else {
        panic!("a hierarchy node was named");
    };
    let Node::Topic(topic) = node.as_ref() else {
        panic!("a topic was named");
    };
    assert_eq!(
        serde_json::to_value(&shared.items[0]).unwrap(),
        serde_json::to_value(node.as_ref()).unwrap(),
        "the additive shared union never wraps or changes the legacy Node JSON"
    );
    assert_eq!(topic.base.id, t.topics[0]);
    assert!(
        topic.base.path.iter().any(|each| each.redacted),
        "the nodes above are shown as references without a name"
    );
    let none = world
        .app
        .call(SharedList, &[])
        .token(&t.dan.token)
        .send()
        .await
        .ok();
    assert!(
        none.items.is_empty(),
        "a member sees the parent, so it is not shared"
    );
}

#[tokio::test]
async fn shared_places_keep_the_newest_and_page_past_the_old_five_hundred_cap() {
    let world = World::new().await;
    let t = tree(&world).await;
    let eve = person(&world, "eve").await;
    const COUNT: i64 = 501; // One beyond the old silent bound.
    {
        let mut conn = world.conn().await;
        sqlx::query("INSERT INTO nodes (id,kind,parent_id,org_id,path,name,owner_id,created_by,created_at,updated_at)
            SELECT id,'topic',$1,$2,ARRAY[$2,$1,id]::uuid[],'Share ' || number,$3,$3,
                   $5::timestamptz - interval '1 day' + number * interval '1 second',$5
            FROM (SELECT ('ffffffff-ffff-7ffe-8fff-' || lpad(to_hex(number),12,'0'))::uuid id,number
                  FROM generate_series(1,$4::bigint) number) made")
            .bind(t.web).bind(t.acme).bind(t.boss.id()).bind(COUNT).bind(world.now())
            .execute(&mut *conn).await.unwrap();
        sqlx::query("INSERT INTO topics (node_id) SELECT id FROM nodes WHERE parent_id=$1 AND name LIKE 'Share %'")
            .bind(t.web).execute(&mut *conn).await.unwrap();
        sqlx::query("INSERT INTO permission_entries (id,object_kind,object_id,page,profile,permission,holder_kind,holder_id,created_by,created_at)
            SELECT gen_random_uuid(),'node',id,'live','any','topic.view','user',$2,$3,$4
            FROM nodes WHERE parent_id=$1 AND name LIKE 'Share %'")
            .bind(t.web).bind(eve.id()).bind(t.boss.id()).bind(world.now())
            .execute(&mut *conn).await.unwrap();
    }
    let first = world
        .app
        .call(SharedList, &[])
        .token(&eve.token)
        .query(&PageQuery {
            limit: Some(200),
            cursor: None,
        })
        .send()
        .await
        .ok();
    assert!(first.partial);
    let SharedListItem::Node(node) = &first.items[0] else {
        panic!("a hierarchy node was shared");
    };
    let Node::Topic(newest) = node.as_ref() else {
        panic!("a topic was shared");
    };
    assert_eq!(newest.base.name, "Share 501");
    assert!(newest.base.path.iter().any(|step| step.redacted));
    let mut ids = std::collections::HashSet::new();
    let mut page = first;
    loop {
        for item in &page.items {
            let SharedListItem::Node(node) = item else {
                panic!("a hierarchy node was shared");
            };
            let Node::Topic(topic) = node.as_ref() else {
                panic!("a topic was shared");
            };
            assert!(ids.insert(topic.base.id));
        }
        assert_eq!(page.partial, page.next_cursor.is_some());
        let Some(cursor) = page.next_cursor else {
            break;
        };
        page = world
            .app
            .call(SharedList, &[])
            .token(&eve.token)
            .query(&PageQuery {
                limit: Some(200),
                cursor: Some(cursor),
            })
            .send()
            .await
            .ok();
    }
    assert_eq!(ids.len(), usize::try_from(COUNT).unwrap());
}

#[tokio::test]
async fn the_activity_of_a_place_is_refused_to_somebody_who_may_not_view_the_place() {
    let world = World::new().await;
    let t = tree(&world).await;
    let stranger = person(&world, "stranger").await;
    let refused = world
        .app
        .call(NodeActivityList, &[&t.acme])
        .token(&stranger.token)
        .query(&NodeActivityListQuery::default())
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "org.view");
    assert_eq!(refused.extra["missing"][0]["permissions"][0], "org.view");
}

#[tokio::test]
async fn activity_lists_the_places_history_newest_first_and_leaves_out_what_is_not_viewable() {
    let world = World::new().await;
    let t = tree(&world).await;
    let secret = world
        .app
        .call(ProjectCreate, &[&t.acme])
        .token(&t.boss.token)
        .body(&ProjectCreateBody {
            name: "Secret".to_owned(),
            description: None,
            access: Some(NewNodeAccess::Private),
        })
        .send()
        .await
        .ok();
    let activity = |who: &Who, place: Id, mut query: NodeActivityListQuery| {
        query.depth = Some(orch_api_contract::nodes::ActivityDepth::Every);
        world
            .app
            .call(NodeActivityList, &[&place])
            .token(&who.token)
            .query(&query)
            .send()
    };
    let boss = activity(&t.boss, t.acme, NodeActivityListQuery::default())
        .await
        .ok();
    let titles: Vec<&str> = boss.items.iter().map(|line| line.title.as_str()).collect();
    assert_eq!(
        titles.len(),
        6,
        "the org, General, Web and Secret, and the two lines of making Secret private: {titles:?}"
    );
    assert!(titles[0].contains("Secret"), "newest first: {titles:?}");
    assert_eq!(boss.items[0].action.as_deref(), Some("node.created"));
    assert_eq!(boss.items[0].kind, ActivityKind::History);
    let actor = match boss.items[0].actor.as_ref().unwrap() {
        orch_api_contract::access::HistoryActor::User(user) => user.username.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(actor, "boss");
    assert_eq!(boss.items[0].node.id, secret.base.id);
    // Dan sees every line but those about the project he may not view.
    let dan = activity(&t.dan, t.acme, NodeActivityListQuery::default())
        .await
        .ok();
    assert_eq!(dan.items.len(), 3);
    assert!(dan.items.iter().all(|line| line.node.id != secret.base.id));
    // Paging.
    let first = activity(
        &t.boss,
        t.acme,
        NodeActivityListQuery {
            limit: Some(4),
            ..NodeActivityListQuery::default()
        },
    )
    .await
    .ok();
    assert_eq!(first.items.len(), 4);
    let rest = activity(
        &t.boss,
        t.acme,
        NodeActivityListQuery {
            limit: Some(4),
            cursor: first.next_cursor,
            ..NodeActivityListQuery::default()
        },
    )
    .await
    .ok();
    assert_eq!(rest.items.len(), 2);
    // A topic's own Activity has its own lines and those of nothing else.
    let topic = activity(&t.dan, t.topics[0], NodeActivityListQuery::default())
        .await
        .ok();
    assert_eq!(topic.items.len(), 1);
}

#[tokio::test]
async fn the_audit_trail_of_a_place_includes_what_is_below_it_and_is_for_its_auditors() {
    let world = World::new().await;
    let t = tree(&world).await;
    let audit = |who: &Who, query: NodeAuditListQuery| {
        world
            .app
            .call(NodeAuditList, &[&t.acme])
            .token(&who.token)
            .query(&query)
            .send()
    };
    let refused = audit(&t.dan, NodeAuditListQuery::default()).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "audit.view");
    let all = audit(&t.boss, NodeAuditListQuery::default()).await.ok();
    assert_eq!(all.items.len(), 5, "the org, General, Web, and two topics");
    assert!(
        all.items
            .iter()
            .all(|row| row.action == "node.created" && row.history)
    );
    assert!(all.items[0].at >= all.items[4].at, "newest first");
    let first = &all.items[0];
    assert_eq!(first.node.as_ref().unwrap().id, t.topics[1]);
    assert_eq!(first.object.kind, "node");
    assert_eq!(first.object.id, Some(t.topics[1]));
    let by_boss = audit(
        &t.boss,
        NodeAuditListQuery {
            actor: Some(t.boss.id()),
            action: Some("node.created".to_owned()),
            limit: Some(2),
            ..NodeAuditListQuery::default()
        },
    )
    .await
    .ok();
    assert_eq!(by_boss.items.len(), 2);
    assert!(by_boss.next_cursor.is_some());
    let none = audit(
        &t.boss,
        NodeAuditListQuery {
            actor: Some(t.dan.id()),
            ..NodeAuditListQuery::default()
        },
    )
    .await
    .ok();
    assert!(none.items.is_empty());
}
