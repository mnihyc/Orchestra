//! The shared tree: what is in an agent's reach as paths, from everything it may read or from its own topic: neighbouring threads by policy, the documents, chats and conversations people shared, and the files each carries; listed with what each entry is, how it is counted and whether it may be written.
//!
//! `list_shared` lists the directory at `path`: the agent's own topic when it is left out, `/` for everything.
//! It goes `depth` levels down in pages, active work and then recent changes first unless names are asked for;
//! `next_cursor` continues the same query and archived entries are counted unless included. What it lists is what the agent may read, laid out by [`tree`]; the
//! other threads of its own topic are always listed, `readable: false` where it may not read them. Listing is not
//! audited and tells nobody.
//!
//! The same tree is what a person is shown of what an agent reaches (`GET /threads/{id}/shared/tree`), so that
//! they see exactly what the agent can read and write: [`Engine::agent_tree`] reads it for a thread and not for a
//! call, and the request leaves out the names of what the person cannot see themself.

use crate::engine::Engine;
use crate::error::EngineError;
use crate::tools::definitions::{Code, Refusal};
use crate::tools::tree::{self, Entry, FoundEntry, Listing, Named, Pattern, Scope, Tree, Where};
use crate::tools::{Answer, CallContext};
use orch_access_contract::audit::AuditAction;
use orch_api_contract::share::SharedEntryKind;
use orch_authz::reach::{Agent, AgentReach, DocumentPolicy};
use orch_db::Conn;
use orch_db::audit::{Actor, Entry as AuditEntry, Origin, append};
use orch_db::queries::identity;
use orch_db::queries::threads::ThreadRow;
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;

/// How many names are offered to a model whose path names nothing.
pub const CLOSEST_NAMES: usize = 8;

/// The arguments of `list_shared`, once the schema has let them through.
#[derive(Debug, Deserialize)]
struct Arguments {
    path: Option<String>,
    depth: Option<i64>,
    kinds: Option<Vec<SharedEntryKind>>,
    names: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
    sort: Option<tree::Sort>,
    include_archived: Option<bool>,
    ids: Option<bool>,
}

/// Who asks, and what their reach is made of.
#[derive(Debug, Clone)]
pub struct Asking {
    /// The agent.
    pub agent: Agent,
    /// What `sharing.agent_reach` says at its topic.
    pub policy: AgentReach,
    /// Independently resolved public topic-document reach.
    pub documents_policy: DocumentPolicy,
    /// The moment of the call.
    pub now: Time,
}

impl Engine {
    /// The agent of a thread and the policy of its topic, at this moment; nothing where the thread is not there.
    pub(crate) async fn asking(
        &self,
        conn: &mut Conn,
        thread: Id,
    ) -> Result<Option<Asking>, EngineError> {
        let Some(agent) = Agent::of(conn, thread).await? else {
            return Ok(None);
        };
        let policy =
            orch_settings::get::<String>(conn, Place::Node(agent.topic), keys::SHARING_AGENT_REACH)
                .await?
                .and_then(|word| AgentReach::parse(&word))
                .unwrap_or_default();
        Ok(Some(Asking {
            documents_policy: orch_settings::get::<String>(
                conn,
                Place::Node(agent.topic),
                keys::SHARING_AGENT_DOCUMENTS,
            )
            .await?
            .as_deref()
            .and_then(DocumentPolicy::parse)
            .unwrap_or(DocumentPolicy::Write),
            agent,
            policy,
            now: Time::new(self.inner.clock.now()),
        }))
    }

    /// Reads the tree of an agent for what a path names: the places on the path are followed by name first, and
    /// the tree is read under the deepest of them. The path comes back written from the top.
    pub(crate) async fn tree_for(
        &self,
        conn: &mut Conn,
        asking: &Asking,
        named: &Named,
    ) -> Result<(Tree, tree::Walked, Named), EngineError> {
        let named = tree::absolute(conn, &asking.agent, named).await?;
        let walked = tree::walk(conn, &named).await?;
        let tree = Tree::load(
            conn,
            asking.agent.clone(),
            asking.policy,
            asking.documents_policy,
            asking.now,
            walked.scope,
        )
        .await?;
        Ok((tree, walked, named))
    }

