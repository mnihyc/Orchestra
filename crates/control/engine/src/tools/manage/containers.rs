//! Agent path/answer adapter for the one transactional document-tree domain.

use super::*;
use crate::document_tree as domain;
use crate::engine::lock;
use crate::tools::{
    CallContext,
    shared::Target,
    tree::{FoundEntry, Tree, Where},
};
use crate::{Engine, EngineError};
use orch_authz::reach::{Reach, Things};
use orch_db::queries::{documents, threads::ThreadRow};
use orch_db::{Conn, DbError};
use orch_types::Time;
use serde_json::json;

/// Domain refusals are already name-free. Native failures use the engine's normal safe error path.
pub(crate) fn domain_result<T>(
    result: Result<T, domain::Error>,
    command: domain::Command,
) -> Result<Result<T, Refusal>, EngineError> {
    Ok(match result {
        Ok(value) => Ok(value),
        Err(domain::Error::Invalid(message)) => Err(Refusal::invalid(message)),
        Err(domain::Error::NotFound) => Err(Refusal::new(
            Code::NotFound,
            "The requested document entry or parent is not available.",
        )),
        Err(domain::Error::Denied) => {
            let manages = matches!(
                command,
                domain::Command::Delete
                    | domain::Command::Move
                    | domain::Command::Restore
                    | domain::Command::RestoreVersion
            );
            Err(Refusal::new(
                if manages {
                    Code::NotManageable
                } else {
                    Code::NotWritable
                },
                if manages {
                    "This operation needs manage access to every selected document and folder. Nothing was changed; ask a person who manages them to share that access."
                } else {
                    "The complete operation needs access to its sources and edit access at its destination. Nothing was changed; ask a person who manages them to share that access."
                },
            ))
        }
        Err(domain::Error::Conflict) => Err(Refusal::new(
            Code::Exists,
            "An entry conflicts with this operation. Nothing was changed; choose another name or destination.",
        )),
        Err(domain::Error::Stale) => Err(Refusal::new(
            Code::Changed,
            "The document tree or its access changed. Nothing was changed; read its current entries and try again.",
        )),
        Err(domain::Error::Limit) => Err(Refusal::new(
            Code::Limit,
            format!(
                "The complete tree or its material exceeds the operation limits (at most {AFFECTS_MAX} documents and folders). Nothing was changed."
            ),
        )),
        Err(domain::Error::Storage(error)) => return Err(EngineError::Db(error)),
        Err(domain::Error::Authorization(error)) => return Err(error.into()),
        Err(domain::Error::Content(error)) => return Err(error.into()),
        Err(domain::Error::Engine(error)) => return Err(error),
    })
}

struct LocatedRequest {
    request: domain::Request,
    parents: HashMap<domain::Directory, String>,
    sources: HashMap<domain::Item, String>,
}

type DocumentMetadata = (HashMap<Id, documents::DocumentRow>, HashMap<Id, i64>);

fn directory(at: Where) -> Option<domain::Directory> {
    match at {
        Where::Docs(node) => Some(domain::Directory::NodeDocs(node)),
        Where::DocumentFolder(id) => Some(domain::Directory::Container(id)),
        _ => None,
    }
}

fn leaf_parent(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit_once('/')
        .map_or_else(|| "/".to_owned(), |(parent, _)| format!("{parent}/"))
}

fn wrong_source(at: FoundEntry, tree: &Tree) -> Refusal {
    let message = match at {
        FoundEntry::Directory(Where::Docs(_)) => {
            "The built-in docs directory is not a mutable object. Select its documents or a real document folder."
        }
        FoundEntry::Directory(
            Where::DocumentFiles(_) | Where::ChatFiles(_) | Where::History(_),
        ) => {
            "Built-in attachment and history directories are not mutable objects. Select a document or a real document folder."
        }
        FoundEntry::File(Where::DocumentFile { .. } | Where::ChatFile { .. }) => {
            "This path names an attachment, not a document or document folder. To change a document's files, name its document in edit_document attach or detach."
        }
        FoundEntry::File(Where::Version(_, _)) => {
            "An earlier document version can only be restored on its own. Select the current document for other operations."
        }
        FoundEntry::File(Where::Conversation(_)) => {
            "Conversations are managed by people. Select a document or document folder."
        }
        FoundEntry::File(Where::NodeChat(_) | Where::PeopleChat(_))
        | FoundEntry::Directory(Where::Messages(_) | Where::PeopleChatDir(_)) => {
            "Chats are managed by people. Select a document or document folder."
        }
        FoundEntry::Directory(Where::Place(id))
            if tree.place(id).is_some_and(|place| place.kind == "thread") =>
        {
            "Threads are managed by people. Select a document or document folder."
        }
        FoundEntry::Directory(Where::Place(_)) => {
            "Places are managed by people. Select a document or document folder."
        }
        _ => {
            "Select a document or document folder; this kind of shared entry cannot be organized by manage_shared."
        }
    };
    Refusal::new(Code::WrongKind, message)
}

