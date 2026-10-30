//! A small world for the tests of chats over HTTP: an org with a topic and the people who stand in different places towards it, and short ways to say things in a chat and to read what was said.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

use crate::places::{self, Who, join, make_org, make_project, make_topic, person, read};
use crate::support::World;
use orch_api_contract::chats::{
    Chat, ChatGet, ChatMessage, ChatMessageCreate, ChatMessageCreateBody, ChatMessageCreateReply,
    ChatPart, ChatSync, ChatSyncGet, ChatSyncGetQuery, OrgChatCreate, OrgChatCreateBody,
};
use orch_api_contract::nodes::Node;
use orch_api_core::testing::Reply;
use orch_types::{Id, new_id};

/// An org of Ada's, its General project, a project and a topic of hers; Bea, Cai and Dee are members
/// of the org (a viewer, a developer, an admin of the org besides Ada) and so is Gus, another viewer; Eve and Fay are
/// strangers.
pub struct Room {
    pub world: World,
    pub ada: Who,
    pub bea: Who,
    pub cai: Who,
    pub dee: Who,
    pub eve: Who,
    pub fay: Who,
    pub gus: Who,
    pub root: Id,
    pub project: Id,
    pub topic: Id,
    /// The chat of the topic.
    pub chat: Id,
}

/// The id of the chat a node has.
pub async fn chat_of(world: &World, by: &Who, node: Id) -> Id {
    match read(world, by, node).await {
        Node::Org(node) => node.base.chat_id,
        Node::Project(node) => node.base.chat_id,
        Node::Topic(node) => node.base.chat_id,
        Node::Thread(node) => node.base.chat_id,
    }
}

impl Room {
    pub async fn new() -> Self {
        let world = World::new().await;
        let ada = person(&world, "ada").await;
        let bea = person(&world, "bea").await;
        let cai = person(&world, "cai").await;
        let dee = person(&world, "dee").await;
        let eve = person(&world, "eve").await;
        let fay = person(&world, "fay").await;
        let gus = person(&world, "gus").await;
        let org = make_org(&world, &ada, "Acme", "acme").await;
        let root = org.base.id;
        join(&world, root, root, &bea, "viewer", &ada).await;
        join(&world, root, root, &cai, "developer", &ada).await;
        join(&world, root, root, &dee, "admin", &ada).await;
        join(&world, root, root, &gus, "viewer", &ada).await;
        let project = make_project(&world, &ada, root, "Web").await.base.id;
        let topic = make_topic(&world, &ada, project, "Login").await.base.id;
        let chat = chat_of(&world, &ada, topic).await;
        Self {
            world,
            ada,
            bea,
            cai,
            dee,
            eve,
            fay,
            gus,
            root,
            project,
            topic,
            chat,
        }
    }

    /// `by` says `words` in `chat`; the message.
    pub async fn say(&self, by: &Who, chat: Id, words: &str) -> ChatMessage {
        self.send(by, chat, vec![text(words)], &new_id().to_string())
            .await
            .ok()
            .message
    }

    /// `by` sends `parts` in `chat` under `client_id`.
    pub async fn send(
        &self,
        by: &Who,
        chat: Id,
        parts: Vec<ChatPart>,
        client_id: &str,
    ) -> Reply<ChatMessageCreateReply> {
        self.world
            .app
            .call(ChatMessageCreate, &[&chat])
            .token(&by.token)
            .body(&ChatMessageCreateBody {
                agent_review: None,
                parts,
                reply_to: None,
                client_id: client_id.to_owned(),
            })
            .send()
            .await
    }

    /// `by` reads the chat.
    pub async fn chat(&self, by: &Who, chat: Id) -> Chat {
        self.world
            .app
            .call(ChatGet, &[&chat])
            .token(&by.token)
            .send()
            .await
            .ok()
    }

    /// `by` opens a direct chat with `other`; the chat.
    pub async fn direct(&self, by: &Who, other: &Who) -> Chat {
        self.world
            .app
            .call(OrgChatCreate, &[&self.root])
            .token(&by.token)
            .body(&OrgChatCreateBody {
                user_ids: vec![other.id()],
                name: None,
            })
            .send()
            .await
            .ok()
    }

    /// `by` opens a group with `others`; the chat.
    pub async fn group(&self, by: &Who, others: &[&Who], name: Option<&str>) -> Chat {
        self.world
            .app
            .call(OrgChatCreate, &[&self.root])
            .token(&by.token)
            .body(&OrgChatCreateBody {
                user_ids: others.iter().map(|other| other.id()).collect(),
                name: name.map(str::to_owned),
            })
            .send()
            .await
            .ok()
    }

    /// What `by` sees of the chat now, from the start.
    pub async fn sync(&self, by: &Who, chat: Id) -> ChatSync {
        self.world
            .app
            .call(ChatSyncGet, &[&chat])
            .token(&by.token)
            .query(&ChatSyncGetQuery::default())
            .send()
            .await
            .ok()
    }

    /// The words of the text parts of a message.
    pub fn words(message: &ChatMessage) -> String {
        Self::words_of(&message.parts)
    }

    /// The words of some parts.
    pub fn words_of(parts: &[ChatPart]) -> String {
        parts
            .iter()
            .filter_map(|part| match part {
                ChatPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The chat of a topic of another project, which nobody is engaged in.
    pub async fn chat_in_another_topic(&self) -> Id {
        let project = make_project(&self.world, &self.ada, self.root, "Mobile").await;
        let topic = make_topic(&self.world, &self.ada, project.base.id, "Build").await;
        chat_of(&self.world, &self.ada, topic.base.id).await
    }

    /// Puts a blob in the store, as an upload would have: its hash and its size.
    pub async fn store(&self, content: &[u8]) -> (orch_types::Hash, i64) {
        let hash = orch_types::Hash::of(content);
        let size = i64::try_from(content.len()).unwrap();
        sqlx::query("INSERT INTO blobs (hash, size) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(hash.as_bytes().to_vec())
            .bind(size)
            .execute(&self.world.db.pool)
            .await
            .unwrap();
        let mut conn = self.world.conn().await;
        orch_db::queries::uploads::remember(&mut conn, self.bea.id(), &hash, self.world.now())
            .await
            .unwrap();
        (hash, size)
    }

    /// How many messages hold a blob.
    pub async fn pins(&self, hash: &orch_types::Hash) -> i32 {
        sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash = $1")
            .bind(hash.as_bytes().to_vec())
            .fetch_one(&self.world.db.pool)
            .await
            .unwrap()
    }

    /// The titles of the rows of a person's Inbox with the kind given, the oldest first.
    pub async fn inbox(&self, who: &Who, kind: &str) -> Vec<(String, String, Option<Id>, bool)> {
        sqlx::query_as(
            "SELECT title, body, chat_id, read_at IS NOT NULL FROM notifications
              WHERE user_id = $1 AND kind = $2 ORDER BY inbox_seq",
        )
        .bind(who.id())
        .bind(kind)
        .fetch_all(&self.world.db.pool)
        .await
        .unwrap()
    }

    /// The audit actions of the rows about a chat.
    pub async fn audit_of(&self, chat: Id) -> Vec<String> {
        sqlx::query_scalar("SELECT action FROM audit_log WHERE object_id = $1 ORDER BY at, id")
            .bind(chat)
            .fetch_all(&self.world.db.pool)
            .await
            .unwrap()
    }
}

pub fn text(words: &str) -> ChatPart {
    ChatPart::Text {
        text: words.to_owned(),
    }
}

#[allow(unused_imports, reason = "each test file uses some of what is here")]
pub use places::missing_permissions;
