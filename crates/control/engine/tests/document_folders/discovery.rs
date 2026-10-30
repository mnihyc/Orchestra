//! Discovery uses only the paths a real tool returned, including redacted structural ancestors.

use super::{Area, TestResult, call, ok, refused, share, string};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::pages::{PageChange, Whole};
use orch_access_contract::permissions::ObjectKind;
use orch_link_contract::commands::OrchTool;
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::collections::{HashSet, VecDeque};

struct Discovered {
    folder: Json,
    document: Json,
    hidden: Vec<Json>,
}

async fn discover(area: &Area, reader: Id) -> TestResult<Discovered> {
    let mut pending = VecDeque::from(["/".to_owned()]);
    let mut visited = HashSet::new();
    let mut folder = None;
    let mut document = None;
    let mut hidden = Vec::new();
    while let Some(path) = pending.pop_front() {
        assert!(
            visited.insert(path.clone()),
            "a returned directory is traversed once"
        );
        let mut cursor: Option<String> = None;
        let mut cursors = HashSet::new();
        loop {
            let mut arguments = json!({"path":path,"depth":1,"limit":2,"sort":"name","ids":false});
            if let Some(cursor) = &cursor {
                arguments["cursor"] = json!(cursor);
            }
            let page = ok(call(&area.world, reader, OrchTool::ListShared, arguments).await);
            let text = page.to_string();
            for secret in [
                "Never reveal parent",
                "Never reveal sibling.md",
                "Unshared sibling content",
            ] {
                assert!(
                    !text.contains(secret),
                    "directory discovery does not reveal a hidden parent or sibling"
                );
            }
            for entry in page["entries"]
                .as_array()
                .ok_or("a tool directory has entries")?
            {
                assert!(
                    entry.get("id").is_none(),
                    "this walk cannot take an optional fixture-ID shortcut"
                );
                if entry["name"] == "Shared research" {
                    folder = Some(entry.clone());
                }
                if entry["name"] == "Readable.md" {
                    document = Some(entry.clone());
                }
                if entry["kind"] == "folder"
                    && entry["name"]
                        .as_str()
                        .is_some_and(|name| name.starts_with("_private-folder~"))
                {
                    hidden.push(entry.clone());
                }
                if matches!(
                    entry["kind"].as_str(),
                    Some("org" | "project" | "topic" | "thread" | "folder")
                ) {
                    pending.push_back(string(entry, "path")?.to_owned());
                }
            }
            let Some(next) = page["next_cursor"].as_str() else {
                break;
            };
            assert!(
                cursors.insert(next.to_owned()),
                "a sparse directory page advances its cursor"
            );
            cursor = Some(next.to_owned());
        }
    }
    Ok(Discovered {
        folder: folder.ok_or("the direct folder grant must be discoverable from /")?,
        document: document.ok_or("its readable child must have a returned path")?,
        hidden,
    })
}

async fn read_returned(area: &Area, reader: Id, path: &str) {
    let read = ok(call(
        &area.world,
        reader,
        OrchTool::ReadShared,
        json!({"path":path,"ids":false}),
    )
    .await);
    assert!(
        read.to_string()
            .contains("Only the directly shared folder is readable.")
    );
    assert!(!read.to_string().contains("Unshared sibling content"));
}

