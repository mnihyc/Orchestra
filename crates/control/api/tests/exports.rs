//! An archive of what a person may read in a place, over HTTP: planned before it is made, asked for as a job, listed, read, fetched and removed by its person alone; what it holds is what its person could take away one thing at a time, byte for byte; and what stands in its way says so with a figure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod archive_world;
mod places;
mod rig;
mod support;

use archive_world::{
    Office, entry, entry_with, everything, has, kept, leads_to, links_of, text_of,
};
use orch_api_contract::chats::{ChatMessageCreate, ChatMessageCreateBody, ChatPart};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    BlockProps, BlockType, DocumentExport, DocumentExportChoice, DocumentExportGet,
    DocumentExportGetQuery, DocumentFile, DocumentFileCreate, DocumentFileCreateQuery, DocumentOp,
    DocumentOpsCreate, DocumentOpsCreateBody, FileProps, ImageProps, NewBlock,
};
use orch_api_contract::exports::{
    ChatExportFiles, ChatExportGet, ChatExportGetQuery, ExportChosen, ExportParts, ExportRequest,
    ExportState, ExportWhat,
};
use orch_api_contract::nodes::{JobState, NodeKind, NodeRef};
use orch_api_contract::threads::{
    ExportDetail, ExportFormat, FILENAME_HEADER, Forwarded, ForwardedAttachment, ForwardedFrom,
    ForwardedImage, ThreadExport, ThreadExportQuery,
};
use orch_api_contract::workspace::JobGet;
use orch_settings::Target;
use orch_types::{Id, Time, new_id};
use serde_json::{Value, json};

fn thread_query(format: ExportFormat) -> ThreadExportQuery {
    ThreadExportQuery {
        continuation_id: None,
        projection_revision: None,
        format,
        detail: None,
        from: None,
        to: None,
        include: None,
    }
}

