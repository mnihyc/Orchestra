//! Importing over HTTP: an Orch package that an export wrote is planned (nothing is made), brought as a job, found again by a later import of the same things, replaced in place when its source changed, taken back as a whole, and refused where the person may not or the limits say no; the report says what became of each object.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod archive_world;
mod places;
mod rig;
mod support;

use archive_world::{Office, entry, everything, kept};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::common::PageQuery;
use orch_api_contract::documents::{
    DocumentDelete, DocumentGet, DocumentUpdate, DocumentUpdateBody, DocumentVersionList,
    DocumentVersionListQuery, NewDocumentAccess, NodeDocumentCreate, NodeDocumentCreateBody,
};
use orch_api_contract::imports::{
    Import, ImportAction, ImportGet, ImportOptions, ImportOutcome, ImportPersonChoice,
    ImportPersonMatch, ImportPlan, ImportPreflightRequest, ImportReportList, ImportReportListQuery,
    ImportReportRow, ImportRequest, ImportSource, ImportSourceFile, ImportSourceMode, ImportState,
    ImportUndo, ImportUndoBody, ImportUndoPreflight, ImportView, MeImportList, NodeImportCreate,
    NodeImportPreflight,
};
use orch_api_contract::nodes::JobState;
use orch_api_contract::package::{PACKAGE_FORMAT, PACKAGE_MANIFEST, PackageKind, PackageManifest};
use orch_types::{Id, new_id};
use places::Who;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Write;

/// The zip an export of `place` as an Orch package writes, as its person fetched it.
async fn package_of(o: &Office, place: Id) -> Vec<u8> {
    let mut request = everything();
    request.package = Some(true);
    let export = o.made(&o.s.ada, place, &request).await;
    let fetched = o.fetch(&o.s.ada, export.id).await;
    assert_eq!(fetched.status, 200);
    fetched.raw.to_vec()
}

/// What an upload of `bytes` by `by` is, as a request names it.
async fn upload(o: &Office, by: &Who, name: &str, bytes: &[u8]) -> ImportSourceFile {
    let (hash, _) = kept(&o.s, bytes).await;
    let mut conn = o.s.rig.db.acquire().await.unwrap();
    orch_db::queries::uploads::remember(&mut conn, by.id(), &hash, o.s.rig.world.now())
        .await
        .unwrap();
    ImportSourceFile {
        name: name.to_owned(),
        blob: hash,
    }
}

fn asked(client: &str, file: &ImportSourceFile) -> ImportRequest {
    ImportRequest {
        client_id: client.to_owned(),
        source: ImportSource {
            files: vec![file.clone()],
            mode: None,
        },
        destination: Default::default(),
        options: ImportOptions::default(),
        choices: Default::default(),
    }
}

async fn plan(
    o: &Office,
    by: &Who,
    place: Id,
    request: &ImportRequest,
) -> orch_api_core::testing::Reply<ImportPlan> {
    o.s.rig
        .world
        .app
        .call(NodeImportPreflight, &[&place])
        .token(&by.token)
        .body(&ImportPreflightRequest {
            request: request.clone(),
            view: ImportView::default(),
        })
        .send()
        .await
}

/// Asks for the import, waits for its job and reads the import as it ended.
async fn imported(o: &Office, by: &Who, place: Id, request: &ImportRequest) -> Import {
    let asked =
        o.s.rig
            .world
            .app
            .call(NodeImportCreate, &[&place])
            .token(&by.token)
            .body(request)
            .send()
            .await;
    assert_eq!(asked.status, 202, "{:?}", asked.body);
    let job = o.ended(by, asked.ok().id).await;
    assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);
    let id: Id = job.result.as_ref().unwrap()["import_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    read(o, by, id).await
}

async fn read(o: &Office, by: &Who, import: Id) -> Import {
    o.s.rig
        .world
        .app
        .call(ImportGet, &[&import])
        .token(&by.token)
        .send()
        .await
        .ok()
}

async fn report(o: &Office, by: &Who, import: Id) -> Vec<ImportReportRow> {
    o.s.rig
        .world
        .app
        .call(ImportReportList, &[&import])
        .token(&by.token)
        .query(&ImportReportListQuery {
            limit: Some(200),
            ..Default::default()
        })
        .send()
        .await
        .ok()
        .items
}

fn of_kind(rows: &[ImportReportRow], kind: PackageKind) -> Vec<&ImportReportRow> {
    rows.iter().filter(|row| row.kind == kind).collect()
}

#[tokio::test]
async fn a_chat_import_cannot_claim_an_unrelated_blob_by_its_hash() {
    use orch_api_contract::chats::{
        ChatAttachmentCreate, ChatBlobGet, ChatMessageCreate, ChatMessageCreateBody,
        ChatMessageList, ChatPart,
    };
    use orch_api_contract::imports::ImportRoot;
    use orch_api_contract::threads::{FILENAME_HEADER, UploadBlobGet, UploadBlobGetQuery};

    let o = Office::new().await;
    let app = &o.s.rig.world.app;
    let ada = &o.s.ada;
    let destination = places::make_topic(
        &o.s.rig.world,
        ada,
        o.s.tree.project.base.id,
        "Imported chat",
    )
    .await;
    let target_chat = destination.base.chat_id;

    let staged = app
        .call(ChatAttachmentCreate, &[&o.topic_chat])
        .token(&ada.token)
        .header(FILENAME_HEADER, "included.txt")
        .bytes(b"included in the package")
        .send()
        .await
        .ok();
    app.call(ChatMessageCreate, &[&o.topic_chat])
        .token(&ada.token)
        .body(&ChatMessageCreateBody {
            parts: vec![ChatPart::Attachment {
                blob: staged.blob,
                name: staged.name.clone(),
                size: staged.size,
                media_type: staged.media_type.clone(),
            }],
            reply_to: None,
            agent_review: None,
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    let existing = app
        .call(ChatAttachmentCreate, &[&target_chat])
        .token(&ada.token)
        .header(FILENAME_HEADER, "already-here.txt")
        .bytes(b"already in the destination")
        .send()
        .await
        .ok();
    app.call(ChatMessageCreate, &[&target_chat])
        .token(&ada.token)
        .body(&ChatMessageCreateBody {
            parts: vec![ChatPart::Attachment {
                blob: existing.blob,
                name: existing.name.clone(),
                size: existing.size,
                media_type: existing.media_type.clone(),
            }],
            reply_to: None,
            agent_review: None,
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();

    let (allowed, allowed_size) = kept(&o.s, b"an independently authorized upload").await;
    let (secret, secret_size) = kept(&o.s, b"another person's private file").await;
    let mut conn = o.s.rig.db.acquire().await.unwrap();
    orch_db::queries::uploads::remember(&mut conn, ada.id(), &allowed, o.s.rig.world.now())
        .await
        .unwrap();
    orch_db::queries::uploads::remember(&mut conn, o.s.bea.id(), &secret, o.s.rig.world.now())
        .await
        .unwrap();
    drop(conn);
    let before = app
        .call(UploadBlobGet, &[&secret])
        .token(&ada.token)
        .query(&UploadBlobGetQuery {
            name: "private.txt".into(),
            ..Default::default()
        })
        .send_bytes()
        .await;
    assert_eq!(before.status, 404);

    let mut export_request = everything();
    export_request.package = Some(true);
    let export = o.made(ada, o.topic(), &export_request).await;
    let mut entries = o.contents(ada, &export).await;
    let mut manifest: PackageManifest = serde_json::from_slice(&entries[PACKAGE_MANIFEST]).unwrap();
    let chat = manifest
        .objects
        .iter_mut()
        .find(|object| object.kind == PackageKind::Chat && object.id == o.topic_chat)
        .unwrap();
    let path = chat.path.as_ref().unwrap().clone();
    let mut messages: Vec<Value> = serde_json::from_slice(&entries[&path]).unwrap();
    let message = messages
        .iter_mut()
        .find(|message| {
            message["parts"]
                .as_array()
                .is_some_and(|parts| parts.iter().any(|part| part["blob"] == json!(staged.blob)))
        })
        .unwrap();
    let parts = message["parts"].as_array_mut().unwrap();
    parts.push(
        json!({"type":"attachment", "blob":existing.blob, "name":existing.name,
        "size":existing.size, "media_type":existing.media_type}),
    );
    parts.push(
        json!({"type":"attachment", "blob":allowed, "name":"authorized.txt",
        "size":allowed_size, "media_type":"text/plain"}),
    );
    parts.push(
        json!({"type":"attachment", "blob":secret, "name":"private.txt",
        "size":secret_size, "media_type":"text/plain"}),
    );
    let record = serde_json::to_vec(&messages).unwrap();
    chat.bytes = i64::try_from(record.len()).unwrap();
    chat.sha256 = Some(hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&record)));
    entries.insert(path, record);
    entries.insert(
        PACKAGE_MANIFEST.to_owned(),
        serde_json::to_vec(&manifest).unwrap(),
    );
    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        archive
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(&bytes).unwrap();
    }
    let bytes = archive.finish().unwrap().into_inner();
    let file = upload(&o, ada, "forged.zip", &bytes).await;
    let mut request = asked("chat-file-authority", &file);
    request.destination.root = Some(ImportRoot::Here);
    let done = imported(&o, ada, destination.base.id, &request).await;
    assert_eq!(done.state, ImportState::Done, "{:?}", done.failure);

    let messages = app
        .call(ChatMessageList, &[&target_chat])
        .token(&ada.token)
        .send()
        .await
        .ok()
        .messages;
    let imported = messages
        .iter()
        .find(|message| {
            message.parts.iter().any(
                |part| matches!(part, ChatPart::Attachment { blob, .. } if *blob == staged.blob),
            )
        })
        .unwrap();
    let carried = orch_chat::attachments::blobs_in(&imported.parts);
    assert!(
        carried.contains(&staged.blob),
        "the package's file is brought"
    );
    assert!(
        carried.contains(&existing.blob),
        "the destination's file remains usable"
    );
    assert!(
        carried.contains(&allowed),
        "an independently authorized upload remains usable"
    );
    assert!(
        !carried.contains(&secret),
        "a guessed hash grants no access"
    );
    let refused = app
        .call(ChatBlobGet, &[&target_chat, &secret])
        .token(&ada.token)
        .send_bytes()
        .await;
    assert_eq!(refused.status, 404);
    let included = app
        .call(ChatBlobGet, &[&target_chat, &staged.blob])
        .token(&ada.token)
        .send_bytes()
        .await;
    assert_eq!(included.status, 200);
    assert_eq!(included.raw.as_ref(), b"included in the package");
    let authorized = app
        .call(ChatBlobGet, &[&target_chat, &allowed])
        .token(&ada.token)
        .send_bytes()
        .await;
    assert_eq!(authorized.status, 200);
    assert_eq!(
        authorized.raw.as_ref(),
        b"an independently authorized upload"
    );
}

#[tokio::test]
async fn a_package_holds_a_manifest_that_names_every_part_with_its_checksum_and_both_forms_of_each_thing()
 {
    let o = Office::new().await;
    let mut request = everything();
    request.package = Some(true);
    let export = o.made(&o.s.ada, o.topic(), &request).await;
    let held = o.contents(&o.s.ada, &export).await;
    let manifest: PackageManifest = serde_json::from_slice(entry(&held, PACKAGE_MANIFEST)).unwrap();
    assert_eq!(manifest.format, PACKAGE_FORMAT);
    assert_eq!(manifest.version, 1);
    assert_eq!(manifest.header.scope.id, o.topic());
    // Ada owns the topic, so she manages who may do what in it and the package carries that; every person it names is one it lists.
    for entry in manifest.access.as_deref().unwrap_or_default() {
        assert!(
            manifest
                .principals
                .iter()
                .any(|person| person.id == entry.principal),
            "{entry:?}"
        );
    }

    let document = manifest
        .objects
        .iter()
        .find(|object| object.kind == PackageKind::Document && object.name == "Handbook")
        .expect("the document is listed");
    let record = document.path.as_deref().expect("it has a record");
    assert!(record.ends_with(".json"), "{record}");
    assert!(
        document
            .renderings
            .iter()
            .any(|reading| reading.path.ends_with(".md")),
        "a reading copy lies beside the record"
    );
    let html = document
        .renderings
        .iter()
        .find(|reading| reading.format == "html")
        .expect("a package also has a document HTML reading copy");
    assert!(html.path.ends_with(".html"));
    let page = std::str::from_utf8(entry(&held, &html.path)).unwrap();
    for fragment in [
        "<!doctype html>",
        "<h1 class=\"title\">Handbook</h1>",
        "<p>Be kind.</p>",
        "<li>one</li>",
    ] {
        assert!(
            page.contains(fragment),
            "{fragment} is missing from the package HTML: {page}"
        );
    }
    // The checksum of every part is the SHA-256 of the entry it names.
    for object in &manifest.objects {
        if let (Some(path), Some(expected)) = (&object.path, &object.sha256) {
            let found = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&held[path.as_str()]));
            assert_eq!(&found, expected, "{path}");
        }
        for reading in &object.renderings {
            let found = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(entry(
                &held,
                &reading.path,
            )));
            assert_eq!(found, reading.sha256, "{}", reading.path);
        }
    }
    assert!(
        manifest
            .objects
            .iter()
            .any(|object| object.kind == PackageKind::Thread),
        "the thread is a part of it"
    );
    assert!(
        manifest
            .principals
            .iter()
            .any(|person| person.username == "ada"),
        "the people the records name are listed"
    );
}

