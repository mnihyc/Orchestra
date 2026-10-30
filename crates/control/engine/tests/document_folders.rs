//! Recursive document-container management through the real agent tools; no folder storage layout is assumed.

mod common;
#[path = "document_folders/discovery.rs"]
mod discovery;
#[path = "document_folders/regressions.rs"]
mod regressions;
mod shared;

use common::World;
use orch_api_contract::documents::{BlockContent, DocumentOp, VersionCause};
use orch_db::queries::documents;
use orch_engine::tools::CallContext;
use orch_link_contract::commands::OrchTool;
use orch_types::{Id, Json, new_id};
use serde_json::json;
use shared::{Places, call, entry, ok, refused, share};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Area {
    world: World,
    agent: Id,
    topic: Id,
    root: String,
}

impl Area {
    async fn new(manage: bool) -> TestResult<Self> {
        let world = shared::a_world_whose_agent_takes_tools().await;
        Self::on_world(world, manage).await
    }

    async fn on_world(world: World, manage: bool) -> TestResult<Self> {
        let agent = world.thread("Document organizer").await;
        let topic = world.topic("Research documents").await;
        let permissions = if manage {
            vec!["doc.view", "doc.edit", "doc.manage"]
        } else {
            vec!["doc.view", "doc.edit"]
        };
        for permission in permissions {
            share(&world, "node", topic, "live", permission, "any", agent).await;
        }
        let listing = ok(call(
            &world,
            agent,
            OrchTool::ListShared,
            json!({ "path": format!("id:{topic}") }),
        )
        .await);
        let root = string(entry(&listing, "docs"), "path")?.to_owned();
        Ok(Self {
            world,
            agent,
            topic,
            root,
        })
    }

    fn path(&self, path: &str) -> String {
        format!("{}{path}", self.root)
    }

    async fn list(&self, path: &str) -> Json {
        ok(call(
            &self.world,
            self.agent,
            OrchTool::ListShared,
            json!({ "path": path, "limit": 200, "ids": true }),
        )
        .await)
    }

    async fn organize(&self, arguments: Json) -> Json {
        ok(call(&self.world, self.agent, OrchTool::ManageShared, arguments).await)
    }

    async fn folder(&self, path: &str) -> TestResult<Id> {
        let answer = self
            .organize(json!({ "command": "create_folder", "paths": [self.path(path)] }))
            .await;
        assert_eq!(answer["done"], true);
        assert!(
            answer.get("confirm").is_none(),
            "folder creation executes directly"
        );
        let path = self.path(path).trim_end_matches('/').to_owned();
        let (parent, basename) = path.rsplit_once('/').ok_or("folder parent absent")?;
        let listed = self.list(&format!("{parent}/")).await;
        identifier(entry(&listed, basename), "id")
    }

    async fn document(&self, path: &str, text: &str) -> TestResult<Id> {
        let answer = ok(call(
            &self.world,
            self.agent,
            OrchTool::EditDocument,
            json!({ "command": "create", "path": self.path(path), "markdown": text }),
        )
        .await);
        identifier(&answer, "id")
    }

    async fn source_state(&self) -> TestResult<Vec<(Id, Id, String, i64, bool)>> {
        Ok(sqlx::query_as("SELECT id,node_id,title,doc_seq,deleted_at IS NOT NULL FROM documents WHERE node_id=$1 ORDER BY id")
            .bind(self.topic).fetch_all(&mut *self.world.pool.acquire().await?).await?)
    }
}

fn string<'a>(value: &'a Json, key: &str) -> TestResult<&'a str> {
    value
        .get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| format!("missing string {key} in {value}").into())
}

fn identifier(value: &Json, key: &str) -> TestResult<Id> {
    Ok(string(value, key)?.parse()?)
}

