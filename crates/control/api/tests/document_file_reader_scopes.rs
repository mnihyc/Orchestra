//! Actual document files and archive entries stay behind current own-operation authority.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod desk;
mod places;
mod support;

use crate::support::FullPermissionPageReply;
use desk::Desk;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    DocumentDelete, DocumentFile, DocumentFileArchiveEntryGet, DocumentFileArchiveEntryGetQuery,
    DocumentFileArchiveList, DocumentFileArchiveListQuery, DocumentFileCreate,
    DocumentFileCreateQuery, DocumentFileDelete, DocumentFileGet, DocumentFileGetQuery,
};
use orch_api_contract::threads::FILENAME_HEADER;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Hash, Id, Json, new_id};
use serde_json::json;
use std::io::{Cursor, Write};

const CONTENT: &[u8] = b"Current scoped document file bytes.\n";
const ENTRY: &[u8] = b"Stored archive entry content.\n";
// Existing reader-rendezvous allowance; no timing or product authority is inferred from it.
const WAIT: std::time::Duration = std::time::Duration::from_secs(30);

fn archive_bytes() -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("top.txt", options).unwrap();
    zip.write_all(b"Top entry").unwrap();
    zip.add_directory("docs/", options).unwrap();
    zip.start_file("docs/note.txt", options).unwrap();
    zip.write_all(ENTRY).unwrap();
    zip.finish().unwrap().into_inner()
}