    /// The refusal for a path that names nothing the agent reaches. It names the entries of the nearest
    /// directory that is there for the agent, and never of one that is not: a place that exists and is not the
    /// agent's to see answers exactly as one that does not exist, so the tree is read again under each place
    /// on the path, from the deepest, until one is there.
    pub(crate) async fn nothing_there(
        &self,
        conn: &mut Conn,
        asking: &Asking,
        tree: &Tree,
        walked: &tree::Walked,
        named: &Named,
        asked: &str,
    ) -> Result<Refusal, EngineError> {
        if matches!(named, Named::Id(_)) {
            return Ok(Refusal::new(
                Code::NotFound,
                format!(
                    "There is nothing with \"{asked}\" as its identifier that you reach. list_shared with \"/\" shows everything you can reach."
                ),
            ));
        }
        match walked.scope {
            Scope::Everything => return Ok(not_found(tree, named, 0, asked)),
            Scope::Under(node) if tree.directory_at(Where::Place(node)) => {
                return Ok(not_found(tree, named, walked.consumed, asked));
            }
            Scope::Under(_) => {}
        }
        for (at, node) in walked.chain.iter().enumerate().rev().skip(1) {
            let above = Tree::load(
                conn,
                asking.agent.clone(),
                asking.policy,
                asking.documents_policy,
                asking.now,
                Scope::Under(*node),
            )
            .await?;
            if above.directory_at(Where::Place(*node)) {
                return Ok(not_found(&above, named, at + 1, asked));
            }
        }
        let top = Tree::load(
            conn,
            asking.agent.clone(),
            asking.policy,
            asking.documents_policy,
            asking.now,
            Scope::Everything,
        )
        .await?;
        Ok(not_found(&top, named, 0, asked))
    }

