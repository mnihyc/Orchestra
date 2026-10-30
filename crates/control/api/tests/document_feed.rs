//! The feed of a document over a real socket: the blocks of each accepted change in order, what a reader that missed some is sent, discussions, who has the document open and who is writing where, and the end of a feed when access or the document goes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod realtime_support;
mod support;

use desk::{paragraph, put, remove, rewrite};
use orch_api_contract::documents::{
    DocumentDelete, DocumentDiscussionCreate, DocumentDiscussionCreateBody, DocumentOp,
    DocumentOpsCreate, DocumentOpsCreateBody, DocumentOpsCreateReply, DocumentSyncGet,
    DocumentSyncGetQuery, DocumentUpdate, DocumentUpdateBody, NodeDocumentCreate,
    NodeDocumentCreateBody,
};
use orch_api_core::registry::Registry;
use orch_api_core::testing::TestApp;
use orch_config::ServerConfig;
use orch_testkit::clock;
use orch_types::{Id, new_id};
use realtime_support::{Client, Rig, Server, quick};
use serde_json::{Value, json};
use std::time::Duration;

/// The tree of a rig with the requests of the API in memory beside its socket.
struct Office {
    rig: Rig,
    app: TestApp,
}

impl Office {
    async fn new() -> Self {
        let rig = Rig::tuned(|tuning| tuning.sweep_every = Duration::from_secs(60)).await;
        let mut registry = Registry::new();
        orch_api_core::routes::mount(&mut registry);
        let app = TestApp::over(rig.server.app.clone(), clock::manual(), registry);
        Self { rig, app }
    }

    /// A document of Ada's beside the topic.
    async fn document(&self, title: &str) -> Id {
        self.app
            .call(NodeDocumentCreate, &[&self.rig.topic])
            .token(&self.rig.token)
            .body(&NodeDocumentCreateBody {
                title: title.to_owned(),
                blocks: None,
                markdown: None,
                access: None,
            })
            .send()
            .await
            .ok()
            .base
            .id
    }

    /// A batch written by the person whose token it is.
    async fn write(
        &self,
        token: &str,
        document: Id,
        ops: Vec<DocumentOp>,
    ) -> DocumentOpsCreateReply {
        self.app
            .call(DocumentOpsCreate, &[&document])
            .token(token)
            .body(&DocumentOpsCreateBody {
                expected_seq: None,
                client_id: new_id().to_string(),
                ops,
            })
            .send()
            .await
            .ok()
    }

    /// A person with a role on the topic, and their token.
    async fn member(&self, name: &str, role: &str) -> (Id, String, Id) {
        let (person, token) = self.rig.person(name).await;
        let given = self.rig.give(person, role).await;
        (person, token, given)
    }
}

/// Waits for the next frame of `op` among those that come.
async fn next_of(client: &mut Client, op: &str) -> Value {
    loop {
        let frame = client.frame().await;
        if frame["op"] == op {
            return frame;
        }
    }
}

/// Follows a document and takes the answer and the list of who is looking.
async fn follow(client: &mut Client, id: i64, document: Id, since: Option<i64>) -> Value {
    client.sub(id, &format!("doc:{document}"), since).await;
    let subscribed = client.expect("subscribed").await;
    if subscribed["mode"] != "reload" {
        client.expect("presence").await;
    }
    subscribed
}