struct Scene {
    desk: Desk,
    document: Id,
    plain: DocumentFile,
    archive: DocumentFile,
    zip: Vec<u8>,
}
impl Scene {
    async fn new() -> Self {
        Self::over(Desk::new().await, CONTENT).await
    }
    async fn over(desk: Desk, content: &[u8]) -> Self {
        let document = desk
            .make(&desk.ada, "Document holding actual scoped files")
            .await
            .base
            .id;
        let plain = put(&desk, document, "actual-note.txt", content).await;
        let zip = archive_bytes();
        let archive = put(&desk, document, "actual-bundle.zip", &zip).await;
        Self {
            desk,
            document,
            plain,
            archive,
            zip,
        }
    }
}
async fn put(desk: &Desk, document: Id, name: &str, bytes: &[u8]) -> DocumentFile {
    desk.world
        .app
        .call(DocumentFileCreate, &[&document])
        .token(&desk.ada.token)
        .query(&DocumentFileCreateQuery::default())
        .header(FILENAME_HEADER, name)
        .bytes(bytes)
        .send()
        .await
        .ok()
        .into_full()
        .expect("this native creation control requires its full response")
}
#[derive(Clone, Copy)]
enum Reader {
    File,
    List,
    Entry,
}
const READERS: [Reader; 3] = [Reader::File, Reader::List, Reader::Entry];
impl Reader {
    fn operation(self, inspect: bool) -> String {
        format!(
            "{}.{}",
            match self {
                Self::File => "DocumentFileGet",
                Self::List => "DocumentFileArchiveList",
                Self::Entry => "DocumentFileArchiveEntryGet",
            },
            if inspect { "inspect_deleted" } else { "read" }
        )
    }
    async fn read(self, s: &Scene, token: &str, inspect: bool) -> RawReply {
        match self {
            Self::File => {
                get(
                    s,
                    token,
                    s.document,
                    s.plain.id,
                    &DocumentFileGetQuery {
                        inspect_deleted: inspect.then_some(true),
                        ..Default::default()
                    },
                )
                .await
            }
            Self::List => {
                list(
                    s,
                    token,
                    s.document,
                    s.archive.id,
                    &DocumentFileArchiveListQuery {
                        inspect_deleted: inspect.then_some(true),
                        ..Default::default()
                    },
                )
                .await
            }
            Self::Entry => {
                entry(
                    s,
                    token,
                    s.document,
                    s.archive.id,
                    &DocumentFileArchiveEntryGetQuery {
                        inspect_deleted: inspect.then_some(true),
                        entry: "docs/note.txt".into(),
                    },
                )
                .await
            }
        }
    }
}
async fn get(
    s: &Scene,
    token: &str,
    document: Id,
    file: Id,
    query: &DocumentFileGetQuery,
) -> RawReply {
    s.desk
        .world
        .app
        .call(DocumentFileGet, &[&document, &file])
        .token(token)
        .query(query)
        .send_bytes()
        .await
}
async fn list(
    s: &Scene,
    token: &str,
    document: Id,
    file: Id,
    query: &DocumentFileArchiveListQuery,
) -> RawReply {
    s.desk
        .world
        .app
        .call(DocumentFileArchiveList, &[&document, &file])
        .token(token)
        .query(query)
        .send_bytes()
        .await
}
async fn entry(
    s: &Scene,
    token: &str,
    document: Id,
    file: Id,
    query: &DocumentFileArchiveEntryGetQuery,
) -> RawReply {
    s.desk
        .world
        .app
        .call(DocumentFileArchiveEntryGet, &[&document, &file])
        .token(token)
        .query(query)
        .send_bytes()
        .await
}
struct Key {
    id: Id,
    secret: String,
}
fn rule(operation: &str, id: Id, action: &str) -> Json {
    json!({"target":{"kind":"document","id":id,"reach":"self"},"allow":[{"operation":operation,"actions":[action]}]})
}
async fn key(s: &Scene, user: Id, rules: Option<Vec<Json>>) -> Key {
    let policy = rules.map(|rules| {
        let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
        assert!(scope.validate().is_empty());
        serde_json::to_value(scope).unwrap()
    });
    let scopes = if policy.is_none() {
        vec!["doc.view".to_owned()]
    } else {
        Vec::new()
    };
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *s.desk.world.conn().await,
        &NewApiToken {
            id,
            user_id: user,
            name: "Document File3 control",
            token_hash: &token_hash(&secret),
            scopes: &scopes,
            node_id: None,
            at: s.desk.world.now(),
            expires_at: Some(s.desk.world.now() + chrono::Duration::hours(1)),
        },
        policy.as_ref(),
    )
    .await
    .unwrap();
    Key { id, secret }
}
async fn deny_current(s: &Scene, key: &Key) {
    // Valid legacy→modern shape too: the same credential/person/hash stays in place.
    let changed=sqlx::query("UPDATE api_tokens SET scopes='{}'::text[],operation_scope=$2 WHERE id=$1 AND node_id IS NULL")
        .bind(key.id).bind(serde_json::to_value(KeyScope::default()).unwrap()).execute(&s.desk.world.db.pool).await.unwrap();
    assert_eq!(changed.rows_affected(), 1);
}
fn shown(reply: RawReply) -> RawReply {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    reply
}
fn denied(reply: RawReply) {
    assert_eq!(
        reply.status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
}
fn same_content(expected: &RawReply, actual: &RawReply) {
    assert_eq!(actual.status, expected.status);
    assert_eq!(actual.raw, expected.raw);
    for name in [
        "content-type",
        "content-disposition",
        "content-range",
        "content-length",
        "etag",
        "accept-ranges",
        "cache-control",
        "content-security-policy",
        "x-content-type-options",
    ] {
        assert_eq!(
            actual.headers.get(name),
            expected.headers.get(name),
            "{name}"
        );
    }
}
async fn selected_case(reader: Reader) {
    let s = Scene::new().await;
    let ordinary = shown(reader.read(&s, &s.desk.ada.token, false).await);
    let legacy = key(&s, s.desk.ada.id(), None).await;
    same_content(
        &ordinary,
        &shown(reader.read(&s, &legacy.secret, false).await),
    );
    let selected = key(
        &s,
        s.desk.ada.id(),
        Some(vec![rule(&reader.operation(false), s.document, "read")]),
    )
    .await;
    same_content(
        &ordinary,
        &shown(reader.read(&s, &selected.secret, false).await),
    );
    let other = s.desk.make(&s.desk.ada, "Another file scope").await.base.id;
    for rules in [
        Vec::new(),
        vec![rule("DocumentGet.read", s.document, "read")],
        vec![rule(&reader.operation(false), other, "read")],
        vec![rule(&reader.operation(false), s.document, "write")],
    ] {
        let wrong = key(&s, s.desk.ada.id(), Some(rules)).await;
        denied(reader.read(&s, &wrong.secret, false).await);
    }
    let stranger = s.desk.stranger("file_scope_outsider").await;
    let wrong = key(
        &s,
        stranger.id(),
        Some(vec![rule(&reader.operation(false), s.document, "read")]),
    )
    .await;
    denied(reader.read(&s, &stranger.token, false).await);
    denied(reader.read(&s, &wrong.secret, false).await);
}

#[tokio::test]
async fn file_read_uses_its_own_document_operation_and_ordinary_rights() {
    selected_case(Reader::File).await;
}
#[tokio::test]
async fn archive_list_uses_its_own_document_operation_and_ordinary_rights() {
    selected_case(Reader::List).await;
}
#[tokio::test]
async fn archive_entry_uses_its_own_document_operation_and_ordinary_rights() {
    selected_case(Reader::Entry).await;
}

async fn inspect_case(reader: Reader) {
    let s = Scene::new().await;
    let viewer = s.desk.member("file_retained_viewer", "viewer").await;
    s.desk
        .world
        .app
        .call(DocumentDelete, &[&s.document])
        .token(&s.desk.ada.token)
        .send()
        .await
        .ok();
    let ordinary = shown(reader.read(&s, &s.desk.ada.token, true).await);
    assert_eq!(
        reader
            .read(&s, &s.desk.ada.token, false)
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    let selected = key(
        &s,
        s.desk.ada.id(),
        Some(vec![rule(&reader.operation(true), s.document, "manage")]),
    )
    .await;
    same_content(
        &ordinary,
        &shown(reader.read(&s, &selected.secret, true).await),
    );
    let read = key(
        &s,
        s.desk.ada.id(),
        Some(vec![rule(&reader.operation(false), s.document, "read")]),
    )
    .await;
    let no_authority = key(
        &s,
        viewer.id(),
        Some(vec![rule(&reader.operation(true), s.document, "manage")]),
    )
    .await;
    denied(reader.read(&s, &read.secret, true).await);
    denied(reader.read(&s, &no_authority.secret, true).await);
    assert!(matches!(
        reader
            .read(&s, &selected.secret, false)
            .await
            .problem()
            .code,
        ErrorCode::NotFound | ErrorCode::AuthzDenied
    ));
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT deleted_at IS NOT NULL FROM documents WHERE id=$1")
            .bind(s.document)
            .fetch_one(&s.desk.world.db.pool)
            .await
            .unwrap()
    );
}
#[tokio::test]
async fn file_inspection_keeps_its_exact_manage_selection_and_restore_authority() {
    inspect_case(Reader::File).await;
}
#[tokio::test]
async fn archive_list_inspection_keeps_its_exact_manage_selection_and_restore_authority() {
    inspect_case(Reader::List).await;
}
#[tokio::test]
async fn archive_entry_inspection_keeps_its_exact_manage_selection_and_restore_authority() {
    inspect_case(Reader::Entry).await;
}

#[tokio::test]
async fn file_ranges_conditional_download_and_preview_preserve_the_ordinary_response() {
    let s = Scene::new().await;
    let ordinary = shown(Reader::File.read(&s, &s.desk.ada.token, false).await);
    assert_eq!(ordinary.raw.as_ref(), CONTENT);
    let token = key(
        &s,
        s.desk.ada.id(),
        Some(vec![rule("DocumentFileGet.read", s.document, "read")]),
    )
    .await;
    same_content(
        &ordinary,
        &shown(Reader::File.read(&s, &token.secret, false).await),
    );
    let zipped = shown(
        get(
            &s,
            &s.desk.ada.token,
            s.document,
            s.archive.id,
            &DocumentFileGetQuery::default(),
        )
        .await,
    );
    assert_eq!(zipped.raw.as_ref(), s.zip.as_slice());
    same_content(
        &zipped,
        &shown(
            get(
                &s,
                &token.secret,
                s.document,
                s.archive.id,
                &DocumentFileGetQuery::default(),
            )
            .await,
        ),
    );
    for range in ["bytes=2-9", "bytes=99999-"] {
        let request = |token: String| {
            s.desk
                .world
                .app
                .call(DocumentFileGet, &[&s.document, &s.plain.id])
                .token(&token)
                .header("range", range)
                .send_bytes()
        };
        let ordinary = request(s.desk.ada.token.clone()).await;
        assert_eq!(
            ordinary.status,
            if range == "bytes=2-9" {
                StatusCode::PARTIAL_CONTENT
            } else {
                StatusCode::RANGE_NOT_SATISFIABLE
            }
        );
        same_content(&ordinary, &request(token.secret.clone()).await);
    }
    let etag = ordinary.headers["etag"].to_str().unwrap();
    let ordinary304 = s
        .desk
        .world
        .app
        .call(DocumentFileGet, &[&s.document, &s.plain.id])
        .token(&s.desk.ada.token)
        .header("if-none-match", etag)
        .send_bytes()
        .await;
    assert_eq!(ordinary304.status, StatusCode::NOT_MODIFIED);
    let scoped304 = s
        .desk
        .world
        .app
        .call(DocumentFileGet, &[&s.document, &s.plain.id])
        .token(&token.secret)
        .header("if-none-match", etag)
        .send_bytes()
        .await;
    same_content(&ordinary304, &scoped304);
    for query in [
        DocumentFileGetQuery {
            download: Some(true),
            ..Default::default()
        },
        DocumentFileGetQuery {
            preview: Some(true),
            ..Default::default()
        },
    ] {
        let ordinary = shown(get(&s, &s.desk.ada.token, s.document, s.plain.id, &query).await);
        same_content(
            &ordinary,
            &shown(get(&s, &token.secret, s.document, s.plain.id, &query).await),
        );
    }
}

#[tokio::test]
async fn archive_listing_and_entry_keep_real_content_paths_and_native_errors() {
    let s = Scene::new().await;
    let list_query = DocumentFileArchiveListQuery {
        inside: Some("/docs/".into()),
        limit: Some(1),
        ..Default::default()
    };
    let ordinary = shown(list(&s, &s.desk.ada.token, s.document, s.archive.id, &list_query).await);
    let page: Json = serde_json::from_slice(&ordinary.raw).unwrap();
    assert!(page.to_string().contains("note.txt"));
    let token = key(
        &s,
        s.desk.ada.id(),
        Some(vec![
            rule("DocumentFileArchiveList.read", s.document, "read"),
            rule("DocumentFileArchiveEntryGet.read", s.document, "read"),
        ]),
    )
    .await;
    same_content(
        &ordinary,
        &shown(list(&s, &token.secret, s.document, s.archive.id, &list_query).await),
    );
    let bytes = shown(Reader::Entry.read(&s, &s.desk.ada.token, false).await);
    assert_eq!(bytes.raw.as_ref(), ENTRY);
    same_content(
        &bytes,
        &shown(Reader::Entry.read(&s, &token.secret, false).await),
    );
    for path in ["", "../note.txt", "docs//note.txt", "missing.txt"] {
        let query = DocumentFileArchiveEntryGetQuery {
            entry: path.into(),
            inspect_deleted: None,
        };
        let ordinary = entry(&s, &s.desk.ada.token, s.document, s.archive.id, &query).await;
        assert!(!ordinary.status.is_success());
        let scoped = entry(&s, &token.secret, s.document, s.archive.id, &query).await;
        assert_eq!(scoped.status, ordinary.status);
        assert_eq!(scoped.problem().code, ordinary.problem().code);
    }
    let query = DocumentFileArchiveListQuery {
        limit: Some(0),
        ..Default::default()
    };
    assert_eq!(
        list(&s, &s.desk.ada.token, s.document, s.archive.id, &query)
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    assert_eq!(
        list(&s, &token.secret, s.document, s.archive.id, &query)
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    let ordinary = list(
        &s,
        &s.desk.ada.token,
        s.document,
        s.plain.id,
        &DocumentFileArchiveListQuery::default(),
    )
    .await;
    let scoped = list(
        &s,
        &token.secret,
        s.document,
        s.plain.id,
        &DocumentFileArchiveListQuery::default(),
    )
    .await;
    assert_eq!(ordinary.problem().code, ErrorCode::CapabilityUnsupported);
    assert_eq!(scoped.problem().code, ordinary.problem().code);
}

#[tokio::test]
async fn a_readable_second_document_cannot_borrow_the_first_documents_file_identity() {
    let s = Scene::new().await;
    let other = s
        .desk
        .make(&s.desk.ada, "Readable wrong document")
        .await
        .base
        .id;
    for reader in READERS {
        shown(reader.read(&s, &s.desk.ada.token, false).await);
        let token = key(
            &s,
            s.desk.ada.id(),
            Some(vec![
                rule(&reader.operation(false), s.document, "read"),
                rule(&reader.operation(false), other, "read"),
            ]),
        )
        .await;
        shown(reader.read(&s, &token.secret, false).await);
        for bearer in [&s.desk.ada.token, &token.secret] {
            let wrong = match reader {
                Reader::File => {
                    get(
                        &s,
                        bearer,
                        other,
                        s.plain.id,
                        &DocumentFileGetQuery::default(),
                    )
                    .await
                }
                Reader::List => {
                    list(
                        &s,
                        bearer,
                        other,
                        s.archive.id,
                        &DocumentFileArchiveListQuery::default(),
                    )
                    .await
                }
                Reader::Entry => {
                    entry(
                        &s,
                        bearer,
                        other,
                        s.archive.id,
                        &DocumentFileArchiveEntryGetQuery {
                            entry: "docs/note.txt".into(),
                            inspect_deleted: None,
                        },
                    )
                    .await
                }
            };
            assert_eq!(wrong.problem().code, ErrorCode::NotFound);
        }
    }
}

fn rebind_blobs(desk: &mut Desk, blobs: orch_api_core::blobs::Blobs) {
    use orch_api_core::{registry::Registry, state::AppState, testing::TestApp};
    use std::sync::Arc;
    let old = desk.world.app.app.clone();
    old.engine.attach_blobs(blobs.store());
    let state = Arc::new(AppState {
        host_ports: old.host_ports.clone(),
        builtin_sources: old.builtin_sources.clone(),
        db: old.db.clone(),
        config: old.config.clone(),
        clock: old.clock.clone(),
        link: old.link.clone(),
        engine: old.engine.clone(),
        realtime: old.realtime.clone(),
        build: old.build,
        blobs,
        cache: old.cache.clone(),
    });
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    desk.world.app = TestApp::over(state, desk.world.app.clock.clone(), registry);
}

async fn disk_scene() -> (Scene, tempfile::TempDir, Vec<u8>) {
    use orch_blob_local::{LocalBlobStore, StoreOptions};
    let mut desk = Desk::new().await;
    let base = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../.tmp/document-file-readers")
        });
    std::fs::create_dir_all(&base).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("document-file-stream-")
        .tempdir_in(base)
        .unwrap();
    let store = LocalBlobStore::open(directory.path().to_path_buf())
        .await
        .unwrap();
    rebind_blobs(
        &mut desk,
        orch_api_core::blobs::Blobs::new(std::sync::Arc::new(store), directory.path()),
    );
    // Three native local-store chunks plus a tail prove that the first frame is not the whole file.
    let chunk = StoreOptions::default().read_chunk_bytes;
    assert!(chunk > 0);
    let bytes = vec![b'Z'; 3 * chunk + 17];
    (Scene::over(desk, &bytes).await, directory, bytes)
}

async fn future_file_chunks_follow_current_credential(revoke: bool) {
    use http_body_util::BodyExt;
    let (s, _directory, bytes) = disk_scene().await;
    let ordinary = shown(Reader::File.read(&s, &s.desk.ada.token, false).await);
    assert_eq!(ordinary.raw.as_ref(), bytes.as_slice());
    let token = key(&s, s.desk.ada.id(), None).await;
    let response = s
        .desk
        .world
        .app
        .call(DocumentFileGet, &[&s.document, &s.plain.id])
        .token(&token.secret)
        .send_response()
        .await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a valid real legacy read must be established first"
    );
    let mut body = response.into_body();
    let frame = tokio::time::timeout(WAIT, body.frame())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let first = frame
        .into_data()
        .expect("the first native blob frame has bytes");
    assert!(!first.is_empty() && first.len() < bytes.len());
    assert_eq!(first.as_ref(), &bytes[..first.len()]);
    if revoke {
        identity::revoke_api_token(
            &mut *s.desk.world.conn().await,
            token.id,
            s.desk.ada.id(),
            s.desk.world.now(),
        )
        .await
        .unwrap();
    } else {
        deny_current(&s, &token).await;
    }
    let next = tokio::time::timeout(WAIT, body.frame()).await.unwrap();
    assert!(
        matches!(next, Some(Err(_))),
        "a later actual file frame must fail after current credential loss, without exposing another byte"
    );
}

