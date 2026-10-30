//! Named file changes use the real history routes, with the same person or thread that stored them.

use crate::{desk::Desk, rig};
#[path = "../rig_tool_source/mod.rs"]
mod tool_source;
use bytes::Bytes;
use orch_agent_contract::events::OrchTool;
use orch_api_contract::documents::{
    Author, DocumentFileChangeKind, DocumentFileCreate, DocumentFileCreateQuery,
    DocumentFileDelete, DocumentFileGet, DocumentFileGetQuery, DocumentGet, DocumentVersionGet,
    DocumentVersionList, DocumentVersionListQuery, NodeDocumentCreate, VersionCause,
};
use orch_api_contract::workspace::{DirEntry, DirEntryKind, FileStat};
use orch_blob_contract::store::stream_of;
use orch_db::queries::{access, documents};
use orch_engine::tools::CallContext;
use orch_link_contract::commands::{Command, CommandKind, CommandResult, FsFetchResult};
use orch_types::{Hash, Time, new_id};
use serde_json::json;

#[tokio::test]
async fn a_file_from_the_agents_workspace_is_downloaded_whole_and_its_history_is_filtered_by_thread()
 {
    let s = rig::Scene::new().await;
    let thread = s.thread().await.base.id;
    let document = s
        .rig
        .world
        .app
        .call(NodeDocumentCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&crate::desk::body("Results"))
        .send()
        .await
        .ok()
        .base
        .id;
    let now = s.rig.world.now();
    {
        let mut conn = s.rig.db.acquire().await.unwrap();
        sqlx::query("UPDATE agents SET descriptor=jsonb_set(descriptor,'{capabilities,orch_tools}','\"mcp\"') WHERE id=$1")
            .bind(s.on.agent).execute(&mut *conn).await.unwrap();
        for permission in ["doc.view", "doc.edit"] {
            access::testing::add_entry(
                &mut conn,
                &access::PageKey::new("document", document, "live"),
                "any",
                permission,
                ("agents", Some(thread), None),
                None,
                now,
            )
            .await
            .unwrap();
        }
        access::bump_epoch(&mut conn).await.unwrap();
    }
    let turn = tool_source::active(&s, thread)
        .await
        .expect("the actual attachment source turn");
    let bytes = Bytes::from_static(b"whole native result\0\xff\n");
    let hash = Hash::of(&bytes);
    let size = i64::try_from(bytes.len()).unwrap();
    let store = s.rig.world.app.app.blobs.store();
    store
        .put(&hash, Some(size), stream_of(vec![bytes.clone()]))
        .await
        .unwrap();
    s.rig.engine.attach_blobs(store);
    let environment = s.on.environment;
    let mtime = Time::new(now);
    let mtime_ns = now.timestamp_nanos_opt().unwrap();
    s.rig
        .machine
        .handler
        .on(CommandKind::FsStat, move |command| {
            let Command::FsStat(body) = command else {
                panic!("a file stat");
            };
            assert_eq!(body.environment_id, environment);
            assert_eq!(body.path, "web/out/result.bin");
            Ok(CommandResult::FsStat(FileStat {
                entry: DirEntry {
                    name: "result.bin".to_owned(),
                    path: body.path.clone(),
                    kind: DirEntryKind::File,
                    size,
                    mtime,
                    mode: 0o600,
                    link_target: None,
                    cached: false,
                },
                hash: Some(hash),
                media_type: "application/octet-stream".to_owned(),
                is_text: false,
            }))
        });
    s.rig
        .machine
        .handler
        .on(CommandKind::FsFetch, move |command| {
            let Command::FsFetch(body) = command else {
                panic!("a capped whole-file read");
            };
            assert_eq!(body.path, "web/out/result.bin");
            assert!(body.max_bytes.is_some_and(|limit| size <= limit));
            Ok(CommandResult::FsFetch(FsFetchResult {
                hash,
                size,
                mtime_ns,
            }))
        });
    let call_ref = new_id().to_string();
    tokio::time::timeout(rig::PATIENCE, s.rig.engine.answer_tool_call(&CallContext {
        machine: s.rig.machine_id, thread, turn, call_ref: call_ref.clone(), attempt: None,
        tool: OrchTool::EditDocument, arguments: json!({ "command": "attach", "path": format!("id:{document}"), "file": "out/result.bin" }),
    })).await.unwrap().unwrap();
    rig::eventually("the attachment tool result", || async {
        s.rig.seen.of_kind(CommandKind::OrchToolResult).iter().any(|command| matches!(command,
            Command::OrchToolResult(result) if result.call_ref == call_ref && result.error.is_none()))
    }).await;
    let actor:(Option<orch_types::Id>,Option<orch_types::Id>)=sqlx::query_as(
        "SELECT added_by,added_by_thread FROM document_files WHERE document_id=$1 AND name='result.bin'")
        .bind(document).fetch_one(s.rig.db.pool()).await.unwrap();
    assert_eq!(
        actor,
        (None, Some(thread)),
        "the real native attach stores its agent thread, never its accountable owner as a person"
    );
    let doc = s
        .rig
        .world
        .app
        .call(DocumentGet, &[&document])
        .token(&s.cai.token)
        .send()
        .await
        .ok();
    assert_eq!(doc.files.len(), 1);
    let file = &doc.files[0];
    assert_eq!(file.name, "result.bin");
    assert!(matches!(&file.added_by, Some(Author::Agent(by)) if by.thread.id == thread));
    let downloaded = s
        .rig
        .world
        .app
        .call(DocumentFileGet, &[&document, &file.id])
        .token(&s.cai.token)
        .query(&DocumentFileGetQuery::default())
        .send_bytes()
        .await;
    assert_eq!(downloaded.raw, bytes);
    let filter = DocumentVersionListQuery {
        author: Some(format!("thread:{thread}")),
        ..Default::default()
    };
    let open = s
        .rig
        .world
        .app
        .call(DocumentVersionList, &[&document])
        .token(&s.cai.token)
        .query(&filter)
        .send()
        .await
        .ok();
    assert_eq!(open.items.len(), 1);
    assert_eq!(open.items[0].version, None);
    assert_eq!(open.items[0].changed.files.len(), 1);
    assert_eq!(open.items[0].changed.files[0].name, "result.bin");
    assert_eq!(
        open.items[0].changed.files[0].kind,
        DocumentFileChangeKind::Added
    );
    assert!(matches!(&open.items[0].authors[..], [Author::Agent(by)] if by.thread.id == thread));
    let number = {
        let mut tx = s.rig.db.begin().await.unwrap();
        let row = documents::lock(&mut tx, document).await.unwrap().unwrap();
        let number = documents::history::cut(&mut tx, &row, VersionCause::Idle, now)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        number
    };
    let closed = s
        .rig
        .world
        .app
        .call(DocumentVersionList, &[&document])
        .token(&s.cai.token)
        .query(&filter)
        .send()
        .await
        .ok();
    assert_eq!(closed.items.len(), 1);
    assert_eq!(closed.items[0].version, Some(i64::from(number)));
    assert_eq!(closed.items[0].changed, open.items[0].changed);
    assert_eq!(closed.items[0].authors, open.items[0].authors);
    let single = s
        .rig
        .world
        .app
        .call(DocumentVersionGet, &[&document, &number])
        .token(&s.cai.token)
        .send()
        .await
        .ok();
    assert_eq!(single.version.changed.files, open.items[0].changed.files);
    assert!(
        single.blocks.is_empty(),
        "file-only history invents no text blocks"
    );
    let after = s
        .rig
        .world
        .app
        .call(DocumentGet, &[&document])
        .token(&s.cai.token)
        .send()
        .await
        .ok();
    assert_eq!(
        after.files, doc.files,
        "cutting named history cannot erase or reinterpret the current file's actual writer"
    );
}