#[tokio::test]
async fn a_dry_run_says_what_would_be_made_and_makes_nothing() {
    let o = Office::new().await;
    let ada = &o.s.ada;
    let project = o.s.tree.project.base.id;
    let file = upload(&o, ada, "topic.zip", &package_of(&o, o.topic()).await).await;
    let before = places::children(&o.s.rig.world, ada, project).await;

    let plan = plan(&o, ada, project, &asked("dry", &file)).await.ok();
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert_eq!(plan.node.id, project);
    assert_eq!(plan.counts.documents, 1);
    assert_eq!(plan.counts.threads, 1);
    // The topic the package is of has the name of one that is there: it is made beside it, not over it.
    let topic = plan
        .objects
        .items
        .iter()
        .find(|object| object.kind == PackageKind::Topic)
        .expect("the topic");
    assert_eq!(topic.action, ImportAction::KeepBoth);
    assert_eq!(
        topic.existing.as_ref().map(|found| found.found_by),
        Some(orch_api_contract::imports::ImportMatch::Name)
    );
    // Ada wrote it here: the same account.
    let person = plan
        .people
        .iter()
        .find(|person| person.username == "ada")
        .unwrap();
    assert_eq!(
        person.matched,
        orch_api_contract::imports::ImportPersonMatch::Id
    );
    // What never comes back is said whatever the package holds.
    let codes: Vec<&str> = plan.losses.iter().map(|loss| loss.code.as_str()).collect();
    for code in [
        "native_sessions",
        "workspaces",
        "secrets",
        "usage_and_audit",
        "personal_state",
        "web_images",
    ] {
        assert!(codes.contains(&code), "{code} in {codes:?}");
    }

    assert_eq!(
        places::children(&o.s.rig.world, ada, project).await,
        before,
        "nothing was made"
    );
    let listed =
        o.s.rig
            .world
            .app
            .call(MeImportList, &[])
            .token(&ada.token)
            .query(&PageQuery::default())
            .send()
            .await
            .ok();
    assert!(listed.items.is_empty(), "and no import was recorded");
}

#[tokio::test]
async fn an_import_makes_what_the_package_holds_and_a_second_one_finds_it_instead_of_making_it_again()
 {
    let o = Office::new().await;
    let ada = &o.s.ada;
    let project = o.s.tree.project.base.id;
    let file = upload(&o, ada, "topic.zip", &package_of(&o, o.topic()).await).await;
    let before = places::children(&o.s.rig.world, ada, project).await;

    // What the dry run said is what the job comes to.
    let dry = plan(&o, ada, project, &asked("one", &file)).await.ok();
    let first = imported(&o, ada, project, &asked("one", &file)).await;
    assert_eq!(first.state, ImportState::Done, "{:?}", first.failure);
    let counts = first
        .counts
        .expect("an import that ended says what it made");
    assert_eq!(
        (
            counts.places,
            counts.folders,
            counts.documents,
            counts.chats,
            counts.threads
        ),
        (
            dry.counts.places,
            dry.counts.folders,
            dry.counts.documents,
            dry.counts.chats,
            dry.counts.threads
        ),
        "the plan and the job count the same things"
    );
    let actions = first.actions.expect("and what it did with them");
    assert_eq!(
        (
            actions.create,
            actions.keep_both,
            actions.replace,
            actions.skip
        ),
        (
            dry.actions.create,
            dry.actions.keep_both,
            dry.actions.replace,
            dry.actions.skip
        )
    );
    let rows = report(&o, ada, first.id).await;
    assert!(
        rows.iter().all(|row| row.outcome != ImportOutcome::Failed),
        "{rows:#?}"
    );
    let topics = of_kind(&rows, PackageKind::Topic);
    assert_eq!(topics.len(), 1);
    assert_eq!(
        topics[0].outcome,
        ImportOutcome::KeptBoth,
        "the name was taken"
    );
    let made_topic = topics[0].made.as_ref().unwrap().id;
    let after = places::children(&o.s.rig.world, ada, project).await;
    assert_eq!(after.len(), before.len() + 1);
    assert!(after.contains(&made_topic));

    let documents = of_kind(&rows, PackageKind::Document);
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].outcome, ImportOutcome::Created);
    let document =
        o.s.rig
            .world
            .app
            .call(DocumentGet, &[&documents[0].made.as_ref().unwrap().id])
            .token(&ada.token)
            .send()
            .await
            .ok();
    assert_eq!(document.base.title, "Handbook");
    assert_ne!(
        document.base.id, o.document.base.id,
        "a copy, not the original"
    );
    let threads = of_kind(&rows, PackageKind::Thread);
    assert_eq!(threads.len(), 1);
    assert_eq!(
        threads[0].outcome,
        ImportOutcome::Created,
        "{:?}",
        threads[0].message
    );

    // The same package again, into the same place: what the first made is found by its source, and nothing is made twice.
    let again = asked("two", &file);
    let dry = plan(&o, ada, project, &again).await.ok();
    let found: Vec<_> = dry
        .objects
        .items
        .iter()
        .filter(|object| object.kind == PackageKind::Document)
        .collect();
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].existing.as_ref().map(|each| each.found_by),
        Some(orch_api_contract::imports::ImportMatch::Source)
    );
    assert_eq!(found[0].action, ImportAction::Replace);
    let second = imported(&o, ada, project, &again).await;
    assert_eq!(second.state, ImportState::Done, "{:?}", second.failure);
    let rows = report(&o, ada, second.id).await;
    let document = of_kind(&rows, PackageKind::Document);
    assert_eq!(document[0].outcome, ImportOutcome::Unchanged);
    assert_eq!(
        places::children(&o.s.rig.world, ada, project).await.len(),
        after.len(),
        "no second copy"
    );

    // The same request again names the same import and makes nothing.
    let repeated = imported(&o, ada, project, &again).await;
    assert_eq!(repeated.id, second.id);
}