#[tokio::test]
async fn a_revoked_actual_legacy_file_stream_exposes_no_later_native_blob_frame() {
    future_file_chunks_follow_current_credential(true).await;
}
#[tokio::test]
async fn a_real_file_stream_intersects_its_accepted_legacy_ceiling_with_current_modern_rules() {
    future_file_chunks_follow_current_credential(false).await;
}

#[tokio::test]
async fn a_stream_stops_after_real_ordinary_document_view_is_removed() {
    use http_body_util::BodyExt;
    let (s, _directory, bytes) = disk_scene().await;
    let viewer = s.desk.member("stream_file_viewer", "viewer").await;
    let ordinary = shown(Reader::File.read(&s, &viewer.token, false).await);
    assert_eq!(ordinary.raw.as_ref(), bytes.as_slice());
    let token = key(&s, viewer.id(), None).await;
    let response = s
        .desk
        .world
        .app
        .call(DocumentFileGet, &[&s.document, &s.plain.id])
        .token(&token.secret)
        .send_response()
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    let first = tokio::time::timeout(WAIT, body.frame())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_data()
        .unwrap();
    assert!(!first.is_empty() && first.len() < bytes.len());
    assert_eq!(first.as_ref(), &bytes[..first.len()]);
    s.desk
        .world
        .app
        .call(
            orch_api_contract::access::PermissionPageUpdate,
            &[&"documents", &s.document],
        )
        .token(&s.desk.ada.token)
        .raw_body(&json!({"whole":"private"}))
        .send()
        .await
        .full_permission_page()
        .ok();
    denied(Reader::File.read(&s, &viewer.token, false).await);
    denied(Reader::File.read(&s, &token.secret, false).await);
    let next = tokio::time::timeout(WAIT, body.frame()).await.unwrap();
    assert!(
        matches!(next, Some(Err(_))),
        "an old response must not expose another native chunk after actual ordinary DocView is gone"
    );
}