#[tokio::test]
async fn folder_creation_uses_existing_parents_and_gives_its_agent_no_management_grant()
-> TestResult {
    let area = Area::new(false).await?;
    let id = area.folder("research/").await?;
    area.folder("research/notes/").await?;
    let listed = area.list(&area.root).await;
    let folder = entry(&listed, "research");
    assert_eq!(folder["id"], json!(id));
    assert_eq!(folder["kind"], "folder");
    assert_eq!(folder["writable"], true);
    assert_ne!(
        folder["manageable"], true,
        "making a folder is not an automatic Manage grant"
    );
    let document = area
        .document("research/notes/Plan.md", "Keep these words.")
        .await?;
    let copied = area.organize(json!({"command":"copy","paths":[area.path("research/")],"to":area.root,"name":"research-copy"})).await;
    assert_eq!(
        copied["done"], true,
        "copy needs source Read and destination creation, not source Manage"
    );
    assert_ne!(
        entry(&area.list(&area.root).await, "research-copy")["manageable"],
        true,
        "copy creation does not clone or invent a Manage grant"
    );
    let read = ok(call(
        &area.world,
        area.agent,
        OrchTool::ReadShared,
        json!({ "path": area.path("research/notes/Plan.md") }),
    )
    .await);
    assert!(read.to_string().contains("Keep these words."));
    let refused_delete = call(
        &area.world,
        area.agent,
        OrchTool::ManageShared,
        json!({ "command": "delete", "paths": [area.path("research/")] }),
    )
    .await;
    assert_eq!(refused(&refused_delete).0, "not_manageable");
    let mut conn = area.world.pool.acquire().await?;
    assert!(
        documents::get(&mut conn, document)
            .await?
            .is_some_and(|row| row.deleted_at.is_none())
    );
    let automatic: i64 = sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE holder_kind='agents' AND holder_id=$1 AND permission='doc.manage' AND source='creation'")
        .bind(area.agent).fetch_one(&mut *conn).await?;
    assert_eq!(automatic, 0);
    drop(conn);
    let before = area.source_state().await?;
    for command in ["create_folder", "copy", "move"] {
        let arguments = match command {
            "create_folder" => {
                json!({ "command": command, "paths": [area.path("missing/child/")] })
            }
            _ => {
                json!({ "command": command, "paths": [area.path("research/notes/Plan.md")], "to": area.path("missing/") })
            }
        };
        let refusal = call(&area.world, area.agent, OrchTool::ManageShared, arguments).await;
        assert!(
            refusal.error.is_some(),
            "{command} cannot create an implicit parent"
        );
        assert_eq!(area.source_state().await?, before);
    }
    Ok(())
}

#[test]
fn direct_folder_sharing_runs_with_default_worker_stacks_even_if_the_suite_stack_is_larger() {
    common::on_default_stack(
        "a_direct_folder_share_reaches_its_descendants_without_revealing_ancestors_or_siblings",
    );
}

#[tokio::test]
async fn a_direct_folder_share_reaches_its_descendants_without_revealing_ancestors_or_siblings()
-> TestResult {
    let area = Area::new(true).await?;
    area.folder("unshared-parent/").await?;
    let shared_folder = area.folder("unshared-parent/shared-research/").await?;
    area.folder("unshared-parent/shared-research/notes/")
        .await?;
    let document = area
        .document(
            "unshared-parent/shared-research/notes/Plan.md",
            "The explicitly shared subtree.",
        )
        .await?;
    area.document(
        "unshared-parent/Hidden-sibling.md",
        "A sibling is not shared.",
    )
    .await?;
    let reader = area.world.thread("Folder reader").await;
    share(
        &area.world,
        "document_container",
        shared_folder,
        "live",
        "doc.view",
        "any",
        reader,
    )
    .await;
    let listing = ok(call(
        &area.world,
        reader,
        OrchTool::ListShared,
        json!({"path":format!("id:{shared_folder}"),"depth":2,"limit":200,"ids":true}),
    )
    .await);
    assert_eq!(
        identifier(entry(&listing, "notes"), "id")?,
        identifier(
            entry(
                &area
                    .list(&area.path("unshared-parent/shared-research/"))
                    .await,
                "notes"
            ),
            "id"
        )?
    );
    let shown = listing.to_string();
    assert!(
        shown.contains("Plan.md"),
        "the folder's descendants inherit its readable page"
    );
    assert!(
        !shown.contains("unshared-parent")
            && !shown.contains("Hidden-sibling")
            && !shown.contains("A sibling is not shared"),
        "structural navigation never reveals an unreadable parent's name or another child"
    );
    let read = ok(call(
        &area.world,
        reader,
        OrchTool::ReadShared,
        json!({"path":format!("id:{document}")}),
    )
    .await);
    assert!(read.to_string().contains("The explicitly shared subtree."));
    let refusal = call(
        &area.world,
        reader,
        OrchTool::ManageShared,
        json!({"command":"delete","paths":[format!("id:{shared_folder}")]}),
    )
    .await;
    assert_eq!(
        refused(&refusal).0,
        "not_manageable",
        "Read alone never becomes recursive Manage"
    );
    Ok(())
}