    /// The reach of a thread's agent as a listing, for a person who wants to see exactly what the agent sees: the
    /// listing, the tree it was read from, and the directory it lists.
    pub async fn agent_tree(
        &self,
        thread: Id,
        path: Option<&str>,
        depth: u32,
    ) -> Result<Option<(Listing, Tree, Where)>, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let Some(asking) = self.asking(&mut conn, thread).await? else {
            return Ok(None);
        };
        let Some(named) = path.map_or_else(
            || {
                Some(Named::Path {
                    absolute: true,
                    parts: Vec::new(),
                })
            },
            tree::parse_path,
        ) else {
            return Ok(None);
        };
        let (tree, walked, named) = self.tree_for(&mut conn, &asking, &named).await?;
        let Some(found) = tree.find(&named, walked.consumed) else {
            return Ok(None);
        };
        let FoundEntry::Directory(at) = found.entry else {
            return Ok(None);
        };
        if !tree.directory_at(at) {
            return Ok(None);
        }
        let mut listing = tree
            .list(&mut conn, at, &found.path, depth, None, None)
            .await?;
        describe_documents(&mut conn, &mut listing.entries).await?;
        Ok(Some((listing, tree, at)))
    }

    /// `list_shared`: the call of an agent.
    pub(crate) async fn list_shared_call(
        &self,
        call: &CallContext,
        thread: &ThreadRow,
    ) -> Result<Answer, EngineError> {
        let arguments: Arguments = match serde_json::from_value(call.arguments.clone()) {
            Ok(arguments) => arguments,
            Err(error) => return Ok(Refusal::invalid(error.to_string()).into()),
        };
        let depth = u32::try_from(arguments.depth.unwrap_or(1).clamp(1, 4)).unwrap_or(1);
        let limit = arguments.limit.unwrap_or(50);
        if !(1..=super::definitions::PAGE_MAX).contains(&limit) {
            return Ok(Refusal::invalid("limit must be between 1 and 200").into());
        }
        let limit = usize::try_from(limit).unwrap_or(50);
        let sort = arguments.sort.unwrap_or_default();
        let include_archived = arguments.include_archived.unwrap_or(false);
        let pattern = match arguments.names.as_deref().map(Pattern::new) {
            Some(None) => {
                return Ok(Refusal::invalid(
                    "\"names\" is not a pattern: it takes * ? and [...], as a shell has them.",
                )
                .into());
            }
            Some(Some(pattern)) => Some(pattern),
            None => None,
        };
        let asked = arguments.path.as_deref().unwrap_or(".");
        let mut conn = self.inner.db.acquire().await?;
        let LocatedPath {
            asking,
            tree,
            found,
            walked,
            named,
            also_named_so,
        } = match self
            .locate_shared(&mut conn, thread.node_id, asked, false, false)
            .await?
        {
            Ok(located) => located,
            Err(refusal) => return Ok(refusal.into()),
        };
        let at = match found.entry {
            FoundEntry::Directory(at) if tree.directory_at(at) => at,
            FoundEntry::Directory(_) => {
                let refusal = self
                    .nothing_there(&mut conn, &asking, &tree, &walked, &named, asked)
                    .await?;
                return Ok(refusal.into());
            }
            FoundEntry::File(_) => {
                return Ok(Refusal::new(
                    Code::WrongKind,
                    format!(
                        "{} is a file, not a directory. Read it with read_shared, or copy it with download_shared.",
                        found.path
                    ),
                )
                .into());
            }
        };
        let query=json!({"path":found.path,"depth":depth,"kinds":arguments.kinds,"names":arguments.names,"sort":sort,"include_archived":include_archived}).to_string();
        let binding = crate::ids::derived(
            "list_shared.page",
            &[thread.node_id.as_bytes(), query.as_bytes()],
        )
        .to_string();
        let after=match arguments.cursor.as_deref() {
            None => None,
            Some(cursor) => match cursor.split_once('|').filter(|(tag,path)| *tag == binding && !path.is_empty()) {
                Some((_,path)) => Some(path),
                None => return Ok(Refusal::invalid("this cursor belongs to another thread or listing; use its original path, depth, filters, sort and archive choice").into()),
            },
        };
        let Some(page) = tree
            .list_page(
                &mut conn,
                at,
                &found.path,
                depth,
                arguments.kinds.as_deref(),
                pattern.as_ref(),
                tree::PageOptions {
                    sort,
                    include_archived,
                    limit,
                    after,
                },
            )
            .await?
        else {
            return Ok(Refusal::invalid("the directory changed or its last entry is no longer readable; list again without a cursor").into());
        };
        let mut listing = page.listing;
        describe_documents(&mut conn, &mut listing.entries).await?;
        let sharers = names_of(&mut conn, &listing.entries).await?;
        let mut value = listing_json(&listing, &sharers, arguments.ids.unwrap_or(false));
        if let Some(entries) = value["entries"].as_array_mut() {
            for entry in entries {
                if let Some(path) = entry["path"].as_str().map(str::to_owned) {
                    entry["cursor"] = json!(format!("{binding}|{path}"));
                }
            }
        }
        value["partial"] = json!(page.next.is_some());
        value["next_cursor"] = json!(page.next.map(|path| format!("{binding}|{path}")));
        value["archived_count"] = json!(page.archived_count);
        value["sort"] = json!(sort);
        if let Some(notice) = also_named_so {
            value["also_named_so"] = json!(notice);
        }
        Ok(Answer::Done(value))
    }
}

/// What a path names, once it is known that the agent may read it.
#[derive(Debug, Clone)]
pub(crate) enum Target {
    /// A directory that is there, and where it stands.
    Directory(Where),
    /// A thread's conversation.
    Conversation {
        /// The thread.
        thread: Id,
    },
    /// The chat of a node.
    NodeChat {
        /// The node.
        node: Id,
        /// The chat.
        chat: Id,
    },
    /// A direct or group chat.
    PeopleChat {
        /// The chat.
        chat: Id,
    },
    /// A document.
    Document {
        /// The document.
        document: Id,
    },
    /// A current attachment, authorized through its owning document.
    DocumentFile { document: Id, file: Id },
    /// A carried chat file in written order; node identifies a node chat's existing export door.
    ChatFile {
        chat: Id,
        node: Option<Id>,
        message: Id,
        carried: usize,
    },
    /// One earlier version of a document.
    Version {
        /// The document.
        document: Id,
        /// Its number.
        number: i32,
    },
}

/// The one shared-tree lookup, before a tool applies its own read/write operation.
pub(crate) struct LocatedPath {
    /// The asking agent and its current scoped policy.
    pub asking: Asking,
    /// Only the currently reachable tree and visible paths.
    pub tree: Tree,
    /// The selected, canonical entry.
    pub found: tree::Found,
    /// The path prefix already followed before the reach was loaded.
    pub walked: tree::Walked,
    /// The normalized absolute path or explicit identifier.
    pub named: Named,
    /// One line describing other reachable interpretations of a read name.
    pub also_named_so: Option<String>,
}