impl Engine {
    async fn document_request(
        &self,
        conn: &mut Conn,
        thread: &ThreadRow,
        asked: &Asked,
        call_ref: &str,
    ) -> Result<Result<LocatedRequest, Refusal>, EngineError> {
        crate::tools::source::check(conn).await?;
        let mut request = domain::Request {
            command: match asked.command {
                Command::CreateFolder => domain::Command::CreateFolder,
                Command::Copy => domain::Command::Copy,
                Command::Move => domain::Command::Move,
                Command::Delete => domain::Command::Delete,
                Command::Restore => domain::Command::Restore,
            },
            items: Vec::new(),
            to: None,
            name: asked.name.clone(),
            request_ref: call_ref.to_owned(),
            version: None,
        };
        let mut parents = HashMap::new();
        let mut sources = HashMap::new();
        let destination = if asked.command == Command::CreateFolder {
            let path = asked.paths[0].trim_end_matches('/');
            let Some((parent, name)) = path.rsplit_once('/') else {
                return Ok(Err(Refusal::invalid(
                    "A new folder path must name its existing document parent, for example docs/research/.",
                )));
            };
            request.name = Some(name.to_owned());
            Some(if parent.is_empty() {
                "/".to_owned()
            } else {
                format!("{parent}/")
            })
        } else {
            asked.to.clone()
        };
        if let Some(destination) = destination {
            let resolved = match self
                .resolve_write(conn, thread.node_id, &destination, false)
                .await?
            {
                Ok(resolved) => resolved,
                Err(refusal) => return Ok(Err(refusal)),
            };
            let Target::Directory(at) = resolved.target else {
                return Ok(Err(Refusal::new(
                    Code::WrongKind,
                    "The destination must be an existing writable document directory.",
                )));
            };
            let Some(at) = directory(at) else {
                return Ok(Err(Refusal::new(
                    Code::WrongKind,
                    "The destination must be a docs directory or a document folder, not a place, attachment or history directory.",
                )));
            };
            let writable = match at {
                domain::Directory::NodeDocs(node) => {
                    resolved.tree.documents_standing(node).writes()
                }
                domain::Directory::Container(id) => resolved.tree.folder_standing(id).write,
            };
            let node = resolved.tree.directory_node(at).ok_or_else(|| {
                EngineError::unreadable("a resolved document directory has no hierarchy anchor")
            })?;
            if resolved.tree.reach().archived(node) {
                return Ok(Err(Refusal::new(
                    Code::NotWritable,
                    "The destination is archived. Choose a live writable document directory.",
                )));
            }
            if !writable {
                return Ok(Err(Refusal::new(
                    Code::NotWritable,
                    "The destination needs edit access (doc.edit) to create documents or folders there. Ask a person who manages it to share that access, or choose a writable directory.",
                )));
            }
            if !orch_settings::get::<bool>(
                conn,
                orch_settings::Place::Node(node),
                orch_settings_contract::registry::keys::DOCS_ENABLED,
            )
            .await?
            .unwrap_or(true)
            {
                return Ok(Err(Refusal::new(
                    Code::Limit,
                    "Documents are switched off at the destination (docs.enabled). Ask a person who manages its settings to enable them, or choose another directory.",
                )));
            }
            parents.insert(at, resolved.path);
            request.to = Some(at);
        }
        if asked.command != Command::CreateFolder {
            for path in &asked.paths {
                let located = match self
                    .locate_shared(conn, thread.node_id, path, true, false)
                    .await?
                {
                    Ok(located) => located,
                    Err(refusal) => return Ok(Err(refusal)),
                };
                let item = match located.found.entry {
                    FoundEntry::File(Where::Document(id)) => domain::Item::Document(id),
                    FoundEntry::Directory(Where::DocumentFolder(id)) => domain::Item::Container(id),
                    FoundEntry::File(Where::Version(id, version))
                        if asked.command == Command::Restore && asked.paths.len() == 1 =>
                    {
                        request.command = domain::Command::RestoreVersion;
                        request.version = Some(version);
                        domain::Item::Document(id)
                    }
                    other => return Ok(Err(wrong_source(other, &located.tree))),
                };
                let found = if let Some(found) = located.tree.organized_entry(item) {
                    found.clone()
                } else {
                    match domain_result(
                        domain::entry_in(
                            conn,
                            domain::Actor::Agent {
                                thread: thread.node_id,
                            },
                            item,
                            Time::new(self.inner.clock.now()),
                        )
                        .await,
                        request.command,
                    )? {
                        Ok(found) => found,
                        Err(refusal) => return Ok(Err(refusal)),
                    }
                };
                if request.command != domain::Command::Copy
                    && located.tree.reach().archived(found.entry.node)
                {
                    return Ok(Err(Refusal::new(
                        Code::NotWritable,
                        "That document's place is archived. Its documents can be read, but cannot be moved, deleted or restored there.",
                    )));
                }
                if request.command == domain::Command::Move
                    && request.to == Some(found.entry.parent)
                    && request
                        .name
                        .as_deref()
                        .is_none_or(|name| name == found.entry.basename.as_str())
                {
                    return Ok(Err(Refusal::invalid(
                        "That entry is already in the destination under that name. Choose another directory or give one source a different name.",
                    )));
                }
                let path = if request.command == domain::Command::RestoreVersion {
                    located.tree.document_path(item.id()).ok_or_else(|| {
                        EngineError::unreadable("a readable document's path disappeared")
                    })?
                } else {
                    located.found.path.clone()
                };
                parents.insert(found.entry.parent, leaf_parent(&path));
                if let domain::Item::Container(id) = item {
                    parents.insert(domain::Directory::Container(id), path.clone());
                }
                sources.insert(item, located.found.path);
                request.items.push(item);
            }
        }
        Ok(Ok(LocatedRequest {
            request,
            parents,
            sources,
        }))
    }

