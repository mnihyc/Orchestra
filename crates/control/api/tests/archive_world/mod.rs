//! A small world for the tests of exports over HTTP: the usual scene (Ada owns an org with a topic, Cai reads, Bea is a stranger), a thread that was talked to twice, a document and two chats beside it, and short ways to ask for an archive, wait for it, fetch it and read what it holds.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

use crate::places::Who;
use crate::rig::{Scene, eventually};
use orch_api_contract::chats::{ChatMessageCreate, ChatMessageCreateBody, ChatPart};
use orch_api_contract::documents::{Document, NodeDocumentCreate, NodeDocumentCreateBody};
use orch_api_contract::exports::{
    Export, ExportDelete, ExportDownload, ExportEverything, ExportGet, ExportPlan, ExportRequest,
    ExportState, ExportWhat, MeExportList, NodeExportCreate, NodeExportPreflight,
};
use orch_api_contract::nodes::JobState;
use orch_api_contract::threads::Thread;
use orch_api_contract::workspace::{Job, JobGet};
use orch_api_core::routes::exports::ApiForms;
use orch_api_core::testing::{RawReply, Reply};
use orch_types::{Id, new_id};
use std::collections::BTreeMap;
use std::io::Read;
use std::sync::Arc;

/// What a test of exports stands in.
pub struct Office {
    pub s: Scene,
    /// The thread "Fix login" that Ada talked to twice.
    pub thread: Thread,
    /// A document Ada wrote beside the topic, "Handbook".
    pub document: Document,
    /// The chat of the topic and the chat of the thread, each with a message of Ada's.
    pub topic_chat: Id,
    pub thread_chat: Id,
}

impl Office {
    pub async fn new() -> Self {
        let s = Scene::new().await;
        Self::ready(&s).await;
        let thread = s.thread().await;
        s.exchange(thread.base.id, 0, "First question", "First answer")
            .await;
        s.exchange(thread.base.id, 1, "Second question", "Second answer")
            .await;
        let document =
            Self::write_document(&s, &s.ada, "Handbook", "Be kind.\n\n- one\n- two\n").await;
        let topic_chat = chat_of(&s, s.tree.topic.base.id).await;
        let thread_chat = thread.base.chat_id;
        say(&s, &s.ada, topic_chat, "Topic chat: keep to the plan.").await;
        say(&s, &s.ada, thread_chat, "Thread chat: the login is fixed.").await;
        Self {
            s,
            thread,
            document,
            topic_chat,
            thread_chat,
        }
    }

    /// What the engine of the scene needs to write an archive: the store of content, and what writes a document and
    /// a thread as their single exports do.
    pub async fn ready(s: &Scene) {
        s.rig.engine.attach_blobs(s.rig.world.app.app.blobs.store());
        s.rig.engine.attach_forms(Arc::new(ApiForms));
    }

    /// A document of `by` beside the topic, written from Markdown.
    pub async fn write_document(s: &Scene, by: &Who, title: &str, markdown: &str) -> Document {
        s.rig
            .world
            .app
            .call(NodeDocumentCreate, &[&s.tree.topic.base.id])
            .token(&by.token)
            .body(&NodeDocumentCreateBody {
                title: title.to_owned(),
                blocks: None,
                markdown: Some(markdown.to_owned()),
                access: None,
            })
            .send()
            .await
            .ok()
    }

    pub fn topic(&self) -> Id {
        self.s.tree.topic.base.id
    }

    pub fn org(&self) -> Id {
        self.s.tree.org.base.id
    }

    // ------------------------------------------------------------ asking

    pub async fn plan(&self, by: &Who, place: Id, request: &ExportRequest) -> Reply<ExportPlan> {
        self.s
            .rig
            .world
            .app
            .call(NodeExportPreflight, &[&place])
            .token(&by.token)
            .body(request)
            .send()
            .await
    }

    pub async fn ask(&self, by: &Who, place: Id, request: &ExportRequest) -> Reply<Job> {
        self.s
            .rig
            .world
            .app
            .call(NodeExportCreate, &[&place])
            .token(&by.token)
            .body(request)
            .send()
            .await
    }

    /// Waits until the job has ended and says how.
    pub async fn ended(&self, by: &Who, job: Id) -> Job {
        let app = &self.s.rig.world.app;
        let token = by.token.clone();
        eventually("the job that writes the archive ends", || {
            let call = app.call(JobGet, &[&job]).token(&token);
            async move {
                let read = call.send().await.ok();
                matches!(
                    read.state,
                    JobState::Succeeded | JobState::Failed | JobState::Cancelled
                )
            }
        })
        .await;
        app.call(JobGet, &[&job]).token(&token).send().await.ok()
    }

    /// Asks for an archive, waits for it, and reads it: it has to be ready.
    pub async fn made(&self, by: &Who, place: Id, request: &ExportRequest) -> Export {
        let job = self.ask(by, place, request).await.ok();
        let job = self.ended(by, job.id).await;
        assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);
        let id: Id = job.result.as_ref().unwrap()["export_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let export = self.read(by, id).await.ok();
        assert_eq!(export.state, ExportState::Ready);
        export
    }

    pub async fn read(&self, by: &Who, id: Id) -> Reply<Export> {
        self.s
            .rig
            .world
            .app
            .call(ExportGet, &[&id])
            .token(&by.token)
            .send()
            .await
    }

    pub async fn list(&self, by: &Who) -> Vec<Export> {
        self.s
            .rig
            .world
            .app
            .call(MeExportList, &[])
            .token(&by.token)
            .query(&orch_api_contract::common::PageQuery::default())
            .send()
            .await
            .ok()
            .items
    }