// ---------------------------------------------------------------- the rows, in order

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reader_is_sent_the_blocks_of_each_accepted_change_in_order_and_a_removed_one_says_so() {
    let office = Office::new().await;
    let (_, reader_token, _) = office.member("vera", "viewer").await;
    let document = office.document("Plan").await;
    let mut reader = Client::signed_in(office.rig.server.addr, &reader_token).await;
    let subscribed = follow(&mut reader, 1, document, None).await;
    assert_eq!(subscribed["seq"], 0);
    assert_eq!(subscribed["mode"], "delta");

    let first = paragraph("one");
    let first_id = first.id;
    office
        .write(&office.rig.token, document, vec![put(first)])
        .await;
    let frame = next_of(&mut reader, "sync").await;
    assert_eq!(frame["feed"], format!("doc:{document}"));
    assert_eq!(
        (frame["prev"].as_i64(), frame["seq"].as_i64()),
        (Some(0), Some(1))
    );
    let blocks = frame["data"]["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["id"], first_id.to_string());
    assert_eq!(blocks[0]["content"][0]["text"], "one");
    assert_eq!(blocks[0]["seq_updated"], 1);
    assert_eq!(blocks[0]["updated_by"]["username"], "ada", "who wrote it");
    assert_eq!(
        frame["data"]["document"]["seq"], 1,
        "the document moved with it"
    );
    assert_eq!(frame["data"]["document"]["title"], "Plan");
    assert!(
        frame["data"].get("discussions").is_none(),
        "nothing that did not change"
    );

    // The next change is the next frame, and a removal arrives as a row that says so.
    let current = office
        .app
        .call(DocumentSyncGet, &[&document])
        .token(&office.rig.token)
        .query(&DocumentSyncGetQuery::default())
        .send()
        .await
        .ok();
    let block = &current.blocks[0];
    office
        .write(
            &office.rig.token,
            document,
            vec![rewrite(block, "one!"), put(paragraph("two"))],
        )
        .await;
    let frame = next_of(&mut reader, "sync").await;
    assert_eq!(
        (frame["prev"].as_i64(), frame["seq"].as_i64()),
        (Some(1), Some(2))
    );
    assert_eq!(frame["data"]["blocks"].as_array().unwrap().len(), 2);
    office
        .write(&office.rig.token, document, vec![remove(block)])
        .await;
    let frame = next_of(&mut reader, "sync").await;
    assert_eq!(
        (frame["prev"].as_i64(), frame["seq"].as_i64()),
        (Some(2), Some(3))
    );
    let gone = &frame["data"]["blocks"][0];
    assert_eq!(gone["id"], first_id.to_string());
    assert_eq!(gone["removed"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_batch_that_was_refused_sends_nothing_to_anybody() {
    let office = Office::new().await;
    let (_, reader_token, _) = office.member("vera", "viewer").await;
    let (_, editor_token, _) = office.member("ines", "interactor").await;
    let document = office.document("Plan").await;
    let mut reader = Client::signed_in(office.rig.server.addr, &reader_token).await;
    follow(&mut reader, 1, document, None).await;
    office
        .write(&office.rig.token, document, vec![put(paragraph("kept"))])
        .await;
    next_of(&mut reader, "sync").await;
    // A conflicting batch changes nothing, and so announces nothing.
    let current = office
        .app
        .call(DocumentSyncGet, &[&document])
        .token(&editor_token)
        .query(&DocumentSyncGetQuery::default())
        .send()
        .await
        .ok();
    let stale = current.blocks[0].clone();
    office
        .write(
            &office.rig.token,
            document,
            vec![rewrite(&stale, "ada was first")],
        )
        .await;
    next_of(&mut reader, "sync").await;
    let refused = office
        .app
        .call(DocumentOpsCreate, &[&document])
        .token(&editor_token)
        .body(&DocumentOpsCreateBody {
            expected_seq: None,
            client_id: new_id().to_string(),
            ops: vec![put(paragraph("never")), rewrite(&stale, "too late")],
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.status, 409);
    assert!(reader.is_quiet_for(Duration::from_millis(500)).await);
}

// ---------------------------------------------------------------- catching up

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reader_that_missed_changes_is_sent_what_came_after_the_position_it_names() {
    let office = Office::new().await;
    let document = office.document("Plan").await;
    office
        .write(&office.rig.token, document, vec![put(paragraph("one"))])
        .await;
    let loaded = office
        .app
        .call(DocumentSyncGet, &[&document])
        .token(&office.rig.token)
        .query(&DocumentSyncGetQuery::default())
        .send()
        .await
        .ok();
    // It loaded the document, and two changes were made before it followed.
    office
        .write(&office.rig.token, document, vec![put(paragraph("two"))])
        .await;
    office
        .write(&office.rig.token, document, vec![put(paragraph("three"))])
        .await;
    let mut reader = Client::signed_in(office.rig.server.addr, &office.rig.token).await;
    let subscribed = follow(&mut reader, 1, document, Some(loaded.seq)).await;
    assert_eq!(subscribed["mode"], "delta");
    assert_eq!(
        subscribed["seq"], loaded.seq,
        "the stream begins where it said"
    );
    let frame = next_of(&mut reader, "sync").await;
    assert_eq!(frame["prev"], loaded.seq);
    assert_eq!(frame["seq"], loaded.seq + 2);
    assert_eq!(frame["data"]["blocks"].as_array().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reader_that_would_continue_from_below_what_was_purged_or_from_the_future_is_told_to_load_again()
 {
    let office = Office::new().await;
    let document = office.document("Plan").await;
    let reply = office
        .write(
            &office.rig.token,
            document,
            vec![put(paragraph("gone soon"))],
        )
        .await;
    office
        .write(
            &office.rig.token,
            document,
            vec![remove(&reply.blocks[0]), put(paragraph("stays"))],
        )
        .await;
    {
        let mut conn = office.rig.pool.acquire().await.unwrap();
        orch_db::queries::documents::purge_removed_blocks(
            &mut conn,
            chrono::Utc::now() + chrono::Duration::days(1),
        )
        .await
        .unwrap();
    }
    let mut reader = Client::signed_in(office.rig.server.addr, &office.rig.token).await;
    reader.sub(1, &format!("doc:{document}"), Some(0)).await;
    let answer = reader.expect("subscribed").await;
    assert_eq!(
        answer["mode"], "reload",
        "the removal it would have to be told of is gone"
    );
    reader.sub(2, &format!("doc:{document}"), Some(500)).await;
    let ahead = reader.expect("subscribed").await;
    assert_eq!(
        ahead["mode"], "reload",
        "the database was restored from an older copy"
    );
    // Following from where the document stands is a stream.
    let fresh = follow(&mut reader, 3, document, None).await;
    assert_eq!(fresh["mode"], "delta");
}

// ---------------------------------------------------------------- discussions

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_discussion_arrives_whole_with_the_document_whenever_anything_in_it_changed() {
    let office = Office::new().await;
    let (_, reader_token, _) = office.member("vera", "viewer").await;
    let document = office.document("Plan").await;
    let reply = office
        .write(
            &office.rig.token,
            document,
            vec![put(paragraph("A claim."))],
        )
        .await;
    let mut reader = Client::signed_in(office.rig.server.addr, &reader_token).await;
    follow(&mut reader, 1, document, None).await;
    let opened = office
        .app
        .call(DocumentDiscussionCreate, &[&document])
        .token(&reader_token)
        .body(&DocumentDiscussionCreateBody {
            block_id: reply.blocks[0].id,
            quote: None,
            content: desk::text("Evidence?"),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    let frame = next_of(&mut reader, "sync").await;
    assert!(
        frame["data"].get("blocks").is_none(),
        "the text did not change"
    );
    let discussions = frame["data"]["discussions"].as_array().unwrap();
    assert_eq!(discussions.len(), 1);
    assert_eq!(discussions[0]["id"], opened.id.to_string());
    assert_eq!(discussions[0]["comments"][0]["author"]["username"], "vera");
    assert_eq!(frame["data"]["document"]["open_discussions"], 1);
    // An answer sends the discussion again, with all its comments.
    office
        .app
        .call(
            orch_api_contract::documents::DocumentCommentCreate,
            &[&document, &opened.id],
        )
        .token(&office.rig.token)
        .body(&orch_api_contract::documents::DocumentCommentCreateBody {
            content: desk::text("Here."),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    let frame = next_of(&mut reader, "sync").await;
    assert_eq!(
        frame["data"]["discussions"][0]["comments"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_description_written_under_the_title_reaches_every_open_copy_with_the_line_lists_show() {
    let office = Office::new().await;
    let (_, reader_token, _) = office.member("vera", "viewer").await;
    let document = office.document("Plan").await;
    let mut reader = Client::signed_in(office.rig.server.addr, &reader_token).await;
    follow(&mut reader, 1, document, None).await;
    office
        .app
        .call(DocumentUpdate, &[&document])
        .token(&office.rig.token)
        .body(&DocumentUpdateBody {
            title: None,
            description: Some("What ships when\nand who decides".to_owned()),
        })
        .send()
        .await
        .ok();
    let frame = next_of(&mut reader, "sync").await;
    assert_eq!(
        (frame["prev"].as_i64(), frame["seq"].as_i64()),
        (Some(0), Some(1))
    );
    assert!(
        frame["data"].get("blocks").is_none(),
        "the text did not change"
    );
    let shown = &frame["data"]["document"];
    assert_eq!(shown["description"], "What ships when\nand who decides");
    assert_eq!(shown["description_source"], "user");
    assert_eq!(shown["description_line"], "What ships when");
}

// ---------------------------------------------------------------- who is here and who is writing

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn who_has_a_document_open_is_told_to_everybody_who_follows_it() {
    let office = Office::new().await;
    let (_, vera_token, _) = office.member("vera", "viewer").await;
    let document = office.document("Plan").await;
    let feed = format!("doc:{document}");
    let mut ada = Client::signed_in(office.rig.server.addr, &office.rig.token).await;
    let mut vera = Client::signed_in(office.rig.server.addr, &vera_token).await;
    follow(&mut ada, 1, document, None).await;
    follow(&mut vera, 1, document, None).await;
    ada.send(json!({ "op": "visible", "feed": feed, "visible": true }))
        .await;
    for client in [&mut ada, &mut vera] {
        let frame = next_of(client, "presence").await;
        let users = frame["users"].as_array().unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0]["user"]["username"], "ada");
    }
    ada.send(json!({ "op": "visible", "feed": feed, "visible": false }))
        .await;
    let frame = next_of(&mut vera, "presence").await;
    assert!(frame["users"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn who_is_writing_in_which_block_is_told_to_the_others_and_never_stored() {
    let office = Office::new().await;
    let (_, vera_token, _) = office.member("vera", "viewer").await;
    let (_, ines_token, _) = office.member("ines", "interactor").await;
    let document = office.document("Plan").await;
    let blocks = office
        .write(&office.rig.token, document, vec![put(paragraph("A line."))])
        .await
        .blocks;
    let block = blocks[0].id;
    let feed = format!("doc:{document}");
    let mut vera = Client::signed_in(office.rig.server.addr, &vera_token).await;
    let mut ines = Client::signed_in(office.rig.server.addr, &ines_token).await;
    follow(&mut vera, 1, document, None).await;
    follow(&mut ines, 1, document, None).await;

    ines.send(json!({ "op": "typing", "feed": feed, "at": block }))
        .await;
    let told = next_of(&mut vera, "event").await;
    assert_eq!(told["feed"], feed);
    assert_eq!(told["data"]["what"], "typing");
    assert_eq!(told["data"]["user"]["username"], "ines");
    assert_eq!(told["data"]["at"], block.to_string());
    assert!(
        ines.is_quiet_for(Duration::from_millis(400)).await,
        "never shown to the person who typed"
    );
    // At most one notice in three seconds comes through from one connection.
    ines.send(json!({ "op": "typing", "feed": feed, "at": block }))
        .await;
    ines.send(json!({ "op": "typing", "feed": feed })).await;
    assert!(
        vera.is_quiet_for(Duration::from_millis(600)).await,
        "the ones that came too soon were dropped"
    );
    // A viewer cannot write, so is not shown as writing.
    vera.send(json!({ "op": "typing", "feed": feed, "at": block }))
        .await;
    assert!(ines.is_quiet_for(Duration::from_millis(600)).await);
    // And nothing of it was stored: the document did not move.
    let after = office
        .app
        .call(DocumentSyncGet, &[&document])
        .token(&office.rig.token)
        .query(&DocumentSyncGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(after.seq, 1);
}

// ---------------------------------------------------------------- ends of a feed

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nobody_follows_a_document_they_may_not_read_or_one_that_is_not_there() {
    let office = Office::new().await;
    let (_, stranger_token) = office.rig.person("sam").await;
    let document = office.document("Plan").await;
    let mut stranger = Client::signed_in(office.rig.server.addr, &stranger_token).await;
    stranger.sub(1, &format!("doc:{document}"), None).await;
    let refused = stranger.expect("error").await;
    assert_eq!(refused["code"], "forbidden");
    stranger.sub(2, &format!("doc:{}", new_id()), None).await;
    let missing = stranger.expect("error").await;
    assert_eq!(missing["code"], "not_found");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reader_who_loses_the_right_to_read_is_told_the_feed_is_closed_before_anything_more_is_sent()
 {
    let office = Office::new().await;
    let (_, vera_token, role) = office.member("vera", "viewer").await;
    let document = office.document("Plan").await;
    let mut vera = Client::signed_in(office.rig.server.addr, &vera_token).await;
    follow(&mut vera, 1, document, None).await;
    office.rig.take(role).await;
    let closed = next_of(&mut vera, "closed").await;
    assert_eq!(closed["reason"], "revoked");
    office
        .write(&office.rig.token, document, vec![put(paragraph("secret"))])
        .await;
    assert!(vera.is_quiet_for(Duration::from_millis(500)).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_feed_ends_when_the_document_is_deleted() {
    let office = Office::new().await;
    let (_, vera_token, _) = office.member("vera", "viewer").await;
    let document = office.document("Plan").await;
    let mut vera = Client::signed_in(office.rig.server.addr, &vera_token).await;
    follow(&mut vera, 1, document, None).await;
    let deleted = office
        .app
        .call(DocumentDelete, &[&document])
        .token(&office.rig.token)
        .send()
        .await;
    assert_eq!(deleted.status, 204);
    let closed = next_of(&mut vera, "closed").await;
    assert_eq!(closed["reason"], "deleted");
}

// ---------------------------------------------------------------- more than one process

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reader_on_another_process_is_sent_the_same_rows_and_is_shown_who_is_writing() {
    let office = Office::new().await;
    let (_, vera_token, _) = office.member("vera", "viewer").await;
    let (_, ines_token, _) = office.member("ines", "interactor").await;
    let document = office.document("Plan").await;
    let config = ServerConfig::default();
    let mut slow = quick(&config);
    slow.sweep_every = Duration::ZERO;
    let second = Server::over(&office.rig.pool, &office.rig.clock, slow).await;
    let feed = format!("doc:{document}");
    let mut vera = Client::signed_in(second.addr, &vera_token).await;
    let mut ines = Client::signed_in(office.rig.server.addr, &ines_token).await;
    follow(&mut vera, 1, document, None).await;
    follow(&mut ines, 1, document, None).await;

    let reply = office
        .write(
            &ines_token,
            document,
            vec![put(paragraph("From the first process"))],
        )
        .await;
    let frame = next_of(&mut vera, "sync").await;
    assert_eq!(
        frame["data"]["blocks"][0]["id"],
        reply.blocks[0].id.to_string()
    );
    ines.send(json!({ "op": "typing", "feed": feed, "at": reply.blocks[0].id }))
        .await;
    let told = next_of(&mut vera, "event").await;
    assert_eq!(told["data"]["user"]["username"], "ines");
    // Both processes tell the same.
    let mut here = Client::signed_in(office.rig.server.addr, &vera_token).await;
    follow(&mut here, 2, document, None).await;
    ines.send(json!({ "op": "typing", "feed": feed })).await;
    // The second one is within three seconds of the first and is dropped; wait it out.
    tokio::time::sleep(Duration::from_millis(3_100)).await;
    ines.send(json!({ "op": "typing", "feed": feed })).await;
    let told = next_of(&mut here, "event").await;
    assert_eq!(told["data"]["what"], "typing");
    second.stop().await;
}