#[tokio::test]
async fn an_authorized_open_file_keeps_its_immutable_bytes_when_only_its_file_row_changes() {
    use http_body_util::BodyExt;
    for replace in [false, true] {
        let (s, _directory, bytes) = disk_scene().await;
        let token = key(&s, s.desk.ada.id(), None).await;
        let response = s
            .desk
            .world
            .app
            .call(DocumentFileGet, &[&s.document, &s.plain.id])
            .token(&token.secret)
            .send_response()
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let old_etag = response.headers()["etag"].clone();
        let mut body = response.into_body();
        let first = tokio::time::timeout(WAIT, body.frame())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .into_data()
            .unwrap();
        assert!(!first.is_empty() && first.len() < bytes.len());
        let mut delivered = first.to_vec();
        if replace {
            let changed = s
                .desk
                .world
                .app
                .call(DocumentFileCreate, &[&s.document])
                .token(&s.desk.ada.token)
                .query(&DocumentFileCreateQuery {
                    overwrite: Some(true),
                })
                .header(FILENAME_HEADER, "actual-note.txt")
                .bytes(b"New replacement bytes")
                .send()
                .await
                .ok()
                .into_full()
                .expect("this native creation control requires its full response");
            assert_eq!(changed.id, s.plain.id);
            let fresh = shown(Reader::File.read(&s, &token.secret, false).await);
            assert_eq!(fresh.raw.as_ref(), b"New replacement bytes");
            assert_ne!(fresh.headers["etag"], old_etag);
        } else {
            s.desk
                .world
                .app
                .call(DocumentFileDelete, &[&s.document, &s.plain.id])
                .token(&s.desk.ada.token)
                .send()
                .await
                .ok();
            assert_eq!(
                Reader::File
                    .read(&s, &token.secret, false)
                    .await
                    .problem()
                    .code,
                ErrorCode::NotFound
            );
        }
        while let Some(frame) = tokio::time::timeout(WAIT, body.frame()).await.unwrap() {
            delivered.extend_from_slice(&frame.unwrap().into_data().unwrap());
        }
        assert_eq!(
            delivered, bytes,
            "the open snapshot neither switches to newer bytes nor acquires a new file-row liveness gate"
        );
    }
}