#[tokio::test]
async fn a_document_that_changed_at_its_source_is_replaced_in_place_and_what_it_said_is_kept_as_a_version()
 {
    let o = Office::new().await;
    let ada = &o.s.ada;
    let project = o.s.tree.project.base.id;
    let first_file = upload(&o, ada, "one.zip", &package_of(&o, o.topic()).await).await;
    let first = imported(&o, ada, project, &asked("one", &first_file)).await;
    let rows = report(&o, ada, first.id).await;
    let made = of_kind(&rows, PackageKind::Document)[0]
        .made
        .as_ref()
        .unwrap()
        .id;

    // The source is renamed and exported again.
    o.s.rig
        .world
        .app
        .call(DocumentUpdate, &[&o.document.base.id])
        .token(&ada.token)
        .body(&DocumentUpdateBody {
            title: Some("Handbook, second edition".to_owned()),
            description: None,
        })
        .send()
        .await
        .ok();
    let second_file = upload(&o, ada, "two.zip", &package_of(&o, o.topic()).await).await;
    let dry = plan(&o, ada, project, &asked("two", &second_file))
        .await
        .ok();
    let document = dry
        .objects
        .items
        .iter()
        .find(|object| object.kind == PackageKind::Document)
        .unwrap();
    assert_eq!(document.action, ImportAction::Replace);
    assert_eq!(document.existing.as_ref().map(|each| each.id), Some(made));

    let second = imported(&o, ada, project, &asked("two", &second_file)).await;
    let rows = report(&o, ada, second.id).await;
    let row = of_kind(&rows, PackageKind::Document)[0];
    assert_eq!(row.outcome, ImportOutcome::Replaced);
    assert_eq!(row.made.as_ref().unwrap().id, made, "the same document");
    let document =
        o.s.rig
            .world
            .app
            .call(DocumentGet, &[&made])
            .token(&ada.token)
            .send()
            .await
            .ok();
    assert_eq!(document.base.title, "Handbook, second edition");
    let versions =
        o.s.rig
            .world
            .app
            .call(DocumentVersionList, &[&made])
            .token(&ada.token)
            .query(&DocumentVersionListQuery::default())
            .send()
            .await
            .ok()
            .items;
    assert!(
        versions.iter().any(|version| version.title == "Handbook"),
        "what it said before is a version: {versions:?}"
    );
}

#[tokio::test]
async fn taking_an_import_back_deletes_what_it_made_and_leaves_what_was_there() {
    let o = Office::new().await;
    let ada = &o.s.ada;
    let project = o.s.tree.project.base.id;
    let file = upload(&o, ada, "topic.zip", &package_of(&o, o.topic()).await).await;
    let before = places::children(&o.s.rig.world, ada, project).await;
    let done = imported(&o, ada, project, &asked("one", &file)).await;
    assert_eq!(
        places::children(&o.s.rig.world, ada, project).await.len(),
        before.len() + 1
    );

    // The plan says what goes, and changes nothing.
    let undo =
        o.s.rig
            .world
            .app
            .call(ImportUndoPreflight, &[&done.id])
            .token(&ada.token)
            .send()
            .await
            .ok();
    assert!(undo.blockers.is_empty(), "{:?}", undo.blockers);
    assert_eq!(undo.counts.documents, 1);
    assert_eq!(undo.counts.places, 1);
    assert_eq!(read(&o, ada, done.id).await.state, ImportState::Done);

    let job =
        o.s.rig
            .world
            .app
            .call(ImportUndo, &[&done.id])
            .token(&ada.token)
            .body(&ImportUndoBody::default())
            .send()
            .await;
    assert_eq!(job.status, 202, "{:?}", job.body);
    let job = o.ended(ada, job.ok().id).await;
    assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);
    assert_eq!(read(&o, ada, done.id).await.state, ImportState::Undone);
    assert_eq!(
        places::children(&o.s.rig.world, ada, project).await,
        before,
        "what was there stays and what the import made is gone"
    );
    // The original is as it was.
    o.s.rig
        .world
        .app
        .call(DocumentGet, &[&o.document.base.id])
        .token(&ada.token)
        .send()
        .await
        .ok();

    // An undone import is not undone twice.
    let twice =
        o.s.rig
            .world
            .app
            .call(ImportUndo, &[&done.id])
            .token(&ada.token)
            .body(&ImportUndoBody::default())
            .send()
            .await;
    assert_eq!(twice.status, 409);
    assert_eq!(twice.problem().code, ErrorCode::ImportUndone);
}

#[tokio::test]
async fn an_undone_package_can_be_imported_again_without_overwriting_its_removed_objects() {
    let o = Office::new().await;
    let ada = &o.s.ada;
    let project = o.s.tree.project.base.id;
    let file = upload(&o, ada, "topic.zip", &package_of(&o, o.topic()).await).await;
    let before = places::children(&o.s.rig.world, ada, project).await;
    let mut incarnations: Vec<BTreeMap<Id, Id>> = Vec::new();

    for cycle in 0..3 {
        let request = asked(&format!("create-{cycle}"), &file);
        let dry = plan(&o, ada, project, &request).await.ok();
        assert!(dry.blockers.is_empty(), "{:?}", dry.blockers);
        let done = imported(&o, ada, project, &request).await;
        assert_eq!(done.state, ImportState::Done, "{:?}", done.failure);
        let rows = report(&o, ada, done.id).await;
        assert!(
            rows.iter().all(|row| row.outcome != ImportOutcome::Failed),
            "{rows:#?}"
        );
        let made: BTreeMap<Id, Id> = rows
            .iter()
            .filter_map(|row| row.made.as_ref().map(|made| (row.source_id, made.id)))
            .collect();
        assert!(!made.is_empty());
        for previous in &incarnations {
            assert_eq!(
                made.keys().collect::<Vec<_>>(),
                previous.keys().collect::<Vec<_>>()
            );
            for (source, id) in &made {
                assert_ne!(
                    Some(id),
                    previous.get(source),
                    "removed objects remain restorable"
                );
            }
        }
        assert_eq!(
            places::children(&o.s.rig.world, ada, project).await.len(),
            before.len() + 1
        );

        let repeated = imported(&o, ada, project, &asked(&format!("live-{cycle}"), &file)).await;
        assert_eq!(repeated.state, ImportState::Done, "{:?}", repeated.failure);
        let repeated_rows = report(&o, ada, repeated.id).await;
        let repeated_ids: BTreeMap<Id, Id> = repeated_rows
            .iter()
            .filter_map(|row| row.made.as_ref().map(|made| (row.source_id, made.id)))
            .collect();
        assert_eq!(
            repeated_ids, made,
            "a live re-import keeps ids and makes no duplicates"
        );
        assert_eq!(
            places::children(&o.s.rig.world, ada, project).await.len(),
            before.len() + 1
        );

        let undo =
            o.s.rig
                .world
                .app
                .call(ImportUndo, &[&done.id])
                .token(&ada.token)
                .body(&ImportUndoBody::default())
                .send()
                .await;
        assert_eq!(undo.status, 202, "{:?}", undo.body);
        let job = o.ended(ada, undo.ok().id).await;
        assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);
        assert_eq!(read(&o, ada, done.id).await.state, ImportState::Undone);
        assert_eq!(places::children(&o.s.rig.world, ada, project).await, before);

        let nodes: Vec<Id> = rows
            .iter()
            .filter(|row| {
                matches!(
                    row.kind,
                    PackageKind::Org
                        | PackageKind::Project
                        | PackageKind::Topic
                        | PackageKind::Thread
                )
            })
            .filter_map(|row| row.made.as_ref().map(|made| made.id))
            .collect();
        assert!(!nodes.is_empty());
        let mut conn = o.s.rig.db.acquire().await.unwrap();
        let retained: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM nodes WHERE id=ANY($1) AND deleted_at IS NOT NULL",
        )
        .bind(&nodes)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(
            retained,
            i64::try_from(nodes.len()).unwrap(),
            "Undo retains the earlier incarnation"
        );
        incarnations.push(made);
    }
}

#[tokio::test]
async fn a_person_who_may_not_make_things_in_a_place_gets_no_plan_to_bring_them_and_an_import_that_is_not_theirs_is_not_found()
 {
    let o = Office::new().await;
    let (ada, cai) = (&o.s.ada, &o.s.cai);
    let project = o.s.tree.project.base.id;
    let file = upload(&o, ada, "topic.zip", &package_of(&o, o.topic()).await).await;

    // Cai only reads: the plan says nothing can be made, and a request to make it is refused.
    let cai_file = upload(&o, cai, "topic.zip", &package_of(&o, o.topic()).await).await;
    let dry = plan(&o, cai, project, &asked("cai", &cai_file)).await.ok();
    assert!(
        dry.blockers
            .iter()
            .any(|blocker| blocker.code == "import.nothing" || blocker.code == "import.forbidden"),
        "{:?}",
        dry.blockers
    );
    let refused =
        o.s.rig
            .world
            .app
            .call(NodeImportCreate, &[&project])
            .token(&cai.token)
            .body(&asked("cai", &cai_file))
            .send()
            .await;
    assert!(
        matches!(refused.status.as_u16(), 403 | 409),
        "{} {:?}",
        refused.status,
        refused.body
    );

    // An upload that is not the caller's is not theirs to import.
    let theirs =
        o.s.rig
            .world
            .app
            .call(NodeImportCreate, &[&project])
            .token(&cai.token)
            .body(&asked("not-mine", &file))
            .send()
            .await;
    assert_eq!(theirs.status, 404);

    // Somebody else's import is not found.
    let done = imported(&o, ada, project, &asked("one", &file)).await;
    let other =
        o.s.rig
            .world
            .app
            .call(ImportGet, &[&done.id])
            .token(&cai.token)
            .send()
            .await;
    assert_eq!(other.status, 404);
}

