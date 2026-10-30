//! What the tests of the shared tree share: a world whose agent takes the tools, a call made as the agent and answered, and the making of places and shares by hand.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

#[path = "../native_tools/mod.rs"]
mod native_tools;

use crate::common::World;
use orch_db::queries::access::{PageKey, testing as access_testing};
use orch_db::queries::hierarchy::testing::insert_node;
use orch_link_contract::commands::{OrchTool, OrchToolResult};
use orch_types::{Id, Json, new_id};
use serde_json::json;

/// A world whose agent takes an MCP server, so that the tools are offered.
pub async fn a_world_whose_agent_takes_tools() -> World {
    let world = World::new().await;
    the_agent_takes_tools_again(&world).await;
    native_tools::existing_workspace(&world);
    world
}

/// Stored discovery and the native agent.start response declare the same own-tool support.
pub async fn the_agent_takes_tools_again(world: &World) {
    native_tools::enable(world).await;
}

/// The harness's side: the agent of `thread` calls a tool, and Orch's answer is waited for.
pub async fn call(world: &World, thread: Id, tool: OrchTool, arguments: Json) -> OrchToolResult {
    call_as(world, thread, tool, arguments, &new_id().to_string()).await
}

/// Prepare the asking agent before a test fixes ordering metadata or native handlers.
/// This authors one baseline input if needed, and leaves its actual turn active.
pub async fn prepare_call_turn(world: &World, thread: Id) -> Id {
    native_tools::prepare(world, thread).await
}

/// The same call, under a name of the harness's own: said twice, it is one call.
pub async fn call_as(
    world: &World,
    thread: Id,
    tool: OrchTool,
    arguments: Json,
    call_ref: &str,
) -> OrchToolResult {
    let turn = prepare_call_turn(world, thread).await;
    native_tools::send(world, thread, turn, call_ref, tool, arguments).await;
    answer_to(world, thread, call_ref).await
}

/// Waits for the answer to a call.
pub async fn answer_to(world: &World, thread: Id, call_ref: &str) -> OrchToolResult {
    native_tools::answer(world, thread, call_ref).await
}

/// The result of a call that was not refused.
pub fn ok(result: OrchToolResult) -> Json {
    assert!(
        result.error.is_none(),
        "the call was refused: {:?}",
        result.error
    );
    result.result.expect("a result")
}

/// The code and the words of a call that was refused.
pub fn refused(result: &OrchToolResult) -> (&str, &str) {
    let error = result.error.as_ref().expect("a refusal");
    (error.code.as_str(), error.message.as_str())
}

/// How many commands of `kind` (`fs.stat`, `fs.fetch`) the engine asked a machine for about a file below the
/// threads' directory `web`: preparing a turn looks at the directory itself, which is no file. The tests answer
/// these kinds with scripts of their own, which the world's record of the commands it answers does not keep.
pub fn file_commands(world: &World, kind: &str) -> usize {
    world
        .commands
        .issued_bodies(kind)
        .iter()
        .filter(|(_, body)| {
            body["path"]
                .as_str()
                .is_some_and(|path| path.starts_with("web/"))
        })
        .count()
}

/// The entry of a listing with this name.
pub fn entry<'a>(listing: &'a Json, name: &str) -> &'a Json {
    listing["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap_or_else(|| panic!("no entry {name} in {listing}"))
}