#[tokio::test]
async fn a_persons_upload_replacement_and_removal_use_the_same_named_history_without_invented_blocks()
 {
    let d = Desk::new().await;
    let doc = d.make(&d.ada, "Results").await.base.id;
    let first = d
        .world
        .app
        .call(DocumentFileCreate, &[&doc])
        .token(&d.ada.token)
        .query(&DocumentFileCreateQuery::default())
        .header("x-orch-filename", "report.csv")
        .bytes(b"a,b\n1,2\n")
        .send()
        .await
        .ok();
    let replacement = d
        .world
        .app
        .call(DocumentFileCreate, &[&doc])
        .token(&d.ada.token)
        .query(&DocumentFileCreateQuery {
            overwrite: Some(true),
        })
        .header("x-orch-filename", "report.csv")
        .bytes(b"a,b\n3,4\n")
        .send()
        .await
        .ok();
    assert_eq!(replacement.id, first.id);
    let first_span = d
        .world
        .app
        .call(DocumentVersionList, &[&doc])
        .token(&d.ada.token)
        .query(&DocumentVersionListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(first_span.items.len(), 1);
    assert_eq!(
        first_span.items[0]
            .changed
            .files
            .iter()
            .map(|file| file.kind)
            .collect::<Vec<_>>(),
        [
            DocumentFileChangeKind::Added,
            DocumentFileChangeKind::Replaced
        ]
    );
    d.world
        .advance(chrono::Duration::seconds(super::WINDOW_SECS + 1));
    d.world
        .app
        .call(DocumentFileDelete, &[&doc, &first.id])
        .token(&d.ada.token)
        .send()
        .await
        .ok();
    let history = d
        .world
        .app
        .call(DocumentVersionList, &[&doc])
        .token(&d.ada.token)
        .query(&DocumentVersionListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(history.items.len(), 2);
    assert_eq!(history.items[0].version, None);
    assert_eq!(
        history.items[0].changed.files[0].kind,
        DocumentFileChangeKind::Removed
    );
    assert_eq!(history.items[0].changed.files[0].name, "report.csv");
    assert_eq!(history.items[1].cause, Some(VersionCause::Window));
    assert_eq!(history.items[1].changed, first_span.items[0].changed);
    for entry in history.items {
        assert_eq!(
            (
                entry.changed.inserted,
                entry.changed.updated,
                entry.changed.removed,
                entry.block_count
            ),
            (0, 0, 0, 0)
        );
        assert!(matches!(&entry.authors[..], [Author::User(by)] if by.id == d.ada.person.id));
    }
}