/// A path found, and what it names, with the tree it was found in.
#[derive(Debug)]
pub(crate) struct Resolved {
    /// Who asks.
    pub asking: Asking,
    /// The tree the path was followed in.
    pub tree: Tree,
    /// What it names.
    pub target: Target,
    /// The path, from the top.
    pub path: String,
    /// One line naming the other readable entries a bare name could also mean.
    pub also_named_so: Option<String>,
}

impl Resolved {
    /// Keeps the read's disambiguation beside its ordinary result without changing its content or positions.
    pub(crate) fn annotate(&self, mut value: Json) -> Json {
        if let Some(notice) = &self.also_named_so
            && let Some(object) = value.as_object_mut()
        {
            object.insert("also_named_so".to_owned(), Json::String(notice.clone()));
        }
        value
    }
}

/// Candidate context is read only after the shared tree has filtered reach; unshared conversations yield no text.
async fn candidate_lines(
    conn: &mut Conn,
    tree: &Tree,
    found: &[tree::Found],
) -> Result<Vec<String>, EngineError> {
    let candidates: Vec<_> = found
        .iter()
        .filter_map(|found| tree.candidate(found))
        .collect();
    let threads: Vec<_> = candidates
        .iter()
        .filter(|entry| entry.readable && entry.kind == SharedEntryKind::Thread)
        .map(|entry| entry.id)
        .collect();
    let documents: Vec<_> = candidates
        .iter()
        .filter(|entry| entry.readable && entry.kind == SharedEntryKind::Document)
        .map(|entry| entry.id)
        .collect();
    let openings = orch_db::queries::shares::entry_openings(
        conn,
        &threads,
        &documents,
        i32::try_from(tree::DESCRIPTION_CHARS).unwrap_or(i32::MAX),
    )
    .await?;
    Ok(candidates
        .iter()
        .map(|entry| {
            let first = openings
                .get(&entry.id)
                .or(entry.description.as_ref())
                .map(|text| tree::opening(text, tree::DESCRIPTION_CHARS))
                .unwrap_or_default();
            let changed = entry
                .updated_at
                .map(|at| at.get().to_rfc3339())
                .unwrap_or_else(|| "—".to_owned());
            format!(
                "{:?} · {} · {} · {} · id:{}",
                entry.path,
                entry.state.as_deref().unwrap_or("live"),
                changed,
                first,
                entry.id
            )
        })
        .collect())
}

impl Engine {
    /// The single path resolver for tool reads and writes, retaining all reachable twins until intent is known.
    pub(crate) async fn locate_shared(
        &self,
        conn: &mut Conn,
        thread: Id,
        asked: &str,
        writing: bool,
        as_file: bool,
    ) -> Result<Result<LocatedPath, Refusal>, EngineError> {
        self.locate_selected(conn, thread, asked, writing, as_file, None)
            .await
    }

    /// Resolves indexed content through the ordinary reach with only its selected source inventory.
    pub(crate) async fn locate_shared_search(
        &self,
        conn: &mut Conn,
        thread: Id,
        asked: &str,
        selected: &orch_db::queries::search::CandidateSet,
    ) -> Result<Result<LocatedPath, Refusal>, EngineError> {
        self.locate_selected(conn, thread, asked, false, false, Some(selected))
            .await
    }

