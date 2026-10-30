//! A small office for the tests of documents over HTTP: an org with a project and a topic, people at each role, and helpers that make a document and write blocks into it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

use crate::places::{Who, join, make_org, make_project, make_topic, person};
use crate::support::World;
use orch_api_contract::documents::{
    BlockContent, BlockType, Document, DocumentBlock, DocumentOp, DocumentOpsCreate,
    DocumentOpsCreateBody, DocumentOpsCreateReply, DocumentSync, DocumentSyncGet,
    DocumentSyncGetQuery, Inline, NewBlock, NodeDocumentCreate, NodeDocumentCreateBody,
};
use orch_api_core::testing::Reply;
use orch_types::{Id, new_id};

/// The office: Ada owns the org, the project and the topic hold nothing of their own.
pub struct Desk {
    pub world: World,
    pub ada: Who,
    pub org: Id,
    pub project: Id,
    pub topic: Id,
}

impl Desk {
    /// An org of Ada's with a project and a topic in it.
    pub async fn new() -> Self {
        let world = World::new().await;
        let ada = person(&world, "ada").await;
        let org = make_org(&world, &ada, "Acme", "acme").await;
        let project = make_project(&world, &ada, org.base.id, "Web").await;
        let topic = make_topic(&world, &ada, project.base.id, "Plans").await;
        Self {
            world,
            ada,
            org: org.base.id,
            project: project.base.id,
            topic: topic.base.id,
        }
    }

    /// A person who is a member of the org with `role`, given where the org begins.
    pub async fn member(&self, username: &str, role: &str) -> Who {
        let who = person(&self.world, username).await;
        join(&self.world, self.org, self.org, &who, role, &self.ada).await;
        who
    }

    /// A person who holds nothing here at all.
    pub async fn stranger(&self, username: &str) -> Who {
        person(&self.world, username).await
    }

    /// A topic of the same project, for a document to be moved to.
    pub async fn another_topic(&self, name: &str) -> Id {
        make_topic(&self.world, &self.ada, self.project, name)
            .await
            .base
            .id
    }

    /// An empty document beside the topic, made by `by`.
    pub async fn make(&self, by: &Who, title: &str) -> Document {
        self.make_body(by, body(title)).await.ok()
    }

    /// A document made with a body of the caller's own.
    pub async fn make_body(&self, by: &Who, body: NodeDocumentCreateBody) -> Reply<Document> {
        self.make_at(by, self.topic, body).await
    }

    /// A document beside any node.
    pub async fn make_at(
        &self,
        by: &Who,
        node: Id,
        body: NodeDocumentCreateBody,
    ) -> Reply<Document> {
        self.world
            .app
            .call(NodeDocumentCreate, &[&node])
            .token(&by.token)
            .body(&body)
            .send()
            .await
    }

    /// Writes a batch, with a fresh `client_id`.
    pub async fn write(
        &self,
        by: &Who,
        document: Id,
        ops: Vec<DocumentOp>,
    ) -> Reply<DocumentOpsCreateReply> {
        self.world
            .app
            .call(DocumentOpsCreate, &[&document])
            .token(&by.token)
            .body(&DocumentOpsCreateBody {
                expected_seq: None,
                client_id: new_id().to_string(),
                ops,
            })
            .send()
            .await
    }

    /// Writes paragraphs one after the other at the top of a document and
    /// returns the blocks as they stand.
    pub async fn paragraphs(&self, by: &Who, document: Id, words: &[&str]) -> Vec<DocumentBlock> {
        let mut after: Option<Id> = None;
        let mut ops = Vec::new();
        for each in words {
            let block = paragraph(each);
            let id = block.id;
            ops.push(DocumentOp::Insert {
                block,
                parent_id: None,
                after_id: after,
            });
            after = Some(id);
        }
        let mut written = self.write(by, document, ops).await.ok().blocks;
        written.sort_by(|a, b| position_order(&a.position, &b.position));
        written
    }

    /// The document and every live block, as `by` reads them.
    pub async fn sync(&self, by: &Who, document: Id) -> DocumentSync {
        self.world
            .app
            .call(DocumentSyncGet, &[&document])
            .token(&by.token)
            .query(&DocumentSyncGetQuery::default())
            .send()
            .await
            .ok()
    }

    /// The live top-level blocks of a document in the order of the page.
    pub async fn page(&self, by: &Who, document: Id) -> Vec<DocumentBlock> {
        let mut blocks = self.sync(by, document).await.blocks;
        blocks.retain(|block| block.parent_id.is_none() && !block.removed);
        blocks.sort_by(|a, b| position_order(&a.position, &b.position));
        blocks
    }

    /// The words of the live top-level blocks in order.
    pub async fn words(&self, by: &Who, document: Id) -> Vec<String> {
        self.page(by, document).await.iter().map(said).collect()
    }
}

/// The body of a request that makes an empty document.
pub fn body(title: &str) -> NodeDocumentCreateBody {
    NodeDocumentCreateBody {
        title: title.to_owned(),
        blocks: None,
        markdown: None,
        access: None,
    }
}

/// Plain text as inline content.
pub fn text(words: &str) -> Vec<Inline> {
    vec![Inline::Text {
        text: words.to_owned(),
        marks: None,
        href: None,
        color: None,
        background: None,
    }]
}

/// A new paragraph that says `words`.
pub fn paragraph(words: &str) -> NewBlock {
    NewBlock {
        id: new_id(),
        block_type: BlockType::Paragraph,
        props: None,
        content: Some(BlockContent::Inline(text(words))),
        children: None,
    }
}

/// Puts a block first among the top-level blocks.
pub fn put(block: NewBlock) -> DocumentOp {
    DocumentOp::Insert {
        block,
        parent_id: None,
        after_id: None,
    }
}

/// Puts a block after another among the top-level blocks.
pub fn put_after(block: NewBlock, after: Id) -> DocumentOp {
    DocumentOp::Insert {
        block,
        parent_id: None,
        after_id: Some(after),
    }
}

/// Puts a block under another, at its end.
pub fn put_under(block: NewBlock, parent: Id) -> DocumentOp {
    DocumentOp::Insert {
        block,
        parent_id: Some(parent),
        after_id: None,
    }
}

/// Rewrites the words of a block that was last stamped `base_seq`.
pub fn rewrite(block: &DocumentBlock, words: &str) -> DocumentOp {
    DocumentOp::Update {
        id: block.id,
        base_seq: block.seq_updated,
        block_type: None,
        props: None,
        content: Some(BlockContent::Inline(text(words))),
    }
}

/// Takes a block out.
pub fn remove(block: &DocumentBlock) -> DocumentOp {
    DocumentOp::Remove { id: block.id }
}

/// The words of a block, as plain text.
pub fn said(block: &DocumentBlock) -> String {
    match &block.content {
        BlockContent::Inline(runs) => runs
            .iter()
            .map(|run| match run {
                Inline::Text { text, .. } => text.clone(),
                Inline::Mention { user, .. } => user
                    .as_ref()
                    .map(|user| format!("@{}", user.username))
                    .unwrap_or_default(),
                _ => String::new(),
            })
            .collect(),
        BlockContent::Rows(_) => String::new(),
    }
}

/// Orders two positions as the numbers they are.
pub fn position_order(a: &str, b: &str) -> std::cmp::Ordering {
    use orch_documents::position::Position;
    match (Position::parse(a), Position::parse(b)) {
        (Some(a), Some(b)) => a.cmp(&b),
        _ => a.cmp(b),
    }
}
