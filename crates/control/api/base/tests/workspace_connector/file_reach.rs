//! Intentional human file reach through the router, current grant and actual native filesystem.

use super::*;
use crate::support::FullPermissionPageReply;
use orch_api_contract::access::{PermissionPage, PermissionPageUpdate};
use orch_api_contract::workspace::{
    EnvironmentFileSearch, EnvironmentFileSearchQuery, EnvironmentFileSearchReply, FileSearchIn,
};
use serde_json::json;

async fn change(site: &Site, rows: orch_types::Json) -> PermissionPage {
    site.app()
        .call(PermissionPageUpdate, &[&"environments", &site.environment])
        .token(&site.ada.token)
        .raw_body(&json!({"rows":rows}))
        .send()
        .await
        .full_permission_page()
        .ok()
}
pub(super) async fn grant(site: &Site, permission: &str, reach: &str) -> PermissionPage {
    change(site, json!([{"permission":permission,"add":[{"holder":{"kind":"user","id":site.vic.id()},"options":{"reach":reach}}]}])).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_explicit_machine_file_grant_reaches_the_native_target_and_narrowing_refuses_the_next_act()
 {
    let cluster = Cluster::start(true).await;
    let site = &cluster.site;
    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("outside.txt");
    std::fs::write(&file, b"outside bytes").unwrap();
    let path = file.to_string_lossy().into_owned();
    let directory = outside.path().to_string_lossy().into_owned();
    assert_eq!(
        ask::stat(site, &site.vic, &path).await.problem().code,
        ApiCode::FileReachDenied
    );
    grant(site, "files.view", "machine").await;
    let listed = ask::list(site, &site.vic, &directory, None).await.ok();
    assert!(listed.entries.iter().any(|entry| entry.path == path));
    assert_eq!(ask::stat(site, &site.vic, &path).await.ok().entry.size, 13);
    let fetched = ask::fetch(site, &site.vic, &path, Some(5)).await.ok();
    assert_eq!(fetched.state, FetchStatus::Ready);
    let hash = fetched.hash.unwrap();
    let card_query = orch_api_contract::documents::ReferencePreviewQuery {
        kind: orch_api_contract::documents::InlineRefKind::EnvironmentFile,
        id: site.environment,
        block: None,
        turn: None,
        message: None,
        path: Some(path.clone()),
        line: Some(1),
    };
    let card = site
        .app()
        .call(orch_api_contract::documents::ReferencePreviewGet, &[])
        .token(&site.vic.token)
        .query(&card_query)
        .send()
        .await
        .ok();
    assert_eq!(card.file.as_ref().and_then(|file| file.blob), Some(hash));
    assert!(
        card.part
            .as_ref()
            .is_some_and(|part| part.text.contains("outside bytes"))
    );
    let bytes = ask::content(site, &site.vic, &path, &hash, None, true).await;
    assert_eq!(bytes.status, StatusCode::OK);
    assert_eq!(&bytes.raw[..], b"outside bytes");
    assert_eq!(
        ask::write(site, &site.vic, &path, None, b"not allowed")
            .await
            .problem()
            .code,
        ApiCode::AuthzDenied
    );
    grant(site, "files.manage", "roots").await;
    assert_eq!(
        ask::write(site, &site.vic, &path, None, b"not allowed")
            .await
            .problem()
            .code,
        ApiCode::FileReachDenied
    );
    ask::copy(site, &site.vic, &path, "web/copied.txt")
        .await
        .ok();
    assert_eq!(
        std::fs::read(cluster.work.path().join("web/copied.txt")).unwrap(),
        b"outside bytes"
    );
    let page = grant(site, "files.manage", "machine").await;
    ask::write(site, &site.vic, &path, None, b"written deliberately")
        .await
        .ok();
    assert_eq!(std::fs::read(&file).unwrap(), b"written deliberately");
    let moved = outside
        .path()
        .join("outside-moved.txt")
        .to_string_lossy()
        .into_owned();
    ask::rename(site, &site.vic, &path, &moved).await.ok();
    let results = site
        .app()
        .call(EnvironmentFileSearch, &[&site.environment])
        .token(&site.vic.token)
        .query(&EnvironmentFileSearchQuery {
            thread: Some(site.thread),
            path: Some(directory),
            q: "outside".to_owned(),
            scope: Some(FileSearchIn::Names),
            cursor: None,
            glob: None,
            regex: None,
            case: None,
            limit: Some(2),
        })
        .send()
        .await
        .ok();
    assert!(
        matches!(results, EnvironmentFileSearchReply::Names(names) if names.entries.iter().any(|entry|entry.path == moved))
    );
    let rows: Vec<_> = page.rows.iter().filter(|row| row.permission == "files.view" || row.permission == "files.manage")
        .filter_map(|row| row.entries.iter().find(|entry| entry.options.get("reach").and_then(orch_types::Json::as_str) == Some("machine"))
            .map(|entry| json!({"permission":row.permission,"update":[{"entry_id":entry.id,"options":{"reach":"roots"}}]}))).collect();
    assert_eq!(rows.len(), 2);
    change(site, json!(rows)).await;
    assert_eq!(
        ask::stat(site, &site.vic, &moved).await.problem().code,
        ApiCode::FileReachDenied
    );
    assert_eq!(
        ask::delete(site, &site.vic, &moved, false)
            .await
            .problem()
            .code,
        ApiCode::FileReachDenied
    );
    let refused_card = site
        .app()
        .call(orch_api_contract::documents::ReferencePreviewGet, &[])
        .token(&site.vic.token)
        .query(&card_query)
        .send()
        .await;
    assert!(
        refused_card.status == StatusCode::FORBIDDEN
            || refused_card
                .body
                .as_ref()
                .is_ok_and(|card| card.unavailable.is_some())
    );
    let old = ask::content(site, &site.vic, &path, &hash, None, true).await;
    assert_eq!(old.status, StatusCode::FORBIDDEN);
    assert!(std::path::Path::new(&moved).exists());
}