#[tokio::test]
async fn deleted_folder_names_take_no_path_from_live_twins_and_a_named_copy_resolves_its_basename()
-> TestResult {
    let area = Area::new(true).await?;
    let old_folder = area.folder("research/").await?;
    let old_document = area
        .document("research/Plan.md", "Old recoverable material.")
        .await?;
    let preview = area
        .organize(json!({"command":"delete","paths":[area.path("research/")]}))
        .await;
    area.organize(json!({"command":"delete","paths":[area.path("research/")],"confirm":string(&preview,"confirm")?})).await;
    let new_folder = area.folder("research/").await?;
    let live_document = area
        .document("research/Plan.md", "Current live material.")
        .await?;
    let root = area.list(&area.root).await;
    assert_eq!(identifier(entry(&root, "research"), "id")?, new_folder);
    let entries = root["entries"].as_array().ok_or("entries absent")?;
    let retired = entries
        .iter()
        .find(|entry| entry["id"] == json!(old_folder))
        .ok_or("manageable deleted folder absent")?;
    assert_eq!(retired["deleted"], true);
    assert_ne!(string(retired, "name")?, "research");
    let read = ok(call(
        &area.world,
        area.agent,
        OrchTool::ReadShared,
        json!({"path":area.path("research/Plan.md")}),
    )
    .await);
    assert!(read.to_string().contains("Current live material."));
    assert!(!read.to_string().contains("Old recoverable material."));
    area.organize(json!({"command":"copy","paths":[area.path("research/Plan.md")],"to":area.root,"name":"Renamed-copy.md"})).await;
    let copied = ok(call(
        &area.world,
        area.agent,
        OrchTool::ReadShared,
        json!({"path":area.path("Renamed-copy.md")}),
    )
    .await);
    assert!(copied.to_string().contains("Current live material."));
    let mut conn = area.world.pool.acquire().await?;
    assert!(
        documents::get(&mut conn, old_document)
            .await?
            .is_some_and(|row| row.deleted_at.is_some())
    );
    assert!(
        documents::get(&mut conn, live_document)
            .await?
            .is_some_and(|row| row.deleted_at.is_none())
    );
    drop(conn);
    let before = area.source_state().await?;
    let refused_restore = call(
        &area.world,
        area.agent,
        OrchTool::ManageShared,
        json!({"command":"restore","paths":[string(retired,"path")?]}),
    )
    .await;
    assert_eq!(refused(&refused_restore).0, "exists");
    assert_eq!(area.source_state().await?, before);
    Ok(())
}

#[test]
fn legacy_document_copy_runs_with_default_worker_stacks_even_if_the_suite_stack_is_larger() {
    common::on_default_stack(
        "legacy_create_from_uses_the_same_copy_authority_identity_and_namespace_checks",
    );
}

