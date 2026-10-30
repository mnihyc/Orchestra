//! Who may take a thing away as a file, with two people at a time: a reader without the Export row gets no file by any way (the thread, its chat, a document, an archive) and still reads it; a person named with a choice may take it away unless the giver left the row out, and the giver gives only what they hold; an archive holds what may be taken away and counts what may only be read.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod archive_world;
mod places;
mod rig;
mod support;

use archive_world::{Office, everything, has};
use orch_api_contract::access::{
    AccessChoice, Holder, HolderInput, PageChange, PageChangeAdd, PageChangeName, PageChangeRow,
    PermissionPage, PermissionPageGet, PermissionPageUpdate,
};
use orch_api_contract::chats::ChatGet;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    DocumentExport, DocumentExportGet, DocumentExportGetQuery, DocumentSyncGet,
    DocumentSyncGetQuery,
};
use orch_api_contract::exports::{ChatExportGet, ChatExportGetQuery, ExportLeftOutWhat};
use orch_api_contract::nodes::{Node, NodeKind};
use orch_api_contract::share::{
    ShareAgentCreate, ShareLevel, ShareParts, ShareTo, ShareToThread, ShareWhat, ShareWhatNode,
    ShareWithAgents,
};
use orch_api_contract::threads::{ExportFormat, ShareMode, ThreadExport, ThreadExportQuery};
use orch_api_core::testing::RawReply;
use orch_types::Id;
use places::{Who, missing_permissions};

fn thread_query(format: ExportFormat, include: Option<&str>) -> ThreadExportQuery {
    ThreadExportQuery {
        continuation_id: None,
        projection_revision: None,
        format,
        detail: None,
        from: None,
        to: None,
        include: include.map(str::to_owned),
    }
}

/// Names `who` on a page: `kind` is `nodes` or `documents`.
async fn name(
    o: &Office,
    by: &Who,
    kind: &str,
    object: Id,
    who: &Who,
    export: Option<bool>,
) -> orch_api_core::testing::Reply<PermissionPage> {
    o.s.rig
        .world
        .app
        .call(PermissionPageUpdate, &[&kind, &object])
        .token(&by.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::User { id: who.id() }],
                choice: AccessChoice::FullView,
                export,
                options: None,
                expires_at: None,
                note: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await
}

async fn page(o: &Office, by: &Who, kind: &str, object: Id) -> PermissionPage {
    o.s.rig
        .world
        .app
        .call(PermissionPageGet, &[&kind, &object])
        .token(&by.token)
        .send()
        .await
        .ok()
}

/// The entry of `user` in the row `permission` of a page.
fn entry_of(page: &PermissionPage, permission: &str, user: Id) -> Option<Id> {
    page.rows
        .iter()
        .find(|row| row.permission == permission)?
        .entries
        .iter()
        .find(|entry| matches!(&entry.holder, Holder::User { user: held } if held.id == user))
        .map(|entry| entry.id)
}