/// The paths of a listing.
pub fn paths(listing: &Json) -> Vec<String> {
    listing["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| entry["path"].as_str().expect("a path").to_owned())
        .collect()
}

/// Names the agents of `place` in a row of a page: `kind` is `node`, `document` or `chat`, `page` is `live` or
/// `defaults`.
pub async fn share(
    world: &World,
    kind: &str,
    thing: Id,
    page: &str,
    permission: &str,
    profile: &str,
    place: Id,
) -> Id {
    let mut conn = world.pool.acquire().await.unwrap();
    access_testing::add_entry(
        &mut conn,
        &PageKey::new(kind, thing, page),
        profile,
        permission,
        ("agents", Some(place), None),
        None,
        chrono::Utc::now(),
    )
    .await
    .unwrap()
}

/// Shares the conversations of what is inside a place, in both profiles of the row.
pub async fn share_threads_inside(world: &World, place: Id, agents_of: Id, write: bool) {
    for permission in if write {
        vec!["thread.view", "thread.interact"]
    } else {
        vec!["thread.view"]
    } {
        for profile in ["managed", "connected"] {
            share(
                world, "node", place, "defaults", permission, profile, agents_of,
            )
            .await;
        }
    }
}

/// A thread in a topic of its own, bound to the world's agent, provider and environment.
pub async fn thread_in_topic(world: &World, topic: Id, name: &str) -> Id {
    world
        .engine()
        .create_thread(
            &orch_authz::Caller::session(world.ada),
            orch_engine::binding::NewThread {
                topic,
                name: name.to_owned(),
                agent: world.agent,
                provider: world.provider,
                environment: world.environment,
                directory: "web".to_owned(),
                arrangement: None,
                model: Some(crate::common::MODEL.to_owned()),
                effort: None,
            },
        )
        .await
        .expect("a thread is created")
        .thread
}

/// Ada says something to a thread, and the agent answers: one more turn.
pub async fn talk(world: &World, thread: Id, said: &str, reply: &str) {
    let before = world.turn_starts(thread).len();
    world
        .engine()
        .submit_input(
            &orch_authz::Caller::session(world.ada),
            thread,
            crate::common::queued(said),
        )
        .await
        .unwrap();
    world.run_turn(thread, before, reply).await;
}

/// The chat of a node: made when it is first asked for.
pub async fn chat_of(world: &World, node: Id) -> Id {
    let mut conn = world.pool.acquire().await.unwrap();
    let found: Option<Id> = sqlx::query_scalar("SELECT id FROM chats WHERE node_id = $1")
        .bind(node)
        .fetch_optional(&mut *conn)
        .await
        .unwrap();
    if let Some(found) = found {
        return found;
    }
    let id = new_id();
    sqlx::query(
        "INSERT INTO chats (id, kind, node_id, org_id) SELECT $1, 'node', n.id, n.org_id FROM nodes n WHERE n.id = $2",
    )
    .bind(id)
    .bind(node)
    .execute(&mut *conn)
    .await
    .unwrap();
    id
}

/// A person writes in a chat.
pub async fn say_in_chat(world: &World, chat: Id, author: Id, text: &str) -> i64 {
    let mut conn = world.pool.acquire().await.unwrap();
    let seq: i64 = sqlx::query_scalar(
        "UPDATE chats SET next_seq = next_seq + 1, last_message_at = now() WHERE id = $1 RETURNING next_seq - 1",
    )
    .bind(chat)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chat_messages (id, chat_id, kind, author_id, content, seq_created, seq_updated, search_text)
         VALUES ($1, $2, 'message', $3, $4, $5, $5, $6)",
    )
    .bind(new_id())
    .bind(chat)
    .bind(author)
    .bind(json!([{ "type": "text", "text": text }]))
    .bind(seq)
    .bind(text)
    .execute(&mut *conn)
    .await
    .unwrap();
    seq
}

/// Makes places by hand.
pub trait Places {
    /// A project in the root org.
    async fn project_in_root(&self, name: &str) -> Id;
    /// A topic in a project.
    async fn topic_in(&self, project: Id, name: &str) -> Id;
    /// A bare thread in a topic.
    async fn thread_in(&self, topic: Id, name: &str) -> Id;
    /// A document beside a node, made by Ada, with these paragraphs.
    async fn document_beside(&self, node: Id, title: &str, paragraphs: &[&str]) -> Id;
    /// Ada adds a paragraph to a document, and a version is cut first.
    async fn add_paragraph(&self, document: Id, text: &str);
    /// Ada rewrites the block of a document that holds `holds`, as she would in the editor.
    async fn change_block(&self, document: Id, holds: &str, to: &str);
    /// The text a document is written as, as a person reads it, one line a block.
    async fn blocks_of(&self, document: Id) -> Vec<(Id, String)>;
}

