//! What a person tells the threads of a whole project reaches the topics of the folders inside it, and passes over a folder or a topic that was made private there.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::common::Page;
use orch_api_contract::nodes::{
    NewNodeAccess, ProjectChildCreate, ProjectCreateBody, TopicCreate, TopicCreateBody,
    TopicCreateReply,
};
use orch_api_contract::topics::{
    Announcement, AnnouncementCreateBody, NodeAnnouncementCreate, NodeAnnouncementList,
    NodeAnnouncementListQuery, PlaceAnnouncementCreateReply,
};
use orch_types::Id;
use places::Who;
use rig::Scene;

fn announcement(title: &str) -> AnnouncementCreateBody {
    AnnouncementCreateBody {
        title: title.to_owned(),
        body: "Details follow.".to_owned(),
        category: None,
        severity: None,
        targets: None,
        delivery: None,
        fallback: None,
        wake_idle: None,
    }
}

async fn folder(s: &Scene, by: &Who, project: Id, name: &str, access: Option<NewNodeAccess>) -> Id {
    s.rig
        .world
        .app
        .call(ProjectChildCreate, &[&project])
        .token(&by.token)
        .body(&ProjectCreateBody {
            name: name.to_owned(),
            description: None,
            access,
        })
        .send()
        .await
        .ok()
        .base
        .id
}

async fn topic(s: &Scene, by: &Who, project: Id, name: &str, access: Option<NewNodeAccess>) -> Id {
    let made = s
        .rig
        .world
        .app
        .call(TopicCreate, &[&project])
        .token(&by.token)
        .body(&TopicCreateBody {
            name: Some(name.to_owned()),
            access,
            ..TopicCreateBody::default()
        })
        .send()
        .await
        .ok();
    match made {
        TopicCreateReply::Topic(topic) => topic.base.id,
        TopicCreateReply::WithThread(both) => both.topic.base.id,
    }
}

async fn told(s: &Scene, by: &Who, project: Id) -> PlaceAnnouncementCreateReply {
    s.rig
        .world
        .app
        .call(NodeAnnouncementCreate, &[&project])
        .token(&by.token)
        .body(&announcement("To the project"))
        .send()
        .await
        .ok()
}

fn topics_of(reply: &PlaceAnnouncementCreateReply) -> Vec<Id> {
    let mut found: Vec<Id> = reply
        .announcements
        .iter()
        .map(|each| each.topic_id)
        .collect();
    found.sort();
    found
}

fn sorted(mut ids: Vec<Id>) -> Vec<Id> {
    ids.sort();
    ids
}

fn grouped(page: Page<Announcement>) -> Vec<(Id, i64)> {
    let mut counts = std::collections::HashMap::<Id, i64>::new();
    for row in page.items {
        *counts.entry(row.group_id.unwrap_or(row.id)).or_default() += 1;
    }
    counts.into_iter().collect()
}

#[tokio::test]
async fn an_announcement_to_a_project_reaches_the_topics_of_the_folders_inside_it() {
    let s = Scene::new().await;
    let project = s.tree.project.base.id;
    let own = s.tree.topic.base.id;
    let checkout = folder(&s, &s.ada, project, "Checkout", None).await;
    let payments = folder(&s, &s.ada, checkout, "Payments", None).await;
    let cart = topic(&s, &s.ada, checkout, "Cart", None).await;
    let refunds = topic(&s, &s.ada, payments, "Refunds", None).await;
    let beside = folder(&s, &s.ada, project, "Docs", None).await;
    let notes = topic(&s, &s.ada, beside, "Notes", None).await;
    sqlx::query("UPDATE topics SET status = 'done' WHERE node_id = $1")
        .bind(notes)
        .execute(s.rig.db.pool())
        .await
        .unwrap();

    let all = told(&s, &s.ada, project).await;
    assert_eq!(
        topics_of(&all),
        sorted(vec![own, cart, refunds]),
        "the topics of the project and of the folders in it that are open"
    );
    assert_eq!(all.topics_skipped, 1, "the finished one is left out");
    // To a folder: its own topics and the ones in the folders inside it, and not the project above.
    let some = told(&s, &s.ada, checkout).await;
    assert_eq!(topics_of(&some), sorted(vec![cart, refunds]));
    let one = told(&s, &s.ada, payments).await;
    assert_eq!(topics_of(&one), vec![refunds]);

    // What reached a project is listed with it, one line for the topics of that project and of the
    // folders in it; a folder lists what reached its own topics.
    let lines = |place: Id| {
        let s = &s;
        async move {
            s.rig
                .world
                .app
                .call(NodeAnnouncementList, &[&place])
                .token(&s.ada.token)
                .query(&NodeAnnouncementListQuery::default())
                .send()
                .await
                .ok()
        }
    };
    let at_project = grouped(lines(project).await);
    assert_eq!(
        sorted(at_project.iter().map(|(group, _)| *group).collect()),
        sorted(vec![all.group_id, some.group_id, one.group_id]),
        "every announcement that reached a topic in it"
    );
    assert!(at_project.contains(&(all.group_id, 3)));
    assert!(at_project.contains(&(some.group_id, 2)));
    let at_checkout = grouped(lines(checkout).await);
    assert!(
        at_checkout.contains(&(all.group_id, 2)),
        "the one for the project reached two topics of the folder"
    );
    assert!(at_checkout.contains(&(some.group_id, 2)) && at_checkout.contains(&(one.group_id, 1)));
}

#[tokio::test]
async fn an_announcement_to_a_project_passes_over_a_folder_and_a_topic_that_were_made_private() {
    let s = Scene::new().await;
    let project = s.tree.project.base.id;
    let own = s.tree.topic.base.id;
    let open = folder(&s, &s.ada, project, "Open", None).await;
    let seen = topic(&s, &s.ada, open, "Seen", None).await;
    let hidden = topic(&s, &s.ada, open, "Hidden", Some(NewNodeAccess::Private)).await;
    let private = folder(&s, &s.ada, project, "Private", Some(NewNodeAccess::Private)).await;
    let inside = topic(&s, &s.ada, private, "Inside", None).await;
    let deeper = folder(&s, &s.ada, private, "Deeper", None).await;
    let way_down = topic(&s, &s.ada, deeper, "Way down", None).await;
    let secret = topic(&s, &s.ada, project, "Secret", Some(NewNodeAccess::Private)).await;

    let all = told(&s, &s.ada, project).await;
    assert_eq!(
        topics_of(&all),
        sorted(vec![own, seen]),
        "what follows the project is told; what was made private is passed over, with all that lies in it"
    );
    assert_eq!(
        all.topics_skipped, 4,
        "the four that are left out are counted"
    );
    // The one who made a place private tells what is in it: it follows itself.
    let to_the_private = told(&s, &s.ada, private).await;
    assert_eq!(topics_of(&to_the_private), sorted(vec![inside, way_down]));
    let _ = (hidden, secret);
}