    async fn locate_selected(
        &self,
        conn: &mut Conn,
        thread: Id,
        asked: &str,
        writing: bool,
        as_file: bool,
        selected: Option<&orch_db::queries::search::CandidateSet>,
    ) -> Result<Result<LocatedPath, Refusal>, EngineError> {
        let Some(named) = tree::parse_path(asked) else {
            return Ok(Err(Refusal::invalid(
                "A path is a path of the tree, from \"/\" or from your own topic, or id: and an identifier.",
            )));
        };
        let Some(asking) = self.asking(conn, thread).await? else {
            return Ok(Err(Refusal::new(
                Code::NotFound,
                "Your thread is gone; nothing can be done for it.",
            )));
        };
        let (tree, walked, named) = if let Some(selected) = selected {
            let absolute = if selected.headers_only {
                tree::absolute_headers(conn, &asking.agent, &named).await?
            } else {
                tree::absolute(conn, &asking.agent, &named).await?
            };
            let walked = if selected.headers_only {
                tree::walk_headers(conn, &absolute).await?
            } else {
                tree::walk(conn, &absolute).await?
            };
            let tree = Tree::load_search(
                conn,
                asking.agent.clone(),
                asking.policy,
                asking.documents_policy,
                asking.now,
                walked.scope,
                selected,
            )
            .await?;
            (tree, walked, absolute)
        } else {
            self.tree_for(conn, &asking, &named).await?
        };
        let mut candidates = tree.find_all(&named, walked.consumed);
        if candidates.is_empty() {
            candidates = tree.find_attachment(conn, &named, walked.consumed).await?;
        }
        let found: Vec<_> = candidates
            .into_iter()
            .filter(|found| match found.entry {
                FoundEntry::Directory(Where::Place(id))
                    if as_file && tree.place(id).is_some_and(|place| place.kind == "thread") =>
                {
                    tree.conversation_of(id)
                        .is_some_and(|(standing, _)| standing.reads())
                }
                FoundEntry::Directory(at) => tree.directory_at(at),
                FoundEntry::File(Where::Document(id) | Where::Version(id, _)) => {
                    tree.document_either(id).is_some_and(|doc| {
                        if writing {
                            doc.standing.any()
                        } else {
                            doc.standing.reads() && doc.row.deleted_at.is_none()
                        }
                    })
                }
                FoundEntry::File(Where::Conversation(id)) => tree
                    .conversation_of(id)
                    .is_some_and(|(standing, _)| standing.reads()),
                FoundEntry::File(Where::NodeChat(id)) => tree
                    .node_chat_of(id)
                    .is_some_and(|(_, standing, _)| standing.reads()),
                FoundEntry::File(Where::PeopleChat(id)) => tree
                    .people_chat_of(id)
                    .is_some_and(|chat| chat.standing.reads()),
                FoundEntry::File(Where::DocumentFile { document, .. }) => tree
                    .document(document)
                    .is_some_and(|doc| doc.standing.reads() && doc.row.deleted_at.is_none()),
                FoundEntry::File(Where::ChatFile { chat, .. }) => tree.chat_origin(chat).is_some(),
                _ => false,
            })
            .collect();
        let Some(first) = found.first().cloned() else {
            return Ok(Err(self
                .nothing_there(conn, &asking, &tree, &walked, &named, asked)
                .await?));
        };
        let also_named_so = if found.len() > 1 {
            let candidates = candidate_lines(conn, &tree, &found).await?;
            if writing {
                return Ok(Err(Refusal::new(
                    Code::Ambiguous,
                    format!(
                        "The name {asked:?} matches several entries. Use an explicit id or a suffixed path:\n{}",
                        candidates.join("\n")
                    ),
                )));
            }
            Some(format!(
                "Also named so: {}",
                candidates
                    .into_iter()
                    .skip(1)
                    .collect::<Vec<_>>()
                    .join("; ")
            ))
        } else {
            None
        };
        Ok(Ok(LocatedPath {
            asking,
            tree,
            found: first,
            walked,
            named,
            also_named_so,
        }))
    }

    /// Finds what a path names for an agent: what it may read, and nothing else. A path that names nothing, or
    /// something the agent may not read, is refused as `not_found`, in the same words. With `as_file`, an
    /// identifier that is a thread's names its conversation.
    pub(crate) async fn resolve(
        &self,
        conn: &mut Conn,
        thread: Id,
        asked: &str,
        as_file: bool,
    ) -> Result<Result<Resolved, Refusal>, EngineError> {
        self.resolve_for(conn, thread, asked, as_file, false).await
    }

    /// Resolves the same names for a write, refusing every ambiguous target before effects.
    pub(crate) async fn resolve_write(
        &self,
        conn: &mut Conn,
        thread: Id,
        asked: &str,
        as_file: bool,
    ) -> Result<Result<Resolved, Refusal>, EngineError> {
        super::source::check(conn).await?;
        self.resolve_for(conn, thread, asked, as_file, true).await
    }