#[tokio::test]
async fn an_archive_is_planned_asked_for_listed_fetched_and_removed_by_its_person_alone() {
    let o = Office::new().await;
    let (ada, cai) = (&o.s.ada, &o.s.cai);
    let place = o.org();

    // The plan says what it would hold before anything is made.
    let plan = o.plan(ada, place, &everything()).await.ok();
    assert_eq!(plan.node.id, place);
    assert_eq!(
        (
            plan.counts.conversations,
            plan.counts.chats,
            plan.counts.documents,
            plan.counts.files
        ),
        (1, 2, 1, 0),
        "the thread, the two chats that say something, the document: {:?}",
        plan.counts
    );
    assert!(
        plan.blockers.is_empty() && plan.left_out.is_empty(),
        "{plan:?}"
    );
    assert!(plan.bytes > 0);
    assert_eq!(
        (plan.limits.max_items, plan.limits.keep_hours),
        (10_000, 24)
    );

    // Asked for, it is a job that writes it.
    let asked = o.ask(ada, place, &everything()).await;
    assert_eq!(asked.status, 202);
    let job = asked.ok();
    assert_eq!(job.kind, "export");
    assert_eq!(job.created_by.as_ref().map(|by| by.id), Some(ada.id()));
    let seen_by_cai =
        o.s.rig
            .world
            .app
            .call(JobGet, &[&job.id])
            .token(&cai.token)
            .send()
            .await
            .problem();
    assert_eq!(
        seen_by_cai.status, 404,
        "the job of an archive is its person's alone"
    );
    let ended = o.ended(ada, job.id).await;
    assert_eq!(ended.state, JobState::Succeeded, "{:?}", ended.error);
    let id = ended.result.as_ref().unwrap()["export_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    // Listed and read: ready, counted, with the time it is kept until.
    let listed = o.list(ada).await;
    assert_eq!(listed.len(), 1);
    let export = &listed[0];
    assert_eq!((export.id, export.state), (id, ExportState::Ready));
    assert_eq!(export.job_id, Some(job.id));
    assert_eq!(export.node.as_ref().map(|node| node.id), Some(place));
    let counts = export.counts.unwrap();
    assert_eq!(
        (
            counts.conversations,
            counts.chats,
            counts.documents,
            counts.files
        ),
        (1, 2, 1, 0)
    );
    assert!(export.bytes.unwrap() > 0 && export.left_out.is_empty());
    let (ready, expires) = (export.ready_at.unwrap(), export.expires_at.unwrap());
    assert_eq!(expires.get() - ready.get(), chrono::Duration::hours(24));
    assert!(export.downloaded_at.is_none());

    // Nobody else reads it, lists it, fetches it or removes it.
    assert!(o.list(cai).await.is_empty());
    assert_eq!(o.read(cai, id).await.problem().status, 404);
    assert_eq!(o.fetch(cai, id).await.status, 404);
    assert_eq!(o.remove(cai, id).await.status, 404);
    let stranger = &o.s.bea;
    assert_eq!(o.fetch(stranger, id).await.status, 404);
    // Not even a system admin: it is nobody's but its person's.
    let admin = places::admin(&o.s.rig.world, "root").await;
    assert_eq!(o.fetch(&admin, id).await.status, 404);
    assert_eq!(o.read(&admin, id).await.problem().status, 404);
    assert_eq!(o.remove(&admin, id).await.status, 404);
    assert!(o.list(&admin).await.is_empty());

    // Fetched: a zip named for the place and the day, again as often as it is wanted, the first time noted.
    let fetched = o.fetch(ada, id).await;
    assert_eq!(fetched.status, 200);
    assert_eq!(fetched.headers["content-type"], "application/zip");
    let disposition = fetched.headers["content-disposition"].to_str().unwrap();
    assert!(
        disposition.starts_with("attachment; filename=\"Acme-") && disposition.contains(".zip\""),
        "{disposition}"
    );
    assert_eq!(fetched.headers["cache-control"], "private, no-store");
    let again = o.fetch(ada, id).await;
    assert_eq!(again.raw, fetched.raw);
    let exported = o.read(ada, id).await.ok();
    assert!(exported.downloaded_at.is_some());
    let noted: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = 'export.downloaded'")
            .fetch_one(o.s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(noted, 1, "the first time, not every time");
    let made: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = 'export.created'")
            .fetch_one(o.s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(
        made, 2,
        "once when it was asked for, and once when it was written"
    );

    // Removed now: the row stays and says so, the bytes are gone, and removing again is not there.
    let removed = o.remove(ada, id).await;
    assert_eq!(removed.status, 204);
    let after = o.read(ada, id).await.ok();
    assert_eq!(after.state, ExportState::Expired);
    let gone = o.fetch(ada, id).await;
    assert_eq!(gone.status, 410);
    assert_eq!(
        serde_json::from_slice::<Value>(&gone.raw).unwrap()["code"],
        "gone"
    );
    assert_eq!(o.remove(ada, id).await.status, 404);
    let blobs: i64 = sqlx::query_scalar("SELECT count(*) FROM blobs WHERE pin_count > 0")
        .fetch_one(o.s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(
        blobs, 0,
        "nothing is pinned for an archive that was removed"
    );
}

#[tokio::test]
async fn what_an_archive_holds_is_what_its_person_could_take_away_one_thing_at_a_time_byte_for_byte()
 {
    let o = Office::new().await;
    let ada = &o.s.ada;
    let app = &o.s.rig.world.app;
    let pool = o.s.rig.db.pool();
    // More than two producer windows, including loose history on a boundary.
    sqlx::query(
        "INSERT INTO turns (id, thread_id, segment_id, ordinal, state, input_id, model, params, started_at, completed_at, seq_updated)
         SELECT gen_random_uuid(), t.thread_id, t.segment_id, n, 'completed', t.input_id, t.model, t.params,
                t.started_at, t.completed_at, 1000000 + n * 4
           FROM turns t CROSS JOIN generate_series(3, 67) n WHERE t.thread_id = $1 AND t.ordinal = 1",
    ).bind(o.thread.base.id).execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO items (id, thread_id, segment_id, turn_id, kind, status, payload, seq_created, seq_updated)
         SELECT gen_random_uuid(), t.thread_id, t.segment_id, t.id, 'agent_message', 'completed',
                jsonb_set(seed.payload, '{text}', to_jsonb('window-' || t.ordinal)),
                1000000 + t.ordinal * 4, 1000000 + t.ordinal * 4
           FROM turns t CROSS JOIN LATERAL (
               SELECT payload FROM items WHERE thread_id = $1 AND kind = 'agent_message' ORDER BY seq_created LIMIT 1
           ) seed WHERE t.thread_id = $1 AND t.ordinal >= 3",
    ).bind(o.thread.base.id).execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO items (id, thread_id, segment_id, kind, status, payload, seq_created, seq_updated)
         SELECT gen_random_uuid(), thread_id, segment_id, 'error', 'failed',
                '{\"code\":\"note\",\"message\":\"loose-window-boundary\",\"retryable\":false}'::jsonb,
                1000129, 1000129 FROM turns WHERE thread_id = $1 AND ordinal = 32",
    ).bind(o.thread.base.id).execute(pool).await.unwrap();
    sqlx::query("UPDATE threads SET next_seq = 1000270 WHERE node_id = $1")
        .bind(o.thread.base.id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO chat_messages (id, chat_id, kind, author_id, content, seq_created, seq_updated, search_text, reply_to_id)
         SELECT gen_random_uuid(), $1, 'message', $2,
                jsonb_build_array(jsonb_build_object('type', 'text', 'text', 'chat-window-' || n)),
                1000000 + n, 1000000 + n, 'chat-window-' || n,
                (SELECT id FROM chat_messages WHERE chat_id = $1 ORDER BY seq_created LIMIT 1)
           FROM generate_series(1, 67) n",
    ).bind(o.thread_chat).bind(ada.id()).execute(pool).await.unwrap();
    sqlx::query("UPDATE chats SET next_seq = 1000069 WHERE id = $1")
        .bind(o.thread_chat)
        .execute(pool)
        .await
        .unwrap();
    for (format, detail, extension) in [
        (ExportFormat::Markdown, None, "md"),
        (ExportFormat::Markdown, Some(ExportDetail::Full), "md"),
        (ExportFormat::Json, None, "json"),
    ] {
        let request = ExportRequest {
            format: Some(format),
            detail,
            ..everything()
        };
        let export = o.made(ada, o.org(), &request).await;
        let held = o.contents(ada, &export).await;

        let conversation = app
            .call(ThreadExport, &[&o.thread.base.id])
            .token(&ada.token)
            .query(&ThreadExportQuery {
                detail,
                ..thread_query(format)
            })
            .send_bytes()
            .await;
        assert_eq!(conversation.status, 200);
        assert_eq!(
            entry(&held, &format!("conversations/Fix login.{extension}")),
            &conversation.raw[..],
            "the conversation, as {format:?}"
        );
        if format == ExportFormat::Markdown {
            let text = text_of(&conversation.raw);
            assert_eq!(
                text.lines().filter(|line| line.starts_with("# ")).count(),
                1
            );
            assert_eq!(text.matches("window-67").count(), 1);
            assert_eq!(
                text.matches("loose-window-boundary").count(),
                usize::from(detail == Some(ExportDetail::Full))
            );
        }
        for (chat, tail) in [
            (o.topic_chat, format!("Web/Login/chat.{extension}")),
            (o.thread_chat, format!("Login/Fix login/chat.{extension}")),
        ] {
            let single = app
                .call(ChatExportGet, &[&chat])
                .token(&ada.token)
                .query(&ChatExportGetQuery {
                    format: Some(format),
                    ..ChatExportGetQuery::default()
                })
                .send_bytes()
                .await;
            assert_eq!(single.status, 200);
            assert_eq!(entry(&held, &tail), &single.raw[..], "the chat {tail}");
        }
        let document = app
            .call(DocumentExportGet, &[&o.document.base.id])
            .token(&ada.token)
            .query(&DocumentExportGetQuery {
                format: Some(match format {
                    ExportFormat::Markdown => DocumentExport::Markdown,
                    ExportFormat::Json => DocumentExport::Json,
                }),
                attachments: None,
                comments: None,
            })
            .send_bytes()
            .await;
        assert_eq!(document.status, 200);
        assert_eq!(
            entry(&held, &format!("docs/Handbook.{extension}")),
            &document.raw[..],
            "the document"
        );

        // The index says what is where, and the manifest says who made it, of what, and how much.
        let index = text_of(entry(&held, "index.md"));
        assert!(
            index.contains("Fix login") && index.contains("Handbook"),
            "{index}"
        );
        let manifest: Value = serde_json::from_slice(entry(&held, "export.json")).unwrap();
        assert_eq!(manifest["made_by"]["username"], "ada");
        assert_eq!(manifest["of"]["id"], json!(o.org()));
        assert_eq!(manifest["counts"]["conversations"], 1);
        assert_eq!(manifest["counts"]["documents"], 1);
        assert_eq!(held.len(), 6, "{:?}", held.keys());
    }
}

#[tokio::test]
async fn what_may_not_be_asked_is_refused_with_what_it_comes_to() {
    let o = Office::new().await;
    let (ada, bea) = (&o.s.ada, &o.s.bea);

    // A person who cannot see the place cannot ask about it.
    let refused = o.plan(bea, o.org(), &everything()).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(
        places::missing_permissions(&refused),
        ["org.view"],
        "the view of the kind of place it is"
    );
    assert_eq!(
        o.ask(bea, o.org(), &everything()).await.problem().code,
        ErrorCode::AuthzDenied
    );

    // A place with nothing that can be taken away: said in the plan, refused when asked.
    let empty = places::make_topic(&o.s.rig.world, ada, o.s.tree.project.base.id, "Empty").await;
    let plan = o.plan(ada, empty.base.id, &everything()).await.ok();
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
    let nothing = o.ask(ada, empty.base.id, &everything()).await.problem();
    assert_eq!(
        (nothing.status, nothing.code),
        (409, ErrorCode::ExportNothing)
    );
    // Choosing no kind at all is nothing as well.
    let none = ExportRequest {
        parts: Some(ExportParts {
            conversations: Some(false),
            chats: Some(false),
            documents: Some(false),
        }),
        ..everything()
    };
    assert_eq!(
        o.ask(ada, o.org(), &none).await.problem().code,
        ErrorCode::ExportNothing
    );

    // A day that is no day, and an end before the start.
    let bad_day = ExportRequest {
        from: Some("yesterday".to_owned()),
        ..everything()
    };
    assert_eq!(
        o.plan(ada, o.org(), &bad_day).await.problem().code,
        ErrorCode::RequestInvalid
    );
    let backwards = ExportRequest {
        from: Some("2026-10-12".to_owned()),
        to: Some("2026-10-11".to_owned()),
        ..everything()
    };
    assert_eq!(
        o.ask(ada, o.org(), &backwards).await.problem().code,
        ErrorCode::RequestInvalid
    );

    // More things than the place allows: the plan says so, and asking is refused with both figures.
    o.s.rig
        .world
        .set_at(Target::Node(o.org()), "export.max_items", json!(3))
        .await;
    let plan = o.plan(ada, o.org(), &everything()).await.ok();
    assert_eq!(plan.limits.max_items, 3);
    assert_eq!(plan.blockers.len(), 1);
    assert_eq!(plan.blockers[0].code, "export.too_many");
    let many = o.ask(ada, o.org(), &everything()).await.problem();
    assert_eq!((many.status, many.code), (409, ErrorCode::ExportTooMany));
    assert_eq!(many.extra["limit"], 3);
    assert_eq!(many.extra["asked"], 4);
    o.s.rig
        .world
        .set_at(Target::Node(o.org()), "export.max_items", json!(10_000))
        .await;

    // Larger than the place allows.
    o.s.rig
        .world
        .set_at(Target::Node(o.org()), "export.max_bytes", json!(100))
        .await;
    let plan = o.plan(ada, o.org(), &everything()).await.ok();
    assert_eq!(plan.blockers.len(), 1);
    assert_eq!(plan.blockers[0].code, "export.too_large");
    let large = o.ask(ada, o.org(), &everything()).await.problem();
    assert_eq!((large.status, large.code), (409, ErrorCode::ExportTooLarge));
    assert_eq!(large.extra["limit"], 100);
    assert!(large.extra["asked"].as_i64().unwrap() > 100);
    assert!(
        o.list(ada).await.is_empty(),
        "nothing was made by a refused request"
    );
}

#[tokio::test]
async fn an_archive_may_be_of_part_of_a_place_in_a_form_and_a_span_that_were_asked_for() {
    let o = Office::new().await;
    let ada = &o.s.ada;
    // Only the chats of the place: no conversation, no document.
    let chats_only = ExportRequest {
        parts: Some(ExportParts {
            conversations: Some(false),
            chats: None,
            documents: Some(false),
        }),
        ..everything()
    };
    let export = o.made(ada, o.org(), &chats_only).await;
    let held = o.contents(ada, &export).await;
    assert!(has(&held, "Login/chat.md") && has(&held, "Fix login/chat.md"));
    assert!(
        !has(&held, "Fix login.md") && !has(&held, "Handbook.md"),
        "{:?}",
        held.keys()
    );

    // Of a thread alone: its conversation and its chat, in a directory of its own.
    let thread = o.made(ada, o.thread.base.id, &everything()).await;
    let held = o.contents(ada, &thread).await;
    assert!(
        has(&held, "Fix login.md") || has(&held, "conversations/Fix login.md"),
        "{:?}",
        held.keys()
    );

    // Chosen by name: the document alone, found in the place.
    let chosen = ExportRequest {
        what: ExportWhat::Chosen(ExportChosen {
            nodes: None,
            documents: Some(vec![o.document.base.id]),
            chats: None,
        }),
        ..everything()
    };
    let export = o.made(ada, o.org(), &chosen).await;
    let held = o.contents(ada, &export).await;
    assert!(has(&held, "docs/Handbook.md"));
    assert!(
        !has(&held, "chat.md") && !has(&held, "Fix login.md"),
        "{:?}",
        held.keys()
    );
}

#[tokio::test]
async fn a_span_of_days_narrows_the_turns_and_the_messages_and_leaves_the_documents_as_they_stand()
{
    let o = Office::new().await;
    let ada = &o.s.ada;
    let day = |text: &str| -> chrono::DateTime<chrono::Utc> { text.parse().unwrap() };
    let pool = o.s.rig.db.pool();
    for (turn, at) in [(1, "2026-09-01T10:00:00Z"), (2, "2026-09-03T10:00:00Z")] {
        sqlx::query("UPDATE turns SET created_at = $2 WHERE thread_id = $1 AND ordinal = $3")
            .bind(o.thread.base.id)
            .bind(day(at))
            .bind(turn)
            .execute(pool)
            .await
            .unwrap();
    }
    for (chat, at) in [
        (o.topic_chat, "2026-09-01T10:00:00Z"),
        (o.thread_chat, "2026-09-03T10:00:00Z"),
    ] {
        sqlx::query("UPDATE chat_messages SET created_at = $2 WHERE chat_id = $1")
            .bind(chat)
            .bind(day(at))
            .execute(pool)
            .await
            .unwrap();
    }
    let span = |from: Option<&str>, to: Option<&str>| ExportRequest {
        from: from.map(str::to_owned),
        to: to.map(str::to_owned),
        ..everything()
    };

    // From the second of September: the second turn, the chat that spoke on the third, the document as it stands.
    let later = o.made(ada, o.org(), &span(Some("2026-09-02"), None)).await;
    let held = o.contents(ada, &later).await;
    let conversation = text_of(entry(&held, "conversations/Fix login.md"));
    assert!(
        conversation.contains("Second answer") && !conversation.contains("First answer"),
        "{conversation}"
    );
    assert!(
        has(&held, "Fix login/chat.md") && !has(&held, "Web/Login/chat.md"),
        "{:?}",
        held.keys()
    );
    assert!(has(&held, "docs/Handbook.md"));
    // It is the conversation of the turns that began in those days, as the single export gives them.
    let single =
        o.s.rig
            .world
            .app
            .call(ThreadExport, &[&o.thread.base.id])
            .token(&ada.token)
            .query(&ThreadExportQuery {
                from: Some(2),
                to: Some(2),
                ..thread_query(ExportFormat::Markdown)
            })
            .send_bytes()
            .await;
    assert_eq!(entry(&held, "conversations/Fix login.md"), &single.raw[..]);

    // To the first of September: the first turn and the chat of that day.
    let earlier = o.made(ada, o.org(), &span(None, Some("2026-09-01"))).await;
    let held = o.contents(ada, &earlier).await;
    let conversation = text_of(entry(&held, "conversations/Fix login.md"));
    assert!(
        conversation.contains("First answer") && !conversation.contains("Second answer"),
        "{conversation}"
    );
    assert!(
        has(&held, "Web/Login/chat.md") && !has(&held, "Fix login/chat.md"),
        "{:?}",
        held.keys()
    );
}

/// The bytes of a picture, as far as a file type goes.
fn picture() -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    bytes.extend_from_slice(b"not really a picture, but it begins as one");
    bytes
}

/// Ada puts a file into `document`, sending its bytes as the body.
async fn upload(o: &Office, document: Id, name: &str, bytes: &[u8]) -> DocumentFile {
    o.s.rig
        .world
        .app
        .call(DocumentFileCreate, &[&document])
        .token(&o.s.ada.token)
        .query(&DocumentFileCreateQuery { overwrite: None })
        .header(FILENAME_HEADER, name)
        .bytes(bytes)
        .send()
        .await
        .ok()
}

/// A top-level block that shows a file of the document.
fn showing(block_type: BlockType, file: Id) -> DocumentOp {
    let props = match block_type {
        BlockType::Image => BlockProps::Image(ImageProps {
            display: None,
            file_id: file,
            width: None,
            align: None,
        }),
        _ => BlockProps::File(FileProps {
            display: None,
            file_id: file,
        }),
    };
    DocumentOp::Insert {
        block: NewBlock {
            id: new_id(),
            block_type,
            props: Some(props),
            content: None,
            children: None,
        },
        parent_id: None,
        after_id: None,
    }
}

#[tokio::test]
async fn every_link_in_the_written_texts_of_an_archive_leads_to_a_file_in_it_with_the_bytes_that_were_kept()
 {
    let o = Office::new().await;
    let ada = &o.s.ada;
    let app = &o.s.rig.world.app;
    let pool = o.s.rig.db.pool();

    // A document that shows a picture and offers a file, both of the document's own.
    let guide = Office::write_document(&o.s, ada, "Guide", "Read this first.\n").await;
    let (sheet, picture) = (b"a,b\n1,2\n".to_vec(), picture());
    let image = upload(&o, guide.base.id, "diagram one.png", &picture).await;
    let data = upload(&o, guide.base.id, "data.csv", &sheet).await;
    app.call(DocumentOpsCreate, &[&guide.base.id])
        .token(&ada.token)
        .body(&DocumentOpsCreateBody {
            expected_seq: None,
            client_id: new_id().to_string(),
            ops: vec![
                showing(BlockType::File, data.id),
                showing(BlockType::Image, image.id),
            ],
        })
        .send()
        .await
        .ok();

    // A conversation whose two first messages each carried a file.
    let (trace, trace_size) = kept(&o.s, b"GET /login 500\n").await;
    sqlx::query(
        "UPDATE items SET payload = jsonb_set(payload, '{parts}', (payload -> 'parts') || $2::jsonb)
          WHERE thread_id = $1 AND kind = 'user_message'",
    )
    .bind(o.thread.base.id)
    .bind(json!([{
        "type": "file", "path": ".orch/uploads/trace.har", "name": "trace.har",
        "size": trace_size, "blob": trace.to_string()
    }]))
    .execute(pool)
    .await
    .unwrap();

    // Repeated Unicode names, including an image and file inside one forwarded part.
    let (notes, notes_size) = kept(&o.s, b"the notes of the meeting").await;
    let (later_notes, later_notes_size) = kept(&o.s, b"the corrected notes").await;
    orch_db::queries::uploads::remember(
        &mut pool.acquire().await.unwrap(),
        ada.id(),
        &notes,
        o.s.rig.world.now(),
    )
    .await
    .unwrap();
    orch_db::queries::uploads::remember(
        &mut pool.acquire().await.unwrap(),
        ada.id(),
        &later_notes,
        o.s.rig.world.now(),
    )
    .await
    .unwrap();
    app.call(ChatMessageCreate, &[&o.thread_chat])
        .token(&ada.token)
        .body(&ChatMessageCreateBody {
            agent_review: None,
            parts: vec![
                ChatPart::Text {
                    text: "see the notes".to_owned(),
                },
                ChatPart::Attachment {
                    blob: notes,
                    name: "会議 notes.txt".to_owned(),
                    size: notes_size,
                    media_type: "text/plain".to_owned(),
                },
                ChatPart::Attachment {
                    blob: later_notes,
                    name: "会議 notes.txt".to_owned(),
                    size: later_notes_size,
                    media_type: "text/plain".to_owned(),
                },
            ],
            reply_to: None,
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    let (shot, _) = kept(&o.s, &picture).await;
    let (log, log_size) = kept(&o.s, b"ten lines of a log").await;
    let forwarded = ChatPart::Forwarded(Box::new(Forwarded {
        r#type: orch_api_contract::threads::ForwardedType::Forwarded,
        from: ForwardedFrom::Thread {
            thread: NodeRef {
                thread_display: None,
                id: o.thread.base.id,
                kind: NodeKind::Thread,
                name: Some("Fix login".to_owned()),
                redacted: false,
            },
        },
        source_id: new_id(),
        author: None,
        at: Time::new(o.s.rig.world.now()),
        text: "this is what it did".to_owned(),
        images: vec![ForwardedImage {
            blob: shot,
            media_type: "image/png".to_owned(),
            name: "図表.png".to_owned(),
        }],
        attachments: vec![ForwardedAttachment {
            blob: log,
            name: "図表.png".to_owned(),
            size: log_size,
            media_type: "text/plain".to_owned(),
        }],
    }));
    let seq: i64 = sqlx::query_scalar(
        "UPDATE chats SET next_seq = next_seq + 1 WHERE id = $1 RETURNING next_seq - 1",
    )
    .bind(o.thread_chat)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chat_messages (id, chat_id, kind, author_id, content, seq_created, seq_updated, search_text)
         VALUES ($1, $2, 'message', $3, $4, $5, $5, 'forwarded')",
    )
    .bind(new_id())
    .bind(o.thread_chat)
    .bind(ada.id())
    .bind(json!([serde_json::to_value(&forwarded).unwrap()]))
    .bind(seq)
    .execute(pool)
    .await
    .unwrap();

    // The archive, with its files and the earlier versions of what has them.
    let request = ExportRequest {
        files: Some(true),
        versions: Some(true),
        ..everything()
    };
    let plan = o.plan(ada, o.org(), &request).await.ok();
    let export = o.made(ada, o.org(), &request).await;
    let held = o.contents(ada, &export).await;
    assert_eq!(
        (plan.counts.files, export.counts.map(|counts| counts.files)),
        (8, Some(8)),
        "two files of the conversation, four of the chat (one forwarded two), two of the document: {:?}",
        plan.counts
    );

    // Every link in every text finds its file, and the files are the bytes that were kept.
    let mut followed: Vec<(String, String)> = Vec::new();
    for (path, bytes) in &held {
        if !path.ends_with(".md") || path == "index.md" {
            continue;
        }
        for link in links_of(&text_of(bytes)) {
            let target = leads_to(path, &link);
            assert!(
                held.contains_key(&target),
                "the link {link} in {path} leads to {target}, which the archive does not hold: {:?}",
                held.keys()
            );
            followed.push((path.clone(), target));
        }
    }
    let by = |tail: &str| -> Vec<&String> {
        followed
            .iter()
            .filter(|(path, _)| path.ends_with(tail))
            .map(|(_, target)| target)
            .collect()
    };
    assert_eq!(
        by("docs/Guide.md").len(),
        2,
        "the file and the picture: {followed:?}"
    );
    assert_eq!(
        by("conversations/Fix login.md").len(),
        2,
        "the file of each of its two messages: {followed:?}"
    );
    let chat_links = by("Fix login/chat.md");
    assert_eq!(
        chat_links.len(),
        4,
        "two files of the message, and the image and file of the one forwarded: {followed:?}"
    );
    assert_eq!(entry(&held, "docs/Guide.files/data.csv"), &sheet[..]);
    assert_eq!(
        entry(&held, "docs/Guide.files/diagram one.png"),
        &picture[..]
    );
    assert_eq!(
        entry(&held, "conversations/Fix login.files/0001/in-1-trace.har"),
        b"GET /login 500\n"
    );
    assert!(has(
        &held,
        "conversations/Fix login.files/0002/in-1-trace.har"
    ));
    // Each carried file keeps its own bytes even when its name matches another's.
    let in_chat = |tail: &str| entry_with(&held, "Fix login/chat.files/", tail);
    assert_eq!(in_chat("-01-会議 notes.txt"), b"the notes of the meeting");
    assert_eq!(in_chat("-02-会議 notes.txt"), b"the corrected notes");
    assert_eq!(in_chat("-01-図表.png"), &picture[..]);
    assert_eq!(in_chat("-02-図表.png"), b"ten lines of a log");
    assert_eq!(
        chat_links
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        4
    );
    assert!(
        !held.keys().any(|path| path.contains("/files/")),
        "nothing lies in a directory that no text names: {:?}",
        held.keys()
    );

    // The same texts, the same files and the same paths in a single export with its files.
    let single = app
        .call(DocumentExportGet, &[&guide.base.id])
        .token(&ada.token)
        .query(&DocumentExportGetQuery {
            format: Some(DocumentExport::Markdown),
            attachments: Some(DocumentExportChoice::Include),
            comments: None,
        })
        .send_bytes()
        .await;
    assert_eq!(single.status, 200);
    assert_eq!(single.headers["content-type"], "application/zip");
    let alone = archive_world::entries(&single);
    assert_eq!(entry(&alone, "Guide.md"), entry(&held, "docs/Guide.md"));
    assert_eq!(entry(&alone, "Guide.files/data.csv"), &sheet[..]);
    assert_eq!(entry(&alone, "Guide.files/diagram one.png"), &picture[..]);
    assert_eq!(alone.len(), 3, "{:?}", alone.keys());
    let chat = app
        .call(ChatExportGet, &[&o.thread_chat])
        .token(&ada.token)
        .query(&ChatExportGetQuery {
            files: Some(ChatExportFiles::Include),
            ..ChatExportGetQuery::default()
        })
        .send_bytes()
        .await;
    assert_eq!(chat.status, 200);
    let alone = archive_world::entries(&chat);
    assert_eq!(entry(&alone, "chat.md"), entry(&held, "Fix login/chat.md"));
    let chat_dir = held
        .keys()
        .find_map(|path| path.strip_suffix("Fix login/chat.md"))
        .map(|head| format!("{head}Fix login/"))
        .unwrap();
    let files: Vec<&String> = alone
        .keys()
        .filter(|path| path.starts_with("chat.files/"))
        .collect();
    assert_eq!(files.len(), 4, "{:?}", alone.keys());
    for name in files {
        assert_eq!(alone[name], held[&format!("{chat_dir}{name}")], "{name}");
    }

    // Asked for without the files, the texts are the same and the files are not there.
    let bare = o.made(ada, o.org(), &everything()).await;
    let without = o.contents(ada, &bare).await;
    assert!(without.keys().all(|path| !path.contains(".files/")));
    assert_eq!(
        entry(&without, "docs/Guide.md"),
        entry(&held, "docs/Guide.md"),
        "and a text names its files the same, there or not"
    );
}

#[tokio::test]
async fn an_archive_whose_place_was_purged_names_no_place_and_is_still_its_persons_to_fetch() {
    let o = Office::new().await;
    let (ada, cai) = (&o.s.ada, &o.s.cai);
    let export = o.made(ada, o.org(), &everything()).await;
    assert_eq!(export.node.as_ref().map(|node| node.id), Some(o.org()));

    // What the reference does when the place is purged: it empties the place of the row and leaves the rest.
    sqlx::query("UPDATE exports SET node_id = NULL WHERE id = $1")
        .bind(export.id)
        .execute(o.s.rig.db.pool())
        .await
        .unwrap();
    let read = o.read(ada, export.id).await.ok();
    assert_eq!(read.state, ExportState::Ready);
    assert!(read.node.is_none(), "{read:?}");
    assert_eq!(read.counts, export.counts);
    let listed = o.list(ada).await;
    assert_eq!(listed.len(), 1);
    assert!(listed[0].node.is_none());

    // Its person fetches it, under a name that has no place in it; nobody else does.
    let fetched = o.fetch(ada, export.id).await;
    assert_eq!(fetched.status, 200);
    let disposition = fetched.headers["content-disposition"].to_str().unwrap();
    assert!(
        disposition.starts_with("attachment; filename=\"export-") && disposition.contains(".zip\""),
        "{disposition}"
    );
    assert!(has(&archive_world::entries(&fetched), "index.md"));
    assert_eq!(o.fetch(cai, export.id).await.status, 404);
    assert_eq!(o.read(cai, export.id).await.problem().status, 404);

    // And it is removed by hand like any other.
    assert_eq!(o.remove(ada, export.id).await.status, 204);
    assert_eq!(
        o.read(ada, export.id).await.ok().state,
        ExportState::Expired
    );
}
