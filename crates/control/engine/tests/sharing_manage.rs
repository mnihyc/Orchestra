//! Sharing documents with agents to manage: given only by somebody who holds `doc.manage` there, written in the rows of the documents and of the place like any share, of a thing, of a place with what is in it or of a place by itself, changed and taken back like any share; and what the agents can then do with it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use orch_api_contract::share::{
    ShareAgentUpdateBody, ShareDocumentsLevel, ShareParts, ShareTo, ShareToNode, ShareToThread,
    ShareWhat, ShareWhatDocument, ShareWhatNode, ShareWithAgents,
};
use orch_api_contract::threads::ShareMode;
use orch_authz::Caller;
use orch_db::audit::Origin;
use orch_engine::sharing::{Record, ShareError};
use orch_link_contract::commands::OrchTool;
use orch_types::Id;
use serde_json::{Value, json};
use shared::*;

struct Place {
    world: common::World,
    reader: Id,
    topic: Id,
}

async fn a_reader_and_documents() -> Place {
    let world = a_world_whose_agent_takes_tools().await;
    let reader = world.thread("Reader").await;
    let topic = world.topic;
    Place {
        world,
        reader,
        topic,
    }
}

/// Cuts every row of a document's page loose, so that its owners alone hold it.
async fn make_private(world: &common::World, document: Id) {
    use orch_access_contract::decisions::ObjectRef;
    use orch_access_contract::pages::{PageChange, Whole};
    use orch_access_contract::permissions::ObjectKind;
    let mut conn = world.pool.acquire().await.unwrap();
    let principal = orch_authz::Principal::load(
        &mut conn,
        Caller::session(world.ada),
        orch_types::Time::new(chrono::Utc::now()),
    )
    .await
    .unwrap();
    orch_authz::pages::change(
        &mut conn,
        &principal,
        orch_authz::pages::Target::Resource(ObjectRef {
            kind: ObjectKind::Document,
            id: document,
        }),
        &PageChange {
            whole: Some(Whole::Private),
            ..PageChange::default()
        },
        orch_authz::Policy::default(),
        &Origin::default(),
    )
    .await
    .unwrap();
}

fn to_manage() -> Option<ShareParts> {
    documents(ShareDocumentsLevel::Manage)
}

fn documents(level: ShareDocumentsLevel) -> Option<ShareParts> {
    Some(ShareParts {
        documents: Some(level),
        ..ShareParts::default()
    })
}

fn ask(what: ShareWhat, parts: Option<ShareParts>, to: ShareTo, client: &str) -> ShareWithAgents {
    ShareWithAgents {
        read_first: None,
        what,
        inside: None,
        parts,
        mode: ShareMode::Live,
        to,
        comment: None,
        expires_at: None,
        client_id: client.to_owned(),
        download: None,
    }
}

fn reader_agents(reader: Id) -> ShareTo {
    ShareTo::Threads(vec![ShareToThread { thread_id: reader }])
}

fn node(id: Id) -> ShareWhat {
    ShareWhat::Node(ShareWhatNode { node_id: id })
}

fn document(id: Id) -> ShareWhat {
    ShareWhat::Document(ShareWhatDocument { document_id: id })
}

async fn share_as(
    world: &common::World,
    person: Id,
    request: &ShareWithAgents,
) -> Result<Vec<Record>, ShareError> {
    world
        .engine()
        .share_with_agents(&Caller::session(person), &Origin::default(), request)
        .await
}

/// The rows an agent is named in, by page and permission, as they stand.
async fn rows(world: &common::World, thing: Id) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = sqlx::query_as(
        "SELECT page, permission FROM permission_entries
          WHERE object_id = $1 AND holder_kind = 'agents' AND removed_at IS NULL ORDER BY page, permission",
    )
    .bind(thing)
    .fetch_all(&mut *world.pool.acquire().await.unwrap())
    .await
    .unwrap();
    found.sort();
    found
}

fn row(page: &str, permission: &str) -> (String, String) {
    (page.to_owned(), permission.to_owned())
}