    async fn resolve_for(
        &self,
        conn: &mut Conn,
        thread: Id,
        asked: &str,
        as_file: bool,
        writing: bool,
    ) -> Result<Result<Resolved, Refusal>, EngineError> {
        let LocatedPath {
            asking,
            tree,
            found,
            walked,
            named,
            also_named_so,
        } = match self
            .locate_shared(conn, thread, asked, writing, as_file)
            .await?
        {
            Ok(located) => located,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let found = Some(found);
        let target = found.as_ref().and_then(|found| match found.entry {
            FoundEntry::Directory(Where::Place(id))
                if as_file && tree.place(id).is_some_and(|place| place.kind == "thread") =>
            {
                tree.conversation_of(id)
                    .filter(|(standing, _)| standing.reads())
                    .map(|_| Target::Conversation { thread: id })
            }
            FoundEntry::Directory(at) => tree.directory_at(at).then_some(Target::Directory(at)),
            FoundEntry::File(Where::Conversation(thread)) => tree
                .conversation_of(thread)
                .filter(|(standing, _)| standing.reads())
                .map(|_| Target::Conversation { thread }),
            FoundEntry::File(Where::NodeChat(node)) => tree
                .node_chat_of(node)
                .filter(|(_, standing, _)| standing.reads())
                .map(|(chat, _, _)| Target::NodeChat { node, chat }),
            FoundEntry::File(Where::PeopleChat(chat)) => tree
                .people_chat_of(chat)
                .filter(|entry| entry.standing.reads())
                .map(|_| Target::PeopleChat { chat }),
            FoundEntry::File(Where::Document(document)) => tree
                .document(document)
                .filter(|entry| entry.standing.reads())
                .map(|_| Target::Document { document }),
            FoundEntry::File(Where::Version(document, number)) => tree
                .document(document)
                .filter(|entry| entry.standing.reads())
                .map(|_| Target::Version { document, number }),
            FoundEntry::File(Where::DocumentFile { document, file }) => tree
                .document(document)
                .filter(|entry| entry.standing.reads())
                .map(|_| Target::DocumentFile { document, file }),
            FoundEntry::File(Where::ChatFile {
                chat,
                message,
                carried,
            }) => tree.chat_origin(chat).map(|node| Target::ChatFile {
                chat,
                node,
                message,
                carried,
            }),
            FoundEntry::File(_) => None,
        });
        match (target, found) {
            (Some(target), Some(found)) => {
                let path = if as_file
                    && let Target::Conversation { .. } = &target
                    && !found.path.ends_with("conversation.md")
                {
                    format!("{}conversation.md", found.path)
                } else {
                    found.path
                };
                Ok(Ok(Resolved {
                    asking,
                    tree,
                    target,
                    path,
                    also_named_so,
                }))
            }
            _ => Ok(Err(self
                .nothing_there(conn, &asking, &tree, &walked, &named, asked)
                .await?)),
        }
    }
}

/// Writes down that an agent did something to content: the line of the audit trail with who did it and what to.
#[allow(
    clippy::too_many_arguments,
    reason = "an audit row is told by who, what, to what, where, and in what words"
)]
pub(crate) async fn audit_by_agent(
    conn: &mut Conn,
    asking: &Asking,
    action: AuditAction,
    kind: &str,
    object: Id,
    node: Option<Id>,
    summary: &str,
    details: Json,
) -> Result<(), EngineError> {
    let mut entry = AuditEntry::new(
        Actor::Agent(asking.agent.thread),
        action.as_str(),
        kind,
        Some(object),
        summary,
    );
    entry.history = action.is_history_line();
    entry.node_id = node;
    entry.org_id = Some(asking.agent.root);
    entry.details = details;
    append(conn, &entry, &Origin::default(), asking.now.get()).await?;
    Ok(())
}

impl Engine {
    /// A thread addressed through the common resolver: its own topic permits sending, elsewhere a write share does.
    pub(crate) async fn thread_shared_for_writing(
        &self,
        thread: &ThreadRow,
        asked: &str,
    ) -> Result<
        Result<(orch_db::queries::threads::tools::TopicThread, String, Id), Refusal>,
        EngineError,
    > {
        let mut conn = self.inner.db.acquire().await?;
        self.thread_shared_for_writing_in(&mut conn, thread, asked)
            .await
    }

