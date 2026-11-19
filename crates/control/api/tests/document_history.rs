//! The history of a document over HTTP: versions cut by the clock after the window and after the idle time, one cut before a version is put back, what differs between two, who wrote in each, and how much is kept.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod rig;
mod support;

#[path = "desk/file_history.rs"]
mod file_history;

use chrono::Duration;
use desk::{Desk, paragraph, put, remove, rewrite, said};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    Author, DocumentChangesGet, DocumentChangesGetQuery, DocumentSeen, DocumentSeenBody,
    DocumentVersion, DocumentVersionGet, DocumentVersionList, DocumentVersionListQuery,
    DocumentVersionRestore, VersionCause,
};
use orch_db::queries::documents::history;
use orch_settings::Target;
use orch_types::Id;
use places::Who;
use serde_json::json;

/// The version window and the idle time the deployment ships.
const WINDOW_SECS: i64 = 600;
const IDLE_SECS: i64 = 120;

async fn versions(desk: &Desk, by: &Who, document: Id) -> Vec<DocumentVersion> {
    desk.world
        .app
        .call(DocumentVersionList, &[&document])
        .token(&by.token)
        .query(&DocumentVersionListQuery::default())
        .send()
        .await
        .ok()
        .items
}

fn causes(list: &[DocumentVersion]) -> Vec<Option<VersionCause>> {
    list.iter().map(|each| each.cause).collect()
}

fn username(author: &Author) -> &str {
    match author {
        Author::User(user) => &user.username,
        Author::Agent(agent) => &agent.agent,
    }
}

// ---------------------------------------------------------------- when a version is cut

#[tokio::test]
async fn nothing_is_cut_while_a_document_is_written_inside_the_window_and_one_is_cut_after_it() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let blocks = desk.paragraphs(&desk.ada, made.base.id, &["one"]).await;
    // Ten minutes exactly is inside the window.
    desk.world.advance(Duration::seconds(WINDOW_SECS));
    desk.write(&desk.ada, made.base.id, vec![rewrite(&blocks[0], "two")])
        .await
        .ok();
    let list = versions(&desk, &desk.ada, made.base.id).await;
    assert_eq!(
        causes(&list),
        [None],
        "only what was changed since: the open span"
    );
    assert_eq!(list[0].version, None);
    assert_eq!(list[0].changed.inserted, 1);
    assert_eq!(list[0].changed.updated, 1);
    assert_eq!(list[0].authors.len(), 1);
    // A second more, and the next change cuts what stood before it.
    desk.world.advance(Duration::seconds(WINDOW_SECS + 1));
    let page = desk.page(&desk.ada, made.base.id).await;
    desk.write(&desk.ada, made.base.id, vec![rewrite(&page[0], "three")])
        .await
        .ok();
    let list = versions(&desk, &desk.ada, made.base.id).await;
    assert_eq!(causes(&list), [None, Some(VersionCause::Window)]);
    assert_eq!(list[1].version, Some(1));
    assert_eq!(list[1].block_count, 1);
    assert_eq!(list[1].title, "Handbook");
    assert_eq!(list[1].changed.inserted, 1, "what was changed to make it");
    assert_eq!(list[1].changed.updated, 1);
    assert_eq!(
        list[0].changed.updated, 1,
        "and the open span holds only what came after"
    );
    assert_eq!(list[0].changed.inserted, 0);
    let held = desk
        .world
        .app
        .call(DocumentVersionGet, &[&made.base.id, &1])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(held.blocks.iter().map(said).collect::<Vec<_>>(), ["two"]);
    assert_eq!(held.version.version, Some(1));
}