#[tokio::test]
async fn a_place_shared_to_manage_names_the_agents_in_the_three_rows_of_the_place_and_of_what_is_inside_it()
 {
    let p = a_reader_and_documents().await;
    let made = share_as(
        &p.world,
        p.world.ada,
        &ask(node(p.topic), to_manage(), reader_agents(p.reader), "m1"),
    )
    .await
    .unwrap();
    assert_eq!(made.len(), 1);
    assert_eq!(made[0].parts.documents, Some(ShareDocumentsLevel::Manage));
    assert!(made[0].inside, "the place with what is in it");
    assert_eq!(
        rows(&p.world, p.topic).await,
        [
            row("defaults", "doc.edit"),
            row("defaults", "doc.export"),
            row("defaults", "doc.manage"),
            row("defaults", "doc.view"),
            row("live", "doc.edit"),
            row("live", "doc.export"),
            row("live", "doc.manage"),
            row("live", "doc.view"),
        ],
        "managing is the three rows, on the place's own page and on what it gives to what is made in it"
    );
}

#[tokio::test]
async fn a_place_shared_by_itself_to_manage_is_the_rows_of_its_own_page_alone() {
    let p = a_reader_and_documents().await;
    let mut request = ask(node(p.topic), to_manage(), reader_agents(p.reader), "m1");
    request.inside = Some(false);
    let made = share_as(&p.world, p.world.ada, &request).await.unwrap();
    assert!(!made[0].inside);
    assert_eq!(made[0].parts.documents, Some(ShareDocumentsLevel::Manage));
    assert_eq!(
        rows(&p.world, p.topic).await,
        [
            row("live", "doc.edit"),
            row("live", "doc.export"),
            row("live", "doc.manage"),
            row("live", "doc.view"),
        ],
        "nothing for what lies inside it"
    );
}