#[tokio::test]
async fn legacy_create_from_uses_the_same_copy_authority_identity_and_namespace_checks()
-> TestResult {
    let area = Area::new(false).await?;
    area.folder("destination/").await?;
    let source = area
        .document("Original.md", "Copy the current material once.")
        .await?;
    let context = CallContext {
        machine: area.world.machine_id,
        thread: area.agent,
        turn: shared::prepare_call_turn(&area.world, area.agent).await,
        call_ref: "legacy-copy-once".into(),
        attempt: None,
        tool: orch_agent_contract::events::OrchTool::EditDocument,
        arguments: json!({"command":"create","path":area.path("destination/Copy.md"),"from":format!("id:{source}")}),
    };
    area.world.engine().answer_tool_call(&context).await?;
    let copied = identifier(
        entry(&area.list(&area.path("destination/")).await, "Copy.md"),
        "id",
    )?;
    assert_ne!(copied, source);
    let before = area.source_state().await?;
    area.world.engine().answer_tool_call(&context).await?;
    assert_eq!(
        area.source_state().await?,
        before,
        "legacy replay reaches the same durable copy receipt"
    );
    assert_ne!(
        entry(&area.list(&area.path("destination/")).await, "Copy.md")["manageable"],
        true
    );
    for arguments in [
        json!({"command":"copy","paths":[format!("id:{source}")],"to":area.path("destination/"),"name":"Copy.md"}),
        json!({"command":"create","path":area.path("destination/Copy.md"),"from":format!("id:{source}")}),
        json!({"command":"create","path":area.path("missing/Copy.md"),"from":format!("id:{source}")}),
        json!({"command":"create","path":area.path("Another.md"),"from":area.path("destination/")}),
    ] {
        let tool = if arguments["command"] == "copy" {
            OrchTool::ManageShared
        } else {
            OrchTool::EditDocument
        };
        let refusal = call(&area.world, area.agent, tool, arguments).await;
        assert!(refusal.error.is_some());
        assert_eq!(
            area.source_state().await?,
            before,
            "neither entry point overwrites, creates parents or copies a folder through edit_document"
        );
    }
    let hidden = area
        .world
        .document_beside(
            area.world.topic("Unshared documents").await,
            "Hidden",
            &["Do not copy these words."],
        )
        .await;
    let denied = call(&area.world,area.agent,OrchTool::EditDocument,
        json!({"command":"create","path":area.path("destination/Hidden.md"),"from":format!("id:{hidden}")})).await;
    assert_eq!(refused(&denied).0, "not_found");
    assert!(!serde_json::to_string(&denied)?.contains("Do not copy these words."));
    assert_eq!(
        area.list(&area.path("destination/")).await["entries"]
            .as_array()
            .ok_or("entries absent")?
            .iter()
            .filter(|entry| entry["kind"] == "document")
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn wrong_kinds_are_refused_with_the_real_kind_and_the_matching_document_act() -> TestResult {
    let area = Area::new(true).await?;
    let folder = area.folder("research/").await?;
    let document = area
        .document("research/Plan.md", "A text document, not a folder.")
        .await?;
    let chat = shared::chat_of(&area.world, area.topic).await;
    share(
        &area.world,
        "node",
        area.topic,
        "live",
        "chat.view",
        "any",
        area.agent,
    )
    .await;
    let before = area.source_state().await?;
    for (tool, arguments, reason) in [
        (
            OrchTool::ReadShared,
            json!({"path":format!("id:{folder}")}),
            "is a directory, not something to read. List it with list_shared",
        ),
        (
            OrchTool::EditDocument,
            json!({"command":"rewrite","path":format!("id:{folder}"),"markdown":"No folder text.","seq":0}),
            "This path names a document folder, not a text document. List its documents with list_shared; use manage_shared",
        ),
        (
            OrchTool::EditDocument,
            json!({"command":"rename","path":format!("id:{folder}"),"title":"not-text"}),
            "use manage_shared move with name for a folder",
        ),
        (
            OrchTool::ManageShared,
            json!({"command":"delete","paths":[area.root]}),
            "The built-in docs directory is not a mutable object",
        ),
        (
            OrchTool::ManageShared,
            json!({"command":"delete","paths":[format!("id:{chat}")]}),
            "Chats are managed by people",
        ),
        (
            OrchTool::ManageShared,
            json!({"command":"delete","paths":[format!("id:{}",area.agent)]}),
            "Threads are managed by people",
        ),
        (
            OrchTool::ManageShared,
            json!({"command":"copy","paths":[format!("id:{document}")],"to":format!("id:{document}")}),
            "The destination must be an existing writable document directory",
        ),
    ] {
        let answer = call(&area.world, area.agent, tool, arguments).await;
        assert_eq!(refused(&answer).0, "wrong_kind");
        assert!(
            refused(&answer).1.contains(reason),
            "the refusal describes the actual kind and action: {answer:?}"
        );
        assert_eq!(area.source_state().await?, before);
    }
    let missing = call(
        &area.world,
        area.agent,
        OrchTool::ManageShared,
        json!({"command":"delete","paths":[format!("id:{}",new_id())]}),
    )
    .await;
    assert_eq!(
        refused(&missing).0,
        "not_found",
        "unreachable identities never acquire a disclosed kind"
    );
    let unknown = call(
        &area.world,
        area.agent,
        OrchTool::ManageShared,
        json!({"command":"destroy","paths":[format!("id:{document}")]}),
    )
    .await;
    assert_eq!(refused(&unknown).0, "invalid");
    assert!(
        refused(&unknown)
            .1
            .contains("create_folder, copy, move, delete, restore")
    );
    assert_eq!(area.source_state().await?, before);
    assert_eq!(
        identifier(entry(&area.list(&area.root).await, "research"), "id")?,
        folder
    );
    Ok(())
}

#[tokio::test]
async fn recursive_copy_has_fresh_identity_and_history_remaps_internal_links_and_replays_once()
-> TestResult {
    let area = Area::new(true).await?;
    let source_folder = area.folder("research/").await?;
    area.folder("research/notes/").await?;
    let first = area
        .document("research/Plan.md", "Source-only historical words.")
        .await?;
    let second = area
        .document("research/notes/Notes.md", "Original notes.")
        .await?;
    let external = area
        .document("Outside.md", "Outside this selection.")
        .await?;
    let source_owner = area.world.person("source-owner").await;
    let discussion = new_id();
    let mut tx = area.world.pool.begin().await?;
    sqlx::query("UPDATE documents SET owner_id=$2 WHERE id=$1")
        .bind(first)
        .bind(source_owner)
        .execute(&mut *tx)
        .await?;
    orch_db::queries::access::testing::add_entry(
        &mut tx,
        &orch_db::queries::access::PageKey::new("document", first, "live"),
        "any",
        "access.own",
        ("user", Some(source_owner), None),
        None,
        area.world.clock.now(),
    )
    .await?;
    let row = documents::lock(&mut tx, first)
        .await?
        .ok_or("source absent")?;
    documents::history::cut(&mut tx, &row, VersionCause::Rewrite, area.world.clock.now()).await?;
    let blocks = documents::live_blocks(&mut tx, first).await?;
    let block = blocks.first().ok_or("source block absent")?;
    let old_block = block.id;
    let content: BlockContent = serde_json::from_value(json!([
        {"type":"ref","kind":"document","id":second},
        {"type":"ref","kind":"document","id":external},
        {"type":"text","text":"own anchor","href":format!("/doc/{first}?block={old_block}")},
    ]))?;
    let applied = documents::batch::apply(
        &mut tx,
        first,
        &orch_documents::versions::Writer::User(area.world.ada),
        Some(new_id()),
        &[DocumentOp::Update {
            id: old_block,
            base_seq: block.seq_updated,
            block_type: None,
            props: None,
            content: Some(content),
        }],
        documents::batch::Limits {
            max_bytes: 2_097_152,
            max_blocks: 5_000,
        },
        documents::batch::Rules {
            window_secs: 600,
            kept: 200,
            history_max_bytes: 16_777_216,
        },
        area.world.clock.now(),
    )
    .await?;
    assert!(
        applied.is_kept(),
        "the source contains valid structured material"
    );
    sqlx::query("INSERT INTO document_discussions (id,document_id,created_by,seq_updated) VALUES ($1,$2,$3,0)")
        .bind(discussion).bind(first).bind(area.world.ada).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO document_comments (id,discussion_id,document_id,author_id,content,seq_updated) VALUES ($1,$2,$3,$4,$5,0)")
        .bind(new_id()).bind(discussion).bind(first).bind(area.world.ada)
        .bind(json!([{ "type": "text", "text": "This source-only comment is not copied." }])).execute(&mut *tx).await?;
    tx.commit().await?;
    area.world
        .add_paragraph(first, "Current material, not old history.")
        .await;
    let original_blocks = area.world.blocks_of(first).await;
    let arguments = json!({ "command": "copy", "paths": [area.path("research/")], "to": area.root, "name": "research-copy" });
    let context = CallContext {
        machine: area.world.machine_id,
        thread: area.agent,
        turn: shared::prepare_call_turn(&area.world, area.agent).await,
        call_ref: "copy-complete-tree-once".into(),
        attempt: None,
        tool: orch_agent_contract::events::OrchTool::ManageShared,
        arguments,
    };
    area.world.engine().answer_tool_call(&context).await?;
    let root = area.list(&area.root).await;
    let copy_folder = identifier(entry(&root, "research-copy"), "id")?;
    assert_ne!(copy_folder, source_folder);
    let first_copy = identifier(
        entry(&area.list(&area.path("research-copy/")).await, "Plan.md"),
        "id",
    )?;
    let second_copy = identifier(
        entry(
            &area.list(&area.path("research-copy/notes/")).await,
            "Notes.md",
        ),
        "id",
    )?;
    assert_ne!(first_copy, first);
    assert_ne!(second_copy, second);
    let before_replay = area.source_state().await?;
    area.world.engine().answer_tool_call(&context).await?;
    assert_eq!(
        area.source_state().await?,
        before_replay,
        "an awaited replay creates no duplicate documents or versions"
    );
    let mut conn = area.world.pool.acquire().await?;
    let made = documents::get(&mut conn, first_copy)
        .await?
        .ok_or("copy absent")?;
    assert_eq!(
        made.owner_id, area.world.ada,
        "ownership comes from destination creation, not the source owner"
    );
    assert_eq!(made.created_by_thread, Some(area.agent));
    let copied_blocks = documents::live_blocks(&mut conn, first_copy).await?;
    assert_eq!(copied_blocks.len(), original_blocks.len());
    assert!(
        copied_blocks
            .iter()
            .all(|block| !original_blocks.iter().any(|(id, _)| *id == block.id))
    );
    let material = copied_blocks
        .iter()
        .map(|block| block.content.clone())
        .collect::<Vec<_>>();
    let content = serde_json::to_string(&material)?;
    assert!(
        content.contains(&second_copy.to_string()),
        "references inside the copied tree point to the copied peer"
    );
    assert!(!content.contains(&second.to_string()));
    assert!(
        content.contains(&external.to_string()),
        "outside references keep their original target"
    );
    assert!(content.contains(&format!("/doc/{first_copy}?block=")));
    assert!(!content.contains(&old_block.to_string()));
    let comments: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_comments WHERE document_id=ANY($1)")
            .bind(vec![first_copy, second_copy])
            .fetch_one(&mut *conn)
            .await?;
    assert_eq!(comments, 0);
    let cloned_access: i64 = sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE object_kind='document' AND object_id=ANY($1) AND holder_id=$2")
        .bind(vec![first_copy, second_copy]).bind(source_owner).fetch_one(&mut *conn).await?;
    assert_eq!(
        cloned_access, 0,
        "the source's people/grants are not copied"
    );
    let provenance: Vec<Json> = sqlx::query_scalar(
        "SELECT details FROM audit_log WHERE object_id=$1 AND actor_kind='agent' AND actor_id=$2",
    )
    .bind(first_copy)
    .bind(area.agent)
    .fetch_all(&mut *conn)
    .await?;
    assert!(
        provenance
            .iter()
            .any(|details| details.to_string().contains(&first.to_string())),
        "copy audit retains its source identity"
    );
    let source_versions: Vec<Json> =
        sqlx::query_scalar("SELECT snapshot FROM document_versions WHERE document_id=$1")
            .bind(first)
            .fetch_all(&mut *conn)
            .await?;
    let copied_versions: Vec<Json> =
        sqlx::query_scalar("SELECT snapshot FROM document_versions WHERE document_id=$1")
            .bind(first_copy)
            .fetch_all(&mut *conn)
            .await?;
    assert!(source_versions.iter().any(|snapshot| {
        snapshot
            .to_string()
            .contains("Source-only historical words.")
    }));
    assert!(
        copied_versions.iter().all(|snapshot| !snapshot
            .to_string()
            .contains("Source-only historical words.")
            && !snapshot.to_string().contains(&old_block.to_string())),
        "old versions and their block identities are not imported"
    );
    assert!(
        copied_versions.len() <= 1,
        "copy starts its own history instead of importing old versions"
    );
    Ok(())
}

#[tokio::test]
async fn a_folder_moves_directly_then_delete_previews_all_descendants_and_restore_keeps_their_ids()
-> TestResult {
    let area = Area::new(true).await?;
    let folder = area.folder("research/").await?;
    let nested = area.folder("research/notes/").await?;
    let document = area
        .document(
            "research/notes/Plan.md",
            "Move and restore these exact blocks.",
        )
        .await?;
    area.folder("archive/").await?;
    let before = area.world.blocks_of(document).await;
    let moved = area
        .organize(
            json!({ "command": "move", "paths": [area.path("research/")],
        "to": area.path("archive/"), "name": "kept-research" }),
        )
        .await;
    assert_eq!(moved["done"], true);
    assert!(moved.get("confirm").is_none());
    assert_eq!(
        identifier(
            entry(&area.list(&area.path("archive/")).await, "kept-research"),
            "id"
        )?,
        folder
    );
    assert_eq!(
        identifier(
            entry(
                &area.list(&area.path("archive/kept-research/")).await,
                "notes"
            ),
            "id"
        )?,
        nested
    );
    assert_eq!(
        identifier(
            entry(
                &area.list(&area.path("archive/kept-research/notes/")).await,
                "Plan.md"
            ),
            "id"
        )?,
        document
    );
    assert_eq!(area.world.blocks_of(document).await, before);
    let selection = area.path("archive/kept-research/");
    let preview = area
        .organize(json!({ "command": "delete", "paths": [selection] }))
        .await;
    assert_eq!(preview["done"], false);
    let affected = preview["affects"]
        .as_array()
        .ok_or("complete affects absent")?;
    for id in [folder, nested, document] {
        assert!(
            affected.iter().any(|item| item["id"] == json!(id)),
            "the preview includes every descendant identity"
        );
    }
    assert!(
        affected.iter().any(|item| item["until"].is_string()),
        "retention is stated before deletion"
    );
    let word = string(&preview, "confirm")?;
    let done = area
        .organize(json!({ "command": "delete", "paths": [selection], "confirm": word }))
        .await;
    assert_eq!(done["done"], true);
    let mut conn = area.world.pool.acquire().await?;
    assert!(
        documents::get(&mut conn, document)
            .await?
            .is_some_and(|row| row.deleted_at.is_some())
    );
    drop(conn);
    let restored = area
        .organize(json!({ "command": "restore", "paths": [selection] }))
        .await;
    assert_eq!(restored["done"], true);
    assert!(restored.get("confirm").is_none());
    assert_eq!(
        identifier(
            entry(
                &area.list(&area.path("archive/kept-research/notes/")).await,
                "Plan.md"
            ),
            "id"
        )?,
        document
    );
    assert_eq!(area.world.blocks_of(document).await, before);
    Ok(())
}

#[tokio::test]
async fn collisions_self_descendants_overlaps_and_builtin_directories_refuse_without_partial_changes()
-> TestResult {
    let area = Area::new(true).await?;
    area.folder("research/").await?;
    area.folder("research/notes/").await?;
    area.document("research/Plan.md", "The original must stay.")
        .await?;
    area.folder("archive/").await?;
    area.folder("archive/research/").await?;
    let before = area.source_state().await?;
    for arguments in [
        json!({"command":"create_folder","paths":[area.path("research/")]}),
        json!({"command":"copy","paths":[area.path("research/")],"to":area.path("research/notes/")}),
        json!({"command":"move","paths":[area.path("research/")],"to":area.path("research/")}),
        json!({"command":"copy","paths":[area.path("research/")],"to":area.path("archive/")}),
        json!({"command":"delete","paths":[area.path("research/"),area.path("research/Plan.md")]}),
        json!({"command":"move","paths":[area.path("research/"),area.path("archive/")],"to":area.root,"name":"one-name"}),
        json!({"command":"delete","paths":[area.root]}),
        json!({"command":"create_folder","paths":[area.path("Plan.history/")]}),
        json!({"command":"create_folder","paths":[area.path("Plan.files/")]}),
    ] {
        let refusal = call(
            &area.world,
            area.agent,
            OrchTool::ManageShared,
            arguments.clone(),
        )
        .await;
        assert!(
            refusal.error.is_some(),
            "must refuse {arguments}: {refusal:?}"
        );
        assert_eq!(area.source_state().await?, before);
        let names: Vec<_> = area.list(&area.root).await["entries"]
            .as_array()
            .ok_or("entries absent")?
            .iter()
            .filter_map(|entry| entry["name"].as_str().map(str::to_owned))
            .collect();
        assert_eq!(names.len(), 2);
        assert!(
            names.contains(&"research".to_owned()) && names.contains(&"archive".to_owned()),
            "no overwrite or automatic renaming"
        );
    }
    Ok(())
}

#[tokio::test]
async fn one_inaccessible_descendant_refuses_every_recursive_operation_without_disclosing_its_name()
-> TestResult {
    use orch_access_contract::decisions::ObjectRef;
    use orch_access_contract::pages::{PageChange, Whole};
    use orch_access_contract::permissions::ObjectKind;
    let area = Area::new(true).await?;
    area.folder("research/").await?;
    area.folder("archive/").await?;
    area.document("research/Public.md", "The public document also stays.")
        .await?;
    let hidden = area
        .document(
            "research/Hidden-findings.md",
            "These hidden words cannot escape.",
        )
        .await?;
    let mut tx = area.world.pool.begin().await?;
    let principal = orch_authz::Principal::load(
        &mut tx,
        orch_authz::Caller::session(area.world.ada),
        orch_types::Time::new(area.world.clock.now()),
    )
    .await?;
    orch_authz::pages::change(
        &mut tx,
        &principal,
        orch_authz::pages::Target::Resource(ObjectRef {
            kind: ObjectKind::Document,
            id: hidden,
        }),
        &PageChange {
            whole: Some(Whole::Private),
            ..PageChange::default()
        },
        orch_authz::Policy::default(),
        &orch_db::audit::Origin::default(),
    )
    .await?;
    // A private page keeps its own named entries; remove the creator's Edit grant as the human
    // owner so that this test truly has an invisible descendant, not merely a private badge.
    sqlx::query("DELETE FROM permission_entries WHERE object_kind='document' AND object_id=$1 AND holder_kind='agents' AND holder_id=$2")
        .bind(hidden).bind(area.agent).execute(&mut *tx).await?;
    orch_authz::epoch::bump(&mut tx).await?;
    tx.commit().await?;
    assert!(
        !area
            .list(&area.path("research/"))
            .await
            .to_string()
            .contains("Hidden-findings")
    );
    let before = area.source_state().await?;
    for command in ["copy", "move", "delete"] {
        let arguments = if command == "delete" {
            json!({"command":command,"paths":[area.path("research/")]})
        } else {
            json!({"command":command,"paths":[area.path("research/")],"to":area.path("archive/")})
        };
        let refusal = call(&area.world, area.agent, OrchTool::ManageShared, arguments).await;
        assert!(
            refusal.error.is_some(),
            "the complete recursive operation must refuse"
        );
        let shown = serde_json::to_string(&refusal)?;
        for secret in ["Hidden-findings", "These hidden words", &hidden.to_string()] {
            assert!(
                !shown.contains(secret),
                "a refused traversal does not disclose inaccessible descendants"
            );
        }
        assert!(
            refusal
                .result
                .as_ref()
                .is_none_or(|result| result.get("confirm").is_none())
        );
        assert_eq!(
            area.source_state().await?,
            before,
            "even the accessible sibling stays untouched"
        );
        assert_eq!(
            area.list(&area.path("archive/")).await["entries"],
            json!([])
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_new_child_invalidates_the_complete_deletion_preview_and_descendants_consume_the_existing_bound()
-> TestResult {
    let area = Area::new(true).await?;
    area.folder("research/").await?;
    area.document("research/First.md", "First.").await?;
    let preview = area
        .organize(json!({"command":"delete","paths":[area.path("research/")]}))
        .await;
    let word = string(&preview, "confirm")?.to_owned();
    area.document(
        "research/Arrived-after-preview.md",
        "The new child must not be missed.",
    )
    .await?;
    let before = area.source_state().await?;
    let stale = call(
        &area.world,
        area.agent,
        OrchTool::ManageShared,
        json!({"command":"delete","paths":[area.path("research/")],"confirm":word}),
    )
    .await;
    assert_eq!(refused(&stale).0, "not_confirmed");
    assert_eq!(
        area.source_state().await?,
        before,
        "a membership change is atomic refusal, not deletion of the old prefix"
    );
    let bounded = Area::new(true).await?;
    bounded.folder("research/").await?;
    // One folder plus fifty leaves exceeds the existing fifty-item bound. The single selected
    // path itself stays well below the twenty-path input bound and the normal tool rate limit.
    for index in 0..orch_engine::tools::manage::AFFECTS_MAX {
        bounded
            .document(
                &format!("research/Draft {index:02}.md"),
                "A short document.",
            )
            .await?;
    }
    let before = bounded.source_state().await?;
    let over = call(
        &bounded.world,
        bounded.agent,
        OrchTool::ManageShared,
        json!({"command":"delete","paths":[bounded.path("research/")],"max_bytes":40000}),
    )
    .await;
    assert_eq!(refused(&over).0, "limit");
    assert!(
        over.result
            .as_ref()
            .is_none_or(|result| result.get("confirm").is_none()),
        "an oversized tree never gets a token for a hidden suffix"
    );
    assert_eq!(bounded.source_state().await?, before);
    Ok(())
}
