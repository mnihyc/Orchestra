//! How many archives one person may have in the making, and what an archive that is not ready, was stopped or failed answers: with nothing running the jobs, so that an archive stays as it was made.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::common::{ErrorCode, Page, PageQuery};
use orch_api_contract::documents::{NodeDocumentCreate, NodeDocumentCreateBody};
use orch_api_contract::exports::{
    ExportDelete, ExportDownload, ExportEverything, ExportGet, ExportRequest, ExportState,
    ExportWhat, MeExportList, NodeExportCreate, NodeExportPreflight,
};
use orch_api_contract::workspace::{Job, JobGet};
use orch_api_core::testing::Reply;
use orch_config::ServerConfig;
use orch_types::Id;
use places::{Who, make_org, person};
use support::World;

fn everything() -> ExportRequest {
    ExportRequest {
        what: ExportWhat::Everything(ExportEverything::All),
        parts: None,
        format: None,
        detail: None,
        files: None,
        versions: None,
        comments: None,
        from: None,
        to: None,
        package: None,
    }
}

struct Place {
    world: World,
    ada: Who,
    bea: Who,
    org: Id,
}

/// A place with a document in it, where one person may have one archive in the making, and no job is run.
async fn place() -> Place {
    let mut config = ServerConfig::default();
    config.limits.exports_per_user = 1;
    place_with(config).await
}

/// The same under the settings given.
async fn place_with(config: ServerConfig) -> Place {
    let world = World::with(config).await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let org = make_org(&world, &ada, "Acme", "acme").await.base.id;
    world
        .app
        .call(NodeDocumentCreate, &[&org])
        .token(&ada.token)
        .body(&NodeDocumentCreateBody {
            title: "Handbook".to_owned(),
            blocks: None,
            markdown: Some("Be kind.\n".to_owned()),
            access: None,
        })
        .send()
        .await
        .ok();
    Place {
        world,
        ada,
        bea,
        org,
    }
}

impl Place {
    async fn ask(&self, by: &Who) -> Reply<Job> {
        self.world
            .app
            .call(NodeExportCreate, &[&self.org])
            .token(&by.token)
            .body(&everything())
            .send()
            .await
    }

    async fn list(&self, by: &Who) -> Page<orch_api_contract::exports::Export> {
        self.world
            .app
            .call(MeExportList, &[])
            .token(&by.token)
            .query(&PageQuery::default())
            .send()
            .await
            .ok()
    }
}

#[tokio::test]
async fn a_third_archive_at_a_time_is_refused_and_the_refusal_says_that_two_are_being_written() {
    // As shipped: two at a time.
    let p = place_with(ServerConfig::default()).await;
    assert_eq!(p.ask(&p.ada).await.status, 202);
    assert_eq!(p.ask(&p.ada).await.status, 202);
    let third = p.ask(&p.ada).await;
    assert_eq!(third.status, 429);
    let third = third.problem();
    assert_eq!(third.code, ErrorCode::RateLimited);
    // Its limit says that two are being written, as many as may be at once; the sentence is the
    // catalogue's for the code.
    assert_eq!(third.extra["limit"], 2);
    assert_eq!(
        third.detail,
        orch_api_contract::messages::for_code(ErrorCode::RateLimited)
    );
    assert_eq!(p.list(&p.ada).await.items.len(), 2, "and nothing was made");
}