    /// The tool owns paths and deletion capabilities; every mutation belongs to the shared domain.
    pub(crate) async fn manage_shared_call(
        &self,
        call: &CallContext,
        thread: &ThreadRow,
    ) -> Result<Answer, EngineError> {
        let (asked, confirm) = match asked_of(&call.arguments) {
            Ok(asked) => asked,
            Err(refusal) => return Ok(refusal.into()),
        };
        if let Some(answer) = lock(&self.inner.manage_plans).replay(
            call.thread,
            &call.call_ref,
            &asked,
            self.inner.clock.now(),
        ) {
            return Ok(answer);
        }
        let answer = self
            .manage_document_tree(call, thread, &asked, confirm.as_deref())
            .await?;
        lock(&self.inner.manage_plans).remember(
            call.thread,
            &call.call_ref,
            &asked,
            self.inner.clock.now(),
            &answer,
        );
        Ok(answer)
    }

    async fn manage_document_tree(
        &self,
        call: &CallContext,
        thread: &ThreadRow,
        asked: &Asked,
        confirm: Option<&str>,
    ) -> Result<Answer, EngineError> {
        use orch_db::queries::transaction_guard::{Scope, request_scope};
        let scope = match (asked.command, confirm.is_some()) {
            (Command::CreateFolder | Command::Copy, _) => Scope::Creation,
            (Command::Delete, false) => Scope::Shared,
            (Command::Restore, _) => Scope::Shared,
            _ => Scope::Authority,
        };
        let mut tx = request_scope(scope, self.inner.db.begin()).await?;
        let mut located = match self
            .document_request(&mut tx, thread, asked, &call.call_ref)
            .await?
        {
            Ok(located) => located,
            Err(_) if asked.command == Command::Delete && confirm.is_some() => {
                return Ok(not_confirmed(
                    "The current selection or its access differs from the deletion preview.",
                ));
            }
            Err(refusal) => return Ok(refusal.into()),
        };
        // Restoring content keeps the ordinary shared gate; undeleting changes reach and uses
        // authority. Resolve the typed command first, release the read transaction, then acquire
        // the actual scope and resolve again. Never upgrade a held shared barrier in place.
        if located.request.command == domain::Command::Restore {
            drop(tx);
            tx = request_scope(Scope::Authority, self.inner.db.begin()).await?;
            located = match self
                .document_request(&mut tx, thread, asked, &call.call_ref)
                .await?
            {
                Ok(located) => located,
                Err(refusal) => return Ok(refusal.into()),
            };
        }
        let actor = domain::Actor::Agent {
            thread: thread.node_id,
        };
        let options = domain::Options::for_engine(self);
        let mut deletion = None;
        if asked.command == Command::Delete {
            let plan = match domain_result(
                domain::plan_delete_in(
                    &mut tx,
                    actor,
                    &located.request,
                    Time::new(self.inner.clock.now()),
                    options,
                )
                .await,
                domain::Command::Delete,
            )? {
                Ok(plan) => plan,
                Err(refusal) if confirm.is_some() => return Ok(not_confirmed(&refusal.message)),
                Err(refusal) => return Ok(refusal.into()),
            };
            let preview = self
                .document_preview(
                    &mut tx,
                    thread,
                    &located,
                    &plan,
                    "00000000000000000000000000000000",
                )
                .await?;
            if !orch_agent_contract::tool_answers::fits("manage_shared", &call.arguments, &preview)
            {
                return Ok(Refusal::new(Code::TooLarge,
                    "The complete management plan does not fit max_bytes. No confirmation word was issued or spent. Request a larger bound or select fewer paths, then read the complete plan without confirm.").into());
            }
            let Some(word) = confirm else {
                let word = new_word();
                let mut preview = preview;
                preview["confirm"] = json!(word);
                lock(&self.inner.manage_plans).keep(
                    thread.node_id,
                    &word,
                    Waiting {
                        made: self.inner.clock.now(),
                        asked: asked.clone(),
                        stamp: Some(plan.stamp),
                    },
                    self.inner.clock.now(),
                );
                // The preview is read-only. Dropping its transaction releases scope locks before answering.
                return Ok(Answer::Done(preview));
            };
            let waiting = match lock(&self.inner.manage_plans).claim(
                thread.node_id,
                word,
                asked,
                self.inner.clock.now(),
            ) {
                Claim::Claimed(waiting) => waiting,
                Claim::Missing => {
                    return Ok(not_confirmed(
                        "That word is missing, expired, made up, used already or belongs to another call.",
                    ));
                }
                Claim::Other => {
                    return Ok(not_confirmed("That word was given for other arguments."));
                }
            };
            if waiting.stamp.as_ref() != Some(&plan.stamp) {
                return Ok(not_confirmed(
                    "The complete tree, material or access changed since the preview.",
                ));
            }
            deletion = waiting.stamp;
        }
        let receipt = match domain_result(
            domain::execute_in(
                &mut tx,
                actor,
                &located.request,
                deletion.as_ref(),
                Time::new(self.inner.clock.now()),
                options,
            )
            .await,
            located.request.command,
        )? {
            Ok(receipt) => receipt,
            Err(refusal) if asked.command == Command::Delete => {
                return Ok(not_confirmed(&refusal.message));
            }
            Err(refusal) => return Ok(refusal.into()),
        };
        let answer = self
            .document_receipt(&mut tx, thread, &located, &receipt)
            .await?;
        if !orch_agent_contract::tool_answers::fits("manage_shared", &call.arguments, &answer) {
            return Ok(Refusal::new(Code::TooLarge, "The complete operation result does not fit max_bytes. Nothing was changed. Request a larger bound or select fewer paths.").into());
        }
        tx.commit().await.map_err(DbError::from)?;
        Ok(Answer::Done(answer))
    }