async fn before_first_frame(reader: Reader) {
    use http_body_util::BodyExt;
    let s = Scene::new().await;
    shown(reader.read(&s, &s.desk.ada.token, false).await);
    let token = key(&s, s.desk.ada.id(), None).await;
    let response = match reader {
        Reader::File => {
            s.desk
                .world
                .app
                .call(DocumentFileGet, &[&s.document, &s.plain.id])
                .token(&token.secret)
                .send_response()
                .await
        }
        Reader::Entry => {
            s.desk
                .world
                .app
                .call(DocumentFileArchiveEntryGet, &[&s.document, &s.archive.id])
                .token(&token.secret)
                .query(&DocumentFileArchiveEntryGetQuery {
                    entry: "docs/note.txt".into(),
                    inspect_deleted: None,
                })
                .send_response()
                .await
        }
        Reader::List => panic!("the archive listing is an ordinary JSON reply"),
    };
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the actual legacy raw response must exist before its credential changes"
    );
    deny_current(&s, &token).await;
    let first = tokio::time::timeout(WAIT, response.into_body().frame())
        .await
        .unwrap();
    assert!(
        matches!(first, Some(Err(_))),
        "current denial before body polling must expose no first body frame"
    );
}
#[tokio::test]
async fn a_file_response_rechecks_current_scope_between_valid_headers_and_its_first_frame() {
    before_first_frame(Reader::File).await;
}
#[tokio::test]
async fn an_archive_entry_rechecks_current_scope_between_valid_headers_and_its_single_frame() {
    before_first_frame(Reader::Entry).await;
}