    pub async fn fetch(&self, by: &Who, id: Id) -> RawReply {
        self.s
            .rig
            .world
            .app
            .call(ExportDownload, &[&id])
            .token(&by.token)
            .send_bytes()
            .await
    }

    pub async fn remove(&self, by: &Who, id: Id) -> RawReply {
        self.s
            .rig
            .world
            .app
            .call(ExportDelete, &[&id])
            .token(&by.token)
            .send_bytes()
            .await
    }

    /// What a ready archive holds, by path.
    pub async fn contents(&self, by: &Who, export: &Export) -> BTreeMap<String, Vec<u8>> {
        let reply = self.fetch(by, export.id).await;
        assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.raw));
        entries(&reply)
    }
}

/// The request that asks for everything in a place that the caller may read, with nothing else said.
pub fn everything() -> ExportRequest {
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

/// Stores `bytes` in the store of content and registers them, as an upload would have: where they are, and how many.
pub async fn kept(s: &Scene, bytes: &[u8]) -> (orch_types::Hash, i64) {
    let blobs = &s.rig.world.app.app.blobs;
    let meta = blobs
        .put_bytes(bytes::Bytes::copy_from_slice(bytes))
        .await
        .unwrap();
    let mut conn = s.rig.db.acquire().await.unwrap();
    blobs
        .register(&mut conn, meta, s.rig.world.now())
        .await
        .unwrap();
    (meta.hash, meta.size)
}

/// Where a link in the text kept in the file `text_file` of an archive leads, as a path of the archive: from the
/// directory the file lies in, `..` going one up.
pub fn leads_to(text_file: &str, link: &str) -> String {
    let mut path: Vec<&str> = text_file.split('/').collect();
    path.pop();
    for piece in link.split('/') {
        match piece {
            "" | "." => {}
            ".." => {
                path.pop();
            }
            other => path.push(other),
        }
    }
    path.join("/")
}

/// Every link in a written text that leads to a file beside it: a Markdown link or image whose address is a relative
/// path, and the path at the end of a line `[image: <name> · <path>]` or `[file: <name> · <size> · <path>]`, quoted
/// or not. What a link names is not looked for: each is returned as the text wrote it.
pub fn links_of(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("](") {
        rest = &rest[at + 2..];
        let (address, after) = if let Some(inner) = rest.strip_prefix('<') {
            match inner.find('>') {
                Some(end) => (&inner[..end], &inner[end + 1..]),
                None => break,
            }
        } else {
            match rest.find(')') {
                Some(end) => (&rest[..end], &rest[end..]),
                None => break,
            }
        };
        if !address.contains("://") && !address.starts_with('#') && !address.is_empty() {
            found.push(address.to_owned());
        }
        rest = after;
    }
    for line in text.lines() {
        let line = line.trim().trim_start_matches('>').trim();
        if (line.starts_with("[image: ") || line.starts_with("[file: "))
            && line.ends_with(']')
            && let Some((_, path)) = line.trim_end_matches(']').rsplit_once(" \u{b7} ")
            && path.contains(".files/")
        {
            found.push(path.to_owned());
        }
    }
    found
}

/// The entries of a zip, by path.
pub fn entries(reply: &RawReply) -> BTreeMap<String, Vec<u8>> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(reply.raw.to_vec())).unwrap();
    let mut out = BTreeMap::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).unwrap();
        let mut content = Vec::new();
        file.read_to_end(&mut content).unwrap();
        out.insert(file.name().unwrap().into_owned(), content);
    }
    out
}

/// The one entry whose path ends in `tail`.
pub fn entry<'a>(held: &'a BTreeMap<String, Vec<u8>>, tail: &str) -> &'a [u8] {
    let mut found = held.iter().filter(|(path, _)| path.ends_with(tail));
    let (_, bytes) = found
        .next()
        .unwrap_or_else(|| panic!("no entry ends in {tail}: {:?}", held.keys()));
    assert!(found.next().is_none(), "more than one entry ends in {tail}");
    bytes
}

/// The one entry whose path contains `head` and ends in `tail`.
pub fn entry_with<'a>(held: &'a BTreeMap<String, Vec<u8>>, head: &str, tail: &str) -> &'a [u8] {
    let mut found = held
        .iter()
        .filter(|(path, _)| path.contains(head) && path.ends_with(tail));
    let (_, bytes) = found
        .next()
        .unwrap_or_else(|| panic!("no entry has {head} and ends in {tail}: {:?}", held.keys()));
    assert!(
        found.next().is_none(),
        "more than one entry has {head} and ends in {tail}"
    );
    bytes
}

/// Whether any entry has a path that ends in `tail`.
pub fn has(held: &BTreeMap<String, Vec<u8>>, tail: &str) -> bool {
    held.keys().any(|path| path.ends_with(tail))
}

pub fn text_of(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// The chat a node has.
pub async fn chat_of(s: &Scene, node: Id) -> Id {
    match crate::places::read(&s.rig.world, &s.ada, node).await {
        orch_api_contract::nodes::Node::Org(node) => node.base.chat_id,
        orch_api_contract::nodes::Node::Project(node) => node.base.chat_id,
        orch_api_contract::nodes::Node::Topic(node) => node.base.chat_id,
        orch_api_contract::nodes::Node::Thread(node) => node.base.chat_id,
    }
}

/// `by` says `words` in `chat`.
pub async fn say(s: &Scene, by: &Who, chat: Id, words: &str) {
    s.rig
        .world
        .app
        .call(ChatMessageCreate, &[&chat])
        .token(&by.token)
        .body(&ChatMessageCreateBody {
            agent_review: None,
            parts: vec![ChatPart::Text {
                text: words.to_owned(),
            }],
            reply_to: None,
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
}