    async fn document_metadata(
        &self,
        conn: &mut Conn,
        thread: &ThreadRow,
        entries: &[domain::Entry],
    ) -> Result<DocumentMetadata, EngineError> {
        let ids: Vec<_> = entries
            .iter()
            .filter_map(|entry| match entry.item {
                domain::Item::Document(id) => Some(id),
                _ => None,
            })
            .collect();
        let rows = documents::get_many(conn, &ids).await?;
        let Some(asking) = self.asking(conn, thread.node_id).await? else {
            return Err(EngineError::unreadable("the calling thread is gone"));
        };
        let things = Things {
            nodes: entries.iter().map(|entry| entry.node).collect(),
            documents: rows
                .iter()
                .map(orch_authz::documents::Document::of_row)
                .collect(),
            people_chats: Vec::new(),
        };
        let reach = Reach::load(
            conn,
            asking.agent.clone(),
            asking.policy,
            asking.now,
            &things,
        )
        .await?
        .with_document_policy(asking.documents_policy);
        let counts = rows
            .iter()
            .map(|row| {
                let count = reach
                    .shared_with(&orch_authz::documents::Document::of_row(row))
                    .into_iter()
                    .filter(|place| !asking.agent.path.contains(place))
                    .count();
                (row.id, i64::try_from(count).unwrap_or(i64::MAX))
            })
            .collect();
        Ok((rows.into_iter().map(|row| (row.id, row)).collect(), counts))
    }