#[tokio::test]
async fn import_limits_follow_exports_until_overridden_and_each_null_restores_that_following() {
    let world = support::World::new().await;
    let by = places::person(&world, "ada").await;
    let place = by.person.personal_org;
    let blobs = &world.app.app.blobs;
    let meta = blobs
        .put_bytes(bytes::Bytes::from_static(b"# Plan\n"))
        .await
        .unwrap();
    let mut conn = world.conn().await;
    blobs.register(&mut conn, meta, world.now()).await.unwrap();
    orch_db::queries::uploads::remember(&mut conn, by.id(), &meta.hash, world.now())
        .await
        .unwrap();
    let exports = world
        .app
        .app
        .engine
        .export_limits(&mut conn, place)
        .await
        .unwrap();
    drop(conn);
    let request = asked(
        "limits",
        &ImportSourceFile {
            name: "plan.md".to_owned(),
            blob: meta.hash,
        },
    );
    // Distinct values expose whether each nullable import limit follows the current export value.
    for (changes, expected) in [
        (vec![], (exports.max_bytes, exports.max_items)),
        (
            vec![
                ("export.max_bytes", serde_json::json!(8_192)),
                ("export.max_items", serde_json::json!(7)),
            ],
            (8_192, 7),
        ),
        (
            vec![
                ("import.max_bytes", serde_json::json!(4_096)),
                ("import.max_items", serde_json::json!(3)),
            ],
            (4_096, 3),
        ),
        (
            vec![
                ("export.max_bytes", serde_json::json!(16_384)),
                ("export.max_items", serde_json::json!(9)),
                ("import.max_bytes", Value::Null),
            ],
            (16_384, 3),
        ),
        (vec![("import.max_items", Value::Null)], (16_384, 9)),
    ] {
        for (key, value) in changes {
            world
                .set_at(orch_settings::Target::Node(place), key, value)
                .await;
        }
        let dry = world
            .app
            .call(NodeImportPreflight, &[&place])
            .token(&by.token)
            .body(&ImportPreflightRequest {
                request: request.clone(),
                view: ImportView::default(),
            })
            .send()
            .await
            .ok();
        assert_eq!((dry.limits.max_bytes, dry.limits.max_items), expected);
        assert!(dry.blockers.is_empty(), "{:?}", dry.blockers);
    }
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn all_five_import_routes_serve_through_the_debug_registry_with_reviewed_action_guards() {
    use orch_api_contract::imports::{ImportPreviewRequest, NodeImportPreview};
    use orch_api_core::middleware::transaction_guard::{Boundary, LEDGER, boundary_of};
    use orch_db::queries::imports as rows;

    let world = support::World::new().await;
    let by = places::person(&world, "ada").await;
    let place = by.person.personal_org;
    let blobs = &world.app.app.blobs;
    let meta = blobs
        .put_bytes(bytes::Bytes::from_static(b"# Plan\n\nBring this text.\n"))
        .await
        .unwrap();
    let mut conn = world.conn().await;
    blobs.register(&mut conn, meta, world.now()).await.unwrap();
    orch_db::queries::uploads::remember(&mut conn, by.id(), &meta.hash, world.now())
        .await
        .unwrap();
    drop(conn);
    let request = asked(
        "guarded-import",
        &ImportSourceFile {
            name: "plan.md".to_owned(),
            blob: meta.hash,
        },
    );

    // These calls exercise Registry::serve, including require_seen; calling engine helpers would miss that boundary.
    let reply = world
        .app
        .call(NodeImportPreflight, &[&place])
        .token(&by.token)
        .body(&ImportPreflightRequest {
            request: request.clone(),
            view: ImportView::default(),
        })
        .send()
        .await;
    assert_eq!(reply.status, 200, "preflight: {:?}", reply.body);
    let dry = reply.ok();
    assert!(dry.blockers.is_empty(), "{:?}", dry.blockers);
    assert_eq!(dry.counts.documents, 1);
    let document = dry
        .objects
        .items
        .iter()
        .find(|object| object.kind == PackageKind::Document)
        .unwrap();
    let reply = world
        .app
        .call(NodeImportPreview, &[&place])
        .token(&by.token)
        .body(&ImportPreviewRequest {
            source: request.source.clone(),
            object: document.source_id,
        })
        .send()
        .await;
    assert_eq!(reply.status, 200, "preview: {:?}", reply.body);
    assert!(reply.ok().text.contains("Bring this text."));
    let mut conn = world.conn().await;
    assert!(
        rows::by_client(&mut conn, by.id(), &request.client_id)
            .await
            .unwrap()
            .is_none(),
        "both reads create no import"
    );
    drop(conn);

    let reply = world
        .app
        .call(NodeImportCreate, &[&place])
        .token(&by.token)
        .body(&request)
        .send()
        .await;
    assert_eq!(reply.status, 202, "create: {:?}", reply.body);
    let created = reply.ok();
    let mut conn = world.conn().await;
    let import = rows::by_client(&mut conn, by.id(), &request.client_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(import.job_id, Some(created.id));
    // This test checks request acceptance, not worker execution. A completed empty fixture lets the Undo door succeed too.
    sqlx::query("UPDATE imports SET state = 'done' WHERE id = $1")
        .bind(import.id)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let reply = world
        .app
        .call(ImportUndoPreflight, &[&import.id])
        .token(&by.token)
        .send()
        .await;
    assert_eq!(reply.status, 200, "Undo preflight: {:?}", reply.body);
    assert!(reply.ok().blockers.is_empty());
    let reply = world
        .app
        .call(ImportUndo, &[&import.id])
        .token(&by.token)
        .body(&ImportUndoBody::default())
        .send()
        .await;
    assert_eq!(reply.status, 202, "Undo: {:?}", reply.body);
    let undo = reply.ok();
    assert_ne!(undo.id, created.id);
    let mut conn = world.conn().await;
    let recorded = rows::get(&mut conn, import.id).await.unwrap().unwrap();
    assert_eq!(recorded.undo_job_id, Some(undo.id));
    assert_eq!(recorded.state, rows::state::UNDOING);

    let ledger: Vec<Value> = serde_json::from_str(LEDGER).unwrap();
    for (name, expected) in [
        ("NodeImportPreflight", Boundary::Open),
        ("NodeImportPreview", Boundary::Open),
        ("ImportUndoPreflight", Boundary::Open),
        ("NodeImportCreate", Boundary::Guarded),
        ("ImportUndo", Boundary::Guarded),
    ] {
        assert_eq!(
            ledger.iter().filter(|row| row["name"] == name).count(),
            1,
            "{name} has one explicit ledger row"
        );
        assert_eq!(boundary_of(name), expected, "{name}");
    }
}

#[tokio::test]
async fn an_import_that_would_pass_a_limit_is_not_begun_and_says_the_limit_and_the_size() {
    let o = Office::new().await;
    let ada = &o.s.ada;
    let project = o.s.tree.project.base.id;
    let file = upload(&o, ada, "topic.zip", &package_of(&o, o.topic()).await).await;
    // An explicit import limit is enforced before the job is begun.
    o.s.rig
        .world
        .set_at(
            orch_settings::Target::Node(project),
            "import.max_items",
            serde_json::json!(1),
        )
        .await;
    let dry = plan(&o, ada, project, &asked("big", &file)).await.ok();
    let blocker = dry
        .blockers
        .iter()
        .find(|blocker| blocker.code == "import.too_many")
        .unwrap_or_else(|| panic!("{:?}", dry.blockers));
    assert_eq!(blocker.params.as_ref().unwrap()["limit"], 1);
    let refused =
        o.s.rig
            .world
            .app
            .call(NodeImportCreate, &[&project])
            .token(&ada.token)
            .body(&asked("big", &file))
            .send()
            .await;
    assert_eq!(refused.status, 409);
    assert_eq!(refused.problem().code, ErrorCode::ImportTooMany);
}

#[tokio::test]
async fn plain_markdown_files_become_documents_and_a_folder_becomes_a_document_folder() {
    let o = Office::new().await;
    let ada = &o.s.ada;
    let topic = o.topic();
    let notes = upload(
        &o,
        ada,
        "notes/plan.md",
        b"# Plan\n\nFirst, [the list](list.md).\n",
    )
    .await;
    let list = upload(&o, ada, "notes/list.md", b"# List\n\n- one\n- two\n").await;
    let request = ImportRequest {
        client_id: "plain".to_owned(),
        source: ImportSource {
            files: vec![notes, list],
            mode: None,
        },
        destination: Default::default(),
        options: ImportOptions::default(),
        choices: Default::default(),
    };
    let done = imported(&o, ada, topic, &request).await;
    assert_eq!(done.state, ImportState::Done, "{:?}", done.failure);
    let rows = report(&o, ada, done.id).await;
    assert_eq!(of_kind(&rows, PackageKind::Folder).len(), 1, "{rows:#?}");
    let documents = of_kind(&rows, PackageKind::Document);
    assert_eq!(documents.len(), 2);
    assert!(
        documents
            .iter()
            .all(|row| row.outcome == ImportOutcome::Created)
    );
    let titles: Vec<&str> = documents.iter().map(|row| row.name.as_str()).collect();
    assert!(
        titles.contains(&"plan") || titles.contains(&"Plan"),
        "{titles:?}"
    );
}

/// A tar of the files `entries` name, packed with gzip.
fn a_tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, bytes) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        builder.append_data(&mut header, *name, *bytes).unwrap();
    }
    let tar = builder.into_inner().unwrap();
    let mut packed = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    packed.write_all(&tar).unwrap();
    packed.finish().unwrap()
}

