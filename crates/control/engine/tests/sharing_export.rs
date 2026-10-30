//! What taking a thing away asks of sharing with agents: a copy is the thing taken away as a file, so whoever makes it holds the Export row; a live share names the agents in the Export rows as far as whoever shares holds them, and an agent that is not named there reads what is shared and does not copy it into its workspace.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use orch_api_contract::share::{
    ShareParts, ShareTo, ShareToThread, ShareWhat, ShareWhatDocument, ShareWithAgents,
};
use orch_api_contract::threads::ShareMode;
use orch_authz::Caller;
use orch_db::audit::Origin;
use orch_db::queries::access::{PageKey, testing as access_testing};
use orch_engine::sharing::{Record, ShareError};
use orch_link_contract::commands::OrchTool;
use orch_types::Id;
use serde_json::json;
use shared::*;
use std::sync::Arc;

fn ask(
    document: Id,
    mode: ShareMode,
    to: Id,
    download: Option<bool>,
    client: &str,
) -> ShareWithAgents {
    ShareWithAgents {
        read_first: None,
        download,
        what: ShareWhat::Document(ShareWhatDocument {
            document_id: document,
        }),
        inside: None,
        parts: None::<ShareParts>,
        mode,
        to: ShareTo::Threads(vec![ShareToThread { thread_id: to }]),
        comment: None,
        expires_at: None,
        client_id: client.to_owned(),
    }
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

struct Place {
    world: common::World,
    /// The thread whose agent is given things.
    reader: Id,
    /// A document that only Ada and Bea hold, Bea in the row that lets her read it and in none that lets her take it
    /// away.
    private: Id,
    /// A document beside the topic that follows the page of the world, where Ada owns and everybody who may read
    /// holds the Export row as the roles ship it.
    open: Id,
    bea: Id,
    parcels: Arc<MemoryParcels>,
}

async fn a_reader_who_may_not_take_it_away() -> Place {
    let world = a_world_whose_agent_takes_tools().await;
    let reader = world.thread("Reader").await;
    let topic = world.topic("Docs").await;
    let private = world
        .document_beside(topic, "Private", &["Only to read."])
        .await;
    let open = world
        .document_beside(topic, "Open", &["For everyone."])
        .await;
    let bea = world.person("bea").await;
    {
        // A developer drives the threads of the org, so that Bea may share with the agent of the reader; the document
        // is hers to read by its page alone, which no role reaches.
        let mut conn = world.pool.acquire().await.unwrap();
        access_testing::give_role(
            &mut conn,
            world.root,
            "user",
            bea,
            "developer",
            world.ada,
            None,
        )
        .await
        .unwrap();
    }
    make_private(&world, private).await;
    {
        let mut conn = world.pool.acquire().await.unwrap();
        access_testing::add_entry(
            &mut conn,
            &PageKey::new("document", private, "live"),
            "any",
            "doc.view",
            ("user", Some(bea), None),
            None,
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    }
    let parcels = Arc::new(MemoryParcels::default());
    world.engine().attach_parcels(parcels.clone());
    Place {
        world,
        reader,
        private,
        open,
        bea,
        parcels,
    }
}

fn names(record: &Record) -> Vec<&str> {
    record
        .grants
        .iter()
        .map(|grant| grant.permission.as_str())
        .collect()
}

async fn download(place: &Place, document: Id) -> orch_link_contract::commands::OrchToolResult {
    call(
        &place.world,
        place.reader,
        OrchTool::DownloadShared,
        json!({ "path": format!("id:{document}") }),
    )
    .await
}

#[tokio::test]
async fn a_copy_is_the_thing_taken_away_and_asks_for_the_export_row_of_whoever_makes_it() {
    let p = a_reader_who_may_not_take_it_away().await;
    // Bea reads the document, and a copy of it is not hers to give.
    let refused = share_as(
        &p.world,
        p.bea,
        &ask(p.private, ShareMode::Snapshot, p.reader, None, "c1"),
    )
    .await;
    assert!(matches!(refused, Err(ShareError::Denied(_))), "{refused:?}");
    assert!(
        p.parcels.kept.lock().unwrap().is_empty(),
        "nothing was kept"
    );
    // Ada holds the row: the same copy is made.
    let made = share_as(
        &p.world,
        p.world.ada,
        &ask(p.private, ShareMode::Snapshot, p.reader, None, "c2"),
    )
    .await
    .unwrap();
    assert_eq!(made.len(), 1);
    assert_eq!(p.parcels.kept.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_live_share_names_the_agents_in_the_export_rows_as_far_as_the_giver_holds_them() {
    let p = a_reader_who_may_not_take_it_away().await;
    // By default the agents may read and copy what Ada shares.
    let shared = share_as(
        &p.world,
        p.world.ada,
        &ask(p.open, ShareMode::Live, p.reader, None, "a1"),
    )
    .await
    .unwrap();
    let rows = names(&shared[0]);
    assert!(
        rows.contains(&"doc.view") && rows.contains(&"doc.export"),
        "{rows:?}"
    );
    let copied = ok(download(&p, p.open).await);
    assert_eq!(copied["status"], "ready");

    // Unticked, they read it and are not let copy it, and are told so.
    let reading = share_as(
        &p.world,
        p.world.ada,
        &ask(p.private, ShareMode::Live, p.reader, Some(false), "a2"),
    )
    .await
    .unwrap();
    assert_eq!(names(&reading[0]), ["doc.view"]);
    let read = ok(call(
        &p.world,
        p.reader,
        OrchTool::ReadShared,
        json!({ "path": format!("id:{}", p.private) }),
    )
    .await);
    assert!(read["text"].as_str().unwrap().contains("Only to read."));
    let refusal = download(&p, p.private).await;
    let (code, message) = refused(&refusal);
    assert_eq!(code, "not_downloadable");
    assert!(message.contains("read_shared"), "{message}");

    // Bea does not hold the Export row of her document: what she shares names the agents in no row of it, though
    // she asked for it to be copyable, and the agents do not copy it.
    let by_bea = share_as(
        &p.world,
        p.bea,
        &ask(p.private, ShareMode::Live, p.reader, Some(true), "a3"),
    )
    .await;
    match by_bea {
        Ok(records) => {
            assert!(
                !names(&records[0]).contains(&"doc.export"),
                "nobody gives what they do not hold: {:?}",
                names(&records[0])
            );
            assert_eq!(
                refused(&download(&p, p.private).await).0,
                "not_downloadable"
            );
        }
        Err(error) => panic!("sharing to read is hers to do: {error:?}"),
    }
}