    async fn document_preview(
        &self,
        conn: &mut Conn,
        thread: &ThreadRow,
        located: &LocatedRequest,
        plan: &domain::Plan,
        word: &str,
    ) -> Result<Json, EngineError> {
        let (rows, shared) = self
            .document_metadata(conn, thread, &plan.stamp.entries)
            .await?;
        let all: HashMap<_, _> = plan
            .stamp
            .entries
            .iter()
            .map(|entry| (entry.item, entry))
            .collect();
        let mut affected = Vec::new();
        for entry in &plan.stamp.entries {
            let path =
                entry_path(entry, &located.parents, &all, &mut Vec::new()).ok_or_else(|| {
                    EngineError::unreadable("an authorized document tree has no parent path")
                })?;
            let until = domain::restore_deadline_in(conn, entry, domain::Options::for_engine(self))
                .await
                .map_err(EngineError::unreadable)?
                .unwrap_or(plan.restore_until);
            affected.push(json!({ "path":path,"name":entry.basename,"id":entry.item.id(),"kind":kind(entry.item),
                "action":"delete","revision":entry.revision,"blocks":rows.get(&entry.item.id()).map_or(0,|row|row.block_count),
                "files":entry.files,"versions":entry.versions,"open_by":self.inner.live.document_readers(entry.item.id()),
                "shared_with":shared.get(&entry.item.id()).copied().unwrap_or(0),"until":Time::new(until) }));
        }
        Ok(
            json!({"done":false,"confirm":word,"expires_in":self.inner.config.manage_plan_kept.as_secs(),
            "affects":affected,"refused":[],"gaining":plan.gaining,"losing":plan.losing}),
        )
    }

    async fn document_receipt(
        &self,
        conn: &mut Conn,
        thread: &ThreadRow,
        located: &LocatedRequest,
        receipt: &domain::Receipt,
    ) -> Result<Json, EngineError> {
        let (rows, shared) = self
            .document_metadata(conn, thread, &receipt.entries)
            .await?;
        let all: HashMap<_, _> = receipt
            .entries
            .iter()
            .map(|entry| (entry.item, entry))
            .collect();
        let originals: HashMap<_, _> = receipt
            .copied
            .iter()
            .map(|pair| (pair.destination, pair.source))
            .collect();
        let mut results = Vec::new();
        for entry in &receipt.entries {
            let new_path = entry_path(entry, &located.parents, &all, &mut Vec::new())
                .unwrap_or_else(|| format!("id:{}", entry.item.id()));
            let source = originals.get(&entry.item).copied().unwrap_or(entry.item);
            let old_path = located
                .sources
                .get(&source)
                .cloned()
                .or_else(|| {
                    located
                        .sources
                        .iter()
                        .filter(|(root, _)| matches!(root, domain::Item::Container(_)))
                        .find_map(|(root, path)| {
                            let current_root = receipt
                                .copied
                                .iter()
                                .find(|pair| pair.source == *root)
                                .map_or(*root, |pair| pair.destination);
                            let root_entry = all.get(&current_root)?;
                            let current_path =
                                entry_path(root_entry, &located.parents, &all, &mut Vec::new())?;
                            new_path
                                .strip_prefix(&current_path)
                                .map(|tail| format!("{path}{tail}"))
                        })
                })
                .unwrap_or_else(|| new_path.clone());
            let mut result = json!({"id":entry.item.id(),"kind":kind(entry.item),"path":old_path,"name":entry.basename,
                "action":if located.request.command==domain::Command::RestoreVersion {"put_back"} else {match located.request.command {
                    domain::Command::CreateFolder=>"create_folder",domain::Command::Copy=>"copy",domain::Command::Move=>"move",
                    domain::Command::Delete=>"delete",domain::Command::Restore=>"restore",domain::Command::Rename=>"rename",domain::Command::RestoreVersion=>"put_back"}},
                "blocks":rows.get(&entry.item.id()).map_or(0,|row|row.block_count),"files":entry.files,"versions":entry.versions,
                "shared_with":shared.get(&entry.item.id()).copied().unwrap_or(0)});
            if located.request.command != domain::Command::Delete {
                result["new_path"] = json!(new_path);
            }
            if let Some(to) = located.request.to.and_then(|to| located.parents.get(&to)) {
                result["to"] = json!(to);
            }
            if let Some(until) = receipt.restore_until {
                result["until"] = json!(Time::new(until));
            }
            if let Some(source) = originals.get(&entry.item) {
                result["source_id"] = json!(source.id());
            }
            if receipt.entries.len() == 1 {
                result["gaining"] = json!(receipt.gaining);
                result["losing"] = json!(receipt.losing);
            }
            results.push(result);
        }
        Ok(
            json!({"done":true,"operation":receipt.operation,"results":results,"copied":receipt.copied,
            "gaining":receipt.gaining,"losing":receipt.losing}),
        )
    }

