//! Reader discussions tell current owners and block writers once, and the personal bell controls additional replies.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod rig;
mod support;

use chrono::Duration;
use desk::{Desk, rewrite, text};
use orch_api_contract::access::{
    HolderInput, PageChange, PageChangeOwners, PageChangeWhole, PermissionPageUpdate,
};
use orch_api_contract::documents::{
    DocumentCommentCreate, DocumentCommentCreateBody, DocumentCommentUpdate,
    DocumentCommentUpdateBody, DocumentDiscussion, DocumentDiscussionCreate,
    DocumentDiscussionCreateBody, DocumentDiscussionLevel, DocumentGet, DocumentSeen,
    DocumentSeenBody, DocumentSubscriptionSet, DocumentSubscriptionSetBody, DocumentSyncGet,
    DocumentSyncGetQuery, NodeDocumentCreate, NodeDocumentCreateBody,
};
use orch_api_contract::notifications::{NotificationList, NotificationListQuery};
use orch_db::queries::documents::batch::{self, Limits, Outcome, Rules};
use orch_db::{Db, sequences};
use orch_documents::versions::Writer;
use orch_realtime_contract::feeds::UserEvent;
use orch_types::{Id, new_id};
use places::Who;

async fn office() -> Desk {
    Desk::new().await
}

async fn bell(desk: &Desk, who: &Who, document: Id, level: DocumentDiscussionLevel) {
    let reply = desk
        .world
        .app
        .call(DocumentSubscriptionSet, &[&document])
        .token(&who.token)
        .body(&DocumentSubscriptionSetBody { level })
        .send()
        .await;
    assert_eq!(reply.status, 204, "{:?}", reply.body);
}

async fn start(desk: &Desk, who: &Who, document: Id, block: Id, client: Id) -> DocumentDiscussion {
    desk.world
        .app
        .call(DocumentDiscussionCreate, &[&document])
        .token(&who.token)
        .body(&DocumentDiscussionCreateBody {
            block_id: block,
            quote: Some("plan".into()),
            content: text("Why this plan?"),
            client_id: client.to_string(),
        })
        .send()
        .await
        .ok()
}