#[tokio::test]
async fn one_person_has_as_many_archives_in_the_making_as_the_limit_says_and_removing_one_makes_room()
 {
    let p = place().await;
    let first = p.ask(&p.ada).await;
    assert_eq!(first.status, 202);
    let first = first.ok();
    // Asked again while the first is not written: refused with the limit, and nothing was made.
    let second = p.ask(&p.ada).await;
    assert_eq!(second.status, 429);
    let second = second.problem();
    assert_eq!(second.code, ErrorCode::RateLimited);
    assert_eq!(second.extra["limit"], 1);
    assert_eq!(p.list(&p.ada).await.items.len(), 1);

    // The limit is a person's own: Bea holds nothing in the place and is refused before she is counted, and she sees
    // nothing of Ada's.
    assert_eq!(p.ask(&p.bea).await.problem().code, ErrorCode::AuthzDenied);
    assert!(p.list(&p.bea).await.items.is_empty());

    // The one in the making is listed as pending, is not fetched, and is removed by its person: that stops its job.
    let listed = p.list(&p.ada).await.items;
    let (id, export) = (listed[0].id, &listed[0]);
    assert_eq!(export.state, ExportState::Pending);
    assert_eq!(
        (export.bytes, export.counts, export.ready_at),
        (None, None, None)
    );
    assert_eq!(export.job_id, Some(first.id));
    let read = p
        .world
        .app
        .call(ExportGet, &[&id])
        .token(&p.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(read.state, ExportState::Pending);
    let fetched = p
        .world
        .app
        .call(ExportDownload, &[&id])
        .token(&p.ada.token)
        .send_bytes()
        .await;
    assert_eq!(
        fetched.status, 404,
        "an archive that is not written is not there to fetch"
    );
    let removed = p
        .world
        .app
        .call(ExportDelete, &[&id])
        .token(&p.ada.token)
        .send_bytes()
        .await;
    assert_eq!(removed.status, 204);
    let stopped: bool = sqlx::query_scalar("SELECT plan ? 'cancel' FROM jobs WHERE id = $1")
        .bind(first.id)
        .fetch_one(&p.world.db.pool)
        .await
        .unwrap();
    assert!(stopped, "the job that was writing it is asked to stop");
    let job = p
        .world
        .app
        .call(JobGet, &[&first.id])
        .token(&p.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(job.kind, "export");
    assert_eq!(
        p.list(&p.ada).await.items[0].state,
        ExportState::Expired,
        "the row stays and says that it was removed"
    );
    let fetched = p
        .world
        .app
        .call(ExportDownload, &[&id])
        .token(&p.ada.token)
        .send_bytes()
        .await;
    assert_eq!(fetched.status, 410);

    // There is room again.
    let again = p.ask(&p.ada).await;
    assert_eq!(again.status, 202);
    let both = p.list(&p.ada).await.items;
    assert_eq!(both.len(), 2);
    assert_eq!(both[1].id, id, "newest first");

    // A page at a time, by the cursor the last gave; a cursor that is none is refused; an archive nobody made is
    // not there.
    let page = |query: PageQuery| {
        let call = p
            .world
            .app
            .call(MeExportList, &[])
            .token(&p.ada.token)
            .query(&query);
        async move { call.send().await }
    };
    let first = page(PageQuery {
        limit: Some(1),
        cursor: None,
    })
    .await
    .ok();
    assert_eq!(
        first.items.iter().map(|each| each.id).collect::<Vec<_>>(),
        [both[0].id]
    );
    let cursor = first.next_cursor.expect("one more waits");
    let second = page(PageQuery {
        limit: Some(1),
        cursor: Some(cursor),
    })
    .await
    .ok();
    assert_eq!(
        second.items.iter().map(|each| each.id).collect::<Vec<_>>(),
        [id]
    );
    assert!(second.next_cursor.is_none());
    let bad = page(PageQuery {
        limit: Some(1),
        cursor: Some("not a cursor".to_owned()),
    })
    .await
    .problem();
    assert_eq!(bad.code, ErrorCode::RequestInvalid);
    let unknown = p
        .world
        .app
        .call(ExportGet, &[&orch_types::new_id()])
        .token(&p.ada.token)
        .send()
        .await
        .problem();
    assert_eq!(unknown.status, 404, "an archive nobody made");
    let nowhere = p
        .world
        .app
        .call(NodeExportPreflight, &[&orch_types::new_id()])
        .token(&p.ada.token)
        .body(&everything())
        .send()
        .await
        .problem();
    assert_eq!(nowhere.status, 404, "a place that is not there");
}

#[tokio::test]
async fn an_archive_that_failed_is_gone_and_says_why_it_holds_nothing() {
    let p = place().await;
    let job = p.ask(&p.ada).await.ok();
    let id = p.list(&p.ada).await.items[0].id;
    // The job failed: its undoing marks the archive, as the engine does when a job ends badly.
    sqlx::query("UPDATE exports SET state = 'failed', left_out = '[]' WHERE id = $1")
        .bind(id)
        .execute(&p.world.db.pool)
        .await
        .unwrap();
    let read = p
        .world
        .app
        .call(ExportGet, &[&id])
        .token(&p.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(
        (read.state, read.job_id),
        (ExportState::Failed, Some(job.id))
    );
    let fetched = p
        .world
        .app
        .call(ExportDownload, &[&id])
        .token(&p.ada.token)
        .send_bytes()
        .await;
    assert_eq!(fetched.status, 410);
    let room = p.ask(&p.ada).await;
    assert_eq!(
        room.status, 202,
        "a failed archive is not one in the making"
    );
}