    /// Private compatibility entry: old create(from) calls use the exact same Copy operation.
    pub(crate) async fn copy_document_for_edit(
        &self,
        thread: &ThreadRow,
        call: &CallContext,
        source: &str,
        path: &str,
    ) -> Result<Result<(documents::DocumentRow, String), Refusal>, EngineError> {
        let Some((to, name)) = path.trim().rsplit_once('/') else {
            return Ok(Err(Refusal::invalid(
                "The copy path must name an existing document parent and a .md basename.",
            )));
        };
        if !name.ends_with(".md") {
            return Ok(Err(Refusal::invalid("A copied document path ends in .md.")));
        }
        let asked = Asked {
            command: Command::Copy,
            paths: vec![source.to_owned()],
            to: Some(format!("{to}/")),
            name: Some(name.to_owned()),
        };
        let mut tx = orch_db::queries::transaction_guard::request_scope(
            orch_db::queries::transaction_guard::Scope::Creation,
            self.inner.db.begin(),
        )
        .await?;
        let located = match self
            .document_request(&mut tx, thread, &asked, &call.call_ref)
            .await?
        {
            Ok(located) => located,
            Err(refusal) => return Ok(Err(refusal)),
        };
        if located.request.items.len() != 1
            || !matches!(located.request.items[0], domain::Item::Document(_))
        {
            return Ok(Err(Refusal::new(
                Code::WrongKind,
                "from must name one current readable document; use manage_shared copy for a folder tree.",
            )));
        }
        let receipt = match domain_result(
            domain::execute_in(
                &mut tx,
                domain::Actor::Agent {
                    thread: thread.node_id,
                },
                &located.request,
                None,
                Time::new(self.inner.clock.now()),
                domain::Options::for_engine(self),
            )
            .await,
            domain::Command::Copy,
        )? {
            Ok(receipt) => receipt,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let source = located.request.items[0];
        let Some(id) = receipt
            .copied
            .iter()
            .find(|pair| pair.source == source)
            .map(|pair| pair.destination.id())
        else {
            return Err(EngineError::unreadable(
                "a committed document copy has no fresh identity",
            ));
        };
        let row = documents::get(&mut tx, id)
            .await?
            .ok_or_else(|| EngineError::unreadable("a fresh document copy disappeared"))?;
        let all: HashMap<_, _> = receipt
            .entries
            .iter()
            .map(|entry| (entry.item, entry))
            .collect();
        let entry = all
            .get(&domain::Item::Document(id))
            .ok_or_else(|| EngineError::unreadable("the copied document entry disappeared"))?;
        let path = entry_path(entry, &located.parents, &all, &mut Vec::new())
            .ok_or_else(|| EngineError::unreadable("the copied document path disappeared"))?;
        let answer = json!({"path":path,"id":id,"created":true,"seq":row.doc_seq,"version":row.version,
            "blocks":row.block_count,"files":receipt.entries.iter().find(|entry|entry.item.id()==id).map_or(0,|entry|entry.files)});
        if !orch_agent_contract::tool_answers::fits("edit_document", &call.arguments, &answer) {
            return Ok(Err(Refusal::new(
                Code::TooLarge,
                "The copied document's complete outcome does not fit max_bytes. Nothing was changed; use a shorter path or a larger bound.",
            )));
        }
        tx.commit().await.map_err(DbError::from)?;
        Ok(Ok((row, path)))
    }

    /// Text-title changes share the common namespace/scope producer, before any content-row lock.
    pub(crate) async fn rename_document_for_edit(
        &self,
        thread: &ThreadRow,
        call: &CallContext,
        path: &str,
        title: &str,
    ) -> Result<Result<(documents::DocumentRow, String), Refusal>, EngineError> {
        let mut tx = orch_db::queries::transaction_guard::request_scope(
            orch_db::queries::transaction_guard::Scope::Shared,
            self.inner.db.begin(),
        )
        .await?;
        let resolved = match self
            .resolve_write(&mut tx, thread.node_id, path, false)
            .await?
        {
            Ok(resolved) => resolved,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let Target::Document { document } = resolved.target else {
            return Ok(Err(Refusal::new(
                Code::WrongKind,
                "Rename names one current document; use manage_shared move with name for a folder.",
            )));
        };
        let request = domain::Request {
            command: domain::Command::Rename,
            items: vec![domain::Item::Document(document)],
            to: None,
            name: Some(title.to_owned()),
            request_ref: call.call_ref.clone(),
            version: None,
        };
        let receipt = match domain_result(
            domain::execute_in(
                &mut tx,
                domain::Actor::Agent {
                    thread: thread.node_id,
                },
                &request,
                None,
                Time::new(self.inner.clock.now()),
                domain::Options::for_engine(self),
            )
            .await,
            domain::Command::Rename,
        )? {
            Ok(receipt) => receipt,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let entry = receipt
            .entries
            .iter()
            .find(|entry| entry.item == domain::Item::Document(document))
            .ok_or_else(|| EngineError::unreadable("a renamed document has no current entry"))?;
        let path = format!("{}{}", leaf_parent(&resolved.path), entry.basename);
        let row = documents::get(&mut tx, document)
            .await?
            .ok_or_else(|| EngineError::unreadable("a renamed document disappeared"))?;
        let answer = json!({"path":path,"id":row.id,"created":false,"seq":row.doc_seq,"version":row.version,
            "blocks":row.block_count,"files":entry.files});
        if !orch_agent_contract::tool_answers::fits("edit_document", &call.arguments, &answer) {
            return Ok(Err(Refusal::new(
                Code::TooLarge,
                "The complete rename outcome does not fit max_bytes. Nothing was changed; use a shorter title or a larger bound.",
            )));
        }
        tx.commit().await.map_err(DbError::from)?;
        Ok(Ok((row, path)))
    }
}

fn kind(item: domain::Item) -> &'static str {
    match item {
        domain::Item::Document(_) => "document",
        domain::Item::Container(_) => "folder",
    }
}

fn entry_path(
    entry: &domain::Entry,
    parents: &HashMap<domain::Directory, String>,
    all: &HashMap<domain::Item, &domain::Entry>,
    seen: &mut Vec<Id>,
) -> Option<String> {
    if seen.contains(&entry.item.id()) {
        return None;
    }
    seen.push(entry.item.id());
    let parent = match entry.parent {
        domain::Directory::Container(id) if all.contains_key(&domain::Item::Container(id)) => {
            entry_path(all.get(&domain::Item::Container(id))?, parents, all, seen)?
        }
        other => parents.get(&other)?.clone(),
    };
    Some(format!(
        "{parent}{}{}",
        entry.basename,
        if matches!(entry.item, domain::Item::Container(_)) {
            "/"
        } else {
            ""
        }
    ))
}

fn not_confirmed(reason: &str) -> Answer {
    Refusal::new(Code::NotConfirmed,format!("{reason} Nothing was done. Call manage_shared again without confirm to read the complete current deletion preview; confirm only the new plan you read.")).into()
}