async fn answer(desk: &Desk, who: &Who, document: Id, discussion: Id, words: &str) {
    desk.world
        .app
        .call(DocumentCommentCreate, &[&document, &discussion])
        .token(&who.token)
        .body(&DocumentCommentCreateBody {
            content: text(words),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
}

async fn notices(world: &support::World, user: Id) -> Vec<(String, Id, i64)> {
    sqlx::query_as("SELECT body, (data->>'discussion_id')::uuid, inbox_seq FROM notifications WHERE user_id = $1 AND kind = 'doc.comment' ORDER BY inbox_seq")
        .bind(user).fetch_all(&world.db.pool).await.unwrap()
}

#[tokio::test]
async fn pw_01_a_reader_starts_a_discussion_and_current_owners_and_the_block_writer_hear_once() {
    let desk = office().await;
    let writer = desk.member("writer", "interactor").await;
    let reader = desk.member("reader", "viewer").await;
    let other = desk.member("other", "viewer").await;
    let doc = desk.make(&desk.ada, "Plan").await;
    let blocks = desk
        .paragraphs(&writer, doc.base.id, &["The plan is ready."])
        .await;
    let before = desk.sync(&reader, doc.base.id).await;
    assert!(
        before
            .document
            .access
            .permissions
            .contains(&"doc.comment".to_owned())
    );
    assert!(
        !before
            .document
            .access
            .permissions
            .contains(&"doc.edit".to_owned())
    );
    let client = new_id();
    let first = start(&desk, &reader, doc.base.id, blocks[0].id, client).await;
    assert_eq!(notices(&desk.world, desk.ada.id()).await.len(), 1);
    assert_eq!(notices(&desk.world, writer.id()).await.len(), 1);
    assert!(notices(&desk.world, reader.id()).await.is_empty());
    assert!(notices(&desk.world, other.id()).await.is_empty());
    let seq = notices(&desk.world, writer.id()).await[0].2;
    let repeated = start(&desk, &reader, doc.base.id, blocks[0].id, client).await;
    assert_eq!(first.id, repeated.id);
    assert_eq!(
        notices(&desk.world, writer.id()).await[0].2,
        seq,
        "a retry sends no new notice"
    );
    let after = desk.sync(&reader, doc.base.id).await;
    assert_eq!(before.blocks, after.blocks, "a comment edits no block");
    assert_eq!(before.document.base.version, after.document.base.version);
    assert_eq!(after.document.base.seq, before.document.base.seq + 1);
    desk.world
        .app
        .call(
            DocumentCommentUpdate,
            &[&doc.base.id, &first.comments[0].id],
        )
        .token(&reader.token)
        .body(&DocumentCommentUpdateBody {
            content: text("A clearer question"),
            client_id: None,
            expected_seq: None,
        })
        .send()
        .await
        .ok();
    desk.write(
        &writer,
        doc.base.id,
        vec![rewrite(&blocks[0], "The plan has more evidence.")],
    )
    .await
    .ok();
    desk.world
        .app
        .call(DocumentGet, &[&doc.base.id])
        .token(&reader.token)
        .send()
        .await
        .ok();
    assert_eq!(
        notices(&desk.world, writer.id()).await[0].2,
        seq,
        "editing or reading starts no discussion"
    );
}

#[tokio::test]
async fn pw_01_all_discussions_adds_readable_subscribers_and_involved_keeps_replies_to_participants()
 {
    let desk = office().await;
    let writer = desk.member("writer", "interactor").await;
    let reader = desk.member("reader", "viewer").await;
    let all = desk.member("allreader", "viewer").await;
    let involved = desk.member("involved", "viewer").await;
    let responder = desk.member("responder", "viewer").await;
    let doc = desk.make(&desk.ada, "Plan").await;
    let blocks = desk
        .paragraphs(&writer, doc.base.id, &["The plan is ready."])
        .await;
    bell(&desk, &all, doc.base.id, DocumentDiscussionLevel::All).await;
    bell(
        &desk,
        &involved,
        doc.base.id,
        DocumentDiscussionLevel::Involved,
    )
    .await;
    bell(&desk, &desk.ada, doc.base.id, DocumentDiscussionLevel::All).await;
    let opened = start(&desk, &reader, doc.base.id, blocks[0].id, new_id()).await;
    assert_eq!(notices(&desk.world, all.id()).await.len(), 1);
    assert_eq!(
        notices(&desk.world, desk.ada.id()).await.len(),
        1,
        "owner and All overlap only once"
    );
    assert!(notices(&desk.world, involved.id()).await.is_empty());
    bell(
        &desk,
        &desk.ada,
        doc.base.id,
        DocumentDiscussionLevel::Involved,
    )
    .await;
    answer(&desk, &responder, doc.base.id, opened.id, "First reply").await;
    assert_eq!(notices(&desk.world, reader.id()).await[0].0, "First reply");
    assert_eq!(notices(&desk.world, all.id()).await[0].0, "First reply");
    assert_eq!(
        notices(&desk.world, writer.id()).await[0].0,
        "Why this plan?",
        "a writer start notice never joins them to replies"
    );
    assert_eq!(
        notices(&desk.world, desk.ada.id()).await[0].0,
        "Why this plan?"
    );
    assert!(
        notices(&desk.world, responder.id()).await.is_empty(),
        "one's own action is suppressed"
    );
    sqlx::query("UPDATE notifications SET read_at = $2 WHERE user_id = $1")
        .bind(all.id())
        .bind(desk.world.now())
        .execute(&desk.world.db.pool)
        .await
        .unwrap();
    desk.world.advance(Duration::minutes(1));
    answer(&desk, &reader, doc.base.id, opened.id, "One more question").await;
    let renewed = notices(&desk.world, all.id()).await;
    assert_eq!(renewed.len(), 1);
    assert_eq!(renewed[0].0, "One more question");
    let read: bool = sqlx::query_scalar(
        "SELECT read_at IS NOT NULL FROM notifications WHERE user_id = $1 AND kind = 'doc.comment'",
    )
    .bind(all.id())
    .fetch_one(&desk.world.db.pool)
    .await
    .unwrap();
    assert!(!read, "the ordinary notice renews after it was read");
    assert_eq!(
        notices(&desk.world, responder.id()).await[0].0,
        "One more question"
    );
    bell(&desk, &all, doc.base.id, DocumentDiscussionLevel::Involved).await;
    answer(&desk, &responder, doc.base.id, opened.id, "Final reply").await;
    assert_eq!(
        notices(&desk.world, all.id()).await[0].0,
        "One more question"
    );
    assert_eq!(notices(&desk.world, reader.id()).await[0].0, "Final reply");
}

#[tokio::test]
async fn pw_01_the_bell_changes_only_its_personal_choice_and_repairs_through_the_user_feed() {
    let desk = office().await;
    let reader = desk.member("reader", "viewer").await;
    let doc = desk.make(&desk.ada, "Plan").await;
    desk.paragraphs(&desk.ada, doc.base.id, &["The plan is ready."])
        .await;
    let before = desk.sync(&reader, doc.base.id).await;
    desk.world
        .app
        .call(DocumentSeen, &[&doc.base.id])
        .token(&reader.token)
        .body(&DocumentSeenBody { seq: before.seq })
        .send()
        .await
        .ok();
    let original: (i64, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
        "SELECT seen_seq, updated_at FROM document_states WHERE user_id = $1 AND document_id = $2",
    )
    .bind(reader.id())
    .bind(doc.base.id)
    .fetch_one(&desk.world.db.pool)
    .await
    .unwrap();
    let db = Db::from_pool(desk.world.db.pool.clone());
    let mut listener = sequences::listen(&db, &[sequences::USER_CHANNEL])
        .await
        .unwrap();
    desk.world.advance(Duration::hours(1));
    bell(&desk, &reader, doc.base.id, DocumentDiscussionLevel::All).await;
    let notice = tokio::time::timeout(std::time::Duration::from_secs(30), listener.next())
        .await
        .unwrap()
        .unwrap();
    let sequences::Notice::Heard { channel, payload } = notice else {
        panic!("a document state event")
    };
    assert_eq!(channel, sequences::USER_CHANNEL);
    let Some(sequences::UserNotice::Event { users, event }) =
        sequences::parse_user_payload(&payload)
    else {
        panic!("a personal event")
    };
    assert_eq!(users, [reader.id()]);
    assert_eq!(
        serde_json::from_value::<UserEvent>(event).unwrap(),
        UserEvent::DocumentState {
            document_id: doc.base.id,
            discussion_level: DocumentDiscussionLevel::All
        }
    );
    let now = desk.sync(&reader, doc.base.id).await;
    assert_eq!(
        now.document.base.discussion_level,
        DocumentDiscussionLevel::All,
        "a reconnect uses the current document projection"
    );
    assert_eq!(now.blocks, before.blocks);
    assert_eq!(now.seq, before.seq);
    let after: (i64, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
        "SELECT seen_seq, updated_at FROM document_states WHERE user_id = $1 AND document_id = $2",
    )
    .bind(reader.id())
    .bind(doc.base.id)
    .fetch_one(&desk.world.db.pool)
    .await
    .unwrap();
    assert_eq!(original, after, "changing the bell records no new read");
    assert!(notices(&desk.world, desk.ada.id()).await.is_empty());
}

#[tokio::test]
async fn pw_01_a_lost_reader_gets_no_new_discussion_notice_and_no_retained_excerpt() {
    let desk = office().await;
    let writer = desk.member("writer", "interactor").await;
    let reader = desk.member("reader", "viewer").await;
    let all = desk.member("allreader", "viewer").await;
    let doc = desk.make(&desk.ada, "Plan").await;
    let blocks = desk
        .paragraphs(&writer, doc.base.id, &["The plan is ready."])
        .await;
    bell(&desk, &all, doc.base.id, DocumentDiscussionLevel::All).await;
    start(&desk, &reader, doc.base.id, blocks[0].id, new_id()).await;
    let last = notices(&desk.world, writer.id()).await[0].2;
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"documents", &doc.base.id])
        .token(&desk.ada.token)
        .body(&PageChange {
            whole: Some(PageChangeWhole::Private),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    start(&desk, &desk.ada, doc.base.id, blocks[0].id, new_id()).await;
    assert_eq!(notices(&desk.world, writer.id()).await[0].2, last);
    assert_eq!(notices(&desk.world, all.id()).await.len(), 1);
    let list = desk
        .world
        .app
        .call(NotificationList, &[])
        .token(&writer.token)
        .query(&NotificationListQuery::default())
        .send()
        .await
        .ok();
    assert!(
        list.items.iter().all(|n| !matches!(
            n.kind,
            orch_api_contract::notifications::NotificationKind::DocComment
                | orch_api_contract::notifications::NotificationKind::DocMention
        )),
        "losing current document View hides its retained words"
    );
    let refused = desk
        .world
        .app
        .call(DocumentSubscriptionSet, &[&doc.base.id])
        .token(&all.token)
        .body(&DocumentSubscriptionSetBody {
            level: DocumentDiscussionLevel::Involved,
        })
        .send()
        .await;
    assert_ne!(refused.status, 204);
}

#[tokio::test]
async fn pw_01_an_agent_block_writer_names_its_current_first_owner_without_an_agent_input() {
    let scene = rig::Scene::new().await;
    let next_owner = scene.rig.person("nextowner").await;
    let reader = scene.rig.person("reader").await;
    places::join(
        &scene.rig.world,
        scene.tree.org.base.id,
        scene.tree.org.base.id,
        &next_owner,
        "viewer",
        &scene.ada,
    )
    .await;
    places::join(
        &scene.rig.world,
        scene.tree.org.base.id,
        scene.tree.org.base.id,
        &reader,
        "viewer",
        &scene.ada,
    )
    .await;
    let thread = scene.thread().await;
    let doc = scene
        .rig
        .world
        .app
        .call(NodeDocumentCreate, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .body(&NodeDocumentCreateBody {
            title: "Findings".into(),
            blocks: Some(vec![desk::paragraph("The plan is ready.")]),
            markdown: None,
            access: None,
        })
        .send()
        .await
        .ok();
    let initial = scene
        .rig
        .world
        .app
        .call(DocumentSyncGet, &[&doc.base.id])
        .token(&scene.ada.token)
        .query(&DocumentSyncGetQuery::default())
        .send()
        .await
        .ok();
    let mut tx = scene.rig.world.app.app.db.begin().await.unwrap();
    // Use the ordinary batch writer so the block has the same stored author as any agent operation.
    let changed = batch::apply(
        &mut tx,
        doc.base.id,
        &Writer::Agent {
            thread: thread.base.id,
            agent: "Scripted agent".into(),
        },
        None,
        &[rewrite(&initial.blocks[0], "The plan has evidence.")],
        Limits {
            max_bytes: 2 * 1024 * 1024,
            max_blocks: 5000,
        },
        Rules {
            window_secs: 600,
            kept: 200,
            history_max_bytes: 16 * 1024 * 1024,
        },
        scene.rig.world.now(),
    )
    .await
    .unwrap();
    assert!(matches!(changed, Outcome::Applied(_)));
    tx.commit().await.unwrap();
    scene
        .rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &thread.base.id])
        .token(&scene.ada.token)
        .body(&PageChange {
            owners: Some(PageChangeOwners {
                add: None,
                remove: None,
                first: Some(next_owner.id()),
            }),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    let inputs: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id = $1")
        .bind(thread.base.id)
        .fetch_one(&scene.rig.world.db.pool)
        .await
        .unwrap();
    scene
        .rig
        .world
        .app
        .call(DocumentDiscussionCreate, &[&doc.base.id])
        .token(&reader.token)
        .body(&DocumentDiscussionCreateBody {
            block_id: initial.blocks[0].id,
            quote: None,
            content: text("Explain this evidence"),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    assert_eq!(notices(&scene.rig.world, next_owner.id()).await.len(), 1);
    assert_eq!(
        notices(&scene.rig.world, scene.ada.id()).await.len(),
        1,
        "the document owner remains a separate recipient"
    );
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id = $1")
        .bind(thread.base.id)
        .fetch_one(&scene.rig.world.db.pool)
        .await
        .unwrap();
    assert_eq!(after, inputs, "a writer notice never wakes an agent");
}

#[tokio::test]
async fn pw_01_every_member_of_an_owners_group_hears_a_new_discussion_even_beyond_a_roster_page() {
    // A group of 501 owners must be treated whole, independently of how a roster is paged.
    const GROUP_OWNERS: usize = 501;
    let desk = office().await;
    let reader = desk.member("reader", "viewer").await;
    let doc = desk.make(&desk.ada, "Plan").await;
    let blocks = desk
        .paragraphs(&desk.ada, doc.base.id, &["The plan is ready."])
        .await;
    let mut conn = desk.world.conn().await;
    let group = orch_db::queries::access::testing::insert_group(
        &mut conn,
        desk.org,
        "Authors",
        desk.ada.id(),
    )
    .await
    .unwrap();
    let mut owners = Vec::new();
    for ordinal in 0..GROUP_OWNERS {
        let user = new_id();
        orch_db::queries::identity::testing::insert_user(
            &mut conn,
            user,
            &format!("owner{ordinal:04}"),
        )
        .await
        .unwrap();
        orch_db::queries::access::testing::insert_member(&mut conn, desk.org, user, desk.org)
            .await
            .unwrap();
        orch_db::queries::access::testing::add_to_group(&mut conn, group, user)
            .await
            .unwrap();
        owners.push(user);
    }
    drop(conn);
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"documents", &doc.base.id])
        .token(&desk.ada.token)
        .body(&PageChange {
            owners: Some(PageChangeOwners {
                add: Some(vec![HolderInput::Group { id: group }]),
                remove: None,
                first: None,
            }),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    let opened = start(&desk, &reader, doc.base.id, blocks[0].id, new_id()).await;
    let notified: Vec<Id> = sqlx::query_scalar("SELECT user_id FROM notifications WHERE kind = 'doc.comment' AND (data->>'discussion_id')::uuid = $1")
        .bind(opened.id).fetch_all(&desk.world.db.pool).await.unwrap();
    assert_eq!(notified.len(), GROUP_OWNERS + 1);
    assert!(
        owners.iter().all(|owner| notified.contains(owner)),
        "every current group owner hears it"
    );
}