    /// Rechecks the same resolver on an existing write connection, without another pool lease.
    pub(crate) async fn thread_shared_for_writing_in(
        &self,
        conn: &mut Conn,
        thread: &ThreadRow,
        asked: &str,
    ) -> Result<
        Result<(orch_db::queries::threads::tools::TopicThread, String, Id), Refusal>,
        EngineError,
    > {
        // A known aside in this topic is not a message target, even though it is not a shared-tree sibling.
        if let Some(Named::Id(id)) = tree::parse_path(asked) {
            super::source::check(conn).await?;
            if orch_db::queries::threads::thread(conn, id)
                .await?
                .is_some_and(|row| row.topic_id == thread.topic_id && row.side_of.is_some())
            {
                return Ok(Err(Refusal::new(
                    Code::WrongKind,
                    "A side thread is not a message target.",
                )));
            }
        }
        let resolved = match self
            .resolve_write(conn, thread.node_id, asked, false)
            .await?
        {
            Ok(resolved) => resolved,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let id = match resolved.target {
            Target::Directory(Where::Place(id)) | Target::Conversation { thread: id } => id,
            _ => {
                return Ok(Err(Refusal::new(
                    Code::WrongKind,
                    "That path does not name a thread.",
                )));
            }
        };
        let Some(place) = resolved.tree.place(id) else {
            return Ok(Err(Refusal::new(
                Code::NotFound,
                "That thread is not in your reach.",
            )));
        };
        let Some(topic) = place.parent_id.filter(|_| place.kind == "thread") else {
            return Ok(Err(Refusal::new(
                Code::WrongKind,
                "That path does not name a thread.",
            )));
        };
        if place.side_of.is_some() {
            return Ok(Err(Refusal::new(
                Code::WrongKind,
                "A side thread is not a message target.",
            )));
        }
        if topic != thread.topic_id
            && !resolved
                .tree
                .conversation_of(id)
                .is_some_and(|(standing, _)| standing.writes())
        {
            return Ok(Err(Refusal::new(
                Code::NotFound,
                "That thread is not shared with you for writing. Reading a conversation alone does not allow sending messages.",
            )));
        }
        let path = resolved.tree.path_of_node(id).unwrap_or(resolved.path);
        let found = orch_db::queries::threads::tools::topic_threads(conn, topic)
            .await?
            .into_iter()
            .find(|row| row.node_id == id);
        Ok(found
            .map(|row| (row, path, topic))
            .ok_or_else(|| Refusal::new(Code::NotFound, "That thread is no longer there.")))
    }
}

/// The refusal for a path that names nothing the agent reaches, with the names that are there.
pub(crate) fn not_found(tree: &Tree, named: &Named, consumed: usize, asked: &str) -> Refusal {
    let (directory, names) = tree.closest(named, consumed, CLOSEST_NAMES);
    if names.is_empty() {
        return Refusal::new(
            Code::NotFound,
            format!(
                "There is nothing at \"{asked}\" that you reach, and nothing to list in {directory}. list_shared with \"/\" shows everything you can reach."
            ),
        );
    }
    Refusal::new(
        Code::NotFound,
        format!(
            "There is nothing at \"{asked}\" that you reach. In {directory} there are: {}.",
            names
                .iter()
                .map(|name| format!("\"{name}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    )
}

/// Gives each readable document of a listing that nobody described the first line of its first paragraph, as
/// every reader of a document is given it then: read from the blocks as they stand, in one statement for the
/// whole listing.
pub(crate) async fn describe_documents(
    conn: &mut Conn,
    entries: &mut [Entry],
) -> Result<(), EngineError> {
    let undescribed = |entry: &Entry| {
        entry.kind == SharedEntryKind::Document
            && entry.readable
            && !entry.deleted
            && entry.description.is_none()
    };
    let wanted: Vec<Id> = entries
        .iter()
        .filter(|entry| undescribed(entry))
        .map(|entry| entry.id)
        .collect();
    if wanted.is_empty() {
        return Ok(());
    }
    // As much of a paragraph is read as a description may hold: its first line is found within it.
    let read = i32::try_from(orch_api_contract::documents::DOCUMENT_DESCRIPTION_MAX_CHARS)
        .unwrap_or(i32::MAX);
    let paragraphs = Box::pin(orch_db::queries::documents::first_paragraphs(
        conn, &wanted, read,
    ))
    .await?;
    for entry in entries.iter_mut().filter(|entry| undescribed(entry)) {
        entry.description = paragraphs
            .get(&entry.id)
            .map(|text| tree::opening(text, tree::DESCRIPTION_CHARS))
            .filter(|text| !text.is_empty());
    }
    Ok(())
}

/// The display names of whoever shared the entries, by identifier.
pub(crate) async fn names_of(
    conn: &mut Conn,
    entries: &[Entry],
) -> Result<HashMap<Id, String>, EngineError> {
    let mut people: Vec<Id> = entries.iter().filter_map(|entry| entry.shared_by).collect();
    people.sort_unstable();
    people.dedup();
    if people.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(identity::users_brief(conn, &people)
        .await?
        .into_iter()
        .map(|person| (person.id, person.display_name))
        .collect())
}

fn word<T: serde::Serialize>(value: &T) -> Json {
    serde_json::to_value(value).unwrap_or(Json::Null)
}

/// One entry as the tool gives it: what is not known is left out.
fn entry_json(entry: &Entry, sharers: &HashMap<Id, String>, ids: bool) -> Json {
    let mut out = serde_json::Map::new();
    out.insert("path".to_owned(), json!(entry.path));
    out.insert("name".to_owned(), json!(entry.name));
    out.insert("kind".to_owned(), word(&entry.kind));
    if ids {
        out.insert("id".to_owned(), json!(entry.id));
    }
    if let Some(description) = &entry.description {
        out.insert("description".to_owned(), json!(description));
    }
    if let Some(unit) = &entry.unit {
        out.insert("unit".to_owned(), word(unit));
    }
    if let Some(parts) = entry.parts {
        out.insert("parts".to_owned(), json!(parts));
    }
    if let Some(size) = entry.size {
        out.insert("size".to_owned(), json!(size));
    }
    if let Some(at) = &entry.updated_at {
        out.insert("updated_at".to_owned(), word(at));
    }
    if let Some(state) = &entry.state {
        out.insert("state".to_owned(), json!(state));
    }
    if let Some(status) = &entry.status {
        out.insert("status".to_owned(), json!(status));
    }
    if entry.archived {
        out.insert("archived".to_owned(), json!(true));
    }
    if let Some(agent) = &entry.agent {
        out.insert("agent".to_owned(), json!(agent));
    }
    if let Some(environment) = &entry.environment {
        out.insert("environment".to_owned(), json!(environment));
    }
    if let Some(directory) = &entry.directory {
        out.insert("directory".to_owned(), json!(directory));
    }
    if entry.is_self {
        out.insert("is_self".to_owned(), json!(true));
    }
    if let Some(children) = entry.children {
        out.insert("children".to_owned(), json!(children));
    }
    out.insert("readable".to_owned(), json!(entry.readable));
    out.insert("writable".to_owned(), json!(entry.writable));
    // What `manage_shared` may do is said where it may do it, and a document that was deleted says so.
    if entry.manageable {
        out.insert("manageable".to_owned(), json!(true));
    }
    if entry.deleted {
        out.insert("deleted".to_owned(), json!(true));
    }
    out.insert("via".to_owned(), word(&entry.via));
    if let Some(by) = entry.shared_by.and_then(|id| sharers.get(&id)) {
        out.insert("shared_by".to_owned(), json!(by));
    }
    Json::Object(out)
}

/// A listing as the tool gives it.
fn listing_json(listing: &Listing, sharers: &HashMap<Id, String>, ids: bool) -> Json {
    let about = listing.about.as_ref().map(|about| {
        let mut out = serde_json::Map::new();
        out.insert("kind".to_owned(), word(&about.kind));
        out.insert("name".to_owned(), json!(about.name));
        if let Some(description) = &about.description {
            out.insert("description".to_owned(), json!(description));
        }
        if let Some(status) = &about.status {
            out.insert("status".to_owned(), json!(status));
        }
        if !about.tags.is_empty() {
            out.insert("tags".to_owned(), json!(about.tags));
        }
        Json::Object(out)
    });
    json!({
        "path": listing.path,
        "about": about,
        "entries": listing.entries.iter().map(|entry| entry_json(entry, sharers, ids)).collect::<Vec<_>>(),
        "truncated": listing.truncated,
    })
}

/// Whether an entry is a place a person is shown.
#[must_use]
pub const fn is_place(entry: &Entry) -> bool {
    matches!(entry.at, Where::Place(_))
}

/// The scope a path was read under, for a caller that wants to say so.
#[must_use]
pub const fn everything(scope: Scope) -> bool {
    matches!(scope, Scope::Everything)
}