impl Places for World {
    async fn blocks_of(&self, document: Id) -> Vec<(Id, String)> {
        let mut conn = self.pool.acquire().await.unwrap();
        orch_db::queries::documents::live_blocks(&mut conn, document)
            .await
            .unwrap()
            .into_iter()
            .map(|block| (block.id, block.text))
            .collect()
    }

    async fn change_block(&self, document: Id, holds: &str, to: &str) {
        use orch_api_contract::documents::{BlockContent, DocumentOp, Inline};
        use orch_db::queries::documents::{self as rows, batch};
        use orch_documents::versions::Writer;
        let mut conn = self.pool.acquire().await.unwrap();
        let blocks = rows::live_blocks(&mut conn, document).await.unwrap();
        let block = blocks
            .iter()
            .find(|block| block.text.contains(holds))
            .expect("a block that holds the text");
        let outcome = batch::apply(
            &mut conn,
            document,
            &Writer::User(self.ada),
            Some(new_id()),
            &[DocumentOp::Update {
                id: block.id,
                base_seq: block.seq_updated,
                block_type: None,
                props: None,
                content: Some(BlockContent::Inline(vec![Inline::Text {
                    text: to.to_owned(),
                    marks: None,
                    href: None,
                    color: None,
                    background: None,
                }])),
            }],
            batch::Limits {
                max_bytes: 2_097_152,
                max_blocks: 5_000,
            },
            batch::Rules {
                window_secs: 600,
                kept: 200,
                history_max_bytes: 16_777_216,
            },
            chrono::Utc::now() + chrono::Duration::seconds(5),
        )
        .await
        .unwrap();
        assert!(outcome.is_kept(), "{outcome:?}");
    }

    async fn add_paragraph(&self, document: Id, text: &str) {
        use orch_api_contract::documents::{DocumentOp, NewBlock};
        use orch_db::queries::documents::batch;
        use orch_documents::versions::Writer;
        let mut conn = self.pool.acquire().await.unwrap();
        let block: NewBlock = serde_json::from_value(json!({
            "id": new_id(), "type": "paragraph", "props": {},
            "content": [{ "type": "text", "text": text }]
        }))
        .unwrap();
        let outcome = batch::apply(
            &mut conn,
            document,
            &Writer::User(self.ada),
            None,
            &[DocumentOp::Insert {
                block,
                parent_id: None,
                after_id: None,
            }],
            batch::Limits {
                max_bytes: 2_097_152,
                max_blocks: 5_000,
            },
            batch::Rules {
                window_secs: 0,
                kept: 200,
                history_max_bytes: 16_777_216,
            },
            chrono::Utc::now() + chrono::Duration::seconds(5),
        )
        .await
        .unwrap();
        assert!(outcome.is_kept(), "{outcome:?}");
    }

    async fn project_in_root(&self, name: &str) -> Id {
        let mut conn = self.pool.acquire().await.unwrap();
        let id = new_id();
        insert_node(&mut conn, id, "project", Some(self.root), name, self.ada)
            .await
            .unwrap();
        id
    }

    async fn topic_in(&self, project: Id, name: &str) -> Id {
        let mut conn = self.pool.acquire().await.unwrap();
        let id = new_id();
        insert_node(&mut conn, id, "topic", Some(project), name, self.ada)
            .await
            .unwrap();
        id
    }

    async fn thread_in(&self, topic: Id, name: &str) -> Id {
        let mut conn = self.pool.acquire().await.unwrap();
        let id = new_id();
        insert_node(&mut conn, id, "thread", Some(topic), name, self.ada)
            .await
            .unwrap();
        id
    }

