//! Bounded native upload summaries and complete pages over existing per-file or legacy audit facts.
use super::*;
use orch_api_contract::access::{HistoryLine, HistoryList, HistoryListQuery};
use orch_api_contract::common::Page;
use orch_api_contract::workspace::EnvironmentUploadFinish;

async fn page(
    site: &Site,
    line: Option<Id>,
    cursor: Option<String>,
    limit: u32,
) -> Page<HistoryLine> {
    let call = site
        .app()
        .call(HistoryList, &[&"environments", &site.environment]);
    call.token(&site.ada.token)
        .query(&HistoryListQuery {
            upload: line,
            cursor,
            limit: Some(limit),
            ..Default::default()
        })
        .send()
        .await
        .ok()
}

async fn all_files(site: &Site, line: Id, expected: usize, limit: u32) {
    let mut cursor = None;
    let mut names = std::collections::BTreeSet::new();
    loop {
        let next = page(site, Some(line), cursor, limit).await;
        let mut emitted = 0;
        for row in next.items {
            let upload = row
                .upload
                .expect("the native page carries actual file outcomes");
            for file in upload.files {
                emitted += 1;
                assert!(
                    names.insert(file.name),
                    "an immutable file cannot reappear on a later page"
                );
                assert_eq!(file.received, Some(1));
            }
        }
        assert!(emitted <= usize::try_from(limit).unwrap());
        cursor = next.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(names.len(), expected);
}

#[tokio::test]
async fn large_upload_finish_keeps_one_small_summary_and_pages_all_indexed_file_receipts_once() {
    let site = live_site().await;
    let batch = new_id();
    let count = 1001_i64;
    sqlx::query("INSERT INTO audit_log(id,at,actor_kind,actor_id,action,history,object_kind,object_id,node_id,environment_id,summary,details)
        SELECT ('00000000-0000-4000-8000-' || lpad(to_hex(n),12,'0'))::uuid,$1,'user',$2,'workspace.file_written',true,
          'environment',$3,$4,$3,'Wrote one file',jsonb_build_object('path','bulk/file-'||n||'.txt','size',1,'hash','kept','replaced',false,'upload_batch',$5::text)
        FROM generate_series(1,$6::bigint) n")
        .bind(site.world.now()).bind(site.ada.id()).bind(site.environment).bind(site.topic).bind(batch.to_string()).bind(count)
        .execute(&site.world.db.pool).await.unwrap();
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM commands")
        .fetch_one(&site.world.db.pool)
        .await
        .unwrap();
    for _ in 0..2 {
        assert_eq!(
            site.app()
                .call(EnvironmentUploadFinish, &[&site.environment, &batch])
                .token(&site.ada.token)
                .raw_body(&json!({"folder":"bulk","outcome":"complete","files":[]}))
                .send()
                .await
                .status,
            StatusCode::NO_CONTENT
        );
    }
    let (line, stored): (Id, Json) = sqlx::query_as(
        "SELECT id,details FROM audit_log WHERE environment_id=$1 AND details #>> '{upload,id}'=$2",
    )
    .bind(site.environment)
    .bind(batch.to_string())
    .fetch_one(&site.world.db.pool)
    .await
    .unwrap();
    assert_eq!(stored["upload"]["file_count"], count);
    assert_eq!(stored["upload"]["files"].as_array().unwrap().len(), 2);
    assert_eq!(stored["upload"]["bytes"], count);
    assert!(
        serde_json::to_vec(&stored).unwrap().len() < 2048,
        "two short filename previews do not grow with the batch"
    );
    let summary = page(&site, None, None, 1).await;
    let upload = summary
        .items
        .iter()
        .find(|row| row.id == line)
        .unwrap()
        .upload
        .as_ref()
        .unwrap();
    assert_eq!(upload.file_count, Some(count));
    assert!(upload.files_paged);
    assert_eq!(upload.files.len(), 2);
    all_files(&site, line, usize::try_from(count).unwrap(), 37).await;
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM audit_log WHERE environment_id=$1 AND history AND details ? 'upload'")
        .bind(site.environment).fetch_one(&site.world.db.pool).await.unwrap(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM commands")
            .fetch_one(&site.world.db.pool)
            .await
            .unwrap(),
        before,
        "history projection never asks the machine to replay writes"
    );
}

#[tokio::test]
async fn legacy_grouped_upload_is_projected_and_paged_without_loading_or_rewriting_its_whole_api_body()
 {
    use orch_db::audit::{Actor, Entry, Origin, append};
    let site = live_site().await;
    let count = 509_usize;
    let files = (0..count).map(|n| json!({"name":format!("legacy-{n}.txt"),"path":format!("old/legacy-{n}.txt"),"size":1,"received":1,"outcome":"complete","replaced":false})).collect::<Vec<_>>();
    let mut entry = Entry::new(
        Actor::User(site.ada.id()),
        "workspace.file_written",
        "environment",
        Some(site.environment),
        "Uploaded legacy files",
    );
    entry.environment_id = Some(site.environment);
    entry.node_id = Some(site.topic);
    entry.history = true;
    entry.details = json!({"upload":{"id":new_id(),"environment_id":site.environment,"folder":"old","files":files,"bytes":count,"replaced":0,"outcome":"complete"}});
    let line = append(
        &mut *site.world.conn().await,
        &entry,
        &Origin::default(),
        site.world.now(),
    )
    .await
    .unwrap();
    let first = page(&site, None, None, 1).await;
    let preview = first
        .items
        .iter()
        .find(|row| row.id == line)
        .unwrap()
        .upload
        .as_ref()
        .unwrap();
    assert_eq!(preview.files.len(), 2);
    assert_eq!(preview.file_count, Some(i64::try_from(count).unwrap()));
    assert!(preview.files_paged);
    all_files(&site, line, count, 23).await;
    let stored: i64 = sqlx::query_scalar(
        "SELECT jsonb_array_length(details #> '{upload,files}')::bigint FROM audit_log WHERE id=$1",
    )
    .bind(line)
    .fetch_one(&site.world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        stored,
        i64::try_from(count).unwrap(),
        "stored audit content is never rewritten for a projection"
    );
    let failure = orch_db::queries::upload_history::page(
        &mut *site.world.conn().await,
        new_id(),
        line,
        None,
        None,
        24,
    )
    .await;
    assert!(
        matches!(
            failure,
            Err(orch_db::queries::upload_history::Error::Missing)
        ),
        "another environment cannot lend the summary association"
    );
}