#[tokio::test]
async fn a_folder_only_agent_discovers_and_reuses_returned_paths_without_parent_rights_or_ids()
-> TestResult {
    let area = Area::new(true).await?;
    let parent = area.folder("Never reveal parent/").await?;
    let folder = area.folder("Never reveal parent/Shared research/").await?;
    area.document(
        "Never reveal parent/Shared research/Readable.md",
        "Only the directly shared folder is readable.",
    )
    .await?;
    area.document(
        "Never reveal parent/Never reveal sibling.md",
        "Unshared sibling content",
    )
    .await?;
    // The owner sets privacy through the real native page operation. This does not name the reader there.
    let mut tx = area.world.pool.begin().await?;
    let principal = orch_authz::Principal::load(
        &mut tx,
        orch_authz::Caller::session(area.world.ada),
        Time::new(area.world.clock.now()),
    )
    .await?;
    orch_authz::pages::change(
        &mut tx,
        &principal,
        orch_authz::pages::Target::Resource(ObjectRef {
            kind: ObjectKind::DocumentContainer,
            id: parent,
        }),
        &PageChange {
            whole: Some(Whole::Private),
            ..Default::default()
        },
        orch_authz::Policy::default(),
        &orch_db::audit::Origin::default(),
    )
    .await?;
    tx.commit().await?;
    let reader = area.world.thread("A folder-only reader").await;
    let agent = orch_authz::reach::Agent::of(&mut *area.world.pool.acquire().await?, reader)
        .await?
        .ok_or("the calling agent is present")?;
    assert_ne!(
        agent.topic, area.topic,
        "the source is outside the reader's implicit topic scope"
    );
    area.world
        .set(agent.topic, "sharing.agent_reach", json!("none"))
        .await;
    area.world
        .set(agent.topic, "sharing.agent_documents", json!("none"))
        .await;
    let grant = share(
        &area.world,
        "document_container",
        folder,
        "live",
        "doc.view",
        "any",
        reader,
    )
    .await;
    let found = discover(&area, reader).await?;
    assert_eq!(found.folder["readable"], true);
    assert_eq!(found.folder["writable"], false);
    assert!(
        found.folder.get("manageable").is_none(),
        "the tool omits Manage when it is not held"
    );
    assert!(
        !found.hidden.is_empty(),
        "an unreadable parent is only a structural directory"
    );
    for entry in &found.hidden {
        assert_eq!(entry["readable"], false);
        assert_eq!(entry["writable"], false);
        assert!(
            entry.get("manageable").is_none(),
            "structural navigation supplies no Manage"
        );
    }
    let folder_path = string(&found.folder, "path")?.to_owned();
    let document_path = string(&found.document, "path")?.to_owned();
    let listed = ok(call(
        &area.world,
        reader,
        OrchTool::ListShared,
        json!({"path":folder_path,"ids":false}),
    )
    .await);
    assert_eq!(listed["entries"].as_array().ok_or("entries")?.len(), 1);
    assert_eq!(listed["entries"][0]["path"], document_path);
    assert!(listed["entries"][0].get("id").is_none());
    // These are separate projection controls after discovery has finished. Neither response
    // supplies knowledge to the walk or to the reads: all paths below came from ids:false.
    let default = ok(call(
        &area.world,
        reader,
        OrchTool::ListShared,
        json!({"path":folder_path}),
    )
    .await);
    assert!(
        default["entries"][0].get("id").is_none(),
        "omitting ids is the same false default"
    );
    assert_eq!(default["entries"][0]["path"], document_path);
    let identifiers = ok(call(
        &area.world,
        reader,
        OrchTool::ListShared,
        json!({"path":folder_path,"ids":true}),
    )
    .await);
    let optional = identifiers["entries"][0]["id"]
        .as_str()
        .ok_or("explicit opt-in keeps the optional object identity")?;
    optional.parse::<Id>()?;
    assert_eq!(identifiers["entries"][0]["path"], document_path);
    read_returned(&area, reader, &document_path).await;
    let mut tx = area.world.pool.begin().await?;
    assert!(
        orch_db::queries::access::remove_entry(
            &mut tx,
            grant,
            area.world.ada,
            area.world.clock.now()
        )
        .await?
    );
    orch_authz::epoch::bump(&mut tx).await?;
    tx.commit().await?;
    let removed = call(
        &area.world,
        reader,
        OrchTool::ListShared,
        json!({"path":folder_path,"ids":false}),
    )
    .await;
    assert_eq!(refused(&removed).0, "not_found");
    let removed = call(
        &area.world,
        reader,
        OrchTool::ReadShared,
        json!({"path":document_path,"ids":false}),
    )
    .await;
    assert_eq!(refused(&removed).0, "not_found");
    let renewed = share(
        &area.world,
        "document_container",
        folder,
        "live",
        "doc.view",
        "any",
        reader,
    )
    .await;
    read_returned(&area, reader, &document_path).await;
    let mut tx = area.world.pool.begin().await?;
    sqlx::query("UPDATE permission_entries SET expires_at=$2 WHERE id=$1")
        .bind(renewed)
        .bind(area.world.clock.now())
        .execute(&mut *tx)
        .await?;
    orch_authz::epoch::bump(&mut tx).await?;
    tx.commit().await?;
    let expired = call(
        &area.world,
        reader,
        OrchTool::ReadShared,
        json!({"path":document_path,"ids":false}),
    )
    .await;
    assert_eq!(refused(&expired).0, "not_found");
    let node_grants: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM permission_entries WHERE object_kind='node' AND object_id=$1
        AND holder_kind='agents' AND holder_id=$2 AND removed_at IS NULL",
    )
    .bind(area.topic)
    .bind(reader)
    .fetch_one(&mut *area.world.pool.acquire().await?)
    .await?;
    assert_eq!(
        node_grants, 0,
        "the discovery walk never creates parent authority"
    );
    area.world.shutdown().await;
    Ok(())
}