#[tokio::test]
async fn a_tar_packed_with_gzip_is_read_as_the_files_it_holds_and_a_folder_in_it_becomes_a_document_folder()
 {
    let o = Office::new().await;
    let ada = &o.s.ada;
    let topic = o.topic();
    let tar = a_tar_gz(&[
        ("notes/plan.md", b"# Plan\n\nFirst, [the list](list.md).\n"),
        ("notes/list.md", b"# List\n\n- one\n- two\n"),
    ]);
    let file = upload(&o, ada, "notes.tar.gz", &tar).await;
    let request = asked("tar", &file);
    let seen = plan(&o, ada, topic, &request).await.ok();
    assert_eq!(seen.source.mode, ImportSourceMode::Files);
    assert_eq!(seen.counts.documents, 2, "{:?}", seen.counts);
    assert_eq!(seen.counts.folders, 1, "{:?}", seen.counts);
    let done = imported(&o, ada, topic, &request).await;
    assert_eq!(done.state, ImportState::Done, "{:?}", done.failure);
    let rows = report(&o, ada, done.id).await;
    assert_eq!(of_kind(&rows, PackageKind::Folder).len(), 1, "{rows:#?}");
    let documents = of_kind(&rows, PackageKind::Document);
    assert_eq!(documents.len(), 2, "{rows:#?}");
    assert!(
        documents
            .iter()
            .all(|row| row.outcome == ImportOutcome::Created),
        "{rows:#?}"
    );
}

/// A zip of the files `entries` name.
fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[tokio::test]
async fn a_zip_that_holds_only_zips_is_read_as_the_files_in_them_and_so_are_zips_chosen_together() {
    let o = Office::new().await;
    let ada = &o.s.ada;
    let topic = o.topic();
    let part_one = zip_of(&[(
        "Plan 1a2b.md",
        b"# Plan\n\nSee [the list](List%203c4d.md).\n",
    )]);
    let part_two = zip_of(&[("List 3c4d.md", b"# List\n\n- one\n")]);
    // A Notion export: the zips are inside one zip, beside what a Mac adds.
    let outer = zip_of(&[
        ("Export-1/Part-1.zip", part_one.as_slice()),
        ("Export-1/Part-2.zip", part_two.as_slice()),
        ("__MACOSX/Export-1/._Part-1.zip", b"litter"),
    ]);
    let file = upload(&o, ada, "Export-1.zip", &outer).await;
    let seen = plan(&o, ada, topic, &asked("zips-inside", &file))
        .await
        .ok();
    assert_eq!(seen.source.mode, ImportSourceMode::Files);
    assert_eq!(seen.counts.documents, 2, "{:?}", seen.counts);
    // The same zips chosen together make the same source.
    let one = upload(&o, ada, "Part-1.zip", &part_one).await;
    let two = upload(&o, ada, "Part-2.zip", &part_two).await;
    let together = ImportRequest {
        client_id: "zips-together".to_owned(),
        source: ImportSource {
            files: vec![one, two],
            mode: None,
        },
        destination: Default::default(),
        options: ImportOptions::default(),
        choices: Default::default(),
    };
    let done = imported(&o, ada, topic, &together).await;
    assert_eq!(done.state, ImportState::Done, "{:?}", done.failure);
    let rows = report(&o, ada, done.id).await;
    let documents = of_kind(&rows, PackageKind::Document);
    assert_eq!(documents.len(), 2, "{rows:#?}");
    assert!(
        documents
            .iter()
            .all(|row| row.outcome == ImportOutcome::Created),
        "{rows:#?}"
    );
}