#[tokio::test]
async fn one_document_shared_to_manage_is_its_own_three_rows_and_changed_and_taken_back_like_any_share()
 {
    let p = a_reader_and_documents().await;
    let doc = p
        .world
        .document_beside(p.topic, "Plan", &["The plan."])
        .await;
    let made = share_as(
        &p.world,
        p.world.ada,
        &ask(document(doc), to_manage(), reader_agents(p.reader), "m1"),
    )
    .await
    .unwrap();
    let share = made[0].clone();
    assert_eq!(share.parts.documents, Some(ShareDocumentsLevel::Manage));
    assert_eq!(
        rows(&p.world, doc).await.len(),
        4,
        "View, Edit and Manage, and the Export row that travels with them"
    );
    // Changed to editing: the right to manage is taken out, and the rest stays.
    let caller = Caller::session(p.world.ada);
    let edit = p
        .world
        .engine()
        .update_share(
            &caller,
            &Origin::default(),
            share.id,
            &ShareAgentUpdateBody {
                read_first: None,
                parts: documents(ShareDocumentsLevel::Edit),
                expires_at: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(edit.parts.documents, Some(ShareDocumentsLevel::Edit));
    assert_eq!(
        rows(&p.world, doc).await,
        [
            row("live", "doc.edit"),
            row("live", "doc.export"),
            row("live", "doc.view")
        ]
    );
    // And back, and then taken back whole.
    let again = p
        .world
        .engine()
        .update_share(
            &caller,
            &Origin::default(),
            share.id,
            &ShareAgentUpdateBody {
                read_first: None,
                parts: to_manage(),
                expires_at: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(again.parts.documents, Some(ShareDocumentsLevel::Manage));
    let listed = p
        .world
        .engine()
        .shares_of(
            &caller,
            &Origin::default(),
            orch_engine::sharing::Subject::Document(doc),
        )
        .await
        .unwrap();
    assert_eq!(listed.len(), 1, "it is listed on the document's page");
    assert_eq!(listed[0].parts.documents, Some(ShareDocumentsLevel::Manage));
    p.world
        .engine()
        .revoke_share(&caller, &Origin::default(), share.id)
        .await
        .unwrap();
    assert!(rows(&p.world, doc).await.is_empty());
}

#[tokio::test]
async fn a_share_to_manage_is_given_by_somebody_who_holds_doc_manage_and_refused_whole_to_somebody_who_does_not()
 {
    let p = a_reader_and_documents().await;
    let doc = p
        .world
        .document_beside(p.topic, "Plan", &["The plan."])
        .await;
    // An Interactor edits documents here and does not manage them; a Developer does both.
    let tom = p.world.person("tom").await;
    p.world.give(tom, "interactor").await;
    let dan = p.world.person("dan").await;
    p.world.give(dan, "developer").await;
    let editing = share_as(
        &p.world,
        tom,
        &ask(
            document(doc),
            documents(ShareDocumentsLevel::Edit),
            reader_agents(p.reader),
            "t-edit",
        ),
    )
    .await;
    assert!(editing.is_ok(), "{editing:?}: what he holds he gives");
    let managing = share_as(
        &p.world,
        tom,
        &ask(
            document(doc),
            to_manage(),
            reader_agents(p.reader),
            "t-manage",
        ),
    )
    .await;
    match managing {
        Err(ShareError::Denied(refusal)) => {
            let lacking: Vec<String> = refusal
                .missing
                .iter()
                .flat_map(|each| each.permissions.iter().map(|p| p.as_str().to_owned()))
                .collect();
            assert!(
                lacking.iter().any(|name| name == "doc.manage")
                    || refusal.required.as_str() == "doc.manage",
                "the refusal says what he lacks: {refusal:?}"
            );
        }
        other => panic!("a refusal was expected: {other:?}"),
    }
    assert_eq!(
        rows(&p.world, doc).await,
        [
            row("live", "doc.edit"),
            row("live", "doc.export"),
            row("live", "doc.view")
        ],
        "refused whole: nothing of the managing share was written"
    );
    let managing = share_as(
        &p.world,
        dan,
        &ask(
            document(doc),
            to_manage(),
            reader_agents(p.reader),
            "d-manage",
        ),
    )
    .await;
    assert!(
        managing.is_ok(),
        "{managing:?}: a Developer manages documents as shipped"
    );
    assert_eq!(
        rows(&p.world, doc).await.len(),
        4,
        "View, Edit and Manage, and the Export row that travels with them"
    );
}

#[tokio::test]
async fn a_copy_holds_no_manage_and_the_thread_is_told_what_it_was_given() {
    let p = a_reader_and_documents().await;
    let doc = p
        .world
        .document_beside(p.topic, "Plan", &["The plan."])
        .await;
    let mut copy = ask(document(doc), to_manage(), reader_agents(p.reader), "copy");
    copy.mode = ShareMode::Snapshot;
    match share_as(&p.world, p.world.ada, &copy).await {
        Err(ShareError::Invalid { field, message }) => {
            assert_eq!(field, "parts");
            assert!(message.contains("no manage"), "{message}");
        }
        other => panic!("a copy cannot be made to manage: {other:?}"),
    }
    share_as(
        &p.world,
        p.world.ada,
        &ask(document(doc), to_manage(), reader_agents(p.reader), "m1"),
    )
    .await
    .unwrap();
    let notices: Value = p
        .world
        .scalar(
            "SELECT pending_notices FROM threads WHERE node_id = $1",
            p.reader,
        )
        .await;
    let body = notices[0]["body"].as_str().unwrap();
    assert!(
        body.contains("also for writing") && body.contains("manage_shared"),
        "{body}"
    );
}

#[tokio::test]
async fn a_project_shared_to_manage_reaches_the_documents_in_the_topics_in_it_and_not_one_made_private()
 {
    let p = a_reader_and_documents().await;
    let open = p.world.document_beside(p.topic, "Open", &["Open."]).await;
    let closed = p
        .world
        .document_beside(p.topic, "Closed", &["Closed."])
        .await;
    share_as(
        &p.world,
        p.world.ada,
        &ask(
            node(p.world.project),
            to_manage(),
            reader_agents(p.reader),
            "m1",
        ),
    )
    .await
    .unwrap();
    // Cut loose from the place's rows, a document is private and is left out unless it was shared itself.
    make_private(&p.world, closed).await;
    let listing = ok(call(
        &p.world,
        p.reader,
        OrchTool::ListShared,
        json!({ "path": format!("id:{}", p.topic) }),
    )
    .await);
    let docs = entry(&listing, "docs")["path"].as_str().unwrap().to_owned();
    let named = ok(call(
        &p.world,
        p.reader,
        OrchTool::ListShared,
        json!({ "path": docs }),
    )
    .await);
    assert_eq!(entry(&named, "Open.md")["manageable"], true);
    let plan = ok(call(
        &p.world,
        p.reader,
        OrchTool::ManageShared,
        json!({ "command": "delete", "paths": [format!("{docs}Open.md")] }),
    )
    .await);
    assert_eq!(plan["affects"][0]["id"], json!(open));
}

#[tokio::test]
async fn a_project_shared_by_itself_to_manage_reaches_the_documents_beside_it_and_none_inside() {
    let p = a_reader_and_documents().await;
    // Beside another topic of the project: the reader's own topic gives its agents its public documents as
    // shipped, and only the share could reach this one.
    let elsewhere = p.world.topic("Elsewhere").await;
    let inside = p
        .world
        .document_beside(elsewhere, "Inside", &["Inside."])
        .await;
    let beside = p
        .world
        .document_beside(p.world.project, "Beside", &["Beside."])
        .await;
    let mut request = ask(
        node(p.world.project),
        to_manage(),
        reader_agents(p.reader),
        "m1",
    );
    request.inside = Some(false);
    share_as(&p.world, p.world.ada, &request).await.unwrap();
    let listing = ok(call(
        &p.world,
        p.reader,
        OrchTool::ListShared,
        json!({ "path": format!("id:{}", p.world.project) }),
    )
    .await);
    let docs = entry(&listing, "docs")["path"].as_str().unwrap().to_owned();
    let named = ok(call(
        &p.world,
        p.reader,
        OrchTool::ListShared,
        json!({ "path": docs }),
    )
    .await);
    assert_eq!(entry(&named, "Beside.md")["manageable"], true);
    let plan = ok(call(
        &p.world,
        p.reader,
        OrchTool::ManageShared,
        json!({ "command": "delete", "paths": [format!("{docs}Beside.md")] }),
    )
    .await);
    assert_eq!(plan["affects"][0]["id"], json!(beside));
    let hidden = call(
        &p.world,
        p.reader,
        OrchTool::ManageShared,
        json!({ "command": "delete", "paths": [format!("id:{inside}")] }),
    )
    .await;
    assert_eq!(
        refused(&hidden).0,
        "not_found",
        "what lies inside the place is not in the share of the place itself"
    );
}

#[tokio::test]
async fn a_place_made_later_under_one_shared_to_manage_is_managed_from_the_first_call() {
    let p = a_reader_and_documents().await;
    share_as(
        &p.world,
        p.world.ada,
        &ask(
            ShareWhat::Node(ShareWhatNode {
                node_id: p.world.project,
            }),
            to_manage(),
            ShareTo::Nodes(vec![ShareToNode {
                node_id: p.world.root,
            }]),
            "m1",
        ),
    )
    .await
    .unwrap();
    // The agents of the whole org are named, and the topic and its document are made afterwards.
    let later = p.world.topic("Later").await;
    let doc = p.world.document_beside(later, "Fresh", &["Fresh."]).await;
    let listing = ok(call(
        &p.world,
        p.reader,
        OrchTool::ListShared,
        json!({ "path": format!("id:{later}") }),
    )
    .await);
    let docs = entry(&listing, "docs")["path"].as_str().unwrap().to_owned();
    let plan = ok(call(
        &p.world,
        p.reader,
        OrchTool::ManageShared,
        json!({ "command": "delete", "paths": [format!("{docs}Fresh.md")] }),
    )
    .await);
    assert_eq!(plan["affects"][0]["id"], json!(doc));
}