#[tokio::test]
async fn a_document_left_alone_for_the_idle_time_has_a_version_cut_and_one_that_nobody_changed_has_none()
 {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let untouched = desk.make(&desk.ada, "Untouched").await;
    desk.paragraphs(&desk.ada, made.base.id, &["draft"]).await;
    let idle =
        i64::from(u32::try_from(desk.world.app.app.config.documents.version_idle_secs).unwrap());
    assert_eq!(idle, IDLE_SECS);
    let sweep = |desk: &Desk| {
        let pool = desk.world.db.pool.clone();
        let now = desk.world.now();
        async move {
            let mut conn = pool.acquire().await.unwrap();
            let mut cut = Vec::new();
            for document in history::left_alone(&mut conn, now, IDLE_SECS, 100)
                .await
                .unwrap()
            {
                if let Some(number) =
                    history::cut_if_left_alone(&mut conn, document, now, IDLE_SECS)
                        .await
                        .unwrap()
                {
                    cut.push((document, number));
                }
            }
            cut
        }
    };
    desk.world.advance(Duration::seconds(IDLE_SECS - 1));
    assert!(sweep(&desk).await.is_empty(), "not yet");
    desk.world.advance(Duration::seconds(1));
    let cut = sweep(&desk).await;
    assert_eq!(
        cut,
        [(made.base.id, 1)],
        "after the idle time, and only the one that was changed"
    );
    assert!(sweep(&desk).await.is_empty(), "and once");
    let list = versions(&desk, &desk.ada, made.base.id).await;
    assert_eq!(
        causes(&list),
        [Some(VersionCause::Idle)],
        "nothing is left open"
    );
    assert!(
        versions(&desk, &desk.ada, untouched.base.id)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn a_batch_that_replaces_the_whole_document_keeps_what_was_there_as_a_version_first() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let blocks = desk
        .paragraphs(&desk.ada, made.base.id, &["old a", "old b"])
        .await;
    desk.write(
        &desk.ada,
        made.base.id,
        vec![
            remove(&blocks[0]),
            remove(&blocks[1]),
            put(paragraph("brand new")),
        ],
    )
    .await
    .ok();
    let list = versions(&desk, &desk.ada, made.base.id).await;
    assert_eq!(causes(&list), [None, Some(VersionCause::Rewrite)]);
    let kept = desk
        .world
        .app
        .call(DocumentVersionGet, &[&made.base.id, &1])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    let mut words: Vec<String> = kept.blocks.iter().map(said).collect();
    words.sort();
    assert_eq!(words, ["old a", "old b"]);
}

// ---------------------------------------------------------------- putting one back

#[tokio::test]
async fn putting_a_version_back_cuts_the_present_state_first_so_that_nothing_is_lost() {
    let desk = Desk::new().await;
    let dev = desk.member("dev", "developer").await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let blocks = desk
        .paragraphs(&desk.ada, made.base.id, &["one", "more"])
        .await;
    desk.world.advance(Duration::seconds(WINDOW_SECS + 1));
    desk.write(
        &desk.ada,
        made.base.id,
        vec![rewrite(&blocks[0], "two"), remove(&blocks[1])],
    )
    .await
    .ok();
    desk.world.advance(Duration::minutes(1));
    // Version 1 holds "one" and "more". The document now says "two".
    assert_eq!(desk.words(&desk.ada, made.base.id).await, ["two"]);
    let synced = desk
        .world
        .app
        .call(DocumentVersionRestore, &[&made.base.id, &1])
        .token(&dev.token)
        .send()
        .await
        .ok();
    let mut words: Vec<String> = synced
        .blocks
        .iter()
        .filter(|b| !b.removed)
        .map(said)
        .collect();
    words.sort();
    assert_eq!(
        words,
        ["more", "one"],
        "the blocks of that version stand again"
    );
    let list = versions(&desk, &desk.ada, made.base.id).await;
    assert_eq!(
        causes(&list),
        [
            None,
            Some(VersionCause::Restore),
            Some(VersionCause::Window)
        ]
    );
    // The present state before it was put back is version 2, whole.
    let kept = desk
        .world
        .app
        .call(DocumentVersionGet, &[&made.base.id, &2])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(kept.blocks.iter().map(said).collect::<Vec<_>>(), ["two"]);
    assert_eq!(kept.version.cause, Some(VersionCause::Restore));
    assert!(
        desk.world
            .audit()
            .await
            .contains(&"document.version_restored".to_owned()),
        "it is written to the document's history"
    );
    // Put back, it can be put back again to where it was: nothing is ever lost.
    desk.world
        .app
        .call(DocumentVersionRestore, &[&made.base.id, &2])
        .token(&dev.token)
        .send()
        .await
        .ok();
    assert_eq!(desk.words(&desk.ada, made.base.id).await, ["two"]);
}

#[tokio::test]
async fn a_version_is_put_back_by_whoever_manages_the_document_and_a_version_that_is_not_there_is_not_found()
 {
    let desk = Desk::new().await;
    let ines = desk.member("ines", "interactor").await;
    let vera = desk.member("vera", "viewer").await;
    let made = desk.make(&desk.ada, "Handbook").await;
    desk.paragraphs(&desk.ada, made.base.id, &["one"]).await;
    desk.world.advance(Duration::seconds(WINDOW_SECS + 1));
    let blocks = desk.page(&desk.ada, made.base.id).await;
    desk.write(&desk.ada, made.base.id, vec![rewrite(&blocks[0], "two")])
        .await
        .ok();
    for who in [&ines, &vera] {
        let refused = desk
            .world
            .app
            .call(DocumentVersionRestore, &[&made.base.id, &1])
            .token(&who.token)
            .send()
            .await
            .problem();
        assert_eq!(
            refused.code,
            ErrorCode::AuthzDenied,
            "{}",
            who.person.username
        );
        assert_eq!(refused.extra["required"], "doc.manage");
    }
    assert_eq!(desk.words(&desk.ada, made.base.id).await, ["two"]);
    // But the history is read by whoever reads the document.
    assert_eq!(versions(&desk, &vera, made.base.id).await.len(), 2);
    let missing = desk
        .world
        .app
        .call(DocumentVersionRestore, &[&made.base.id, &9])
        .token(&desk.ada.token)
        .send()
        .await
        .problem();
    assert_eq!(missing.code, ErrorCode::NotFound);
    let missing = desk
        .world
        .app
        .call(DocumentVersionGet, &[&made.base.id, &9])
        .token(&vera.token)
        .send()
        .await
        .problem();
    assert_eq!(missing.code, ErrorCode::NotFound);
}

// ---------------------------------------------------------------- what changed, and by whom

#[tokio::test]
async fn what_differs_between_two_states_is_given_block_by_block_with_who_changed_each_last() {
    let desk = Desk::new().await;
    let ines = desk.member("ines", "interactor").await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let blocks = desk
        .paragraphs(&desk.ada, made.base.id, &["alpha", "beta"])
        .await;
    desk.world.advance(Duration::seconds(WINDOW_SECS + 1));
    desk.write(&ines, made.base.id, vec![rewrite(&blocks[0], "alpha!")])
        .await
        .ok();
    desk.world.advance(Duration::seconds(WINDOW_SECS + 1));
    let page = desk.page(&desk.ada, made.base.id).await;
    let gamma = paragraph("gamma");
    let gamma_id = gamma.id;
    desk.write(&desk.ada, made.base.id, vec![remove(&page[1]), put(gamma)])
        .await
        .ok();
    // Versions: 1 = [alpha, beta], 2 = [alpha!, beta]; now [gamma, alpha!].
    let changes = |from: &str, to: Option<i64>| {
        let call = desk.world.app.call(DocumentChangesGet, &[&made.base.id]);
        let call = call.token(&desk.ada.token).query(&DocumentChangesGetQuery {
            inspect_deleted: None,
            from: from.to_owned(),
            to,
        });
        async move { call.send().await.ok() }
    };
    // What the second entry of the history changed.
    let second = changes("1", Some(2)).await;
    assert_eq!((second.from, second.to), (1, Some(2)));
    assert_eq!(second.blocks.len(), 1);
    let one = &second.blocks[0];
    assert_eq!(said(one.before.as_ref().unwrap()), "alpha");
    assert_eq!(said(one.after.as_ref().unwrap()), "alpha!");
    assert_eq!(username(one.by.as_ref().unwrap()), "ines");
    assert_eq!(
        second.authors.iter().map(username).collect::<Vec<_>>(),
        ["ines"]
    );
    // What the open span changed: a block inserted, a block removed.
    let open = changes("2", None).await;
    assert_eq!((open.from, open.to), (2, None));
    let inserted = open.blocks.iter().find(|each| each.id == gamma_id).unwrap();
    assert!(inserted.before.is_none() && inserted.after.is_some());
    let removed = open
        .blocks
        .iter()
        .find(|each| each.id == page[1].id)
        .unwrap();
    assert!(removed.before.is_some() && removed.after.is_none());
    assert_eq!(
        username(removed.by.as_ref().unwrap()),
        "ada",
        "who took it out"
    );
    assert_eq!(
        open.authors.iter().map(username).collect::<Vec<_>>(),
        ["ada"]
    );
    // From the empty document, everything in it was inserted.
    let all = changes("0", None).await;
    assert!(all.blocks.iter().all(|each| each.before.is_none()));
    assert_eq!(all.blocks.len(), 2);
    // Two states that are the same differ in nothing.
    assert!(changes("2", Some(2)).await.blocks.is_empty());
    // A title that changed is told.
    desk.world
        .app
        .call(
            orch_api_contract::documents::DocumentUpdate,
            &[&made.base.id],
        )
        .token(&desk.ada.token)
        .body(&orch_api_contract::documents::DocumentUpdateBody {
            title: Some("Guide".to_owned()),
            description: None,
        })
        .send()
        .await
        .ok();
    let titled = changes("1", None).await;
    assert_eq!(
        titled.title.as_ref().map(|t| t.before.as_str()),
        Some("Handbook")
    );
    assert_eq!(
        titled.title.as_ref().map(|t| t.after.as_str()),
        Some("Guide")
    );
    let refused = desk
        .world
        .app
        .call(DocumentChangesGet, &[&made.base.id])
        .token(&desk.ada.token)
        .query(&DocumentChangesGetQuery {
            inspect_deleted: None,
            from: "yesterday".to_owned(),
            to: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert_eq!(refused.extra["errors"][0]["path"], "from");
}

#[tokio::test]
async fn what_changed_since_i_last_looked_is_asked_with_seen() {
    let desk = Desk::new().await;
    let ines = desk.member("ines", "interactor").await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let blocks = desk.paragraphs(&desk.ada, made.base.id, &["alpha"]).await;
    desk.world.advance(Duration::seconds(WINDOW_SECS + 1));
    desk.write(
        &ines,
        made.base.id,
        vec![rewrite(&blocks[0], "alpha, by ines")],
    )
    .await
    .ok();
    desk.world.advance(Duration::seconds(1));
    // Ada has the document open now, and says up to where.
    let seq = desk.sync(&desk.ada, made.base.id).await.seq;
    desk.world
        .app
        .call(DocumentSeen, &[&made.base.id])
        .token(&desk.ada.token)
        .body(&DocumentSeenBody { seq })
        .send()
        .await
        .ok();
    desk.world.advance(Duration::seconds(WINDOW_SECS + 1));
    let page = desk.page(&ines, made.base.id).await;
    desk.write(&ines, made.base.id, vec![rewrite(&page[0], "alpha, later")])
        .await
        .ok();
    let since = desk
        .world
        .app
        .call(DocumentChangesGet, &[&made.base.id])
        .token(&desk.ada.token)
        .query(&DocumentChangesGetQuery {
            inspect_deleted: None,
            from: "seen".to_owned(),
            to: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(since.from, 2, "the last version cut before she looked");
    assert_eq!(since.blocks.len(), 1);
    assert_eq!(
        said(since.blocks[0].after.as_ref().unwrap()),
        "alpha, later"
    );
    // Somebody who never looked is told everything.
    let vera = desk.member("vera", "viewer").await;
    let never = desk
        .world
        .app
        .call(DocumentChangesGet, &[&made.base.id])
        .token(&vera.token)
        .query(&DocumentChangesGetQuery {
            inspect_deleted: None,
            from: "seen".to_owned(),
            to: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(never.from, 0);
}

#[tokio::test]
async fn the_entries_one_author_wrote_in_are_listed_by_asking_for_them_and_an_author_that_is_none_is_refused()
 {
    let desk = Desk::new().await;
    let ines = desk.member("ines", "interactor").await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let blocks = desk.paragraphs(&desk.ada, made.base.id, &["one"]).await;
    desk.world.advance(Duration::seconds(WINDOW_SECS + 1));
    desk.write(&ines, made.base.id, vec![rewrite(&blocks[0], "two")])
        .await
        .ok();
    desk.world.advance(Duration::seconds(WINDOW_SECS + 1));
    let page = desk.page(&ines, made.base.id).await;
    desk.write(&desk.ada, made.base.id, vec![rewrite(&page[0], "three")])
        .await
        .ok();
    // v1: ada, v2: ines, the open span: ada.
    let by = |author: String| {
        let call = desk.world.app.call(DocumentVersionList, &[&made.base.id]);
        let call = call
            .token(&desk.ada.token)
            .query(&DocumentVersionListQuery {
                author: Some(author),
                ..DocumentVersionListQuery::default()
            });
        async move { call.send().await.ok().items }
    };
    let by_ines = by(ines.id().to_string()).await;
    assert_eq!(
        by_ines.iter().map(|each| each.version).collect::<Vec<_>>(),
        [Some(2)]
    );
    let by_ada = by(desk.ada.id().to_string()).await;
    assert_eq!(
        by_ada.iter().map(|each| each.version).collect::<Vec<_>>(),
        [None, Some(1)]
    );
    let refused = desk
        .world
        .app
        .call(DocumentVersionList, &[&made.base.id])
        .token(&desk.ada.token)
        .query(&DocumentVersionListQuery {
            author: Some("not an author".to_owned()),
            ..DocumentVersionListQuery::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert_eq!(refused.extra["errors"][0]["path"], "author");
}

#[tokio::test]
async fn a_list_of_versions_is_paged_newest_first_with_the_open_span_on_the_first_page_only() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let blocks = desk.paragraphs(&desk.ada, made.base.id, &["v0"]).await;
    let mut latest = blocks[0].clone();
    for number in 1..=3 {
        desk.world.advance(Duration::seconds(WINDOW_SECS + 1));
        let reply = desk
            .write(
                &desk.ada,
                made.base.id,
                vec![rewrite(&latest, &format!("v{number}"))],
            )
            .await
            .ok();
        latest = reply.blocks[0].clone();
    }
    let ask = |limit: u32, cursor: Option<String>| {
        let call = desk.world.app.call(DocumentVersionList, &[&made.base.id]);
        let call = call
            .token(&desk.ada.token)
            .query(&DocumentVersionListQuery {
                limit: Some(limit),
                cursor,
                ..DocumentVersionListQuery::default()
            });
        async move { call.send().await.ok() }
    };
    let first = ask(2, None).await;
    assert_eq!(
        first
            .items
            .iter()
            .map(|each| each.version)
            .collect::<Vec<_>>(),
        [None, Some(3)],
        "the open span counts toward the page"
    );
    let second = ask(2, first.next_cursor.clone()).await;
    assert_eq!(
        second
            .items
            .iter()
            .map(|each| each.version)
            .collect::<Vec<_>>(),
        [Some(2), Some(1)]
    );
    assert_eq!(second.next_cursor, None);
}

// ---------------------------------------------------------------- how much is kept

#[tokio::test]
async fn a_history_keeps_no_more_versions_than_the_place_says_and_the_oldest_go_first() {
    let desk = Desk::new().await;
    desk.world
        .set_at(Target::Node(desk.topic), "docs.versions_kept", json!(2))
        .await;
    let made = desk.make(&desk.ada, "Handbook").await;
    let blocks = desk.paragraphs(&desk.ada, made.base.id, &["v0"]).await;
    let mut latest = blocks[0].clone();
    for number in 1..=4 {
        desk.world.advance(Duration::seconds(WINDOW_SECS + 1));
        let reply = desk
            .write(
                &desk.ada,
                made.base.id,
                vec![rewrite(&latest, &format!("v{number}"))],
            )
            .await
            .ok();
        latest = reply.blocks[0].clone();
    }
    let list = versions(&desk, &desk.ada, made.base.id).await;
    let numbers: Vec<Option<i64>> = list.iter().map(|each| each.version).collect();
    assert_eq!(
        numbers,
        [None, Some(4), Some(3)],
        "four were cut, two are kept"
    );
    let gone = desk
        .world
        .app
        .call(DocumentVersionGet, &[&made.base.id, &1])
        .token(&desk.ada.token)
        .send()
        .await
        .problem();
    assert_eq!(gone.code, ErrorCode::NotFound);
}