#[tokio::test]
async fn notion_export_ids_leave_display_names_while_collisions_links_and_reimport_keep_their_identities()
 {
    use orch_api_contract::documents::{
        BlockContent, DocumentSyncGet, DocumentSyncGetQuery, Inline,
    };
    use std::collections::{BTreeSet, HashSet};

    let o = Office::new().await;
    let ada = &o.s.ada;
    let topic = o.topic();
    let first = "0a1b2c3d4e5f60718293a4b5c6d7e8f9";
    let second = "1a1b2c3d4e5f60718293a4b5c6d7e8f9";
    assert_eq!(first.len(), 32);
    assert_eq!(second.len(), 32);
    let third = "2a1b2c3d4e5f60718293a4b5c6d7e8f9";
    assert_eq!(third.len(), 32);
    let home = format!(
        "# Home\n\n[Later plan](Roadmap%20{first}/Plan%20{second}.md) and [meeting](Roadmap%20{second}/Meeting.md) and [roadmap](Roadmap%20{third}.md).\n"
    );
    let entries = [
        (format!("Roadmap {first}/Plan {first}.md"), "# First plan\n"),
        (
            format!("Roadmap {first}/Plan {second}.md"),
            "# Later plan\n",
        ),
        (
            format!("Roadmap {first}/Plan (2).md"),
            "# Existing numbered name\n",
        ),
        (format!("Roadmap {second}/Meeting.md"), "# Meeting\n"),
        (format!("Home {first}.md"), home.as_str()),
        (format!("Roadmap {third}.md"), "# Root roadmap\n"),
    ];
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (path, text) in entries {
        writer
            .start_file(path, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(text.as_bytes()).unwrap();
    }
    let file = upload(
        &o,
        ada,
        "notion.zip",
        &writer.finish().unwrap().into_inner(),
    )
    .await;
    let request = asked("notion-names-first", &file);
    let dry = plan(&o, ada, topic, &request).await.ok();
    assert!(dry.blockers.is_empty(), "{:?}", dry.blockers);
    let folder_names: BTreeSet<_> = dry
        .objects
        .items
        .iter()
        .filter(|row| row.kind == PackageKind::Folder)
        .map(|row| row.name.as_str())
        .collect();
    assert_eq!(folder_names, BTreeSet::from(["Roadmap", "Roadmap (2)"]));
    let document_names: BTreeSet<_> = dry
        .objects
        .items
        .iter()
        .filter(|row| row.kind == PackageKind::Document)
        .map(|row| row.name.as_str())
        .collect();
    assert_eq!(
        document_names,
        BTreeSet::from([
            "Home",
            "Meeting",
            "Plan",
            "Plan (2)",
            "Plan (3)",
            "Roadmap (3)"
        ])
    );

    let done = imported(&o, ada, topic, &request).await;
    assert_eq!(done.state, ImportState::Done, "{:?}", done.failure);
    let rows = report(&o, ada, done.id).await;
    assert_eq!(rows.len(), 8, "{rows:#?}");
    assert!(
        rows.iter().all(|row| row.outcome == ImportOutcome::Created),
        "{rows:#?}"
    );
    for row in of_kind(&rows, PackageKind::Document) {
        let id = row.made.as_ref().unwrap().id;
        let document =
            o.s.rig
                .world
                .app
                .call(DocumentGet, &[&id])
                .token(&ada.token)
                .send()
                .await
                .ok();
        assert_eq!(document.base.title, row.name);
    }
    let id_of = |name: &str| {
        rows.iter()
            .find(|row| row.kind == PackageKind::Document && row.name == name)
            .unwrap()
            .made
            .as_ref()
            .unwrap()
            .id
    };
    let sync =
        o.s.rig
            .world
            .app
            .call(DocumentSyncGet, &[&id_of("Home")])
            .token(&ada.token)
            .query(&DocumentSyncGetQuery::default())
            .send()
            .await
            .ok();
    let references: HashSet<_> = sync
        .blocks
        .iter()
        .filter_map(|block| match &block.content {
            BlockContent::Inline(runs) => Some(runs),
            _ => None,
        })
        .flatten()
        .filter_map(|run| match run {
            Inline::Ref { id, .. } => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(
        references,
        HashSet::from([id_of("Plan (3)"), id_of("Meeting"), id_of("Roadmap (3)")])
    );

    let later = imported(&o, ada, topic, &asked("notion-names-again", &file)).await;
    assert_eq!(later.state, ImportState::Done, "{:?}", later.failure);
    let after = report(&o, ada, later.id).await;
    assert_eq!(after.len(), rows.len());
    assert!(
        after.iter().all(|row| matches!(
            row.outcome,
            ImportOutcome::Replaced | ImportOutcome::Unchanged
        )),
        "{after:#?}"
    );
    assert_eq!(
        after
            .iter()
            .map(|row| row.made.as_ref().unwrap().id)
            .collect::<HashSet<_>>(),
        rows.iter()
            .map(|row| row.made.as_ref().unwrap().id)
            .collect::<HashSet<_>>()
    );
}

/// The path of an entry written with a checksum is set to the checksum of what the entry holds now.
fn rechecked(value: &mut Value, digests: &BTreeMap<String, String>) {
    match value {
        Value::Array(items) => {
            for item in items {
                rechecked(item, digests);
            }
        }
        Value::Object(map) => {
            let path = map.get("path").and_then(Value::as_str).map(str::to_owned);
            if map.contains_key("sha256")
                && let Some(digest) = path.and_then(|path| digests.get(&path))
            {
                map.insert("sha256".to_owned(), Value::String(digest.clone()));
            }
            for member in map.values_mut() {
                rechecked(member, digests);
            }
        }
        _ => {}
    }
}

/// The package that `held` is, written again with one person of it under another identifier, which is how a package of another
/// deployment names the same people: every record says the new identifier and the manifest holds the checksums of what changed.
fn renamed(held: &BTreeMap<String, Vec<u8>>, from: Id, to: Id) -> Vec<u8> {
    let (from, to) = (from.to_string(), to.to_string());
    let mut entries: BTreeMap<String, Vec<u8>> = held
        .iter()
        .map(|(path, content)| {
            let content = match std::str::from_utf8(content) {
                Ok(text) => text.replace(&from, &to).into_bytes(),
                Err(_) => content.clone(),
            };
            (path.clone(), content)
        })
        .collect();
    let digests: BTreeMap<String, String> = entries
        .iter()
        .map(|(path, content)| {
            (
                path.clone(),
                hex::encode(<sha2::Sha256 as sha2::Digest>::digest(content)),
            )
        })
        .collect();
    let mut manifest: Value = serde_json::from_slice(&entries[PACKAGE_MANIFEST]).unwrap();
    rechecked(&mut manifest, &digests);
    entries.insert(
        PACKAGE_MANIFEST.to_owned(),
        serde_json::to_vec(&manifest).unwrap(),
    );
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in &entries {
        writer
            .start_file(name.as_str(), zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[tokio::test]
async fn the_people_of_a_package_are_taken_for_accounts_by_address_or_username_and_then_as_the_person_chooses()
 {
    let o = Office::new().await;
    let (ada, bea) = (&o.s.ada, &o.s.bea);
    let project = o.s.tree.project.base.id;
    let mut request = everything();
    request.package = Some(true);
    let export = o.made(ada, o.topic(), &request).await;
    let held = o.contents(ada, &export).await;
    let elsewhere = new_id();
    let file = upload(
        &o,
        ada,
        "elsewhere.zip",
        &renamed(&held, ada.id(), elsewhere),
    )
    .await;

    // Nobody here has the identifier the package names her by, but the username is the same.
    let dry = plan(&o, ada, project, &asked("people", &file)).await.ok();
    assert!(dry.blockers.is_empty(), "{:?}", dry.blockers);
    let person = dry
        .people
        .iter()
        .find(|person| person.principal == elsewhere)
        .unwrap_or_else(|| panic!("the person of the package: {:?}", dry.people));
    assert_eq!(person.username, "ada");
    // By her confirmed address where she has one, by her username where she does not.
    assert!(
        matches!(
            person.matched,
            ImportPersonMatch::Email | ImportPersonMatch::Username
        ),
        "{:?}",
        person.matched
    );
    assert_eq!(person.user.as_ref().map(|user| user.id), Some(ada.id()));
    assert!(person.objects > 0, "she made things in it");

    // The person chooses another account for her.
    let mut chosen = asked("people", &file);
    chosen.choices.people.push(ImportPersonChoice {
        principal: elsewhere,
        user: Some(bea.id()),
    });
    let dry = plan(&o, ada, project, &chosen).await.ok();
    let person = dry
        .people
        .iter()
        .find(|person| person.principal == elsewhere)
        .unwrap();
    assert_eq!(person.matched, ImportPersonMatch::Choice);
    assert_eq!(person.user.as_ref().map(|user| user.id), Some(bea.id()));

    // Or nobody: she stays an imported person, and the import goes on.
    chosen.choices.people[0].user = None;
    let dry = plan(&o, ada, project, &chosen).await.ok();
    let person = dry
        .people
        .iter()
        .find(|person| person.principal == elsewhere)
        .unwrap();
    assert_eq!(person.matched, ImportPersonMatch::None);
    assert!(person.user.is_none());
    let done = imported(&o, ada, project, &chosen).await;
    assert_eq!(done.state, ImportState::Done, "{:?}", done.failure);
    let rows = report(&o, ada, done.id).await;
    assert!(
        rows.iter().all(|row| row.outcome != ImportOutcome::Failed),
        "{rows:#?}"
    );
}

/// The document, its block writers and its saved versions obey one people choice; ownership stays with the importer.
async fn check_document_authorship(mode: &str) {
    use orch_api_contract::documents::{
        Author, DocumentSyncGet, DocumentSyncGetQuery, DocumentVersionGet, VersionCause,
    };
    use orch_api_contract::imports::ImportAuthors;
    use orch_db::queries::documents::{self as document_rows, history};
    let o = Office::new().await;
    let ada = &o.s.ada;
    let bea = &o.s.bea;
    {
        let mut conn = o.s.rig.db.acquire().await.unwrap();
        let source = document_rows::get(&mut conn, o.document.base.id)
            .await
            .unwrap()
            .unwrap();
        history::cut(
            &mut conn,
            &source,
            VersionCause::Rewrite,
            o.s.rig.world.now(),
        )
        .await
        .unwrap();
    }
    let mut export_request = everything();
    export_request.package = Some(true);
    export_request.versions = Some(true);
    let export = o.made(ada, o.topic(), &export_request).await;
    let held = o.contents(ada, &export).await;
    let exported_versions: Vec<Value> = held
        .iter()
        .filter(|(path, _)| path.contains(".history/") && path.ends_with(".json"))
        .map(|(_, bytes)| serde_json::from_slice(bytes).unwrap())
        .collect();
    assert!(!exported_versions.is_empty());
    for version in exported_versions {
        assert!(
            version["authors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|author| author["user"] == json!(ada.id())),
            "version writers travel in the package, independently of its block writers"
        );
    }
    let source_person = if mode == "choice" { new_id() } else { bea.id() };
    let file = upload(
        &o,
        ada,
        "authors.zip",
        &renamed(&held, ada.id(), source_person),
    )
    .await;
    let mut request = asked(mode, &file);
    request.options.authors = Some(if mode == "labels" {
        ImportAuthors::Labels
    } else {
        ImportAuthors::Accounts
    });
    if mode == "choice" || mode == "unmatched" {
        request.choices.people.push(ImportPersonChoice {
            principal: source_person,
            user: (mode == "choice").then_some(bea.id()),
        });
    }
    let dry = plan(&o, ada, o.s.tree.project.base.id, &request).await.ok();
    assert!(dry.blockers.is_empty(), "{:?}", dry.blockers);
    let matched = dry
        .people
        .iter()
        .find(|person| person.principal == source_person)
        .unwrap();
    let expected = if mode == "labels" || mode == "unmatched" {
        ada.id()
    } else {
        bea.id()
    };
    match mode {
        "accounts" | "labels" => assert_eq!(matched.matched, ImportPersonMatch::Id),
        "choice" => assert_eq!(matched.matched, ImportPersonMatch::Choice),
        "unmatched" => assert_eq!(matched.matched, ImportPersonMatch::None),
        _ => panic!("unknown test mode"),
    }
    let done = imported(&o, ada, o.s.tree.project.base.id, &request).await;
    assert_eq!(done.state, ImportState::Done, "{:?}", done.failure);
    let rows = report(&o, ada, done.id).await;
    let row = of_kind(&rows, PackageKind::Document)[0];
    let document = row.made.as_ref().unwrap().id;
    let read =
        o.s.rig
            .world
            .app
            .call(DocumentSyncGet, &[&document])
            .token(&ada.token)
            .query(&DocumentSyncGetQuery::default())
            .send()
            .await
            .ok();
    let user_of = |author: &Author| match author {
        Author::User(user) => user.id,
        Author::Agent(_) => panic!("a mapped account is a person"),
    };
    assert_eq!(
        user_of(&read.document.base.created_by),
        expected,
        "document maker"
    );
    assert_eq!(
        user_of(read.document.base.updated_by.as_ref().unwrap()),
        expected,
        "document writer"
    );
    assert_eq!(
        read.document.base.owner.id,
        ada.id(),
        "mapping authors does not transfer ownership"
    );
    assert!(!read.blocks.is_empty());
    for block in &read.blocks {
        assert_eq!(
            user_of(block.updated_by.as_ref().unwrap()),
            expected,
            "block {}",
            block.id
        );
    }
    let versions =
        o.s.rig
            .world
            .app
            .call(DocumentVersionList, &[&document])
            .token(&ada.token)
            .query(&DocumentVersionListQuery::default())
            .send()
            .await
            .ok()
            .items;
    let saved: Vec<_> = versions
        .iter()
        .filter(|version| version.version.is_some())
        .collect();
    assert!(
        !saved.is_empty(),
        "the package must exercise saved versions"
    );
    for version in saved {
        assert!(!version.authors.is_empty());
        assert!(
            version
                .authors
                .iter()
                .all(|author| user_of(author) == expected),
            "version authors: {:?}",
            version.authors
        );
        let number = version.version.unwrap();
        let snapshot =
            o.s.rig
                .world
                .app
                .call(DocumentVersionGet, &[&document, &number])
                .token(&ada.token)
                .send()
                .await
                .ok();
        assert!(!snapshot.blocks.is_empty());
        for block in snapshot.blocks {
            assert_eq!(
                user_of(block.updated_by.as_ref().unwrap()),
                expected,
                "saved block"
            );
        }
    }
    if mode == "labels" || mode == "unmatched" {
        assert!(
            row.author.is_some(),
            "the original credit remains in the report"
        );
    }
}

#[tokio::test]
async fn document_block_and_version_authors_follow_the_same_account_match() {
    check_document_authorship("accounts").await;
}
#[tokio::test]
async fn document_block_and_version_authors_follow_the_explicit_person_choice() {
    check_document_authorship("choice").await;
}
#[tokio::test]
async fn document_block_and_version_authors_keep_original_labels_when_labels_are_chosen() {
    check_document_authorship("labels").await;
}
#[tokio::test]
async fn document_block_and_version_authors_keep_original_labels_for_an_unmatched_person() {
    check_document_authorship("unmatched").await;
}

/// A small zip of no package and no notes: what a person attaches to a document.
fn a_zip() -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file("data.txt", zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"data").unwrap();
    writer.finish().unwrap().into_inner()
}

/// A document of `by` beside `node`, written from Markdown, private where `private` says.
async fn document_beside(o: &Office, by: &Who, node: Id, title: &str, private: bool) -> Id {
    o.s.rig
        .world
        .app
        .call(NodeDocumentCreate, &[&node])
        .token(&by.token)
        .body(&NodeDocumentCreateBody {
            title: title.to_owned(),
            blocks: None,
            markdown: Some("Words.\n".to_owned()),
            access: private.then_some(NewDocumentAccess::Private),
        })
        .send()
        .await
        .ok()
        .base
        .id
}

#[tokio::test]
async fn taking_an_import_back_leaves_what_people_added_since_and_what_holds_it_and_takes_the_rest()
{
    let o = Office::new().await;
    let (ada, dan) = (&o.s.ada, &o.s.dan);
    let project = o.s.tree.project.base.id;
    let file = upload(&o, ada, "topic.zip", &package_of(&o, o.topic()).await).await;
    let done = imported(&o, ada, project, &asked("one", &file)).await;
    let rows = report(&o, ada, done.id).await;
    let topic = of_kind(&rows, PackageKind::Topic)[0]
        .made
        .as_ref()
        .unwrap()
        .id;
    let made_document = of_kind(&rows, PackageKind::Document)[0]
        .made
        .as_ref()
        .unwrap()
        .id;

    // Somebody else writes a document in the imported topic.
    let theirs = document_beside(&o, dan, topic, "Dan's note", false).await;

    let undo =
        o.s.rig
            .world
            .app
            .call(ImportUndoPreflight, &[&done.id])
            .token(&ada.token)
            .send()
            .await
            .ok();
    assert!(
        undo.added_since >= 1,
        "what Dan wrote is counted as what stays: {undo:?}"
    );

    let job =
        o.s.rig
            .world
            .app
            .call(ImportUndo, &[&done.id])
            .token(&ada.token)
            .body(&ImportUndoBody::default())
            .send()
            .await;
    let job = o.ended(ada, job.ok().id).await;
    assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);

    // What the import made and nobody touched is gone; what Dan wrote stays, and so does the topic that holds it.
    let get = |id: Id, who: &Who| {
        let call =
            o.s.rig
                .world
                .app
                .call(DocumentGet, &[&id])
                .token(&who.token);
        async move { call.send().await }
    };
    assert_eq!(get(theirs, dan).await.status, 200, "his note stays");
    assert_ne!(
        get(made_document, ada).await.status,
        200,
        "the document the import made is taken back"
    );
    assert!(
        places::children(&o.s.rig.world, ada, project)
            .await
            .contains(&topic),
        "the topic that holds his note stays"
    );
}

#[tokio::test]
async fn a_document_somebody_else_changed_since_the_import_stays_when_it_is_taken_back() {
    let o = Office::new().await;
    let (ada, dan) = (&o.s.ada, &o.s.dan);
    let project = o.s.tree.project.base.id;
    let file = upload(&o, ada, "topic.zip", &package_of(&o, o.topic()).await).await;
    let done = imported(&o, ada, project, &asked("one", &file)).await;
    let rows = report(&o, ada, done.id).await;
    let made_document = of_kind(&rows, PackageKind::Document)[0]
        .made
        .as_ref()
        .unwrap()
        .id;
    o.s.rig.world.advance(chrono::Duration::minutes(5));
    o.s.rig
        .world
        .app
        .call(DocumentUpdate, &[&made_document])
        .token(&dan.token)
        .body(&DocumentUpdateBody {
            title: Some("Handbook, as Dan has it".to_owned()),
            description: None,
        })
        .send()
        .await
        .ok();
    let undo =
        o.s.rig
            .world
            .app
            .call(ImportUndoPreflight, &[&done.id])
            .token(&ada.token)
            .send()
            .await
            .ok();
    assert!(undo.edited_since >= 1, "{undo:?}");
    let job =
        o.s.rig
            .world
            .app
            .call(ImportUndo, &[&done.id])
            .token(&ada.token)
            .body(&ImportUndoBody::default())
            .send()
            .await;
    let job = o.ended(ada, job.ok().id).await;
    assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);
    let after =
        o.s.rig
            .world
            .app
            .call(DocumentGet, &[&made_document])
            .token(&dan.token)
            .send()
            .await;
    assert_eq!(
        after.status, 200,
        "his edit is not taken back with the import"
    );
}

#[tokio::test]
async fn undo_waits_for_an_in_flight_edit_before_deciding_whether_to_keep_the_document() {
    let o = Office::new().await;
    let (ada, dan) = (&o.s.ada, &o.s.dan);
    let project = o.s.tree.project.base.id;
    let file = upload(&o, ada, "topic.zip", &package_of(&o, o.topic()).await).await;
    let done = imported(&o, ada, project, &asked("concurrent", &file)).await;
    let rows = report(&o, ada, done.id).await;
    let document = of_kind(&rows, PackageKind::Document)[0]
        .made
        .as_ref()
        .unwrap()
        .id;
    o.s.rig.world.advance(chrono::Duration::minutes(5));

    let mut writer = o.s.rig.world.db.pool.begin().await.unwrap();
    let writer_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *writer)
        .await
        .unwrap();
    sqlx::query("UPDATE documents SET updated_by=$2, updated_at=$3 WHERE id=$1")
        .bind(document)
        .bind(dan.id())
        .bind(o.s.rig.world.now())
        .execute(&mut *writer)
        .await
        .unwrap();
    sqlx::query("UPDATE document_blocks SET text=$2, content=$3, updated_by=$4, updated_at=$5
        WHERE id=(SELECT id FROM document_blocks WHERE document_id=$1 AND type='paragraph' LIMIT 1)")
        .bind(document).bind("Dan's new words")
        .bind(json!([{ "type": "text", "text": "Dan's new words" }]))
        .bind(dan.id()).bind(o.s.rig.world.now())
        .execute(&mut *writer).await.unwrap();
    let job =
        o.s.rig
            .world
            .app
            .call(ImportUndo, &[&done.id])
            .token(&ada.token)
            .body(&ImportUndoBody::default())
            .send()
            .await
            .ok();
    rig::eventually("Undo waits for the document writer", || async {
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)))")
            .bind(writer_pid).fetch_one(&o.s.rig.world.db.pool).await.unwrap()
    }).await;
    writer.commit().await.unwrap();
    let job = o.ended(ada, job.id).await;
    assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);
    o.s.rig
        .world
        .app
        .call(DocumentGet, &[&document])
        .token(&dan.token)
        .send()
        .await
        .ok();
    let words: String = sqlx::query_scalar(
        "SELECT text FROM document_blocks WHERE document_id=$1 AND text=$2 AND removed_at IS NULL",
    )
    .bind(document)
    .bind("Dan's new words")
    .fetch_one(&o.s.rig.world.db.pool)
    .await
    .unwrap();
    assert_eq!(words, "Dan's new words");
}