use orch_blob_contract::range::ByteRange;
use orch_blob_contract::store::{BlobError, BlobRead, BlobStore, ByteStream, Stored};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Notify;

struct OpenGate {
    inner: Arc<dyn BlobStore>,
    hash: Hash,
    used: AtomicBool,
    reached: Notify,
    release: Notify,
}
#[async_trait::async_trait]
impl BlobStore for OpenGate {
    async fn put(
        &self,
        hash: &Hash,
        size: Option<i64>,
        body: ByteStream,
    ) -> Result<Stored, BlobError> {
        self.inner.put(hash, size, body).await
    }
    async fn get(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError> {
        let opened = self.inner.get(hash, range).await?;
        if *hash == self.hash && !self.used.swap(true, Ordering::SeqCst) {
            assert_eq!(opened.meta.hash, self.hash);
            assert!(opened.meta.size > 0);
            self.reached.notify_one();
            self.release.notified().await;
        }
        Ok(opened)
    }
    async fn exists(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.inner.exists(hash).await
    }
    async fn size(&self, hash: &Hash) -> Result<Option<i64>, BlobError> {
        self.inner.size(hash).await
    }
    async fn delete(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.inner.delete(hash).await
    }
}

#[tokio::test]
async fn the_same_real_key_is_rechecked_after_immutable_blob_open_before_any_response_metadata() {
    let mut s = Scene::new().await;
    let ordinary = shown(Reader::File.read(&s, &s.desk.ada.token, false).await);
    assert_eq!(ordinary.raw.as_ref(), CONTENT);
    let token = key(&s, s.desk.ada.id(), None).await;
    let old = s.desk.world.app.app.clone();
    let gate = Arc::new(OpenGate {
        inner: old.blobs.store(),
        hash: Hash::of(CONTENT),
        used: AtomicBool::new(false),
        reached: Notify::new(),
        release: Notify::new(),
    });
    rebind_blobs(
        &mut s.desk,
        orch_api_core::blobs::Blobs::new(
            gate.clone(),
            std::path::Path::new(&old.config.storage.root),
        ),
    );
    let request = async { Reader::File.read(&s, &token.secret, false).await };
    let change = async {
        tokio::time::timeout(WAIT, gate.reached.notified())
            .await
            .expect("the real selected blob must have been opened");
        deny_current(&s, &token).await;
        gate.release.notify_one();
    };
    let (reply, ()) = tokio::time::timeout(WAIT, async { tokio::join!(request, change) })
        .await
        .unwrap();
    assert_eq!(
        reply.status,
        StatusCode::FORBIDDEN,
        "a real opened legacy response must not survive the same key's current modern denial"
    );
    assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
    assert!(
        !reply
            .raw
            .windows(CONTENT.len())
            .any(|bytes| bytes == CONTENT)
    );
    for name in [
        "etag",
        "content-disposition",
        "content-range",
        "accept-ranges",
    ] {
        assert!(
            !reply.headers.contains_key(name),
            "old raw metadata escaped in {name}"
        );
    }
}

#[tokio::test]
async fn retained_parent_file_reads_keep_the_current_inspection_tuple_and_do_not_restore() {
    let s = Scene::new().await;
    s.desk
        .world
        .app
        .call(orch_api_contract::ownership::NodeDelete, &[&s.desk.project])
        .token(&s.desk.ada.token)
        .raw_body(&json!({}))
        .send()
        .await
        .ok();
    let before: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT deleted_at FROM nodes WHERE id=$1")
            .bind(s.desk.project)
            .fetch_one(&s.desk.world.db.pool)
            .await
            .unwrap();
    assert!(before.is_some());
    for reader in READERS {
        let ordinary = shown(reader.read(&s, &s.desk.ada.token, true).await);
        let token = key(
            &s,
            s.desk.ada.id(),
            Some(vec![rule(&reader.operation(true), s.document, "manage")]),
        )
        .await;
        same_content(
            &ordinary,
            &shown(reader.read(&s, &token.secret, true).await),
        );
        assert!(matches!(
            reader.read(&s, &token.secret, false).await.problem().code,
            ErrorCode::AuthzDenied | ErrorCode::NotFound
        ));
    }
    let after: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT deleted_at FROM nodes WHERE id=$1")
            .bind(s.desk.project)
            .fetch_one(&s.desk.world.db.pool)
            .await
            .unwrap();
    assert_eq!(after, before);
}