    async fn document_beside(&self, node: Id, title: &str, paragraphs: &[&str]) -> Id {
        use orch_api_contract::documents::{DocumentOp, NewBlock};
        use orch_db::queries::documents::{self as rows, batch};
        use orch_documents::versions::Writer;
        let mut conn = self.pool.acquire().await.unwrap();
        let id = new_id();
        let org: Id = sqlx::query_scalar("SELECT org_id FROM nodes WHERE id = $1")
            .bind(node)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        let now = chrono::Utc::now();
        rows::insert(
            &mut conn,
            &rows::NewDocument {
                id,
                node_id: node,
                org_id: org,
                owner_id: self.ada,
                created_by_thread: None,
                created_by_agent: None,
                title,
                now,
            },
        )
        .await
        .unwrap();
        orch_authz::documents::start(&mut conn, id, self.ada, orch_types::Time::new(now))
            .await
            .unwrap();
        let mut previous: Option<Id> = None;
        let mut ops = Vec::new();
        for text in paragraphs {
            let block: NewBlock = serde_json::from_value(json!({
                "id": new_id(), "type": "paragraph", "props": {},
                "content": [{ "type": "text", "text": text }]
            }))
            .unwrap();
            let block_id = block.id;
            ops.push(DocumentOp::Insert {
                block,
                parent_id: None,
                after_id: previous,
            });
            previous = Some(block_id);
        }
        if !ops.is_empty() {
            let outcome = batch::apply(
                &mut conn,
                id,
                &Writer::User(self.ada),
                None,
                &ops,
                batch::Limits {
                    max_bytes: 2_097_152,
                    max_blocks: 5_000,
                },
                batch::Rules {
                    window_secs: 600,
                    kept: 200,
                    history_max_bytes: 16_777_216,
                },
                now,
            )
            .await
            .unwrap();
            assert!(outcome.is_kept(), "{outcome:?}");
        }
        id
    }
}

/// What keeps the files of copies and writes files into workspaces, for the tests: it remembers them.
#[derive(Default)]
pub struct MemoryParcels {
    /// The copies kept: the copy, where it is to be written, and its files.
    pub kept: std::sync::Mutex<Vec<(Id, String, orch_engine::copies::Files)>>,
    /// The copies taken out of a workspace.
    pub withdrawn: std::sync::Mutex<Vec<Id>>,
    /// The files written into workspaces, by thread and path.
    pub files: std::sync::Mutex<std::collections::HashMap<(Id, String), Vec<u8>>>,
}

#[async_trait::async_trait]
impl orch_engine::sharing::Parcels for MemoryParcels {
    async fn keep(
        &self,
        _: &mut orch_db::Conn,
        copy: Id,
        path: &str,
        files: &orch_engine::copies::Files,
    ) -> Result<Option<Vec<u8>>, orch_engine::sharing::ParcelFault> {
        self.kept
            .lock()
            .unwrap()
            .push((copy, path.to_owned(), files.clone()));
        Ok(None)
    }

    async fn withdraw(
        &self,
        _: &mut orch_db::Conn,
        copy: &orch_db::queries::shares::CopyRow,
    ) -> Result<(), orch_engine::sharing::ParcelFault> {
        self.withdrawn.lock().unwrap().push(copy.id);
        Ok(())
    }

    async fn write_file(
        &self,
        _: &mut orch_db::Conn,
        thread: Id,
        path: &str,
        bytes: &[u8],
    ) -> Result<orch_engine::sharing::FileWritten, orch_engine::sharing::ParcelFault> {
        let mut files = self.files.lock().unwrap();
        match files.get(&(thread, path.to_owned())) {
            Some(there) if there.as_slice() != bytes => {
                Ok(orch_engine::sharing::FileWritten::Differs)
            }
            Some(_) => Ok(orch_engine::sharing::FileWritten::Ready),
            None => {
                files.insert((thread, path.to_owned()), bytes.to_vec());
                Ok(orch_engine::sharing::FileWritten::Ready)
            }
        }
    }
}