#[tokio::test]
async fn undo_keeps_an_imported_folder_when_a_new_document_commits_inside_it() {
    let o = Office::new().await;
    let (ada, dan) = (&o.s.ada, &o.s.dan);
    let requested = plain(&o, ada, "folder-race", &["notes/plan.md"]).await;
    let done = imported(&o, ada, o.topic(), &requested).await;
    let rows = report(&o, ada, done.id).await;
    let folder = of_kind(&rows, PackageKind::Folder)[0]
        .made
        .as_ref()
        .unwrap()
        .id;
    let imported_document = of_kind(&rows, PackageKind::Document)[0]
        .made
        .as_ref()
        .unwrap()
        .id;
    assert_eq!(
        o.s.rig
            .world
            .app
            .call(DocumentDelete, &[&imported_document])
            .token(&ada.token)
            .send()
            .await
            .status,
        204
    );

    let mut writer = o.s.rig.world.db.pool.begin().await.unwrap();
    let writer_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *writer)
        .await
        .unwrap();
    let new_document = new_id();
    sqlx::query("INSERT INTO documents (id,node_id,org_id,owner_id,parent_container_id,title,created_by,updated_by,created_at,updated_at)
        SELECT $1,node_id,org_id,$2,id,'New work',$2,$2,$3,$3 FROM document_containers WHERE id=$4")
        .bind(new_document).bind(dan.id()).bind(o.s.rig.world.now()).bind(folder)
        .execute(&mut *writer).await.unwrap();
    let job =
        o.s.rig
            .world
            .app
            .call(ImportUndo, &[&done.id])
            .token(&ada.token)
            .body(&ImportUndoBody::default())
            .send()
            .await
            .ok();
    rig::eventually("Undo waits for the folder writer", || async {
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)))")
            .bind(writer_pid).fetch_one(&o.s.rig.world.db.pool).await.unwrap()
    }).await;
    writer.commit().await.unwrap();
    let job = o.ended(ada, job.id).await;
    assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);
    let held: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM document_containers WHERE id=$1 AND deleted_at IS NULL)
        AND EXISTS(SELECT 1 FROM documents WHERE id=$2 AND deleted_at IS NULL)",
    )
    .bind(folder)
    .bind(new_document)
    .fetch_one(&o.s.rig.world.db.pool)
    .await
    .unwrap();
    assert!(held, "the new document and its folder remain visible");
}