/// Puts `who` in, or takes them out of, the Export rows `permissions` of a page.
async fn rows(o: &Office, kind: &str, object: Id, who: &Who, permissions: &[&str], put: bool) {
    let current = page(o, &o.s.ada, kind, object).await;
    let changes: Vec<PageChangeRow> = permissions
        .iter()
        .filter_map(|permission| {
            let held = entry_of(&current, permission, who.id());
            if put == held.is_some() {
                return None;
            }
            Some(PageChangeRow {
                permission: (*permission).to_owned(),
                profile: None,
                inherit: None,
                locked: None,
                add: put.then(|| {
                    vec![PageChangeAdd {
                        holder: HolderInput::User { id: who.id() },
                        options: None,
                        expires_at: None,
                        note: None,
                    }]
                }),
                update: None,
                remove: held.filter(|_| !put).map(|entry| vec![entry]),
            })
        })
        .collect();
    o.s.rig
        .world
        .app
        .call(PermissionPageUpdate, &[&kind, &object])
        .token(&o.s.ada.token)
        .body(&PageChange {
            rows: Some(changes),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
}

async fn thread_export(
    o: &Office,
    by: &Who,
    format: ExportFormat,
    include: Option<&str>,
) -> RawReply {
    o.s.rig
        .world
        .app
        .call(ThreadExport, &[&o.thread.base.id])
        .token(&by.token)
        .query(&thread_query(format, include))
        .send_bytes()
        .await
}

async fn chat_export(o: &Office, by: &Who, chat: Id) -> RawReply {
    o.s.rig
        .world
        .app
        .call(ChatExportGet, &[&chat])
        .token(&by.token)
        .query(&ChatExportGetQuery::default())
        .send_bytes()
        .await
}

async fn document_export(o: &Office, by: &Who) -> RawReply {
    o.s.rig
        .world
        .app
        .call(DocumentExportGet, &[&o.document.base.id])
        .token(&by.token)
        .query(&DocumentExportGetQuery {
            format: Some(DocumentExport::Markdown),
            attachments: None,
            comments: None,
        })
        .send_bytes()
        .await
}

fn refusal(reply: &RawReply) -> orch_api_contract::common::Problem {
    assert_eq!(reply.status, 403, "{}", String::from_utf8_lossy(&reply.raw));
    serde_json::from_slice(&reply.raw).unwrap()
}

#[tokio::test]
async fn a_reader_without_the_export_row_gets_no_file_by_any_way_and_still_reads() {
    let o = Office::new().await;
    let (ada, bea) = (&o.s.ada, &o.s.bea);
    let thread = o.thread.base.id;
    // Bea is let read the thread and its chat, and the document, and is named in no Export row.
    name(&o, ada, "nodes", thread, bea, Some(false)).await.ok();
    name(&o, ada, "documents", o.document.base.id, bea, Some(false))
        .await
        .ok();
    let app = &o.s.rig.world.app;

    // She reads all three, and the views of the thread and of its chat say that she does not take them away.
    let Node::Thread(seen) = places::read(&o.s.rig.world, bea, thread).await else {
        panic!("a thread is a thread");
    };
    assert!(!seen.can.export);
    let chat = app
        .call(ChatGet, &[&o.thread_chat])
        .token(&bea.token)
        .send()
        .await
        .ok();
    assert!(!chat.can.export);
    let Node::Thread(hers) = places::read(&o.s.rig.world, ada, thread).await else {
        panic!("a thread is a thread");
    };
    assert!(hers.can.export, "Ada holds the row");
    assert!(
        app.call(ChatGet, &[&o.thread_chat])
            .token(&ada.token)
            .send()
            .await
            .ok()
            .can
            .export
    );
    let sync = app
        .call(DocumentSyncGet, &[&o.document.base.id])
        .token(&bea.token)
        .query(&DocumentSyncGetQuery::default())
        .send()
        .await
        .ok();
    assert!(!sync.blocks.is_empty());

    // The conversation, in both forms and with its chat: refused with the row it lacks.
    for (format, include) in [
        (ExportFormat::Markdown, None),
        (ExportFormat::Json, None),
        (ExportFormat::Markdown, Some("chat")),
        (ExportFormat::Json, Some("chat")),
    ] {
        let refused = refusal(&thread_export(&o, bea, format, include).await);
        assert_eq!(
            refused.code,
            ErrorCode::AuthzDenied,
            "{format:?} {include:?}"
        );
        assert_eq!(missing_permissions(&refused), ["thread.export"]);
    }
    // The chat of the thread.
    let refused = refusal(&chat_export(&o, bea, o.thread_chat).await);
    assert_eq!(missing_permissions(&refused), ["chat.export"]);
    // The document.
    let refused = refusal(&document_export(&o, bea).await);
    assert_eq!(missing_permissions(&refused), ["doc.export"]);

    // An archive of the thread: nothing in it, and what she reads and may not take away is counted, not named.
    let plan = o.plan(bea, thread, &everything()).await.ok();
    assert_eq!(
        (
            plan.counts.conversations,
            plan.counts.chats,
            plan.counts.documents
        ),
        (0, 0, 0)
    );
    assert_eq!(plan.blockers.len(), 1);
    assert_eq!(plan.blockers[0].code, "export.nothing");
    assert_eq!(plan.left_out.len(), 1);
    assert_eq!(plan.left_out[0].what, ExportLeftOutWhat::NotExportable);
    assert_eq!(plan.left_out[0].count, 2, "the conversation and the chat");
    let place = plan.left_out[0].place.as_ref().unwrap();
    assert_eq!(
        (place.id, place.kind, place.redacted),
        (thread, NodeKind::Thread, false)
    );
    assert_eq!(
        o.ask(bea, thread, &everything()).await.problem().code,
        ErrorCode::ExportNothing
    );
    assert!(o.list(bea).await.is_empty());

    // Given the rows, she takes each away, and each is the same file that Ada gets.
    rows(
        &o,
        "nodes",
        thread,
        bea,
        &["thread.export", "chat.export"],
        true,
    )
    .await;
    rows(
        &o,
        "documents",
        o.document.base.id,
        bea,
        &["doc.export"],
        true,
    )
    .await;
    for (format, include) in [
        (ExportFormat::Markdown, None),
        (ExportFormat::Json, Some("chat")),
    ] {
        let hers = thread_export(&o, bea, format, include).await;
        let ours = thread_export(&o, ada, format, include).await;
        assert_eq!(hers.status, 200, "{}", String::from_utf8_lossy(&hers.raw));
        assert!(!hers.raw.is_empty());
        if format == ExportFormat::Markdown {
            assert_eq!(hers.raw, ours.raw);
        }
    }
    assert_eq!(chat_export(&o, bea, o.thread_chat).await.status, 200);
    assert_eq!(document_export(&o, bea).await.status, 200);
    let Node::Thread(seen) = places::read(&o.s.rig.world, bea, thread).await else {
        panic!("a thread is a thread");
    };
    assert!(seen.can.export, "given the row, the view says so");
    let chat = app
        .call(ChatGet, &[&o.thread_chat])
        .token(&bea.token)
        .send()
        .await
        .ok();
    assert!(chat.can.export);
    let made = o.made(bea, thread, &everything()).await;
    let held = o.contents(bea, &made).await;
    assert!(
        has(&held, "conversations/Fix login.md") || has(&held, "Fix login.md"),
        "{:?}",
        held.keys()
    );
    assert!(has(&held, "chat.md"));

    // Taken away again: refused again, and the reading is as it was.
    rows(
        &o,
        "nodes",
        thread,
        bea,
        &["thread.export", "chat.export"],
        false,
    )
    .await;
    rows(
        &o,
        "documents",
        o.document.base.id,
        bea,
        &["doc.export"],
        false,
    )
    .await;
    assert_eq!(
        thread_export(&o, bea, ExportFormat::Markdown, None)
            .await
            .status,
        403
    );
    assert_eq!(document_export(&o, bea).await.status, 403);
    let chat = app
        .call(ChatGet, &[&o.thread_chat])
        .token(&bea.token)
        .send()
        .await
        .ok();
    assert!(!chat.can.export, "and the chat says so again");
    let Node::Thread(seen) = places::read(&o.s.rig.world, bea, thread).await else {
        panic!("a thread is a thread");
    };
    assert!(!seen.can.export);
}

#[tokio::test]
async fn named_with_a_choice_a_person_may_take_it_away_unless_the_giver_left_it_out_or_does_not_hold_it()
 {
    let o = Office::new().await;
    let (ada, bea) = (&o.s.ada, &o.s.bea);
    let fay = o.s.rig.person("fay").await;
    let thread = o.thread.base.id;

    // Ada holds the Export rows and names Bea with the choice as it comes: Bea may take it away.
    name(&o, ada, "nodes", thread, bea, None).await.ok();
    assert_eq!(
        thread_export(&o, bea, ExportFormat::Markdown, None)
            .await
            .status,
        200
    );
    let hers = page(&o, ada, "nodes", thread).await;
    assert!(entry_of(&hers, "thread.export", bea.id()).is_some());
    assert!(entry_of(&hers, "chat.export", bea.id()).is_some());

    // Ada leaves it out for Fay: she reads and does not take it away.
    name(&o, ada, "nodes", thread, &fay, Some(false)).await.ok();
    assert_eq!(
        thread_export(&o, &fay, ExportFormat::Markdown, None)
            .await
            .status,
        403
    );
    let shown = page(&o, ada, "nodes", thread).await;
    assert!(entry_of(&shown, "thread.view", fay.id()).is_some());
    assert!(entry_of(&shown, "thread.export", fay.id()).is_none());

    // Fay holds no Export row, so what she gives has none either, whatever she asks: the call is not refused.
    let gus = o.s.rig.person("gus").await;
    let given = name(&o, &fay, "nodes", thread, &gus, Some(true)).await;
    assert!(
        given.status.is_success() || given.status == 403,
        "naming to read is either hers to do or not, never half done: {}",
        given.status
    );
    if given.status.is_success() {
        let page = page(&o, ada, "nodes", thread).await;
        assert!(entry_of(&page, "thread.view", gus.id()).is_some());
        assert!(
            entry_of(&page, "thread.export", gus.id()).is_none(),
            "nobody gives what they do not hold"
        );
        assert_eq!(
            thread_export(&o, &gus, ExportFormat::Markdown, None)
                .await
                .status,
            403
        );
    }

    // Bea holds them, and gives them: the ones she names with the choice may take it away.
    let cat = o.s.rig.person("cat").await;
    let given = name(&o, bea, "nodes", thread, &cat, None).await;
    if given.status.is_success() {
        assert_eq!(
            thread_export(&o, &cat, ExportFormat::Markdown, None)
                .await
                .status,
            200,
            "Bea held the Export row and passed it on"
        );
    }
}

#[tokio::test]
async fn an_archive_holds_what_may_be_taken_away_and_counts_what_may_only_be_read() {
    let o = Office::new().await;
    let ada = &o.s.ada;
    let fay = &o.s.rig.person("fay").await;
    let topic = o.topic();
    // Fay may read the topic, its chat, the thread in it and its chat, and take away only the document: she is named on the
    // page of the topic and on the page of the thread, in no Export row of either.
    name(&o, ada, "nodes", topic, fay, Some(false)).await.ok();
    name(&o, ada, "nodes", o.thread.base.id, fay, Some(false))
        .await
        .ok();
    rows(
        &o,
        "documents",
        o.document.base.id,
        fay,
        &["doc.export"],
        true,
    )
    .await;

    let plan = o.plan(fay, topic, &everything()).await.ok();
    assert_eq!(
        (
            plan.counts.conversations,
            plan.counts.chats,
            plan.counts.documents
        ),
        (0, 0, 1),
        "{plan:?}"
    );
    let left: i64 = plan
        .left_out
        .iter()
        .filter(|each| each.what == ExportLeftOutWhat::NotExportable)
        .map(|each| each.count)
        .sum();
    assert_eq!(
        left, 3,
        "the conversation and the two chats: {:?}",
        plan.left_out
    );
    let export = o.made(fay, topic, &everything()).await;
    let held = o.contents(fay, &export).await;
    assert!(has(&held, "docs/Handbook.md"), "{:?}", held.keys());
    assert!(
        !has(&held, "chat.md") && !has(&held, "Fix login.md"),
        "{:?}",
        held.keys()
    );
    let counted: i64 = export
        .left_out
        .iter()
        .filter(|each| each.what == ExportLeftOutWhat::NotExportable)
        .map(|each| each.count)
        .sum();
    assert_eq!(counted, 3);
    // The same archive for Ada holds all of it.
    let all = o.made(ada, topic, &everything()).await;
    let held = o.contents(ada, &all).await;
    assert!(has(&held, "docs/Handbook.md") && has(&held, "Fix login.md") && has(&held, "chat.md"));
    assert!(all.left_out.is_empty());
}

/// A live share of the timeline of `what` to the agents of `to`, as `by` asks it.
async fn share(
    o: &Office,
    by: &Who,
    what: Id,
    to: Id,
    download: Option<bool>,
    client: &str,
) -> orch_api_core::testing::Reply<orch_api_contract::share::ShareAgentCreateReply> {
    o.s.rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&by.token)
        .body(&ShareWithAgents {
            read_first: None,
            download,
            what: ShareWhat::Node(ShareWhatNode { node_id: what }),
            inside: None,
            parts: Some(ShareParts {
                timeline: Some(ShareLevel::Read),
                ..ShareParts::default()
            }),
            mode: ShareMode::Live,
            to: ShareTo::Threads(vec![ShareToThread { thread_id: to }]),
            comment: None,
            expires_at: None,
            client_id: client.to_owned(),
        })
        .send()
        .await
}

#[tokio::test]
async fn a_live_share_says_whether_the_agents_may_copy_it_and_names_them_in_the_export_rows_as_far_as_the_giver_holds_them()
 {
    let o = Office::new().await;
    sqlx::query("UPDATE agents SET descriptor = jsonb_set(descriptor, '{capabilities,orch_tools}', '\"mcp\"')")
        .execute(o.s.rig.db.pool())
        .await
        .unwrap();
    let (ada, thread) = (&o.s.ada, o.thread.base.id);
    let readers = [
        o.s.thread_by(ada, "Reader one").await.base.id,
        o.s.thread_by(ada, "Reader two").await.base.id,
        o.s.thread_by(ada, "Reader three").await.base.id,
    ];

    // As it comes, the agents may read and copy: they are named in the View row and in the Export row.
    let made = share(&o, ada, thread, readers[0], None, "c1").await;
    assert_eq!(made.status, 201);
    let made = made.ok();
    assert!(made.shares[0].download);
    assert_eq!(
        made.shares[0].entries.len(),
        2,
        "the View row and the Export row"
    );

    // Unticked, they read it and do not copy it.
    let reading = share(&o, ada, thread, readers[1], Some(false), "c2")
        .await
        .ok();
    assert!(!reading.shares[0].download);
    assert_eq!(reading.shares[0].entries.len(), 1, "the View row alone");

    // Somebody who reads the thread and does not hold its Export row shares it to be read, whatever they ask:
    // the agents are named in no Export row, and the share says so.
    let bea = &o.s.bea;
    name(&o, ada, "nodes", thread, bea, Some(false)).await.ok();
    let by_bea = share(&o, bea, thread, readers[2], Some(true), "c3").await;
    if by_bea.status.is_success() {
        let by_bea = by_bea.ok();
        assert!(
            !by_bea.shares[0].download,
            "nobody gives what they do not hold"
        );
        assert_eq!(by_bea.shares[0].entries.len(), 1);
    } else {
        assert_eq!(by_bea.problem().code, ErrorCode::AuthzDenied);
    }
}