#[tokio::test]
async fn undo_records_recovery_deadlines_for_its_places_documents_and_folders() {
    let o = Office::new().await;
    let ada = &o.s.ada;
    let file = upload(&o, ada, "topic.zip", &package_of(&o, o.topic()).await).await;
    let imported_place =
        imported(&o, ada, o.s.tree.project.base.id, &asked("retained", &file)).await;
    let rows = report(&o, ada, imported_place.id).await;
    let place = of_kind(&rows, PackageKind::Topic)[0]
        .made
        .as_ref()
        .unwrap()
        .id;
    let document = of_kind(&rows, PackageKind::Document)[0]
        .made
        .as_ref()
        .unwrap()
        .id;
    let preview =
        o.s.rig
            .world
            .app
            .call(ImportUndoPreflight, &[&imported_place.id])
            .token(&ada.token)
            .send()
            .await
            .ok();
    let job =
        o.s.rig
            .world
            .app
            .call(ImportUndo, &[&imported_place.id])
            .token(&ada.token)
            .body(&ImportUndoBody::default())
            .send()
            .await
            .ok();
    assert_eq!(o.ended(ada, job.id).await.state, JobState::Succeeded);

    let request = plain(&o, ada, "retained-folder", &["notes/plan.md"]).await;
    let imported_folder = imported(&o, ada, o.topic(), &request).await;
    let rows = report(&o, ada, imported_folder.id).await;
    let folder = of_kind(&rows, PackageKind::Folder)[0]
        .made
        .as_ref()
        .unwrap()
        .id;
    let nested = of_kind(&rows, PackageKind::Document)[0]
        .made
        .as_ref()
        .unwrap()
        .id;
    let job =
        o.s.rig
            .world
            .app
            .call(ImportUndo, &[&imported_folder.id])
            .token(&ada.token)
            .body(&ImportUndoBody::default())
            .send()
            .await
            .ok();
    assert_eq!(o.ended(ada, job.id).await.state, JobState::Succeeded);

    for (kind, id) in [
        ("node", place),
        ("document", document),
        ("document_container", folder),
        ("document", nested),
    ] {
        let deadline: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
            "SELECT purge_at FROM audit_object_state WHERE object_kind=$1 AND object_id=$2",
        )
        .bind(kind)
        .bind(id)
        .fetch_one(&o.s.rig.world.db.pool)
        .await
        .unwrap();
        assert_eq!(deadline, Some(preview.restore_until.get()), "{kind} {id}");
    }
    o.s.rig.world.advance(chrono::Duration::days(2));
    let mut tx = o.s.rig.world.db.pool.begin().await.unwrap();
    orch_db::queries::notifications::upkeep::purge_soft_deleted(
        &mut tx,
        o.s.rig.world.now() - chrono::Duration::days(1),
        o.s.rig.world.now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let held: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM nodes WHERE id=$1)
        AND EXISTS(SELECT 1 FROM documents WHERE id=$2)
        AND EXISTS(SELECT 1 FROM document_containers WHERE id=$3)
        AND EXISTS(SELECT 1 FROM documents WHERE id=$4)",
    )
    .bind(place)
    .bind(document)
    .bind(folder)
    .bind(nested)
    .fetch_one(&o.s.rig.world.db.pool)
    .await
    .unwrap();
    assert!(
        held,
        "a shorter current interval cannot take the promised recovery time away"
    );
}

#[tokio::test]
async fn undo_keeps_an_imported_place_when_a_new_child_commits_inside_it() {
    let o = Office::new().await;
    let (ada, dan) = (&o.s.ada, &o.s.dan);
    let file = upload(&o, ada, "topic.zip", &package_of(&o, o.topic()).await).await;
    let done = imported(
        &o,
        ada,
        o.s.tree.project.base.id,
        &asked("place-race", &file),
    )
    .await;
    let rows = report(&o, ada, done.id).await;
    let topic = of_kind(&rows, PackageKind::Topic)[0]
        .made
        .as_ref()
        .unwrap()
        .id;

    // Inserting the new thread holds its parent node until the transaction commits.
    let mut writer = o.s.rig.world.db.pool.begin().await.unwrap();
    let writer_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *writer)
        .await
        .unwrap();
    let thread = new_id();
    orch_db::queries::hierarchy::testing::insert_node(
        &mut writer,
        thread,
        "thread",
        Some(topic),
        "New work",
        dan.id(),
    )
    .await
    .unwrap();
    let job =
        o.s.rig
            .world
            .app
            .call(ImportUndo, &[&done.id])
            .token(&ada.token)
            .body(&ImportUndoBody::default())
            .send()
            .await
            .ok();
    rig::eventually("Undo waits for the new child's parent", || async {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
            WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid))
              AND query='SELECT id FROM nodes WHERE id = $1 FOR UPDATE')",
        )
        .bind(writer_pid)
        .fetch_one(&o.s.rig.world.db.pool)
        .await
        .unwrap()
    })
    .await;
    writer.commit().await.unwrap();
    let job = o.ended(ada, job.id).await;
    assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);
    let held: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM nodes WHERE id=$1 AND deleted_at IS NULL)
        AND EXISTS(SELECT 1 FROM nodes WHERE id=$2 AND deleted_at IS NULL)",
    )
    .bind(topic)
    .bind(thread)
    .fetch_one(&o.s.rig.world.db.pool)
    .await
    .unwrap();
    assert!(
        held,
        "the new thread and its containing topic remain visible"
    );
}

/// Plain Markdown files named `names`, as a request for the topic of the scene.
async fn plain(o: &Office, by: &Who, client: &str, names: &[&str]) -> ImportRequest {
    let mut files = Vec::new();
    for name in names {
        files.push(upload(o, by, name, format!("# {name}\n\nWords.\n").as_bytes()).await);
    }
    ImportRequest {
        client_id: client.to_owned(),
        source: ImportSource { files, mode: None },
        destination: Default::default(),
        options: ImportOptions::default(),
        choices: Default::default(),
    }
}

#[tokio::test]
async fn a_folder_the_person_may_not_add_to_is_no_folder_of_this_place_to_them_and_one_they_may_is()
{
    use orch_api_contract::access::{
        AccessChoice, HolderInput, PageChange, PageChangeName, PageChangeWhole,
        PermissionPageUpdate,
    };
    let o = Office::new().await;
    let (ada, cai, dan) = (&o.s.ada, &o.s.cai, &o.s.dan);
    let topic = o.topic();
    // Ada brings a folder of notes in: the report names the folder that was made.
    let first = imported(
        &o,
        ada,
        topic,
        &plain(&o, ada, "folder", &["notes/plan.md"]).await,
    )
    .await;
    let rows = report(&o, ada, first.id).await;
    let folder = of_kind(&rows, PackageKind::Folder)[0]
        .made
        .as_ref()
        .unwrap()
        .id;

    // The shipped viewer role can create documents; set up a read-only share explicitly.
    o.s.rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"document-containers", &folder])
        .token(&ada.token)
        .body(&PageChange {
            whole: Some(PageChangeWhole::Private),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    o.s.rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"document-containers", &folder])
        .token(&ada.token)
        .body(&PageChange {
            name: Some(vec![
                PageChangeName {
                    holders: vec![HolderInput::User { id: cai.id() }],
                    choice: AccessChoice::FullView,
                    options: None,
                    expires_at: None,
                    note: None,
                    export: None,
                },
                PageChangeName {
                    holders: vec![HolderInput::User { id: dan.id() }],
                    choice: AccessChoice::FullInteract,
                    options: None,
                    expires_at: None,
                    note: None,
                    export: None,
                },
            ]),
            ..Default::default()
        })
        .send()
        .await
        .ok();

    // Cai may read but not add: the folder is not one he may bring files into, with the answer that a folder that is not there gets.
    let mut asked_by_cai = plain(&o, cai, "cai", &["more.md"]).await;
    asked_by_cai.destination.folder = Some(folder);
    let refused = plan(&o, cai, topic, &asked_by_cai).await;
    let mut ghost = asked_by_cai.clone();
    ghost.destination.folder = Some(new_id());
    let missing = plan(&o, cai, topic, &ghost).await;
    assert_eq!(refused.status, missing.status);
    assert_eq!(
        refused.problem().detail,
        missing.problem().detail,
        "a folder he may not use answers as one that is not there"
    );

    // Dan may.
    let mut asked_by_dan = plain(&o, dan, "dan", &["more.md"]).await;
    asked_by_dan.destination.folder = Some(folder);
    let allowed = plan(&o, dan, topic, &asked_by_dan).await;
    assert_eq!(allowed.status, 200, "{:?}", allowed.body);
}

#[tokio::test]
async fn a_document_the_person_cannot_see_is_not_told_of_by_a_name_that_is_the_same_and_does_not_stop_the_import()
 {
    let o = Office::new().await;
    let (ada, cai, dan) = (&o.s.ada, &o.s.cai, &o.s.dan);
    let topic = o.topic();
    document_beside(&o, ada, topic, "Secret", true).await;

    // Ada sees her own document and the plan says it is there; Dan, who cannot see it, is told nothing of it.
    let seen_by_ada = plan(&o, ada, topic, &plain(&o, ada, "ada", &["Secret.md"]).await)
        .await
        .ok();
    assert!(
        seen_by_ada
            .objects
            .items
            .iter()
            .any(|object| object.existing.is_some()),
        "she is told of what she can see: {:?}",
        seen_by_ada.objects.items
    );
    let seen_by_dan = plan(&o, dan, topic, &plain(&o, dan, "dan", &["Secret.md"]).await)
        .await
        .ok();
    assert!(
        seen_by_dan
            .objects
            .items
            .iter()
            .all(|object| object.existing.is_none()),
        "he is told nothing of what he cannot see: {:?}",
        seen_by_dan.objects.items
    );
    let _ = cai;

    // His import is made, under a name of its own.
    let done = imported(&o, dan, topic, &plain(&o, dan, "dan", &["Secret.md"]).await).await;
    assert_eq!(done.state, ImportState::Done, "{:?}", done.failure);
    let rows = report(&o, dan, done.id).await;
    let document = of_kind(&rows, PackageKind::Document)[0];
    assert_eq!(
        document.outcome,
        ImportOutcome::Created,
        "{:?}",
        document.message
    );
}

#[tokio::test]
async fn a_zip_among_other_files_is_a_file_like_the_others_and_not_an_archive_to_unpack() {
    let o = Office::new().await;
    let ada = &o.s.ada;
    let topic = o.topic();
    let notes = upload(&o, ada, "notes.md", b"# Notes\n\nSee the attachment.\n").await;
    let attachment = upload(&o, ada, "attachment.zip", &a_zip()).await;
    let request = ImportRequest {
        client_id: "mixed".to_owned(),
        source: ImportSource {
            files: vec![notes, attachment],
            mode: None,
        },
        destination: Default::default(),
        options: ImportOptions::default(),
        choices: Default::default(),
    };
    let dry = plan(&o, ada, topic, &request).await;
    assert_eq!(dry.status, 200, "{:?}", dry.body);
    let dry = dry.ok();
    assert!(dry.blockers.is_empty(), "{:?}", dry.blockers);
    assert_eq!(dry.counts.documents, 1);
}
